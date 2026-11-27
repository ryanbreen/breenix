use std::fs;
use std::process::Command;

#[test]
fn test_refuses_missing_local_userland_inputs_before_ssh() {
    let files = tempfile::tempdir().unwrap();
    let checkout = files.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&checkout)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "--allow-empty",
        "-qm",
        "before userland",
    ]);
    git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

    let config = files.path().join("bdev.json");
    fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "host": "devbox",
            "source": "/work/src/breenix",
            "ssh_config": files.path().join("missing-ssh-config"),
        }))
        .unwrap(),
    )
    .unwrap();
    let refuses = |selector: &str, expected: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_bdev"))
            .current_dir(&checkout)
            .env("HOME", files.path())
            .arg("--config")
            .arg(&config)
            .args(["test", selector])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("missing-ssh-config"), "{stderr}");
        assert!(!files.path().join("Library/Caches/breenix-devbox").exists());
    };
    refuses(
        "yes",
        "missing local workspace manifest userland/Cargo.toml",
    );
    refuses(
        "--changed",
        "missing local workspace manifest userland/Cargo.toml",
    );

    let workspace = checkout.join("userland");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join("src/lib.rs"), "").unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let output = Command::new("cargo")
        .current_dir(workspace)
        .args(["generate-lockfile", "--offline"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    refuses("yes", "no local btest suite for package \"yes\"");
}
