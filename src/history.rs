//! Bounded local metadata history. No file contents are read or stored.
use crate::model::{FileIdentity, Model, ScanState};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// At most five 128 MiB files plus one 128 MiB atomic staging file (768 MiB).
pub const MAX_SNAPSHOTS: usize = 5;
pub const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_DISK_BYTES: u64 = 768 * 1024 * 1024;
pub const MAX_NODES: usize = 1_000_000;
const MAX_NAME: usize = 32_768;
const MAX_DEPTH: usize = 256;
// Expanded paths are bounded too: a tiny deep hierarchy must not inflate to gigabytes.
const MAX_EXPANDED_PATH_UNITS: usize = 256 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"DSHIST02";
const SCHEMA: u32 = 2;
// Logical bytes per directory entry; allocation per entry; reparse directories not followed.
const POLICY: u32 = 1;
const OWNED_ELSEWHERE: &str = "History is unavailable because another window owns this directory. Close that window and reopen history.";
const OWNERSHIP_RETRY: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub struct HistorySnapshot {
    pub revision: u64,
    pub busy: bool,
    pub scans: Vec<ScanSummary>,
    pub comparison: Option<Arc<Comparison>>,
    pub message: String,
}
#[derive(Clone, Debug)]
pub struct ScanSummary {
    pub id: u64,
    pub root: PathBuf,
    pub finished_unix: u64,
    pub logical: u64,
    pub files: u64,
    pub folders: u64,
    pub backend: String,
    pub live_generation: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
    Moved,
}
#[derive(Clone, Debug)]
pub struct ChangeRow {
    pub kind: ChangeKind,
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub before: u64,
    pub after: u64,
    pub delta: i128,
}
#[derive(Clone, Debug)]
pub struct FolderChange {
    pub path: PathBuf,
    pub before: u64,
    pub after: u64,
    pub delta: i128,
}
#[derive(Clone, Debug)]
pub struct Comparison {
    pub root: PathBuf,
    pub older_id: u64,
    pub newer_id: u64,
    pub newer_generation: Option<u64>,
    pub before: u64,
    pub after: u64,
    pub delta: i128,
    pub added: u64,
    pub removed: u64,
    pub modified: u64,
    pub moved: u64,
    pub total_changes: u64,
    pub rows: Vec<ChangeRow>,
    pub folder_changes: Vec<FolderChange>,
}
pub struct HistoryService {
    capture: HistoryCapture,
    commands: Sender<Command>,
    updates: Receiver<Arc<HistorySnapshot>>,
    stop: Arc<AtomicBool>,
}
#[derive(Clone)]
pub struct HistoryCapture {
    captures: Sender<Capture>,
    rejected: Arc<AtomicBool>,
}
struct Capture {
    model: Arc<Model>,
    generation: u64,
    backend: String,
}
enum Command {
    Compare(u64, u64),
    Forget,
}
impl HistoryService {
    /// Publishes the reason once, performs no disk I/O and starts no worker.
    /// Captures and commands are disconnected, so requests fail immediately.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        let (captures, captures_rx) = bounded(1);
        drop(captures_rx);
        let (commands, commands_rx) = bounded(2);
        drop(commands_rx);
        let (updates_tx, updates) = bounded(1);
        updates_tx
            .try_send(Arc::new(HistorySnapshot {
                revision: 1,
                message: reason.into(),
                ..Default::default()
            }))
            .expect("initial unavailable history snapshot");
        Self {
            capture: HistoryCapture {
                captures,
                rejected: Arc::new(AtomicBool::new(false)),
            },
            commands,
            updates,
            stop: Arc::new(AtomicBool::new(true)),
        }
    }
    /// Starts one detached worker. This method and all UI-facing methods do no disk I/O.
    pub fn new(directory: PathBuf) -> Self {
        let (captures_tx, captures) = bounded(1);
        let (commands_tx, commands) = bounded(2);
        let (updates_tx, updates) = bounded(1);
        let output = updates.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let rejected = Arc::new(AtomicBool::new(false));
        let capture = HistoryCapture {
            captures: captures_tx,
            rejected: rejected.clone(),
        };
        let stopping = stop.clone();
        thread::Builder::new()
            .name("metadata-history".into())
            .spawn(move || {
                history_worker(
                    directory, captures, commands, updates_tx, output, stopping, rejected,
                )
            })
            .expect("history worker");
        Self {
            capture,
            commands: commands_tx,
            updates,
            stop,
        }
    }
    pub fn capture(&self) -> HistoryCapture {
        self.capture.clone()
    }
    pub fn poll(&self) -> Option<Arc<HistorySnapshot>> {
        self.updates.try_recv().ok()
    }
    pub fn compare(&self, older_id: u64, newer_id: u64) -> bool {
        self.command(Command::Compare(older_id, newer_id))
    }
    pub fn forget_all(&self) -> bool {
        self.command(Command::Forget)
    }
    fn command(&self, command: Command) -> bool {
        let accepted = self.commands.try_send(command).is_ok();
        if !accepted {
            self.capture.rejected.store(true, Ordering::Release);
        }
        accepted
    }
}
impl Drop for HistoryService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
impl HistoryCapture {
    /// Admission checks are constant-time. The worker validates/copies the shared complete model.
    pub fn submit(
        &self,
        model: Arc<Model>,
        generation: u64,
        state: ScanState,
        backend: String,
    ) -> bool {
        if state != ScanState::Complete
            || model.errors_total != 0
            || model.overflowed
            || model.nodes.len() > MAX_NODES
        {
            self.rejected.store(true, Ordering::Release);
            return false;
        }
        let accepted = self
            .captures
            .try_send(Capture {
                model,
                generation,
                backend,
            })
            .is_ok();
        if !accepted {
            self.rejected.store(true, Ordering::Release);
        }
        accepted
    }
}
pub fn default_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("DISKSCOPE_HISTORY_DIR") {
        return PathBuf::from(path);
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("DiskScope")
        .join("history")
}

struct SavedNode {
    parent: Option<u32>,
    name: OsString,
    is_dir: bool,
    reparse: bool,
    logical: u64,
    allocated: Option<u64>,
    modified: Option<u64>,
    identity: Option<FileIdentity>,
}
struct Saved {
    summary: ScanSummary,
    nodes: Vec<SavedNode>,
}

