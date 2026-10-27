//! Full-sort oracle and deterministic work-count regressions for page ranking.
use super::*;
use std::cell::Cell;

thread_local! {
    static COMPARISONS: Cell<Option<usize>> = const { Cell::new(None) };
}

pub(super) fn record_comparison() {
    COMPARISONS.with(|counter| {
        if let Some(value) = counter.get() {
            counter.set(Some(value + 1));
        }
    });
}

fn counted<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    COMPARISONS.with(|counter| assert_eq!(counter.replace(Some(0)), None));
    let result = operation();
    let count = COMPARISONS.with(|counter| counter.replace(None).unwrap());
    (result, count)
}

fn file(name: &str, index: usize) -> Entry {
    Entry {
        name: name.into(),
        is_dir: false,
        reparse: false,
        logical: [0, 10, 10, 40, 80][index % 5],
        allocated: [None, Some(0), Some(4096), Some(4096)][index % 4],
        modified: [None, Some(0), Some(20), Some(40), Some(60)][index % 5],
        identity: None,
    }
}

fn oracle_fixture() -> Model {
    let mut model = Model::new("ranking-fixture".into());
    let directory = |name: &str| Entry {
        name: name.into(),
        is_dir: true,
        reparse: false,
        logical: 0,
        allocated: Some(0),
        modified: None,
        identity: None,
    };
    model.insert_batch(0, vec![directory("A"), directory("B")]);
    model.insert_batch(1, vec![directory("nested")]);
    for (parent, names) in [
        (
            0,
            [
                "alpha.dat",
                "Alpha.dat",
                "alpha.dat",
                "zeta.bin",
                "equal",
                "same.dat",
            ],
        ),
        (
            1,
            ["SS.dat", "ß.dat", "Σ.dat", "σ.dat", "ALPHA.dat", "same.dat"],
        ),
        (
            2,
            ["a.bin", "b.dat", "a.bin", "no extension", "İ.dat", "i.dat"],
        ),
        (
            3,
            [
                "alpha.dat",
                "Alpha.dat",
                "same.dat",
                "same.dat",
                "omega.bin",
                "zero.dat",
            ],
        ),
    ] {
        model.insert_batch(
            parent,
            names
                .iter()
                .enumerate()
                .map(|(i, name)| file(name, i))
                .collect(),
        );
    }
    model
}

/// The former algorithm: collect every matching ID, fully sort, then page.
fn full_sort_oracle(model: &Model, request: &ViewRequest) -> Vec<NodeId> {
    let filtered = !request.search.trim().is_empty() || request.filters.is_active();
    let scope = if request.search_all && filtered {
        0
    } else {
        request.root
    };
    let query = request.search.trim().to_lowercase();
    let mut ids = Vec::new();
    visit_subtree(model, scope, |id| {
        if file_matches(&model.nodes[id as usize], &query, &request.filters) {
            ids.push(id);
        }
    });
    ids.sort_unstable_by(|a, b| compare(model, request, *a, *b));
    ids
}

// Keep the former predicate as an independent reference for the prepared filter.
fn file_matches(node: &Node, query: &str, filters: &QueryFilter) -> bool {
    !node.is_dir
        && (query.is_empty() || node.name.to_string_lossy().to_lowercase().contains(query))
        && (filters.extensions.is_empty()
            || filters
                .extensions
                .iter()
                .any(|e| e.to_lowercase() == extension(node)))
        && filters.min_bytes.is_none_or(|min| node.logical >= min)
        && filters.max_bytes.is_none_or(|max| node.logical <= max)
        && filters
            .modified_after
            .is_none_or(|after| node.modified.is_some_and(|m| m >= after))
        && filters
            .modified_before
            .is_none_or(|before| node.modified.is_some_and(|m| m <= before))
}

fn assert_exact_page(model: &Model, request: &ViewRequest) {
    let oracle = full_sort_oracle(model, request);
    let view = build_view(model, request, 17);
    let expected: Vec<_> = oracle
        .iter()
        .copied()
        .skip(request.offset)
        .take(request.limit.min(MAX_PAGE_ROWS))
        .collect();
    assert_eq!(
        view.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected,
        "mode={:?}, sort={:?}, descending={}, root={}, all={}, offset={}, limit={}",
        request.mode,
        request.sort,
        request.descending,
        request.root,
        request.search_all,
        request.offset,
        request.limit
    );
    assert_eq!(view.total_rows, oracle.len());
    assert_eq!(view.matching_files, oracle.len() as u64);
    assert_eq!(
        view.matching_logical,
        oracle
            .iter()
            .map(|id| model.nodes[*id as usize].logical)
            .sum::<u64>()
    );
    assert_eq!(
        view.total_logical,
        model.nodes[request.root as usize].logical
    );
}

