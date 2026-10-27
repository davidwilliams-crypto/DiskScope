//! Confirmed single-file actions: normal-token Shell recycling and explicit
//! permanent deletion by a freshly verified native handle.
use crate::model::FileIdentity;
pub use crate::recycle_policy::{RecycleBlock, RecycleVerdict};
use std::{
    fmt,
    num::NonZeroIsize,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    Unsupported,
    InvalidPath,
    IdentityUnavailable,
    Changed,
    NotRegularFile,
    PermissionDenied,
    Busy,
    ElevatedRecycleUnavailable,
    Cancelled,
    /// Recycling was refused before it could delete anything: Windows would
    /// have deleted the file permanently instead of using the Recycle Bin.
    RecycleUnavailable(RecycleBlock),
    /// Windows reported success but kept no Recycle Bin copy and the file is gone.
    DeletedNotRecycled,
    /// The file is gone or unreadable and no Recycle Bin copy was confirmed.
    RecycleOutcomeUnconfirmed,
    Failed(String),
}

impl ActionError {
    /// Whether the file may no longer be at its original path. The caller must
    /// rescan for these, so a list never keeps a row for a deleted file. The
    /// match is exhaustive on purpose: a new variant must be classified.
    pub fn file_may_be_gone(&self) -> bool {
        match self {
            Self::DeletedNotRecycled | Self::RecycleOutcomeUnconfirmed => true,
            Self::Unsupported
            | Self::InvalidPath
            | Self::IdentityUnavailable
            | Self::Changed
            | Self::NotRegularFile
            | Self::PermissionDenied
            | Self::Busy
            | Self::ElevatedRecycleUnavailable
            | Self::Cancelled
            | Self::RecycleUnavailable(_)
            | Self::Failed(_) => false,
        }
    }
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unsupported => "This file action is not supported on this system.",
            Self::InvalidPath => {
                "Select a regular file on a local drive. Network, device, and alternate-stream paths are not supported."
            }
            Self::IdentityUnavailable => {
                "The scan did not provide a reliable file identity. Rescan before trying again."
            }
            Self::Changed => {
                "The file changed or moved since it was selected. Rescan and confirm the current file."
            }
            Self::NotRegularFile => {
                "This action supports individual regular files only. Folders, links, and paths through junctions are excluded."
            }
            Self::PermissionDenied => {
                "Windows denied this action. File permissions and read-only attributes were not changed."
            }
            Self::Busy => "The file is in use. Close programs using it and try again.",
            Self::ElevatedRecycleUnavailable => {
                "Recycle Bin actions require a normal DiskScope window. Administrator mode offers separately confirmed permanent deletion."
            }
            Self::Cancelled => "The file action was cancelled.",
            Self::RecycleUnavailable(block) => {
                return write!(f, "{} Nothing was changed.", block.reason());
            }
            Self::DeletedNotRecycled => {
                "Windows deleted this file without keeping a copy in the Recycle Bin (it reported no Recycle Bin item), so it probably cannot be restored from the Recycle Bin. Check your backups if you need it back."
            }
            Self::RecycleOutcomeUnconfirmed => {
                "The file is no longer at its original location, or its state could not be read, and Windows did not confirm that it reached the Recycle Bin. Check the Recycle Bin; if it is not there, assume the file was deleted."
            }
            Self::Failed(message) => return f.write_str(message),
        };
        f.write_str(message)
    }
}
impl std::error::Error for ActionError {}

#[derive(Debug)]
pub struct RecyclePlan {
    path: PathBuf,
    size: u64,
    verdict: RecycleVerdict,
    #[cfg(windows)]
    snapshot: native::Snapshot,
}
impl RecyclePlan {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    /// The Recycle Bin preflight computed while the file was verified. It only
    /// matters to the Recycle action; permanent deletion ignores it.
    pub fn recycle_verdict(&self) -> RecycleVerdict {
        self.verdict
    }
}

#[derive(Debug)]
pub enum ActionOutcome {
    Recycled { original_path: PathBuf },
    DeletionRequested { original_path: PathBuf },
}

