//! Headless Controller probe.
//!
//! Argument parsing, the scan wait loop (including its 5 ms polling and timeout
//! handling) and the exit-code mapping are the shipped CLI's own, from
//! `diskscope::cli`, so what this measures is what users run. Only the
//! profiling extras live here: selectable query views, process diagnostics and
//! a post-timing coverage query.
use diskscope::{
    cli::{self, OptionSet, ScanOptions, ScanRun},
    diagnostics,
    engine::Controller,
    model::*,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    time::{Duration, Instant},
};

const MEASUREMENT_NOTES: &str = "\
    Scan elapsed freezes before completion sweep and terminal query. Reported \
    wall and diagnostics freeze at terminal/timeout observation, before \
    coverage collection and report serialization/write. Finished reports \
    perform an additional diagnostic-only root Explorer query bounded by three \
    seconds; external process wall/CPU include that query and are not pure \
    shipped-CLI comparisons. Coverage names/paths are emitted only to an \
    explicit local JSON file, never stdout. Worker wall overlaps, nested spans \
    overlap, timeout does not await cancellation and does not request \
    coverage. Overview uses GUI mode/limit but preserves CLI \
    polling/history/aspect; this isolates query-mode differences and is not a \
    full native GUI measurement.";

fn run(options: ScanOptions) -> Result<i32, Box<dyn std::error::Error>> {
    diagnostics::reset();
    let mut controller = Controller::new();
    let scan = cli::execute_scan(&mut controller, &options);
    // Freeze the timed workload above: diagnostics are read before the coverage
    // query below, which must not enter them or the reported wall time.
    let measurements = serde_json::to_value(diagnostics::snapshot())?;
    // A timeout is a partial scan and does not wait for cancellation, so it
    // never requests coverage.
    let coverage_scopes = if scan.timed_out {
        Value::Null
    } else {
        collect_coverage_scopes(&controller, scan.generation, options.json.is_some())
    };
    let report = profile_report(&options, &scan, measurements, coverage_scopes);
    cli::write_report(&report, options.json.as_deref())?;
    Ok(scan.exit_code())
}

/// The shipped CLI's scan summary plus the profiling-only fields.
fn profile_report(
    options: &ScanOptions,
    scan: &ScanRun,
    diagnostics: Value,
    coverage_scopes: Value,
) -> Value {
    let request = options.view_request();
    let include_paths = options.json.is_some();
    let mut report = cli::summary_fields(scan);
    report.insert("application".into(), json!("DiskScope profile_scan"));
    report.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    report.insert("view".into(), json!(format!("{:?}", options.view)));
    report.insert("limit".into(), json!(request.limit));
    report.insert("history_enabled".into(), json!(false));
    report.insert("poll_interval_ms".into(), json!(cli::POLL_INTERVAL_MS));
    report.insert("map_aspect".into(), json!(request.map_aspect));
    let errors = error_report(&scan.snapshot.progress.errors, include_paths);
    report.insert("errors".into(), Value::Array(errors));
    report.insert("diagnostics".into(), diagnostics);
    report.insert("coverage_scopes".into(), coverage_scopes);
    report.insert("measurement_notes".into(), json!(MEASUREMENT_NOTES));
    Value::Object(report)
}

