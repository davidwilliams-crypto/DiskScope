//! Bounded local metadata history. No file contents are read or stored.
//!
//! # What happens to each file in the history directory
//!
//! The directory is owned exclusively by one window, so nothing else is writing
//! while it is examined. Each `diskscope-<id>.dsh` file gets exactly one fate,
//! decided by evidence that is definitive, never by a failed read:
//!
//! * The header is this build's and the streamed checksum matches: the file is
//!   listed in the catalog.
//! * The header says an older version, schema or policy; or the file is not a
//!   snapshot at all (wrong magic, too short); or the header is ours but the
//!   body is damaged: the file is deleted at startup, because this build can
//!   never read it.
//! * The header says a newer version, schema or policy: the file is kept
//!   untouched. It is budgeted apart and hard-bounded, so it can neither be
//!   destroyed nor wedge saving.
//! * The read failed (sharing violation, I/O error) or the file changed while it
//!   was read: nothing is known, so the file is kept, reported and looked at
//!   again at the next start.
//!
//! Loading never materializes nodes: the summary sits in the first bytes and
//! the checksum is verified by streaming, so memory does not grow with the
//! node count. Structural validation of the nodes is deferred to Compare, which
//! reports a damaged snapshot as an error and never as a result. The number of
//! files in the directory is irrelevant to correctness: it is streamed, and one
//! load reads in full at most `MAX_EXAMINED_FILES` of them.
use crate::model::{FileIdentity, Model, ScanState};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    cmp::{Ordering as Cmp, Reverse},
    collections::{BinaryHeap, HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
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
const MAX_BACKEND_LEN: usize = 512;
const MEBIBYTE: u64 = 1024 * 1024;

// On-disk identity of a snapshot. The magic is the family name plus a two-digit
// version; a newer DiskScope bumps the digits and/or SCHEMA/POLICY. Existing
// DSHIST02 files must stay readable, so none of these values may change.
const MAGIC_FAMILY: &[u8; 6] = b"DSHIST";
const MAGIC: &[u8; 8] = b"DSHIST02";
const MAGIC_VERSION: u32 = 2;
const SCHEMA: u32 = 2;
// Logical bytes per directory entry; allocation per entry; reparse directories not followed.
const POLICY: u32 = 1;
/// Magic, schema word and policy word: all that is needed to say whether a file is ours.
const HEADER_PROBE_LEN: usize = 16;
/// Trailing FNV checksum over every preceding byte.
const CHECKSUM_LEN: usize = 8;
/// Smallest possible node record: parent, flags, logical size, three one-byte
/// presence tags (allocated, modified, identity) and the name length word.
const MIN_NODE_BYTES: usize = 4 + 1 + 8 + 1 + 1 + 1 + 4;
/// Everything before the first node: fixed words, a maximal UTF-16 root, a
/// maximal backend string and the node count. The catalog summary lives here.
const MAX_HEADER_LEN: usize =
    MAGIC.len() + 4 + 4 + 5 * 8 + 4 + MAX_NAME * 2 + 4 + MAX_BACKEND_LEN + 4;
/// Snapshots are verified by streaming through a buffer of this size, so catalog
/// loading needs memory proportional to this constant, never to a file or node count.
const VERIFY_CHUNK_BYTES: usize = 1024 * 1024;
const _: () = assert!(VERIFY_CHUNK_BYTES >= MAX_HEADER_LEN);
const _: () = assert!(MAGIC_VERSION == SCHEMA);

// FNV-1a, 64 bit. Part of the wire format: changing either constant orphans every saved file.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

const SNAPSHOT_SUFFIX: &str = ".dsh";
const STAGING_SUFFIX: &str = ".tmp";

/// One load streams at most this many snapshot bodies in full. Normal operation
/// keeps five (six while a retention delete is pending); the rest is headroom for
/// damaged leftovers. Older files are only triaged by their 16-byte header.
const MAX_EXAMINED_FILES: usize = 16;
/// Files written by a newer DiskScope are never deleted automatically, so they
/// are budgeted separately from this build's own snapshots (see `Usage`). Past
/// either bound saving is refused instead of growing the disk footprint without limit.
const MAX_FOREIGN_FILES: usize = 64;
const MAX_FOREIGN_BYTES: u64 = MAX_DISK_BYTES;
const OWNED_ELSEWHERE: &str = concat!(
    "History is unavailable because another window owns this directory. ",
    "Close that window and reopen history."
);
const REQUEST_SKIPPED: &str = concat!(
    "History request skipped: scan incomplete/oversized, ",
    "or background queue busy. Retry when idle."
);
const REQUESTS_WERE_SKIPPED: &str = concat!(
    " Some history requests were skipped because the scan was ",
    "incomplete/oversized or the background queue was busy."
);
const COMPARISON_NOTE: &str = concat!(
    "Comparison uses logical bytes per entry. Moves require unique known identity; ",
    "hardlink aliases and unknown identities remain additions/removals."
);
const SAVED_NOTE: &str = concat!(
    "Saved complete metadata locally; five snapshots maximum, 128 MiB each, ",
    "768 MiB including staging. No file contents stored."
);
const INCOMPATIBLE_SCANS: &str = concat!(
    "Choose two compatible scans of the identical native root and backend family ",
    "(normal and raw NTFS preview are separate)."
);
const CATALOG_STALE: &str = concat!(
    "History metadata changed or is no longer compatible; ",
    "restart history before comparing."
);
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
            snapshot.message = REQUEST_SKIPPED.into();
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
            Ok(Command::Forget) => {
                let cleared = directory.clear();
                // Clear removes every file it can; it only fails for files that
                // are in use, and those must not stay listed once they are gone.
                forget_vanished_scans(&directory, &mut snapshot);
                cleared.map(|()| {
                    // A queued completion belongs to the state the user just cleared.
                    let _ = captures.try_recv();
                    snapshot.message = "Local metadata history cleared.".into();
                })
            }
            Ok(Command::Compare(older, newer)) => {
                snapshot.comparison = None;
                compare_saved(&directory, &snapshot.scans, older, newer).map(|comparison| {
                    snapshot.comparison = Some(Arc::new(comparison));
                    snapshot.message = COMPARISON_NOTE.into();
                })
            }
        };
        if let Err(error) = result {
            snapshot.message = error;
        }
        if std::mem::take(&mut skipped) {
            snapshot.message.push_str(REQUESTS_WERE_SKIPPED);
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
    snapshot.message = SAVED_NOTE.into();
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

/// After a Clear that removed all or only some files: drops catalog entries
/// whose files are gone, and a comparison that depended on one of them.
fn forget_vanished_scans(directory: &Directory, snapshot: &mut HistorySnapshot) {
    snapshot
        .scans
        .retain(|scan| directory.file(scan.id).exists());
    let still_listed = |id: u64| snapshot.scans.iter().any(|scan| scan.id == id);
    if let Some(comparison) = &snapshot.comparison
        && !(still_listed(comparison.older_id) && still_listed(comparison.newer_id))
    {
        snapshot.comparison = None;
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

/// A regular file in the history directory whose name follows this app's pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OwnedFile {
    id: u64,
    len: u64,
}

/// Keeps the newest `capacity` files offered, so a directory holding any number
/// of files costs memory proportional to `capacity` only.
struct NewestFiles {
    capacity: usize,
    kept: BinaryHeap<Reverse<OwnedFile>>,
    offered: usize,
}
impl NewestFiles {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            kept: BinaryHeap::with_capacity(capacity + 1),
            offered: 0,
        }
    }
    fn offer(&mut self, file: OwnedFile) {
        self.offered += 1;
        self.kept.push(Reverse(file));
        if self.kept.len() > self.capacity {
            // `Reverse` makes the heap's maximum the oldest file.
            self.kept.pop();
        }
    }
    fn into_newest_first(self) -> Vec<OwnedFile> {
        // Ascending order of `Reverse<T>` is descending order of `T`.
        self.kept
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse(file)| file)
            .collect()
    }
}

/// Disk accounting for the write budget.
///
/// `counted` is what the 768 MiB budget applies to: this build's snapshots
/// (including ones it could not read, which may still be valid) and staging
/// files. Files written by a newer DiskScope are tallied apart, because this
/// build must never delete them and so cannot let them wedge saving.
#[derive(Debug, Default, PartialEq, Eq)]
struct Usage {
    counted: u64,
    foreign_files: usize,
    foreign_bytes: u64,
}

