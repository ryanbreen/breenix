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
    assert_eq!(receive(&dup, 1, 0).unwrap().data, b"x");
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
    assert_eq!(s.ops().try_local_name(&s), Ok(name.clone()));
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
        rights: vec![crate::ipc::fd::FileDescriptor {
            kind: crate::ipc::fd::FdKind::File(file.clone()),
        }],
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

#[test]
fn unconnected_errors_and_poll_match_linux() {
    for kind in [1, 5] {
        let a = sk(kind);
        assert_eq!(send(&a, b"x"), Err(107));
        assert_eq!(a.ops().poll(&a) & 0x14, 0x14);
        let mut msg = SendMsg {
            data: b"x",
            dest: Some(SockAddr::Unix(UnixName::Unnamed)),
            flags: 0,
            nonblock: true,
            rights: Vec::new(),
            cred: caller_ucred(),
        };
        assert_eq!(a.ops().sendmsg(&a, &mut msg), Err(95));
    }
    let d = sk(2);
    assert!(matches!(d.ops().accept(&d, true), Err(95)));
}
#[test]
fn read_shutdown_preserves_queued_bytes_and_rejects_new_writes() {
    for kind in [1, 2, 5] {
        let (a, b) = pair(kind);
        send(&a, b"before").unwrap();
        b.ops().shutdown(&b, 0).unwrap();
        assert_eq!(send(&a, b"after"), Err(32));
        assert_eq!(receive(&b, 64, 0).unwrap().data, b"before");
        assert_eq!(receive(&b, 64, 0).unwrap().data, b"");
    }
}
#[test]
fn close_with_unread_bytes_resets_peer_then_returns_eof() {
    let (a, b) = pair(1);
    send(&a, b"unread").unwrap();
    drop(b);
    assert!(matches!(receive(&a, 64, 0), Err(104)));
    assert_eq!(receive(&a, 64, 0).unwrap().data, b"");
}
#[test]
fn listener_close_resets_pending_connections() {
    let l = sk(1);
    let name = SockAddr::Unix(UnixName::Abstract(b"reset-pending".to_vec()));
    l.ops().bind(&l, &name).unwrap();
    l.ops().listen(&l, 1).unwrap();
    let c = sk(1);
    c.ops().connect(&c, &name, true).unwrap();
    drop(l);
    assert!(matches!(receive(&c, 1, 0), Err(104)));
}
#[test]
fn datagram_nonblocking_sender_gets_space_and_close_notifications() {
    let (a, b) = pair(2);
    send(&a, &vec![0; 65504]).unwrap();
    send(&a, &vec![0; 32]).unwrap();
    assert_eq!(send(&a, b"x"), Err(11));
    let before = a.common.seq.load(std::sync::atomic::Ordering::Acquire);
    receive(&b, 65536, 0).unwrap();
    assert!(a.common.seq.load(std::sync::atomic::Ordering::Acquire) > before);
    assert_eq!(a.ops().poll(&a) & 4, 4);
    drop(b);
    assert_eq!(a.ops().poll(&a) & 12, 12);
}
#[test]
fn blocking_stream_send_finishes_after_reader_frees_space() {
    let (a, b) = pair(1);
    let sender = a.clone();
    let task = std::thread::spawn(move || {
        sender.ops().sendmsg(
            &sender,
            &mut SendMsg {
                data: &vec![42; 140000],
                dest: None,
                flags: 0,
                nonblock: false,
                rights: Vec::new(),
                cred: caller_ucred(),
            },
        )
    });
    let mut count = 0;
    while count < 140000 {
        let mut msg = RecvMsg {
            cap: 40000,
            data: Vec::new(),
            flags: 0,
            nonblock: false,
            out_flags: 0,
            from: None,
            rights: Vec::new(),
        };
        b.ops().recvmsg(&b, &mut msg).unwrap();
        assert!(msg.data.iter().all(|b| *b == 42));
        count += msg.data.len();
    }
    assert_eq!(task.join().unwrap(), Ok(140000));
}
#[test]
fn self_connect_and_competing_connects_do_not_overwrite_peer() {
    let l = sk(1);
    let name = SockAddr::Unix(UnixName::Abstract(b"self-connect".to_vec()));
    l.ops().bind(&l, &name).unwrap();
    l.ops().listen(&l, 8).unwrap();
    assert_eq!(l.ops().connect(&l, &name, true), Err(22));
    let c = sk(1);
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let c = c.clone();
        let name = name.clone();
        tasks.push(std::thread::spawn(move || c.ops().connect(&c, &name, true)));
    }
    let mut results: Vec<_> = tasks.into_iter().map(|t| t.join().unwrap()).collect();
    results.sort();
    assert_eq!(results, vec![Ok(()), Err(106)]);
    assert!(l.ops().accept(&l, true).is_ok());
    assert!(matches!(l.ops().accept(&l, true), Err(11)));
}
#[test]
fn datagram_connect_rejects_receiver_connected_elsewhere() {
    let (a, b) = pair(2);
    let name = SockAddr::Unix(UnixName::Abstract(b"dgram-exclusive".to_vec()));
    b.ops().bind(&b, &name).unwrap();
    let c = sk(2);
    assert_eq!(c.ops().connect(&c, &name, true), Err(1));
    assert_eq!(send(&a, b"ok"), Ok(2));
}
#[test]
fn credentials_and_unnamed_datagram_source_match_linux() {
    let l = sk(1);
    let name = SockAddr::Unix(UnixName::Abstract(b"listen-cred".to_vec()));
    l.ops().bind(&l, &name).unwrap();
    l.ops().listen(&l, 1).unwrap();
    let mut out = OptBuf {
        bytes: [0; 16],
        len: 0,
    };
    l.ops().getsockopt(&l, 1, 17, &mut out).unwrap();
    assert_eq!(u32::from_ne_bytes(out.bytes[..4].try_into().unwrap()), 1);
    let (a, b) = pair(2);
    send(&a, b"x").unwrap();
    assert_eq!(receive(&b, 1, 0).unwrap().from, None);
}
#[test]
fn rights_graph_rejects_depth_growth_from_either_end() {
    use crate::ipc::fd::{FdKind, FileDescriptor};
    use crate::unix_msg::UnixMsg;
    let nodes: Vec<_> = (0..6).map(|_| sk(2)).collect();
    let mut messages = Vec::new();
    for i in 0..4 {
        let mut m = UnixMsg::new(
            b"x",
            vec![FileDescriptor {
                kind: FdKind::Socket(nodes[i + 1].clone()),
            }],
            caller_ucred(),
            UnixName::Unnamed,
        )
        .unwrap();
        m.register_rights(std::sync::Arc::as_ptr(&nodes[i]) as usize)
            .unwrap();
        messages.push(m);
    }
    let mut m = UnixMsg::new(
        b"x",
        vec![FileDescriptor {
            kind: FdKind::Socket(nodes[5].clone()),
        }],
        caller_ucred(),
        UnixName::Unnamed,
    )
    .unwrap();
    assert_eq!(
        m.register_rights(std::sync::Arc::as_ptr(&nodes[4]) as usize),
        Err(109)
    );
    drop(messages);
    m.register_rights(std::sync::Arc::as_ptr(&nodes[4]) as usize)
        .unwrap();
}

