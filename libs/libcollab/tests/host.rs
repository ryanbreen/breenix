use libcollab::host::{Event, HostCore, Output, VERSION};
use libcollab::wire::{decode, encode, DrawOp, Message};

fn send(h: &mut HostCore, conn: u32, msg: Message) -> Vec<Output> {
    h.bytes(conn, &encode(&msg).unwrap())
}
fn join(h: &mut HostCore, conn: u32) {
    h.connect(conn);
    send(h, conn, Message::Hello { version: VERSION, name: vec![conn as u8] });
}
fn frame(conn: u32, msg: Message) -> Output {
    Output::Frame { conn, data: encode(&msg).unwrap() }
}
fn op(local: u32, x: i16) -> Message {
    Message::Op { local, op: DrawOp::Fill { x, y: 0, color: [1, 2, 3] } }
}

#[test]
fn version_mismatch_is_refused() {
    let mut h = HostCore::new(4, 3);
    h.connect(1);
    let out = send(&mut h, 1, Message::Hello { version: VERSION + 1, name: vec![] });
    assert!(out.contains(&frame(1, Message::JoinRefused { reason: b"version".to_vec() })));
    assert!(out.contains(&Output::Event(Event::Close { conn: 1, reason: b"version".to_vec() })));
}

#[test]
fn sixteenth_client_is_refused() {
    let mut h = HostCore::new(4, 3);
    for conn in 1..=15 { join(&mut h, conn); }
    h.connect(16);
    let out = send(&mut h, 16, Message::Hello { version: VERSION, name: vec![] });
    assert!(out.contains(&frame(16, Message::JoinRefused { reason: b"full".to_vec() })));
}

#[test]
fn invalid_coordinates_are_refused_without_apply() {
    let mut h = HostCore::new(4, 3);
    join(&mut h, 1); join(&mut h, 2);
    let out = send(&mut h, 1, op(1, 4));
    assert_eq!(out, vec![frame(1, Message::Refused { local: 1, reason: b"invalid-coords".to_vec() })]);
    let out = send(&mut h, 1, op(2, 3));
    assert!(out.contains(&frame(1, Message::Ack { local: 2, global: 1 })));
}

#[test]
fn admitted_operations_reach_all_peers_in_order() {
    let mut h = HostCore::new(4, 3);
    join(&mut h, 1); join(&mut h, 2);
    let draw = DrawOp::Fill { x: 1, y: 0, color: [1, 2, 3] };
    let apply = Message::Apply { global: 1, peer: 1, local: 1, op: draw.clone() };
    let out = send(&mut h, 1, op(1, 1));
    assert!(out.contains(&frame(1, Message::Ack { local: 1, global: 1 })));
    assert!(out.contains(&frame(1, apply.clone())) && out.contains(&frame(2, apply.clone())));
    assert!(out.contains(&Output::Event(Event::Apply(apply))));
    let out = h.submit(draw.clone());
    let apply = Message::Apply { global: 2, peer: 0, local: 1, op: draw };
    assert!(out.contains(&frame(1, apply.clone())) && out.contains(&frame(2, apply.clone())));
    assert!(out.contains(&Output::Event(Event::Apply(apply))));
}

#[test]
fn sequence_gap_or_repeat_closes_only_offender() {
    for local in [1, 3] {
        let mut h = HostCore::new(4, 3);
        join(&mut h, 1); join(&mut h, 2);
        send(&mut h, 1, op(1, 0));
        let out = send(&mut h, 1, op(local, 0));
        assert!(out.contains(&Output::Event(Event::Close { conn: 1, reason: b"sequence".to_vec() })));
        assert!(out.contains(&frame(2, Message::PeerLeft { peer: 1, reason: b"sequence".to_vec() })));
        assert!(send(&mut h, 2, op(1, 0)).contains(&frame(2, Message::Ack { local: 1, global: 2 })));
    }
}

#[test]
fn malformed_bytes_close_and_notify_others() {
    let mut h = HostCore::new(4, 3);
    join(&mut h, 1); join(&mut h, 2);
    let out = h.bytes(1, &[0]);
    assert!(out.contains(&Output::Event(Event::Close { conn: 1, reason: b"decode".to_vec() })));
    assert!(out.contains(&frame(2, Message::PeerLeft { peer: 1, reason: b"decode".to_vec() })));
}

#[test]
fn fragmented_frames_and_close_are_handled() {
    let mut h = HostCore::new(4, 3);
    join(&mut h, 1); join(&mut h, 2);
    let bytes = encode(&op(1, 0)).unwrap();
    assert!(h.bytes(1, &bytes[..3]).is_empty());
    let out = h.bytes(1, &bytes[3..]);
    assert!(out.iter().any(|o| matches!(o, Output::Frame { conn: 2, data } if matches!(decode(data), Ok(Some((Message::Apply { global: 1, .. }, _)))))));
    assert!(h.close(1).contains(&frame(2, Message::PeerLeft { peer: 1, reason: b"disconnected".to_vec() })));
}
