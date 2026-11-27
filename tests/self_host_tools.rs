//! Run with `rustc --edition=2021 --test tests/self_host_tools.rs`.
#[path = "../userspace/programs/src/suite_support/self_host_behave.rs"]
mod self_host_behave;
#[path = "../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

use self_host_behave::ToolTable;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn executes_only_the_digest_bound_rebuilt_path() {
    let root = std::env::temp_dir().join(format!("breenix-self-host-tools-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let fixture = Fixture(root);
    let profile = fixture.0.join(self_host_behave::PROFILE);
    fs::create_dir_all(&profile).unwrap();
    let mut digests = BTreeMap::new();
    for package in ["bcc", "bas", "bld", "bar", "bnm", "bmake"] {
        let script = format!("#!/bin/sh\nprintf '%s\\n' '{package}' \"$PATH\"\n");
        let executable = profile.join(package);
        fs::write(&executable, &script).unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        let digest: String = sha256::sha256(script.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        digests.insert(package.to_owned(), digest);
    }
    let table = ToolTable::stage(&fixture.0, &digests).unwrap();
    let bin = fixture.0.join("rebuilt-bin");
    let rebuilt = bin.join("cc");
    let bundle = fixture.0.join("bundle/bin/cc");
    fs::create_dir_all(bundle.parent().unwrap()).unwrap();
    fs::copy(&rebuilt, &bundle).unwrap();

    let output = table.output("cc", &mut Command::new(&rebuilt)).unwrap();
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        format!("bcc\n{}:/bin\n", bin.display()).as_bytes()
    );

    for executable in [PathBuf::from("cc"), bundle] {
        let error = table
            .output("cc", Command::new(executable).env("PATH", &bin))
            .unwrap_err();
        assert!(error.contains("expected rebuilt executable"), "{error}");
    }

    // The compiler would invoke the linker, so a changed ld also stops cc.
    fs::write(bin.join("ld"), b"#!/bin/sh\nexit 0\n").unwrap();
    let error = table.output("cc", &mut Command::new(&rebuilt)).unwrap_err();
    assert!(
        error.contains("SHA-256 changed") && error.contains("/ld"),
        "{error}"
    );
    fs::copy(profile.join("bld"), bin.join("ld")).unwrap();

    fs::write(&rebuilt, b"#!/bin/sh\nexit 0\n").unwrap();
    let error = table.output("cc", &mut Command::new(&rebuilt)).unwrap_err();
    assert!(
        error.contains("SHA-256 changed") && error.contains("/cc"),
        "{error}"
    );
    assert!(table.verify_all().unwrap_err().contains("SHA-256 changed"));
}

#[cfg(target_arch = "aarch64")]
#[test]
fn aarch64_text_bytes_match_arm_encoding_fixture() {
    let root = std::env::temp_dir().join(format!("breenix-arm-encodings-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let fixture = Fixture(root);
    let object = fixture.0.join("encodings.o");
    let corpus = PathBuf::from("toolchain/corpus/assembler-aarch64");
    let status = Command::new("toolchain/target/debug/bas")
        .args(["--target", "aarch64", "-o"])
        .arg(&object)
        .arg(corpus.join("encodings.s"))
        .status()
        .unwrap();
    assert!(status.success());
    let expected: Vec<u8> = fs::read_to_string(corpus.join("encodings.hex"))
        .unwrap()
        .lines()
        .flat_map(|line| {
            u32::from_str_radix(line.split_ascii_whitespace().next().unwrap(), 16)
                .unwrap()
                .to_le_bytes()
        })
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(
        self_host_behave::text_bytes(&fs::read(object).unwrap()),
        Some(expected.as_slice())
    );
}
