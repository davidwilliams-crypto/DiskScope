//! Confirmed single-file actions: normal-token Shell recycling and explicit
//! permanent deletion by a freshly verified native handle.
use crate::model::FileIdentity;
use std::{
    fmt,
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
    Failed(String),
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
pub fn recycle(plan: RecyclePlan) -> Result<ActionOutcome, ActionError> {
    #[cfg(windows)]
    {
        ensure_recycle_token(crate::admin::is_elevated().map_err(ActionError::Failed)?)?;
        let file = native::open_recycle_file(&plan.path)?;
        if native::snapshot(&file)? != plan.snapshot {
            return Err(ActionError::Changed);
        }
        // Keep the verified file and its ancestor guards alive until Windows
        // has finished. Releasing them before PerformOperations would reopen
        // the checked namespace to ancestor replacement.
        shell::recycle(plan, file)
    }
    #[cfg(not(windows))]
    {
        let _ = plan;
        Err(ActionError::Unsupported)
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
            ERROR_SHARING_VIOLATION,
        },
        Storage::FileSystem::*,
    };

    #[derive(Debug, PartialEq, Eq)]
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
            Foundation::E_ABORT,
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::Shell::*,
        },
        core::{Error, HRESULT, PCWSTR, Ref, Result as ComResult, implement},
    };

    #[derive(Default)]
    struct State {
        pre_seen: bool,
        recycled: bool,
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
            _ => ActionError::Failed(format!(
                "Windows could not complete the Recycle Bin operation ({:#010X}). No permanent fallback was requested.",
                code.0 as u32
            )),
        }
    }

    pub(super) fn recycle(
        plan: RecyclePlan,
        _guard: native::VerifiedFile,
    ) -> Result<ActionOutcome, ActionError> {
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
        // alone is insufficient. Do not auto-answer any Shell confirmation.
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
        if let Some(error) = finished.error.take() {
            return Err(error);
        }
        if result? {
            return Err(ActionError::Cancelled);
        }
        if !finished.pre_seen || !finished.recycled {
            return Err(ActionError::Failed("Windows did not confirm a Recycle Bin destination. The action was not reported as recycled.".into()));
        }
        Ok(ActionOutcome::Recycled {
            original_path: plan.path.clone(),
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
            if !first || flags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
                return self.refuse(ActionError::Failed("Windows did not offer the approved single-file Recycle Bin operation. Permanent deletion was not approved.".into()));
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
                "DiskScope fixture PostDeleteItem: result={:#010x}, recycled_destination={}",
                result.0 as u32,
                !destination.is_null()
            );
            if result.is_err() {
                return self.refuse(hresult_error(result));
            }
            if destination.is_null() {
                return self.refuse(ActionError::Failed("Windows returned no recycled destination; the result could not be verified as a Recycle Bin operation.".into()));
            }
            let mut state = self
                .state
                .try_borrow_mut()
                .map_err(|_| Error::from_hresult(E_ABORT))?;
            state.recycled = true;
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
    #[ignore = "Disposable Recycle Bin integration fixture: run only with explicit test authorization"]
    fn recycle_sends_only_the_confirmed_fixture_to_the_bin() {
        let (directory, path, identity) = fixture();
        let sibling = directory.path().join("unselected-sibling.txt");
        fs::write(&sibling, b"must remain").unwrap();
        let plan = prepare(&path, &identity, fs::metadata(&path).unwrap().len(), None).unwrap();
        let elevated = crate::admin::is_elevated().unwrap();
        eprintln!("DiskScope fixture Windows token elevated={elevated}");
        if elevated {
            assert!(matches!(
                recycle(plan),
                Err(ActionError::ElevatedRecycleUnavailable)
            ));
            assert!(path.is_file());
        } else {
            assert!(matches!(
                recycle(plan).unwrap(),
                ActionOutcome::Recycled { .. }
            ));
            assert!(!path.exists());
        }
        assert_eq!(fs::read(sibling).unwrap(), b"must remain");
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
