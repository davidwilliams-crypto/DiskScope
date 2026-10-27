use diskscope::{
    engine::Controller,
    model::{BackendChoice, ScanState, UiSnapshot, ViewRequest},
};
use std::{
    fs,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn terminal(controller: &Controller, generation: u64) -> Arc<UiSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(snapshot) = controller.poll()
            && snapshot.progress.generation == generation
            && !snapshot.progress.state.is_active()
        {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "scan {generation} did not terminate"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn scan(path: &Path) -> Arc<UiSnapshot> {
    let mut controller = Controller::new();
    let generation = controller.start(path.into(), BackendChoice::Normal);
    terminal(&controller, generation)
}

#[test]
fn nested_empty_unicode_and_hardlinks_have_additive_logical_totals() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("資料").join("empty")).unwrap();
    fs::write(temp.path().join("資料").join("é🙂.bin"), [7; 19]).unwrap();
    fs::hard_link(
        temp.path().join("資料").join("é🙂.bin"),
        temp.path().join("alias.bin"),
    )
    .unwrap();
    fs::write(temp.path().join("zero"), []).unwrap();
    let result = scan(temp.path());
    assert_eq!(result.progress.state, ScanState::Complete);
    assert_eq!(
        (
            result.progress.logical,
            result.progress.files,
            result.progress.folders
        ),
        (38, 3, 2)
    );
    assert_eq!(result.progress.errors_total, 0);
    #[cfg(windows)]
    {
        assert_eq!(result.progress.hardlinks, 1);
        assert!(result.progress.unique_allocated.is_some());
    }
}

#[test]
fn missing_root_reports_failure() {
    let temp = tempfile::tempdir().unwrap();
    let result = scan(&temp.path().join("missing"));
    assert_eq!(result.progress.state, ScanState::Failed);
    assert_eq!(result.progress.errors_total, 1);
}

#[test]
fn publication_pressure_does_not_block_cancel_or_latest_scan_replacement() {
    let large = tempfile::tempdir().unwrap();
    for i in 0..600 {
        fs::create_dir(large.path().join(format!("d{i}"))).unwrap();
        fs::write(large.path().join(format!("d{i}/payload")), [1; 31]).unwrap();
    }
    let small = tempfile::tempdir().unwrap();
    fs::write(small.path().join("winner"), [1; 7]).unwrap();
    let mut controller = Controller::new();
    let started = Instant::now();
    controller.start(large.path().into(), BackendChoice::Normal);
    // Deliberately do not poll: the latest snapshot slot stays full.
    thread::sleep(Duration::from_millis(100));
    controller.cancel();
    for revision in 0..300 {
        controller.request_view(ViewRequest {
            revision,
            ..Default::default()
        });
    }
    controller.start(large.path().into(), BackendChoice::Normal);
    let generation = controller.start(small.path().into(), BackendChoice::Normal);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "UI-facing commands blocked"
    );
    let result = terminal(&controller, generation);
    assert_eq!(result.progress.state, ScanState::Complete);
    assert_eq!(result.progress.root, small.path());
    assert_eq!(result.progress.logical, 7);
    assert_eq!(result.view.generation, generation);
}

#[test]
fn cancellation_is_terminal_without_draining_publications() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..1000 {
        fs::create_dir(temp.path().join(format!("d{i}"))).unwrap();
    }
    let mut controller = Controller::new();
    let generation = controller.start(temp.path().into(), BackendChoice::Normal);
    controller.cancel();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        terminal(&controller, generation).progress.state,
        ScanState::Cancelled
    );
}

