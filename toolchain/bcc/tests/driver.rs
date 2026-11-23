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

#[test]
fn default_target_helper() {
    use bcc::driver::default_target;
    assert_eq!(default_target("breenix", "aarch64"), "aarch64");
    assert_eq!(default_target("breenix", "x86_64"), "x86_64");
    assert_eq!(default_target("macos", "aarch64"), "x86_64");
    assert_eq!(default_target("linux", "x86_64"), "x86_64");
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