#[test]
fn failed_receive_delivery_keeps_bytes_and_rights_queued() {
    for kind in [1, 2, 5] {
        let (a, b) = pair(kind);
        let file = std::sync::Arc::new(());
        a.ops()
            .sendmsg(
                &a,
                &mut SendMsg {
                    data: b"preserved",
                    dest: None,
                    flags: 0,
                    nonblock: true,
                    rights: vec![crate::ipc::fd::FileDescriptor {
                        kind: crate::ipc::fd::FdKind::File(file.clone()),
                    }],
                    cred: caller_ucred(),
                },
            )
            .unwrap();
        let mut msg = RecvMsg {
            cap: 64,
            data: Vec::new(),
            flags: 0,
            nonblock: true,
            out_flags: 0,
            from: None,
            rights: Vec::new(),
        };
        assert_eq!(
            b.ops().recvmsg_with(&b, &mut msg, &mut |snapshot| {
                assert_eq!(snapshot.data, b"preserved");
                assert_eq!(snapshot.rights.len(), 1);
                Err(14)
            }),
            Err(14)
        );
        assert_eq!(std::sync::Arc::strong_count(&file), 2);
        let got = receive(&b, 64, 0).unwrap();
        assert_eq!(got.data, b"preserved");
        assert_eq!(got.rights.len(), 1);
        drop(got);
        assert_eq!(std::sync::Arc::strong_count(&file), 1);
    }
}
