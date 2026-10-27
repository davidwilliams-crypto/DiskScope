use diskscope::{
    history::{HistoryService, HistorySnapshot},
    model::{Entry, FileIdentity, Model, ScanState},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::{
        Arc,
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
    let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
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
