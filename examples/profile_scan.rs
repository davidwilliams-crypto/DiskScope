//! Headless Controller probe. It deliberately preserves shipped CLI 5 ms polling.
use diskscope::{diagnostics, engine::Controller, model::*};
use std::{
    ffi::OsString,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct Options {
    root: PathBuf,
    json: Option<PathBuf>,
    backend: BackendChoice,
    view: ViewMode,
    timeout: u64,
}
impl Options {
    fn request(&self) -> ViewRequest {
        ViewRequest {
            mode: self.view,
            limit: if self.view == ViewMode::Largest {
                100
            } else {
                256
            },
            ..Default::default()
        }
    }
}

fn parse_args(args: Vec<OsString>) -> Result<Options, String> {
    let (mut root, mut json, mut backend, mut view, mut timeout) =
        (None, None, BackendChoice::Normal, ViewMode::Largest, 600);
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        let next = args
            .get(i + 1)
            .ok_or_else(|| format!("Missing value for {}", flag.to_string_lossy()))?;
        match flag.to_str() {
            Some("--scan") => {
                if root.is_some() {
                    return Err("Only one scan root is supported".into());
                }
                if next.is_empty() {
                    return Err("Scan root must not be empty".into());
                }
                root = Some(PathBuf::from(next));
            }
            Some("--json") => json = Some(PathBuf::from(next)),
            Some("--backend") => {
                backend = match next.to_str() {
                    Some("normal") => BackendChoice::Normal,
                    Some("ntfs") => BackendChoice::Ntfs,
                    _ => return Err("Backend must be normal or ntfs".into()),
                }
            }
            Some("--view") => {
                view = match next.to_str() {
                    Some("largest") => ViewMode::Largest,
                    Some("overview") => ViewMode::Overview,
                    Some("explorer") => ViewMode::Explorer,
                    Some("types") => ViewMode::Types,
                    _ => return Err("View must be largest, overview, explorer or types".into()),
                }
            }
            Some("--timeout") => {
                timeout = next
                    .to_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .filter(|n| *n > 0)
                    .ok_or("Timeout must be a positive number of seconds")?
            }
            _ => return Err(format!("Unknown option: {}", flag.to_string_lossy())),
        }
        i += 2;
    }
    Ok(Options {
        root: root.ok_or("Use --scan PATH; no implicit scan is permitted")?,
        json,
        backend,
        view,
        timeout,
    })
}

fn run(options: Options) -> Result<i32, Box<dyn std::error::Error>> {
    diagnostics::reset();
    let mut controller = Controller::new();
    let generation = controller.start(options.root.clone(), options.backend);
    controller.request_view(options.request());
    // Match shipped CLI: clock begins after controller creation/start/request.
    let started = Instant::now();
    let mut latest = None;
    let mut timed_out = false;
    let mut first_result_ms = None;
    loop {
        if let Some(snapshot) = controller.poll()
            && snapshot.progress.generation == generation
        {
            if first_result_ms.is_none() && snapshot.progress.files + snapshot.progress.folders > 0
            {
                first_result_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
            }
            latest = Some(snapshot);
        }
        if latest
            .as_ref()
            .is_some_and(|s| !s.progress.state.is_active())
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(options.timeout) {
            controller.cancel();
            timed_out = true;
            // Like shipped CLI, do not wait for blocked OS I/O or cancel acknowledgement.
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let observed_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    let measurements = diagnostics::snapshot();
    let snapshot = latest.unwrap_or_else(|| std::sync::Arc::new(UiSnapshot::default()));
    let p = &snapshot.progress;
    let complete = !timed_out && p.state == ScanState::Complete && p.errors_total == 0;
    // Freeze the timed workload above. This terminal-only diagnostic query must
    // not enter its reported wall time or process-aggregate diagnostics.
    let coverage_scopes = if timed_out {
        serde_json::Value::Null
    } else {
        collect_coverage_scopes(&controller, generation, options.json.is_some())
    };
    let report = serde_json::json!({
        "application": "DiskScope profile_scan", "version": env!("CARGO_PKG_VERSION"),
        "view": format!("{:?}", options.view), "limit": options.request().limit,
        "history_enabled": false, "poll_interval_ms": 5, "map_aspect": options.request().map_aspect,
        "state": if timed_out { "Timed out" } else { p.state.label() }, "complete": complete,
        "backend": p.backend, "files": p.files, "folders": p.folders, "logical_bytes": p.logical,
        "unique_allocated_bytes": p.unique_allocated, "hardlink_aliases": p.hardlinks,
        "skipped_reparse_entries": p.skipped, "errors_total": p.errors_total,
        "errors": error_report(&p.errors, options.json.is_some()),
        "first_result_ms": first_result_ms, "scan_elapsed_ms": p.elapsed.as_secs_f64() * 1000.0,
        "wall_elapsed_ms": observed_wall_ms, "message": p.message,
        "diagnostics": measurements,
        "coverage_scopes": coverage_scopes,
        "measurement_notes": "Scan elapsed freezes before completion sweep and terminal query. Reported wall and diagnostics freeze at terminal/timeout observation, before coverage collection and report serialization/write. Finished reports perform an additional diagnostic-only root Explorer query bounded by three seconds; external process wall/CPU include that query and are not pure shipped-CLI comparisons. Coverage names/paths are emitted only to an explicit local JSON file, never stdout. Worker wall overlaps, nested spans overlap, timeout does not await cancellation and does not request coverage. Overview uses GUI mode/limit but preserves CLI polling/history/aspect; this isolates query-mode differences and is not a full native GUI measurement."
    });
    let encoded = serde_json::to_vec_pretty(&report)?;
    if let Some(path) = options.json {
        std::fs::write(path, encoded)?;
    } else {
        use std::io::Write;
        std::io::stdout().lock().write_all(&encoded)?;
    }
    Ok(exit_code(timed_out, p.state))
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
        std::thread::sleep(Duration::from_millis(5));
    }
    serde_json::json!({
        "timed_out": true, "truncated": null, "total_rows": null,
        "paths_included": include_paths, "rows": [],
        "query_elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
    })
}

fn error_report(errors: &[ScanError], include_paths: bool) -> Vec<serde_json::Value> {
    errors.iter().map(|error| {
        if include_paths {
            serde_json::json!({ "message": error.message, "path_display": error.path.to_string_lossy() })
        } else {
            serde_json::json!(error.message)
        }
    }).collect()
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
        "aggregate_returned_scopes": { "files": files, "folders": folders, "logical_bytes": logical },
        "rows": rows,
    })
}

