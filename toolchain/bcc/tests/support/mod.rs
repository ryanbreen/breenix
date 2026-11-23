use std::path::Path;
use std::process::Command;

pub fn aarch64_check_exit(args: &[&str]) {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let result = Command::new("python3")
        .arg(toolchain.join("tests/bcc/aarch64_return_42_exit_status.py"))
        .args([bcc, &bcc.with_file_name("bas"), &bcc.with_file_name("bld")])
        .args(args)
        .output()
        .expect("run the ARM64 codegen harness with native Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}
