use ufs_abi::{Op, Reply, Request, MAX_DATA, MAX_MESSAGE, REPLY_HEADER, REQUEST_HEADER};

#[test]
fn operations_and_replies_round_trip() {
    let ops = [Op::Lookup, Op::Getattr, Op::Setattr, Op::Readdir, Op::Open, Op::Read,
        Op::Write, Op::Create, Op::Mkdir, Op::Unlink, Op::Rmdir, Op::Rename,
        Op::Fsync, Op::Statfs, Op::Release];
    let mut buf = vec![0; MAX_MESSAGE];
    for op in ops {
        let request = Request { id: 0x1020304050607080, op, node: 0x8877665544332211, payload: b"payload" };
        let n = request.encode(&mut buf).unwrap();
        assert_eq!(Request::decode(&buf[..n]).unwrap(), request);
        let reply = Reply { id: request.id, errno: -5, payload: b"reply" };
        let n = reply.encode(&mut buf).unwrap();
        assert_eq!(Reply::decode(&buf[..n]).unwrap(), reply);
    }
    let data = vec![42; MAX_DATA];
    let request = Request { id: 1, op: Op::Write, node: 2, payload: &data };
    let n = request.encode(&mut buf).unwrap();
    assert_eq!(n, MAX_MESSAGE);
    assert_eq!(Request::decode(&buf[..n]).unwrap(), request);
}

#[test]
fn malformed_messages_are_refused() {
    let mut buf = vec![0; MAX_MESSAGE + 1];
    let request = Request { id: 1, op: Op::Read, node: 2, payload: b"abc" };
    let n = request.encode(&mut buf).unwrap();
    for end in 0..n { assert!(Request::decode(&buf[..end]).is_err()); }
    assert!(Request::decode(&buf[..n + 1]).is_err());
    buf[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(Request::decode(&buf[..n]).is_err());
    assert!(Request::decode(&buf[..MAX_MESSAGE + 1]).is_err());
    assert!(Request { payload: &buf[..MAX_DATA + 1], ..request }.encode(&mut vec![0; MAX_MESSAGE + 1]).is_err());
    let reply = Reply { id: 1, errno: 5, payload: b"abc" };
    let n = reply.encode(&mut buf).unwrap();
    for end in 0..n { assert!(Reply::decode(&buf[..end]).is_err()); }
    assert!(Reply::decode(&buf[..n + 1]).is_err());
    assert!(Reply::decode(&buf[..REPLY_HEADER + MAX_DATA + 1]).is_err());
    assert!(request.encode(&mut [0; REQUEST_HEADER - 1]).is_err());
    assert!(reply.encode(&mut [0; REPLY_HEADER - 1]).is_err());
}