/// Performs read-only verification before the UI displays its confirmation.
pub fn prepare(
    path: &Path,
    expected_identity: &FileIdentity,
    expected_size: u64,
    expected_modified: Option<u64>,
) -> Result<RecyclePlan, ActionError> {
    #[cfg(windows)]
    {
        native::validate_path(path)?;
        if expected_identity.id == [0; 16] {
            return Err(ActionError::IdentityUnavailable);
        }
        let file = native::open_file(path, false)?;
        let snapshot = native::snapshot(&file)?;
        if &snapshot.identity != expected_identity
            || snapshot.size != expected_size
            || expected_modified.is_some_and(|time| Some(time) != snapshot.modified_seconds())
        {
            return Err(ActionError::Changed);
        }
        Ok(RecyclePlan {
            path: path.to_owned(),
            size: snapshot.size,
            verdict: native::recycle_verdict(path, snapshot.size),
            snapshot,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (path, expected_identity, expected_size, expected_modified);
        Err(ActionError::Unsupported)
    }
}

/// Consumes a user-confirmed plan. Run on a worker thread, never the UI thread.
///
/// `owner` is the main window's handle. It lets Windows parent any prompt of its
/// own to DiskScope so it cannot hide behind the window.
///
/// Windows decides whether a file is recycled, and it deletes files it cannot
/// recycle. So this refuses first when the preflight predicts that, vetoes in
/// the sink if Windows does not offer the Recycle Bin, and finally checks the
/// disk: `Ok` is returned only with a confirmed Recycle Bin destination.
pub fn recycle(
    plan: RecyclePlan,
    owner: Option<NonZeroIsize>,
) -> Result<ActionOutcome, ActionError> {
    #[cfg(windows)]
    {
        ensure_recycle_token(crate::admin::is_elevated().map_err(ActionError::Failed)?)?;
        let file = native::open_recycle_file(&plan.path)?;
        if native::snapshot(&file)? != plan.snapshot {
            return Err(ActionError::Changed);
        }
        // Authoritative gate: the settings may have changed since the dialog
        // opened (a drive removed, a Recycle Bin limit lowered).
        if let RecycleVerdict::Blocked(block) = native::recycle_verdict(&plan.path, plan.size) {
            return Err(ActionError::RecycleUnavailable(block));
        }
        let path = plan.path.clone();
        let approved = plan.snapshot.clone();
        // The verified file and its ancestor guards stay alive until Windows
        // has finished; they drop when shell::recycle returns, so the leaf is
        // not held open by this process while its presence is checked below.
        let report = shell::recycle(plan, file, owner)?;
        settle(report, native::presence(&path, &approved))?;
        Ok(ActionOutcome::Recycled {
            original_path: path,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (plan, owner);
        Err(ActionError::Unsupported)
    }
}

/// The text for an unclassified Shell failure. It must not suggest the file is
/// safe: the sink can fail after Windows has already acted.
#[cfg(any(windows, test))]
fn failed_text(code: u32) -> String {
    format!("Windows could not complete the Recycle Bin operation ({code:#010X}).")
}

/// `TSF_DELETE_RECYCLE_IF_POSSIBLE`. Reported (not yet verified on Windows) to
/// be cleared when the Shell already knows it will not recycle, for example a
/// volume set to remove files immediately, and to stay set for an oversized
/// file. So it can only veto, never prove a recycle.
#[cfg(any(windows, test))]
const TSF_DELETE_RECYCLE_IF_POSSIBLE_BIT: u32 = 0x80;

/// The sink's first look at the single queued delete.
#[cfg(any(windows, test))]
fn pre_delete_check(first: bool, flags: u32) -> Result<(), ActionError> {
    if !first {
        return Err(ActionError::Failed(
            "Windows queued more than the one approved file, so the action was stopped.".into(),
        ));
    }
    if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE_BIT == 0 {
        return Err(ActionError::RecycleUnavailable(RecycleBlock::ShellDeclined));
    }
    Ok(())
}

/// What the Shell sink observed, without any interpretation.
#[cfg(any(windows, test))]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ShellReport {
    pre_seen: bool,
    recycled: bool,
    /// `PostDeleteItem` succeeded with no Recycle Bin destination. Only a hint:
    /// the same shape can follow a skipped item that is still intact.
    no_destination: bool,
    error: Option<ActionError>,
    aborted: bool,
}

/// Whether the approved file is still at its original path after the Shell
/// returned. This is about the file, not the name: a different file at the
/// same path is `Absent`, and a name Windows will not let us open (a
/// delete-pending name, an access error) is `Unknown`.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// Opened, and its identity, size and time match the approved snapshot.
    Present,
    Absent,
    Unknown,
}

/// `observed` is the result of opening the path and comparing it with the
/// approved snapshot: `Ok(true)` for the same file, `Ok(false)` for another.
#[cfg(any(windows, test))]
fn presence_of(observed: &Result<bool, ActionError>) -> Presence {
    match observed {
        Ok(true) => Presence::Present,
        // Another file now lives at the path.
        Ok(false) => Presence::Absent,
        // `native::io_error` maps file or path not found to `Changed`.
        Err(ActionError::Changed) => Presence::Absent,
        Err(_) => Presence::Unknown,
    }
}

/// Turns the sink's observations and the disk's state into the truth. The disk
/// arbitrates: a Recycle Bin destination is the only proof of success, and no
/// error text is allowed to claim the file is safe unless it is present.
///
/// Invariants (tested exhaustively): `Ok` implies a confirmed Recycle Bin
/// destination, and any error that does not say the file may be gone implies
/// the file was observed present.
#[cfg(any(windows, test))]
fn settle(report: ShellReport, presence: Presence) -> Result<(), ActionError> {
    if report.pre_seen && report.recycled {
        return Ok(());
    }
    match presence {
        Presence::Present => Err(report.error.unwrap_or(if report.aborted {
            ActionError::Cancelled
        } else {
            ActionError::Failed(
                "Windows did not move the file to the Recycle Bin, and it is still in its original location."
                    .into(),
            )
        })),
        Presence::Absent if report.no_destination => Err(ActionError::DeletedNotRecycled),
        Presence::Absent | Presence::Unknown => Err(ActionError::RecycleOutcomeUnconfirmed),
    }
}

#[cfg(windows)]
fn ensure_recycle_token(elevated: bool) -> Result<(), ActionError> {
    if elevated {
        Err(ActionError::ElevatedRecycleUnavailable)
    } else {
        Ok(())
    }
}

/// Permanently marks the verified file object for deletion. The caller must
/// obtain separate permanent-delete confirmation before invoking this function.
pub fn delete_permanently(plan: RecyclePlan) -> Result<ActionOutcome, ActionError> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        };
        // Open once for DELETE while denying all other writers and deleters.
        // The confirmation never holds a handle or locks a user's directory.
        let file = native::open_file(&plan.path, true)?;
        if native::snapshot(&file)? != plan.snapshot {
            return Err(ActionError::Changed);
        }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: live DELETE-capable handle, checked identity/type/size/time on
        // that same handle, and a correctly sized FileDispositionInfo buffer.
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(native::io_error(std::io::Error::last_os_error()));
        }
        drop(file);
        // Outstanding readers and other hard links may retain data. This does
        // not promise freed bytes, secure erasure, or immediate completion.
        Ok(ActionOutcome::DeletionRequested {
            original_path: plan.path,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = plan;
        Err(ActionError::Unsupported)
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::recycle_policy::{self, RecycleFacts, Setting};
    use std::{
        fs::{File, OpenOptions},
        io,
        os::windows::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        },
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION, ERROR_PATH_NOT_FOUND,
            ERROR_SHARING_VIOLATION, ERROR_SUCCESS, HWND,
        },
        Storage::FileSystem::*,
        System::{
            Registry::{
                HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW,
            },
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow},
    };

    const POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    const BITBUCKET_VOLUMES: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume";

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct Snapshot {
        pub identity: FileIdentity,
        pub size: u64,
        pub modified: u64,
    }
    impl Snapshot {
        pub fn modified_seconds(&self) -> Option<u64> {
            (self.modified / 10_000_000).checked_sub(11_644_473_600)
        }
    }

    pub(super) fn supported_drive_type(drive_type: u32) -> bool {
        // GetDriveTypeW: DRIVE_REMOVABLE=2 and DRIVE_FIXED=3. Remote,
        // missing, unknown, optical, RAM disks and future values fail closed.
        matches!(drive_type, 2 | 3)
    }

    pub(super) fn direct_local_volume(target: &[u16]) -> bool {
        // Match the NTFS scanner's direct-volume policy. In particular, do not
        // follow SUBST / DOS-device aliases that could hide a network path or
        // an ancestor junction behind an otherwise ordinary drive letter.
        let prefix: Vec<_> = r"\Device\HarddiskVolume".encode_utf16().collect();
        target
            .strip_prefix(prefix.as_slice())
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.iter().all(|unit| (48..=57).contains(unit))
            })
    }

    fn ensure_local_drive(path: &Path) -> Result<(), ActionError> {
        let drive = path
            .as_os_str()
            .encode_wide()
            .next()
            .ok_or(ActionError::InvalidPath)?;
        let root = [drive, 58, 92, 0];
        // SAFETY: validated drive letter followed by colon, trailing backslash,
        // and NUL, as required by GetDriveTypeW. This is a read-only query.
        let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
        if !supported_drive_type(drive_type) {
            return Err(ActionError::InvalidPath);
        }
        let device = [drive, 58, 0];
        let mut target = [0u16; 1024];
        // SAFETY: NUL-terminated drive device (without slash), bounded writable
        // output. An unresolved/truncated device mapping is never accepted.
        let count =
            unsafe { QueryDosDeviceW(device.as_ptr(), target.as_mut_ptr(), target.len() as u32) }
                as usize;
        if count == 0 || count > target.len() {
            return Err(ActionError::InvalidPath);
        }
        let end = target[..count]
            .iter()
            .position(|unit| *unit == 0)
            .ok_or(ActionError::InvalidPath)?;
        if !direct_local_volume(&target[..end]) {
            return Err(ActionError::InvalidPath);
        }
        Ok(())
    }

    pub(super) fn validate_path(path: &Path) -> Result<(), ActionError> {
        let units: Vec<_> = path.as_os_str().encode_wide().collect();
        if units.len() < 4
            || units.len() > 32_760
            || !matches!(units[0], 65..=90 | 97..=122)
            || units[1] != b':' as u16
            || units[2] != b'\\' as u16
        {
            return Err(ActionError::InvalidPath);
        }
        for segment in units[3..].split(|unit| *unit == b'\\' as u16) {
            if segment.is_empty()
                || segment == [b'.' as u16]
                || segment == [b'.' as u16, b'.' as u16]
                || matches!(segment.last(), Some(32 | 46))
                || segment
                    .iter()
                    .any(|unit| matches!(*unit, 0 | 34 | 42 | 47 | 58 | 60 | 62 | 63 | 124))
                || reserved_name(segment)
            {
                return Err(ActionError::InvalidPath);
            }
        }
        Ok(())
    }

    fn reserved_name(segment: &[u16]) -> bool {
        let mut base: Vec<_> = segment
            .iter()
            .take_while(|unit| **unit != b'.' as u16)
            .map(|unit| {
                if (97..=122).contains(unit) {
                    unit - 32
                } else {
                    *unit
                }
            })
            .collect();
        while base.last() == Some(&32) {
            base.pop();
        }
        let name = |text: &str| base.iter().copied().eq(text.encode_utf16());
        name("CON")
            || name("PRN")
            || name("AUX")
            || name("NUL")
            || name("CONIN$")
            || name("CONOUT$")
            || (base.len() == 4
                && (base[..3] == [67, 79, 77] || base[..3] == [76, 80, 84])
                && matches!(base[3], 49..=57 | 0x00b9 | 0x00b2 | 0x00b3))
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// Read-only registry DWORD. A missing key or value is a Windows default
    /// (`Absent`); any other failure is missing evidence (`Unreadable`).
    fn read_dword(hive: HKEY, subkey: &str, value: &str) -> Setting {
        let subkey = wide(subkey);
        let value = wide(value);
        let mut data = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: NUL-terminated names, a writable DWORD with its exact size,
        // and RRF_RT_REG_DWORD restricts the type. This is a read-only query.
        let status = unsafe {
            RegGetValueW(
                hive,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS => Setting::Value(data),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Setting::Absent,
            _ => Setting::Unreadable,
        }
    }

    /// `\\?\Volume{GUID}\` for a drive root such as `C:\`.
    fn volume_name(root: &[u16; 4]) -> Option<String> {
        let mut buffer = [0u16; 50];
        // SAFETY: NUL-terminated drive root and a writable buffer of the
        // documented minimum length, whose size is passed in characters.
        let ok = unsafe {
            GetVolumeNameForVolumeMountPointW(
                root.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let end = buffer.iter().position(|unit| *unit == 0)?;
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    /// Gathers the Recycle Bin facts for the file's drive. Every environmental
    /// failure becomes `Unreadable` or `None`; none can refuse by itself.
    fn recycle_facts(path: &Path) -> RecycleFacts {
        let drive = path.as_os_str().encode_wide().next().unwrap_or(0);
        if drive == 0 {
            return RecycleFacts {
                drive_type: 0,
                policy_no_recycle: Setting::Absent,
                volume_resolved: false,
                volume_nuke_on_delete: Setting::Absent,
                volume_max_capacity_mb: Setting::Absent,
                volume_total_bytes: None,
            };
        }
        let root = [drive, 58, 92, 0];
        // SAFETY: drive letter, colon, backslash and NUL, as GetDriveTypeW
        // requires. This is a read-only query.
        let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
        let policy_no_recycle = read_dword(HKEY_LOCAL_MACHINE, POLICY_KEY, "NoRecycleFiles")
            .combine(read_dword(HKEY_CURRENT_USER, POLICY_KEY, "NoRecycleFiles"));
        let name = volume_name(&root);
        let guid = name.as_deref().and_then(recycle_policy::volume_guid_braced);
        let (volume_nuke_on_delete, volume_max_capacity_mb) = match guid {
            Some(guid) => {
                let key = format!(r"{BITBUCKET_VOLUMES}\{guid}");
                (
                    read_dword(HKEY_CURRENT_USER, &key, "NukeOnDelete"),
                    read_dword(HKEY_CURRENT_USER, &key, "MaxCapacity"),
                )
            }
            None => (Setting::Absent, Setting::Absent),
        };
        let mut total = 0u64;
        // SAFETY: NUL-terminated drive root and one writable u64; the other
        // out-parameters are optional and passed as null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                root.as_ptr(),
                std::ptr::null_mut(),
                &mut total,
                std::ptr::null_mut(),
            )
        };
        RecycleFacts {
            drive_type,
            policy_no_recycle,
            volume_resolved: guid.is_some(),
            volume_nuke_on_delete,
            volume_max_capacity_mb,
            volume_total_bytes: (ok != 0).then_some(total),
        }
    }

    pub(super) fn recycle_verdict(path: &Path, logical_size: u64) -> RecycleVerdict {
        let facts = recycle_facts(path);
        #[cfg(test)]
        eprintln!("DiskScope fixture recycle facts: {facts:?}");
        recycle_policy::recycle_verdict(&facts, logical_size)
    }

    /// Opens the path without the standard library's metadata fallback (which
    /// can list a delete-pending name as present) and compares what it finds
    /// with the approved snapshot.
    pub(super) fn presence(path: &Path, expected: &Snapshot) -> Presence {
        let observed = open_file(path, false)
            .and_then(|file| snapshot(&file))
            .map(|found| found == *expected);
        presence_of(&observed)
    }

    /// Rebuilds the owner window from its address and accepts it only if it is a
    /// live window of this process; a stale or foreign handle is ignored.
    pub(super) fn own_window(owner: NonZeroIsize) -> Option<HWND> {
        let hwnd = owner.get() as HWND;
        let mut process = 0u32;
        // SAFETY: both calls accept any handle value and report failure for a
        // stale one; the process id out-parameter is writable.
        let ours = unsafe {
            IsWindow(hwnd) != 0
                && GetWindowThreadProcessId(hwnd, &mut process) != 0
                && process == GetCurrentProcessId()
        };
        ours.then_some(hwnd)
    }

    pub(super) fn io_error(error: io::Error) -> ActionError {
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_ACCESS_DENIED) => ActionError::PermissionDenied,
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => ActionError::Busy,
            Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => ActionError::Changed,
            _ => ActionError::Failed(format!(
                "Windows could not complete the file action: {error}"
            )),
        }
    }

    #[derive(Debug)]
    pub(super) struct VerifiedFile {
        file: File,
        // Drop the file before releasing its ancestors. Each ancestor was
        // opened from root downward while the earlier components were pinned.
        _ancestors: Vec<File>,
    }

    impl std::ops::Deref for VerifiedFile {
        type Target = File;

        fn deref(&self) -> &Self::Target {
            &self.file
        }
    }

    pub(super) fn open_file(path: &Path, for_delete: bool) -> Result<File, ActionError> {
        open_checked(path, for_delete, false).map(|opened| opened.file)
    }

    pub(super) fn open_recycle_file(path: &Path) -> Result<VerifiedFile, ActionError> {
        open_checked(path, false, true)
    }

    fn open_checked(
        path: &Path,
        for_delete: bool,
        pin_ancestors: bool,
    ) -> Result<VerifiedFile, ActionError> {
        validate_path(path)?;
        // Re-run at preparation AND every commit/callback; drive syntax alone
        // does not distinguish a mapped SMB drive from a local volume. These
        // namespace queries are not an atomic lock on drive remapping; the
        // permanent action still validates and acts on its final pinned handle.
        ensure_local_drive(path)?;
        // Shell recycling pins the directory chain from root downward. Deny
        // write (including reparse mutation) and delete/rename until completion.
        // Preparation and same-handle permanent deletion only need the original
        // metadata policy check, not additional directory-list permissions.
        let ancestors: Vec<_> = path
            .parent()
            .ok_or(ActionError::InvalidPath)?
            .ancestors()
            .collect();
        let mut guards = Vec::with_capacity(if pin_ancestors { ancestors.len() } else { 0 });
        for ancestor in ancestors.into_iter().rev() {
            let directory = OpenOptions::new()
                // Attribute-only opens do not participate in Windows share
                // access checks. LIST_DIRECTORY makes these guards effective.
                .access_mode(
                    FILE_READ_ATTRIBUTES
                        | if pin_ancestors {
                            FILE_LIST_DIRECTORY
                        } else {
                            0
                        },
                )
                .share_mode(if pin_ancestors {
                    FILE_SHARE_READ
                } else {
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
                })
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(ancestor)
                .map_err(io_error)?;
            let attributes = directory.metadata().map_err(io_error)?.file_attributes();
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
            {
                return Err(ActionError::NotRegularFile);
            }
            if pin_ancestors {
                guards.push(directory);
            }
        }
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | if for_delete { DELETE } else { 0 })
            .share_mode(if for_delete {
                FILE_SHARE_READ
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
            })
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(io_error)?;
        Ok(VerifiedFile {
            file,
            _ancestors: guards,
        })
    }

    pub(super) fn snapshot(file: &File) -> Result<Snapshot, ActionError> {
        let metadata = file.metadata().map_err(io_error)?;
        if metadata.file_attributes() & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != 0
        {
            return Err(ActionError::NotRegularFile);
        }
        let mut info = FILE_ID_INFO::default();
        // SAFETY: valid file handle and exactly sized writable FileIdInfo buffer.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(io_error(io::Error::last_os_error()));
        }
        if info.FileId.Identifier == [0; 16] {
            return Err(ActionError::IdentityUnavailable);
        }
        Ok(Snapshot {
            identity: FileIdentity {
                volume: info.VolumeSerialNumber,
                id: info.FileId.Identifier,
            },
            size: metadata.file_size(),
            modified: metadata.last_write_time(),
        })
    }
}