/// Admission control for one new snapshot of `incoming` bytes.
fn check_write_budget(usage: &Usage, incoming: usize) -> Result<(), String> {
    let fits = usage
        .counted
        .checked_add(incoming as u64)
        .is_some_and(|total| total <= MAX_DISK_BYTES);
    if incoming > MAX_SNAPSHOT_BYTES || !fits {
        return Err("History skipped: metadata storage budget exceeded".into());
    }
    if usage.foreign_files > MAX_FOREIGN_FILES || usage.foreign_bytes > MAX_FOREIGN_BYTES {
        return Err(format!(
            "History skipped: the history folder holds {} written by a newer DiskScope ({}). \
             This version keeps them untouched but will not save beside more than \
             {MAX_FOREIGN_FILES} files or {} MiB of them. Clear saved scan history, \
             or use the newer DiskScope, to continue.",
            count(usage.foreign_files, "history file", "history files"),
            format_mebibytes(usage.foreign_bytes),
            MAX_FOREIGN_BYTES / MEBIBYTE,
        ));
    }
    Ok(())
}

/// Outcome of deleting a batch of files. A failed delete is never fatal: the
/// file stays on disk, keeps counting against the budget and is retried later.
#[derive(Debug, Default, PartialEq, Eq)]
struct Removals {
    removed: usize,
    stuck: usize,
}
impl Removals {
    fn record(&mut self, removed: bool) {
        if removed {
            self.removed += 1;
        } else {
            self.stuck += 1;
        }
    }
}

