#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
mod app;
#[cfg(feature = "gui")]
mod theme;

use std::{ffi::OsString, path::PathBuf};
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Gui(Option<PathBuf>),
    Scan {
        root: PathBuf,
        json: Option<PathBuf>,
        ntfs: bool,
        timeout: u64,
    },
    Help,
    Version,
}
fn parse_args(args: Vec<OsString>) -> Result<Command, String> {
    if args.is_empty() {
        return Ok(Command::Gui(None));
    }
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Command::Help);
    }
    if args.len() == 1 && args[0] == "--version" {
        return Ok(Command::Version);
    }
    if args.len() == 1 && !args[0].to_string_lossy().starts_with('-') {
        return Ok(Command::Gui(Some(args[0].clone().into())));
    }
    let (mut root, mut json, mut ntfs, mut timeout) = (None, None, false, 600u64);
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
                root = Some(PathBuf::from(next));
            }
            Some("--json") => json = Some(PathBuf::from(next)),
            Some("--backend") => {
                ntfs = match next.to_str() {
                    Some("normal") => false,
                    Some("ntfs") => true,
                    _ => return Err("Backend must be normal or ntfs".into()),
                }
            }
            Some("--timeout") => {
                timeout = next
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .filter(|n| *n > 0)
                    .ok_or("Timeout must be a positive number of seconds")?
            }
            _ => return Err(format!("Unknown option: {}", flag.to_string_lossy())),
        }
        i += 2;
    }
    Ok(Command::Scan {
        root: root.ok_or("Use --scan PATH for command-line analysis")?,
        json,
        ntfs,
        timeout,
    })
}

/// The release build uses the Windows GUI subsystem, which starts without a
/// console. Attaching to the parent console lets `--help`, `--version`, scan
/// JSON and error text reach a terminal that launched the executable. Without
/// a parent console the call fails harmlessly and output stays discarded.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: AttachConsole takes a process id constant and touches no memory
    // owned by this program.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
#[cfg(not(windows))]
fn attach_parent_console() {}

fn main() {
    attach_parent_console();
    let command = match parse_args(std::env::args_os().skip(1).collect()) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}\nUse --help for usage.");
            std::process::exit(2)
        }
    };
    let initial_path = match command {
        Command::Help => {
            println!(
                "DiskScope {}\n\n\
                 DiskScope.exe [FOLDER]\n\
                 DiskScope.exe --scan PATH [--json OUTPUT.json] [--backend normal|ntfs] [--timeout SECONDS]\n\
                 DiskScope.exe --help | --version\n\n\
                 Scans are read-only. Directory junctions and links are listed but not followed. \
                 NTFS mode requests no elevation and falls back when unavailable. \
                 Exit codes: 0 complete, 1 failure, 2 argument error, 3 timeout, 4 partial results.",
                env!("CARGO_PKG_VERSION")
            );
            return;
        }
        Command::Version => {
            println!("DiskScope {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Command::Scan {
            root,
            json,
            ntfs,
            timeout,
        } => match run_scan(root, json, ntfs, timeout) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1)
            }
        },
        Command::Gui(path) => path,
    };
    #[cfg(feature = "gui")]
    {
        let root = initial_path;
        let access_mode = app::AccessMode::from_probe(diskscope::admin::is_elevated());
        let mut options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_title(format!("DiskScope {}", env!("CARGO_PKG_VERSION")))
                .with_inner_size([1440.0, 900.0])
                .with_min_inner_size([900.0, 640.0]),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        };
        access_mode.configure_native_options(&mut options);
        if let Err(error) = eframe::run_native(
            "DiskScope",
            options,
            Box::new(move |cc| Ok(Box::new(app::DiskScopeApp::new(cc, root, access_mode)))),
        ) {
            eprintln!("DiskScope could not start: {error}");
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = initial_path;
        eprintln!("Build with the gui feature to launch DiskScope.");
        std::process::exit(2)
    }
}