#[cfg(windows)]
mod shell {
    use super::*;
    use std::{cell::RefCell, os::windows::ffi::OsStrExt, rc::Rc};
    use windows::{
        Win32::{
            Foundation::{E_ABORT, HWND},
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::Shell::*,
        },
        core::{Error, HRESULT, PCWSTR, Ref, Result as ComResult, implement},
    };

    /// Test hook: pretends Windows cleared the recycle bit so the sink veto
    /// can be exercised against a real fixture.
    #[cfg(test)]
    pub(super) static FORCE_SHELL_DECLINED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    const _: () =
        assert!(TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == TSF_DELETE_RECYCLE_IF_POSSIBLE_BIT);

    #[derive(Default)]
    struct State {
        pre_seen: bool,
        recycled: bool,
        no_destination: bool,
        error: Option<ActionError>,
    }

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn com_error(error: Error) -> ActionError {
        hresult_error(error.code())
    }
    /// HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED).
    const E_ACCESS_DENIED: u32 = 0x8007_0005;
    /// HRESULT_FROM_WIN32(ERROR_SHARING_VIOLATION).
    const E_SHARING_VIOLATION: u32 = 0x8007_0020;
    /// HRESULT_FROM_WIN32(ERROR_LOCK_VIOLATION).
    const E_LOCK_VIOLATION: u32 = 0x8007_0021;
    /// E_ABORT, also what [`RecycleSink::refuse`] returns; the stored sink
    /// error is checked before this mapping so the real reason wins.
    const E_ABORTED: u32 = 0x8000_4004;
    /// HRESULT_FROM_WIN32(ERROR_CANCELLED).
    const E_CANCELLED: u32 = 0x8007_04c7;
    /// COPYENGINE_E_USER_CANCELLED.
    const COPYENGINE_E_USER_CANCELLED: u32 = 0x8027_0000;
    /// COPYENGINE_E_CANCELLED.
    const COPYENGINE_E_CANCELLED: u32 = 0x8027_0001;

