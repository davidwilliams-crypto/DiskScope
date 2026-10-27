use diskscope::{history::*, model::*};
use std::{
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn model(root: PathBuf, entries: &[(&str, u64, u8)]) -> Arc<Model> {
    let mut m = Model::new(root);
    m.insert_batch(
        0,
        entries
            .iter()
            .map(|(name, bytes, id)| Entry {
                name: (*name).into(),
                is_dir: false,
                reparse: false,
                logical: *bytes,
                allocated: Some(*bytes),
                modified: Some(100),
                identity: Some(FileIdentity {
                    volume: 1,
                    id: [*id; 16],
                }),
            })
            .collect(),
    );
    Arc::new(m)
}
fn wait(
    service: &HistoryService,
    predicate: impl Fn(&HistorySnapshot) -> bool,
) -> Arc<HistorySnapshot> {
    let end = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    loop {
        if let Some(s) = service.poll() {
            last = format!("{s:?}");
            if predicate(&s) {
                return s;
            }
        }
        assert!(
            Instant::now() < end,
            "history did not publish expected result: {last}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn successful_metadata_roundtrip_never_reuses_live_generation() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let root = dir.path().join("scan");
    let store = dir.path().join("history");
    let service = HistoryService::new(store.clone());
    wait(&service, |s| !s.busy);
    assert!(service.capture().submit(
        model(root.clone(), &[("one", 42, 1)]),
        77,
        ScanState::Complete,
        "Normal".into()
    ));
    let snapshot = wait(&service, |s| s.scans.len() == 1 && !s.busy);
    assert_eq!(snapshot.scans[0].logical, 42);
    assert_eq!(snapshot.scans[0].live_generation, Some(77));
    drop(service);
    let loaded = HistoryService::new(store);
    let snapshot = wait(&loaded, |s| !s.busy);
    assert_eq!(snapshot.scans.len(), 1);
    assert_eq!(snapshot.scans[0].root, root);
    assert_eq!(snapshot.scans[0].logical, 42);
    assert_eq!(snapshot.scans[0].live_generation, None);
}

#[test]
fn comparison_matches_paths_then_only_unambiguous_identity_moves() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let root = dir.path().join("scan");
    let service = HistoryService::new(dir.path().join("history"));
    wait(&service, |s| !s.busy);
    let capture = service.capture();
    capture.submit(
        model(
            root.clone(),
            &[
                ("same", 10, 1),
                ("old-name", 20, 2),
                ("gone", 7, 3),
                ("alias-a", 5, 9),
                ("alias-b", 5, 9),
            ],
        ),
        1,
        ScanState::Complete,
        "Normal".into(),
    );
    let old = wait(&service, |s| s.scans.len() == 1 && !s.busy).scans[0].id;
    capture.submit(
        model(
            root,
            &[
                ("same", 15, 1),
                ("new-name", 20, 2),
                ("new", 9, 4),
                ("alias-a", 5, 9),
                ("alias-c", 5, 9),
            ],
        ),
        2,
        ScanState::Complete,
        "Normal".into(),
    );
    let new = wait(&service, |s| s.scans.len() == 2 && !s.busy).scans[0].id;
    assert!(service.compare(old, new));
    let s = wait(&service, |s| s.comparison.is_some() && !s.busy);
    let c = s.comparison.as_ref().unwrap();
    assert_eq!((c.added, c.removed, c.modified, c.moved), (2, 2, 1, 1));
    assert_eq!(c.delta, 7);
    assert_eq!(c.newer_generation, Some(2));
    assert!(c.rows.iter().any(|r| r.kind == ChangeKind::Moved
        && r.previous_path.as_ref().unwrap().ends_with("old-name")
        && r.path.ends_with("new-name")));
}

#[test]
fn comparison_matches_native_hierarchy_independently_of_enumeration_order() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let root = dir.path().join("scan");
    let service = HistoryService::new(dir.path().join("history"));
    wait(&service, |s| !s.busy);
    let make = |reversed: bool| {
        let mut m = Model::new(root.clone());
        let entry = |name: &str, bytes, id, is_dir| Entry {
            name: name.into(),
            is_dir,
            reparse: false,
            logical: bytes,
            allocated: Some(bytes),
            modified: Some(100),
            identity: (!is_dir).then_some(FileIdentity {
                volume: 1,
                id: [id; 16],
            }),
        };
        let folders = if reversed {
            ["right", "left"]
        } else {
            ["left", "right"]
        };
        for folder in folders {
            let parent = m.insert_batch(0, vec![entry(folder, 0, 0, true)])[0];
            let bytes = if folder == "left" && reversed { 15 } else { 10 };
            let id = if folder == "left" { 1 } else { 2 };
            // Same basename in different parents must never alias, even when
            // both snapshots assign different node IDs to those parents.
            m.insert_batch(parent, vec![entry("same.dat", bytes, id, false)]);
            let nested = m.insert_batch(parent, vec![entry("nested", 0, 0, true)])[0];
            m.insert_batch(nested, vec![entry("same.dat", 7, id + 2, false)]);
        }
        Arc::new(m)
    };
    assert!(
        service
            .capture()
            .submit(make(false), 1, ScanState::Complete, "Normal".into())
    );
    let old = wait(&service, |s| s.scans.len() == 1 && !s.busy).scans[0].id;
    assert!(
        service
            .capture()
            .submit(make(true), 2, ScanState::Complete, "Normal".into())
    );
    let new = wait(&service, |s| s.scans.len() == 2 && !s.busy).scans[0].id;
    assert!(service.compare(old, new));
    let snapshot = wait(&service, |s| s.comparison.is_some() && !s.busy);
    let comparison = snapshot.comparison.as_ref().unwrap();
    assert_eq!(
        (
            comparison.added,
            comparison.removed,
            comparison.modified,
            comparison.moved
        ),
        (0, 0, 1, 0)
    );
    assert_eq!(comparison.rows[0].path, root.join("left/same.dat"));
    assert_eq!(comparison.delta, 5);
}

#[test]
fn partial_cancelled_errors_and_overflow_never_become_baselines() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let service = HistoryService::new(dir.path().join("history"));
    wait(&service, |s| !s.busy);
    for state in [
        ScanState::Partial,
        ScanState::Cancelled,
        ScanState::Failed,
        ScanState::Scanning,
    ] {
        assert!(!service.capture().submit(
            model(dir.path().join("root"), &[]),
            1,
            state,
            "Normal".into()
        ));
    }
    let mut m = Model::new(dir.path().join("root"));
    m.errors_total = 1;
    assert!(
        !service
            .capture()
            .submit(Arc::new(m), 2, ScanState::Complete, "Normal".into())
    );
    let mut m = Model::new(dir.path().join("root"));
    m.overflowed = true;
    assert!(
        !service
            .capture()
            .submit(Arc::new(m), 3, ScanState::Complete, "Normal".into())
    );
    assert!(service.capture().submit(
        model(dir.path().join("root"), &[]),
        4,
        ScanState::Complete,
        "Normal".into()
    ));
    let s = wait(&service, |s| s.scans.len() == 1 && !s.busy);
    assert_eq!(s.scans[0].live_generation, Some(4));
}

