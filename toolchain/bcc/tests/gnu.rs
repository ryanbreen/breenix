use std::path::Path;
use std::process::Command;

fn check(name: &str) {
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let harness = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/bcc/gnu.py");
    let mut python = if cfg!(target_os = "macos") {
        let mut command = Command::new("arch");
        command.args(["-x86_64", "/usr/bin/python3"]);
        command
    } else {
        Command::new("python3")
    };
    let result = python.arg(harness).arg(bcc).arg(name).output().unwrap();
    assert!(
        result.status.success(),
        "{}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
}

#[test]
fn statement_expression_value() {
    check("statement_expression_value");
}

#[test]
fn typeof_preserves_type_without_evaluation() {
    check("typeof_preserves_type_without_evaluation");
}

#[test]
fn extension_marker() {
    check("extension_marker");
}

#[test]
fn omitted_middle_evaluates_once() {
    check("omitted_middle_evaluates_once");
}

#[test]
fn case_ranges_are_inclusive() {
    check("case_ranges_are_inclusive");
}

#[test]
fn zero_length_array_layout() {
    check("zero_length_array_layout");
}

#[test]
fn range_designator_evaluates_once() {
    check("range_designator_evaluates_once");
}
