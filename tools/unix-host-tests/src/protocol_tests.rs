use crate::addr::*;
use crate::object::*;
use crate::proto::UnixSock;
fn sk(kind: u16) -> std::sync::Arc<Socket> {
    Socket::new(1, kind, 0, SocketProto::Unix(UnixSock::new(kind))).unwrap()
}
fn send(sk: &std::sync::Arc<Socket>, data: &[u8]) -> Result<usize, i32> {
    sk.ops().sendmsg(
        sk,
        &mut SendMsg {
            data,
            dest: None,
            flags: 0,
            nonblock: true,
            rights: Vec::new(),
            cred: caller_ucred(),
        },
    )
}
fn receive(sk: &std::sync::Arc<Socket>, cap: usize, flags: u32) -> Result<RecvMsg, i32> {
    let mut msg = RecvMsg {
        cap,
        data: Vec::new(),
        flags,
        nonblock: true,
        out_flags: 0,
        from: None,
        rights: Vec::new(),
    };
    sk.ops().recvmsg(sk, &mut msg)?;
    Ok(msg)
}
fn pair(kind: u16) -> (std::sync::Arc<Socket>, std::sync::Arc<Socket>) {
    UnixSock::new_pair(kind, caller_ucred()).unwrap()
}
#[test]
fn bidirectional_stream_and_record_socketpairs() {
    for kind in [1, 2, 5] {
        let (a, b) = pair(kind);
        assert_eq!(send(&a, b"ping"), Ok(4));
        assert_eq!(receive(&b, 64, 0).unwrap().data, b"ping");
        assert_eq!(send(&b, b"pong"), Ok(4));
        assert_eq!(receive(&a, 64, 0).unwrap().data, b"pong");
    }
}
#[test]
fn dup_references_delay_eof_and_epipe() {
    let (a, b) = pair(1);
    let dup = b.clone();
    drop(b);
    assert_eq!(send(&a, b"x"), Ok(1));
    drop(dup);
    assert_eq!(send(&a, b"x"), Err(32));
    assert_eq!(receive(&a, 64, 0).unwrap().data, b"");
}
#[test]
fn records_and_truncation() {
    for kind in [2, 5] {
        let (a, b) = pair(kind);
        send(&a, b"abcdef").unwrap();
        send(&a, b"ghi").unwrap();
        let m = receive(&b, 3, 2).unwrap();
        assert_eq!(m.data, b"abc");
        assert_eq!(m.out_flags, 32);
        let m = receive(&b, 8, 0).unwrap();
        assert_eq!(m.data, b"abcdef");
        assert_eq!(receive(&b, 8, 0).unwrap().data, b"ghi");
    }
}
#[test]
fn stream_peek_does_not_consume() {
    let (a, b) = pair(1);
    send(&a, b"abcdef").unwrap();
    assert_eq!(receive(&b, 3, 2).unwrap().data, b"abc");
    assert_eq!(receive(&b, 8, 0).unwrap().data, b"abcdef");
}
#[test]
fn empty_datagrams_are_readable_and_consumed() {
    let (a, b) = pair(2);
    assert_eq!(send(&a, b""), Ok(0));
    assert_eq!(b.ops().poll(&b) & 1, 1);
    assert_eq!(receive(&b, 8, 0).unwrap().data, b"");
    assert_eq!(b.ops().poll(&b) & 1, 0);
}
#[test]
fn named_connect_accept_and_credentials() {
    let l = sk(1);
    let name = SockAddr::Unix(UnixName::Abstract(b"proto-listen".to_vec()));
    l.ops().bind(&l, &name).unwrap();
    l.ops().listen(&l, 0).unwrap();
    let c = sk(1);
    c.ops().connect(&c, &name, true).unwrap();
    let (s, from) = l.ops().accept(&l, true).unwrap();
    assert_eq!(from, SockAddr::Unix(UnixName::Unnamed));
    assert_eq!(c.ops().peer_name(&c).unwrap(), name);
    assert_eq!(s.ops().local_name(&s), name);
    send(&c, b"ok").unwrap();
    assert_eq!(receive(&s, 4, 0).unwrap().data, b"ok");
    let mut out = OptBuf {
        bytes: [0; 16],
        len: 0,
    };
    s.ops().getsockopt(&s, 1, 17, &mut out).unwrap();
    assert_eq!(out.len, 12);
    assert_eq!(u32::from_ne_bytes(out.bytes[..4].try_into().unwrap()), 1);
}
#[test]
fn refusal_and_protocol_mismatch() {
    let l = sk(1);
    let name = SockAddr::Unix(UnixName::Abstract(b"proto-refuse".to_vec()));
    l.ops().bind(&l, &name).unwrap();
    assert_eq!(sk(1).ops().connect(&sk(1), &name, true), Err(111));
    let d = sk(2);
    let name = SockAddr::Unix(UnixName::Abstract(b"proto-dg".to_vec()));
    d.ops().bind(&d, &name).unwrap();
    let c = sk(1);
    assert_eq!(c.ops().connect(&c, &name, true), Err(91));
}
#[test]
fn queue_full_and_read_frees_space() {
    let (a, b) = pair(1);
    assert_eq!(send(&a, &vec![0; 65536]), Ok(65536));
    assert_eq!(send(&a, b"x"), Err(11));
    receive(&b, 1, 0).unwrap();
    assert_eq!(send(&a, b"x"), Ok(1));
}
#[test]
fn shutdown_half_close_wakes_eof() {
    let (a, b) = pair(1);
    a.ops().shutdown(&a, 1).unwrap();
    assert_eq!(send(&a, b"x"), Err(32));
    assert_eq!(receive(&b, 1, 0).unwrap().data, b"");
    assert_eq!(b.ops().poll(&b) & 0x2001, 0x2001);
    assert_eq!(send(&b, b"y"), Ok(1));
    assert_eq!(receive(&a, 8, 0).unwrap().data, b"y");
}

