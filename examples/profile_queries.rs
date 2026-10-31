//! Repeatable metadata-only query benchmark; model construction is not timed.
use diskscope::{model::*, view::build_view};
use std::{hint::black_box, time::Instant};

fn main() {
    let count = std::env::args()
        .nth(1)
        .map(|n| n.parse().unwrap())
        .unwrap_or(1_000_000usize);
    let mut model = Model::new("query-benchmark".into());
    model.insert_batch(
        0,
        (0..count)
            .map(|i| Entry {
                name: format!(
                    "file-{:07}.{}",
                    (i * 7919) % count,
                    if i % 4 == 0 { "zip" } else { "dat" }
                )
                .into(),
                logical: ((i * 104729) % count) as u64,
                allocated: None,
                modified: Some(i as u64),
                identity: None,
                is_dir: false,
                reparse: false,
            })
            .collect(),
    );
    for (label, request) in [
        (
            "largest",
            ViewRequest {
                mode: ViewMode::Largest,
                limit: 100,
                ..Default::default()
            },
        ),
        (
            "broad-search",
            ViewRequest {
                mode: ViewMode::Largest,
                search: "file".into(),
                limit: 100,
                ..Default::default()
            },
        ),
        (
            "selective-search",
            ViewRequest {
                mode: ViewMode::Largest,
                search: "0000".into(),
                limit: 100,
                ..Default::default()
            },
        ),
        (
            "overview",
            ViewRequest {
                mode: ViewMode::Overview,
                ..Default::default()
            },
        ),
    ] {
        // Independent full-sort reference outside the measured region. These
        // fixtures have lowercase names and unique sizes, so native-name ties
        // are straightforward; the unit suite covers every sort and tie rule.
        let mut expected: Vec<_> = model
            .nodes
            .iter()
            .skip(1)
            .filter(|n| n.name.to_string_lossy().contains(&request.search))
            .map(|n| n.id)
            .collect();
        expected.sort_unstable_by(|a, b| {
            model.nodes[*b as usize]
                .logical
                .cmp(&model.nodes[*a as usize].logical)
                .then_with(|| {
                    model.nodes[*a as usize]
                        .name
                        .cmp(&model.nodes[*b as usize].name)
                })
                .then_with(|| a.cmp(b))
        });
        let matches = expected.len();
        let logical: u64 = expected
            .iter()
            .map(|id| model.nodes[*id as usize].logical)
            .sum();
        expected.truncate(if request.mode == ViewMode::Overview {
            6
        } else {
            100
        });
        let _ = black_box(build_view(&model, &request, 1));
        let mut times = Vec::new();
        for _ in 0..7 {
            let started = Instant::now();
            let view = black_box(build_view(black_box(&model), black_box(&request), 1));
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(
                view.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
                expected
            );
            assert_eq!(view.matching_files, matches as u64);
            assert_eq!(view.matching_logical, logical);
            black_box(view.matching_logical);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "case={label} nodes={count} matches={matches} median_ms={:.3} min_ms={:.3} max_ms={:.3}",
            times[3], times[0], times[6]
        );
    }
}