fn publish(
    snapshot: &mut HistorySnapshot,
    tx: &Sender<Arc<HistorySnapshot>>,
    rx: &Receiver<Arc<HistorySnapshot>>,
) {
    snapshot.revision = snapshot.revision.wrapping_add(1);
    let update = Arc::new(snapshot.clone());
    if let Err(crossbeam_channel::TrySendError::Full(update)) = tx.try_send(update) {
        let _ = rx.try_recv();
        let _ = tx.try_send(update);
    }
}
fn history_worker(
    directory: PathBuf,
    captures: Receiver<Capture>,
    commands: Receiver<Command>,
    tx: Sender<Arc<HistorySnapshot>>,
    rx: Receiver<Arc<HistorySnapshot>>,
    stop: Arc<AtomicBool>,
    rejected: Arc<AtomicBool>,
) {
    let mut snapshot = HistorySnapshot {
        busy: true,
        ..Default::default()
    };
    publish(&mut snapshot, &tx, &rx);
    // Detached shutdown can leave the preceding worker alive for a few milliseconds.
    // Retry only on this worker; callers never wait for ownership or disk I/O.
    let deadline = Instant::now() + OWNERSHIP_RETRY;
    let opened = loop {
        let result = Directory::open(directory.clone());
        if result.as_ref().is_err_and(|e| e == OWNED_ELSEWHERE)
            && Instant::now() < deadline
            && !stop.load(Ordering::Acquire)
        {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        break result;
    };
    let directory = match opened {
        Ok(d) => d,
        Err(e) => {
            snapshot.busy = false;
            snapshot.message = format!("History unavailable: {e}");
            publish(&mut snapshot, &tx, &rx);
            return;
        }
    };
    match directory.load() {
        Ok((scans, message)) => {
            snapshot.scans = scans;
            snapshot.message = match retain_snapshots(&directory, &mut snapshot) {
                Ok(()) => message,
                Err(error) => format!("History retention could not finish: {error}"),
            };
        }
        Err(e) => snapshot.message = format!("History could not be loaded: {e}"),
    }
    snapshot.busy = false;
    publish(&mut snapshot, &tx, &rx);
    let mut skipped = false;
    while !stop.load(Ordering::Acquire) {
        if rejected.swap(false, Ordering::AcqRel) {
            skipped = true;
            snapshot.message="History request skipped: scan incomplete/oversized, or background queue busy. Retry when idle.".into();
            publish(&mut snapshot, &tx, &rx);
        }
        let work = if let Ok(c) = commands.try_recv() {
            Some(Ok(c))
        } else {
            captures
                .recv_timeout(Duration::from_millis(20))
                .ok()
                .map(Err)
        };
        let Some(work) = work else { continue };
        snapshot.busy = true;
        publish(&mut snapshot, &tx, &rx);
        let result = match work {
            Err(capture) => save_capture(&directory, capture, &mut snapshot),
            Ok(Command::Forget) => directory.clear().map(|()| {
                // A queued completion belongs to the state the user just cleared.
                let _ = captures.try_recv();
                snapshot.scans.clear();
                snapshot.comparison = None;
                snapshot.message = "Local metadata history cleared.".into();
            }),
            Ok(Command::Compare(older, newer)) => {
                snapshot.comparison = None;
                compare_saved(&directory,&snapshot.scans,older,newer).map(|comparison|{
                    snapshot.comparison=Some(Arc::new(comparison));
                    snapshot.message="Comparison uses logical bytes per entry. Moves require unique known identity; hardlink aliases and unknown identities remain additions/removals.".into();
                })
            }
        };
        if let Err(error) = result {
            snapshot.message = error;
        }
        if std::mem::take(&mut skipped) {
            snapshot.message.push_str(" Some history requests were skipped because the scan was incomplete/oversized or the background queue was busy.");
        }
        snapshot.busy = false;
        publish(&mut snapshot, &tx, &rx);
    }
}

fn save_capture(
    directory: &Directory,
    capture: Capture,
    snapshot: &mut HistorySnapshot,
) -> Result<(), String> {
    let m = &capture.model;
    if m.nodes.is_empty() || m.nodes.len() > MAX_NODES || m.errors_total != 0 || m.overflowed {
        return Err("History skipped: incomplete or oversized metadata.".into());
    }
    // A previous write may have committed before Windows temporarily refused
    // retention. Recover that excess before another write reserves disk space.
    retain_snapshots(directory, snapshot)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut id = next_snapshot_id(
        now.as_micros().min(u64::MAX as u128) as u64,
        snapshot.scans.iter().map(|s| s.id),
    )?;
    while directory.file(id).exists() || snapshot.scans.iter().any(|s| s.id == id) {
        id = id.checked_add(1).ok_or("History ID exhausted")?;
    }
    let summary = ScanSummary {
        id,
        root: m.root_path.clone(),
        finished_unix: now.as_secs(),
        logical: m.nodes[0].logical,
        files: m.nodes[0].files,
        folders: m.nodes[0].folders,
        backend: capture.backend,
        live_generation: Some(capture.generation),
    };
    let nodes = m
        .nodes
        .iter()
        .map(|n| SavedNode {
            parent: n.parent,
            name: if n.parent.is_none() {
                OsString::new()
            } else {
                n.name.clone()
            },
            is_dir: n.is_dir,
            reparse: n.reparse,
            logical: n.logical,
            allocated: n.allocated,
            modified: n.modified,
            identity: n.identity.clone(),
        })
        .collect();
    let saved = Saved { summary, nodes };
    validate(&saved)?;
    let bytes = encode(&saved)?;
    directory.write(id, &bytes)?;
    snapshot.scans.push(saved.summary);
    snapshot.scans.sort_by_key(|s| std::cmp::Reverse(s.id));
    retain_snapshots(directory, snapshot)?;
    snapshot.message="Saved complete metadata locally; five snapshots maximum, 128 MiB each, 768 MiB including staging. No file contents stored.".into();
    Ok(())
}

/// Snapshot ids order the catalog and decide retention, so they must grow even
/// when the wall clock moves backwards (VM restore, manual change, a shared
/// history directory). The new id is the current time or one past the newest
/// retained id, whichever is larger.
fn next_snapshot_id(now_micros: u64, existing: impl Iterator<Item = u64>) -> Result<u64, String> {
    match existing.max() {
        Some(newest) => Ok(now_micros.max(newest.checked_add(1).ok_or("History ID exhausted")?)),
        None => Ok(now_micros),
    }
}

fn retain_snapshots(directory: &Directory, snapshot: &mut HistorySnapshot) -> Result<(), String> {
    while snapshot.scans.len() > MAX_SNAPSHOTS {
        let old = snapshot.scans.last().unwrap();
        directory.remove(old.id)?;
        if snapshot
            .comparison
            .as_ref()
            .is_some_and(|c| c.older_id == old.id || c.newer_id == old.id)
        {
            snapshot.comparison = None;
        }
        snapshot.scans.pop();
    }
    Ok(())
}

struct Directory {
    requested: PathBuf,
    resolved: PathBuf,
    identity: FileIdentity,
    // OS-backed lifetime exclusivity covers catalog load, byte admission, retention and Clear.
    _ownership: File,
}
impl Directory {
    fn open(requested: PathBuf) -> Result<Self, String> {
        if !requested.is_absolute() {
            return Err("History directory must be absolute".into());
        }
        fs::create_dir_all(&requested).map_err(|e| e.to_string())?;
        let resolved = fs::canonicalize(&requested).map_err(|e| e.to_string())?;
        if resolved.parent().is_none() {
            return Err("History cannot use a filesystem root".into());
        }
        let identity = directory_identity(&resolved)?;
        let ownership = acquire_ownership(&resolved)?;
        let directory = Self {
            requested,
            resolved,
            identity,
            _ownership: ownership,
        };
        directory.check()?;
        Ok(directory)
    }
    fn check(&self) -> Result<(), String> {
        if fs::canonicalize(&self.requested).map_err(|e| e.to_string())? != self.resolved
            || directory_identity(&self.resolved)? != self.identity
        {
            return Err("History directory changed; refusing disk operation".into());
        }
        Ok(())
    }
    fn file(&self, id: u64) -> PathBuf {
        self.resolved.join(format!("diskscope-{id:016x}.dsh"))
    }
    fn owned(&self) -> Result<Vec<(u64, PathBuf, u64)>, String> {
        self.check()?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.resolved).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let Some(id) = owned_id(&entry.file_name()) else {
                continue;
            };
            let meta = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            if !meta.file_type().is_file() || meta.file_type().is_symlink() {
                continue;
            }
            if entries.len() >= 128 {
                return Err("Too many history files; refusing unbounded load".into());
            }
            entries.push((id, entry.path(), meta.len()));
        }
        Ok(entries)
    }
    fn load(&self) -> Result<(Vec<ScanSummary>, String), String> {
        let pruned = self.prune_staging()?;
        let mut files = self.owned()?;
        let total = self.disk_usage()?;
        if total > MAX_DISK_BYTES {
            return Err("History disk budget exceeded; clear local history before retrying".into());
        }
        files.sort_by_key(|(id, _, _)| std::cmp::Reverse(*id));
        let mut scans = Vec::new();
        let mut rejected = 0;
        for (id, _, _) in files {
            match self.read(id) {
                // Keep the catalog intact if later retention hits a transient
                // Windows sharing failure; the next capture can retry it.
                Ok(saved) => scans.push(saved.summary),
                Err(_) => rejected += 1,
            }
        }
        let mut message = if rejected > 0 {
            format!(
                "Ignored {rejected} corrupt, oversized or incompatible history file(s). Clear history to remove them."
            )
        } else {
            "Local metadata history ready. Five snapshots maximum; no file contents stored.".into()
        };
        if pruned > 0 {
            message.push_str(&format!(" Removed {pruned} interrupted history write(s)."));
        }
        Ok((scans, message))
    }
    /// Removes staging files left by an interrupted write. This directory is
    /// exclusively owned, so no other window can be mid-write; without this,
    /// an orphaned 128 MiB `.tmp` would count against the disk budget until
    /// the user clears all history.
    fn prune_staging(&self) -> Result<usize, String> {
        let mut pruned = 0;
        for path in self.staging()? {
            self.check()?;
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
                fs::remove_file(path).map_err(|e| e.to_string())?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }
    fn read(&self, id: u64) -> Result<Saved, String> {
        self.check()?;
        let path = self.file(id);
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.file_type().is_file()
            || meta.file_type().is_symlink()
            || meta.len() > MAX_SNAPSHOT_BYTES as u64
        {
            return Err("Unsafe or oversized history file".into());
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_SNAPSHOT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err("Oversized history file".into());
        }
        let saved = decode(&bytes)?;
        if saved.summary.id != id {
            return Err("History filename/ID mismatch".into());
        }
        Ok(saved)
    }
    fn write(&self, id: u64, bytes: &[u8]) -> Result<(), String> {
        self.check()?;
        let disk = self.disk_usage()?;
        if bytes.len() > MAX_SNAPSHOT_BYTES
            || disk
                .checked_add(bytes.len() as u64)
                .is_none_or(|n| n > MAX_DISK_BYTES)
        {
            return Err("History skipped: metadata storage budget exceeded".into());
        }
        let staging = self.resolved.join(format!("diskscope-{id:016x}.tmp"));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|e| e.to_string())?;
        let result = (|| {
            f.write_all(bytes).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
            drop(f);
            self.check()?;
            fs::rename(&staging, self.file(id)).map_err(|e| e.to_string())
        })();
        if result.is_err() && self.check().is_ok() {
            let _ = fs::remove_file(staging);
        }
        result
    }
    fn remove(&self, id: u64) -> Result<(), String> {
        self.check()?;
        let path = self.file(id);
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.file_type().is_file() || meta.file_type().is_symlink() {
            return Err("Refusing to remove a history link or directory".into());
        }
        fs::remove_file(path).map_err(|e| e.to_string())
    }
    fn clear(&self) -> Result<(), String> {
        for (id, _, _) in self.owned()? {
            self.remove(id)?;
        }
        self.prune_staging().map(|_| ())
    }
    fn staging(&self) -> Result<Vec<PathBuf>, String> {
        self.check()?;
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.resolved).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if owned_id_with_suffix(&entry.file_name(), ".tmp").is_none() {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            if meta.file_type().is_file() && !meta.file_type().is_symlink() {
                if files.len() >= 128 {
                    return Err("Too many history staging files; refusing unbounded load".into());
                }
                files.push(entry.path());
            }
        }
        Ok(files)
    }
    fn disk_usage(&self) -> Result<u64, String> {
        let mut total = self
            .owned()?
            .iter()
            .try_fold(0u64, |n, (_, _, len)| n.checked_add(*len))
            .ok_or("History disk budget exceeded")?;
        for path in self.staging()? {
            total = total
                .checked_add(fs::symlink_metadata(path).map_err(|e| e.to_string())?.len())
                .ok_or("History disk budget exceeded")?;
        }
        Ok(total)
    }
}