fn exit_code(timed_out: bool, state: ScanState) -> i32 {
    if timed_out {
        3
    } else if state == ScanState::Failed {
        1
    } else if state == ScanState::Partial {
        4
    } else {
        0
    }
}

fn main() {
    if !cfg!(feature = "diagnostics") {
        eprintln!("Build with --no-default-features --features diagnostics --example profile_scan");
        std::process::exit(2);
    }
    let options = match parse_args(std::env::args_os().skip(1).collect()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    match run(options) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
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
    fn exit_codes_match_shipped_cli_including_partial_and_cancelled() {
        assert_eq!(exit_code(false, ScanState::Complete), 0);
        assert_eq!(exit_code(false, ScanState::Cancelled), 0);
        assert_eq!(exit_code(false, ScanState::Failed), 1);
        assert_eq!(exit_code(false, ScanState::Partial), 4);
        for state in [
            ScanState::Complete,
            ScanState::Cancelled,
            ScanState::Failed,
            ScanState::Partial,
        ] {
            assert_eq!(exit_code(true, state), 3);
        }
    }
    #[test]
    fn explicit_root_and_defaults_match_shipped_cli_workload() {
        let options = parse_args(args(&["--scan", "a folder"])).unwrap();
        assert_eq!(options.root, PathBuf::from("a folder"));
        assert_eq!(options.backend, BackendChoice::Normal);
        assert_eq!(options.view, ViewMode::Largest);
        assert_eq!(options.timeout, 600);
        assert_eq!(options.request().limit, 100);
    }
    #[test]
    fn all_modes_and_backend_timeout_are_explicit() {
        for (name, mode) in [
            ("largest", ViewMode::Largest),
            ("overview", ViewMode::Overview),
            ("explorer", ViewMode::Explorer),
            ("types", ViewMode::Types),
        ] {
            let options = parse_args(args(&[
                "--scan",
                "fixture",
                "--view",
                name,
                "--backend",
                "ntfs",
                "--timeout",
                "42",
                "--json",
                "report.json",
            ]))
            .unwrap();
            assert_eq!(options.view, mode);
            assert_eq!(options.backend, BackendChoice::Ntfs);
            assert_eq!(options.timeout, 42);
            assert_eq!(options.json, Some(PathBuf::from("report.json")));
        }
    }
    #[test]
    fn malformed_arguments_never_turn_into_implicit_scans() {
        for values in [
            vec![],
            vec!["fixture"],
            vec!["--scan"],
            vec!["--view", "largest"],
            vec!["--scan", "x", "--scan", "y"],
            vec!["--scan", "x", "--backend", "auto"],
            vec!["--scan", "x", "--view", "bad"],
            vec!["--scan", "x", "--timeout", "0"],
            vec!["--scan", "x", "--timeout", "-1"],
            vec!["--scan", "x", "--unknown", "x"],
        ] {
            assert!(parse_args(args(&values)).is_err(), "{values:?}");
        }
    }
    #[cfg(windows)]
    #[test]
    fn paths_retain_unpaired_windows_units() {
        use std::os::windows::ffi::OsStringExt;
        let path = OsString::from_wide(&[0x44, 0x3a, 0x5c, 0xd800]);
        let options = parse_args(vec![
            "--scan".into(),
            path.clone(),
            "--json".into(),
            path.clone(),
        ])
        .unwrap();
        assert_eq!(options.root.as_os_str(), path);
        assert_eq!(options.json.unwrap().as_os_str(), path);
    }
}
