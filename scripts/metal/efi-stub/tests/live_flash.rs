use std::format;
use std::string::{String, ToString};
use crate::live_flash::{Action, Manifest, choose};

fn manifest(token: char, digest: &str, size: usize) -> String {
    format!("BREENIX-FLASH 1\n{}\n{}\n{size}\n{digest}\n", token.to_string().repeat(32), "a".repeat(40))
}

#[test]
fn stage_must_be_complete_and_checksum_verified() {
    let text = manifest('1', "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", 3);
    let staged = Manifest::parse(text.as_bytes()).unwrap();
    assert!(staged.verifies(b"abc"));
    assert!(!staged.verifies(b"abd"));
    assert!(!staged.verifies(b"ab"));
    assert!(Manifest::parse(&text.as_bytes()[..text.len() - 1]).is_none());
    assert!(Manifest::parse(format!("{text}extra\n").as_bytes()).is_none());
    assert!(Manifest::parse(manifest('g', &"a".repeat(64), 3).as_bytes()).is_none());
    assert!(Manifest::parse(manifest('1', &"a".repeat(64), 0).as_bytes()).is_none());
}

#[test]
fn restart_at_each_publication_boundary_cannot_launch_twice_or_commit_early() {
    let old = [b'0'; 32];
    let new = [b'1'; 32];
    let cases = [
        // Image is incomplete or manifest is unpublished.
        (None, None, None, false, Action::Current),
        // The durable guest manifest arms the try.
        (Some(new), None, None, false, Action::Try),
        // The ESP copy is verified, but no attempted marker is durable.
        (Some(new), None, None, true, Action::Try),
        // Marker flushed, before or after handing off to candidate.
        (Some(new), Some(new), None, true, Action::Current),
        // An old acknowledgement cannot confirm this attempt.
        (Some(new), Some(new), Some(old), true, Action::Current),
        // After SSH and PID 1, the durable acknowledgement authorizes promotion.
        (Some(new), Some(new), Some(new), true, Action::Promote),
        // Journal recovery already moved the candidate into CURRENT.
        (Some(new), Some(new), Some(new), false, Action::Current),
        // A second flash can publish while the preceding commit is still on the ESP.
        (Some(old), Some(new), Some(new), true, Action::Promote),
        (Some(old), Some(new), Some(new), false, Action::Try),
    ];
    for (pending, attempted, acknowledged, retained, action) in cases {
        assert_eq!(choose(pending, attempted, acknowledged, retained), action);
    }
}
