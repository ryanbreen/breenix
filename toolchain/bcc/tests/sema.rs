use std::path::Path;
use std::process::Command;

#[test]
fn hello_translation_unit_types() {
    let runner = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/cconf/run.py");
    let failures: Vec<_> = ["x86_64", "aarch64"]
        .into_iter()
        .filter_map(|target| {
            let result = Command::new("python3")
                .arg(&runner)
                .args([
                    "--cc",
                    env!("CARGO_BIN_EXE_bcc"),
                    "--mode",
                    "types",
                    "--stage",
                    "b1",
                    "--types-flag",
                    "--dump-types",
                    "--target",
                    target,
                    "--cc-arg",
                    &format!("--target={target}"),
                ])
                .output()
                .expect("run cconf hello-types");
            let stdout = String::from_utf8_lossy(&result.stdout);
            if result.status.success() && stdout.lines().any(|line| line == "cconf::hello-types PASS") {
                return None;
            }
            Some(format!(
                "{target} ({}):\n{stdout}{}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            ))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn record_layout_psabi() {
    use bcc::{pp, sema};

    let path = Path::new("record-layout.c");
    let source = b"\
        struct Bytes { char first; char second; };\n\
        struct Padded { char first; long middle; short last; };\n\
        union Rounded { char bytes[9]; long word; };\n\
        struct Nested { char head; struct Padded inner; union Rounded tail; };\n\
        struct Wide { short first; long double wide; char last; };\n";
    let options = pp::Options {
        target: pp::Target::X86_64,
        ..pp::Options::default()
    };
    let input = pp::preprocess_source(path, source, &options).unwrap();
    let unit = sema::analyze(&input, path, options.target).unwrap();
    // AMD64 psABI §3.1.2 and Figure 3.1. Round each struct member's
    // offset up to its alignment, then round the total span up to the
    // strictest alignment. Union members all start at offset zero.
    let expected: &[(&str, usize, usize, &[usize])] = &[
        ("Bytes", 2, 1, &[0, 1]),
        ("Padded", 24, 8, &[0, 8, 16]),
        ("Rounded", 16, 8, &[0, 0]),
        ("Nested", 48, 8, &[0, 8, 32]),
        ("Wide", 48, 16, &[0, 16, 32]),
    ];
    assert_eq!(unit.records.len(), expected.len());
    for (record, &(tag, size, align, offsets)) in unit.records.iter().zip(expected) {
        assert_eq!(record.tag, Some(tag));
        let definition = record.definition.as_ref().unwrap();
        assert_eq!(definition.layout.size, size, "{tag} size");
        assert_eq!(definition.layout.align, align, "{tag} alignment");
        assert_eq!(
            definition.members.iter().map(|member| member.offset).collect::<Vec<_>>(),
            offsets,
            "{tag} member offsets",
        );
    }
}
