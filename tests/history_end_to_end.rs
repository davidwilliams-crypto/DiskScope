#![cfg(windows)]

use diskscope::{
    engine::Controller,
    history::{HistoryService, HistorySnapshot},
    model::BackendChoice,
};
use std::{
    fs,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn history_until(
    service: &HistoryService,
    ready: impl Fn(&HistorySnapshot) -> bool,
) -> Arc<HistorySnapshot> {
    let started = Instant::now();
    loop {
        if let Some(snapshot) = service.poll()
            && ready(&snapshot)
        {
            return snapshot;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "history operation timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn scan(controller: &mut Controller, service: &HistoryService, root: &std::path::Path) -> u64 {
    let generation = controller.start(root.to_owned(), BackendChoice::Normal);
    let started = Instant::now();
    loop {
        if let Some(snapshot) = controller.poll()
            && snapshot.progress.generation == generation
            && !snapshot.progress.state.is_active()
        {
            assert_eq!(
                snapshot.progress.state,
                diskscope::model::ScanState::Complete
            );
            assert_eq!(snapshot.progress.errors_total, 0);
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "scan timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let saved = history_until(service, |s| {
        s.scans
            .iter()
            .any(|scan| scan.live_generation == Some(generation))
    });
    saved
        .scans
        .iter()
        .find(|scan| scan.live_generation == Some(generation))
        .unwrap()
        .id
}

#[test]
fn actual_two_scan_history_preserves_moves_growth_and_reload() {
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    fs::create_dir_all(&workspace).unwrap();
    let fixture = tempfile::tempdir_in(workspace).unwrap();
    let root = fixture.path().join("scan");
    let directory = fixture.path().join("history");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.dat"), vec![0; 100]).unwrap();
    fs::write(root.join("sub/grow.bin"), vec![0; 200]).unwrap();
    fs::write(root.join("old.dat"), vec![0; 50]).unwrap();
    fs::write(root.join("move.dat"), vec![0; 70]).unwrap();
    let history = HistoryService::new(directory.clone());
    let mut controller = Controller::with_history(history.capture());
    let older = scan(&mut controller, &history, &root);

    fs::write(root.join("sub/grow.bin"), vec![0; 500]).unwrap();
    fs::remove_file(root.join("old.dat")).unwrap();
    fs::rename(root.join("move.dat"), root.join("relocated.dat")).unwrap();
    fs::write(root.join("new.dat"), vec![0; 30]).unwrap();
    let newer = scan(&mut controller, &history, &root);
    assert!(history.compare(older, newer));
    let compared = history_until(&history, |s| {
        s.comparison
            .as_ref()
            .is_some_and(|c| c.older_id == older && c.newer_id == newer)
    });
    let comparison = compared.comparison.as_ref().unwrap();
    assert_eq!(
        (comparison.before, comparison.after, comparison.delta),
        (420, 700, 280)
    );
    assert_eq!(
        (
            comparison.added,
            comparison.removed,
            comparison.modified,
            comparison.moved
        ),
        (1, 1, 1, 1)
    );
    assert!(
        comparison
            .folder_changes
            .iter()
            .any(|change| change.path.ends_with("sub") && change.delta == 300)
    );
    drop(controller);
    drop(history);

    let reloaded = HistoryService::new(directory);
    let catalog = history_until(&reloaded, |s| s.scans.len() == 2);
    assert!(
        catalog
            .scans
            .iter()
            .all(|scan| scan.live_generation.is_none())
    );
    assert!(reloaded.compare(older, newer));
    let compared = history_until(&reloaded, |s| {
        s.comparison
            .as_ref()
            .is_some_and(|c| c.older_id == older && c.newer_id == newer)
    });
    assert_eq!(compared.comparison.as_ref().unwrap().delta, 280);
    assert_eq!(compared.comparison.as_ref().unwrap().newer_generation, None);
}
