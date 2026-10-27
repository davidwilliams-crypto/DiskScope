use std::{
    ffi::OsStr,
    fs,
    process::{Command, Output},
};

#[test]
fn report_matches_real_files_and_keeps_output_outside_the_scan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data with spaces");
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("a.txt"), [1u8; 123]).unwrap();
    fs::write(root.join("nested").join("b.bin"), [2u8; 4097]).unwrap();
    fs::write(root.join("empty"), []).unwrap();
    let output = temp.path().join("report.json");
    let result = Command::new(env!("CARGO_BIN_EXE_diskscope"))
        .arg("--scan")
        .arg(&root)
        .arg("--json")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(report["complete"], true);
    assert_eq!(report["logical_bytes"], 4220);
    assert_eq!(report["files"], 3);
    assert_eq!(report["folders"], 1);
    assert_eq!(report["errors_total"], 0);
    assert!(report["largest_files"].as_array().unwrap().len() <= 100);
}

#[test]
fn missing_root_is_a_failed_report_and_nonzero_exit() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("failed.json");
    let result = Command::new(env!("CARGO_BIN_EXE_diskscope"))
        .arg("--scan")
        .arg(temp.path().join("missing"))
        .arg("--json")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(report["complete"], false);
    assert_eq!(report["state"], "Scan failed");
    assert!(report["errors_total"].as_u64().unwrap() > 0);
}

fn run_cli(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_diskscope"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_and_version_go_to_stdout_and_exit_zero() {
    let help = run_cli(&["--help".as_ref()]);
    assert_eq!(help.status.code(), Some(0));
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.starts_with(&format!("DiskScope {}\n", env!("CARGO_PKG_VERSION"))));
    // The README's exit-code table is the contract; --help must state the same.
    assert!(
        help.contains(
            "Exit codes: 0 complete, 1 failure, 2 argument error, 3 timeout, 4 partial results."
        ),
        "{help}"
    );
    let version = run_cli(&["--version".as_ref()]);
    assert_eq!(version.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("DiskScope {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn argument_errors_exit_two_without_scanning_or_printing_a_report() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().as_os_str();
    let cases: [&[&OsStr]; 7] = [
        &["--scan".as_ref()],
        &["--scan".as_ref(), "".as_ref()],
        &["--scan".as_ref(), root, "--timeout".as_ref(), "0".as_ref()],
        &[
            "--scan".as_ref(),
            root,
            "--backend".as_ref(),
            "zfs".as_ref(),
        ],
        &["--scan".as_ref(), root, "--unknown".as_ref(), "x".as_ref()],
        // --view belongs to the profiling example only.
        &["--scan".as_ref(), root, "--view".as_ref(), "types".as_ref()],
        &["--json".as_ref(), "report.json".as_ref()],
    ];
    for args in cases {
        let result = run_cli(args);
        assert_eq!(result.status.code(), Some(2), "{args:?}");
        assert!(result.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("Use --help for usage."),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn stdout_report_is_a_single_json_document_ending_in_a_newline() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("only.txt"), [7u8; 11]).unwrap();
    let result = run_cli(&["--scan".as_ref(), temp.path().as_os_str()]);
    assert_eq!(result.status.code(), Some(0));
    assert!(result.stdout.ends_with(b"\n"));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["application"], "DiskScope");
    assert_eq!(report["complete"], true);
    assert_eq!(report["logical_bytes"], 11);
}