    fn hresult_error(code: HRESULT) -> ActionError {
        match code.0 as u32 {
            E_ACCESS_DENIED => ActionError::PermissionDenied,
            E_SHARING_VIOLATION | E_LOCK_VIOLATION => ActionError::Busy,
            E_ABORTED | E_CANCELLED | COPYENGINE_E_USER_CANCELLED | COPYENGINE_E_CANCELLED => {
                ActionError::Cancelled
            }
            _ => ActionError::Failed(failed_text(code.0 as u32)),
        }
    }

    /// Runs the single approved `DeleteItem` and reports what the sink saw. It
    /// never decides the user-visible outcome: `file_actions::recycle` checks
    /// the disk after the pinned handles in `_guard` have dropped. Errors
    /// before anything is queued return early with the file untouched.
    pub(super) fn recycle(
        plan: RecyclePlan,
        _guard: native::VerifiedFile,
        owner: Option<NonZeroIsize>,
    ) -> Result<ShellReport, ActionError> {
        // IFileOperation requires STA. A pre-existing MTA is refused, never used.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }.map_err(com_error)?;
        let _apartment = Apartment;
        let mut wide: Vec<_> = plan.path.as_os_str().encode_wide().collect();
        wide.push(0);
        // SAFETY: native absolute path checked before reaching this function;
        // the buffer is NUL-terminated and lives through item creation.
        let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }
            .map_err(com_error)?;
        let operation: IFileOperation =
            unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER) }
                .map_err(com_error)?;
        let plan = Rc::new(plan);
        let state = Rc::new(RefCell::new(State::default()));
        let sink: IFileOperationProgressSink = RecycleSink {
            plan: plan.clone(),
            expected_item: item.clone(),
            state: state.clone(),
        }
        .into();
        // FOFX_RECYCLEONDELETE is the documented recycle request; ALLOWUNDO
        // alone is insufficient. No documented flag makes the Shell fail
        // instead of delete when it cannot recycle: FOF_WANTNUKEWARNING is the
        // only Shell-level brake, and FOF_NOCONFIRMATION / FOF_NO_UI would
        // answer that prompt Yes for the user. The preflight, the sink veto
        // and the settlement in file_actions::recycle carry the rest. These
        // are the flags the validated happy-path fixture used.
        let flags = FOFX_RECYCLEONDELETE
            | FOFX_ADDUNDORECORD
            | FOF_WANTNUKEWARNING
            | FOF_NOERRORUI
            | FOFX_EARLYFAILURE
            | FOF_SILENT
            | FOF_NO_CONNECTED_ELEMENTS
            | FOF_NORECURSION
            | FOF_NORECURSEREPARSE
            | FOFX_NOCOPYHOOKS;
        unsafe { operation.SetOperationFlags(flags) }.map_err(com_error)?;
        // Best effort: parent any prompt of Windows' own to DiskScope so it is
        // modal and in front rather than hidden behind the main window. It is
        // defence in depth, so a failure must not disable every recycle. The
        // UI thread must never block on or join this worker while an owner is
        // set (the two threads' input queues become attached); it only polls.
        if let Some(hwnd) = owner.and_then(native::own_window) {
            let _ = unsafe { operation.SetOwnerWindow(HWND(hwnd)) };
        }
        #[cfg(test)]
        eprintln!("DiskScope fixture SetOperationFlags: {:#010x}", flags.0);
        let cookie = unsafe { operation.Advise(&sink) }.map_err(com_error)?;
        let result = (|| -> Result<bool, ActionError> {
            // Only this one DeleteItem is queued. No permanent API is called
            // on failure, and no connected sidecars or directories are queued.
            unsafe { operation.DeleteItem(&item, None) }.map_err(com_error)?;
            unsafe { operation.PerformOperations() }.map_err(com_error)?;
            Ok(unsafe { operation.GetAnyOperationsAborted() }
                .map_err(com_error)?
                .as_bool())
        })();
        let _ = unsafe { operation.Unadvise(cookie) };
        let mut finished = state.borrow_mut();
        let (call_error, aborted) = match result {
            Ok(aborted) => (None, aborted),
            Err(error) => (Some(error), false),
        };
        Ok(ShellReport {
            pre_seen: finished.pre_seen,
            recycled: finished.recycled,
            no_destination: finished.no_destination,
            // The sink's reason comes first: it saw the Shell's own report.
            error: finished.error.take().or(call_error),
            aborted,
        })
    }

    // The callbacks and non-Send Shell interfaces stay in this single STA.
    #[implement(IFileOperationProgressSink, Agile = false)]
    struct RecycleSink {
        plan: Rc<RecyclePlan>,
        expected_item: IShellItem,
        state: Rc<RefCell<State>>,
    }

    impl RecycleSink {
        fn refuse(&self, error: ActionError) -> ComResult<()> {
            if let Ok(mut state) = self.state.try_borrow_mut()
                && state.error.is_none()
            {
                state.error = Some(error);
            }
            Err(Error::from_hresult(E_ABORT))
        }
    }

    #[allow(non_snake_case)]
    impl IFileOperationProgressSink_Impl for RecycleSink_Impl {
        fn StartOperations(&self) -> ComResult<()> {
            Ok(())
        }
        fn FinishOperations(&self, result: HRESULT) -> ComResult<()> {
            if result.is_err() {
                let _ = self.refuse(hresult_error(result));
            }
            Ok(())
        }
        fn PreDeleteItem(&self, flags: u32, item: Ref<'_, IShellItem>) -> ComResult<()> {
            let first = self
                .state
                .try_borrow_mut()
                .map(|mut state| {
                    let first = !state.pre_seen;
                    state.pre_seen = true;
                    first
                })
                .unwrap_or(false);
            #[cfg(test)]
            eprintln!("DiskScope fixture PreDeleteItem: flags={flags:#010x}, first={first}");
            #[cfg(test)]
            let flags = if FORCE_SHELL_DECLINED.load(std::sync::atomic::Ordering::SeqCst) {
                flags & !TSF_DELETE_RECYCLE_IF_POSSIBLE_BIT
            } else {
                flags
            };
            // Veto by failure HRESULT (never an S_ code) when Windows does not
            // offer the Recycle Bin or queues more than the one approved item.
            if let Err(error) = pre_delete_check(first, flags) {
                return self.refuse(error);
            }
            let Some(item) = item.as_ref() else {
                return self.refuse(ActionError::Changed);
            };
            // Canonical Shell identity must match the sole queued item; native
            // identity and exact metadata are independently checked below.
            if unsafe { item.Compare(&self.expected_item, SICHINT_CANONICAL.0 as u32) }.ok()
                != Some(0)
            {
                return self.refuse(ActionError::Changed);
            }
            let verified =
                native::open_file(&self.plan.path, false).and_then(|file| native::snapshot(&file));
            match verified {
                Ok(snapshot) if snapshot == self.plan.snapshot => Ok(()),
                Ok(_) => self.refuse(ActionError::Changed),
                Err(error) => self.refuse(error),
            }
            // The outer operation retains the checked ancestor handles. Shell
            // still owns the final leaf-name operation, so this is not an atomic
            // same-file guarantee like permanent deletion by handle.
        }
        fn PostDeleteItem(
            &self,
            _flags: u32,
            _item: Ref<'_, IShellItem>,
            result: HRESULT,
            destination: Ref<'_, IShellItem>,
        ) -> ComResult<()> {
            #[cfg(test)]
            eprintln!(
                "DiskScope fixture PostDeleteItem: flags={_flags:#010x}, result={:#010x}, recycled_destination={}",
                result.0 as u32,
                !destination.is_null()
            );
            if result.is_err() {
                return self.refuse(hresult_error(result));
            }
            let mut state = self
                .state
                .try_borrow_mut()
                .map_err(|_| Error::from_hresult(E_ABORT))?;
            if destination.is_null() {
                // Success without a Recycle Bin item. Only a hint: the item may
                // already be gone or still intact (a skipped item reports the
                // same), so the disk decides what the user is told.
                state.no_destination = true;
            } else {
                state.recycled = true;
            }
            Ok(())
        }
        fn PreRenameItem(
            &self,
            _flags: u32,
            _item: Ref<'_, IShellItem>,
            _name: &PCWSTR,
        ) -> ComResult<()> {
            self.refuse(ActionError::Cancelled)
        }
        fn PostRenameItem(
            &self,
            _flags: u32,
            _item: Ref<'_, IShellItem>,
            _name: &PCWSTR,
            _result: HRESULT,
            _created: Ref<'_, IShellItem>,
        ) -> ComResult<()> {
            Ok(())
        }
        fn PreMoveItem(
            &self,
            _flags: u32,
            _item: Ref<'_, IShellItem>,
            _destination: Ref<'_, IShellItem>,
            _name: &PCWSTR,
        ) -> ComResult<()> {
            self.refuse(ActionError::Cancelled)
        }
        fn PostMoveItem(
            &self,
            _flags: u32,
            _item: Ref<'_, IShellItem>,
            _destination: Ref<'_, IShellItem>,
            _name: &PCWSTR,
            _result: HRESULT,
            _created: Ref<'_, IShellItem>,
        ) -> ComResult<()> {
            Ok(())
        }
        fn PreCopyItem(
            &self,
            _flags: u32,
            _item: Ref<'_, IShellItem>,
            _destination: Ref<'_, IShellItem>,
            _name: &PCWSTR,
        ) -> ComResult<()> {
            self.refuse(ActionError::Cancelled)
        }
        fn PostCopyItem(
            &self,
            _flags: u32,
            _item: Ref<'_, IShellItem>,
            _destination: Ref<'_, IShellItem>,
            _name: &PCWSTR,
            _result: HRESULT,
            _created: Ref<'_, IShellItem>,
        ) -> ComResult<()> {
            Ok(())
        }
        fn PreNewItem(
            &self,
            _flags: u32,
            _destination: Ref<'_, IShellItem>,
            _name: &PCWSTR,
        ) -> ComResult<()> {
            self.refuse(ActionError::Cancelled)
        }
        fn PostNewItem(
            &self,
            _flags: u32,
            _destination: Ref<'_, IShellItem>,
            _name: &PCWSTR,
            _template: &PCWSTR,
            _attributes: u32,
            _result: HRESULT,
            _created: Ref<'_, IShellItem>,
        ) -> ComResult<()> {
            Ok(())
        }
        fn UpdateProgress(&self, _total: u32, _done: u32) -> ComResult<()> {
            Ok(())
        }
        fn ResetTimer(&self) -> ComResult<()> {
            Ok(())
        }
        fn PauseTimer(&self) -> ComResult<()> {
            Ok(())
        }
        fn ResumeTimer(&self) -> ComResult<()> {
            Ok(())
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::*;

    // These fixtures create/read/remove their own workspace files only. They do
    // not invoke the Recycle Bin or inspect any user data.
    fn fixture() -> (tempfile::TempDir, PathBuf, FileIdentity) {
        let directory = tempfile::Builder::new()
            .prefix("DiskScope-recycle-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        assert!(
            directory
                .path()
                .canonicalize()
                .unwrap()
                .starts_with(&workspace),
            "fixtures must remain inside the verified workspace"
        );
        let path = directory.path().join("disposable-fixture.txt");
        fs::write(&path, b"DiskScope disposable recycle policy fixture").unwrap();
        let file = fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .unwrap();
        let mut info = FILE_ID_INFO::default();
        // SAFETY: live handle and correctly sized output buffer.
        assert_ne!(
            unsafe {
                GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileIdInfo,
                    (&mut info as *mut FILE_ID_INFO).cast(),
                    size_of::<FILE_ID_INFO>() as u32,
                )
            },
            0
        );
        (
            directory,
            path,
            FileIdentity {
                volume: info.VolumeSerialNumber,
                id: info.FileId.Identifier,
            },
        )
    }

    #[test]
    fn prepares_only_the_selected_regular_file() {
        let (_directory, path, identity) = fixture();
        let size = fs::metadata(&path).unwrap().len();
        let plan = prepare(&path, &identity, size, None).unwrap();
        assert_eq!(plan.path(), path);
        assert_eq!(plan.size(), size);
        assert!(
            path.is_file(),
            "preparation must not move or remove the fixture"
        );
    }

    #[test]
    fn verified_file_guard_blocks_ancestor_replacement_until_released() {
        let (directory, path, _identity) = fixture();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let inside = nested.join("selected.txt");
        fs::rename(&path, &inside).unwrap();
        let open_directory_for_write = || {
            fs::OpenOptions::new()
                .access_mode(windows_sys::Win32::Foundation::GENERIC_WRITE)
                .share_mode(7)
                .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
                .open(&nested)
        };
        drop(open_directory_for_write().unwrap());
        let verified = native::open_recycle_file(&inside).unwrap();
        assert!(
            open_directory_for_write().is_err(),
            "a retained guard must deny ancestor reparse writers"
        );
        let replacement = directory.path().join("moved");
        let rename = fs::rename(&nested, &replacement);
        assert!(rename.is_err(), "a retained guard must pin every ancestor");
        assert!(inside.is_file());
        drop(verified);
        drop(open_directory_for_write().unwrap());
        fs::rename(&nested, &replacement).unwrap();
        assert!(replacement.join("selected.txt").is_file());
    }

    #[test]
    fn verification_refuses_an_existing_ancestor_writer_and_releases_after_preparation() {
        let (directory, path, identity) = fixture();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let inside = nested.join("selected.txt");
        fs::rename(&path, &inside).unwrap();
        let writer = fs::OpenOptions::new()
            .access_mode(windows_sys::Win32::Foundation::GENERIC_WRITE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
            .open(&nested)
            .unwrap();
        assert!(matches!(
            native::open_recycle_file(&inside),
            Err(ActionError::Busy)
        ));
        // Preparing a confirmation does not need directory-list exclusivity.
        assert!(
            prepare(
                &inside,
                &identity,
                fs::metadata(&inside).unwrap().len(),
                None
            )
            .is_ok()
        );
        drop(writer);
        let plan = prepare(
            &inside,
            &identity,
            fs::metadata(&inside).unwrap().len(),
            None,
        )
        .unwrap();
        let renamed = directory.path().join("renamed");
        fs::rename(&nested, &renamed).unwrap();
        assert_eq!(plan.path(), inside);
        assert!(
            renamed.join("selected.txt").is_file(),
            "confirmation must not hold directory locks"
        );
    }

    #[test]
    fn rejects_a_replaced_scan_identity() {
        let (_directory, path, mut identity) = fixture();
        identity.id[0] ^= 1;
        assert_eq!(
            prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap_err(),
            ActionError::Changed
        );
        assert!(path.is_file());
    }

    #[test]
    fn rejects_size_changes_since_the_scan() {
        let (_directory, path, identity) = fixture();
        assert_eq!(
            prepare(&path, &identity, 0, None).unwrap_err(),
            ActionError::Changed
        );
    }

    #[test]
    fn refuses_directory_targets() {
        let (directory, _path, identity) = fixture();
        assert_eq!(
            prepare(directory.path(), &identity, 0, None).unwrap_err(),
            ActionError::NotRegularFile
        );
    }

    #[test]
    fn refuses_unknown_identity() {
        let (_directory, path, identity) = fixture();
        let unknown = FileIdentity {
            volume: identity.volume,
            id: [0; 16],
        };
        assert_eq!(
            prepare(&path, &unknown, fs::metadata(&path).unwrap().len(), None).unwrap_err(),
            ActionError::IdentityUnavailable
        );
    }

    #[test]
    fn refuses_ambiguous_or_nonlocal_paths() {
        let identity = FileIdentity {
            volume: 1,
            id: [1; 16],
        };
        for path in [
            r"relative.txt",
            r"C:relative.txt",
            r"\\server\share\file.txt",
            r"\\?\C:\file.txt",
            r"C:\file.txt:stream",
            r"C:\",
            r"C:\a\..\file.txt",
        ] {
            assert_eq!(
                prepare(Path::new(path), &identity, 0, None).unwrap_err(),
                ActionError::InvalidPath,
                "{path}"
            );
        }
    }

    #[test]
    fn rejects_last_write_changes_since_the_scan() {
        let (_directory, path, identity) = fixture();
        assert_eq!(
            prepare(
                &path,
                &identity,
                fs::metadata(&path).unwrap().len(),
                Some(1)
            )
            .unwrap_err(),
            ActionError::Changed
        );
    }

    #[test]
    fn elevated_recycle_is_refused() {
        assert_eq!(
            ensure_recycle_token(true),
            Err(ActionError::ElevatedRecycleUnavailable)
        );
        assert_eq!(ensure_recycle_token(false), Ok(()));
    }

    #[test]
    fn drive_policy_rejects_remote_unknown_and_unsupported_media() {
        // Microsoft GetDriveTypeW return values, independently enumerated here.
        for (drive_type, accepted) in [
            (0, false),
            (1, false),
            (2, true),
            (3, true),
            (4, false),
            (5, false),
            (6, false),
            (99, false),
        ] {
            assert_eq!(
                native::supported_drive_type(drive_type),
                accepted,
                "drive type {drive_type}"
            );
        }
    }

    #[test]
    fn drive_policy_refuses_substitution_and_network_device_aliases() {
        for (target, accepted) in [
            (r"\Device\HarddiskVolume3", true),
            (r"\Device\HarddiskVolume42", true),
            (r"\??\D:\folder", false),
            (r"\??\UNC\server\share", false),
            (r"\Device\Mup\server\share", false),
            (r"\Device\LanmanRedirector\server\share", false),
            (r"\Device\HarddiskVolume3\folder", false),
            (r"\Device\HarddiskVolume", false),
            ("", false),
        ] {
            assert_eq!(
                native::direct_local_volume(&target.encode_utf16().collect::<Vec<_>>()),
                accepted,
                "{target}"
            );
        }
    }

    #[test]
    #[ignore = "Disposable Recycle Bin integration fixture: run only with explicit test authorization, with --test-threads=1"]
    fn recycle_sends_only_the_confirmed_fixture_to_the_bin() {
        let (directory, path, identity) = fixture();
        let sibling = directory.path().join("unselected-sibling.txt");
        fs::write(&sibling, b"must remain").unwrap();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        let elevated = crate::admin::is_elevated().unwrap();
        eprintln!("DiskScope fixture Windows token elevated={elevated}");
        eprintln!("DiskScope fixture verdict={:?}", plan.recycle_verdict());
        if elevated {
            assert!(matches!(
                recycle(plan, None),
                Err(ActionError::ElevatedRecycleUnavailable)
            ));
            assert!(path.is_file());
        } else {
            // Happy-path guard: an ordinary file on a fixed volume is not refused.
            assert!(!matches!(
                plan.recycle_verdict(),
                RecycleVerdict::Blocked(_)
            ));
            assert!(matches!(
                recycle(plan, None).unwrap(),
                ActionOutcome::Recycled { .. }
            ));
            assert!(!path.exists());
        }
        assert_eq!(fs::read(sibling).unwrap(), b"must remain");
    }

    #[test]
    #[ignore = "Reads this host's Recycle Bin settings; run on a standard fixed workspace volume"]
    fn prepared_plan_carries_a_recycle_verdict_that_does_not_block_the_fixture() {
        let (_directory, path, identity) = fixture();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        let verdict = plan.recycle_verdict();
        eprintln!("DiskScope fixture verdict={verdict:?}");
        match verdict {
            RecycleVerdict::Blocked(RecycleBlock::NoBinOnDrive) => {
                eprintln!("workspace drive is not a fixed disk; nothing to assert")
            }
            other => assert!(!matches!(other, RecycleVerdict::Blocked(_)), "{other:?}"),
        }
    }

    #[test]
    fn presence_follows_the_approved_file_not_the_name() {
        let (_directory, path, _identity) = fixture();
        let file = native::open_file(&path, false).unwrap();
        let approved = native::snapshot(&file).unwrap();
        drop(file);
        assert_eq!(native::presence(&path, &approved), Presence::Present);
        fs::remove_file(&path).unwrap();
        assert_eq!(native::presence(&path, &approved), Presence::Absent);
        // Something else re-created the path: the approved file is still gone.
        fs::write(&path, b"a different, longer replacement file").unwrap();
        assert_eq!(native::presence(&path, &approved), Presence::Absent);
    }

    #[test]
    fn owner_window_rejects_a_stale_handle() {
        assert!(native::own_window(NonZeroIsize::new(1).unwrap()).is_none());
    }

    #[test]
    #[ignore = "Disposable Recycle Bin integration fixture: run only with explicit test authorization, with --test-threads=1"]
    fn sink_veto_leaves_the_file_intact() {
        // Proves the central assumption that a failure HRESULT from
        // PreDeleteItem stops the Shell. If it fails the fixture is destroyed
        // and the result must be DeletedNotRecycled, never a plain failure.
        // Run with --test-threads=1: the hook is process-wide.
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                shell::FORCE_SHELL_DECLINED.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        if crate::admin::is_elevated().unwrap() {
            eprintln!("elevated token: Recycle is refused earlier; nothing to prove");
            return;
        }
        let (directory, path, identity) = fixture();
        let sibling = directory.path().join("unselected-sibling.txt");
        fs::write(&sibling, b"must remain").unwrap();
        let before = fs::read(&path).unwrap();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        if matches!(plan.recycle_verdict(), RecycleVerdict::Blocked(_)) {
            eprintln!("workspace volume is refused before the Shell; nothing to prove");
            return;
        }
        let _reset = Reset;
        shell::FORCE_SHELL_DECLINED.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            recycle(plan, None).unwrap_err(),
            ActionError::RecycleUnavailable(RecycleBlock::ShellDeclined)
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read(sibling).unwrap(), b"must remain");
    }

    #[test]
    #[ignore = "Disposable Recycle Bin integration fixture: run only with explicit test authorization, with --test-threads=1"]
    fn recycle_with_an_owner_window_completes() {
        use windows_sys::Win32::{
            System::Threading::GetCurrentThreadId,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
                PostThreadMessageW, TranslateMessage, WM_QUIT,
            },
        };
        if crate::admin::is_elevated().unwrap() {
            return;
        }
        // A hidden top-level window on a helper thread that pumps messages,
        // like the application's main window on its own thread.
        let (sender, receiver) = std::sync::mpsc::channel();
        let pump = std::thread::spawn(move || {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: predefined class, null title, no parent or menu; the
            // window is created, pumped and destroyed on this one thread.
            unsafe {
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                sender.send((hwnd as isize, GetCurrentThreadId())).unwrap();
                let mut message = std::mem::zeroed::<MSG>();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                DestroyWindow(hwnd);
            }
        });
        let (hwnd, thread) = receiver.recv().unwrap();
        // The point of the test is an owner that is actually applied.
        let owner = NonZeroIsize::new(hwnd).expect("the hidden window was created");
        assert!(
            native::own_window(owner).is_some(),
            "a live window of this process must be accepted as the owner"
        );
        let (_directory, path, identity) = fixture();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        // Run on a worker so a stuck prompt fails the test instead of hanging it.
        let (done, outcome) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(recycle(plan, Some(owner)));
        });
        let outcome = outcome.recv_timeout(std::time::Duration::from_secs(10));
        // SAFETY: posts WM_QUIT to the pump thread created above.
        unsafe { PostThreadMessageW(thread, WM_QUIT, 0, 0) };
        pump.join().unwrap();
        let outcome = outcome.expect("recycle with an owner window finished within 10 seconds");
        assert!(matches!(outcome.unwrap(), ActionOutcome::Recycled { .. }));
        assert!(!path.exists());
    }

    #[test]
    #[ignore = "Disposable deletion integration fixture: run only with explicit test authorization"]
    fn permanent_delete_removes_only_the_confirmed_fixture() {
        let (directory, path, identity) = fixture();
        let sibling = directory.path().join("unselected-sibling.txt");
        fs::write(&sibling, b"must remain").unwrap();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        assert!(matches!(
            delete_permanently(plan).unwrap(),
            ActionOutcome::DeletionRequested { .. }
        ));
        assert!(!path.exists());
        assert_eq!(fs::read(sibling).unwrap(), b"must remain");
    }

    #[test]
    #[ignore = "Disposable deletion integration fixture: run only with explicit test authorization"]
    fn permanent_delete_refuses_a_changed_confirmed_file() {
        let (_directory, path, identity) = fixture();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        fs::write(&path, b"changed after confirmation").unwrap();
        assert!(matches!(
            delete_permanently(plan),
            Err(ActionError::Changed)
        ));
        assert_eq!(fs::read(path).unwrap(), b"changed after confirmation");
    }

    #[test]
    #[ignore = "Disposable deletion integration fixture: run only with explicit test authorization"]
    fn permanent_delete_refuses_a_replaced_path() {
        let (directory, path, identity) = fixture();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        let original = directory.path().join("original-moved-by-test.txt");
        fs::rename(&path, &original).unwrap();
        fs::write(&path, b"DiskScope disposable recycle policy fixture").unwrap();
        assert!(matches!(
            delete_permanently(plan),
            Err(ActionError::Changed)
        ));
        assert!(original.is_file());
        assert!(path.is_file());
    }

    #[test]
    #[ignore = "Disposable deletion integration fixture: run only with explicit test authorization"]
    fn permanent_delete_refuses_an_open_writer() {
        let (_directory, path, identity) = fixture();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        let _writer = fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&path)
            .unwrap();
        assert!(matches!(delete_permanently(plan), Err(ActionError::Busy)));
        assert!(path.is_file());
    }

    #[test]
    #[ignore = "Disposable deletion integration fixture: run only with explicit test authorization"]
    fn permanent_delete_does_not_clear_the_read_only_attribute() {
        let (_directory, path, identity) = fixture();
        let original_permissions = fs::metadata(&path).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        let result = delete_permanently(plan);
        let still_read_only = fs::metadata(&path).unwrap().permissions().readonly();
        // Restore only this fixture's original Windows attributes. Production
        // never changes the read-only attribute or file permissions.
        fs::set_permissions(&path, original_permissions).unwrap();
        assert!(matches!(result, Err(ActionError::PermissionDenied)));
        assert!(still_read_only);
    }

    fn make_fixture_junction(link: &Path, target: &Path) {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        assert!(
            link.parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .starts_with(&workspace)
        );
        assert!(target.canonicalize().unwrap().starts_with(&workspace));
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "could not create the owned junction fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "Own workspace junction integration fixture: run only with explicit test authorization"]
    fn junction_ancestor_is_refused_at_preparation() {
        let (directory, path, identity) = fixture();
        let target = directory.path().join("junction-target");
        fs::create_dir(&target).unwrap();
        let selected = target.join("disposable-fixture.txt");
        fs::rename(&path, &selected).unwrap();
        let link = directory.path().join("junction-link");
        make_fixture_junction(&link, &target);
        let result = prepare(&link.join("disposable-fixture.txt"), &identity, 43, None);
        // Remove only the directory-junction entry, never its target tree.
        fs::remove_dir(&link).unwrap();
        assert!(matches!(result, Err(ActionError::NotRegularFile)));
        assert_eq!(
            fs::read(selected).unwrap(),
            b"DiskScope disposable recycle policy fixture"
        );
    }

    #[test]
    #[ignore = "Own workspace junction integration fixture: run only with explicit test authorization"]
    fn junction_added_after_confirmation_is_refused_at_commit() {
        let (directory, path, identity) = fixture();
        let selected_directory = directory.path().join("original-selected-directory");
        fs::create_dir(&selected_directory).unwrap();
        let selected = selected_directory.join("disposable-fixture.txt");
        fs::rename(path, &selected).unwrap();
        let plan = prepare(&selected, &identity, 43, None).unwrap();
        let moved = directory.path().join("renamed-original-directory");
        fs::rename(&selected_directory, &moved).unwrap();
        make_fixture_junction(&selected_directory, &moved);
        let result = delete_permanently(plan);
        fs::remove_dir(&selected_directory).unwrap();
        assert!(matches!(result, Err(ActionError::NotRegularFile)));
        assert_eq!(
            fs::read(moved.join("disposable-fixture.txt")).unwrap(),
            b"DiskScope disposable recycle policy fixture"
        );
    }
}

