//! Command-line front end shared by the `diskscope` executable and the
//! `profile_scan` example.
//!
//! Argument parsing, the scan wait loop, the report summary and the exit-code
//! mapping live here so the profiling example measures the code users actually
//! run instead of a copy that can drift. (It had drifted: the copy mapped a
//! cancelled scan to the "complete" exit code.)
//!
//! Exit codes, as documented in the README: 0 complete, 1 failure, 2 argument or
//! build-mode error, 3 timeout, 4 partial results.

use crate::{
    engine::Controller,
    evidence::{EvidenceContext, Redaction},
    model::{BackendChoice, ScanState, UiSnapshot, ViewMode, ViewRequest},
};
use serde_json::{Map, Value, json};
use std::{
    ffi::{OsStr, OsString},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// The scan finished and every entry was read.
pub const EXIT_COMPLETE: i32 = 0;
/// The scan failed outright, or the report could not be produced.
pub const EXIT_FAILURE: i32 = 1;
/// Bad command line, or the executable was built without the needed feature.
pub const EXIT_ARGUMENT_ERROR: i32 = 2;
/// The scan was abandoned after `--timeout` seconds.
pub const EXIT_TIMEOUT: i32 = 3;
/// The scan produced results, but not a full inventory.
pub const EXIT_PARTIAL: i32 = 4;

/// Used when `--timeout` is not given.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
/// How often the wait loop asks the controller for a new snapshot.
pub const POLL_INTERVAL_MS: u64 = 5;
const POLL_INTERVAL: Duration = Duration::from_millis(POLL_INTERVAL_MS);
/// Rows requested for the "largest files" view the report lists.
pub const LARGEST_VIEW_LIMIT: usize = 100;
/// Rows requested for every other view the profiling example can select.
pub const OTHER_VIEW_LIMIT: usize = 256;

const MILLIS_PER_SECOND: f64 = 1000.0;
const TIMED_OUT_LABEL: &str = "Timed out";
const ACCOUNTING_NOTE: &str = "Logical bytes count file entries. Unique allocation deduplicates \
                               file IDs. ADS and filesystem metadata excluded.";

/// What the command line asked the executable to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the window, optionally on a folder.
    Gui(Option<PathBuf>),
    /// Restricted GUI session; restrictions also merge with the companion policy.
    GuiSession {
        root: Option<PathBuf>,
        policy: crate::session::SessionPolicy,
    },
    /// Scan without a window and print a JSON report.
    Scan(ScanOptions),
    Help,
    Version,
}

/// Which optional flags a front end accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionSet {
    /// The shipped executable. Its report is always the largest-files view, so
    /// it has no `--view`.
    Shipped,
    /// The profiling example, which adds `--view` to compare query modes.
    Profiling,
}

/// A fully parsed `--scan` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    pub root: PathBuf,
    /// Write the report here instead of to stdout.
    pub json: Option<PathBuf>,
    pub backend: BackendChoice,
    pub view: ViewMode,
    /// Wall-clock budget after which the scan is cancelled and reported as timed out.
    pub timeout: Duration,
}

impl ScanOptions {
    /// The query the scan keeps asking for while it runs.
    pub fn view_request(&self) -> ViewRequest {
        let limit = if self.view == ViewMode::Largest {
            LARGEST_VIEW_LIMIT
        } else {
            OTHER_VIEW_LIMIT
        };
        ViewRequest {
            mode: self.view,
            limit,
            ..Default::default()
        }
    }
}

/// Interprets the executable's arguments (without the program name).
pub fn parse_args(args: &[OsString]) -> Result<Command, String> {
    if args
        .iter()
        .any(|arg| arg == "--helpdesk" || arg == "--no-history")
    {
        let mut policy = crate::session::SessionPolicy::default();
        let mut root = None;
        for arg in args {
            if arg == "--helpdesk" {
                policy.helpdesk = true;
            } else if arg == "--no-history" {
                policy.no_history = true;
            } else if arg.to_string_lossy().starts_with('-') || root.is_some() {
                return Err("Use --helpdesk [FOLDER] or --no-history [FOLDER] for a GUI session; --scan is already diagnostic and does not save GUI history".into());
            } else if arg.is_empty() {
                return Err("Folder must not be empty".into());
            } else {
                root = Some(PathBuf::from(arg));
            }
        }
        return Ok(Command::GuiSession { root, policy });
    }
    match args {
        [] => return Ok(Command::Gui(None)),
        [only] if only == "--help" => return Ok(Command::Help),
        [only] if only == "--version" => return Ok(Command::Version),
        // A lone argument that is not an option is a folder to open in the GUI.
        [only] if !only.to_string_lossy().starts_with('-') => {
            return Ok(Command::Gui(Some(only.into())));
        }
        _ => {}
    }
    parse_scan_args(args, OptionSet::Shipped).map(Command::Scan)
}

