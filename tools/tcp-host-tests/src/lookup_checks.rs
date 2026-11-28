//! Fixed-clock checks of the TCP demux and port ownership paths.

use crate::errno::{EADDRINUSE, ENOMEM};
use crate::harness::{self, Flow, PEER_IP, PEER_PORT};
use crate::net::tcp::lookup_probe::{self, Fixture, Index, FAIL, VISITS};
use crate::net::tcp::*;
use crate::net::LOCAL_IP;
use crate::process::process::ProcessId;

use crate::random::siphash as fixture_hash;

fn visits(index: Index, f: impl FnOnce()) -> usize {
    VISITS.set([0; 3]);
    f();
    VISITS.get()[index as usize]
}

fn fail(index: Index, f: impl FnOnce()) {
    FAIL.set(Some(index));
    f();
    assert_eq!(FAIL.take(), None, "the requested reservation was reached");
}

fn active(local_port: u16, remote_port: u16) -> Result<ConnectionId, i32> {
    tcp_connect(
        [0; 4],
        local_port,
        PEER_IP,
        remote_port,
        ProcessId(1),
        64,
        DEFAULT_RCVBUF,
    )
}

fn listen(addr: [u8; 4], port: u16) -> Result<ListenerId, i32> {
    tcp_listen(addr, port, 4, ProcessId(1), false, 64, DEFAULT_RCVBUF, None)
}

fn deliver(dst: [u8; 4], port: u16, peer_port: u16, seq: u32, ack: u32, flags: TcpFlags) {
    let mut wire = [0; 20];
    wire[..2].copy_from_slice(&peer_port.to_be_bytes());
    wire[2..4].copy_from_slice(&port.to_be_bytes());
    wire[4..8].copy_from_slice(&seq.to_be_bytes());
    wire[8..12].copy_from_slice(&ack.to_be_bytes());
    wire[12] = 5 << 4;
    wire[13] = flags.to_byte();
    wire[14..16].copy_from_slice(&u16::MAX.to_be_bytes());
    let checksum = tcp_checksum(PEER_IP, dst, &wire);
    wire[16..18].copy_from_slice(&checksum.to_be_bytes());
    let ip = crate::net::ipv4::Ipv4Packet {
        src_ip: PEER_IP,
        dst_ip: dst,
        payload: &wire,
    };
    handle_tcp(&ip, &wire);
}

fn tuple_hash(local_port: u16, remote_port: u16) -> u64 {
    let mut key = [0; 12];
    key[..4].copy_from_slice(&LOCAL_IP);
    key[4..6].copy_from_slice(&local_port.to_be_bytes());
    key[6..10].copy_from_slice(&PEER_IP);
    key[10..].copy_from_slice(&remote_port.to_be_bytes());
    fixture_hash::hash(&[0; 16], &key)
}

fn port_hash(port: u16) -> u64 {
    fixture_hash::hash(&[0; 16], &port.to_be_bytes())
}

fn listener_hash(ip: [u8; 4], port: u16) -> u64 {
    let mut key = [0; 6];
    key[..4].copy_from_slice(&ip);
    key[4..].copy_from_slice(&port.to_be_bytes());
    fixture_hash::hash(&[0; 16], &key)
}

#[test]
fn indexed_demux_and_bind_consult_only_their_buckets() {
    let _clock = harness::lock(1000);
    let _fixture = Fixture::new();
    let target = 15000;
    let bind_port = 15001;
    let listen_port = 15002;
    // Unrelated entries stay outside the selected buckets at every size.
    for port in (16000..60000)
        .filter(|&port| {
            tuple_hash(port, PEER_PORT) & 7 != tuple_hash(target, PEER_PORT) & 7
                && port_hash(port) & 7 != port_hash(bind_port) & 7
                && port_hash(port + 1000) & 7 != port_hash(bind_port) & 7
                && listener_hash(LOCAL_IP, port) & 7 != listener_hash(LOCAL_IP, listen_port) & 7
                && listener_hash(LOCAL_IP, port) & 7 != listener_hash([0; 4], listen_port) & 7
        })
        .take(96)
    {
        active(port, PEER_PORT).unwrap();
        tcp_bind_port(LOCAL_IP, port + 1000, false).unwrap();
        listen(LOCAL_IP, port).unwrap();
    }
    let mut flow = Flow::connect(target);
    tcp_bind_port(LOCAL_IP, bind_port, false).unwrap();
    let exact = listen(LOCAL_IP, listen_port).unwrap();
    // Public listen rejects overlap. Seed a wildcard only to exercise demux
    // precedence with both candidates present.
    let wildcard = lookup_probe::seed_wildcard(listen_port, ProcessId(1));

    // Same buckets, different complete keys, inserted after the wanted entry.
    let peer = (PEER_PORT + 1..u16::MAX)
        .find(|&port| tuple_hash(target, port) & 255 == tuple_hash(target, PEER_PORT) & 255)
        .unwrap();
    let other = active(target, peer).unwrap();
    // The bind index carries all three kinds of owner. The 96 unrelated
    // endpoints of each kind grow it to 512 buckets in this fixture.
    let bind_collision = (60001..u16::MAX)
        .find(|&port| port_hash(port) & 511 == port_hash(bind_port) & 511)
        .unwrap();
    tcp_bind_port(LOCAL_IP, bind_collision, false).unwrap();
    let listener_collision = (60001..u16::MAX)
        .find(|&port| {
            listener_hash(LOCAL_IP, port) & 255 == listener_hash(LOCAL_IP, listen_port) & 255
                && port_hash(port) & 511 != port_hash(bind_port) & 511
        })
        .unwrap();
    listen(LOCAL_IP, listener_collision).unwrap();

    let demux = visits(Index::Established, || flow.send(9));
    assert_eq!(flow.read(9), 9);
    assert_eq!(tcp_get_state(&other), Some(TcpState::SynSent));
    let bind = visits(Index::Bind, || {
        assert_eq!(tcp_bind_port(LOCAL_IP, bind_port, false), Err(EADDRINUSE))
    });
    harness::refuse_output(true);
    let exact_visits = visits(Index::Listener, || {
        deliver(LOCAL_IP, listen_port, PEER_PORT, 19, 0, TcpFlags::syn());
    });
    let alt = [127, 0, 0, 2];
    deliver(alt, listen_port, PEER_PORT, 19, 0, TcpFlags::syn());
    for (local_ip, wanted) in [(LOCAL_IP, exact), (alt, wildcard)] {
        let id = ConnectionId {
            local_ip,
            local_port: listen_port,
            remote_ip: PEER_IP,
            remote_port: PEER_PORT,
        };
        assert_eq!(lookup_probe::parent(&id), Some(wanted));
    }
    tcp_close_connection(&other, true, false).unwrap();
    assert_eq!(tcp_get_state(&other), None);
    assert_eq!(
        tcp_get_state(&ConnectionId {
            local_ip: alt,
            local_port: listen_port,
            remote_ip: PEER_IP,
            remote_port: PEER_PORT
        }),
        Some(TcpState::SynReceived)
    );
    flow.send(2);
    assert_eq!(flow.read(2), 2);
    assert_eq!(
        (demux, bind, exact_visits),
        (2, 2, 2),
        "candidate visits in the three affected buckets"
    );
}

