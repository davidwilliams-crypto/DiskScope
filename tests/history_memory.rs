use diskscope::{
    history::{HistoryService, HistorySnapshot},
    model::{Entry, FileIdentity, Model, ScanState},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

// Keep this resource regression in its own integration-test process so other
// tests cannot contribute allocations to the measured comparison interval.
struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn allocated(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::SeqCst) + bytes;
    PEAK.fetch_max(live, Ordering::SeqCst);
}

// SAFETY: allocation and deallocation are forwarded unchanged to System; only
// atomic counters are updated, without allocating or dereferencing the buffers.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplied the valid layout required by GlobalAlloc.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: pointer/layout are the caller's matching System allocation.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplies a live allocation and valid replacement size.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
            allocated(size);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// The counters are process-wide, so every test in this file holds this lock for
// its whole body; otherwise tests run in parallel and measure each other.
static EXCLUSIVE: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    // A failed test must not turn every later test into a poisoned-lock failure.
    EXCLUSIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait(
    service: &HistoryService,
    ready: impl Fn(&HistorySnapshot) -> bool,
) -> Arc<HistorySnapshot> {
    let started = Instant::now();
    loop {
        if let Some(snapshot) = service.poll()
            && !snapshot.busy
            && ready(&snapshot)
        {
            return snapshot;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "history timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn comparison_memory_does_not_expand_every_shared_path_prefix() {
    let _exclusive = exclusive();
    let temp = tempfile::tempdir().unwrap();
    let mut model = Model::new(temp.path().join("scan"));
    let mut parent = 0;
    for _ in 0..12 {
        parent = model.insert_batch(
            parent,
            vec![Entry {
                name: "a".repeat(200).into(),
                is_dir: true,
                reparse: false,
                logical: 0,
                allocated: Some(0),
                modified: None,
                identity: None,
            }],
        )[0];
    }
    model.insert_batch(
        parent,
        (1u64..=20_000)
            .map(|index| {
                let mut id = [0; 16];
                id[..8].copy_from_slice(&index.to_le_bytes());
                Entry {
                    name: format!("file-{index:05}").into(),
                    is_dir: false,
                    reparse: false,
                    logical: 10,
                    allocated: Some(10),
                    modified: Some(1),
                    identity: Some(FileIdentity { volume: 1, id }),
                }
            })
            .collect(),
    );
    let model = Arc::new(model);
    let history = HistoryService::new(temp.path().join("history"));
    wait(&history, |_| true);
    assert!(
        history
            .capture()
            .submit(model.clone(), 1, ScanState::Complete, "Normal".into())
    );
    let older = wait(&history, |s| s.scans.len() == 1).scans[0].id;
    assert!(
        history
            .capture()
            .submit(model, 2, ScanState::Complete, "Normal".into())
    );
    let newer = wait(&history, |s| s.scans.len() == 2).scans[0].id;

    let baseline = LIVE.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    assert!(history.compare(older, newer));
    let snapshot = wait(&history, |s| s.comparison.is_some());
    let peak = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);
    let comparison = snapshot.comparison.as_ref().unwrap();
    assert_eq!(comparison.total_changes, 0);
    assert_eq!((comparison.before, comparison.after), (200_000, 200_000));
    // Two materialized 2,400-character prefixes per file alone exceed 90 MiB.
    // This generous budget includes decoding, validation, indexes and results.
    assert!(
        peak < 32 * 1024 * 1024,
        "comparison allocated {peak} peak extra bytes"
    );
    eprintln!("history comparison peak extra heap: {peak} bytes");
}

// ----- Catalog loading must not scale with the number of nodes ------------

// FNV-1a, 64 bit: the checksum that closes every snapshot file.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Passes bytes through while folding them into the snapshot checksum.
struct ChecksumWriter<W: Write> {
    inner: W,
    hash: u64,
}
impl<W: Write> Write for ChecksumWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        for byte in &bytes[..written] {
            self.hash = (self.hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes a DSHIST02 snapshot of `nodes` nodes (a root and `nodes - 1` one-byte
/// files directly under it) straight from the documented layout, using no
/// code from the crate. That keeps this a check of the wire format itself:
/// files written by earlier builds must stay readable. It streams to disk, so
/// the size of the file never shows up in the heap. `bumped` makes one file two
/// bytes long instead of one. Returns the file size.
fn write_snapshot(path: &Path, id: u64, root: &Path, nodes: u32, bumped: Option<u32>) -> u64 {
    let files = u64::from(nodes) - 1;
    let logical = files + u64::from(bumped.is_some());
    let mut out = ChecksumWriter {
        inner: BufWriter::with_capacity(64 * 1024, File::create(path).unwrap()),
        hash: FNV_OFFSET_BASIS,
    };
    let u32le = |out: &mut ChecksumWriter<_>, value: u32| out.write_all(&value.to_le_bytes());
    let u64le = |out: &mut ChecksumWriter<_>, value: u64| out.write_all(&value.to_le_bytes());

    out.write_all(b"DSHIST02").unwrap();
    u32le(&mut out, 2).unwrap(); // schema
    u32le(&mut out, 1).unwrap(); // accounting policy
    for word in [id, 1, logical, files, 0] {
        // id, finished (unix seconds), logical bytes, files, folders
        u64le(&mut out, word).unwrap();
    }
    let root_units: Vec<u16> = root.to_str().unwrap().encode_utf16().collect();
    u32le(&mut out, root_units.len() as u32).unwrap();
    for unit in root_units {
        out.write_all(&unit.to_le_bytes()).unwrap();
    }
    let backend = b"Normal";
    u32le(&mut out, backend.len() as u32).unwrap();
    out.write_all(backend).unwrap();
    u32le(&mut out, nodes).unwrap();

    // Node: parent u32 (MAX = none), flags u8 (bit 0 directory), logical u64,
    // allocated (tag u8 [+ u64]), modified (tag u8 [+ u64]), identity (tag u8),
    // name (u32 UTF-16 length + units).
    let node = |out: &mut ChecksumWriter<_>, parent: u32, flags: u8, size: u64, name: &str| {
        u32le(out, parent).unwrap();
        out.write_all(&[flags]).unwrap();
        u64le(out, size).unwrap();
        out.write_all(&[1]).unwrap();
        u64le(out, size).unwrap();
        out.write_all(&[0, 0]).unwrap();
        let units: Vec<u16> = name.encode_utf16().collect();
        u32le(out, units.len() as u32).unwrap();
        for unit in units {
            out.write_all(&unit.to_le_bytes()).unwrap();
        }
    };
    node(&mut out, u32::MAX, 1, logical, "");
    for index in 0..nodes - 1 {
        let size = if bumped == Some(index) { 2 } else { 1 };
        node(&mut out, 0, 0, size, &format!("f{index:08}"));
    }
    let checksum = out.hash;
    out.inner.write_all(&checksum.to_le_bytes()).unwrap();
    out.inner.flush().unwrap();
    fs::metadata(path).unwrap().len()
}

fn snapshot_name(id: u64) -> String {
    format!("diskscope-{id:016x}.dsh")
}

/// Extra heap, at its peak, while a new service loads the catalog from two
/// snapshots of `nodes` nodes each. Returns that peak and the catalog.
fn catalog_peak(nodes: u32) -> (usize, u64, Arc<HistorySnapshot>) {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("history");
    fs::create_dir(&store).unwrap();
    let root = temp.path().join("scan");
    let (older, newer) = (1_000_000, 2_000_000);
    let file_len = write_snapshot(&store.join(snapshot_name(older)), older, &root, nodes, None);
    write_snapshot(
        &store.join(snapshot_name(newer)),
        newer,
        &root,
        nodes,
        Some(7),
    );

    let baseline = LIVE.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    let history = HistoryService::new(store);
    let catalog = wait(&history, |s| s.scans.len() == 2);
    let peak = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);
    (peak, file_len, catalog)
}

#[test]
fn catalog_load_memory_does_not_grow_with_the_number_of_nodes() {
    let _exclusive = exclusive();
    const SMALL: u32 = 200_000;
    const LARGE: u32 = 800_000;
    // The ceiling for this build's catalog load: one verification buffer plus
    // thread, channel and message overhead. A decode that materializes nodes
    // needs tens of megabytes for these files and fails this by an order of magnitude.
    const CEILING: usize = 2 * 1024 * 1024;

    let (small_peak, small_len, small) = catalog_peak(SMALL);
    let (large_peak, large_len, large) = catalog_peak(LARGE);

    assert!(
        small_len > 8 * 1024 * 1024,
        "fixture too small to mean anything: {small_len}"
    );
    assert!(large_len > 3 * small_len, "{large_len} vs {small_len}");
    for catalog in [&small, &large] {
        assert_eq!(catalog.scans.len(), 2, "{}", catalog.message);
        assert!(catalog.message.contains("ready"), "{}", catalog.message);
    }
    // The summary comes from the file header: it must match what was written.
    let newest = &large.scans[0];
    assert_eq!(newest.id, 2_000_000);
    assert_eq!((newest.files, newest.folders), (u64::from(LARGE) - 1, 0));
    assert_eq!(newest.logical, u64::from(LARGE));
    assert_eq!(newest.backend, "Normal");
    assert_eq!(large.scans[1].logical, u64::from(LARGE) - 1);

    eprintln!(
        "catalog load peak extra heap: {small_peak} bytes for {small_len} byte files, \
         {large_peak} bytes for {large_len} byte files"
    );
    assert!(
        small_peak < CEILING,
        "catalog of {small_len} byte files: {small_peak} bytes"
    );
    assert!(
        large_peak < CEILING,
        "catalog of {large_len} byte files: {large_peak} bytes"
    );
    // Four times the nodes, the same memory.
    assert!(
        large_peak < small_peak + 256 * 1024,
        "peak grew with node count: {small_peak} -> {large_peak}"
    );
}

#[test]
fn independently_written_dshist02_files_stay_readable_and_comparable() {
    let _exclusive = exclusive();
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("history");
    fs::create_dir(&store).unwrap();
    let root = temp.path().join("scan");
    write_snapshot(&store.join(snapshot_name(10)), 10, &root, 50, None);
    write_snapshot(&store.join(snapshot_name(20)), 20, &root, 50, Some(7));

    let history = HistoryService::new(store);
    let catalog = wait(&history, |s| s.scans.len() == 2);
    assert_eq!(catalog.scans[0].id, 20);
    assert_eq!(catalog.scans[1].id, 10);
    assert_eq!(catalog.scans[1].root, root);

    assert!(history.compare(10, 20));
    let compared = wait(&history, |s| s.comparison.is_some());
    let comparison = compared.comparison.as_ref().unwrap();
    assert_eq!(
        (comparison.before, comparison.after, comparison.delta),
        (49, 50, 1)
    );
    assert_eq!(comparison.modified, 1);
    assert!(comparison.rows[0].path.ends_with("f00000007"));
}