#[cfg(windows)]
fn acquire_ownership(directory: &std::path::Path) -> Result<File, String> {
    use std::{
        mem::size_of,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::{Foundation::ERROR_SHARING_VIOLATION, Storage::FileSystem::*};
    // No delete sharing: another app instance cannot replace this lock while it is held.
    // OPEN_REPARSE_POINT prevents following an attacker-supplied link in the lock slot.
    let ownership = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory.join(".diskscope-history.lock"))
        .map_err(|e| {
            if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) {
                OWNED_ELSEWHERE.into()
            } else {
                format!("Cannot acquire history ownership: {e}")
            }
        })?;
    let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: live owned file handle and correctly sized writable tag output.
    if unsafe {
        GetFileInformationByHandleEx(
            ownership.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if tag.FileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0 {
        return Err("Refusing unsafe history ownership link or directory".into());
    }
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: same live handle and initialized file information output.
    if unsafe { GetFileInformationByHandle(ownership.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if info.nNumberOfLinks != 1 {
        return Err("Refusing a hardlinked history ownership file".into());
    }
    Ok(ownership)
}
#[cfg(unix)]
fn acquire_ownership(directory: &std::path::Path) -> Result<File, String> {
    let path = directory.join(".diskscope-history.lock");
    if fs::symlink_metadata(&path)
        .is_ok_and(|m| !m.file_type().is_file() || m.file_type().is_symlink())
    {
        return Err("Refusing unsafe history ownership link or directory".into());
    }
    let ownership = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())?;
    ownership.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => OWNED_ELSEWHERE.into(),
        std::fs::TryLockError::Error(e) => e.to_string(),
    })?;
    Ok(ownership)
}

