#[path = "../libs/libuitest/src/supervisor.rs"]
pub mod supervisor;
use supervisor::{decode, encode, Decoder, Message, MAX_FRAME};

#[test]
fn launch_preserves_argument_and_environment_boundaries() {
    let message = Message::Launch {
        argv: vec![b"/bin/app".to_vec(), vec![], b"space newline\n=\\\"".to_vec(), vec![255]],
        env: vec![b"EMPTY=".to_vec(), b"VALUE=a b\n=x".to_vec()],
    };
    let frame = encode(&message).unwrap();
    assert_eq!(decode(&frame[4..]).unwrap(), message);
    for at in 0..frame.len() {
        let mut stream = Decoder::default();
        stream.push(&frame[..at]).unwrap();
        assert!(stream.next().unwrap().is_none());
        stream.push(&frame[at..]).unwrap();
        assert_eq!(stream.next().unwrap(), Some(message.clone()));
        assert!(stream.next().unwrap().is_none());
    }
}

#[test]
fn status_events_and_coalesced_messages_round_trip() {
    let messages = [Message::Ready(44), Message::Launched(45), Message::Exited { pid: 46, status: 7 << 8 },
        Message::Exited { pid: 45, status: 11 }, Message::Output { pid: 45, bytes: b"echo\n".to_vec() },
        Message::Error("exec failed".into()), Message::Teardown, Message::Stopped];
    let mut stream = Decoder::default();
    for message in &messages { stream.push(&encode(message).unwrap()).unwrap(); }
    for message in messages { assert_eq!(stream.next().unwrap(), Some(message)); }
    assert!(stream.next().unwrap().is_none());
    assert_eq!(supervisor::signal_name(11), "SIGSEGV");
    assert_eq!(supervisor::signal_name(9), "SIGKILL");
}

#[test]
fn grammar_refuses_truncation_trailing_bytes_and_invalid_launches() {
    let frame = encode(&Message::Launch { argv: vec![b"/app".to_vec()], env: vec![] }).unwrap();
    for end in 0..frame.len()-4 { assert!(decode(&frame[4..4+end]).is_err()); }
    let mut extra = frame[4..].to_vec(); extra.push(0);
    assert!(decode(&extra).is_err());
    assert!(decode(&[255]).is_err());
    for (argv, env) in [(vec![], vec![]), (vec![vec![]], vec![]), (vec![b"/a\0b".to_vec()], vec![]),
        (vec![b"/app".to_vec()], vec![b"NO_EQUALS".to_vec()]),
        (vec![b"/app".to_vec()], vec![b"=empty-key".to_vec()])] {
        assert!(encode(&Message::Launch { argv, env }).is_err());
    }
    let mut stream = Decoder::default();
    assert!(stream.push(&((MAX_FRAME+1) as u32).to_le_bytes()).is_err());
    assert!(encode(&Message::Output { pid: 1, bytes: vec![0; MAX_FRAME] }).is_err());
}