/// What one catalog load did and found, in a form the status message and the
/// tests can both read.
#[derive(Debug, Default, PartialEq, Eq)]
struct LoadReport {
    /// Snapshot files this build can never read (older format, damaged, not a
    /// history file at all), deleted.
    unusable: Removals,
    /// Intact snapshots beyond the five retained, deleted without a full read.
    surplus: Removals,
    /// Staging files left by an interrupted write, deleted.
    staging: Removals,
    /// Files from a newer DiskScope, left alone.
    kept_newer: usize,
    /// Files that could not be read or examined, left alone.
    unreadable: usize,
    first_unreadable: Option<String>,
    /// Snapshot bodies streamed in full.
    examined: usize,
    /// Intact-looking snapshots older than the examined window, left alone.
    unexamined: usize,
}
impl LoadReport {
    fn note_unreadable(&mut self, reason: &str) {
        self.unreadable += 1;
        self.first_unreadable
            .get_or_insert_with(|| reason.to_owned());
    }
    fn message(&self) -> String {
        let mut parts = vec![String::from(
            "Local metadata history ready. Five snapshots maximum; no file contents stored.",
        )];
        if self.unusable.removed > 0 {
            parts.push(format!(
                "Removed {} unreadable {} (corrupt or incompatible format).",
                self.unusable.removed,
                plural(self.unusable.removed, "history file", "history files"),
            ));
        }
        if self.surplus.removed > 0 {
            parts.push(format!(
                "Removed {} older history {} beyond the {MAX_SNAPSHOTS} retained.",
                self.surplus.removed,
                plural(self.surplus.removed, "file", "files"),
            ));
        }
        if self.staging.removed > 0 {
            parts.push(format!(
                "Removed {} interrupted history {}.",
                self.staging.removed,
                plural(self.staging.removed, "write", "writes"),
            ));
        }
        let stuck = self.unusable.stuck + self.surplus.stuck + self.staging.stuck;
        if stuck > 0 {
            parts.push(format!(
                "{} could not be removed (in use?) and will be retried at the next start.",
                count(stuck, "history file", "history files"),
            ));
        }
        if self.kept_newer > 0 {
            parts.push(format!(
                "Kept {} written by a newer DiskScope; this version cannot read {}.",
                count(self.kept_newer, "history file", "history files"),
                if self.kept_newer == 1 { "it" } else { "them" },
            ));
        }
        if let Some(reason) = &self.first_unreadable {
            parts.push(format!(
                "Could not read {} (kept, not deleted): {reason}",
                count(self.unreadable, "history file", "history files"),
            ));
        }
        if self.unexamined > 0 {
            parts.push(format!(
                "{} older than the newest {MAX_EXAMINED_FILES} {} not examined; \
                 Clear saved scan history removes {}.",
                count(self.unexamined, "history file", "history files"),
                if self.unexamined == 1 { "was" } else { "were" },
                if self.unexamined == 1 { "it" } else { "them" },
            ));
        }
        parts.join(" ")
    }
}
fn plural<'a>(n: usize, singular: &'a str, many: &'a str) -> &'a str {
    if n == 1 { singular } else { many }
}
fn count(n: usize, singular: &str, many: &str) -> String {
    format!("{n} {}", plural(n, singular, many))
}
fn format_mebibytes(bytes: u64) -> String {
    format!("{} MiB", bytes.div_ceil(MEBIBYTE))
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
    fn staging_file(&self, id: u64) -> PathBuf {
        self.resolved.join(format!("diskscope-{id:016x}.tmp"))
    }
    /// Streams the regular files named `diskscope-<16 hex digits><suffix>`.
    /// Nothing is collected here, so any number of files costs constant memory;
    /// links and directories with such names are not ours to touch and are
    /// skipped. `visit` may delete the file it is handed.
    fn for_each_owned(
        &self,
        suffix: &str,
        mut visit: impl FnMut(OwnedFile) -> Result<(), String>,
    ) -> Result<(), String> {
        self.check()?;
        for entry in fs::read_dir(&self.resolved).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let Some(id) = owned_id_with_suffix(&entry.file_name(), suffix) else {
                continue;
            };
            let meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                // Removed since it was listed; nothing left to account for.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
            };
            if !meta.file_type().is_file() || meta.file_type().is_symlink() {
                continue;
            }
            visit(OwnedFile {
                id,
                len: meta.len(),
            })?;
        }
        Ok(())
    }
    /// Loads the catalog. The directory is exclusively owned, so nothing else
    /// can be writing while this runs.
    fn load(&self) -> Result<(Vec<ScanSummary>, String), String> {
        let (scans, report) = self.load_catalog()?;
        Ok((scans, report.message()))
    }
    /// Reclamation policy. A file is deleted automatically only on a definitive
    /// verdict that this build can never use it:
    ///
    /// * its header says an older format, or it is not a DiskScope history file
    ///   at all (wrong magic, too short to hold a header), or
    /// * its header is ours but the body fails the integrity check or the
    ///   header is malformed.
    ///
    /// Never deleted: a file whose header says a NEWER format (the user may
    /// still run the newer DiskScope; such files are budgeted apart and capped),
    /// and any file that could not be read (sharing violation, I/O error, size
    /// changing under the read). A failed read is not a verdict.
    ///
    /// Reads are bounded: every file costs one 16-byte header probe; at most
    /// `MAX_EXAMINED_FILES` of the newest candidates are streamed through the
    /// checksum, in constant memory. Nodes are never materialized here; full
    /// structural validation is deferred to Compare, which fails loudly.
    fn load_catalog(&self) -> Result<(Vec<ScanSummary>, LoadReport), String> {
        let mut report = LoadReport {
            staging: self.prune_staging()?,
            ..Default::default()
        };
        // Pass 1: classify every snapshot by header. Unusable ones are deleted
        // as they are met; only the newest candidates are remembered.
        let mut newest = NewestFiles::new(MAX_EXAMINED_FILES);
        self.triage(&mut report, |file| newest.offer(file))?;
        let candidates = newest.offered;
        let window = newest.into_newest_first();
        // Pass 2: verify the newest candidates by streaming.
        let mut scans = Vec::with_capacity(window.len());
        for file in &window {
            self.check()?;
            report.examined += 1;
            match verify_snapshot(&self.file(file.id), file.id) {
                Verdict::Usable(summary) => scans.push(summary),
                Verdict::Damaged => self.reclaim(file.id, &mut report.unusable)?,
                Verdict::Unreadable(reason) => report.note_unreadable(&reason),
            }
        }
        // Pass 3: intact-looking files older than the window. Once five newer
        // snapshots are verified they are retention surplus. Otherwise they may
        // be the only usable ones, so they are left for a later load.
        let overflow = candidates - window.len();
        if overflow > 0
            && let Some(oldest_examined) = window.last()
        {
            if scans.len() >= MAX_SNAPSHOTS {
                report.surplus = self.remove_surplus(oldest_examined.id)?;
            }
            report.unexamined = overflow.saturating_sub(report.surplus.removed);
        }
        Ok((scans, report))
    }
    /// Classifies every snapshot file by its header alone and settles the
    /// verdicts that need no further reading. Files whose header is ours are
    /// handed to `candidate`.
    fn triage(
        &self,
        report: &mut LoadReport,
        mut candidate: impl FnMut(OwnedFile),
    ) -> Result<(), String> {
        self.for_each_owned(SNAPSHOT_SUFFIX, |file| {
            let format = match probe_header(&self.file(file.id)) {
                Ok(format) => format,
                Err(error) => {
                    // A failed read says nothing about the format: never delete on it.
                    report.note_unreadable(&error.to_string());
                    return Ok(());
                }
            };
            if format == HeaderFormat::Current {
                candidate(file);
            } else if format.reclaimable() {
                self.reclaim(file.id, &mut report.unusable)?;
            } else {
                debug_assert_eq!(format, HeaderFormat::Newer);
                report.kept_newer += 1;
            }
            Ok(())
        })
    }
    /// Deletes intact-format snapshots older than `below`. Only call this once
    /// five newer snapshots are known to be usable.
    fn remove_surplus(&self, below: u64) -> Result<Removals, String> {
        let mut removals = Removals::default();
        self.for_each_owned(SNAPSHOT_SUFFIX, |file| {
            if file.id >= below {
                return Ok(());
            }
            // Anything but a header of ours (newer format, unreadable) stays.
            if matches!(probe_header(&self.file(file.id)), Ok(HeaderFormat::Current)) {
                self.reclaim(file.id, &mut removals)?;
            }
            Ok(())
        })?;
        Ok(removals)
    }
    /// Deletes one snapshot through the same guards as `remove`. Only a failed
    /// directory check is an error; a file that cannot be deleted right now is
    /// recorded and kept.
    fn reclaim(&self, id: u64, removals: &mut Removals) -> Result<(), String> {
        removals.record(self.try_remove(&self.file(id))?);
        Ok(())
    }
    /// Removes staging files left by an interrupted write. This directory is
    /// exclusively owned, so no other window can be mid-write; without this,
    /// an orphaned 128 MiB `.tmp` would count against the disk budget until
    /// the user clears all history.
    fn prune_staging(&self) -> Result<Removals, String> {
        let mut removals = Removals::default();
        self.for_each_owned(STAGING_SUFFIX, |file| {
            removals.record(self.try_remove(&self.staging_file(file.id))?);
            Ok(())
        })?;
        Ok(removals)
    }
    /// Reads and fully validates one snapshot. Compare uses this: a body that
    /// fails the checksum or the structural validation is an error, never a result.
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
        // Sized up front: growing a 128 MiB vector by doubling would briefly hold twice that.
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
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
        check_write_budget(&self.usage()?, bytes.len())?;
        let staging = self.staging_file(id);
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
        remove_regular_file(&self.file(id))
    }
    /// `remove` for bulk reclamation: `Err` only when the directory guard
    /// fails (stop everything); `Ok(false)` when this one file could not be
    /// deleted (for example a Windows sharing violation) and should be kept.
    fn try_remove(&self, path: &Path) -> Result<bool, String> {
        self.check()?;
        Ok(remove_regular_file(path).is_ok())
    }
    /// Removes every snapshot and staging file, however many there are.
    /// Streams the directory, so the file count cannot make it fail; a file
    /// that cannot be deleted does not stop the others.
    fn clear(&self) -> Result<(), String> {
        let mut snapshots = Removals::default();
        self.for_each_owned(SNAPSHOT_SUFFIX, |file| {
            self.reclaim(file.id, &mut snapshots)
        })?;
        let staging = self.prune_staging()?;
        let stuck = snapshots.stuck + staging.stuck;
        if stuck > 0 {
            return Err(format!(
                "Could not remove {} (in use?). \
                 Close other programs using the history folder and clear again.",
                count(stuck, "history file", "history files"),
            ));
        }
        Ok(())
    }
    /// Streams the directory once. Snapshot headers decide which files this
    /// build must budget for (see `Usage`).
    fn usage(&self) -> Result<Usage, String> {
        let mut usage = Usage::default();
        self.for_each_owned(SNAPSHOT_SUFFIX, |file| {
            // A file that cannot be probed is counted: it may well be one of ours.
            if matches!(probe_header(&self.file(file.id)), Ok(HeaderFormat::Newer)) {
                usage.foreign_files += 1;
                usage.foreign_bytes = usage.foreign_bytes.saturating_add(file.len);
            } else {
                usage.counted = usage.counted.saturating_add(file.len);
            }
            Ok(())
        })?;
        self.for_each_owned(STAGING_SUFFIX, |file| {
            usage.counted = usage.counted.saturating_add(file.len);
            Ok(())
        })?;
        Ok(usage)
    }
}
/// The one place a snapshot or staging file is deleted: it must be a plain
/// file, never a link or directory someone planted under that name.
fn remove_regular_file(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.file_type().is_symlink() {
        return Err("Refusing to remove a history link or directory".into());
    }
    fs::remove_file(path).map_err(|e| e.to_string())
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

/// The checks on a summary that need no nodes. The catalog applies them at
/// startup; `validate` applies them again with the full hierarchy. Returns the
/// root as native UTF-16 units.
fn validate_root(summary: &ScanSummary) -> Result<Vec<u16>, String> {
    if !summary.root.is_absolute()
        || summary
            .root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || summary.backend.len() > MAX_BACKEND_LEN
    {
        return Err("Invalid history root, backend or node count".into());
    }
    let root = wide(summary.root.as_os_str());
    if root.len() > MAX_NAME || root.contains(&0) {
        return Err("Invalid native root".into());
    }
    Ok(root)
}

fn validate(saved: &Saved) -> Result<(), String> {
    if saved.nodes.is_empty() || saved.nodes.len() > MAX_NODES {
        return Err("Invalid history root, backend or node count".into());
    }
    let root = validate_root(&saved.summary)?;
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
        if out.len() > MAX_SNAPSHOT_BYTES - CHECKSUM_LEN {
            return Err("History skipped: metadata exceeds 128 MiB snapshot limit".into());
        }
    }
    let checksum = checksum(&out);
    put_u64(&mut out, checksum);
    Ok(out)
}
fn checksum(bytes: &[u8]) -> u64 {
    fnv1a(FNV_OFFSET_BASIS, bytes)
}
/// Integrity check for accidental corruption, not an authentication boundary.
/// Incremental, so a snapshot can be verified chunk by chunk.
fn fnv1a(state: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(state, |h, b| (h ^ u64::from(*b)).wrapping_mul(FNV_PRIME))
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
/// Reads everything before the first node: identity words, summary and node
/// count. Shared by `decode` and the streaming catalog check, so the two cannot
/// disagree about what a header means. `body_len` is the length of the file
/// without its trailing checksum.
fn parse_header(c: &mut Cursor, body_len: usize) -> Result<(ScanSummary, usize), String> {
    if c.take(MAGIC.len())? != MAGIC || c.u32()? != SCHEMA || c.u32()? != POLICY {
        return Err("Incompatible history schema or accounting policy".into());
    }
    let id = c.u64()?;
    let finished_unix = c.u64()?;
    let logical = c.u64()?;
    let files = c.u64()?;
    let folders = c.u64()?;
    let root = PathBuf::from(c.wide()?);
    let len = c.u32()? as usize;
    if len > MAX_BACKEND_LEN {
        return Err("History backend too long".into());
    }
    let backend = String::from_utf8(c.take(len)?.to_vec()).map_err(|e| e.to_string())?;
    let count = c.u32()? as usize;
    if count == 0 || count > MAX_NODES || count > body_len / MIN_NODE_BYTES {
        return Err("Invalid history node count".into());
    }
    let summary = ScanSummary {
        id,
        root,
        finished_unix,
        logical,
        files,
        folders,
        backend,
        live_generation: None,
    };
    Ok((summary, count))
}
fn decode(bytes: &[u8]) -> Result<Saved, String> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err("Oversized history".into());
    }
    if bytes.len() < CHECKSUM_LEN {
        return Err("Truncated history".into());
    }
    let (bytes, tail) = bytes.split_at(bytes.len() - CHECKSUM_LEN);
    if checksum(bytes) != u64::from_le_bytes(tail.try_into().unwrap()) {
        return Err("History integrity check failed".into());
    }
    let mut c = Cursor { bytes, at: 0 };
    let (summary, count) = parse_header(&mut c, bytes.len())?;
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
    let saved = Saved { summary, nodes };
    validate(&saved)?;
    Ok(saved)
}