#[cfg(windows)]
fn directory_identity(path: &std::path::Path) -> Result<FileIdentity, String> {
    use std::{
        mem::size_of,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::*;
    let directory = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut info = FILE_ID_INFO::default();
    // SAFETY: the directory owns a live handle; output matches FileIdInfo and its size.
    if unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(FileIdentity {
        volume: info.VolumeSerialNumber,
        id: info.FileId.Identifier,
    })
}
#[cfg(unix)]
fn directory_identity(path: &std::path::Path) -> Result<FileIdentity, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let mut id = [0; 16];
    id[..8].copy_from_slice(&meta.ino().to_le_bytes());
    Ok(FileIdentity {
        volume: meta.dev(),
        id,
    })
}
fn owned_id(name: &OsStr) -> Option<u64> {
    owned_id_with_suffix(name, ".dsh")
}
fn owned_id_with_suffix(name: &OsStr, suffix: &str) -> Option<u64> {
    let s = name.to_str()?;
    let hex = s.strip_prefix("diskscope-")?.strip_suffix(suffix)?;
    if hex.len() != 16
        || !hex
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

#[cfg(windows)]
fn wide(s: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().collect()
}
#[cfg(windows)]
fn native(w: &[u16]) -> Result<OsString, String> {
    use std::os::windows::ffi::OsStringExt;
    Ok(OsString::from_wide(w))
}
#[cfg(not(windows))]
fn wide(s: &OsStr) -> Vec<u16> {
    s.to_string_lossy().encode_utf16().collect()
}
#[cfg(not(windows))]
fn native(w: &[u16]) -> Result<OsString, String> {
    String::from_utf16(w)
        .map(OsString::from)
        .map_err(|e| e.to_string())
}

fn validate(saved: &Saved) -> Result<(), String> {
    if !saved.summary.root.is_absolute()
        || saved
            .summary
            .root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || saved.nodes.is_empty()
        || saved.nodes.len() > MAX_NODES
        || saved.summary.backend.len() > 512
    {
        return Err("Invalid history root, backend or node count".into());
    }
    let root = wide(saved.summary.root.as_os_str());
    if root.len() > MAX_NAME || root.contains(&0) {
        return Err("Invalid native root".into());
    }
    let mut depths = vec![0usize; saved.nodes.len()];
    let mut lengths = vec![root.len(); saved.nodes.len()];
    let mut expanded = root.len();
    let mut children = HashSet::new();
    let mut logical = vec![0u64; saved.nodes.len()];
    let mut files = vec![0u64; saved.nodes.len()];
    let mut folders = vec![0u64; saved.nodes.len()];
    for (i, node) in saved.nodes.iter().enumerate() {
        if i == 0 {
            if node.parent.is_some() || !node.is_dir || !node.name.is_empty() {
                return Err("Invalid history root node".into());
            }
            continue;
        }
        let parent = node.parent.ok_or("History node without parent")? as usize;
        if parent >= i || !saved.nodes[parent].is_dir {
            return Err("History parent cycle or invalid hierarchy".into());
        }
        depths[i] = depths[parent] + 1;
        let name = wide(&node.name);
        lengths[i] = lengths[parent]
            .checked_add(name.len() + 1)
            .ok_or("History path length overflow")?;
        expanded = expanded
            .checked_add(lengths[i])
            .ok_or("History expanded path overflow")?;
        if expanded > MAX_EXPANDED_PATH_UNITS {
            return Err("History skipped: expanded path memory budget exceeded".into());
        }
        if depths[i] > MAX_DEPTH
            || lengths[i] > MAX_NAME
            || name.is_empty()
            || name.iter().any(|c| [0, 47, 92, 58].contains(c))
            || node.name == OsStr::new(".")
            || node.name == OsStr::new("..")
        {
            return Err("Invalid history native name, depth or path length".into());
        }
        if !children.insert((parent, node.name.clone())) {
            return Err("Duplicate history path".into());
        }
        if !node.is_dir {
            logical[i] = node.logical;
            files[i] = 1;
        }
    }
    for i in (1..saved.nodes.len()).rev() {
        let node = &saved.nodes[i];
        if node.is_dir && logical[i] != node.logical {
            return Err("History directory totals do not match".into());
        }
        let p = node.parent.unwrap() as usize;
        logical[p] = logical[p]
            .checked_add(logical[i])
            .ok_or("History byte overflow")?;
        files[p] = files[p]
            .checked_add(files[i])
            .ok_or("History file overflow")?;
        folders[p] = folders[p]
            .checked_add(folders[i] + u64::from(node.is_dir))
            .ok_or("History folder overflow")?;
    }
    if logical[0] != saved.nodes[0].logical
        || logical[0] != saved.summary.logical
        || files[0] != saved.summary.files
        || folders[0] != saved.summary.folders
    {
        return Err("History totals do not match metadata".into());
    }
    Ok(())
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend(v.to_le_bytes());
}
fn put_wide(out: &mut Vec<u8>, value: &OsStr) -> Result<(), String> {
    let w = wide(value);
    if w.len() > MAX_NAME {
        return Err("Native name too long".into());
    }
    put_u32(out, w.len() as u32);
    for c in w {
        out.extend(c.to_le_bytes());
    }
    Ok(())
}
fn put_optional(out: &mut Vec<u8>, v: Option<u64>) {
    out.push(u8::from(v.is_some()));
    if let Some(v) = v {
        put_u64(out, v);
    }
}
fn encode(saved: &Saved) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend(MAGIC);
    put_u32(&mut out, SCHEMA);
    put_u32(&mut out, POLICY);
    let s = &saved.summary;
    for n in [s.id, s.finished_unix, s.logical, s.files, s.folders] {
        put_u64(&mut out, n);
    }
    put_wide(&mut out, s.root.as_os_str())?;
    put_u32(&mut out, s.backend.len() as u32);
    out.extend(s.backend.as_bytes());
    put_u32(&mut out, saved.nodes.len() as u32);
    for node in &saved.nodes {
        put_u32(&mut out, node.parent.unwrap_or(u32::MAX));
        out.push(u8::from(node.is_dir) | (u8::from(node.reparse) << 1));
        put_u64(&mut out, node.logical);
        put_optional(&mut out, node.allocated);
        put_optional(&mut out, node.modified);
        out.push(u8::from(node.identity.is_some()));
        if let Some(id) = &node.identity {
            put_u64(&mut out, id.volume);
            out.extend(id.id);
        }
        put_wide(&mut out, &node.name)?;
        if out.len() > MAX_SNAPSHOT_BYTES - 8 {
            return Err("History skipped: metadata exceeds 128 MiB snapshot limit".into());
        }
    }
    let checksum = checksum(&out);
    put_u64(&mut out, checksum);
    Ok(out)
}
fn checksum(bytes: &[u8]) -> u64 {
    // Integrity check for accidental corruption, not an authentication boundary.
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("History offset overflow")?;
        let b = self.bytes.get(self.at..end).ok_or("Truncated history")?;
        self.at = end;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn wide(&mut self) -> Result<OsString, String> {
        let n = self.u32()? as usize;
        if n > MAX_NAME {
            return Err("History native name too long".into());
        }
        let b = self.take(n.checked_mul(2).ok_or("History length overflow")?)?;
        let w: Vec<_> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|x| u16::from_le_bytes(*x))
            .collect();
        native(&w)
    }
    fn optional(&mut self) -> Result<Option<u64>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.u64().map(Some),
            _ => Err("Invalid optional metadata".into()),
        }
    }
}
fn decode(bytes: &[u8]) -> Result<Saved, String> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err("Oversized history".into());
    }
    if bytes.len() < 8 {
        return Err("Truncated history".into());
    }
    let (bytes, tail) = bytes.split_at(bytes.len() - 8);
    if checksum(bytes) != u64::from_le_bytes(tail.try_into().unwrap()) {
        return Err("History integrity check failed".into());
    }
    let mut c = Cursor { bytes, at: 0 };
    if c.take(8)? != MAGIC || c.u32()? != SCHEMA || c.u32()? != POLICY {
        return Err("Incompatible history schema or accounting policy".into());
    }
    let id = c.u64()?;
    let finished_unix = c.u64()?;
    let logical = c.u64()?;
    let files = c.u64()?;
    let folders = c.u64()?;
    let root = PathBuf::from(c.wide()?);
    let len = c.u32()? as usize;
    if len > 512 {
        return Err("History backend too long".into());
    }
    let backend = String::from_utf8(c.take(len)?.to_vec()).map_err(|e| e.to_string())?;
    let count = c.u32()? as usize;
    if count == 0 || count > MAX_NODES || count > bytes.len() / 20 {
        return Err("Invalid history node count".into());
    }
    let mut nodes = Vec::with_capacity(count);
    for _ in 0..count {
        let parent = c.u32()?;
        let flags = c.u8()?;
        if flags > 3 {
            return Err("Invalid history node flags".into());
        }
        let logical = c.u64()?;
        let allocated = c.optional()?;
        let modified = c.optional()?;
        let identity = match c.u8()? {
            0 => None,
            1 => Some(FileIdentity {
                volume: c.u64()?,
                id: c.take(16)?.try_into().unwrap(),
            }),
            _ => return Err("Invalid history identity".into()),
        };
        nodes.push(SavedNode {
            parent: (parent != u32::MAX).then_some(parent),
            name: c.wide()?,
            is_dir: flags & 1 != 0,
            reparse: flags & 2 != 0,
            logical,
            allocated,
            modified,
            identity,
        });
    }
    if c.at != bytes.len() {
        return Err("Unexpected trailing history data".into());
    }
    let saved = Saved {
        summary: ScanSummary {
            id,
            root,
            finished_unix,
            logical,
            files,
            folders,
            backend,
            live_generation: None,
        },
        nodes,
    };
    validate(&saved)?;
    Ok(saved)
}

