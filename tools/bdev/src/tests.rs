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
    assert!(source_plan(&local, &remote, &excludes, false)
        .unwrap()
        .is_empty());
    remote.insert("old.rs".into(), local["src/lib.rs"].clone());
    assert!(source_plan(&local, &remote, &excludes, false).is_err());
    assert!(remote.contains_key("old.rs"));
}

#[test]
fn source_sync_resumes_with_bsync_staging_and_can_prune_removed_sources() {
    let manifest = SyncManifest::load().unwrap();
    let excludes = Excludes::new(&manifest.excludes).unwrap();
    let dir = Entry::Dir(0o755);
    let file = Entry::File {
        size: 4,
        mode: 0o644,
        digest: "source-digest".into(),
    };
    let local = [
        ("".into(), dir.clone()),
        ("src".into(), dir.clone()),
        ("src/new.rs".into(), file.clone()),
    ]
    .into();
    let mut remote: Tree = [
        ("".into(), dir.clone()),
        ("src".into(), dir.clone()),
        (
            "src/.bsync-e4f4fda39a28e6c798ca06e1e41d4b71".into(),
            Entry::Other,
        ),
        ("target".into(), Entry::Other),
    ]
    .into();
    let plan = |remote: &Tree, delete| source_plan(&local, remote, &excludes, delete);
    assert_eq!(
        plan(&remote, false)
            .unwrap()
            .iter()
            .map(Action::label)
            .collect::<Vec<_>>(),
        ["UPLOAD"]
    );

    remote.extend([
        ("src/old.rs".into(), file.clone()),
        ("unused".into(), dir),
        ("unused/lib.rs".into(), file),
    ]);
    assert!(plan(&remote, false).is_err());
    assert_eq!(
        plan(&remote, true)
            .unwrap()
            .iter()
            .map(|action| (action.label(), action.path()))
            .collect::<Vec<_>>(),
        [
            ("UPLOAD", "src/new.rs"),
            ("DELETE", "src/old.rs"),
            ("DELETE", "unused/lib.rs"),
            ("RMDIR", "unused"),
        ]
    );
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

#[test]
fn ssh_exec_starts_with_a_bare_program() {
    let remote = BoxPaths {
        host: "USER@HOST",
        source: "/work/src/breenix",
        cache: "/work/.cache/btest",
        ssh_program: Path::new("/usr/bin/ssh"),
        ssh_args: &[],
    };
    let command = remote.ssh(&["/bin/env", "-i", "RUSTFLAGS=one two"]);
    assert_eq!(
        command.get_args().last().unwrap(),
        "/bin/env '-i' 'RUSTFLAGS=one two'"
    );
}

#[test]
fn interrupted_transfer_invalidates_stamp_before_writes() {
    let files = tempfile::tempdir().unwrap();
    let source = files.path().join("source");
    let cache = files.path().join("cache");
    let ssh_program = files.path().join("ssh");
    fs::create_dir(&source).unwrap();
    fs::write(
        &ssh_program,
        "#!/bin/sh\nfor request do :; done\nexec /bin/sh -c \"$request\"\n",
    )
    .unwrap();
    fs::set_permissions(&ssh_program, fs::Permissions::from_mode(0o700)).unwrap();
    let remote = BoxPaths {
        host: "USER@HOST",
        source: source.to_str().unwrap(),
        cache: cache.to_str().unwrap(),
        ssh_program: &ssh_program,
        ssh_args: &[],
    };
    let stamp = source.join(STAMP);
    let old_stamp =
        br#"{"commit":"old","origin_main":"old","ancestor_of_main":true,"dirty":false,"roots":{"toolchain":"old-digest"}}"#;
    fs::write(&stamp, old_stamp).unwrap();

    let mut stamp_existed_at_transfer = None;
    let result = publish_source(&remote, || {
        stamp_existed_at_transfer = Some(stamp.exists());
        fs::write(source.join("changed.rs"), b"new")?;
        Err("connection lost before the next upload".into())
    });
    assert_eq!(
        result.unwrap_err().to_string(),
        "connection lost before the next upload"
    );
    assert_eq!(stamp_existed_at_transfer, Some(false));
    assert!(!stamp.exists());
    assert!(build(&remote, "bobj")
        .unwrap_err()
        .to_string()
        .contains("sync the source tree before building"));
    assert!(!cache.exists());
}

#[test]
fn changed_paths_select_only_suites_reached_by_the_synced_sources() {
    let files = tempfile::tempdir().unwrap();
    let checkout = files.path().canonicalize().unwrap();
    let git = |args: &[&str]| super::git(&checkout, args).unwrap();
    git(&["init", "-q"]);
    for path in [
        "userland/yes/src/main.rs",
        "userland/yes/suite/manifest.json",
        "userland/cat/src/main.rs",
        "userland/cat/suite/manifest.json",
        "userland/crates/bu-format/src/lib.rs",
        "libs/format/src/lib.rs",
        "libs/format-extra/src/lib.rs",
        "kernel/src/main.rs",
    ] {
        let path = checkout.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "old").unwrap();
    }
    fs::write(
        checkout.join("userland/Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"yes\", \"cat\", \"crates/bu-format\"]\n",
    )
    .unwrap();
    for (dir, name, dependency) in [
        (
            "userland/yes",
            "yes",
            "bu-format = {path = \"../crates/bu-format\"}",
        ),
        ("userland/cat", "cat", ""),
        (
            "userland/crates/bu-format",
            "bu-format",
            "format = {path = \"../../../libs/format\"}",
        ),
        ("libs/format", "format", ""),
    ] {
        fs::write(
            checkout.join(dir).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n[dependencies]\n{dependency}\n"
            ),
        )
        .unwrap();
    }
    super::checked(
        Command::new("cargo")
            .current_dir(checkout.join("userland"))
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap(),
        "fixture Cargo lockfile",
    )
    .unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "base",
    ]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    fs::write(checkout.join("libs/format/src/lib.rs"), "committed").unwrap();
    fs::write(checkout.join("kernel/src/main.rs"), "unrelated").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "changes",
    ]);
    fs::write(checkout.join("userland/yes/src/helper.rs"), "untracked").unwrap();
    fs::write(checkout.join("libs/format-extra/src/new.rs"), "untracked").unwrap();

    let paths = crate::testing::changed_paths(&checkout).unwrap();
    assert_eq!(
        paths.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "kernel/src/main.rs",
            "libs/format-extra/src/new.rs",
            "libs/format/src/lib.rs",
            "userland/yes/src/helper.rs",
        ]
    );
    let layout = SyncManifest::load().unwrap().layout(&checkout).unwrap();
    assert!(layout.roots.contains("libs/format"));
    assert!(!layout.roots.contains("libs/format-extra"));
    let select = |paths: &BTreeSet<String>| {
        crate::testing::select_suites(paths, &layout.roots, &layout.suites).unwrap()
    };
    assert_eq!(select(&paths), ["yes"]);
    assert_eq!(
        select(&BTreeSet::from(["libs/format/src/lib.rs".into()])),
        ["yes"]
    );
    assert!(select(&BTreeSet::from([
        "libs/format-extra/src/new.rs".into(),
        "userland/yes/.cargo/config.toml".into()
    ]))
    .is_empty());
    assert_eq!(
        select(&BTreeSet::from(["userland/Cargo.lock".into()])),
        ["cat", "yes"]
    );
}

