//! Source ratchets supplement the live SSH oracle and host transcripts.
use std::{fs, path::PathBuf};
fn source(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}
#[test]
fn termination_waits_for_peer_and_preserves_shell_status() {
    let transport = source("libs/libbreenix/src/ssh/transport.rs");
    let server = source("userspace/programs/src/bsshd.rs");
    assert!(transport.contains("pub fn finish(&mut self, status: i32)"));
    assert!(source("libs/libbreenix/src/ssh/channel.rs").contains("channel.close_sent"));
    assert!(transport.contains("ch.closed"));
    assert!(server.contains("session.finish(status)"));
    assert!(server.contains("session.finish(shell_status)"));
}
#[test]
fn wrong_signer_reaches_signature_verification() {
    let auth = source("libs/libbreenix/src/ssh/auth.rs");
    assert!(auth.contains("wrong_identity.sign(data)"));
    assert!(!auth.contains("*last ^= 0x01"));
    assert!(auth.contains("keys::verify_rsa_signature(key_blob, signature, &signed_data)"));
    assert!(auth.contains("signature_algo == Some(algo)"));
}
#[test]
fn oracle_requires_auth_refusal_and_strict_scorer_pins_it() {
    let init = source("userspace/programs/src/init.rs");
    let client = source("userspace/programs/src/bssh.rs");
    let gate = source("docker/qemu/run-aarch64-boot-test-strict.sh");
    assert!(client.contains("std::process::exit(77)"));
    assert!(init.contains("right == 0 && wrong == 77"));
    assert!(init.contains("[BSSH_PUBKEY_ORACLE:right=ok:wrong=refused:PASS]"));
    assert!(gate.contains("[BSSH_PUBKEY_ORACLE:right=ok:wrong=refused:PASS]"));
    assert!(gate.contains("[BSSH_PUBKEY_ORACLE:FAIL"));
}

#[test]
fn scorer_rejects_missing_failed_and_duplicate_auth_evidence() {
    use std::process::Command;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = source("tests/fixtures/udp-socket-lock-aarch64-serial.txt");
    let marker = "[BSSH_PUBKEY_ORACLE:right=ok:wrong=refused:PASS]";
    let dir = std::env::temp_dir().join(format!("bssh-score-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    for (name, body, want) in [
        ("valid", fixture.clone(), true),
        ("missing", fixture.replace(marker, ""), false),
        (
            "malformed",
            fixture.replace(marker, &format!("{marker}suffix")),
            false,
        ),
        (
            "failed",
            fixture.replace(marker, "[BSSH_PUBKEY_ORACLE:FAIL:right=0:wrong=0]"),
            false,
        ),
        ("duplicate", format!("{fixture}\n{marker}\n"), false),
    ] {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let result = Command::new("bash")
            .arg(root.join("docker/qemu/run-aarch64-boot-test-strict.sh"))
            .env("BREENIX_STRICT_SCORE_ONLY", &path)
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            want,
            "{name}: {}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
    fs::remove_dir_all(dir).unwrap();
}