/// Parses `--scan PATH` plus the optional flags of `flags`.
///
/// Every option takes exactly one value, and there is deliberately no implicit
/// scan root: a scan only starts when `--scan` names one.
pub fn parse_scan_args(args: &[OsString], flags: OptionSet) -> Result<ScanOptions, String> {
    let mut root = None;
    let mut json = None;
    let mut backend = BackendChoice::Normal;
    let mut view = ViewMode::Largest;
    let mut timeout = DEFAULT_TIMEOUT;
    let mut pending = args.iter();
    while let Some(flag) = pending.next() {
        let value = pending
            .next()
            .ok_or_else(|| format!("Missing value for {}", flag.to_string_lossy()))?;
        match flag.to_str() {
            Some("--scan") => {
                if root.is_some() {
                    return Err("Only one scan root is supported".into());
                }
                if value.is_empty() {
                    return Err("Scan root must not be empty".into());
                }
                root = Some(PathBuf::from(value));
            }
            Some("--json") => json = Some(PathBuf::from(value)),
            Some("--backend") => backend = parse_backend(value)?,
            Some("--view") if flags == OptionSet::Profiling => view = parse_view(value)?,
            Some("--timeout") => timeout = parse_timeout(value)?,
            _ => return Err(format!("Unknown option: {}", flag.to_string_lossy())),
        }
    }
    Ok(ScanOptions {
        root: root.ok_or("Use --scan PATH for command-line analysis")?,
        json,
        backend,
        view,
        timeout,
    })
}

fn parse_backend(value: &OsStr) -> Result<BackendChoice, String> {
    match value.to_str() {
        Some("normal") => Ok(BackendChoice::Normal),
        Some("ntfs") => Ok(BackendChoice::Ntfs),
        _ => Err("Backend must be normal or ntfs".into()),
    }
}

fn parse_view(value: &OsStr) -> Result<ViewMode, String> {
    match value.to_str() {
        Some("largest") => Ok(ViewMode::Largest),
        Some("overview") => Ok(ViewMode::Overview),
        Some("explorer") => Ok(ViewMode::Explorer),
        Some("types") => Ok(ViewMode::Types),
        _ => Err("View must be largest, overview, explorer or types".into()),
    }
}

fn parse_timeout(value: &OsStr) -> Result<Duration, String> {
    value
        .to_str()
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .ok_or_else(|| "Timeout must be a positive number of seconds".into())
}

/// Text printed by `--help`.
pub fn help_text() -> String {
    format!(
        "DiskScope {}\n\n\
         DiskScope.exe [FOLDER]\n\
         DiskScope.exe --helpdesk [FOLDER]\n\
         DiskScope.exe --no-history [FOLDER]\n\
         DiskScope.exe --scan PATH [--json OUTPUT.json] [--backend normal|ntfs] [--timeout SECONDS]\n\
         DiskScope.exe --help | --version\n\n\
         Helpdesk mode disables file actions, NTFS preview and profile persistence. \
         No-history mode disables history, preferences and window-state persistence. \
         An adjacent diskscope-policy.json can enforce these restrictions. \
         Explicit report exports still write their chosen destination. \
         Scans are read-only. Directory junctions and links are listed but not followed. \
         NTFS mode requests no elevation and falls back when unavailable. \
         Exit codes: {EXIT_COMPLETE} complete, {EXIT_FAILURE} failure, \
         {EXIT_ARGUMENT_ERROR} argument error, {EXIT_TIMEOUT} timeout, \
         {EXIT_PARTIAL} partial results.",
        env!("CARGO_PKG_VERSION")
    )
}

/// Maps the observed outcome of a scan to the process exit code.
///
/// A scan that ended in `Cancelled` keeps the entries found before it stopped
/// (the GUI labels it "Stopped - partial results" and the engine's message says
/// the totals are incomplete), which is what `EXIT_PARTIAL` means. Reporting it
/// as `EXIT_COMPLETE` would let a script treat an incomplete inventory as a
/// whole one. The CLI itself only cancels on timeout and reports that as
/// `EXIT_TIMEOUT` without waiting, so this arm is defensive.
///
/// The match has no wildcard so a new `ScanState` forces a decision here.
pub fn exit_code(timed_out: bool, state: ScanState) -> i32 {
    if timed_out {
        return EXIT_TIMEOUT;
    }
    match state {
        ScanState::Complete => EXIT_COMPLETE,
        ScanState::Failed => EXIT_FAILURE,
        ScanState::Partial | ScanState::Cancelled => EXIT_PARTIAL,
        // The wait loop returns one of these only on a timeout (handled above)
        // or, for `Idle`, when the controller published nothing at all. Either
        // way the scan did not finish, and "success" would be the wrong thing to
        // tell a script.
        ScanState::Idle
        | ScanState::Preparing
        | ScanState::Scanning
        | ScanState::Finalizing
        | ScanState::Cancelling => EXIT_FAILURE,
    }
}

