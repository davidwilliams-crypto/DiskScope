use super::*;
use std::cell::Cell;

thread_local! {
    static VISITS: Cell<Option<usize>> = const { Cell::new(None) };
}
pub(super) fn record_visit() {
    VISITS.with(|visits| {
        if let Some(value) = visits.get() {
            visits.set(Some(value + 1));
        }
    });
}
fn counted<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    VISITS.with(|visits| assert_eq!(visits.replace(Some(0)), None));
    let result = operation();
    (result, VISITS.with(|visits| visits.replace(None).unwrap()))
}
fn file(name: &str, logical: u64) -> Entry {
    Entry {
        name: name.into(),
        is_dir: false,
        reparse: false,
        logical,
        allocated: Some(logical),
        modified: Some(20),
        identity: None,
    }
}
fn append(
    model: &mut Model,
    summary: &mut RootSummary,
    parent: NodeId,
    entries: Vec<Entry>,
) -> Vec<NodeId> {
    let ids = model.insert_batch(parent, entries);
    summary.append(model, &ids);
    ids
}
fn assert_equivalent(model: &Model, summary: &RootSummary, request: &ViewRequest) {
    let pure = build_view(model, request, 42);
    let cached = build_view_with_summary(model, request, 42, summary);
    // Every snapshot field is included, not just the newly aggregated fields.
    assert_eq!(
        format!("{cached:?}"),
        format!("{pure:?}"),
        "request={request:?}"
    );
}

#[test]
fn unfiltered_root_summary_avoids_visiting_the_complete_subtree() {
    let mut model = Model::new("summary-fixture".into());
    let mut summary = RootSummary::new();
    append(
        &mut model,
        &mut summary,
        0,
        (0..64)
            .map(|i| file(&format!("file-{i}.dat"), i + 1))
            .collect(),
    );
    for mode in [ViewMode::Overview, ViewMode::Types, ViewMode::Explorer] {
        let request = ViewRequest {
            mode,
            ..Default::default()
        };
        assert_equivalent(&model, &summary, &request);
        let (_, visits) = counted(|| build_view_with_summary(&model, &request, 42, &summary));
        assert_eq!(
            visits, 0,
            "cached {mode:?} still visited {visits} subtree nodes"
        );
    }
}

#[test]
fn public_queries_keep_observing_direct_mutations_without_revision_changes() {
    let mut model = Model::new("public-model".into());
    let mut summary = RootSummary::new();
    append(&mut model, &mut summary, 0, vec![file("original.dat", 5)]);
    let revision = model.revision;
    model.nodes[1].name = "renamed.LOG".into();
    model.nodes[1].logical = 17;
    // Same length, unchanged revision and unchanged root totals cannot certify
    // cached names/type groups. Public queries must observe these edits now.
    let renamed = build_view(
        &model,
        &ViewRequest {
            mode: ViewMode::Types,
            ..Default::default()
        },
        1,
    );
    assert_eq!(renamed.matching_files, 1);
    assert_eq!(renamed.matching_logical, 17);
    assert_eq!(renamed.types[0].extension, ".log");
    let mut direct = model.nodes[1].clone();
    direct.id = 2;
    direct.name = "manual.extra".into();
    direct.logical = 3;
    model.nodes.push(direct);
    model.nodes[0].children.push(2);
    assert_eq!(model.revision, revision);
    for mode in [ViewMode::Overview, ViewMode::Types, ViewMode::Explorer] {
        let view = build_view(
            &model,
            &ViewRequest {
                mode,
                ..Default::default()
            },
            1,
        );
        assert_eq!(view.matching_files, 2);
        assert_eq!(view.matching_logical, 20);
        if mode != ViewMode::Explorer {
            assert!(
                view.types
                    .iter()
                    .any(|stat| stat.extension == ".log" && stat.logical == 17)
            );
            assert!(
                view.types
                    .iter()
                    .any(|stat| stat.extension == ".extra" && stat.logical == 3)
            );
        }
    }
}

fn directory(name: &str) -> Entry {
    Entry {
        name: name.into(),
        is_dir: true,
        reparse: false,
        logical: 0,
        allocated: Some(0),
        modified: None,
        identity: None,
    }
}

