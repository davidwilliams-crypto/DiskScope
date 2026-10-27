use std::{fs, process::Command};

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
