use libcollab::wire::{decode, encode, DecodeError, DrawOp, Message};

fn messages() -> Vec<Message> {
    let ops = vec![
        DrawOp::StrokeSegment { gesture:3, tool:1, x0:1,y0:2,x1:3,y1:4,radius:5,color:[6,7,8] },
        DrawOp::Pencil { x0: 1, y0: 2, x1: 3, y1: 4, color: [5, 6, 7] },
        DrawOp::Brush { x0: 1, y0: 2, x1: 3, y1: 4, radius: 5, color: [6, 7, 8] },
        DrawOp::Eraser { x0: 1, y0: 2, x1: 3, y1: 4, radius: 5 },
        DrawOp::Line { x0: 1, y0: 2, x1: 3, y1: 4, color: [5, 6, 7] },
        DrawOp::Rectangle { x: 1, y: 2, width: 3, height: 4, color: [5, 6, 7] },
        DrawOp::Circle { x: 1, y: 2, radius: 3, color: [4, 5, 6] },
        DrawOp::Fill { x: 1, y: 2, color: [3, 4, 5] }, DrawOp::Clear, DrawOp::Undo,
    ];
    let mut messages = vec![
        Message::Hello { version: 1, name: b"Alice".to_vec() },
        Message::Welcome { peer: 2, width: 640, height: 480 },
        Message::JoinRefused { reason: b"full".to_vec() },
        Message::SyncChunk { offset: 42, data: vec![1, 2, 3] }, Message::SyncEnd,
        Message::Ack { local: 3, global: 4 }, Message::Refused { local: 3, reason: b"no".to_vec() },
        Message::PeerJoined { peer: 2, name: b"Bob".to_vec() },
        Message::PeerLeft { peer: 2, reason: b"bye".to_vec() }, Message::Bye,
    ];
    for op in ops { messages.push(Message::Op { local: 3, op: op.clone() }); messages.push(Message::Apply { global: 4, peer: 2, local: 3, op }); }
    messages
}

#[test]
fn round_trip_and_stream_boundaries() {
    let messages = messages();
    let mut stream = Vec::new();
    for message in &messages {
        let frame = encode(message).unwrap();
        assert_eq!(frame[0], 0xbc);
        assert_eq!(u16::from_le_bytes([frame[2], frame[3]]) as usize, frame.len() - 4);
        for end in 0..frame.len() { assert_eq!(decode(&frame[..end]), Ok(None)); }
        assert_eq!(decode(&frame), Ok(Some((message.clone(), frame.len()))));
        stream.extend(frame);
    }
    let mut offset = 0;
    for expected in messages { let (actual, used) = decode(&stream[offset..]).unwrap().unwrap(); assert_eq!(actual, expected); offset += used; }
    assert_eq!(offset, stream.len());
}

#[test]
fn malformed_frames_return_typed_errors() {
    assert_eq!(decode(&[0, 1, 0, 0]), Err(DecodeError::BadMagic));
    assert_eq!(decode(&[0xbc, 2, 0, 0]), Err(DecodeError::BadLength));
    assert_eq!(decode(&[0xbc, 255, 0, 0]), Err(DecodeError::UnknownType(255)));
}
