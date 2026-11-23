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

#[test]
fn attribute_section_weak_visibility_in_object() {
    use bobj::elf::{Elf, SHF_ALLOC, SHF_EXECINSTR, SHF_WRITE, STB_WEAK, STT_FUNC, STT_OBJECT};
    use std::fs;

    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let root = std::env::temp_dir().join(format!("bcc-gnu-attributes-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/bcc/gnu/object_attributes.c");
    let run = |tool: &Path, args: &[&str]| {
        let out = Command::new(tool)
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    for (target, elfcheck_args) in [
        ("x86_64", &["fixture.o"][..]),
        ("aarch64", &["--machine", "aarch64", "fixture.o"][..]),
    ] {
        let cc_target = format!("--target={target}");
        run(
            bcc,
            &[
                &cc_target,
                "-std=gnu11",
                "-S",
                "-o",
                "fixture.s",
                source.to_str().unwrap(),
            ],
        );
        run(
            &bcc.with_file_name("bas"),
            &["--target", target, "-o", "fixture.o", "fixture.s"],
        );
        run(&bcc.with_file_name("elfcheck"), elfcheck_args);
        let nm = run(&bcc.with_file_name("bnm"), &["-P", "fixture.o"]);
        assert!(
            nm.lines().any(|line| line.starts_with("dispatch W ")),
            "{nm}"
        );
        assert!(nm.lines().any(|line| line.starts_with("mirror V ")), "{nm}");

        let bytes = fs::read(root.join("fixture.o")).unwrap();
        let elf = Elf::parse(&bytes).unwrap();
        let symbols = elf.static_symbols().unwrap().unwrap();
        let find = |name: &[u8]| {
            (1..symbols.len())
                .map(|i| symbols.get(i).unwrap())
                .find(|sym| sym.name == name)
                .unwrap()
        };
        // gABI 5.2–5.4: weak binding, function/object type, hidden=2, protected=3.
        let entry = find(b"entry");
        let dispatch = find(b"dispatch");
        assert_eq!((entry.kind, entry.other), (STT_FUNC, 2));
        assert_eq!(
            (dispatch.binding, dispatch.kind, dispatch.other),
            (STB_WEAK, STT_FUNC, 3)
        );
        assert_eq!(
            (dispatch.section, dispatch.value, dispatch.size),
            (entry.section, entry.value, entry.size)
        );
        assert!(entry.size > 0);
        let section = usize::from(entry.section);
        assert_eq!(elf.section_name(section).unwrap(), b".text.unit_entry");
        assert_eq!(
            elf.section(section).unwrap().flags,
            SHF_ALLOC | SHF_EXECINSTR
        );

        let data = find(b"payload");
        let mirror = find(b"mirror");
        assert_eq!(
            (data.binding, data.kind, data.other),
            (STB_WEAK, STT_OBJECT, 2)
        );
        assert_eq!((mirror.binding, mirror.kind), (STB_WEAK, STT_OBJECT));
        assert_eq!(
            (mirror.section, mirror.value, mirror.size),
            (data.section, data.value, 4)
        );
        let section = usize::from(data.section);
        assert_eq!(elf.section_name(section).unwrap(), b".data.unit_payload");
        assert_eq!(elf.section(section).unwrap().flags, SHF_ALLOC | SHF_WRITE);
        assert_eq!(
            elf.section_data(section).unwrap().unwrap(),
            42_u32.to_le_bytes()
        );
        assert!(nm.lines().any(|line| line.starts_with("keep t ")), "{nm}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn may_alias_type_rule() {
    check("may_alias_type_rule");
}

#[test]
fn named_variadic_and_comma_deletion() {
    check("named_variadic_and_comma_deletion");
}

#[test]
fn builtins_fold_and_lower() {
    check("builtins_fold_and_lower");
}

#[test]
fn enumerators_use_integer_constant_expressions() {
    check("enumerators_use_integer_constant_expressions");
}

#[test]
fn nostdinc_uses_only_explicit_header_paths() {
    check("nostdinc_uses_only_explicit_header_paths");
}
