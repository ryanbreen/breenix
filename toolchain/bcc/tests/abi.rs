use std::path::Path;
use std::process::Command;

#[test]
fn record_members_and_copy() {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let result = python
        .arg(toolchain.join("tests/bcc/records.py"))
        .arg(bcc)
        .arg(bcc.with_file_name("bas"))
        .arg(bcc.with_file_name("bld"))
        .output()
        .expect("run the record ABI harness with x86-64 Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}