#[test]
fn retains_five_and_clear_leaves_unrelated_files_and_links() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let storage = dir.path().join("history");
    let service = HistoryService::new(storage.clone());
    wait(&service, |s| !s.busy);
    std::fs::write(storage.join("keep.txt"), b"unrelated").unwrap();
    for generation in 1..=7 {
        assert!(service.capture().submit(
            model(dir.path().join("root"), &[("a", generation, 1)]),
            generation,
            ScanState::Complete,
            "Normal".into()
        ));
        wait(&service, |s| {
            !s.busy
                && s.scans
                    .first()
                    .is_some_and(|x| x.live_generation == Some(generation))
        });
    }
    let s = wait_snapshot_count(&service, 5);
    assert_eq!(s.scans.last().unwrap().live_generation, Some(3));
    assert_eq!(
        std::fs::read_dir(&storage)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "dsh"))
            .count(),
        5
    );
    assert!(service.forget_all());
    wait(&service, |s| s.scans.is_empty() && !s.busy);
    assert_eq!(
        std::fs::read(storage.join("keep.txt")).unwrap(),
        b"unrelated"
    );
}
fn wait_snapshot_count(service: &HistoryService, count: usize) -> Arc<HistorySnapshot> {
    // Request a harmless compare to obtain fresh state after a previous poll consumed it.
    service.compare(0, 0);
    wait(service, |s| s.scans.len() == count && !s.busy)
}