/// Platform-independent tests for the Recycle outcome rules. They run on every
/// host because the rules are pure functions over plain data.
#[cfg(test)]
mod outcome_tests {
    use super::*;

    fn report(
        pre_seen: bool,
        recycled: bool,
        no_destination: bool,
        error: Option<ActionError>,
        aborted: bool,
    ) -> ShellReport {
        ShellReport {
            pre_seen,
            recycled,
            no_destination,
            error,
            aborted,
        }
    }

    #[test]
    fn pre_delete_check_table() {
        // 0x80 is the Shell's *request* bit: it can veto but never prove a
        // recycle (reported, not yet verified, to stay set for a file larger
        // than the Bin maximum).
        assert_eq!(pre_delete_check(true, 0x282), Ok(()));
        assert_eq!(
            pre_delete_check(true, 0x202),
            Err(ActionError::RecycleUnavailable(RecycleBlock::ShellDeclined))
        );
        assert!(matches!(
            pre_delete_check(false, 0x282),
            Err(ActionError::Failed(_))
        ));
    }

    #[test]
    fn presence_of_table() {
        assert_eq!(presence_of(&Ok(true)), Presence::Present);
        // A different file at the same path is not the approved file.
        assert_eq!(presence_of(&Ok(false)), Presence::Absent);
        assert_eq!(presence_of(&Err(ActionError::Changed)), Presence::Absent);
        // A name we cannot open, such as a delete-pending one, is unknown.
        for error in [
            ActionError::PermissionDenied,
            ActionError::Busy,
            ActionError::NotRegularFile,
            ActionError::InvalidPath,
            ActionError::Failed("x".into()),
        ] {
            assert_eq!(
                presence_of(&Err(error.clone())),
                Presence::Unknown,
                "{error:?}"
            );
        }
    }