#[test]
fn successful_sync_publishes_the_stamp_after_transfer() {
    let files = tempfile::tempdir().unwrap();
    let source = files.path().to_str().unwrap();
    let script = files.path().join("ssh");
    fs::write(
        &script,
        "#!/bin/sh\nfor request do :; done\ncase \"$request\" in\n/bin/bput*) eval \"set -- ${request#/bin/bput }\"; cat > \"$1\";;\n*) exec /bin/sh -c \"$request\";;\nesac\n",
    ).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let remote = BoxPaths {
        host: "devbox",
        cache: "/unused",
        source,
        ssh_program: &script,
        ssh_args: &[],
    };
    let path = files.path().join(STAMP);
    fs::write(&path, "old stamp").unwrap();

    let bytes = publish_source(&remote, || {
        assert!(!path.exists());
        Ok(TreeStamp {
            git: GitState {
                commit: "branch".into(),
                origin_main: "main".into(),
                ancestor_of_main: false,
                dirty: true,
            },
            roots: BTreeMap::from([("userland".into(), "source-digest".into())]),
        })
    })
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        serde_json::json!({
            "commit": "branch", "origin_main": "main", "ancestor_of_main": false,
            "dirty": true, "roots": {"userland": "source-digest"}
        })
    );
}

#[test]
fn test_keeps_the_summary_and_returns_the_runner_status() {
    let files = tempfile::tempdir().unwrap();
    let script = files.path().join("ssh");
    fs::write(&script, concat!(
        "#!/bin/sh\nfor request do :; done\n",
        "case \"$request\" in *\"'btest' 'run' 'yes' '--json'\") ;; *) exit 90;; esac\n",
        "printf '%s\\n' '{\"kind\":\"case\",\"pkg\":\"yes\",\"id\":\"default-y\",\"status\":\"fail\",\"diff\":\"--- expected.out\\n+++ actual.out\"}'\n",
        "printf '%s\\n' '{\"kind\":\"summary\",\"suites\":[]}'\nexit 7\n",
    )).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let remote = BoxPaths {
        host: "devbox",
        source: "/work/src/breenix",
        cache: "/unused",
        ssh_program: &script,
        ssh_args: &[],
    };
    let receipt = files.path().join("run.json");
    let mut output = Vec::new();
    let status =
        crate::testing::run_btest(&remote, &["yes".into()], &receipt, &mut output).unwrap();
    assert_eq!(status, ExitCode::from(7));
    assert!(String::from_utf8(output)
        .unwrap()
        .contains("--- expected.out"));
    assert_eq!(
        fs::read_to_string(receipt).unwrap(),
        "{\"kind\":\"summary\",\"suites\":[]}\n"
    );
}