fn run_scan(
    root: PathBuf,
    output: Option<PathBuf>,
    ntfs: bool,
    timeout: u64,
) -> Result<i32, Box<dyn std::error::Error>> {
    use diskscope::{engine::Controller, model::*};
    use std::time::{Duration, Instant};
    let mut controller = Controller::new();
    let generation = controller.start(
        root.clone(),
        if ntfs {
            BackendChoice::Ntfs
        } else {
            BackendChoice::Normal
        },
    );
    controller.request_view(ViewRequest {
        mode: ViewMode::Largest,
        limit: 100,
        ..Default::default()
    });
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
                first_result_ms = Some(started.elapsed().as_secs_f64() * 1000.0)
            }
            latest = Some(snapshot)
        }
        if latest
            .as_ref()
            .is_some_and(|s| !s.progress.state.is_active())
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(timeout) {
            controller.cancel();
            timed_out = true;
            // A timeout is a partial report; do not wait for blocked OS I/O.
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let snapshot = latest.unwrap_or_else(|| std::sync::Arc::new(UiSnapshot::default()));
    let p = &snapshot.progress;
    let errors: Vec<_> = p
        .errors
        .iter()
        .map(|e| {
            serde_json::json!({
                "path_display": e.path.to_string_lossy(),
                "message": e.message,
            })
        })
        .collect();
    let largest_files: Vec<_> = snapshot
        .view
        .rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "path_display": r.path.to_string_lossy(),
                "logical_bytes": r.logical,
                "allocated_bytes": r.allocated,
                "hardlink": r.hard_link,
            })
        })
        .collect();
    let report = serde_json::json!({
        "application": "DiskScope",
        "version": env!("CARGO_PKG_VERSION"),
        "root_display": root.to_string_lossy(),
        "state": if timed_out { "Timed out" } else { p.state.label() },
        "complete": !timed_out && p.state == ScanState::Complete && p.errors_total == 0,
        "backend": p.backend,
        "files": p.files,
        "folders": p.folders,
        "logical_bytes": p.logical,
        "unique_allocated_bytes": p.unique_allocated,
        "hardlink_aliases": p.hardlinks,
        "skipped_reparse_entries": p.skipped,
        "errors_total": p.errors_total,
        "first_result_ms": first_result_ms,
        "scan_elapsed_ms": p.elapsed.as_secs_f64() * 1000.0,
        "wall_elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
        "message": p.message,
        "accounting": "Logical bytes count file entries. Unique allocation deduplicates file IDs. ADS and filesystem metadata excluded.",
        "errors": errors,
        "largest_files": largest_files,
    });
    let encoded = serde_json::to_vec_pretty(&report)?;
    if let Some(path) = output {
        std::fs::write(path, encoded)?
    } else {
        use std::io::Write;
        std::io::stdout().write_all(&encoded)?;
        println!()
    }
    Ok(if timed_out {
        3
    } else if p.state == ScanState::Failed {
        1
    } else if p.state == ScanState::Partial {
        4
    } else {
        0
    })
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }
    #[test]
    fn scan_and_output_paths_remain_os_paths() {
        assert_eq!(
            parse_args(args(&[
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
            Command::Scan {
                root: PathBuf::from("C:\\data with spaces"),
                json: Some("result.json".into()),
                ntfs: true,
                timeout: 12
            }
        );
    }
    #[test]
    fn no_args_does_not_implicitly_scan_anything() {
        assert_eq!(parse_args(vec![]).unwrap(), Command::Gui(None));
    }
    #[test]
    fn malformed_options_never_become_scan_roots() {
        assert!(parse_args(args(&["--scan"])).is_err());
        assert!(parse_args(args(&["--scan", "C:\\", "--timeout", "0"])).is_err());
        assert!(parse_args(args(&["--backend", "broken"])).is_err());
        assert!(parse_args(args(&["--unknown"])).is_err());
    }
}
