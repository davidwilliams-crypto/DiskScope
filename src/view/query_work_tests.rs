use super::*;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Work {
    match_evaluations: usize,
    match_storage: usize,
    match_storage_bytes: usize,
    allocation_checks: usize,
    filter_normalizations: usize,
    candidate_storage: usize,
    traversal_storage: usize,
}
thread_local! {
    static WORK: Cell<Option<Work>> = const { Cell::new(None) };
}
fn record(update: impl FnOnce(&mut Work)) {
    WORK.with(|work| {
        if let Some(mut value) = work.get() {
            update(&mut value);
            work.set(Some(value));
        }
    });
}
pub(super) fn record_match_evaluation() {
    record(|work| work.match_evaluations += 1);
}
pub(super) fn record_match_storage(amount: usize) {
    record(|work| work.match_storage += amount);
}
pub(super) fn record_match_bytes(amount: usize) {
    record(|work| work.match_storage_bytes += amount);
}
pub(super) fn record_allocation_check() {
    record(|work| work.allocation_checks += 1);
}
pub(super) fn record_filter_normalization() {
    record(|work| work.filter_normalizations += 1);
}
pub(super) fn record_candidate_storage(amount: usize) {
    record(|work| work.candidate_storage = work.candidate_storage.max(amount));
}
pub(super) fn record_traversal_storage(amount: usize) {
    record(|work| work.traversal_storage = work.traversal_storage.max(amount));
}
fn counted<T>(operation: impl FnOnce() -> T) -> (T, Work) {
    WORK.with(|work| assert_eq!(work.replace(Some(Work::default())), None));
    let result = operation();
    (result, WORK.with(|work| work.replace(None).unwrap()))
}
fn entry(name: &str, is_dir: bool, logical: u64, identity: u64) -> Entry {
    let mut id = [0; 16];
    id[..8].copy_from_slice(&identity.to_le_bytes());
    Entry {
        name: name.into(),
        is_dir,
        reparse: false,
        logical,
        allocated: Some(logical),
        modified: Some(100),
        identity: Some(FileIdentity { volume: 1, id }),
    }
}
fn fixture(unrelated: usize) -> (Model, RootSummary) {
    let mut model = Model::new("scoped-query".into());
    let mut summary = RootSummary::new();
    for (parent, entries) in [
        (
            0,
            vec![entry("local", true, 0, 1), entry("unrelated", true, 0, 2)],
        ),
        (1, vec![entry("needle.TXT", false, 7, 3)]),
        (
            2,
            (0..unrelated)
                .map(|i| entry("outside.txt", false, 1, i as u64 + 4))
                .collect(),
        ),
    ] {
        let ids = model.insert_batch(parent, entries);
        summary.append(&model, &ids);
    }
    (model, summary)
}

#[test]
fn wide_query_scratch_is_bounded_by_page_and_depth() {
    let (model, _) = fixture(30_000);
    for limit in [0, 1, 100] {
        let (view, work) = counted(|| {
            build_view(
                &model,
                &ViewRequest {
                    mode: ViewMode::Largest,
                    search: "txt".into(),
                    limit,
                    ..Default::default()
                },
                1,
            )
        });
        assert_eq!(view.total_rows, 30_001);
        assert_eq!(view.rows.len(), limit);
        // Folder breakdown independently retains at most twelve candidate IDs.
        assert!(
            work.candidate_storage <= (2 * limit).max(12),
            "page scratch: {work:?}"
        );
        assert!(
            work.traversal_storage <= 3,
            "wide traversal scratch: {work:?}"
        );
    }
}

#[test]
fn depth_first_walk_preserves_preorder_without_a_sibling_queue() {
    let (model, _) = fixture(5);
    let (ids, work) = counted(|| {
        let mut ids = Vec::new();
        visit_subtree(&model, 0, |id| ids.push(id));
        ids
    });
    assert_eq!(ids, vec![0, 1, 3, 2, 4, 5, 6, 7, 8]);
    assert!(work.traversal_storage <= 3, "{work:?}");
}

#[test]
fn invalid_search_does_not_traverse_matches_or_allocate_match_flags() {
    let (model, summary) = fixture(30_000);
    let (view, work) = counted(|| {
        build_view_with_summary(
            &model,
            &ViewRequest {
                mode: ViewMode::Largest,
                search: "size:invalid".into(),
                ..Default::default()
            },
            1,
            &summary,
        )
    });
    assert_eq!(view.total_rows, 0);
    assert_eq!(work.match_evaluations, 0);
    assert_eq!(work.match_storage_bytes, 0);
}