    #[test]
    fn unclassified_shell_failures_do_not_suggest_the_file_is_safe() {
        let text = failed_text(0x8000_4005);
        assert!(text.contains("0x80004005"), "{text}");
        assert!(!text.contains("fallback"), "{text}");
        assert!(!text.contains("No permanent"), "{text}");
        assert!(!text.contains("Nothing was changed"), "{text}");
    }

    #[test]
    fn settle_truth_table() {
        let veto = ActionError::RecycleUnavailable(RecycleBlock::ShellDeclined);
        // A confirmed Recycle Bin destination wins over everything else.
        for presence in [Presence::Present, Presence::Absent, Presence::Unknown] {
            assert_eq!(
                settle(
                    report(true, true, false, Some(ActionError::Cancelled), false),
                    presence
                ),
                Ok(())
            );
        }
        // File present: the recorded reason, else the abort, else a plain failure.
        let present = |r| settle(r, Presence::Present).unwrap_err();
        assert!(
            matches!(present(report(true, false, false, None, false)), ActionError::Failed(text) if text.contains("still in its original location"))
        );
        assert_eq!(
            present(report(true, false, false, Some(veto.clone()), false)),
            veto
        );
        assert_eq!(
            present(report(true, false, false, None, true)),
            ActionError::Cancelled
        );
        assert_eq!(
            present(report(
                true,
                false,
                false,
                Some(ActionError::PermissionDenied),
                false
            )),
            ActionError::PermissionDenied
        );
        // File gone with a null destination: deleted, even if a veto was ignored.
        for error in [None, Some(veto.clone())] {
            for aborted in [false, true] {
                assert_eq!(
                    settle(
                        report(true, false, true, error.clone(), aborted),
                        Presence::Absent
                    ),
                    Err(ActionError::DeletedNotRecycled)
                );
            }
        }
        // Gone or unreadable without a destination hint: never "cancelled".
        for (presence, hint) in [
            (Presence::Absent, false),
            (Presence::Unknown, true),
            (Presence::Unknown, false),
        ] {
            assert_eq!(
                settle(
                    report(true, false, hint, Some(ActionError::Cancelled), false),
                    presence
                ),
                Err(ActionError::RecycleOutcomeUnconfirmed)
            );
        }
        // A destination without the pre-delete callback is not proof.
        assert_eq!(
            settle(report(false, true, false, None, false), Presence::Absent),
            Err(ActionError::RecycleOutcomeUnconfirmed)
        );
    }

