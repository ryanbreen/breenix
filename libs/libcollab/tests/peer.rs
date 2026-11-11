use libcollab::{host::{HostCore, Output, VERSION}, live_ops, peer::{PeerCore, Response}, wire::{decode, encode, DrawOp, Message}};

#[test]
fn join_replays_base_and_log_including_undo() {
    let mut host = HostCore::with_canvas(2, 2, vec![1, 2, 3, 4]);
    host.submit(DrawOp::Clear);
    host.submit(DrawOp::Undo);
    host.submit(DrawOp::Fill { x: 1, y: 1, color: [3, 2, 1] });
    host.connect(8);
    let frames = host.bytes(8, &encode(&Message::Hello { version: VERSION, name: vec![] }).unwrap());
    let mut peer = PeerCore::new();
    for output in frames {
        if let Output::Frame { conn: 8, data } = output { peer.receive(decode(&data).unwrap().unwrap().0).unwrap(); }
    }
    assert_eq!(peer.base_canvas(), &[1, 2, 3, 4]);
    assert_eq!(peer.log(), host.log());
    assert_eq!(live_ops(peer.log()).len(), 1);
}

#[test]
fn peer_tracks_answers_order_and_disconnect() {
    let mut peer = PeerCore::new();
    assert_eq!(peer.submit(DrawOp::Clear), 1);
    assert_eq!(peer.submit(DrawOp::Undo), 2);
    assert_eq!(peer.submit(DrawOp::Clear), 3);
    peer.receive(Message::Ack { local: 1, global: 1 }).unwrap();
    peer.receive(Message::Refused { local: 2, reason: b"nothing-to-undo".to_vec() }).unwrap();
    peer.receive(Message::Apply { global: 1, peer: 1, local: 1, op: DrawOp::Clear }).unwrap();
    assert!(peer.receive(Message::Apply { global: 3, peer: 2, local: 1, op: DrawOp::Clear }).is_err());
    assert_eq!(peer.log().len(), 1);
    peer.disconnect();
    assert_eq!(peer.response(1), Some(&Response::Ack { global: 1 }));
    assert_eq!(peer.response(2), Some(&Response::Refused(b"nothing-to-undo".to_vec())));
    assert_eq!(peer.response(3), Some(&Response::Refused(b"disconnected".to_vec())));
    assert_eq!(peer.disconnect_reason(), Some(&b"disconnected"[..]));
}