fn check_root_modes(model: &Model, summary: &RootSummary) {
    for mode in [ViewMode::Overview, ViewMode::Types, ViewMode::Explorer] {
        for sort in [
            SortKey::Name,
            SortKey::Logical,
            SortKey::Allocated,
            SortKey::Files,
            SortKey::Modified,
        ] {
            for descending in [false, true] {
                for (offset, limit) in [
                    (0, 0),
                    (0, 1),
                    (1, 2),
                    (4, usize::MAX),
                    (usize::MAX, usize::MAX),
                ] {
                    let request = ViewRequest {
                        mode,
                        sort,
                        descending,
                        offset,
                        limit,
                        search: " \t ".into(),
                        selected: Some(1),
                        pinned: vec![1, 3],
                        expanded: [1, 2, 3].into(),
                        ..Default::default()
                    };
                    assert_equivalent(model, summary, &request);
                }
            }
        }
    }
}

#[test]
fn every_root_mode_sort_and_page_matches_pure_queries_after_each_batch() {
    let mut model = Model::new("incremental-root".into());
    let mut summary = RootSummary::new();
    append(
        &mut model,
        &mut summary,
        0,
        vec![directory("A"), directory("B")],
    );
    append(&mut model, &mut summary, 1, vec![directory("nested")]);
    check_root_modes(&model, &summary);
    for (parent, entries) in [
        (0, vec![file("alpha.TXT", 12), file("root.bin", 4)]),
        (1, vec![file("α.txt", 12), file(".gitignore", 0)]),
        (2, vec![file("same.TXT", 9), file("same.txt", 9)]),
        (3, vec![file("no extension", 4), file("nested.BIN", 31)]),
    ] {
        append(&mut model, &mut summary, parent, entries);
        check_root_modes(&model, &summary);
    }
}

#[test]
fn filtered_nested_largest_and_focus_queries_retain_the_pure_path() {
    let mut model = Model::new("scope-fixture".into());
    let mut summary = RootSummary::new();
    append(
        &mut model,
        &mut summary,
        0,
        vec![directory("nested"), file("outside.dat", 100)],
    );
    append(
        &mut model,
        &mut summary,
        1,
        vec![file("alpha.TXT", 10), file("local.dat", 30)],
    );
    let filtered = QueryFilter {
        extensions: vec![".dat".into()],
        min_bytes: Some(20),
        max_bytes: Some(200),
        modified_after: Some(1),
        modified_before: Some(40),
    };
    for mode in [
        ViewMode::Overview,
        ViewMode::Types,
        ViewMode::Explorer,
        ViewMode::Largest,
    ] {
        for request in [
            ViewRequest {
                mode,
                root: 1,
                ..Default::default()
            },
            ViewRequest {
                mode,
                search: "alpha".into(),
                ..Default::default()
            },
            ViewRequest {
                mode,
                filters: filtered.clone(),
                ..Default::default()
            },
            ViewRequest {
                mode,
                root: 1,
                search_all: true,
                filters: filtered.clone(),
                ..Default::default()
            },
            ViewRequest {
                mode,
                focus_path: Some(model.path(3)),
                ..Default::default()
            },
        ] {
            assert_equivalent(&model, &summary, &request);
            let (_, visits) = counted(|| build_view_with_summary(&model, &request, 42, &summary));
            assert!(
                visits > 0,
                "non-root/filtered/focused query unexpectedly bypassed traversal: {request:?}"
            );
        }
    }
    let largest = ViewRequest {
        mode: ViewMode::Largest,
        sort: SortKey::Name,
        ..Default::default()
    };
    assert_equivalent(&model, &summary, &largest);
    assert!(counted(|| build_view_with_summary(&model, &largest, 42, &summary)).1 > 0);
    // Focusing a root-level file still has an unfiltered root scope and may cache.
    let focused_root = ViewRequest {
        mode: ViewMode::Overview,
        focus_path: Some(model.path(2)),
        ..Default::default()
    };
    assert_equivalent(&model, &summary, &focused_root);
    assert_eq!(
        counted(|| build_view_with_summary(&model, &focused_root, 42, &summary)).1,
        0
    );
}