#[test]
fn stream_reads_and_peeks_coalesce_writes() {
    let (a, b) = pair(1);
    send(&a, b"abc").unwrap();
    send(&a, b"def").unwrap();
    assert_eq!(receive(&b, 6, 2).unwrap().data, b"abcdef");
    assert_eq!(receive(&b, 6, 0).unwrap().data, b"abcdef");
}

#[test]
fn closing_peer_interrupts_a_blocked_full_send() {
    let (a, b) = pair(1);
    send(&a, &vec![0; 65536]).unwrap();
    let sender = a.clone();
    let task = std::thread::spawn(move || {
        sender.ops().sendmsg(
            &sender,
            &mut SendMsg {
                data: b"x",
                dest: None,
                flags: 0,
                nonblock: false,
                rights: Vec::new(),
                cred: caller_ucred(),
            },
        )
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !a.common.waiting.load(std::sync::atomic::Ordering::Acquire) {
        assert!(
            std::time::Instant::now() < deadline,
            "sender did not reach a full-queue wait"
        );
        std::thread::yield_now();
    }
    drop(b);
    assert_eq!(task.join().unwrap(), Err(32));
}

#[test]
fn ancillary_rights_are_peeked_and_transferred_once() {
    let (a, b) = pair(1);
    let file = std::sync::Arc::new(());
    send(&a, b"abc").unwrap();
    let mut msg = SendMsg {
        data: b"F",
        dest: None,
        flags: 0,
        nonblock: true,
        rights: vec![crate::ipc::fd::FileDescriptor(file.clone())],
        cred: caller_ucred(),
    };
    assert_eq!(a.ops().sendmsg(&a, &mut msg), Ok(1));
    assert!(msg.rights.is_empty());
    send(&a, b"xyz").unwrap();
    let peek = receive(&b, 16, 2).unwrap();
    assert_eq!(peek.data, b"abcF");
    assert_eq!(peek.rights.len(), 1);
    drop(peek);
    let got = receive(&b, 16, 0).unwrap();
    assert_eq!(got.data, b"abcF");
    assert_eq!(got.rights.len(), 1);
    assert_eq!(std::sync::Arc::strong_count(&file), 2);
    assert_eq!(receive(&b, 16, 0).unwrap().data, b"xyz");
    drop(got);
    assert_eq!(std::sync::Arc::strong_count(&file), 1);
}
#[test]
fn waitall_collects_a_split_stream() {
    let (a, b) = pair(1);
    let sender = a.clone();
    let task = std::thread::spawn(move || {
        send(&sender, b"abc").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        send(&sender, b"defgh").unwrap();
    });
    let mut msg = RecvMsg {
        cap: 8,
        data: Vec::new(),
        flags: 0x100,
        nonblock: false,
        out_flags: 0,
        from: None,
        rights: Vec::new(),
    };
    assert_eq!(b.ops().recvmsg(&b, &mut msg), Ok(8));
    assert_eq!(msg.data, b"abcdefgh");
    task.join().unwrap();
}
#[test]
fn connected_datagram_reports_peer_closure() {
    let (a, b) = pair(2);
    drop(b);
    assert_eq!(send(&a, b"x"), Err(111));
}