/// What a file's first 16 bytes say about it. This is the only evidence used to
/// decide, without reading the body, whether a file may ever be deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeaderFormat {
    /// Magic, schema and accounting policy are exactly this build's.
    Current,
    /// Same family, older version, schema or policy. A newer build supersedes
    /// it and this one cannot read it.
    Older,
    /// Same family, newer version, schema or policy (or a version this build
    /// cannot even parse). Written by a DiskScope the user may still run.
    Newer,
    /// Not a snapshot of any version: wrong magic, or too short to hold a header.
    Unrecognized,
}
impl HeaderFormat {
    /// Only these two verdicts are definitive and permanent, so only they may
    /// lead to deletion. `Newer` must be preserved; `Current` still needs its
    /// body verified.
    fn reclaimable(self) -> bool {
        matches!(self, Self::Older | Self::Unrecognized)
    }
}
/// Parses the two decimal digits of `DSHIST<nn>`.
fn magic_version(magic: &[u8]) -> Option<u32> {
    let digits = magic.strip_prefix(MAGIC_FAMILY.as_slice())?;
    let &[tens, ones] = digits else { return None };
    if !tens.is_ascii_digit() || !ones.is_ascii_digit() {
        return None;
    }
    Some(u32::from(tens - b'0') * 10 + u32::from(ones - b'0'))
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let word = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(word.try_into().ok()?))
}
/// `head` is the start of the file: up to `HEADER_PROBE_LEN` bytes, fewer if
/// the file is shorter.
fn classify_header(head: &[u8]) -> HeaderFormat {
    let Some(magic) = head.get(..MAGIC.len()) else {
        return HeaderFormat::Unrecognized;
    };
    if !magic.starts_with(MAGIC_FAMILY) {
        return HeaderFormat::Unrecognized;
    }
    // Same family. If the version digits cannot be read, assume newer: not
    // knowing what a file is must never make it deletable.
    let Some(version) = magic_version(magic) else {
        return HeaderFormat::Newer;
    };
    match version.cmp(&MAGIC_VERSION) {
        Cmp::Greater => return HeaderFormat::Newer,
        Cmp::Less => return HeaderFormat::Older,
        Cmp::Equal => {}
    }
    let (Some(schema), Some(policy)) = (u32_at(head, MAGIC.len()), u32_at(head, MAGIC.len() + 4))
    else {
        return HeaderFormat::Unrecognized;
    };
    // Lexicographic: a newer schema wins over any policy, and so on.
    match (schema, policy).cmp(&(SCHEMA, POLICY)) {
        Cmp::Equal => HeaderFormat::Current,
        Cmp::Less => HeaderFormat::Older,
        Cmp::Greater => HeaderFormat::Newer,
    }
}
/// Reads at most 16 bytes. An `Err` is a failed read, which is not a verdict.
fn probe_header(path: &Path) -> io::Result<HeaderFormat> {
    let mut head = Vec::with_capacity(HEADER_PROBE_LEN);
    File::open(path)?
        .take(HEADER_PROBE_LEN as u64)
        .read_to_end(&mut head)?;
    Ok(classify_header(&head))
}