#[test]
fn different_roots_are_incompatible_and_rows_are_capped() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let service = HistoryService::new(dir.path().join("history"));
    wait(&service, |s| !s.busy);
    service.capture().submit(
        model(dir.path().join("one"), &[]),
        1,
        ScanState::Complete,
        "Normal".into(),
    );
    let old = wait(&service, |s| s.scans.len() == 1 && !s.busy).scans[0].id;
    service.capture().submit(
        model(dir.path().join("two"), &[]),
        2,
        ScanState::Complete,
        "Normal".into(),
    );
    let new = wait(&service, |s| s.scans.len() == 2 && !s.busy).scans[0].id;
    service.compare(old, new);
    let s = wait(&service, |s| !s.busy && s.message.contains("compatible"));
    assert!(s.comparison.is_none());
    let entries: Vec<_> = (0..600).map(|i| (format!("file-{i}"), i + 1)).collect();
    let mut m = Model::new(dir.path().join("one"));
    m.insert_batch(
        0,
        entries
            .iter()
            .map(|(name, size)| Entry {
                name: name.into(),
                is_dir: false,
                reparse: false,
                logical: *size,
                allocated: Some(*size),
                modified: None,
                identity: None,
            })
            .collect(),
    );
    service
        .capture()
        .submit(Arc::new(m), 3, ScanState::Complete, "Normal".into());
    let new = wait(&service, |s| s.scans.len() == 3 && !s.busy).scans[0].id;
    service.compare(old, new);
    let s = wait(&service, |s| s.comparison.is_some() && !s.busy);
    let c = s.comparison.as_ref().unwrap();
    assert_eq!(c.added, 600);
    assert_eq!(c.rows.len(), 256);
    assert!(c.folder_changes.len() <= 8);
    assert!(
        c.rows
            .windows(2)
            .all(|p| p[0].delta.abs() >= p[1].delta.abs())
    );
}

#[test]
fn corrupted_metadata_never_becomes_a_complete_baseline() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let store = dir.path().join("history");
    let service = HistoryService::new(store.clone());
    wait(&service, |s| !s.busy);
    service.capture().submit(
        model(dir.path().join("scan"), &[("one", 42, 1)]),
        1,
        ScanState::Complete,
        "Normal".into(),
    );
    wait(&service, |s| s.scans.len() == 1 && !s.busy);
    drop(service);
    let file = std::fs::read_dir(&store)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().extension().is_some_and(|x| x == "dsh"))
        .unwrap()
        .path();
    let original = std::fs::read(&file).unwrap();
    let mut corrupt = original.clone();
    // A change to otherwise-valid metadata must be detected, not silently accepted.
    let index = corrupt.len() - 2;
    corrupt[index] ^= 1;
    std::fs::write(&file, &corrupt).unwrap();
    let loaded = HistoryService::new(store.clone());
    let s = wait(&loaded, |s| !s.busy);
    assert!(s.scans.is_empty());
    assert!(s.message.contains("corrupt"));
    drop(loaded);
    std::fs::write(&file, &original[..original.len() / 2]).unwrap();
    let loaded = HistoryService::new(store);
    assert!(wait(&loaded, |s| !s.busy).scans.is_empty());
}

