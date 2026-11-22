use std::path::Path;
use std::process::Command;

// Build bas, bld and elfcheck in the same Cargo target/profile as bcc before
// running this test, and build libbreenix-libc for x86_64-breenix-c in release.
#[test]
fn return_42_exit_status() {
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let bas = bcc.with_file_name("bas");
    let bld = bcc.with_file_name("bld");
    let elfcheck = bcc.with_file_name("elfcheck");
    let libc = toolchain.join("../libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a");
    for input in [&bas, &bld, &elfcheck, &libc] {
        assert!(input.is_file(), "build the test input {}", input.display());
    }

    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let result = python
        .arg(toolchain.join("tests/bcc/return_42_exit_status.py"))
        .arg("--bcc")
        .arg(bcc)
        .arg("--bas")
        .arg(bas)
        .arg("--bld")
        .arg(bld)
        .arg("--elfcheck")
        .arg(elfcheck)
        .arg("--libc")
        .arg(libc)
        .output()
        .expect("run the codegen harness with x86-64 Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}

#[test]
fn sysv_scalar_calls() {
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
        .arg(toolchain.join("tests/bcc/scalar_calls.py"))
        .arg(bcc)
        .arg(bcc.with_file_name("bas"))
        .arg(bcc.with_file_name("bld"))
        .output()
        .expect("run the scalar ABI harness with x86-64 Python");
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}