#[test]
fn indexed_largest_root_page_avoids_full_subtree_visits() {
    let mut model = Model::new("largest-summary".into());
    let mut summary = RootSummary::new();
    append(
        &mut model,
        &mut summary,
        0,
        (0..64)
            .map(|i| file(&format!("file-{i}.dat"), i + 1))
            .collect(),
    );
    for (offset, limit) in [
        (0, 1),
        (0, 100),
        (1, 3),
        (0, 0),
        (64, 1),
        (usize::MAX, usize::MAX),
    ] {
        let request = ViewRequest {
            mode: ViewMode::Largest,
            offset,
            limit,
            ..Default::default()
        };
        assert_equivalent(&model, &summary, &request);
        let (_, visits) = counted(|| build_view_with_summary(&model, &request, 42, &summary));
        assert_eq!(
            visits, 0,
            "indexed Largest offset={offset} limit={limit} visited {visits} subtree nodes"
        );
    }
    let focused = ViewRequest {
        mode: ViewMode::Largest,
        offset: usize::MAX,
        focus_path: Some(model.path(1)),
        ..Default::default()
    };
    assert_equivalent(&model, &summary, &focused);
    assert_eq!(
        counted(|| build_view_with_summary(&model, &focused, 42, &summary)).1,
        0
    );
}

fn append_topk_batches(model: &mut Model, summary: &mut RootSummary, zeros: bool) {
    append(model, summary, 0, vec![directory("A"), directory("B")]);
    let count = MAX_PAGE_ROWS + 97;
    for start in (0..count).step_by(128) {
        let entries = (start..(start + 128).min(count))
            .map(|index| {
                let name = [
                    "zeta.dat",
                    "same.dat",
                    "Same.dat",
                    "alpha.dat",
                    "alpha.dat",
                    "ß.dat",
                    "SS.dat",
                    "Ω.dat",
                ][index % 8];
                let logical = if zeros {
                    0
                } else if index % 19 == 0 {
                    u64::MAX
                } else {
                    (index % 11) as u64 * 10
                };
                let mut entry = file(name, logical);
                entry.allocated = if index % 3 == 0 {
                    None
                } else {
                    Some(index as u64 * 4096)
                };
                entry
            })
            .collect();
        append(
            model,
            summary,
            if start / 128 % 2 == 0 { 1 } else { 2 },
            entries,
        );
        assert_eq!(
            summary.largest.len(),
            (model.nodes[0].files as usize).min(MAX_PAGE_ROWS)
        );
        assert_eq!(summary.largest.capacity(), MAX_PAGE_ROWS);
        assert!(summary.largest.len() * std::mem::size_of::<NodeId>() <= 8192);
        assert_eq!(
            summary
                .largest
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            summary.largest.len()
        );
        for child in 1..summary.largest.len() {
            assert_ne!(
                compare_order(
                    model,
                    SortKey::Logical,
                    true,
                    summary.largest[(child - 1) / 2],
                    summary.largest[child]
                ),
                Ordering::Less,
                "heap root must retain the worst-ranked boundary"
            );
        }
        // The exact result must remain correct at every heap-growth/replacement boundary.
        assert_equivalent(
            model,
            summary,
            &ViewRequest {
                mode: ViewMode::Largest,
                limit: 100,
                ..Default::default()
            },
        );
    }
}

#[test]
fn largest_index_matches_exact_ids_at_k_boundaries_ties_zeroes_and_extreme_pages() {
    for zeros in [false, true] {
        let mut model = Model::new("largest-boundaries".into());
        let mut summary = RootSummary::new();
        append_topk_batches(&mut model, &mut summary, zeros);
        let count = MAX_PAGE_ROWS + 97;
        for (offset, limit, indexed) in [
            (0, 0, true),
            (0, 1, true),
            (0, 100, true),
            (0, MAX_PAGE_ROWS, true),
            (0, usize::MAX, true),
            (1, MAX_PAGE_ROWS - 1, true),
            (MAX_PAGE_ROWS - 1, 1, true),
            (MAX_PAGE_ROWS - 1, 2, false),
            (MAX_PAGE_ROWS, 1, false),
            (MAX_PAGE_ROWS + 1, 1, false),
            (count - 1, 1, false),
            (count, 1, true),
            (count + 1, usize::MAX, true),
            (usize::MAX, usize::MAX, true),
            (usize::MAX, 0, true),
            (1, usize::MAX, false),
        ] {
            let request = ViewRequest {
                mode: ViewMode::Largest,
                offset,
                limit,
                ..Default::default()
            };
            assert_equivalent(&model, &summary, &request);
            let view = build_view_with_summary(&model, &request, 42, &summary);
            assert_eq!(
                view.total_rows, count,
                "total_rows must not be the bounded index length"
            );
            assert_eq!(view.matching_files, count as u64);
            assert!(view.rows.iter().all(|row| !row.is_dir));
            let (_, visits) = counted(|| build_view_with_summary(&model, &request, 42, &summary));
            if indexed {
                assert_eq!(visits, 0, "offset={offset} limit={limit}");
            } else {
                assert!(visits > 0, "unsupported page must retain traversal");
            }
        }
    }
}