#[test]
fn supports_practical_drive_metadata_budget() {
    const { assert!(MAX_NODES >= 1_000_000) };
    const { assert!(MAX_SNAPSHOT_BYTES >= 128 * 1024 * 1024) };
    assert_eq!(MAX_SNAPSHOTS, 5);
    assert!(MAX_DISK_BYTES <= (MAX_SNAPSHOT_BYTES as u64) * 6);
}

#[test]
fn ntfs_preview_and_normal_accounting_are_not_interchangeable_baselines() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let service = HistoryService::new(dir.path().join("history"));
    wait(&service, |s| !s.busy);
    let root = dir.path().join("root");
    service.capture().submit(
        model(root.clone(), &[("one", 1, 1)]),
        1,
        ScanState::Complete,
        "Normal (Windows directory metadata)".into(),
    );
    let old = wait(&service, |s| s.scans.len() == 1 && !s.busy).scans[0].id;
    service.capture().submit(
        model(root.clone(), &[("one", 2, 1)]),
        2,
        ScanState::Complete,
        "NTFS volume preview (unbenchmarked)".into(),
    );
    let new = wait(&service, |s| s.scans.len() == 2 && !s.busy).scans[0].id;
    service.compare(old, new);
    let s = wait(&service, |s| !s.busy && s.message.contains("compatible"));
    assert!(s.comparison.is_none());
    service.capture().submit(
        model(root, &[("one", 3, 1)]),
        3,
        ScanState::Complete,
        "Normal fallback (NTFS reader unavailable)".into(),
    );
    let fallback = wait(&service, |s| s.scans.len() == 3 && !s.busy).scans[0].id;
    service.compare(old, fallback);
    assert!(
        wait(&service, |s| !s.busy && s.comparison.is_some())
            .comparison
            .is_some()
    );
}

#[test]
fn clear_removes_only_own_staging_and_metadata_not_neighbor_files() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let store = dir.path().join("history");
    let service = HistoryService::new(store.clone());
    wait(&service, |s| !s.busy);
    std::fs::write(
        store.join("diskscope-0000000000000123.tmp"),
        b"interrupted own staging",
    )
    .unwrap();
    std::fs::write(store.join("diskscope-not-history.tmp"), b"unrelated").unwrap();
    service.forget_all();
    wait(&service, |s| !s.busy && s.message.contains("cleared"));
    assert!(!store.join("diskscope-0000000000000123.tmp").exists());
    assert!(store.join("diskscope-not-history.tmp").exists());
}

#[test]
fn replacing_history_directory_never_redirects_clear_to_new_contents() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let store = dir.path().join("history");
    let service = HistoryService::new(store.clone());
    wait(&service, |s| !s.busy);
    if let Err(error) = std::fs::rename(&store, dir.path().join("original-history")) {
        // Windows refuses the rename while the lifetime ownership file is open without
        // delete sharing. Preventing replacement also protects the directory binding.
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        return;
    }
    std::fs::create_dir(&store).unwrap();
    let protected = store.join("diskscope-0000000000000123.dsh");
    std::fs::write(&protected, b"different directory must not be touched").unwrap();
    service.forget_all();
    let s = wait(&service, |s| {
        !s.busy && s.message.contains("directory changed")
    });
    assert!(s.scans.is_empty());
    assert_eq!(
        std::fs::read(protected).unwrap(),
        b"different directory must not be touched"
    );
}

