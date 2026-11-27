use super::*;
use bsync::tree::Entry;
use std::os::unix::fs::PermissionsExt;

#[test]
fn manifest_scopes_sync_and_protects_local_outputs() {
    let manifest = SyncManifest::load().unwrap();
    assert_eq!(
        manifest.roots,
        [
            "toolchain",
            "userland",
            "libs/libbreenix",
            "libs/input-abi",
            "libs/breengel-abi",
            "libs/gpu-abi",
        ]
    );
    let files = tempfile::tempdir().unwrap();
    for path in [
        "src/lib.rs",
        ".cargo/config.toml",
        "fixture/.cargo/config.toml",
        ".git/HEAD",
        "pkg/target/debug/foo",
        "boot.elf",
        "disk.img",
        ".btest-tree.json",
    ] {
        let path = files.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"example").unwrap();
    }
    let excludes = Excludes::new(&manifest.excludes).unwrap();
    let local = source_tree(files.path(), &excludes).unwrap();
    assert_eq!(
        local.keys().map(String::as_str).collect::<Vec<_>>(),
        ["", "src", "src/lib.rs"]
    );

    let mut remote = local.clone();
    remote.insert("target".into(), Entry::Other);
    assert!(source_plan(&local, &remote, &excludes).unwrap().is_empty());
    remote.insert("old.rs".into(), local["src/lib.rs"].clone());
    assert!(source_plan(&local, &remote, &excludes).is_err());
    assert!(remote.contains_key("old.rs"));
}

#[test]
fn stamp_binds_git_state_paths_modes_and_content() {
    let files = tempfile::tempdir().unwrap();
    let file = files.path().join("lib.rs");
    fs::write(&file, b"one").unwrap();
    let digest = || root_digest(&scan(files.path(), &Excludes::default()).unwrap());
    let before = digest();
    fs::write(&file, b"two").unwrap();
    let after = digest();
    assert_ne!(before, after);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    let executable = digest();
    assert_ne!(after, executable);
    fs::rename(&file, files.path().join("new.rs")).unwrap();
    assert_ne!(executable, digest());

    let stamp = TreeStamp {
        git: GitState {
            commit: "one".into(),
            origin_main: "two".into(),
            ancestor_of_main: false,
            dirty: true,
        },
        roots: [("toolchain".into(), after.clone())].into(),
    };
    assert_eq!(
        serde_json::to_value(&stamp).unwrap(),
        serde_json::json!({
            "commit": "one",
            "origin_main": "two",
            "ancestor_of_main": false,
            "dirty": true,
            "roots": {"toolchain": after}
        })
    );
}

#[test]
fn build_uses_current_and_the_selfhost_recipe() {
    let current = toolchain_id("x86_64-12345678-abcdef01\n").unwrap();
    for invalid in ["../unsafe", "x86_64-12345678-abcdef01\nextra", ""] {
        assert!(toolchain_id(invalid).is_err());
    }

    let words = cargo_words("/work/src/breenix", "/work/.cache/btest", current, "bobj");
    let fixture = "/opt/breenix/rust/x86_64-12345678-abcdef01";
    assert_eq!(
        &words[..5],
        ["/bin/time", "-p", "/bin/env", "-i", "PATH=/bin"]
    );
    assert!(words.contains(&format!("RUSTC={fixture}/bin/rustc")));
    assert_eq!(
        &words[13..],
        [
            &format!("{fixture}/bin/cargo"),
            "-Z",
            "unstable-options",
            "-C",
            "/work/src/breenix/toolchain",
            "build",
            "--offline",
            "--frozen",
            "--profile",
            "selfhost",
            "--target",
            &format!("{fixture}/targets/x86_64-breenix.json"),
            "--manifest-path",
            "/work/src/breenix/toolchain/Cargo.toml",
            "-p",
            "bobj",
            "--target-dir",
            "/work/.cache/btest/target",
        ]
    );
    let encoded = words
        .iter()
        .find_map(|word| word.strip_prefix("CARGO_ENCODED_RUSTFLAGS="))
        .unwrap();
    assert_eq!(
        encoded.split('\u{1f}').collect::<Vec<_>>(),
        [
            &format!("--sysroot={fixture}/sysroot"),
            &format!("-Clinker={fixture}/bin/bld"),
            &format!("-Clink-arg=-T{fixture}/lib/linker.ld"),
            &format!("-Lnative={fixture}/lib"),
            "-Clink-arg=--allow-multiple-definition",
            "-Cdefault-linker-libraries=no",
            "--remap-path-prefix=/work/src/breenix=/breenix-src",
        ]
    );
    assert_eq!(
        elapsed_ms("compile output\nreal 12.34\nuser 1.0\nsys 1.0\n"),
        Some(12340)
    );
    assert_eq!(elapsed_ms("real NaN\n"), None);
}