#[test]
fn every_sort_direction_and_filter_page_matches_full_sort_ids() {
    let model = oracle_fixture();
    let requests = [
        ViewRequest {
            mode: ViewMode::Largest,
            ..Default::default()
        },
        ViewRequest {
            mode: ViewMode::Largest,
            root: 1,
            search: "a".into(),
            ..Default::default()
        },
        ViewRequest {
            mode: ViewMode::Largest,
            root: 1,
            filters: QueryFilter {
                extensions: vec![".DAT".into()],
                min_bytes: Some(10),
                max_bytes: Some(80),
                modified_after: Some(20),
                modified_before: Some(60),
            },
            ..Default::default()
        },
        ViewRequest {
            mode: ViewMode::Explorer,
            root: 1,
            search_all: true,
            filters: QueryFilter {
                extensions: vec![".dat".into()],
                ..Default::default()
            },
            ..Default::default()
        },
        ViewRequest {
            mode: ViewMode::Explorer,
            root: 1,
            search: "alpha".into(),
            ..Default::default()
        },
    ];
    for base in requests {
        let matches = full_sort_oracle(&model, &base).len();
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
                    (0, 3),
                    (1, 2),
                    (3, 3),
                    (matches.saturating_sub(1), 2),
                    (matches, 1),
                    (matches.saturating_add(1), usize::MAX),
                    (usize::MAX, usize::MAX),
                    (0, MAX_PAGE_ROWS + 1),
                ] {
                    assert_exact_page(
                        &model,
                        &ViewRequest {
                            sort,
                            descending,
                            offset,
                            limit,
                            ..base.clone()
                        },
                    );
                }
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn native_names_with_identical_lossy_display_keep_exact_ranked_ids() {
    use std::os::windows::ffi::OsStringExt;
    let mut model = oracle_fixture();
    let first = model.nodes.len() as NodeId;
    let entries = [0xd800, 0xd801]
        .into_iter()
        .map(|unit| {
            let mut entry = file("unused", 1);
            entry.name = std::ffi::OsString::from_wide(&[unit, 46, 100, 97, 116]);
            entry
        })
        .collect();
    model.insert_batch(0, entries);
    assert_eq!(
        model.nodes[first as usize].name.to_string_lossy(),
        model.nodes[first as usize + 1].name.to_string_lossy()
    );
    assert_ne!(
        model.nodes[first as usize].name,
        model.nodes[first as usize + 1].name
    );
    for sort in [
        SortKey::Name,
        SortKey::Logical,
        SortKey::Allocated,
        SortKey::Files,
        SortKey::Modified,
    ] {
        for descending in [false, true] {
            for offset in 0..model.nodes.len() {
                assert_exact_page(
                    &model,
                    &ViewRequest {
                        mode: ViewMode::Largest,
                        sort,
                        descending,
                        offset,
                        limit: 1,
                        ..Default::default()
                    },
                );
            }
        }
    }
}

fn shuffled_model(count: usize) -> Model {
    let mut order: Vec<_> = (0..count).collect();
    let mut seed = 0x1234_5678u32;
    for index in (1..count).rev() {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        order.swap(index, seed as usize % (index + 1));
    }
    let mut model = Model::new("ranking-work-fixture".into());
    model.insert_batch(
        0,
        order
            .into_iter()
            .map(|value| Entry {
                name: format!("item-{value:08}.dat").into(),
                is_dir: false,
                reparse: false,
                logical: value as u64 + 1,
                allocated: Some(0),
                modified: None,
                identity: None,
            })
            .collect(),
    );
    model
}

#[test]
fn a_bounded_page_uses_less_than_half_the_full_sort_comparisons() {
    let model = shuffled_model(32_768);
    for offset in [0, 16_384, 32_640] {
        let request = ViewRequest {
            mode: ViewMode::Largest,
            offset,
            limit: 128,
            ..Default::default()
        };
        let (oracle, full_comparisons) = counted(|| full_sort_oracle(&model, &request));
        let (view, page_comparisons) = counted(|| build_view(&model, &request, 1));
        assert_eq!(
            view.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            oracle
                .into_iter()
                .skip(offset)
                .take(128)
                .collect::<Vec<_>>()
        );
        assert!(
            page_comparisons * 2 < full_comparisons,
            "offset={offset}: page ranking used {page_comparisons} comparisons versus full sort {full_comparisons}"
        );
    }
}

#[test]
fn empty_and_out_of_range_pages_do_no_ranking_comparisons() {
    let model = shuffled_model(64);
    for (offset, limit) in [(0, 0), (64, 1), (65, usize::MAX), (usize::MAX, usize::MAX)] {
        let request = ViewRequest {
            mode: ViewMode::Largest,
            offset,
            limit,
            ..Default::default()
        };
        let (view, comparisons) = counted(|| build_view(&model, &request, 1));
        assert!(view.rows.is_empty());
        assert_eq!(view.total_rows, 64);
        assert_eq!(comparisons, 0, "offset={offset}, limit={limit}");
    }
}

#[test]
fn excessive_requested_page_size_still_obeys_output_cap() {
    let model = shuffled_model(MAX_PAGE_ROWS + 100);
    let request = ViewRequest {
        mode: ViewMode::Largest,
        offset: 10,
        limit: usize::MAX,
        ..Default::default()
    };
    assert_exact_page(&model, &request);
    assert_eq!(build_view(&model, &request, 1).rows.len(), MAX_PAGE_ROWS);
}

#[test]
#[ignore = "explicit release page-ranking microbenchmark; construction and cloning excluded"]
fn page_ranking_release_microbenchmark() {
    use std::{
        hint::black_box,
        time::{Duration, Instant},
    };
    eprintln!(
        "page-ranking microbenchmark optimized={}; use --release for performance evidence",
        !cfg!(debug_assertions)
    );

    fn measure(
        model: &Model,
        request: &ViewRequest,
        values: &mut [NodeId],
        partial: bool,
    ) -> (Duration, Vec<NodeId>) {
        let started = Instant::now();
        if partial {
            let page = rank_page_by(values, request.offset, request.limit, |a, b| {
                compare(model, request, *a, *b)
            });
            let elapsed = started.elapsed();
            (elapsed, black_box(page).to_vec())
        } else {
            values.sort_unstable_by(|a, b| compare(model, request, *a, *b));
            let elapsed = started.elapsed();
            (
                elapsed,
                black_box(values)
                    .iter()
                    .copied()
                    .skip(request.offset)
                    .take(request.limit)
                    .collect(),
            )
        }
    }

    const COUNT: usize = 250_000;
    const RUNS: usize = 5;
    for equal_sizes in [false, true] {
        let mut model = shuffled_model(COUNT);
        if equal_sizes {
            for node in &mut model.nodes[1..] {
                node.logical = 1;
            }
            model.nodes[0].logical = COUNT as u64;
        }
        let distribution = if equal_sizes {
            "equal_sizes_name_ties"
        } else {
            "varied_sizes"
        };
        let candidates: Vec<_> = (1..=COUNT as NodeId).collect();
        for offset in [0, COUNT / 2] {
            let request = ViewRequest {
                mode: ViewMode::Largest,
                offset,
                limit: 100,
                ..Default::default()
            };
            let mut full_times = Vec::with_capacity(RUNS);
            let mut page_times = Vec::with_capacity(RUNS);
            for run in 0..RUNS {
                // Prepare identical inputs before either timer; alternate execution order.
                let mut full = candidates.clone();
                let mut page = candidates.clone();
                let (full_result, page_result) = if run % 2 == 0 {
                    let full_result = measure(&model, &request, &mut full, false);
                    let page_result = measure(&model, &request, &mut page, true);
                    (full_result, page_result)
                } else {
                    let page_result = measure(&model, &request, &mut page, true);
                    let full_result = measure(&model, &request, &mut full, false);
                    (full_result, page_result)
                };
                assert_eq!(full_result.1, page_result.1);
                let checksum = page_result.1.iter().fold(0u64, |sum, id| {
                    sum.wrapping_mul(31).wrapping_add(u64::from(*id))
                });
                black_box(checksum);
                full_times.push(full_result.0);
                page_times.push(page_result.0);
                eprintln!(
                    "page_rank distribution={distribution} nodes={COUNT} offset={offset} limit=100 run={run} full_sort_ms={:.3} page_rank_ms={:.3} checksum={checksum}",
                    full_result.0.as_secs_f64() * 1000.0,
                    page_result.0.as_secs_f64() * 1000.0
                );
            }
            full_times.sort_unstable();
            page_times.sort_unstable();
            let expected = full_sort_oracle(&model, &request);
            let started = Instant::now();
            let view = black_box(build_view(&model, &request, 1));
            let query_elapsed = started.elapsed();
            assert_eq!(
                view.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
                expected
                    .into_iter()
                    .skip(offset)
                    .take(100)
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "page_rank_summary distribution={distribution} nodes={COUNT} offset={offset} full_sort_median_ms={:.3} page_rank_median_ms={:.3} full_query_ms={:.3} rows={} tiles={} (background query; excludes model construction/cloning, rendering and I/O)",
                full_times[RUNS / 2].as_secs_f64() * 1000.0,
                page_times[RUNS / 2].as_secs_f64() * 1000.0,
                query_elapsed.as_secs_f64() * 1000.0,
                view.rows.len(),
                view.map.len()
            );
        }
    }
}
