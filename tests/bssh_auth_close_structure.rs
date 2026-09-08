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

/// Compile the production CLOSE arm, uint32 decoder, and CLOSE sender with a
/// packet sink. This exercises channel state and reply bytes without guest I/O.
#[test]
fn close_recipient_validation_precedes_completion() {
    use std::process::Command;
    let transport = source("libs/libbreenix/src/ssh/transport.rs");
    let arm = transport
        .split("SSH_MSG_CHANNEL_CLOSE => {")
        .nth(1)
        .unwrap()
        .split("SSH_MSG_DISCONNECT =>")
        .next()
        .unwrap();
    let buf = source("libs/libbreenix/src/ssh/mod.rs");
    let decoder = buf
        .split("    pub fn get_u32(")
        .nth(1)
        .unwrap()
        .split("    /// Read an SSH string")
        .next()
        .unwrap();
    let channel = source("libs/libbreenix/src/ssh/channel.rs");
    let sender = channel
        .split("pub fn send_channel_close(")
        .nth(1)
        .unwrap()
        .split("\n}\n")
        .next()
        .unwrap();
    let harness = format!(
        r#"
#[derive(Debug)]
pub enum SshError {{ Protocol(&'static str), ChannelNotFound, Disconnected, Io }}
pub struct Channel {{ pub local_id: u32, remote_id: u32, closed: bool, close_sent: bool }}
#[derive(Default)]
struct PacketIo {{ sent: Vec<Vec<u8>> }}
impl PacketIo {{ fn send_packet(&mut self, msg: &[u8]) -> Result<(), ()> {{ self.sent.push(msg.to_vec()); Ok(()) }} }}
pub struct SshBuf;
impl SshBuf {{
    pub fn get_u32({decoder}
    pub fn put_u32(buf: &mut Vec<u8>, n: u32) {{ buf.extend_from_slice(&n.to_be_bytes()); }}
}}
const SSH_MSG_CHANNEL_CLOSE: u8 = 97;
mod channel {{ use super::*; pub fn send_channel_close({sender}
}} }}
struct Session {{ channel: Option<Channel>, io: PacketIo }}
impl Session {{
fn recv_close(&mut self, msg: &[u8]) -> Result<Option<Vec<u8>>, SshError> {{
    match msg[0] {{ SSH_MSG_CHANNEL_CLOSE => {{ {arm} _ => unreachable!() }}
}}
}}
fn session(sent: bool) -> Session {{ Session {{ channel: Some(Channel {{ local_id: 0x01020304, remote_id: 9, closed: false, close_sent: sent }}), io: PacketIo::default() }} }}
fn main() {{
    let valid = [97, 1, 2, 3, 4];
    for sent in [false, true] {{
        for invalid in [vec![97], vec![97, 1], vec![97, 1, 2], vec![97, 1, 2, 3], vec![97, 0, 0, 0, 9], vec![97, 1, 2, 3, 5], vec![97, 1, 2, 3, 4, 0]] {{
            let mut s = session(sent);
            match s.recv_close(&invalid) {{
                Err(SshError::Protocol(reason)) => assert!(!reason.is_empty()),
                Err(SshError::ChannelNotFound) => (),
                other => panic!("invalid CLOSE accepted: {{invalid:?}}: {{other:?}}"),
            }}
            assert!(!s.channel.as_ref().unwrap().closed);
            assert_eq!(s.channel.as_ref().unwrap().close_sent, sent);
            assert!(s.io.sent.is_empty());
            assert!(matches!(s.recv_close(&valid), Err(SshError::Disconnected)));
            assert!(s.channel.as_ref().unwrap().closed);
            assert_eq!(s.io.sent.len(), usize::from(!sent));
            if !sent {{ assert_eq!(s.io.sent[0], [97, 0, 0, 0, 9]); }}
            assert!(matches!(s.recv_close(&valid), Err(SshError::Disconnected)));
            assert_eq!(s.io.sent.len(), usize::from(!sent));
        }}
    }}
    let mut s = Session {{ channel: None, io: PacketIo::default() }};
    assert!(matches!(s.recv_close(&valid), Err(SshError::ChannelNotFound)));
    assert!(s.io.sent.is_empty());
}}
"#
    );
    let dir = std::env::temp_dir().join(format!("bssh-close-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("close.rs");
    let binary = dir.join("close");
    fs::write(&path, harness).unwrap();
    let build = Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings"])
        .arg(&path)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&binary).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    fs::remove_dir_all(dir).unwrap();
}