#[test]
fn largest_outside_the_index_window_or_order_and_filters_uses_pure_queries() {
    let mut model = Model::new("largest-fallback".into());
    let mut summary = RootSummary::new();
    append_topk_batches(&mut model, &mut summary, false);
    let base = ViewRequest {
        mode: ViewMode::Largest,
        ..Default::default()
    };
    for request in [
        ViewRequest {
            offset: MAX_PAGE_ROWS - 1,
            limit: 2,
            ..base.clone()
        },
        ViewRequest {
            offset: MAX_PAGE_ROWS,
            limit: 1,
            ..base.clone()
        },
        ViewRequest {
            offset: 1,
            limit: usize::MAX,
            ..base.clone()
        },
        ViewRequest {
            descending: false,
            ..base.clone()
        },
        ViewRequest {
            sort: SortKey::Name,
            ..base.clone()
        },
        ViewRequest {
            sort: SortKey::Name,
            limit: 0,
            ..base.clone()
        },
        ViewRequest {
            sort: SortKey::Allocated,
            ..base.clone()
        },
        ViewRequest {
            sort: SortKey::Allocated,
            offset: usize::MAX,
            ..base.clone()
        },
        ViewRequest {
            sort: SortKey::Modified,
            ..base.clone()
        },
        ViewRequest {
            sort: SortKey::Files,
            ..base.clone()
        },
        ViewRequest {
            root: 1,
            ..base.clone()
        },
        ViewRequest {
            search: "alpha".into(),
            ..base.clone()
        },
        ViewRequest {
            filters: QueryFilter {
                min_bytes: Some(1),
                ..Default::default()
            },
            ..base.clone()
        },
        ViewRequest {
            focus_path: Some(model.path(3)),
            ..base.clone()
        },
    ] {
        assert_equivalent(&model, &summary, &request);
        assert!(
            counted(|| build_view_with_summary(&model, &request, 42, &summary)).1 > 0,
            "unsupported index request must traverse: {request:?}"
        );
    }
}

#[test]
fn indexed_ids_keep_current_hardlink_flags_and_unknown_allocation() {
    let mut model = Model::new("largest-metadata".into());
    let mut summary = RootSummary::new();
    let mut first = file("same.dat", u64::MAX);
    first.identity = Some(FileIdentity {
        volume: 1,
        id: [9; 16],
    });
    first.allocated = None;
    append(&mut model, &mut summary, 0, vec![first.clone()]);
    first.name = "alias.dat".into();
    append(
        &mut model,
        &mut summary,
        0,
        vec![first, file("zero.dat", 0)],
    );
    let request = ViewRequest {
        mode: ViewMode::Largest,
        ..Default::default()
    };
    assert_equivalent(&model, &summary, &request);
    let view = build_view_with_summary(&model, &request, 42, &summary);
    assert_eq!(view.matching_files, 3);
    assert_eq!(view.matching_logical, u64::MAX);
    assert_eq!(view.total_rows, 3);
    assert!(
        view.rows
            .iter()
            .filter(|row| row.logical == u64::MAX)
            .all(|row| row.hard_link && row.allocated.is_none())
    );
}