/// What the wait loop observed.
#[derive(Debug)]
pub struct ScanRun {
    pub generation: u64,
    pub started_utc_epoch_ms: Option<u64>,
    pub finished_utc_epoch_ms: Option<u64>,
    /// The newest snapshot of this scan, or an empty one when the timeout fired
    /// before the first publication.
    pub snapshot: Arc<UiSnapshot>,
    pub timed_out: bool,
    /// Milliseconds until a snapshot first showed any file or folder.
    pub first_result_ms: Option<f64>,
    /// Wall time until the terminal state (or the timeout) was observed.
    pub wall_elapsed: Duration,
}

impl ScanRun {
    pub fn state(&self) -> ScanState {
        self.snapshot.progress.state
    }

    /// True only for a scan that finished, in time, without a single error.
    pub fn is_complete(&self) -> bool {
        !self.timed_out
            && self.state() == ScanState::Complete
            && self.snapshot.progress.errors_total == 0
    }

    pub fn exit_code(&self) -> i32 {
        exit_code(self.timed_out, self.state())
    }
}

/// The part of the controller the wait loop depends on, so the loop can be
/// tested with scripted snapshots and no filesystem.
trait ScanSource {
    fn poll(&self) -> Option<Arc<UiSnapshot>>;
    fn cancel(&self);
}

impl ScanSource for Controller {
    fn poll(&self) -> Option<Arc<UiSnapshot>> {
        Controller::poll(self)
    }

    fn cancel(&self) {
        Controller::cancel(self);
    }
}

/// Starts a scan on `controller` and waits for it to finish or time out.
///
/// The timing clock starts after the controller is created, started and given
/// its view request, so setup cost is excluded from the reported wall time.
pub fn execute_scan(controller: &mut Controller, options: &ScanOptions) -> ScanRun {
    execute_scan_limited(controller, options, crate::engine::ScanLimits::default())
}

pub fn execute_scan_limited(
    controller: &mut Controller,
    options: &ScanOptions,
    limits: crate::engine::ScanLimits,
) -> ScanRun {
    let generation = controller.start_limited(options.root.clone(), options.backend, limits);
    controller.request_view(options.view_request());
    wait_for_scan(&*controller, generation, options.timeout)
}

