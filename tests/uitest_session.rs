#[path = "../libs/libuitest/src/supervisor.rs"]
pub mod supervisor;
use supervisor::{SessionEnvironment, StderrRing, Message, encode, decode, STDERR_CAP};

#[test]
fn stderr_ring_keeps_the_tail_and_counts_every_evicted_byte() {
    assert_eq!(STDERR_CAP, 65_536);
    let mut ring = StderrRing::default();
    let input: Vec<u8> = (0..STDERR_CAP * 3 + 37).map(|n| (n % 251) as u8).collect();
    for chunk in input.chunks(997) { ring.push(chunk); }
    assert_eq!(ring.bytes(), input[input.len() - STDERR_CAP..]);
    assert_eq!(ring.dropped(), (input.len() - STDERR_CAP) as u64);
    ring.push(&[]);
    assert_eq!(ring.bytes().len(), STDERR_CAP);
    let large = vec![13; STDERR_CAP + 1];
    ring.push(&large);
    assert_eq!(ring.bytes(), vec![13; STDERR_CAP]);
    assert_eq!(ring.dropped(), (input.len() + large.len() - STDERR_CAP) as u64);
    let other = StderrRing::default();
    assert_eq!(other.bytes(), b"");
    assert_eq!(other.dropped(), 0);
}

#[test]
fn session_environment_is_complete_and_has_no_inherited_or_duplicate_keys() {
    let session = SessionEnvironment::new(123, 17, "2560x1600@2").unwrap();
    assert_eq!(session.config_root(), "/tmp/uitest/123/");
    assert_eq!(session.environment(), vec![
        b"TZ=UTC".to_vec(), b"LANG=C.UTF-8".to_vec(),
        b"BREENIX_UI_CLOCK=/tmp/uitest/123/ui-clock".to_vec(),
        b"BREENIX_UI_SEED=17".to_vec(), b"BREENIX_UI_MODE=2560x1600@2".to_vec(),
        b"BREENIX_CONFIG_ROOT=/tmp/uitest/123/".to_vec(),
    ]);
    assert!(SessionEnvironment::new(0, 1, "2560x1600@2").is_err());
    assert!(SessionEnvironment::new(1, 1, "2560x1600@2").is_err());
    assert!(SessionEnvironment::new(123, 1, "").is_err());
    assert!(SessionEnvironment::new(123, 1, "x\0y").is_err());
    for message in [
        Message::Configure { case_pid: 123, seed: 17, mode: "2560x1600@2".into() },
        Message::Configured,
        Message::Motion { x: i32::MIN, y: i32::MAX },
        Message::StartCompositorWithPointer(17),
        Message::LaunchApp { argv: vec![b"/app".to_vec()], uid: 1000 },
        Message::ReadStderr(23),
        Message::Stderr { pid: 23, dropped: 999, bytes: vec![255, 0, 1], done: true },
    ] {
        let frame = encode(&message).unwrap();
        assert_eq!(decode(&frame[4..]).unwrap(), message);
    }
    assert!(encode(&Message::LaunchApp { argv: vec![b"/app".to_vec()], uid: 99 }).is_err());
}