    #[test]
    fn settle_never_lies_about_a_missing_file() {
        let errors = [
            None,
            Some(ActionError::Cancelled),
            Some(ActionError::PermissionDenied),
            Some(ActionError::Busy),
            Some(ActionError::Changed),
            Some(ActionError::RecycleUnavailable(RecycleBlock::ShellDeclined)),
            Some(ActionError::Failed("x".into())),
        ];
        for pre_seen in [false, true] {
            for recycled in [false, true] {
                for no_destination in [false, true] {
                    for aborted in [false, true] {
                        for error in &errors {
                            for presence in [Presence::Present, Presence::Absent, Presence::Unknown]
                            {
                                let input = report(
                                    pre_seen,
                                    recycled,
                                    no_destination,
                                    error.clone(),
                                    aborted,
                                );
                                let label = format!("{input:?} {presence:?}");
                                match settle(input, presence) {
                                    // I1: success needs a confirmed Bin destination.
                                    Ok(()) => assert!(pre_seen && recycled, "{label}"),
                                    // I2: an error that does not say the file may be
                                    // gone is only allowed for a file seen present.
                                    Err(e) if !e.file_may_be_gone() => {
                                        assert_eq!(presence, Presence::Present, "{label}")
                                    }
                                    // And "deleted" needs the file seen absent.
                                    Err(ActionError::DeletedNotRecycled) => {
                                        assert_eq!(presence, Presence::Absent, "{label}")
                                    }
                                    Err(_) => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn file_may_be_gone_truth_table() {
        // Written without a wildcard so a new variant cannot be forgotten.
        for (error, gone) in [
            (ActionError::Unsupported, false),
            (ActionError::InvalidPath, false),
            (ActionError::IdentityUnavailable, false),
            (ActionError::Changed, false),
            (ActionError::NotRegularFile, false),
            (ActionError::PermissionDenied, false),
            (ActionError::Busy, false),
            (ActionError::ElevatedRecycleUnavailable, false),
            (ActionError::Cancelled, false),
            (
                ActionError::RecycleUnavailable(RecycleBlock::NoBinOnDrive),
                false,
            ),
            (ActionError::DeletedNotRecycled, true),
            (ActionError::RecycleOutcomeUnconfirmed, true),
            (ActionError::Failed("x".into()), false),
        ] {
            assert_eq!(error.file_may_be_gone(), gone, "{error:?}");
            match error {
                ActionError::Unsupported
                | ActionError::InvalidPath
                | ActionError::IdentityUnavailable
                | ActionError::Changed
                | ActionError::NotRegularFile
                | ActionError::PermissionDenied
                | ActionError::Busy
                | ActionError::ElevatedRecycleUnavailable
                | ActionError::Cancelled
                | ActionError::RecycleUnavailable(_)
                | ActionError::DeletedNotRecycled
                | ActionError::RecycleOutcomeUnconfirmed
                | ActionError::Failed(_) => {}
            }
        }
    }

    #[test]
    fn error_messages_tell_the_truth() {
        let blocked = ActionError::RecycleUnavailable(RecycleBlock::DeletesImmediately).to_string();
        assert!(blocked.contains("remove files immediately"), "{blocked}");
        assert!(blocked.ends_with("Nothing was changed."), "{blocked}");
        let deleted = ActionError::DeletedNotRecycled.to_string();
        assert!(deleted.contains("without keeping a copy in the Recycle Bin"));
        assert!(deleted.contains("probably cannot be restored"));
        let unconfirmed = ActionError::RecycleOutcomeUnconfirmed.to_string();
        assert!(unconfirmed.contains("did not confirm"));
        assert!(unconfirmed.contains("Check the Recycle Bin"));
        for error in [
            ActionError::Cancelled,
            ActionError::Busy,
            ActionError::PermissionDenied,
            blocked_error(),
            ActionError::DeletedNotRecycled,
            ActionError::RecycleOutcomeUnconfirmed,
        ] {
            let text = error.to_string();
            assert!(!text.contains("No permanent fallback"), "{text}");
            assert!(!text.contains("without falling back"), "{text}");
        }
    }

    fn blocked_error() -> ActionError {
        ActionError::RecycleUnavailable(RecycleBlock::TooLarge {
            size: 5 << 20,
            limit: 1 << 20,
        })
    }
}