#[test]
fn competing_owners_cannot_exceed_retention_or_clear_each_others_history() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let store = dir.path().join("history");
    let first = HistoryService::new(store.clone());
    wait(&first, |s| !s.busy);
    let second = HistoryService::new(store.clone());
    let status = wait(&second, |s| !s.busy);
    let unavailable = status.message.contains("another window");
    for generation in 1..=6 {
        let service = if generation % 2 == 1 { &first } else { &second };
        let admitted = service.capture().submit(
            model(dir.path().join("root"), &[("a", generation, 1)]),
            generation,
            ScanState::Complete,
            "Normal".into(),
        );
        if generation % 2 == 0 && unavailable {
            assert!(!admitted);
        } else {
            assert!(admitted);
            wait(service, |s| {
                !s.busy
                    && s.scans
                        .first()
                        .is_some_and(|s| s.live_generation == Some(generation))
            });
        }
    }
    let count = std::fs::read_dir(&store)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "dsh"))
        .count();
    assert!(
        count <= MAX_SNAPSHOTS,
        "two owners retained {count} successful metadata files"
    );
    assert!(
        unavailable,
        "second owner must explicitly report exclusive ownership"
    );
    assert!(
        !second.forget_all(),
        "unavailable owner must not accept Clear"
    );
    assert_eq!(
        std::fs::read_dir(&store)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "dsh"))
            .count(),
        count
    );
}

#[test]
fn ownership_is_released_after_detached_shutdown_for_immediate_reopen() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let store = dir.path().join("history");
    for _ in 0..12 {
        let service = HistoryService::new(store.clone());
        let status = wait(&service, |s| !s.busy);
        assert!(!status.message.contains("unavailable"), "{status:?}");
        drop(service);
    }
}

#[test]
#[cfg(windows)]
fn unsafe_hardlinked_ownership_slot_is_rejected_without_changing_target() {
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let store = dir.path().join("history");
    std::fs::create_dir(&store).unwrap();
    let target = dir.path().join("keep.txt");
    std::fs::write(&target, b"not history").unwrap();
    std::fs::hard_link(&target, store.join(".diskscope-history.lock")).unwrap();
    let service = HistoryService::new(store);
    let status = wait(&service, |s| !s.busy);
    assert!(status.message.contains("hardlinked"), "{status:?}");
    assert!(!service.forget_all());
    assert_eq!(std::fs::read(target).unwrap(), b"not history");
}

#[cfg(windows)]
#[test]
fn native_unpaired_utf16_names_and_hierarchy_survive_history() {
    use std::os::windows::ffi::OsStringExt;
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
    let store = dir.path().join("history");
    let root = dir.path().join("scan");
    let make = |bytes| {
        let mut m = Model::new(root.clone());
        let folder = m.insert_batch(
            0,
            vec![Entry {
                name: "folder".into(),
                is_dir: true,
                reparse: false,
                logical: 0,
                allocated: Some(0),
                modified: None,
                identity: None,
            }],
        )[0];
        m.insert_batch(
            folder,
            vec![Entry {
                name: std::ffi::OsString::from_wide(&[0x61, 0xd800, 0x62]),
                is_dir: false,
                reparse: false,
                logical: bytes,
                allocated: Some(bytes),
                modified: Some(bytes),
                identity: Some(FileIdentity {
                    volume: 44,
                    id: [5; 16],
                }),
            }],
        );
        Arc::new(m)
    };
    let service = HistoryService::new(store.clone());
    wait(&service, |s| !s.busy);
    service
        .capture()
        .submit(make(10), 1, ScanState::Complete, "Normal".into());
    let old = wait(&service, |s| s.scans.len() == 1 && !s.busy).scans[0].id;
    service
        .capture()
        .submit(make(20), 2, ScanState::Complete, "Normal".into());
    let new = wait(&service, |s| s.scans.len() == 2 && !s.busy).scans[0].id;
    drop(service);
    let service = HistoryService::new(store);
    wait(&service, |s| !s.busy);
    service.compare(old, new);
    let s = wait(&service, |s| s.comparison.is_some() && !s.busy);
    let c = s.comparison.as_ref().unwrap();
    assert_eq!(c.modified, 1);
    assert_eq!(c.newer_generation, None);
    assert_eq!(
        c.rows[0].path,
        root.join("folder")
            .join(std::ffi::OsString::from_wide(&[0x61, 0xd800, 0x62]))
    );
    assert!(
        c.folder_changes
            .iter()
            .any(|f| f.path.ends_with("folder") && f.delta == 10)
    );
}