#[test]
fn reservation_failure_keeps_all_memberships() {
    let _clock = harness::lock(1000);
    let _fixture = Fixture::new();
    let held = Flow::connect(15010);
    let port = 15011;
    let listener = listen(LOCAL_IP, 15012).unwrap();
    tcp_bind_port([0; 4], port, false).unwrap();
    let before = proc_net_tcp().unwrap();
    let assert_unchanged = || {
        assert_eq!(proc_net_tcp().unwrap(), before);
        assert_eq!(tcp_get_state(&held.id), Some(TcpState::Established));
        assert_eq!(tcp_bind_port(LOCAL_IP, port, false), Err(EADDRINUSE));
        assert_eq!(tcp_bind_port(LOCAL_IP, 15012, true), Err(EADDRINUSE));
        assert!(lookup_probe::has_listener(listener));
    };

    fail(Index::Established, || {
        assert_eq!(active(port, PEER_PORT), Err(ENOMEM))
    });
    assert_unchanged();
    fail(Index::Listener, || {
        assert_eq!(listen([0; 4], port), Err(ENOMEM))
    });
    assert_unchanged();
    fail(Index::Bind, || {
        assert_eq!(active(port, PEER_PORT), Err(ENOMEM))
    });
    assert_unchanged();
    fail(Index::Bind, || {
        assert_eq!(listen([0; 4], port), Err(ENOMEM))
    });
    assert_unchanged();

    let child = ConnectionId {
        local_ip: LOCAL_IP,
        local_port: 15012,
        remote_ip: PEER_IP,
        remote_port: PEER_PORT,
    };
    fail(Index::Bind, || {
        deliver(LOCAL_IP, 15012, PEER_PORT, 19, 0, TcpFlags::syn())
    });
    assert_eq!(tcp_get_state(&child), None);
    assert!(harness::take_sent(15012).is_empty());
    assert_unchanged();
    deliver(LOCAL_IP, 15012, PEER_PORT, 19, 0, TcpFlags::syn());
    assert_eq!(tcp_get_state(&child), Some(TcpState::SynReceived));
    tcp_unlisten(listener);
    assert_eq!(tcp_get_state(&child), None);
    assert_eq!(tcp_bind_port(LOCAL_IP, 15012, false), Ok(()));
}

#[test]
fn simultaneous_close_holds_port_through_time_wait() {
    let _clock = harness::lock(1000);
    let _fixture = Fixture::new();
    let flow = Flow::connect(15020);
    let port = flow.id.local_port;
    tcp_close_connection(&flow.id, false, true).unwrap();
    assert_eq!(tcp_get_state(&flow.id), Some(TcpState::FinWait1));
    deliver(
        LOCAL_IP,
        port,
        PEER_PORT,
        flow.peer_next(),
        flow.data_start,
        TcpFlags::fin_ack(),
    );
    assert_eq!(tcp_get_state(&flow.id), Some(TcpState::Closing));
    deliver(
        LOCAL_IP,
        port,
        PEER_PORT,
        flow.peer_next().wrapping_add(1),
        flow.data_start.wrapping_add(1),
        TcpFlags::ack(),
    );
    assert_eq!(tcp_get_state(&flow.id), Some(TcpState::TimeWait));
    assert_eq!(tcp_bind_port([0; 4], port, false), Err(EADDRINUSE));
    assert_eq!(tcp_bind_port([0; 4], port, true), Ok(()));
    tcp_unbind_port([0; 4], port);
    harness::advance(60_000);
    assert_eq!(tcp_bind_port([0; 4], port, false), Ok(()));
    run_timers();
    assert_eq!(tcp_get_state(&flow.id), None);
    let id = active(port, PEER_PORT).unwrap();
    assert_eq!(id, flow.id);
    assert_eq!(tcp_get_state(&id), Some(TcpState::SynSent));
    tcp_close_connection(&id, true, false).unwrap();
    assert_eq!(tcp_get_state(&id), None);
    assert_eq!(tcp_bind_port([0; 4], port, false), Ok(()));
}
