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
        .arg("--bmake")
        .arg(bcc.with_file_name("bmake"))
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
fn md_writes_make_dependencies() {
    check("md_writes_make_dependencies");
}

#[test]
fn gnu_mode_and_include_flags() {
    check("gnu_mode_and_include_flags");
}

#[test]
fn capital_s_preprocesses_before_assembly() {
    use bobj::elf::Elf;
    use std::fs;

    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let root = std::env::temp_dir().join(format!("bcc-capital-s-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::copy(bcc.with_file_name("bas"), root.join("as")).unwrap();
    fs::write(root.join("constant.h"), "#define VALUE 42\n").unwrap();
    fs::write(
        root.join("entry.S"),
        concat!(
            "#include \"constant.h\"\n#ifdef BUILD_ENTRY\nsection .text\nglobal entry\n",
            "entry:\n    mov eax, VALUE\n    ret\n#endif\n",
        ),
    )
    .unwrap();

    let output = Command::new(bcc)
        .current_dir(&root)
        .args([
            "--target=x86_64", "-B", ".", "-c", "-DBUILD_ENTRY", "-Wp,-MD,entry.d", "entry.S",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(root.join("entry.o")).unwrap();
    let elf = Elf::parse(&bytes).unwrap();
    let text = (1..elf.section_count())
        .find(|&index| elf.section_name(index).unwrap() == b".text")
        .unwrap();
    assert_eq!(elf.section_data(text).unwrap().unwrap(), [0xb8, 42, 0, 0, 0, 0xc3]);
    assert_eq!(
        fs::read_to_string(root.join("entry.d")).unwrap(),
        "entry.o: entry.S constant.h\n",
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn b_dir_pins_as_and_ld() {
    check("b_dir_pins_as_and_ld");
}

#[test]
fn default_target_helper() {
    use bcc::driver::default_target;
    assert_eq!(default_target("breenix", "aarch64"), "aarch64");
    assert_eq!(default_target("breenix", "x86_64"), "x86_64");
    assert_eq!(default_target("macos", "aarch64"), "x86_64");
    assert_eq!(default_target("linux", "x86_64"), "x86_64");
}

#[test]
fn aarch64_links_brt_before_libc() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("bcc-arm-link-order-{}", std::process::id()));
    fs::create_dir_all(root.join("bin")).unwrap();
    let ld = root.join("bin/ld");
    fs::write(&ld, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$BCC_LINK_ARGS\"\n").unwrap();
    fs::set_permissions(&ld, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bcc"))
        .args(["--target=aarch64", "-B"]).arg(root.join("bin"))
        .arg("user.o").env("BCC_LINK_ARGS", root.join("args"))
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let args = fs::read_to_string(root.join("args")).unwrap();
    let lines: Vec<_> = args.lines().collect();
    let user = lines.iter().position(|s| s.ends_with("user.o")).unwrap();
    let brt = lines.iter().position(|s| s.ends_with("lib/libbrt.a")).unwrap();
    let libc = lines.iter().position(|s| s.ends_with("lib/libc.a")).unwrap();
    assert!(user < brt && brt < libc, "{args}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn aarch64_stages_agree_bytewise() {
    use std::fs;
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let libc = toolchain.join("../libs/libbreenix-libc/target/aarch64-breenix/release/libc.a");
    let root = std::env::temp_dir().join(format!("bcc-arm-stages-{}", std::process::id()));
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::copy(bcc, root.join("bin/bcc")).unwrap();
    fs::copy(bcc.with_file_name("bas"), root.join("bin/as")).unwrap();
    fs::copy(bcc.with_file_name("bld"), root.join("bin/ld")).unwrap();
    fs::copy(&libc, root.join("lib/libc.a")).expect("build the ARM libc archive");
    // This program uses no brt helpers. The driver still opens the archive.
    fs::write(root.join("lib/libbrt.a"), b"!<arch>\n").unwrap();
    fs::copy(
        toolchain.join("../userspace/programs/linker.ld"),
        root.join("linker.ld"),
    )
    .unwrap();
    fs::write(root.join("main.c"), "int main(void) { return 19 + 23; }\n").unwrap();
    let run = |program: &Path, args: &[&str]| {
        let out = Command::new(program)
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}: {}",
            program.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let cc = root.join("bin/bcc");
    let as_ = root.join("bin/as");
    let ld = root.join("bin/ld");
    let bindir = root.join("bin");
    let bindir = bindir.to_str().unwrap();
    run(
        &cc,
        &[
            "--target=aarch64",
            "-B",
            bindir,
            "-S",
            "-o",
            "split.s",
            "main.c",
        ],
    );
    run(&as_, &["--target", "aarch64", "-o", "split.o", "split.s"]);
    run(
        &cc,
        &[
            "--target=aarch64",
            "-B",
            bindir,
            "-c",
            "-o",
            "direct.o",
            "main.c",
        ],
    );
    assert_eq!(
        fs::read(root.join("direct.o")).unwrap(),
        fs::read(root.join("split.o")).unwrap()
    );
    run(
        &ld,
        &[
            "-T",
            "linker.ld",
            "--gc-sections",
            "-o",
            "split",
            "split.o",
            "lib/libc.a",
        ],
    );
    run(
        &cc,
        &["--target=aarch64", "-B", bindir, "-o", "direct", "main.c"],
    );
    assert_eq!(
        fs::read(root.join("direct")).unwrap(),
        fs::read(root.join("split")).unwrap()
    );
    run(
        &bcc.with_file_name("elfcheck"),
        &["--machine", "aarch64", "--exec", "direct"],
    );
    fs::remove_dir_all(root).unwrap();
}

fn preprocess_fixture(name: &str, source: &str, args: &[&str]) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("breenix-bcc-{name}-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("input.c");
    std::fs::write(&path, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bcc"))
        .current_dir(&dir)
        .args(args)
        .args(["-E", "input.c"])
        .output();
    std::fs::remove_dir_all(&dir).unwrap();
    output.unwrap()
}

#[test]
fn std_c17_sets_version_201710() {
    for (flags, expected) in [
        (&[][..], "201112L"),
        (&["-std=c11"][..], "201112L"),
        (&["-std=c17"][..], "201710L"),
        (&["-std=c17", "-std=c11"][..], "201112L"),
    ] {
        let out = preprocess_fixture("standard", "__STDC_VERSION__\n", flags);
        assert!(out.status.success(), "{flags:?}: {out:?}");
        assert!(out.stderr.is_empty(), "{flags:?}: {out:?}");
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), expected);
    }
}

#[test]
fn unknown_std_rejected() {
    let out = Command::new(env!("CARGO_BIN_EXE_bcc"))
        .args(["-std=gnu17", "-E", "unused.c"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("<command-line>:1:1: error: ") && stderr.contains("-std=gnu17"),
        "{stderr}"
    );
}

#[test]
fn warning_flags() {
    let source = "#warning marker\n#pragma vendor_option\nint x;\n";
    for (flags, succeeds, severity, wall) in [
        (&["-fno-builtin"][..], true, Some("warning"), false),
        (&["-Wall"][..], true, Some("warning"), true),
        (&["-Wall", "-Werror"][..], false, Some("error"), true),
        (&["-w", "-Wall", "-Werror"][..], true, None, false),
    ] {
        let out = preprocess_fixture("warnings", source, flags);
        assert_eq!(out.status.success(), succeeds, "{flags:?}: {out:?}");
        assert_eq!(out.stdout.is_empty(), !succeeds, "{flags:?}: {out:?}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        if let Some(severity) = severity {
            assert!(
                stderr.starts_with(&format!("input.c:1:2: {severity}: marker\n")),
                "{flags:?}: {stderr}"
            );
            assert_eq!(
                stderr.contains(&format!("input.c:2:2: {severity}: unknown pragma")),
                wall,
                "{flags:?}: {stderr}"
            );
        } else {
            assert!(stderr.is_empty(), "{flags:?}: {stderr}");
        }
    }
    let fatal = preprocess_fixture("fatal", "#error fatal\n", &["-w"]);
    assert!(!fatal.status.success(), "{fatal:?}");
    assert_eq!(fatal.stderr, b"input.c:1:2: error: fatal\n");
}