#[cfg(windows)]
#[test]
fn indexed_boundary_native_name_and_id_ties_match_pure_sort() {
    use std::os::windows::ffi::OsStringExt;
    let mut model = Model::new("native-largest-boundary".into());
    let mut summary = RootSummary::new();
    let entries = (0..MAX_PAGE_ROWS + 4)
        .map(|index| {
            let mut entry = file("unused", 5);
            let unit = if index >= MAX_PAGE_ROWS {
                0xd800
            } else {
                0xd801
            };
            entry.name = std::ffi::OsString::from_wide(&[unit, 46, 100, 97, 116]);
            entry
        })
        .collect::<Vec<_>>();
    for batch in entries.chunks(128) {
        append(&mut model, &mut summary, 0, batch.to_vec());
    }
    for (offset, limit) in [(0, 8), (MAX_PAGE_ROWS - 8, 8), (MAX_PAGE_ROWS - 1, 2)] {
        assert_equivalent(
            &model,
            &summary,
            &ViewRequest {
                mode: ViewMode::Largest,
                offset,
                limit,
                ..Default::default()
            },
        );
    }
}

#[test]
fn per_entry_hardlinks_unknown_allocation_reparse_errors_and_overflow_stay_exact() {
    let mut model = Model::new("accounting-fixture".into());
    let mut summary = RootSummary::new();
    let mut reparse = directory("skipped-link");
    reparse.reparse = true;
    append(
        &mut model,
        &mut summary,
        0,
        vec![directory("A"), directory("B"), reparse],
    );
    let mut first = file("original.dat", 100);
    first.identity = Some(FileIdentity {
        volume: 7,
        id: [1; 16],
    });
    first.allocated = Some(4096);
    let mut alias = first.clone();
    alias.name = "alias.bin".into();
    append(&mut model, &mut summary, 1, vec![first]);
    append(&mut model, &mut summary, 2, vec![alias]);
    let mut unknown = file("unknown", 9);
    unknown.allocated = None;
    let mut reparse_file = file("visible.link", 5);
    reparse_file.reparse = true;
    append(&mut model, &mut summary, 0, vec![unknown, reparse_file]);
    assert_eq!(model.hardlinks, 1);
    assert_eq!(model.unique_allocated, None);
    let request = ViewRequest {
        mode: ViewMode::Types,
        ..Default::default()
    };
    let cached = build_view_with_summary(&model, &request, 42, &summary);
    assert_eq!(cached.matching_files, 4);
    assert_eq!(cached.matching_logical, 214);
    assert!(
        cached
            .types
            .iter()
            .any(|t| t.extension == ".dat" && t.logical == 100 && t.files == 1)
    );
    assert!(
        cached
            .types
            .iter()
            .any(|t| t.extension == ".bin" && t.logical == 100 && t.files == 1)
    );
    check_root_modes(&model, &summary);
    append(
        &mut model,
        &mut summary,
        1,
        vec![file("huge.dat", u64::MAX), file("overflow.dat", 1)],
    );
    assert!(model.overflowed);
    model.record_error("denied".into(), "injected failure".into());
    for node in &mut model.nodes {
        if node.is_dir {
            node.allocation_complete = false;
        }
    }
    check_root_modes(&model, &summary);
    let cached = build_view_with_summary(&model, &request, 42, &summary);
    assert_eq!(cached.matching_logical, u64::MAX);
    assert!(
        cached
            .types
            .iter()
            .any(|t| t.extension == ".dat" && t.logical == u64::MAX && t.files == 3)
    );
    // Invalid parents insert no IDs and therefore cannot pollute cached stats.
    assert!(
        append(
            &mut model,
            &mut summary,
            u32::MAX,
            vec![file("not-inserted.bad", 42)]
        )
        .is_empty()
    );
    assert_equivalent(&model, &summary, &request);
}

#[cfg(windows)]
#[test]
fn native_extensions_follow_the_same_lossy_case_normalization_as_pure_queries() {
    use std::os::windows::ffi::OsStringExt;
    let mut model = Model::new("native-summary".into());
    let mut summary = RootSummary::new();
    let entries = [0xd800, 0xd801]
        .into_iter()
        .enumerate()
        .map(|(index, unit)| {
            let mut entry = file("unused", index as u64 + 1);
            entry.name = std::ffi::OsString::from_wide(&[120, 46, unit]);
            entry
        })
        .collect();
    append(&mut model, &mut summary, 0, entries);
    for mode in [ViewMode::Overview, ViewMode::Types, ViewMode::Explorer] {
        assert_equivalent(
            &model,
            &summary,
            &ViewRequest {
                mode,
                ..Default::default()
            },
        );
    }
}
