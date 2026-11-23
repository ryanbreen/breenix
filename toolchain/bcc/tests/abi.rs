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

#[test]
fn x87_return_roundtrip() {
    run_harness("x87.py", &[]);
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

#[test]
fn aarch64_va_list_layout() {
    use bcc::{pp, sema, types::Kind};

    let path = Path::new("aarch64-va-list.c");
    let options = pp::Options {
        target: pp::Target::Aarch64,
        ..pp::Options::default()
    };
    let source = b"extern __builtin_va_list list;";
    let input = pp::preprocess_source(path, source, &options).unwrap();
    let unit = sema::analyze(&input, path, options.target).unwrap();

    // AAPCS64 2025Q1 §10.1.5 Table 6 and the LP64 sizes in Table 3.
    let Kind::Record(record) = unit.objects[0].ty.kind else {
        panic!("va_list must be a struct");
    };
    let layout = record.layout.unwrap();
    assert_eq!((layout.size, layout.align), (32, 8));
    let definition = unit.records[record.id].definition.as_ref().unwrap();
    let members = definition
        .members
        .iter()
        .map(|member| (member.name, member.offset));
    assert_eq!(
        members.collect::<Vec<_>>(),
        [
            ("__stack", 0),
            ("__gr_top", 8),
            ("__vr_top", 16),
            ("__gr_offs", 24),
            ("__vr_offs", 28),
        ]
    );
}