fn collect_coverage_scopes(
    controller: &Controller,
    generation: u64,
    include_paths: bool,
) -> serde_json::Value {
    let started = Instant::now();
    controller.request_view(ViewRequest {
        revision: 1,
        root: 0,
        mode: ViewMode::Explorer,
        sort: SortKey::Name,
        descending: false,
        limit: diskscope::view::MAX_PAGE_ROWS,
        ..Default::default()
    });
    while started.elapsed() < Duration::from_secs(3) {
        if let Some(snapshot) = controller.poll()
            && snapshot.progress.generation == generation
            && snapshot.view.generation == generation
            && snapshot.view.revision == 1
        {
            return coverage_report(&snapshot.view, include_paths, started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(cli::POLL_INTERVAL_MS));
    }
    serde_json::json!({
        "timed_out": true, "truncated": null, "total_rows": null,
        "paths_included": include_paths, "rows": [],
        "query_elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
    })
}

fn error_report(errors: &[ScanError], include_paths: bool) -> Vec<serde_json::Value> {
    errors
        .iter()
        .map(|error| {
            if include_paths {
                serde_json::json!({
                    "message": error.message,
                    "path_display": error.path.to_string_lossy(),
                })
            } else {
                serde_json::json!(error.message)
            }
        })
        .collect()
}

fn coverage_report(
    view: &ViewSnapshot,
    include_paths: bool,
    elapsed: Duration,
) -> serde_json::Value {
    let mut files = 0u64;
    let mut folders = 0u64;
    let mut logical = 0u64;
    let rows: Vec<_> = view
        .rows
        .iter()
        .map(|row| {
            let scope_files = if row.is_dir { row.files } else { 1 };
            files = files.saturating_add(scope_files);
            // Directory row totals exclude the directory itself. Include it only
            // in the aggregate so root-level scopes reconcile to whole-scan folders.
            folders = folders
                .saturating_add(row.folders)
                .saturating_add(u64::from(row.is_dir));
            logical = logical.saturating_add(row.logical);
            let mut scope = serde_json::json!({
                "is_dir": row.is_dir, "reparse": row.reparse,
                "files": scope_files, "folders": row.folders,
                "logical_bytes": row.logical,
            });
            if include_paths {
                scope["name"] = serde_json::json!(row.name);
                scope["path_display"] = serde_json::json!(row.path.to_string_lossy());
            }
            scope
        })
        .collect();
    serde_json::json!({
        "timed_out": false, "truncated": view.total_rows > view.rows.len(),
        "total_rows": view.total_rows, "paths_included": include_paths,
        "query_elapsed_ms": elapsed.as_secs_f64() * 1000.0,
        "aggregate_returned_scopes": {
            "files": files,
            "folders": folders,
            "logical_bytes": logical,
        },
        "rows": rows,
    })
}

fn main() {
    if !cfg!(feature = "diagnostics") {
        eprintln!("Build with --no-default-features --features diagnostics --example profile_scan");
        std::process::exit(cli::EXIT_ARGUMENT_ERROR);
    }
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let options = match cli::parse_scan_args(&arguments, OptionSet::Profiling) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(cli::EXIT_ARGUMENT_ERROR);
        }
    };
    match run(options) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(cli::EXIT_FAILURE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::Arc};

    #[test]
    fn error_paths_are_available_only_in_explicit_local_json_reports() {
        let errors = vec![ScanError {
            path: "private/error-path".into(),
            message: "Access denied".into(),
        }];
        let local = error_report(&errors, true);
        assert_eq!(local[0]["message"], "Access denied");
        assert_eq!(local[0]["path_display"], "private/error-path");
        let stdout = error_report(&errors, false);
        assert_eq!(stdout, vec![serde_json::json!("Access denied")]);
    }
    #[test]
    fn coverage_reconciles_root_files_and_directory_descendants_and_redacts_stdout() {
        let mut model = Model::new("coverage-fixture".into());
        let entry = |name: &str, is_dir, logical| Entry {
            name: name.into(),
            is_dir,
            logical,
            allocated: Some(0),
            reparse: false,
            modified: None,
            identity: None,
        };
        model.insert_batch(
            0,
            vec![entry("folder", true, 0), entry("root-file", false, 5)],
        );
        model.insert_batch(
            1,
            vec![entry("nested", true, 0), entry("child-file", false, 7)],
        );
        let view = diskscope::view::build_view(
            &model,
            &ViewRequest {
                sort: SortKey::Name,
                ..Default::default()
            },
            1,
        );
        let local = coverage_report(&view, true, Duration::from_millis(2));
        assert_eq!(local["aggregate_returned_scopes"]["files"], 2);
        assert_eq!(local["aggregate_returned_scopes"]["folders"], 2);
        assert_eq!(local["aggregate_returned_scopes"]["logical_bytes"], 12);
        assert_eq!(local["truncated"], false);
        assert!(
            local["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["name"].is_string() && row["path_display"].is_string())
        );
        let stdout = coverage_report(&view, false, Duration::ZERO);
        assert!(
            stdout["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row.get("name").is_none() && row.get("path_display").is_none())
        );
        assert_eq!(
            stdout["aggregate_returned_scopes"],
            local["aggregate_returned_scopes"]
        );
    }
    #[test]
    fn coverage_marks_capped_root_scopes_as_truncated() {
        let view = ViewSnapshot {
            total_rows: 10,
            ..Default::default()
        };
        let report = coverage_report(&view, false, Duration::ZERO);
        assert_eq!(report["truncated"], true);
        assert_eq!(report["total_rows"], 10);
        assert_eq!(report["rows"].as_array().unwrap().len(), 0);
    }
    #[test]
    fn report_keeps_the_profiling_schema_and_the_shared_summary() {
        let options = ScanOptions {
            root: PathBuf::from("fixture"),
            json: None,
            backend: BackendChoice::Normal,
            view: ViewMode::Overview,
            timeout: cli::DEFAULT_TIMEOUT,
        };
        let mut snapshot = UiSnapshot::default();
        snapshot.progress.state = ScanState::Cancelled;
        snapshot.progress.files = 3;
        let scan = ScanRun {
            generation: 1,
            started_utc_epoch_ms: None,
            finished_utc_epoch_ms: None,
            snapshot: Arc::new(snapshot),
            timed_out: false,
            first_result_ms: None,
            wall_elapsed: Duration::from_millis(5),
        };
        let report = profile_report(&options, &scan, json!({}), Value::Null);
        let mut keys: Vec<_> = report.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "application",
                "backend",
                "complete",
                "coverage_scopes",
                "diagnostics",
                "errors",
                "errors_total",
                "files",
                "first_result_ms",
                "folders",
                "hardlink_aliases",
                "history_enabled",
                "limit",
                "logical_bytes",
                "map_aspect",
                "measurement_notes",
                "message",
                "poll_interval_ms",
                "scan_elapsed_ms",
                "skipped_reparse_entries",
                "state",
                "unique_allocated_bytes",
                "version",
                "view",
                "wall_elapsed_ms",
            ]
        );
        assert_eq!(report["application"], "DiskScope profile_scan");
        assert_eq!(report["view"], "Overview");
        assert_eq!(report["limit"], cli::OTHER_VIEW_LIMIT);
        assert_eq!(report["poll_interval_ms"], 5);
        assert_eq!(report["history_enabled"], false);
        // The summary comes from the shipped CLI, so a cancelled scan is
        // reported as the incomplete scan it is.
        assert_eq!(report["state"], "Scan cancelled");
        assert_eq!(report["complete"], false);
        assert_eq!(report["files"], 3);
        assert_eq!(scan.exit_code(), cli::EXIT_PARTIAL);
    }
    #[test]
    fn the_profiling_front_end_accepts_view_but_stays_a_strict_scan_parser() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let options = cli::parse_scan_args(
            &args(&["--scan", "a folder", "--view", "explorer"]),
            OptionSet::Profiling,
        )
        .unwrap();
        assert_eq!(options.view, ViewMode::Explorer);
        assert_eq!(options.view_request().limit, cli::OTHER_VIEW_LIMIT);
        assert!(cli::parse_scan_args(&args(&["fixture"]), OptionSet::Profiling).is_err());
    }
}
