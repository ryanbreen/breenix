use std::path::Path;
use std::process::Command;

#[test]
fn record_members_and_copy() {
    run_harness("records.py", &[]);
}

#[test]
fn struct_int_double_by_value_bcc_to_clang() {
    run_harness("aggregates.py", &[]);
}

#[test]
fn bitfields_and_packed_bcc_to_clang() {
    run_harness("aggregates.py", &["bitfields", "packed"]);
}

#[test]
fn va_arg_after_gp_exhaustion() {
    run_harness("varargs.py", &[]);
}

fn run_harness(script: &str, args: &[&str]) {
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
        .arg(toolchain.join("tests/bcc").join(script))
        .arg(bcc)
        .arg(bcc.with_file_name("bas"))
        .arg(bcc.with_file_name("bld"))
        .args(args)
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
