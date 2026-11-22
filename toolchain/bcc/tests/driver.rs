use std::path::Path;
use std::process::Command;

fn check(case: &str) {
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/bcc/stages_agree_bytewise.py");
    let output = Command::new("python3")
        .arg(harness)
        .arg(case)
        .arg("--bcc")
        .arg(bcc)
        .arg("--bas")
        .arg(bcc.with_file_name("bas"))
        .arg("--bld")
        .arg(bcc.with_file_name("bld"))
        .arg("--bar")
        .arg(bcc.with_file_name("bar"))
        .output()
        .expect("run the driver harness");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn stages_agree_bytewise() {
    check("stages_agree_bytewise");
}

#[test]
fn unknown_option_rejected() {
    check("unknown_option_rejected");
}

#[test]
fn b_dir_pins_as_and_ld() {
    check("b_dir_pins_as_and_ld");
}
