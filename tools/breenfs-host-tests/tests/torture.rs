use std::{fs, process::Command};

#[test]
fn short_run_writes_a_summary() {
    let dir = std::env::temp_dir().join(format!("breenfs-torture-test-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_breenfs-torture"))
        .args(["--hours", "0.002", "--checker", env!("CARGO_BIN_FILE_BREENFS_breenfs-check"), "--out"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let summary: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("summary.json")).unwrap()).unwrap();
    assert!(summary["images_checked"].as_u64().unwrap() > 0, "{summary}");
    assert!(summary["first_seed"].as_u64().is_some(), "{summary}");
    assert!(summary["last_seed"].as_u64().unwrap() >= summary["first_seed"].as_u64().unwrap(), "{summary}");
    assert_eq!(summary["failures"].as_u64(), Some(0), "{summary}");
    fs::remove_dir_all(dir).unwrap();
}