fn wait_for_scan(source: &impl ScanSource, generation: u64, timeout: Duration) -> ScanRun {
    let started_utc_epoch_ms = crate::evidence::utc_now_epoch_ms();
    let started = Instant::now();
    let mut latest: Option<Arc<UiSnapshot>> = None;
    let mut first_result_ms = None;
    let timed_out = loop {
        if let Some(snapshot) = source.poll()
            && snapshot.progress.generation == generation
        {
            let seen = snapshot.progress.files + snapshot.progress.folders;
            if first_result_ms.is_none() && seen > 0 {
                first_result_ms = Some(millis(started.elapsed()));
            }
            latest = Some(snapshot);
        }
        if latest
            .as_ref()
            .is_some_and(|snapshot| !snapshot.progress.state.is_active())
        {
            break false;
        }
        if started.elapsed() > timeout {
            source.cancel();
            // A timeout yields a partial report. Do not wait for the cancel to
            // be acknowledged: it can sit behind blocked OS I/O indefinitely.
            break true;
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    let finished_utc_epoch_ms = if !timed_out
        && latest.as_ref().is_some_and(|snapshot| {
            matches!(
                snapshot.progress.state,
                ScanState::Complete | ScanState::Partial | ScanState::Cancelled | ScanState::Failed
            )
        }) {
        crate::evidence::utc_now_epoch_ms()
    } else {
        None
    };
    ScanRun {
        generation,
        started_utc_epoch_ms,
        finished_utc_epoch_ms,
        snapshot: latest.unwrap_or_default(),
        timed_out,
        first_result_ms,
        wall_elapsed: started.elapsed(),
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * MILLIS_PER_SECOND
}

/// The report fields every front end shares: the scan outcome and its timings.
/// Callers add their own identification, error list and extras.
pub fn summary_fields(run: &ScanRun) -> Map<String, Value> {
    let progress = &run.snapshot.progress;
    let state = if run.timed_out {
        TIMED_OUT_LABEL
    } else {
        progress.state.label()
    };
    [
        ("state", json!(state)),
        ("complete", json!(run.is_complete())),
        ("backend", json!(progress.backend)),
        ("files", json!(progress.files)),
        ("folders", json!(progress.folders)),
        ("logical_bytes", json!(progress.logical)),
        ("unique_allocated_bytes", json!(progress.unique_allocated)),
        ("hardlink_aliases", json!(progress.hardlinks)),
        ("skipped_reparse_entries", json!(progress.skipped)),
        ("errors_total", json!(progress.errors_total)),
        ("first_result_ms", json!(run.first_result_ms)),
        ("scan_elapsed_ms", json!(millis(progress.elapsed))),
        ("wall_elapsed_ms", json!(millis(run.wall_elapsed))),
        ("message", json!(progress.message)),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect()
}

/// The JSON report written by `--scan`.
pub fn build_report(root: &Path, run: &ScanRun) -> Value {
    let mut context = crate::evidence::EvidenceContext::new(root.to_path_buf());
    context.redaction = crate::evidence::Redaction::Full;
    context.access_context = match crate::admin::elevation_kind() {
        Ok(crate::admin::ElevationKind::Standard) => "standard",
        Ok(crate::admin::ElevationKind::Elevated) => "elevated_split_token",
        Ok(crate::admin::ElevationKind::AlwaysElevated) => "always_elevated",
        Err(_) => "unknown",
    }
    .into();
    context.session_context = "cli_read_only_no_gui_history".into();
    build_report_with_context(root, run, &context)
}

/// CLI-compatible fields plus the shared bounded evidence envelope. Redaction
/// applies to both legacy fields and the envelope, including opaque messages.
pub fn build_report_with_context(
    root: &Path,
    run: &ScanRun,
    context: &crate::evidence::EvidenceContext,
) -> Value {
    let mut context = context.clone();
    context.root = root.to_path_buf();
    context.timed_out = run.timed_out;
    context.started_utc_epoch_ms = run.started_utc_epoch_ms;
    context.finished_utc_epoch_ms = run.finished_utc_epoch_ms;
    let evidence = crate::evidence::build_report(&run.snapshot, &context);
    let mut report = summary_fields(run);
    report.insert("application".into(), json!("DiskScope"));
    report.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    report.insert("root_display".into(), evidence["root_display"].clone());
    report.insert("message".into(), evidence["scan"]["message"].clone());
    report.insert("backend".into(), evidence["scan"]["backend"].clone());
    report.insert("accounting".into(), json!(ACCOUNTING_NOTE));
    report.insert("errors".into(), evidence["errors"].clone());
    report.insert("largest_files".into(), evidence["rows"].clone());
    report.insert("evidence".into(), evidence);
    Value::Object(report)
}

/// Writes `report` as pretty JSON to `destination`, or to stdout (followed by a
/// newline) when there is none.
pub fn write_report(
    report: &Value,
    destination: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    match destination {
        Some(path) => crate::evidence::write_report_new(report, path)?,
        None => {
            let encoded = serde_json::to_vec_pretty(report)?;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&encoded)?;
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
}

/// Runs a `--scan` command to completion and returns the process exit code.
pub fn run_scan_command(options: &ScanOptions) -> Result<i32, Box<dyn std::error::Error>> {
    run_scan_command_with_policy(options, crate::session::SessionPolicy::default())
}

pub fn run_scan_command_with_policy(
    options: &ScanOptions,
    policy: crate::session::SessionPolicy,
) -> Result<i32, Box<dyn std::error::Error>> {
    if policy.helpdesk && options.backend != BackendChoice::Normal {
        return Err("Helpdesk policy requires the normal backend".into());
    }
    let mut controller = Controller::new();
    let run = execute_scan_limited(
        &mut controller,
        options,
        if policy.helpdesk {
            crate::engine::ScanLimits::helpdesk()
        } else {
            crate::engine::ScanLimits::default()
        },
    );
    let mut context = EvidenceContext::new(options.root.clone());
    context.redaction = Redaction::Full;
    context.access_context = match crate::admin::elevation_kind() {
        Ok(crate::admin::ElevationKind::Standard) => "standard",
        Ok(crate::admin::ElevationKind::Elevated) => "administrator",
        Ok(crate::admin::ElevationKind::AlwaysElevated) => "always-administrator",
        Err(_) => "unknown",
    }
    .into();
    context.session_context = if policy.helpdesk {
        "CLI helpdesk diagnostics; normal backend; bounded helpdesk inventory; no automatic profile persistence"
    } else {
        "CLI diagnostic scan; no automatic profile persistence"
    }.into();
    context.view_request = Some(options.view_request());
    let report = build_report_with_context(&options.root, &run, &context);
    write_report(&report, options.json.as_deref())?;
    Ok(run.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Progress, ScanError, ViewSnapshot};
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        fs,
    };

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    const ALL_STATES: [ScanState; 9] = [
        ScanState::Idle,
        ScanState::Preparing,
        ScanState::Scanning,
        ScanState::Finalizing,
        ScanState::Cancelling,
        ScanState::Cancelled,
        ScanState::Complete,
        ScanState::Partial,
        ScanState::Failed,
    ];

    // ---- argument parsing -------------------------------------------------

    #[test]
    fn helpdesk_and_session_only_launches_are_accepted_without_a_scan() {
        for values in [
            vec!["--helpdesk"],
            vec!["--no-history", "C:\\customer data"],
            vec!["--helpdesk", "--no-history", "D:\\"],
        ] {
            assert!(parse_args(&args(&values)).is_ok(), "{values:?}");
        }
        assert!(parse_args(&args(&["--helpdesk", "--scan", "C:\\"])).is_err());
    }

    #[test]
    fn scan_and_output_paths_remain_os_paths() {
        assert_eq!(
            parse_args(&args(&[
                "--scan",
                "C:\\data with spaces",
                "--json",
                "result.json",
                "--backend",
                "ntfs",
                "--timeout",
                "12"
            ]))
            .unwrap(),
            Command::Scan(ScanOptions {
                root: PathBuf::from("C:\\data with spaces"),
                json: Some("result.json".into()),
                backend: BackendChoice::Ntfs,
                view: ViewMode::Largest,
                timeout: Duration::from_secs(12),
            })
        );
    }

    #[test]
    fn no_args_does_not_implicitly_scan_anything() {
        assert_eq!(parse_args(&[]).unwrap(), Command::Gui(None));
    }

    #[test]
    fn single_arguments_select_help_version_or_a_gui_folder() {
        assert_eq!(parse_args(&args(&["--help"])).unwrap(), Command::Help);
        assert_eq!(parse_args(&args(&["--version"])).unwrap(), Command::Version);
        assert_eq!(
            parse_args(&args(&["a folder"])).unwrap(),
            Command::Gui(Some(PathBuf::from("a folder")))
        );
    }

    #[test]
    fn malformed_options_never_become_scan_roots() {
        assert!(parse_args(&args(&["--scan"])).is_err());
        assert!(parse_args(&args(&["--scan", "C:\\", "--timeout", "0"])).is_err());
        assert!(parse_args(&args(&["--backend", "broken"])).is_err());
        assert!(parse_args(&args(&["--unknown"])).is_err());
    }

    #[test]
    fn an_empty_scan_root_is_an_argument_error() {
        let error = parse_args(&args(&["--scan", ""])).unwrap_err();
        assert_eq!(error, "Scan root must not be empty");
    }

    #[test]
    fn defaults_describe_the_shipped_workload() {
        let options = parse_scan_args(&args(&["--scan", "a folder"]), OptionSet::Shipped).unwrap();
        assert_eq!(options.root, PathBuf::from("a folder"));
        assert_eq!(options.json, None);
        assert_eq!(options.backend, BackendChoice::Normal);
        assert_eq!(options.view, ViewMode::Largest);
        assert_eq!(options.timeout, DEFAULT_TIMEOUT);
        assert_eq!(options.timeout, Duration::from_secs(600));
        assert_eq!(options.view_request().mode, ViewMode::Largest);
        assert_eq!(options.view_request().limit, 100);
    }

    #[test]
    fn view_is_accepted_only_by_the_profiling_option_set() {
        let with_view = args(&["--scan", "x", "--view", "types"]);
        let error = parse_scan_args(&with_view, OptionSet::Shipped).unwrap_err();
        assert_eq!(error, "Unknown option: --view");
        let options = parse_scan_args(&with_view, OptionSet::Profiling).unwrap();
        assert_eq!(options.view, ViewMode::Types);
        assert_eq!(options.view_request().limit, OTHER_VIEW_LIMIT);
    }

    #[test]
    fn every_view_backend_and_timeout_value_is_explicit() {
        for (name, mode) in [
            ("largest", ViewMode::Largest),
            ("overview", ViewMode::Overview),
            ("explorer", ViewMode::Explorer),
            ("types", ViewMode::Types),
        ] {
            let options = parse_scan_args(
                &args(&[
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
                ]),
                OptionSet::Profiling,
            )
            .unwrap();
            assert_eq!(options.view, mode);
            assert_eq!(options.backend, BackendChoice::Ntfs);
            assert_eq!(options.timeout, Duration::from_secs(42));
            assert_eq!(options.json, Some(PathBuf::from("report.json")));
        }
    }

    #[test]
    fn malformed_arguments_never_turn_into_implicit_scans() {
        for values in [
            vec![],
            vec!["fixture", "other"],
            vec!["--scan"],
            vec!["--view", "largest"],
            vec!["--scan", "x", "--scan", "y"],
            vec!["--scan", "x", "--backend", "auto"],
            vec!["--scan", "x", "--view", "bad"],
            vec!["--scan", "x", "--timeout", "0"],
            vec!["--scan", "x", "--timeout", "-1"],
            vec!["--scan", "x", "--timeout", "soon"],
            vec!["--scan", "x", "--unknown", "x"],
            vec!["--scan", "x", "--json"],
        ] {
            for flags in [OptionSet::Shipped, OptionSet::Profiling] {
                assert!(
                    parse_scan_args(&args(&values), flags).is_err(),
                    "{values:?} with {flags:?}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn paths_retain_bytes_that_are_not_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(vec![b'/', b't', 0xff, 0xfe]);
        let options = parse_scan_args(
            &["--scan".into(), path.clone(), "--json".into(), path.clone()],
            OptionSet::Shipped,
        )
        .unwrap();
        assert_eq!(options.root.as_os_str(), path);
        assert_eq!(options.json.unwrap().as_os_str(), path);
    }

    #[cfg(windows)]
    #[test]
    fn paths_retain_unpaired_windows_units() {
        use std::os::windows::ffi::OsStringExt;
        let path = OsString::from_wide(&[0x44, 0x3a, 0x5c, 0xd800]);
        let options = parse_scan_args(
            &["--scan".into(), path.clone(), "--json".into(), path.clone()],
            OptionSet::Shipped,
        )
        .unwrap();
        assert_eq!(options.root.as_os_str(), path);
        assert_eq!(options.json.unwrap().as_os_str(), path);
    }

    // ---- help and exit codes ---------------------------------------------

    #[test]
    fn help_text_is_stable_and_lists_every_exit_code() {
        let text = help_text();
        for required in [
            "--helpdesk [FOLDER]",
            "--no-history [FOLDER]",
            "--scan PATH",
            "--json OUTPUT.json",
            "--backend normal|ntfs",
            "--timeout SECONDS",
            "--help | --version",
            "diskscope-policy.json",
            "profile persistence",
            "Explicit report exports still write",
            "Exit codes: 0 complete, 1 failure, 2 argument error, 3 timeout, 4 partial results.",
        ] {
            assert!(text.contains(required), "Help omitted {required}");
        }
    }

    #[test]
    fn exit_codes_follow_the_documented_table() {
        assert_eq!(exit_code(false, ScanState::Complete), 0);
        assert_eq!(exit_code(false, ScanState::Failed), 1);
        assert_eq!(exit_code(false, ScanState::Partial), 4);
        assert_eq!(EXIT_ARGUMENT_ERROR, 2);
        for state in ALL_STATES {
            assert_eq!(exit_code(true, state), 3, "timeout beats {state:?}");
        }
    }

    #[test]
    fn a_cancelled_scan_retains_partial_results_and_is_not_success() {
        assert_eq!(exit_code(false, ScanState::Cancelled), EXIT_PARTIAL);
        assert_ne!(exit_code(false, ScanState::Cancelled), EXIT_COMPLETE);
    }

    #[test]
    fn only_a_complete_scan_exits_zero() {
        for state in ALL_STATES {
            let code = exit_code(false, state);
            assert_eq!(
                code == EXIT_COMPLETE,
                state == ScanState::Complete,
                "{state:?}"
            );
        }
    }

    // ---- wait loop --------------------------------------------------------

    /// Hands out scripted snapshots one per poll, like `Controller::poll`
    /// (which takes the newest snapshot and then reports nothing until the next).
    struct ScriptedSource {
        snapshots: RefCell<VecDeque<Arc<UiSnapshot>>>,
        cancels: Cell<u32>,
    }

    impl ScriptedSource {
        fn new(snapshots: Vec<UiSnapshot>) -> Self {
            Self {
                snapshots: RefCell::new(snapshots.into_iter().map(Arc::new).collect()),
                cancels: Cell::new(0),
            }
        }
    }

    impl ScanSource for ScriptedSource {
        fn poll(&self) -> Option<Arc<UiSnapshot>> {
            self.snapshots.borrow_mut().pop_front()
        }

        fn cancel(&self) {
            self.cancels.set(self.cancels.get() + 1);
        }
    }

    fn snapshot(generation: u64, state: ScanState, files: u64, folders: u64) -> UiSnapshot {
        UiSnapshot {
            progress: Progress {
                generation,
                state,
                files,
                folders,
                ..Default::default()
            },
            view: ViewSnapshot::default(),
        }
    }

    /// Long enough that a scripted source is always drained first, short enough
    /// that the tests that expect a timeout stay fast.
    const TEST_TIMEOUT: Duration = Duration::from_millis(60);

    #[test]
    fn the_loop_returns_the_terminal_snapshot_without_cancelling() {
        let source = ScriptedSource::new(vec![
            snapshot(7, ScanState::Scanning, 0, 0),
            snapshot(7, ScanState::Scanning, 3, 1),
            snapshot(7, ScanState::Complete, 5, 2),
        ]);
        let run = wait_for_scan(&source, 7, Duration::from_secs(30));
        assert!(!run.timed_out);
        assert_eq!(source.cancels.get(), 0);
        assert_eq!(run.generation, 7);
        assert_eq!(run.state(), ScanState::Complete);
        assert_eq!(run.snapshot.progress.files, 5);
        assert!(run.first_result_ms.is_some());
        assert!(run.is_complete());
        assert_eq!(run.exit_code(), EXIT_COMPLETE);
    }

    #[test]
    fn first_result_stays_unset_for_a_scan_that_found_nothing() {
        let source = ScriptedSource::new(vec![snapshot(1, ScanState::Failed, 0, 0)]);
        let run = wait_for_scan(&source, 1, Duration::from_secs(30));
        assert_eq!(run.first_result_ms, None);
        assert_eq!(run.exit_code(), EXIT_FAILURE);
        assert!(!run.is_complete());
    }

    #[test]
    fn a_timeout_cancels_once_and_keeps_the_newest_partial_snapshot() {
        let source = ScriptedSource::new(vec![
            snapshot(3, ScanState::Scanning, 1, 0),
            snapshot(3, ScanState::Scanning, 9, 4),
        ]);
        let run = wait_for_scan(&source, 3, TEST_TIMEOUT);
        assert!(run.timed_out);
        assert_eq!(source.cancels.get(), 1);
        assert_eq!(run.snapshot.progress.files, 9);
        assert!(run.first_result_ms.is_some());
        assert!(!run.is_complete());
        assert_eq!(run.exit_code(), EXIT_TIMEOUT);
        assert!(run.wall_elapsed >= TEST_TIMEOUT);
    }

    #[test]
    fn snapshots_of_another_generation_never_end_the_wait() {
        // A finished scan from an earlier generation must not be mistaken for
        // the one just started.
        let source = ScriptedSource::new(vec![snapshot(2, ScanState::Complete, 99, 99)]);
        let run = wait_for_scan(&source, 3, TEST_TIMEOUT);
        assert!(run.timed_out);
        assert_eq!(source.cancels.get(), 1);
        assert_eq!(run.state(), ScanState::Idle);
        assert_eq!(run.snapshot.progress.files, 0);
        assert_eq!(run.first_result_ms, None);
    }

    #[test]
    fn a_cancelled_terminal_snapshot_ends_the_wait_as_partial() {
        let source = ScriptedSource::new(vec![snapshot(5, ScanState::Cancelled, 2, 1)]);
        let run = wait_for_scan(&source, 5, Duration::from_secs(30));
        assert!(!run.timed_out);
        assert!(!run.is_complete());
        assert_eq!(run.exit_code(), EXIT_PARTIAL);
    }

    // ---- report -----------------------------------------------------------

    fn finished(state: ScanState, errors_total: u64, timed_out: bool) -> ScanRun {
        let mut snapshot = snapshot(1, state, 4, 2);
        snapshot.progress.errors_total = errors_total;
        ScanRun {
            generation: 1,
            started_utc_epoch_ms: None,
            finished_utc_epoch_ms: None,
            snapshot: Arc::new(snapshot),
            timed_out,
            first_result_ms: Some(1.5),
            wall_elapsed: Duration::from_millis(20),
        }
    }

    #[test]
    fn complete_requires_a_finished_in_time_error_free_scan() {
        assert!(finished(ScanState::Complete, 0, false).is_complete());
        assert!(!finished(ScanState::Complete, 1, false).is_complete());
        assert!(!finished(ScanState::Complete, 0, true).is_complete());
        assert!(!finished(ScanState::Partial, 0, false).is_complete());
        assert!(!finished(ScanState::Cancelled, 0, false).is_complete());
    }

    #[test]
    fn the_report_keeps_the_documented_schema() {
        let report = build_report(
            Path::new("some root"),
            &finished(ScanState::Complete, 0, false),
        );
        let mut keys: Vec<_> = report.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "accounting",
                "application",
                "backend",
                "complete",
                "errors",
                "errors_total",
                "evidence",
                "files",
                "first_result_ms",
                "folders",
                "hardlink_aliases",
                "largest_files",
                "logical_bytes",
                "message",
                "root_display",
                "scan_elapsed_ms",
                "skipped_reparse_entries",
                "state",
                "unique_allocated_bytes",
                "version",
                "wall_elapsed_ms",
            ]
        );
        assert_eq!(report["application"], "DiskScope");
        assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(report["root_display"], "some root");
        assert_eq!(report["state"], "Scan complete");
        assert_eq!(report["complete"], true);
        assert_eq!(report["files"], 4);
        assert_eq!(report["folders"], 2);
        assert_eq!(report["first_result_ms"], 1.5);
        let wall = report["wall_elapsed_ms"].as_f64().unwrap();
        assert!((wall - 20.0).abs() < 1e-6, "{wall}");
    }

    #[test]
    fn a_timed_out_report_says_so_whatever_the_last_state_was() {
        let report = build_report(Path::new("r"), &finished(ScanState::Scanning, 0, true));
        assert_eq!(report["state"], "Timed out");
        assert_eq!(report["complete"], false);
    }

    #[test]
    fn errors_and_rows_are_listed_with_their_paths() {
        let mut snapshot = snapshot(1, ScanState::Partial, 1, 1);
        snapshot.progress.errors_total = 1;
        snapshot.progress.errors = vec![ScanError {
            path: "private/denied".into(),
            message: "Access denied".into(),
        }];
        let run = ScanRun {
            generation: 1,
            started_utc_epoch_ms: None,
            finished_utc_epoch_ms: None,
            snapshot: Arc::new(snapshot),
            timed_out: false,
            first_result_ms: None,
            wall_elapsed: Duration::ZERO,
        };
        let report = build_report(Path::new("r"), &run);
        assert_eq!(report["errors"][0]["path_display"], "private/denied");
        assert_eq!(report["errors"][0]["message"], "Access denied");
        assert_eq!(report["largest_files"], json!([]));
        assert_eq!(report["first_result_ms"], Value::Null);
        assert_eq!(report["complete"], false);
    }

    #[test]
    fn cli_report_adds_evidence_without_claiming_an_inventory_export() {
        let report = build_report(Path::new("r"), &finished(ScanState::Complete, 0, false));
        assert_eq!(report["evidence"]["schema_version"], 1);
        assert_eq!(report["evidence"]["coverage"]["scan_complete"], true);
        assert_eq!(
            report["evidence"]["coverage"]["full_inventory_export"],
            false
        );
        assert_eq!(report["evidence"]["privacy"]["path_mode"], "full");
        assert!(
            report["evidence"]["timestamps"]["generated_utc_epoch_ms"]
                .as_u64()
                .is_some()
        );
    }

    #[test]
    fn cli_observes_start_and_terminal_utc_without_inventing_timeout_completion() {
        let source = ScriptedSource::new(vec![snapshot(1, ScanState::Complete, 1, 1)]);
        let run = wait_for_scan(&source, 1, Duration::from_secs(30));
        assert!(run.started_utc_epoch_ms.is_some());
        assert!(run.finished_utc_epoch_ms.is_some());
        let report = build_report(Path::new("r"), &run);
        assert_eq!(
            report["evidence"]["timestamps"]["started_utc_epoch_ms"],
            json!(run.started_utc_epoch_ms)
        );
        assert_eq!(
            report["evidence"]["timestamps"]["finished_utc_epoch_ms"],
            json!(run.finished_utc_epoch_ms)
        );
        let timed_out = wait_for_scan(&ScriptedSource::new(vec![]), 1, TEST_TIMEOUT);
        assert!(timed_out.started_utc_epoch_ms.is_some());
        assert_eq!(timed_out.finished_utc_epoch_ms, None);
    }

    #[test]
    fn redacted_cli_report_cannot_leak_through_legacy_fields() {
        let mut run = finished(ScanState::Partial, 1, false);
        let snapshot = Arc::make_mut(&mut run.snapshot);
        snapshot.progress.message = "private-customer cannot be read".into();
        snapshot.progress.errors = vec![ScanError {
            path: "private-customer/denied".into(),
            message: "private-customer was denied".into(),
        }];
        let context = crate::evidence::EvidenceContext::new("private-customer/root".into());
        let report = build_report_with_context(Path::new("private-customer/root"), &run, &context);
        assert!(!report.to_string().contains("private-customer"));
        assert_eq!(report["root_display"], "[redacted]");
        assert_eq!(report["errors"][0]["message"], "[redacted]");
    }

    #[test]
    fn writing_a_report_never_clobbers_an_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.json");
        fs::write(&path, b"existing incident evidence").unwrap();
        assert!(write_report(&json!({"replacement":true}), Some(&path)).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"existing incident evidence");
    }

    #[test]
    fn report_files_are_pretty_json_without_a_trailing_newline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.json");
        let report = json!({ "b": 1, "a": [true] });
        write_report(&report, Some(&path)).unwrap();
        let written = fs::read(&path).unwrap();
        assert_eq!(written, serde_json::to_vec_pretty(&report).unwrap());
        assert_eq!(serde_json::from_slice::<Value>(&written).unwrap(), report);
    }

    // ---- real scans -------------------------------------------------------

    #[test]
    fn execute_scan_reports_a_real_directory() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("a.txt"), [1u8; 10]).unwrap();
        fs::write(temp.path().join("nested").join("b.bin"), [2u8; 20]).unwrap();
        let options = ScanOptions {
            root: temp.path().to_path_buf(),
            json: None,
            backend: BackendChoice::Normal,
            view: ViewMode::Largest,
            timeout: Duration::from_secs(60),
        };
        let mut controller = Controller::new();
        let run = execute_scan(&mut controller, &options);
        assert!(run.is_complete(), "{:?}", run.snapshot.progress);
        assert_eq!(run.exit_code(), EXIT_COMPLETE);
        assert_eq!(run.snapshot.progress.files, 2);
        assert_eq!(run.snapshot.progress.logical, 30);
        let report = build_report(&options.root, &run);
        assert_eq!(report["largest_files"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn execute_scan_of_a_missing_root_fails() {
        let temp = tempfile::tempdir().unwrap();
        let options = ScanOptions {
            root: temp.path().join("missing"),
            json: None,
            backend: BackendChoice::Normal,
            view: ViewMode::Largest,
            timeout: Duration::from_secs(60),
        };
        let mut controller = Controller::new();
        let run = execute_scan(&mut controller, &options);
        assert!(!run.timed_out);
        assert_eq!(run.state(), ScanState::Failed);
        assert_eq!(run.exit_code(), EXIT_FAILURE);
    }
}