fn saved_path(saved: &Saved, mut index: usize) -> PathBuf {
    // The decoder already validates ancestry and depth. Expand one change
    // candidate's path, instead of retaining every expanded path in both scans.
    let mut names = Vec::new();
    while let Some(parent) = saved.nodes[index].parent {
        names.push(saved.nodes[index].name.as_os_str());
        index = parent as usize;
    }
    let mut path = saved.summary.root.clone();
    for name in names.into_iter().rev() {
        path.push(name);
    }
    path
}

fn matching_paths(old: &Saved, new: &Saved) -> Vec<Option<u32>> {
    // Both roots are equal, names contain no separators/dot components, and
    // parents precede children. Matching (matched parent, exact native name)
    // therefore gives full-path equality without expanded strings or hash-only
    // identities. HashMap resolves hash collisions through ordinary key equality.
    let children: HashMap<_, _> = old
        .nodes
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, node)| ((node.parent.unwrap(), node.name.as_os_str()), i as u32))
        .collect();
    let mut matches = vec![None; new.nodes.len()];
    matches[0] = Some(0);
    for (j, node) in new.nodes.iter().enumerate().skip(1) {
        if let Some(parent) = matches[node.parent.unwrap() as usize] {
            matches[j] = children.get(&(parent, node.name.as_os_str())).copied();
        }
    }
    matches
}