#[test]
fn dropping_a_controller_does_not_join_workers() {
    let temp = tempfile::tempdir().unwrap();
    let mut controller = Controller::new();
    controller.start(temp.path().into(), BackendChoice::Normal);
    let started = Instant::now();
    drop(controller);
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn latest_view_request_is_applied_after_scan_completion() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("small"), [1; 3]).unwrap();
    fs::write(temp.path().join("large"), [1; 19]).unwrap();
    let mut controller = Controller::new();
    let generation = controller.start(temp.path().into(), BackendChoice::Normal);
    terminal(&controller, generation);
    for revision in 1..=500 {
        controller.request_view(ViewRequest {
            revision,
            search: if revision == 500 { "small" } else { "large" }.into(),
            ..Default::default()
        });
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(snapshot) = controller.poll()
            && snapshot.view.revision == 500
        {
            assert_eq!(snapshot.view.rows.len(), 1);
            assert_eq!(snapshot.view.rows[0].logical, 3);
            break;
        }
        assert!(Instant::now() < deadline, "latest view was lost");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn empty_directory_completes_with_known_zero_totals() {
    let temp = tempfile::tempdir().unwrap();
    let result = scan(temp.path());
    assert_eq!(result.progress.state, ScanState::Complete);
    assert_eq!(
        (
            result.progress.logical,
            result.progress.files,
            result.progress.folders
        ),
        (0, 0, 0)
    );
    assert_eq!(result.progress.unique_allocated, Some(0));
}

#[test]
fn ntfs_folder_request_uses_normal_worker_and_preserves_reason() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("normal-fallback"), [1; 29]).unwrap();
    let mut controller = Controller::new();
    let generation = controller.start(temp.path().into(), BackendChoice::Ntfs);
    let snapshot = terminal(&controller, generation);
    assert_eq!(snapshot.progress.state, ScanState::Complete);
    assert_eq!(snapshot.progress.logical, 29);
    assert_eq!(snapshot.progress.files, 1);
    assert!(snapshot.progress.backend.contains("Normal fallback"));
    assert!(snapshot.progress.message.contains("whole local drive root"));
}

#[test]
fn platform_emits_bounded_batches_and_observes_cancellation() {
    use diskscope::platform::{EnumerationEvent, enumerate};
    use std::cell::Cell;
    let temp = tempfile::tempdir().unwrap();
    for i in 0..300 {
        fs::write(temp.path().join(format!("entry-{i}")), [1; 2]).unwrap();
    }
    let cancelled = Cell::new(false);
    let mut seen = 0;
    enumerate(
        temp.path(),
        || cancelled.get(),
        |event| {
            if let EnumerationEvent::Entries(entries) = event {
                assert!(entries.len() <= 128);
                seen += entries.len();
                cancelled.set(true);
            }
            true
        },
    )
    .unwrap();
    assert_eq!(
        seen, 128,
        "enumerator must stop before another batch after cancellation"
    );
}

#[cfg(windows)]
#[test]
fn non_unicode_windows_name_is_preserved_in_snapshot_paths() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let temp = tempfile::tempdir().unwrap();
    let name = OsString::from_wide(&[b'a' as u16, 0xd800, b'z' as u16]);
    let path = temp.path().join(&name);
    fs::write(&path, [1; 13]).unwrap();
    let result = scan(temp.path());
    assert_eq!(result.progress.logical, 13);
    assert_eq!(result.view.rows[0].path, path);
    assert_eq!(result.view.rows[0].path.file_name().unwrap(), name);
}

#[cfg(windows)]
#[test]
fn directory_junction_is_visible_but_not_traversed() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("outside"), [1; 333]).unwrap();
    let link = temp.path().join("junction");
    let output = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = scan(temp.path());
    assert_eq!(result.progress.logical, 0);
    assert_eq!(result.progress.skipped, 1);
    assert_eq!(result.progress.folders, 1);
    fs::remove_dir(link).unwrap();
}

#[cfg(windows)]
#[test]
fn sparse_file_uses_allocation_instead_of_logical_length() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sparse.bin");
    fs::write(&path, []).unwrap();
    let output = Command::new("fsutil")
        .args(["sparse", "setflag"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "sparse fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(16 * 1024 * 1024)
        .unwrap();
    let result = scan(temp.path());
    assert_eq!(result.progress.logical, 16 * 1024 * 1024);
    assert!(result.progress.unique_allocated.unwrap() < result.progress.logical);
}