#[test]
fn local_filtered_queries_do_not_visit_or_allocate_for_unrelated_files() {
    for unrelated in [10, 10_000] {
        let (model, summary) = fixture(unrelated);
        for mode in [
            ViewMode::Explorer,
            ViewMode::Largest,
            ViewMode::Types,
            ViewMode::Overview,
        ] {
            let request = ViewRequest {
                root: 1,
                mode,
                search: "needle".into(),
                filters: QueryFilter {
                    extensions: vec![".tXt".into()],
                    ..Default::default()
                },
                ..Default::default()
            };
            let (view, work) = counted(|| build_view_with_summary(&model, &request, 1, &summary));
            assert_eq!((view.matching_files, view.matching_logical), (1, 7));
            assert!(view.map.iter().all(|tile| tile.matched));
            assert!(
                work.match_evaluations <= 2,
                "local folder query scanned unrelated nodes: {work:?}"
            );
            assert!(
                work.match_storage <= 2,
                "local folder allocated scan-wide match storage: {work:?}"
            );
            assert!(
                work.allocation_checks <= 2,
                "local allocation scanned unrelated nodes: {work:?}"
            );
            assert_eq!(
                work.filter_normalizations, 1,
                "extension filters should normalize once: {work:?}"
            );
        }
    }
}

#[test]
fn unfiltered_queries_do_not_allocate_match_storage() {
    let (model, summary) = fixture(10_000);
    for root in [0, 1] {
        let (_, work) = counted(|| {
            build_view_with_summary(
                &model,
                &ViewRequest {
                    root,
                    ..Default::default()
                },
                1,
                &summary,
            )
        });
        assert_eq!(
            work.match_storage, 0,
            "unfiltered query allocated matching scratch: {work:?}"
        );
    }
}

#[test]
fn completed_cached_root_queries_do_not_sweep_allocation_completeness() {
    let (mut model, summary) = fixture(10_000);
    for complete in [true, false, true] {
        // Scan keeps its root incomplete until every directory has completed,
        // and clears every directory on cancellation or partial completion.
        for node in &mut model.nodes {
            if node.is_dir {
                node.allocation_complete = complete;
            }
        }
        for mode in [
            ViewMode::Explorer,
            ViewMode::Largest,
            ViewMode::Types,
            ViewMode::Overview,
        ] {
            let (view, work) = counted(|| {
                build_view_with_summary(
                    &model,
                    &ViewRequest {
                        mode,
                        ..Default::default()
                    },
                    1,
                    &summary,
                )
            });
            assert_eq!(view.unique_allocated, complete.then_some(10_007));
            assert!(
                work.allocation_checks <= 1,
                "cached root rescanned completeness: {work:?}"
            );
            assert_eq!(work.match_evaluations, 0);
            assert_eq!(work.match_storage, 0);
        }
    }
}

#[test]
fn search_all_evaluates_each_file_once_and_keeps_local_map_highlighting() {
    let (model, summary) = fixture(10);
    let request = ViewRequest {
        root: 1,
        mode: ViewMode::Largest,
        search: "outside".into(),
        search_all: true,
        ..Default::default()
    };
    let (view, work) = counted(|| build_view_with_summary(&model, &request, 1, &summary));
    assert_eq!((view.matching_files, view.matching_logical), (10, 10));
    assert_eq!((view.row_scope_root, view.row_scope_logical), (0, 17));
    assert!(view.rows.iter().all(|row| row.name == "outside.txt"));
    assert!(view.map.iter().all(|tile| !tile.matched));
    assert!(
        work.match_evaluations <= model.nodes.len(),
        "search-all evaluated the same nodes twice: {work:?}"
    );
}

#[test]
fn whole_scan_lenses_keep_matching_scratch_within_one_byte_per_node() {
    let (model, summary) = fixture(10_000);
    for extension in [".txt", ".missing"] {
        let request = ViewRequest {
            root: 1,
            search_all: true,
            mode: ViewMode::Types,
            filters: QueryFilter {
                extensions: vec![extension.into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let (view, work) = counted(|| build_view_with_summary(&model, &request, 1, &summary));
        assert_eq!(
            view.matching_files,
            if extension == ".txt" { 10_001 } else { 0 }
        );
        assert!(
            work.match_storage_bytes <= model.nodes.len(),
            "whole-scan matching scratch exceeds one byte/node: {work:?}"
        );
    }
}

#[test]
fn nested_other_highlighting_uses_omitted_matches_in_its_own_parent() {
    let mut model = Model::new("nested-other".into());
    model.insert_batch(0, vec![entry("folder", true, 0, 1)]);
    model.insert_batch(
        1,
        (0..2100)
            .map(|i| {
                entry(
                    if i == 0 {
                        "visible-match.txt"
                    } else if i == 2099 {
                        "hidden-match.txt"
                    } else {
                        "unmatched.dat"
                    },
                    false,
                    1,
                    i + 2,
                )
            })
            .collect(),
    );
    for (search, expected_other) in [("visible-match", false), ("hidden-match", true)] {
        let view = build_view(
            &model,
            &ViewRequest {
                map_layout: MapLayout::Icicle,
                search: search.into(),
                ..Default::default()
            },
            1,
        );
        let other = view.map.iter().find(|tile| tile.others).unwrap();
        assert_eq!(other.id, 1);
        assert_eq!(other.matched, expected_other);
        assert_eq!(view.matching_files, 1);
    }
}