fn unique_identities(saved: &Saved) -> HashMap<&FileIdentity, Option<usize>> {
    let mut identities = HashMap::new();
    for (i, node) in saved.nodes.iter().enumerate().skip(1) {
        if !node.is_dir
            && let Some(id) = &node.identity
        {
            // None marks aliases anywhere in the snapshot, including files
            // already matched by path. No per-identity vector allocation.
            identities
                .entry(id)
                .and_modify(|single| *single = None)
                .or_insert(Some(i));
        }
    }
    identities
}
fn compare_saved(
    directory: &Directory,
    summaries: &[ScanSummary],
    older: u64,
    newer: u64,
) -> Result<Comparison, String> {
    let a = summaries
        .iter()
        .find(|s| s.id == older)
        .ok_or("Choose two retained scans")?;
    let b = summaries
        .iter()
        .find(|s| s.id == newer)
        .ok_or("Choose two retained scans")?;
    if older == newer || a.root != b.root || !compatible_backends(&a.backend, &b.backend) {
        return Err("Choose two compatible scans of the identical native root and backend family (normal and raw NTFS preview are separate).".into());
    }
    let old = directory.read(older)?;
    let new = directory.read(newer)?;
    if old.summary.root != new.summary.root
        || !compatible_backends(&old.summary.backend, &new.summary.backend)
        || !matches_catalog(&old.summary, a)
        || !matches_catalog(&new.summary, b)
    {
        return Err("History metadata changed or is no longer compatible; restart history before comparing.".into());
    }
    let mut c = Comparison {
        root: b.root.clone(),
        older_id: older,
        newer_id: newer,
        newer_generation: b.live_generation,
        before: a.logical,
        after: b.logical,
        delta: i128::from(b.logical) - i128::from(a.logical),
        added: 0,
        removed: 0,
        modified: 0,
        moved: 0,
        total_changes: 0,
        rows: Vec::new(),
        folder_changes: Vec::new(),
    };
    let mut old_used = vec![false; old.nodes.len()];
    let mut new_used = vec![false; new.nodes.len()];
    for (j, matched) in matching_paths(&old, &new).into_iter().enumerate() {
        if let Some(i) = matched {
            let i = i as usize;
            let before = &old.nodes[i];
            let after = &new.nodes[j];
            if before.is_dir != after.is_dir {
                continue;
            }
            old_used[i] = true;
            new_used[j] = true;
            if after.is_dir {
                let delta = i128::from(after.logical) - i128::from(before.logical);
                if delta != 0 {
                    add_folder(
                        &mut c,
                        FolderChange {
                            path: saved_path(&new, j),
                            before: before.logical,
                            after: after.logical,
                            delta,
                        },
                    );
                }
            } else if before.logical != after.logical
                || before.modified != after.modified
                || before.allocated != after.allocated
                || before.identity != after.identity
            {
                add_row(
                    &mut c,
                    ChangeKind::Modified,
                    saved_path(&new, j),
                    None,
                    before.logical,
                    after.logical,
                );
            }
        }
    }
    // The hierarchy index has been dropped before building either identity map.
    let old_ids = unique_identities(&old);
    let new_ids = unique_identities(&new);
    for (id, old_single) in &old_ids {
        let (Some(i), Some(Some(j))) = (old_single, new_ids.get(id)) else {
            continue;
        };
        let (i, j) = (*i, *j);
        if !old_used[i] && !new_used[j] {
            old_used[i] = true;
            new_used[j] = true;
            add_row(
                &mut c,
                ChangeKind::Moved,
                saved_path(&new, j),
                Some(saved_path(&old, i)),
                old.nodes[i].logical,
                new.nodes[j].logical,
            );
        }
    }
    drop(old_ids);
    drop(new_ids);
    for (i, n) in old.nodes.iter().enumerate().skip(1) {
        if !old_used[i] {
            add_row(
                &mut c,
                ChangeKind::Removed,
                saved_path(&old, i),
                None,
                if n.is_dir { 0 } else { n.logical },
                0,
            );
            if n.is_dir && n.logical > 0 {
                add_folder(
                    &mut c,
                    FolderChange {
                        path: saved_path(&old, i),
                        before: n.logical,
                        after: 0,
                        delta: -i128::from(n.logical),
                    },
                );
            }
        }
    }
    for (j, n) in new.nodes.iter().enumerate().skip(1) {
        if !new_used[j] {
            add_row(
                &mut c,
                ChangeKind::Added,
                saved_path(&new, j),
                None,
                0,
                if n.is_dir { 0 } else { n.logical },
            );
            if n.is_dir && n.logical > 0 {
                add_folder(
                    &mut c,
                    FolderChange {
                        path: saved_path(&new, j),
                        before: 0,
                        after: n.logical,
                        delta: i128::from(n.logical),
                    },
                );
            }
        }
    }
    Ok(c)
}
fn add_row(
    c: &mut Comparison,
    kind: ChangeKind,
    path: PathBuf,
    previous_path: Option<PathBuf>,
    before: u64,
    after: u64,
) {
    match kind {
        ChangeKind::Added => c.added += 1,
        ChangeKind::Removed => c.removed += 1,
        ChangeKind::Modified => c.modified += 1,
        ChangeKind::Moved => c.moved += 1,
    };
    c.total_changes += 1;
    let row = ChangeRow {
        kind,
        path,
        previous_path,
        before,
        after,
        delta: i128::from(after) - i128::from(before),
    };
    let position = c.rows.partition_point(|r| {
        r.delta.abs() > row.delta.abs() || (r.delta.abs() == row.delta.abs() && r.path < row.path)
    });
    if position >= 256 {
        return;
    }
    c.rows.insert(position, row);
    c.rows.truncate(256);
}
fn add_folder(c: &mut Comparison, folder: FolderChange) {
    let position = c.folder_changes.partition_point(|f| {
        f.delta.abs() > folder.delta.abs()
            || (f.delta.abs() == folder.delta.abs() && f.path < folder.path)
    });
    if position >= 8 {
        return;
    }
    c.folder_changes.insert(position, folder);
    c.folder_changes.truncate(8);
}
fn compatible_backends(a: &str, b: &str) -> bool {
    let family = |s: &str| {
        if s == "Normal" || s.starts_with("Normal (") || s.starts_with("Normal fallback (") {
            1
        } else if s.starts_with("NTFS volume preview (") {
            2
        } else {
            0
        }
    };
    match (family(a), family(b)) {
        (1, 1) | (2, 2) => true,
        (0, 0) => a == b,
        _ => false,
    }
}
fn matches_catalog(disk: &ScanSummary, catalog: &ScanSummary) -> bool {
    disk.id == catalog.id
        && disk.root == catalog.root
        && disk.finished_unix == catalog.finished_unix
        && disk.logical == catalog.logical
        && disk.files == catalog.files
        && disk.folders == catalog.folders
        && disk.backend == catalog.backend
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_history_rejects_requests_without_background_work() {
        let reason = "History is disabled while running as administrator.";
        let service = HistoryService::unavailable(reason);
        let snapshot = service.poll().expect("initial unavailable status");
        assert_eq!(snapshot.message, reason);
        assert!(!snapshot.busy);
        assert!(snapshot.scans.is_empty());
        assert!(snapshot.comparison.is_none());
        assert!(service.poll().is_none());

        // Pure in-memory metadata: no directory, environment override or file fixture.
        let model = Arc::new(Model::new(PathBuf::from("unavailable-history-test")));
        let capture = service.capture();
        assert!(!capture.submit(model.clone(), 1, ScanState::Complete, "Normal".into()));
        assert_eq!(Arc::strong_count(&model), 1, "must not retain a scan");
        assert!(!service.compare(1, 2));
        assert!(!service.forget_all());
        assert!(matches!(
            service.commands.try_send(Command::Forget),
            Err(crossbeam_channel::TrySendError::Disconnected(_))
        ));
        assert_eq!(
            snapshot.message, reason,
            "requests must preserve the reason"
        );
        assert!(service.poll().is_none(), "there is no background worker");
    }
    fn fixture() -> Saved {
        Saved {
            summary: ScanSummary {
                id: 1,
                root: std::env::current_dir().unwrap().join("fixture"),
                finished_unix: 1,
                logical: 12,
                files: 1,
                folders: 0,
                backend: "Normal".into(),
                live_generation: Some(99),
            },
            nodes: vec![
                SavedNode {
                    parent: None,
                    name: OsString::new(),
                    is_dir: true,
                    reparse: false,
                    logical: 12,
                    allocated: Some(12),
                    modified: None,
                    identity: None,
                },
                SavedNode {
                    parent: Some(0),
                    name: "safe".into(),
                    is_dir: false,
                    reparse: false,
                    logical: 12,
                    allocated: Some(12),
                    modified: Some(42),
                    identity: Some(FileIdentity {
                        volume: 8,
                        id: [7; 16],
                    }),
                },
            ],
        }
    }
    fn signed_mutation(mut bytes: Vec<u8>, offset: usize, value: u32) -> Vec<u8> {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let length = bytes.len() - 8;
        let digest = checksum(&bytes[..length]);
        bytes[length..].copy_from_slice(&digest.to_le_bytes());
        bytes
    }
    #[test]
    fn rejects_schema_policy_lengths_and_inconsistent_totals_even_with_valid_integrity() {
        let bytes = encode(&fixture()).unwrap();
        assert!(
            decode(&signed_mutation(bytes.clone(), 8, SCHEMA + 1))
                .err()
                .unwrap_or_default()
                .contains("schema")
        );
        assert!(decode(&signed_mutation(bytes.clone(), 12, POLICY + 1)).is_err());
        // Root UTF16 length is checked before attempting allocation/read.
        assert!(decode(&signed_mutation(bytes.clone(), 56, u32::MAX)).is_err());
        assert!(decode(&signed_mutation(bytes, 32, 900)).is_err());
    }
    #[test]
    fn rejects_cycles_traversal_native_separators_and_duplicate_paths() {
        for name in ["..", ".", "../outside", "sub\\outside", "C:outside", ""] {
            let mut saved = fixture();
            saved.nodes[1].name = name.into();
            assert!(
                decode(&encode(&saved).unwrap()).is_err(),
                "accepted {name:?}"
            );
        }
        let mut saved = fixture();
        saved.nodes[1].parent = Some(1);
        assert!(decode(&encode(&saved).unwrap()).is_err());
        let mut saved = fixture();
        saved.nodes[1].parent = Some(u32::MAX - 1);
        assert!(decode(&encode(&saved).unwrap()).is_err());
        let mut saved = fixture();
        saved.nodes.push(SavedNode {
            parent: Some(0),
            name: "safe".into(),
            is_dir: false,
            reparse: false,
            logical: 0,
            allocated: Some(0),
            modified: None,
            identity: None,
        });
        assert!(decode(&encode(&saved).unwrap()).is_err());
        let mut saved = fixture();
        saved.summary.root.push("..");
        assert!(decode(&encode(&saved).unwrap()).is_err());
    }
    #[test]
    fn saturated_capture_admission_does_not_wait_or_silently_accept() {
        let (captures, _held_receiver) = bounded(1);
        let rejected = Arc::new(AtomicBool::new(false));
        let capture = HistoryCapture {
            captures,
            rejected: rejected.clone(),
        };
        let model = Arc::new(Model::new(std::env::current_dir().unwrap()));
        assert!(capture.submit(model.clone(), 1, ScanState::Complete, "Normal".into()));
        assert!(!capture.submit(model, 2, ScanState::Complete, "Normal".into()));
        assert!(rejected.load(Ordering::Acquire));
    }
    #[test]
    fn comparison_revalidates_disk_metadata_after_catalog_load() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let directory = Directory::open(temp.path().join("history")).unwrap();
        let old = fixture();
        let mut new = fixture();
        new.summary.id = 2;
        directory.write(1, &encode(&old).unwrap()).unwrap();
        directory.write(2, &encode(&new).unwrap()).unwrap();
        let summaries = vec![old.summary.clone(), new.summary.clone()];
        new.summary.backend = "NTFS volume preview (unbenchmarked)".into();
        fs::write(directory.file(2), encode(&new).unwrap()).unwrap();
        assert!(compare_saved(&directory, &summaries, 1, 2).is_err());
    }
    #[test]
    #[cfg(windows)]
    fn retention_retries_before_storage_admission_after_delete_failure() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let directory = Directory::open(temp.path().join("history")).unwrap();
        let model = Arc::new(Model::new(temp.path().join("scan")));
        let capture = |generation| Capture {
            model: model.clone(),
            generation,
            backend: "Normal".into(),
        };
        let mut snapshot = HistorySnapshot::default();
        for generation in 1..=5 {
            save_capture(&directory, capture(generation), &mut snapshot).unwrap();
        }
        let oldest = snapshot.scans.last().unwrap().id;
        // A real Windows reader without delete sharing prevents retention only;
        // the new snapshot can still be staged and committed normally.
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(directory.file(oldest))
            .unwrap();
        assert!(save_capture(&directory, capture(6), &mut snapshot).is_err());
        assert_eq!(snapshot.scans.len(), 6);
        assert_eq!(directory.owned().unwrap().len(), 6);

        // Reserve the remaining budget without allocating a large metadata tree.
        // A successful retry must recover retention BEFORE admitting new bytes.
        let staging = directory.resolved.join("diskscope-0000000000000000.tmp");
        let reservation = File::create(staging).unwrap();
        reservation
            .set_len(MAX_DISK_BYTES - directory.disk_usage().unwrap())
            .unwrap();
        drop(reservation);
        assert_eq!(directory.disk_usage().unwrap(), MAX_DISK_BYTES);
        drop(reader);

        save_capture(&directory, capture(7), &mut snapshot)
            .expect("releasing the transient reader must recover without restart or Clear");
        assert_eq!(snapshot.scans.len(), 5);
        assert_eq!(snapshot.scans[0].live_generation, Some(7));
        assert_eq!(directory.owned().unwrap().len(), 5);
        assert!(!directory.file(oldest).exists());
        assert!(directory.disk_usage().unwrap() <= MAX_DISK_BYTES);
    }
    #[test]
    #[cfg(windows)]
    fn startup_retention_failure_keeps_catalog_for_later_recovery() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let path = temp.path().join("history");
        let directory = Directory::open(path.clone()).unwrap();
        let mut saved = fixture();
        for id in 1..=6 {
            saved.summary.id = id;
            directory.write(id, &encode(&saved).unwrap()).unwrap();
        }
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(directory.file(1))
            .unwrap();
        let reservation =
            File::create(directory.resolved.join("diskscope-0000000000000000.tmp")).unwrap();
        reservation
            .set_len(MAX_DISK_BYTES - directory.disk_usage().unwrap())
            .unwrap();
        drop(reservation);
        drop(directory);

        let history = HistoryService::new(path.clone());
        let idle = || {
            let started = Instant::now();
            loop {
                if let Some(snapshot) = history.poll()
                    && !snapshot.busy
                {
                    break snapshot;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "history timed out"
                );
                thread::sleep(Duration::from_millis(5));
            }
        };
        let _loaded = idle();
        drop(reader);
        assert!(history.capture().submit(
            Arc::new(Model::new(saved.summary.root)),
            7,
            ScanState::Complete,
            "Normal".into(),
        ));
        let recovered = idle();
        assert_eq!(recovered.scans.len(), 5, "{}", recovered.message);
        assert_eq!(recovered.scans[0].live_generation, Some(7));
        assert!(!path.join("diskscope-0000000000000001.dsh").exists());
    }

    #[test]
    fn exclusive_owner_serializes_staging_budget_reservations_and_clear() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let path = temp.path().join("history");
        let directory = Directory::open(path.clone()).unwrap();
        let staging = directory.resolved.join("diskscope-0000000000000001.tmp");
        let reserved = File::create(&staging).unwrap();
        reserved.set_len(MAX_DISK_BYTES).unwrap();
        drop(reserved);
        assert_eq!(directory.disk_usage().unwrap(), MAX_DISK_BYTES);
        assert!(
            directory
                .write(2, b"exceeds reserved disk budget")
                .unwrap_err()
                .contains("budget")
        );
        assert!(!directory.file(2).exists());
        assert!(matches!(Directory::open(path.clone()),Err(e) if e==OWNED_ELSEWHERE));
        assert_eq!(directory.disk_usage().unwrap(), MAX_DISK_BYTES);
        directory.clear().unwrap();
        assert_eq!(directory.disk_usage().unwrap(), 0);
        // Clear must preserve ownership, so no rival can clear/write during a new reservation.
        assert!(matches!(Directory::open(path.clone()),Err(e) if e==OWNED_ELSEWHERE));
        drop(directory);
        let successor = Directory::open(path).unwrap();
        successor.write(3, b"new owner").unwrap();
        assert_eq!(successor.disk_usage().unwrap(), 9);
    }
}