/// What streaming through one snapshot file concluded.
#[derive(Debug)]
enum Verdict {
    /// The checksum matches and the summary is well formed; usable for the
    /// catalog. Node structure is validated later, when Compare decodes the file.
    Usable(ScanSummary),
    /// Definitive: the bytes were read and they are not a valid snapshot (bad
    /// checksum, malformed header, wrong id, over the size limit).
    Damaged,
    /// Nothing is known: a failed read, or a file that changed while being
    /// read. Never grounds for deletion.
    Unreadable(String),
}
/// Verifies a snapshot without materializing it. Memory is one buffer of at
/// most `VERIFY_CHUNK_BYTES`, however many nodes the file holds.
fn verify_snapshot(path: &Path, id: u64) -> Verdict {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return Verdict::Unreadable(error.to_string()),
    };
    let len = match file.metadata() {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(_) => return Verdict::Unreadable("not a regular file".into()),
        Err(error) => return Verdict::Unreadable(error.to_string()),
    };
    verify_stream(&mut file, len, id, VERIFY_CHUNK_BYTES)
}
/// Running checksum over everything but the final `CHECKSUM_LEN` bytes, which
/// are collected on the side as the stored value.
struct StreamChecksum {
    hash: u64,
    body_left: u64,
    stored: Vec<u8>,
}
impl StreamChecksum {
    fn new(body_len: u64) -> Self {
        Self {
            hash: FNV_OFFSET_BASIS,
            body_left: body_len,
            stored: Vec::with_capacity(CHECKSUM_LEN),
        }
    }
    fn absorb(&mut self, chunk: &[u8]) {
        let in_body =
            usize::try_from(self.body_left).map_or(chunk.len(), |left| left.min(chunk.len()));
        let (body, tail) = chunk.split_at(in_body);
        self.hash = fnv1a(self.hash, body);
        self.body_left -= body.len() as u64;
        self.stored.extend_from_slice(tail);
    }
    fn matches(&self) -> bool {
        <[u8; CHECKSUM_LEN]>::try_from(self.stored.as_slice())
            .is_ok_and(|stored| u64::from_le_bytes(stored) == self.hash)
    }
}
/// `reader` yields the `len` bytes of a file that was classified as
/// `HeaderFormat::Current`. `chunk_len` is a parameter so tests can exercise
/// every chunk boundary; it never drops below `MAX_HEADER_LEN`.
fn verify_stream(reader: &mut impl Read, len: u64, id: u64, chunk_len: usize) -> Verdict {
    let unreadable = |error: io::Error| Verdict::Unreadable(error.to_string());
    let chunk_len = chunk_len.max(MAX_HEADER_LEN);
    let buffer_len = usize::try_from(len).map_or(chunk_len, |len| len.min(chunk_len));
    let mut buffer = vec![0u8; buffer_len];

    // The first read holds the whole header, so the summary needs no second pass.
    let head_len = buffer_len.min(MAX_HEADER_LEN);
    if let Err(error) = reader.read_exact(&mut buffer[..head_len]) {
        return unreadable(error);
    }
    let head = &buffer[..head_len];
    if classify_header(head) != HeaderFormat::Current {
        // It was Current when it was triaged; it is something else now.
        return Verdict::Unreadable("history file changed while it was being examined".into());
    }
    if len > MAX_SNAPSHOT_BYTES as u64 {
        return Verdict::Damaged;
    }
    let body_len = len.saturating_sub(CHECKSUM_LEN as u64);
    let summary = {
        let body_usize = usize::try_from(body_len).unwrap_or(usize::MAX);
        // The checksum may share the first read with the header of a tiny file.
        let mut cursor = Cursor {
            bytes: &head[..head.len().min(body_usize)],
            at: 0,
        };
        match parse_header(&mut cursor, body_usize) {
            Ok((summary, _)) if summary.id == id && validate_root(&summary).is_ok() => summary,
            _ => return Verdict::Damaged,
        }
    };

    let mut checksum = StreamChecksum::new(body_len);
    checksum.absorb(head);
    let mut offset = head_len as u64;
    while offset < len {
        let want = usize::try_from(len - offset).map_or(chunk_len, |left| left.min(chunk_len));
        if let Err(error) = reader.read_exact(&mut buffer[..want]) {
            return unreadable(error);
        }
        checksum.absorb(&buffer[..want]);
        offset += want as u64;
    }
    // Bytes beyond the measured length mean the file is not what was checked.
    match reader.read(&mut buffer[..1]) {
        Ok(0) => {}
        Ok(_) => {
            return Verdict::Unreadable("history file grew while it was being examined".into());
        }
        Err(error) => return unreadable(error),
    }
    if checksum.matches() {
        Verdict::Usable(summary)
    } else {
        Verdict::Damaged
    }
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
        return Err(INCOMPATIBLE_SCANS.into());
    }
    let old = directory.read(older)?;
    let new = directory.read(newer)?;
    if old.summary.root != new.summary.root
        || !compatible_backends(&old.summary.backend, &new.summary.backend)
        || !matches_catalog(&old.summary, a)
        || !matches_catalog(&new.summary, b)
    {
        return Err(CATALOG_STALE.into());
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
    fn snapshot_ids(directory: &Directory) -> Vec<u64> {
        let mut ids = Vec::new();
        directory
            .for_each_owned(SNAPSHOT_SUFFIX, |file| {
                ids.push(file.id);
                Ok(())
            })
            .unwrap();
        ids.sort_unstable();
        ids
    }
    fn counted_bytes(directory: &Directory) -> u64 {
        directory.usage().unwrap().counted
    }
    fn open_directory(temp: &tempfile::TempDir) -> Directory {
        Directory::open(temp.path().join("history")).unwrap()
    }
    /// A valid snapshot file for `id`, written the way the app writes it.
    fn valid_bytes(id: u64) -> Vec<u8> {
        let mut saved = fixture();
        saved.summary.id = id;
        encode(&saved).unwrap()
    }
    /// 16 header bytes: magic, schema word, policy word.
    fn header(magic: &[u8; 8], schema: u32, policy: u32) -> Vec<u8> {
        let mut bytes = magic.to_vec();
        bytes.extend(schema.to_le_bytes());
        bytes.extend(policy.to_le_bytes());
        bytes
    }
    /// Valid except for one flipped bit in the body: the header is ours, the checksum is not.
    fn damaged_bytes(id: u64) -> Vec<u8> {
        let mut bytes = valid_bytes(id);
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        bytes
    }
    /// A valid snapshot of exactly `target` bytes: a root, `files` equal-sized
    /// file nodes, and a backend string padded by single bytes.
    fn snapshot_with_len(id: u64, target: usize) -> Vec<u8> {
        const BYTES_PER_FILE: usize = 42;
        let sized = |files: usize, padding: usize| {
            let mut saved = fixture();
            saved.summary.id = id;
            saved.summary.backend = "x".repeat(padding);
            saved.summary.logical = files as u64;
            saved.summary.files = files as u64;
            saved.nodes.truncate(1);
            saved.nodes[0].logical = files as u64;
            for index in 0..files {
                saved.nodes.push(SavedNode {
                    parent: Some(0),
                    name: format!("f{index:06}").into(),
                    is_dir: false,
                    reparse: false,
                    logical: 1,
                    allocated: Some(1),
                    modified: None,
                    identity: None,
                });
            }
            encode(&saved).unwrap()
        };
        let extra = target - sized(0, 0).len();
        let bytes = sized(extra / BYTES_PER_FILE, extra % BYTES_PER_FILE);
        assert_eq!(bytes.len(), target);
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
        let temp = tempfile::tempdir().unwrap();
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

        let temp = tempfile::tempdir().unwrap();
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
        assert_eq!(snapshot_ids(&directory).len(), 6);

        // Reserve the remaining budget without allocating a large metadata tree.
        // A successful retry must recover retention BEFORE admitting new bytes.
        let staging = directory.resolved.join("diskscope-0000000000000000.tmp");
        let reservation = File::create(staging).unwrap();
        reservation
            .set_len(MAX_DISK_BYTES - counted_bytes(&directory))
            .unwrap();
        drop(reservation);
        assert_eq!(counted_bytes(&directory), MAX_DISK_BYTES);
        drop(reader);

        save_capture(&directory, capture(7), &mut snapshot)
            .expect("releasing the transient reader must recover without restart or Clear");
        assert_eq!(snapshot.scans.len(), 5);
        assert_eq!(snapshot.scans[0].live_generation, Some(7));
        assert_eq!(snapshot_ids(&directory).len(), 5);
        assert!(!directory.file(oldest).exists());
        assert!(counted_bytes(&directory) <= MAX_DISK_BYTES);
    }
    #[test]
    #[cfg(windows)]
    fn startup_retention_failure_keeps_catalog_for_later_recovery() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let temp = tempfile::tempdir().unwrap();
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
            .set_len(MAX_DISK_BYTES - counted_bytes(&directory))
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
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history");
        let directory = Directory::open(path.clone()).unwrap();
        let staging = directory.resolved.join("diskscope-0000000000000001.tmp");
        let reserved = File::create(&staging).unwrap();
        reserved.set_len(MAX_DISK_BYTES).unwrap();
        drop(reserved);
        assert_eq!(counted_bytes(&directory), MAX_DISK_BYTES);
        assert!(
            directory
                .write(2, b"exceeds reserved disk budget")
                .unwrap_err()
                .contains("budget")
        );
        assert!(!directory.file(2).exists());
        assert!(matches!(Directory::open(path.clone()),Err(e) if e==OWNED_ELSEWHERE));
        assert_eq!(counted_bytes(&directory), MAX_DISK_BYTES);
        directory.clear().unwrap();
        assert_eq!(counted_bytes(&directory), 0);
        // Clear must preserve ownership, so no rival can clear/write during a new reservation.
        assert!(matches!(Directory::open(path.clone()),Err(e) if e==OWNED_ELSEWHERE));
        drop(directory);
        let successor = Directory::open(path).unwrap();
        successor.write(3, b"new owner").unwrap();
        assert_eq!(counted_bytes(&successor), 9);
    }

    // ----- Header classification and the deletion rule ---------------------

    #[test]
    fn header_classification_separates_ours_older_newer_and_unrecognized() {
        use HeaderFormat::*;
        let ours = header(MAGIC, SCHEMA, POLICY);
        let cases: Vec<(&str, Vec<u8>, HeaderFormat)> = vec![
            ("current", ours.clone(), Current),
            (
                "current with a body",
                [ours.clone(), vec![9; 100]].concat(),
                Current,
            ),
            (
                "older magic version",
                header(b"DSHIST01", SCHEMA, POLICY),
                Older,
            ),
            (
                "oldest magic version",
                header(b"DSHIST00", SCHEMA, POLICY),
                Older,
            ),
            ("older schema", header(MAGIC, SCHEMA - 1, POLICY), Older),
            ("older policy", header(MAGIC, SCHEMA, POLICY - 1), Older),
            (
                "newer magic version",
                header(b"DSHIST03", SCHEMA, POLICY),
                Newer,
            ),
            ("far newer magic version", header(b"DSHIST99", 0, 0), Newer),
            ("newer magic, truncated", b"DSHIST03".to_vec(), Newer),
            ("newer schema", header(MAGIC, SCHEMA + 1, POLICY), Newer),
            (
                "newer schema beats older policy",
                header(MAGIC, SCHEMA + 1, 0),
                Newer,
            ),
            ("newer policy", header(MAGIC, SCHEMA, POLICY + 1), Newer),
            (
                "unparseable family version",
                header(b"DSHISTxx", SCHEMA, POLICY),
                Newer,
            ),
            (
                "family with a NUL version",
                header(b"DSHIST0\0", SCHEMA, POLICY),
                Newer,
            ),
            ("zero filled", vec![0; 64], Unrecognized),
            (
                "other format",
                b"PK\x03\x04 not history at all".to_vec(),
                Unrecognized,
            ),
            ("empty", Vec::new(), Unrecognized),
            ("shorter than a magic", b"DSHIST0".to_vec(), Unrecognized),
            (
                "current magic, header cut short",
                ours[..12].to_vec(),
                Unrecognized,
            ),
            ("current magic only", MAGIC.to_vec(), Unrecognized),
        ];
        for (name, bytes, expected) in cases {
            assert_eq!(classify_header(&bytes), expected, "{name}");
        }
    }

    #[test]
    fn only_a_definitive_format_verdict_can_lead_to_deletion() {
        // Newer files belong to a DiskScope the user may still run, and
        // Current ones still need their body verified: neither is reclaimable.
        assert!(HeaderFormat::Older.reclaimable());
        assert!(HeaderFormat::Unrecognized.reclaimable());
        assert!(!HeaderFormat::Newer.reclaimable());
        assert!(!HeaderFormat::Current.reclaimable());
    }

    #[test]
    fn the_magic_constants_describe_the_current_wire_format() {
        // Existing DSHIST02 files must stay readable: pin the literal values.
        assert_eq!(MAGIC, b"DSHIST02");
        assert_eq!((SCHEMA, POLICY), (2, 1));
        assert_eq!(magic_version(MAGIC), Some(MAGIC_VERSION));
        assert_eq!(
            &encode(&fixture()).unwrap()[..HEADER_PROBE_LEN],
            &header(MAGIC, 2, 1)[..]
        );
    }

    // ----- Streaming verification ------------------------------------------

    fn same_summary(a: &ScanSummary, b: &ScanSummary) -> bool {
        (
            a.id,
            &a.root,
            a.finished_unix,
            a.logical,
            a.files,
            a.folders,
            &a.backend,
        ) == (
            b.id,
            &b.root,
            b.finished_unix,
            b.logical,
            b.files,
            b.folders,
            &b.backend,
        )
    }

    #[test]
    fn streaming_verification_returns_the_same_summary_as_a_full_decode() {
        let bytes = valid_bytes(7);
        let Verdict::Usable(streamed) =
            verify_stream(&mut &bytes[..], bytes.len() as u64, 7, VERIFY_CHUNK_BYTES)
        else {
            panic!("a valid snapshot must verify");
        };
        let decoded = decode(&bytes).unwrap().summary;
        assert!(
            same_summary(&streamed, &decoded),
            "{streamed:?} vs {decoded:?}"
        );
        assert_eq!(streamed.live_generation, None);
    }

    #[test]
    fn streaming_verification_is_correct_at_every_chunk_boundary() {
        let chunk = MAX_HEADER_LEN;
        let mut checked = 0;
        for multiple in 1..=3 {
            for delta in -9i64..=9 {
                let target = (multiple * chunk as i64 + delta) as usize;
                let bytes = snapshot_with_len(5, target);
                let verdict = verify_stream(&mut &bytes[..], bytes.len() as u64, 5, chunk);
                let Verdict::Usable(streamed) = verdict else {
                    panic!(
                        "length {target} (chunk {chunk}, delta {delta}) must verify: {verdict:?}"
                    );
                };
                assert!(same_summary(&streamed, &decode(&bytes).unwrap().summary));
                checked += 1;
            }
        }
        assert_eq!(checked, 57);
    }

    #[test]
    fn any_single_changed_byte_is_damage_never_a_usable_snapshot() {
        let chunk = MAX_HEADER_LEN;
        let bytes = snapshot_with_len(5, 2 * chunk + 3);
        let len = bytes.len();
        // Around both chunk joins, the end of the body, the trailer, and the start of the body.
        let positions = [
            HEADER_PROBE_LEN,
            HEADER_PROBE_LEN + 1,
            100,
            chunk - 1,
            chunk,
            chunk + 1,
            2 * chunk - 1,
            2 * chunk,
            2 * chunk + 1,
            len - CHECKSUM_LEN - 1,
            len - CHECKSUM_LEN,
            len - 1,
        ];
        for position in positions {
            let mut changed = bytes.clone();
            changed[position] ^= 0x40;
            let verdict = verify_stream(&mut &changed[..], len as u64, 5, chunk);
            assert!(
                matches!(verdict, Verdict::Damaged),
                "byte {position}: {verdict:?}"
            );
        }
        // Bytes inside the 16-byte header change what the file is, which is
        // reported as a changed file and never as a usable one.
        for position in 0..HEADER_PROBE_LEN {
            let mut changed = bytes.clone();
            changed[position] ^= 0x40;
            let verdict = verify_stream(&mut &changed[..], len as u64, 5, chunk);
            assert!(
                !matches!(verdict, Verdict::Usable(_)),
                "byte {position}: {verdict:?}"
            );
        }
        // Truncation and an appended byte both change the checksummed length.
        let truncated = &bytes[..len / 2];
        assert!(matches!(
            verify_stream(&mut &truncated[..], truncated.len() as u64, 5, chunk),
            Verdict::Damaged
        ));
        let mut appended = bytes.clone();
        appended.push(0);
        assert!(matches!(
            verify_stream(&mut &appended[..], appended.len() as u64, 5, chunk),
            Verdict::Damaged
        ));
    }

    #[test]
    fn malformed_summaries_and_wrong_ids_are_damage() {
        let bytes = valid_bytes(7);
        let len = bytes.len() as u64;
        assert!(
            matches!(
                verify_stream(&mut &bytes[..], len, 8, VERIFY_CHUNK_BYTES),
                Verdict::Damaged
            ),
            "file name says 8, file says 7"
        );
        let mut relative = fixture();
        relative.summary.id = 7;
        relative.summary.root = PathBuf::from("relative/root");
        let relative = encode(&relative).unwrap();
        assert!(
            matches!(
                verify_stream(
                    &mut &relative[..],
                    relative.len() as u64,
                    7,
                    VERIFY_CHUNK_BYTES
                ),
                Verdict::Damaged
            ),
            "a valid checksum does not make a relative root acceptable"
        );
        // A current-format file over the size limit can never have been written by this build.
        let oversized = MAX_SNAPSHOT_BYTES as u64 + 1;
        let mut endless = io::Read::chain(&bytes[..], io::repeat(0));
        assert!(matches!(
            verify_stream(&mut endless, oversized, 7, VERIFY_CHUNK_BYTES),
            Verdict::Damaged
        ));
    }

    /// Serves `bytes`, then fails the way a locked or failing file would.
    struct FailingReader<'a> {
        bytes: &'a [u8],
        fail_after: usize,
        read: usize,
    }
    impl Read for FailingReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.read >= self.fail_after {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            let n = buffer
                .len()
                .min(self.fail_after - self.read)
                .min(self.bytes.len() - self.read);
            buffer[..n].copy_from_slice(&self.bytes[self.read..self.read + n]);
            self.read += n;
            Ok(n)
        }
    }

    #[test]
    fn a_failed_or_inconsistent_read_is_never_a_verdict_of_damage() {
        let chunk = MAX_HEADER_LEN;
        let bytes = snapshot_with_len(5, 2 * chunk + 3);
        let len = bytes.len();
        // An I/O error (sharing violation, failing disk) at any point of the stream.
        for fail_after in [0, 5, 16, 100, chunk - 1, chunk, chunk + 1, len - 9, len - 1] {
            let mut reader = FailingReader {
                bytes: &bytes,
                fail_after,
                read: 0,
            };
            let verdict = verify_stream(&mut reader, len as u64, 5, chunk);
            assert!(
                matches!(verdict, Verdict::Unreadable(_)),
                "failure after {fail_after}: {verdict:?}"
            );
        }
        // The file shrank after it was measured.
        let shrunk = &bytes[..len - 5];
        assert!(matches!(
            verify_stream(&mut &shrunk[..], len as u64, 5, chunk),
            Verdict::Unreadable(_)
        ));
        // The file grew after it was measured.
        assert!(matches!(
            verify_stream(&mut &bytes[..], len as u64 - 1, 5, chunk),
            Verdict::Unreadable(_)
        ));
        // The file was replaced by something else after it was triaged.
        let zeros = vec![0u8; len];
        assert!(matches!(
            verify_stream(&mut &zeros[..], len as u64, 5, chunk),
            Verdict::Unreadable(_)
        ));
    }

    #[test]
    fn unopenable_or_non_file_paths_are_unreadable_not_damaged() {
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(
            verify_snapshot(&temp.path().join("missing.dsh"), 1),
            Verdict::Unreadable(_)
        ));
        assert!(matches!(
            verify_snapshot(temp.path(), 1),
            Verdict::Unreadable(_)
        ));
        assert!(probe_header(&temp.path().join("missing.dsh")).is_err());
    }

    // ----- Bounded collection, messages and budget rules --------------------

    #[test]
    fn newest_files_keeps_a_bounded_newest_first_window() {
        let mut newest = NewestFiles::new(16);
        // Scrambled order, as a directory listing gives it.
        for step in 0..1000u64 {
            let id = (step * 7919) % 1000;
            newest.offer(OwnedFile { id, len: id });
        }
        assert_eq!(newest.offered, 1000);
        assert!(newest.kept.len() <= 16);
        let ids: Vec<u64> = newest.into_newest_first().iter().map(|f| f.id).collect();
        assert_eq!(ids, (984..=999).rev().collect::<Vec<u64>>());
    }

    #[test]
    fn load_report_messages_say_what_was_removed_kept_and_skipped() {
        assert_eq!(
            LoadReport::default().message(),
            "Local metadata history ready. Five snapshots maximum; no file contents stored."
        );
        let mut report = LoadReport::default();
        report.unusable.removed = 5;
        assert!(
            report
                .message()
                .contains("Removed 5 unreadable history files (corrupt or incompatible format).")
        );
        report.unusable.removed = 1;
        assert!(
            report
                .message()
                .contains("Removed 1 unreadable history file (corrupt")
        );
        report.staging.removed = 2;
        assert!(
            report
                .message()
                .contains("Removed 2 interrupted history writes.")
        );
        report.surplus.removed = 3;
        assert!(
            report
                .message()
                .contains("Removed 3 older history files beyond the 5 retained.")
        );
        report.unusable.stuck = 1;
        report.staging.stuck = 1;
        assert!(
            report
                .message()
                .contains("2 history files could not be removed")
        );
        report.kept_newer = 1;
        assert!(report.message().contains(
            "Kept 1 history file written by a newer DiskScope; this version cannot read it."
        ));
        report.kept_newer = 4;
        assert!(report.message().contains(
            "Kept 4 history files written by a newer DiskScope; this version cannot read them."
        ));
        report.note_unreadable("os error 32");
        report.note_unreadable("second error is not repeated");
        assert!(
            report
                .message()
                .contains("Could not read 2 history files (kept, not deleted): os error 32")
        );
        report.unexamined = 3;
        assert!(
            report
                .message()
                .contains("3 history files older than the newest 16 were not examined")
        );
    }

    #[test]
    fn newer_files_are_budgeted_apart_and_hard_bounded() {
        let own = |counted| Usage {
            counted,
            ..Usage::default()
        };
        assert!(check_write_budget(&own(MAX_DISK_BYTES - 10), 10).is_ok());
        let over = check_write_budget(&own(MAX_DISK_BYTES - 10), 11).unwrap_err();
        assert!(over.contains("budget"), "{over}");
        assert!(
            check_write_budget(&own(0), MAX_SNAPSHOT_BYTES + 1)
                .unwrap_err()
                .contains("budget")
        );

        // Newer-format bytes at the cap do not consume this build's budget...
        let foreign = |files, bytes| Usage {
            counted: 0,
            foreign_files: files,
            foreign_bytes: bytes,
        };
        assert!(
            check_write_budget(
                &foreign(MAX_FOREIGN_FILES, MAX_FOREIGN_BYTES),
                MAX_SNAPSHOT_BYTES
            )
            .is_ok()
        );
        // ...but past either bound saving stops, with an explanation.
        for usage in [
            foreign(MAX_FOREIGN_FILES + 1, 1),
            foreign(1, MAX_FOREIGN_BYTES + 1),
        ] {
            let message = check_write_budget(&usage, 10).unwrap_err();
            assert!(message.contains("newer DiskScope"), "{message}");
            assert!(message.contains("Clear saved scan history"), "{message}");
        }
    }

    // ----- Directory behaviour on real files ---------------------------------

    #[test]
    fn load_reclaims_only_definitively_unusable_files() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        let write = |id: u64, bytes: &[u8]| fs::write(directory.file(id), bytes).unwrap();
        for id in [10, 20, 30] {
            write(id, &valid_bytes(id));
        }
        let unusable: Vec<(u64, Vec<u8>)> = vec![
            (
                40,
                [header(MAGIC, SCHEMA - 1, POLICY), vec![1; 500]].concat(),
            ),
            (50, [header(b"DSHIST01", 1, 1), vec![1; 500]].concat()),
            (60, vec![0; 2000]),
            (70, b"definitely not a snapshot".to_vec()),
            (80, Vec::new()),
            (90, damaged_bytes(90)),
            (100, valid_bytes(100)[..60].to_vec()),
            (110, valid_bytes(999)),
        ];
        for (id, bytes) in &unusable {
            write(*id, bytes);
        }
        let newer: Vec<(u64, Vec<u8>)> = vec![
            (120, [header(b"DSHIST03", 3, 1), vec![7; 500]].concat()),
            (
                130,
                [header(MAGIC, SCHEMA + 1, POLICY), vec![8; 500]].concat(),
            ),
            (
                140,
                [header(MAGIC, SCHEMA, POLICY + 1), vec![9; 500]].concat(),
            ),
        ];
        for (id, bytes) in &newer {
            write(*id, bytes);
        }
        fs::write(directory.staging_file(150), b"interrupted write").unwrap();
        fs::write(directory.resolved.join("keep.txt"), b"unrelated").unwrap();
        fs::create_dir(directory.file(160)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(directory.resolved.join("keep.txt"), directory.file(170))
            .unwrap();

        let (scans, report) = directory.load_catalog().unwrap();

        let ids: Vec<u64> = scans.iter().map(|s| s.id).collect();
        assert_eq!(ids, [30, 20, 10]);
        assert_eq!(
            report.unusable,
            Removals {
                removed: unusable.len(),
                stuck: 0
            }
        );
        assert_eq!(
            report.staging,
            Removals {
                removed: 1,
                stuck: 0
            }
        );
        assert_eq!(report.kept_newer, 3);
        assert_eq!(report.unreadable, 0);
        for (id, _) in &unusable {
            assert!(
                !directory.file(*id).exists(),
                "unusable file {id} should be reclaimed"
            );
        }
        for (id, bytes) in &newer {
            assert_eq!(
                &fs::read(directory.file(*id)).unwrap(),
                bytes,
                "newer file {id} must be untouched"
            );
        }
        assert!(!directory.staging_file(150).exists());
        assert!(directory.resolved.join("keep.txt").exists());
        assert!(
            directory.file(160).is_dir(),
            "a directory with a snapshot name is not ours to remove"
        );
        #[cfg(unix)]
        assert!(
            fs::symlink_metadata(directory.file(170))
                .unwrap()
                .file_type()
                .is_symlink(),
            "a link with a snapshot name is not ours to remove"
        );
        assert!(
            report
                .message()
                .contains("Removed 8 unreadable history files")
        );
    }

    #[test]
    fn five_wedging_old_schema_files_no_longer_block_saving() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        // Together they fill the whole budget, as five large old-format files would.
        let near_limit = MAX_DISK_BYTES / 5;
        for id in 1..=5 {
            let file = File::create(directory.file(id)).unwrap();
            (&file)
                .write_all(&header(MAGIC, SCHEMA - 1, POLICY))
                .unwrap();
            file.set_len(near_limit).unwrap();
        }
        assert!(counted_bytes(&directory) > MAX_DISK_BYTES - 1024);
        assert!(
            directory.write(100, &valid_bytes(100)).is_err(),
            "the wedge: nothing fits"
        );

        let (scans, report) = directory.load_catalog().unwrap();

        assert!(scans.is_empty());
        assert_eq!(report.unusable.removed, 5);
        assert_eq!(counted_bytes(&directory), 0);
        directory.write(100, &valid_bytes(100)).unwrap();
    }

    #[test]
    fn newer_files_cannot_wedge_saving_until_their_hard_bound() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        // Counted against the budget this one file would leave no room at all.
        let huge = File::create(directory.file(1)).unwrap();
        (&huge).write_all(&header(b"DSHIST03", 3, 1)).unwrap();
        huge.set_len(MAX_DISK_BYTES - 1024 * 1024).unwrap();
        drop(huge);
        directory.write(100, &valid_bytes(100)).unwrap();
        let usage = directory.usage().unwrap();
        assert_eq!(usage.foreign_files, 1);
        assert_eq!(usage.foreign_bytes, MAX_DISK_BYTES - 1024 * 1024);
        assert_eq!(usage.counted, valid_bytes(100).len() as u64);

        // Many small newer files, up to the bound, still allow saving...
        for id in 2..=MAX_FOREIGN_FILES as u64 {
            fs::write(directory.file(id), header(b"DSHIST03", 3, 1)).unwrap();
        }
        directory.write(101, &valid_bytes(101)).unwrap();
        // ...and one more makes saving refuse, without touching anything.
        fs::write(directory.file(1000), header(b"DSHIST03", 3, 1)).unwrap();
        let refused = directory.write(102, &valid_bytes(102)).unwrap_err();
        assert!(refused.contains("newer DiskScope"), "{refused}");
        assert!(!directory.file(102).exists());
        assert!(!directory.staging_file(102).exists());
        // Only an explicit Clear removes newer-format files.
        directory.clear().unwrap();
        assert_eq!(directory.usage().unwrap(), Usage::default());
        directory.write(102, &valid_bytes(102)).unwrap();
    }

    #[test]
    fn hundreds_of_files_load_with_bounded_reads_and_reclaim_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        let valid_ids = 1000..1040u64;
        for id in valid_ids.clone() {
            fs::write(directory.file(id), valid_bytes(id)).unwrap();
        }
        // 300 junk files, deliberately newer by id than every valid snapshot.
        for id in 2000..2300u64 {
            fs::write(directory.file(id), header(MAGIC, SCHEMA - 1, POLICY)).unwrap();
        }
        for id in 3000..3200u64 {
            fs::write(directory.staging_file(id), b"interrupted").unwrap();
        }
        // Newer-format files older than every window must survive the surplus pass.
        for id in 1..=3u64 {
            fs::write(directory.file(id), header(b"DSHIST03", 3, 1)).unwrap();
        }

        let (scans, report) = directory.load_catalog().unwrap();

        assert!(
            report.examined <= MAX_EXAMINED_FILES,
            "examined {}",
            report.examined
        );
        assert_eq!(scans.len(), MAX_EXAMINED_FILES);
        assert_eq!(scans[0].id, 1039);
        assert_eq!(report.unusable.removed, 300);
        assert_eq!(report.staging.removed, 200);
        assert_eq!(report.surplus.removed, 40 - MAX_EXAMINED_FILES);
        assert_eq!(report.unexamined, 0);
        assert_eq!(report.kept_newer, 3);
        let left = snapshot_ids(&directory);
        assert_eq!(left.len(), MAX_EXAMINED_FILES + 3);
        assert_eq!(&left[..3], &[1, 2, 3]);
        // The worker's retention then trims the catalog to the five newest.
        let mut snapshot = HistorySnapshot {
            scans,
            ..Default::default()
        };
        retain_snapshots(&directory, &mut snapshot).unwrap();
        assert_eq!(snapshot.scans.len(), MAX_SNAPSHOTS);
        assert_eq!(snapshot_ids(&directory).len(), MAX_SNAPSHOTS + 3);
    }

    #[test]
    fn older_intact_snapshots_wait_when_the_newest_candidates_are_damaged() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        for id in 1..=4u64 {
            fs::write(directory.file(id), valid_bytes(id)).unwrap();
        }
        for id in 5..=20u64 {
            fs::write(directory.file(id), damaged_bytes(id)).unwrap();
        }

        let (scans, report) = directory.load_catalog().unwrap();
        // Sixteen damaged candidates fill the window; the four intact ones behind
        // them are neither read nor deleted, because they may be the only usable ones.
        assert!(scans.is_empty());
        assert_eq!(report.unusable.removed, 16);
        assert_eq!(report.unexamined, 4);
        assert_eq!(snapshot_ids(&directory), [1, 2, 3, 4]);

        // Each load makes progress: the damaged ones are gone, so the next reaches them.
        let (scans, report) = directory.load_catalog().unwrap();
        assert_eq!(scans.iter().map(|s| s.id).collect::<Vec<_>>(), [4, 3, 2, 1]);
        assert_eq!(report.unexamined, 0);
    }

    #[test]
    fn clear_always_succeeds_whatever_the_file_count_and_removes_only_owned_files() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        for id in 0..300u64 {
            let bytes = match id % 3 {
                0 => valid_bytes(id),
                1 => header(b"DSHIST03", 3, 1),
                _ => b"junk".to_vec(),
            };
            fs::write(directory.file(id), bytes).unwrap();
            fs::write(directory.staging_file(id), b"interrupted").unwrap();
        }
        fs::write(directory.resolved.join("keep.txt"), b"unrelated").unwrap();
        fs::write(
            directory.resolved.join("diskscope-not-history.tmp"),
            b"unrelated",
        )
        .unwrap();
        fs::create_dir(directory.file(5000)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(directory.resolved.join("keep.txt"), directory.file(5001))
            .unwrap();

        // Before this change more than 128 files made both load and Clear fail.
        directory.clear().unwrap();

        assert!(snapshot_ids(&directory).is_empty());
        assert_eq!(directory.usage().unwrap(), Usage::default());
        assert!(directory.resolved.join("keep.txt").exists());
        assert!(
            directory
                .resolved
                .join("diskscope-not-history.tmp")
                .exists()
        );
        assert!(directory.file(5000).is_dir());
        #[cfg(unix)]
        assert!(
            fs::symlink_metadata(directory.file(5001))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn catalog_load_defers_node_validation_and_compare_reports_it_as_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        // Valid checksum, valid summary, invalid body: a duplicate path.
        let mut structurally_invalid = fixture();
        structurally_invalid.summary.id = 1;
        structurally_invalid.nodes.push(SavedNode {
            parent: Some(0),
            name: "safe".into(),
            is_dir: false,
            reparse: false,
            logical: 0,
            allocated: Some(0),
            modified: None,
            identity: None,
        });
        fs::write(directory.file(1), encode(&structurally_invalid).unwrap()).unwrap();
        fs::write(directory.file(2), valid_bytes(2)).unwrap();

        let (scans, _) = directory.load_catalog().unwrap();
        assert_eq!(scans.iter().map(|s| s.id).collect::<Vec<_>>(), [2, 1]);
        let error = compare_saved(&directory, &scans, 1, 2).unwrap_err();
        assert!(error.contains("Duplicate"), "{error}");

        // A body that was intact when the catalog loaded and is damaged now.
        fs::write(directory.file(1), valid_bytes(1)).unwrap();
        fs::write(directory.file(2), damaged_bytes(2)).unwrap();
        let error = compare_saved(&directory, &scans, 1, 2).unwrap_err();
        assert!(error.contains("integrity"), "{error}");
    }

    /// Windows reports a file held open by another program as a sharing
    /// violation. That is the realistic transient failure (antivirus, indexer,
    /// backup), so it is exercised with real handles. Type-checked on Linux only.
    #[test]
    #[cfg(windows)]
    fn a_sharing_violation_never_deletes_and_the_next_start_recovers() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        fs::write(directory.file(1), valid_bytes(1)).unwrap();
        fs::write(
            directory.file(2),
            [header(MAGIC, SCHEMA - 1, POLICY), vec![1; 100]].concat(),
        )
        .unwrap();

        // A valid snapshot that cannot even be opened is neither listed nor deleted.
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(directory.file(1))
            .unwrap();
        // An unusable one that can be read but not deleted is kept and retried.
        let undeletable = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(directory.file(2))
            .unwrap();
        let (scans, report) = directory.load_catalog().unwrap();
        assert!(scans.is_empty());
        assert_eq!(report.unreadable, 1);
        assert_eq!(
            report.unusable,
            Removals {
                removed: 0,
                stuck: 1
            }
        );
        assert!(directory.file(1).exists());
        assert!(directory.file(2).exists());
        assert!(report.message().contains("could not be removed"));

        drop((locked, undeletable));
        let (scans, report) = directory.load_catalog().unwrap();
        assert_eq!(scans.iter().map(|s| s.id).collect::<Vec<_>>(), [1]);
        assert_eq!(
            report.unusable,
            Removals {
                removed: 1,
                stuck: 0
            }
        );
        assert!(!directory.file(2).exists());
    }

    #[test]
    fn a_clear_that_could_not_remove_everything_still_unlists_what_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(&temp);
        fs::write(directory.file(1), valid_bytes(1)).unwrap();
        fs::write(directory.file(2), valid_bytes(2)).unwrap();
        let (scans, _) = directory.load_catalog().unwrap();
        let comparison = compare_saved(&directory, &scans, 1, 2).unwrap();
        let mut snapshot = HistorySnapshot {
            scans,
            comparison: Some(Arc::new(comparison)),
            ..Default::default()
        };

        forget_vanished_scans(&directory, &mut snapshot);
        assert_eq!(snapshot.scans.len(), 2, "nothing was removed");
        assert!(snapshot.comparison.is_some());

        // Snapshot 2 was removed; snapshot 1 is stuck (in use) and stays listed.
        fs::remove_file(directory.file(2)).unwrap();
        forget_vanished_scans(&directory, &mut snapshot);
        let ids: Vec<u64> = snapshot.scans.iter().map(|scan| scan.id).collect();
        assert_eq!(ids, [1]);
        assert!(snapshot.comparison.is_none(), "it compared a removed scan");
    }

    #[test]
    fn reclamation_stops_when_the_directory_was_swapped() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history");
        let directory = Directory::open(path.clone()).unwrap();
        fs::rename(&path, temp.path().join("moved-away")).ok();
        if path.exists() {
            // Windows holds the ownership file open without delete sharing, so the
            // swap cannot even happen; that protects the binding just as well.
            return;
        }
        fs::create_dir(&path).unwrap();
        let planted = path.join("diskscope-0000000000000007.dsh");
        fs::write(&planted, header(MAGIC, SCHEMA - 1, POLICY)).unwrap();

        let error = directory.load_catalog().unwrap_err();

        assert!(error.contains("changed"), "{error}");
        assert!(
            planted.exists(),
            "a swapped directory must never be reclaimed from"
        );
    }
}
