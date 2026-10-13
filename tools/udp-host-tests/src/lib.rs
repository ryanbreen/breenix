//! Host endpoint checks use the production UDP implementation. Hardware output
//! and timed waiting are outside these checks; the harness only delivers local
//! datagrams and permits nonblocking receive, so it cannot certify guest waits.
extern crate alloc;
use alloc::sync::Arc;

#[path = "../../../kernel/src/log_notice.rs"]
pub mod log_notice;
pub mod serial {
    pub fn write_notices() {
        panic!("no heap notices in endpoint checks")
    }
}
pub mod syscall {
    pub mod errno {
        pub const EAGAIN: i32 = 11;
        pub const EAFNOSUPPORT: i32 = 97;
        pub const EINVAL: i32 = 22;
        pub const EMSGSIZE: i32 = 90;
        pub const ENOTCONN: i32 = 107;
        pub const EPIPE: i32 = 32;
        pub const ENETUNREACH: i32 = 101;
        pub const ECONNREFUSED: i32 = 111;
        pub const EACCES: i32 = 13;
    }
}
pub mod net;
pub mod socket;

pub fn endpoint() -> Arc<socket::object::Socket> {
    socket::object::Socket::new(
        2,
        2,
        17,
        socket::object::SocketProto::Udp(socket::udp::UdpSock::new().unwrap()),
    )
    .unwrap()
}
pub fn address(port: u16) -> socket::addr::SockAddr {
    socket::addr::SockAddr::Inet {
        addr: [127, 0, 0, 1],
        port,
    }
}
pub fn send(
    sk: &Arc<socket::object::Socket>,
    data: &[u8],
    dest: Option<socket::addr::SockAddr>,
) -> Result<usize, i32> {
    sk.ops().sendmsg(
        sk,
        &mut socket::object::SendMsg {
            data,
            dest,
            flags: 0,
            nonblock: true,
            rights: Vec::new(),
            cred: (),
        },
    )
}
pub fn recv(
    sk: &Arc<socket::object::Socket>,
    cap: usize,
    flags: u32,
) -> (Result<usize, i32>, socket::object::RecvMsg) {
    let mut msg = socket::object::RecvMsg {
        cap,
        data: Vec::new(),
        flags,
        nonblock: true,
        out_flags: 0,
        from: None,
        rights: Vec::new(),
    };
    let result = sk.ops().recvmsg(sk, &mut msg);
    (result, msg)
}
#[cfg(test)]
mod tests {
    use super::*;
    use socket::addr::SockAddr;
    use socket::object::ProtoOps;
    use std::sync::atomic::Ordering;
    #[test]
    fn dup_reference_keeps_binding_until_last_detached_release() {
        let a = endpoint();
        a.ops().bind(&a, &address(45212)).unwrap();
        let duplicate = a.clone();
        drop(a);
        let b = endpoint();
        assert_eq!(b.ops().bind(&b, &address(45212)), Err(98));
        drop(duplicate);
        b.ops().bind(&b, &address(45212)).unwrap();
    }
    #[test]
    fn deferred_release_identity_cannot_remove_a_replacement_binding() {
        let a = endpoint();
        a.ops().bind(&a, &address(45213)).unwrap();
        let mut original = Arc::try_unwrap(a).ok().unwrap();
        let detached = match &mut original.proto {
            socket::object::SocketProto::Udp(udp) => udp.detach(),
            _ => unreachable!(),
        };
        let b = endpoint();
        b.ops().bind(&b, &address(45213)).unwrap();
        detached.release(&original);
        let c = endpoint();
        assert_eq!(c.ops().bind(&c, &address(45213)), Err(98));
    }
    #[test]
    fn maximum_datagram_and_odd_checksum_roundtrip() {
        let a = endpoint();
        let b = endpoint();
        b.ops().bind(&b, &address(45214)).unwrap();
        let data = vec![0x5a; 65507];
        assert_eq!(send(&a, &data, Some(address(45214))), Ok(65507));
        let (n, msg) = recv(&b, 65535, 0);
        assert_eq!(n, Ok(65507));
        assert_eq!(msg.data, data);
        let wire =
            net::udp::try_build_udp_packet([127, 0, 0, 1], [127, 0, 0, 2], 1, 2, b"odd").unwrap();
        assert_eq!(
            net::udp::udp_checksum([127, 0, 0, 1], [127, 0, 0, 2], &wire),
            0
        );
        let mut corrupt = wire.clone();
        corrupt[8] ^= 1;
        assert_ne!(
            net::udp::udp_checksum([127, 0, 0, 1], [127, 0, 0, 2], &corrupt),
            0
        );
    }
    #[test]
    fn bad_inbound_checksum_is_dropped_and_zero_checksum_is_valid() {
        let b = endpoint();
        b.ops().bind(&b, &address(45215)).unwrap();
        let mut wire =
            net::udp::try_build_udp_packet([127, 0, 0, 2], [127, 0, 0, 1], 123, 45215, b"checksum")
                .unwrap();
        wire[8] ^= 1;
        net::ip_output([127, 0, 0, 2], [127, 0, 0, 1], 17, &wire).unwrap();
        assert_eq!(recv(&b, 16, 0).0, Err(11));
        wire[6..8].fill(0);
        net::ip_output([127, 0, 0, 2], [127, 0, 0, 1], 17, &wire).unwrap();
        assert_eq!(recv(&b, 16, 0).0, Ok(8));
    }
    #[test]
    fn connected_reuse_binding_beats_a_later_unconnected_binding() {
        let peer = endpoint();
        peer.ops().bind(&peer, &address(45216)).unwrap();
        let a = endpoint();
        a.common.reuse.store(true, Ordering::Release);
        a.ops().bind(&a, &address(45217)).unwrap();
        a.ops().connect(&a, &address(45216), false).unwrap();
        let b = endpoint();
        b.common.reuse.store(true, Ordering::Release);
        b.ops().bind(&b, &address(45217)).unwrap();
        send(&peer, b"peer", Some(address(45217))).unwrap();
        assert_eq!(recv(&a, 8, 0).1.data, b"peer");
        assert_eq!(recv(&b, 8, 0).0, Err(11));
        let third = endpoint();
        send(&third, b"third", Some(address(45217))).unwrap();
        assert_eq!(recv(&b, 8, 0).1.data, b"third");
    }
    #[test]
    fn per_call_nonblocking_and_out_of_band_rejection() {
        let s = endpoint();
        let mut msg = socket::object::RecvMsg {
            cap: 8,
            data: Vec::new(),
            flags: 0x40,
            nonblock: false,
            out_flags: 0,
            from: None,
            rights: Vec::new(),
        };
        assert_eq!(s.ops().recvmsg(&s, &mut msg), Err(11));
        msg.flags = 1;
        assert_eq!(s.ops().recvmsg(&s, &mut msg), Err(95));
    }
    #[test]
    fn msg_more_coalesces_one_datagram_and_preserves_first_destination() {
        let a = endpoint();
        let b = endpoint();
        let c = endpoint();
        b.ops().bind(&b, &address(45218)).unwrap();
        c.ops().bind(&c, &address(45219)).unwrap();
        let mut msg = socket::object::SendMsg {
            data: b"part",
            dest: Some(address(45218)),
            flags: 0x8000,
            nonblock: true,
            rights: Vec::new(),
            cred: (),
        };
        assert_eq!(a.ops().sendmsg(&a, &mut msg), Ok(4));
        assert_eq!(recv(&b, 16, 0).0, Err(11));
        msg.data = b"two";
        msg.dest = Some(address(45219));
        msg.flags = 0;
        assert_eq!(a.ops().sendmsg(&a, &mut msg), Ok(3));
        assert_eq!(recv(&b, 16, 0).1.data, b"parttwo");
        assert_eq!(recv(&c, 16, 0).0, Err(11));
    }
    #[test]
    fn cork_overflow_discards_unfinished_datagram_without_delivering_fragments() {
        let a = endpoint();
        let b = endpoint();
        b.ops().bind(&b, &address(45220)).unwrap();
        let data = vec![1; 65507];
        let mut msg = socket::object::SendMsg {
            data: &data,
            dest: Some(address(45220)),
            flags: 0x8000,
            nonblock: true,
            rights: Vec::new(),
            cred: (),
        };
        assert_eq!(a.ops().sendmsg(&a, &mut msg), Ok(65507));
        msg.data = b"x";
        msg.flags = 0;
        assert_eq!(a.ops().sendmsg(&a, &mut msg), Err(90));
        assert_eq!(recv(&b, 16, 0).0, Err(11));
        send(&a, b"new", Some(address(45220))).unwrap();
        assert_eq!(recv(&b, 16, 0).1.data, b"new");
    }
    #[test]
    fn dont_route_rejects_a_gateway_destination() {
        let a = endpoint();
        let mut msg = socket::object::SendMsg {
            data: b"x",
            dest: Some(SockAddr::Inet {
                addr: [192, 0, 2, 1],
                port: 53,
            }),
            flags: 4,
            nonblock: true,
            rights: Vec::new(),
            cred: (),
        };
        assert_eq!(a.ops().sendmsg(&a, &mut msg), Err(101));
        msg.dest = Some(SockAddr::Inet {
            addr: [10, 0, 2, 3],
            port: 53,
        });
        assert_eq!(a.ops().sendmsg(&a, &mut msg), Ok(1));
    }
    #[test]
    fn bind_names_ephemeral_validation_and_double_bind() {
        let s = endpoint();
        assert_eq!(
            s.ops().bind(
                &s,
                &SockAddr::Inet {
                    addr: [192, 0, 2, 1],
                    port: 45200
                }
            ),
            Err(99)
        );
        s.ops().bind(&s, &address(0)).unwrap();
        assert!(
            matches!(s.ops().local_name(&s),SockAddr::Inet{addr:[127,0,0,1],port} if port>=49152)
        );
        assert_eq!(s.ops().bind(&s, &address(0)), Err(22));
        assert_eq!(s.ops().peer_name(&s), Err(107));
    }
    #[test]
    fn explicit_bind_is_exclusive_and_close_releases() {
        let a = endpoint();
        a.ops().bind(&a, &address(45201)).unwrap();
        let b = endpoint();
        assert_eq!(b.ops().bind(&b, &address(45201)), Err(98));
        drop(a);
        b.ops().bind(&b, &address(45201)).unwrap();
    }
    #[test]
    fn reuse_requires_all_holders_and_last_bound_receives() {
        let a = endpoint();
        a.common.reuse.store(true, Ordering::Release);
        a.ops().bind(&a, &address(45202)).unwrap();
        let b = endpoint();
        assert_eq!(b.ops().bind(&b, &address(45202)), Err(98));
        b.common.reuse.store(true, Ordering::Release);
        b.ops().bind(&b, &address(45202)).unwrap();
        let sender = endpoint();
        send(&sender, b"last", Some(address(45202))).unwrap();
        assert_eq!(recv(&b, 8, 0).0, Ok(4));
        assert_eq!(recv(&a, 8, 0).0, Err(11));
    }
    #[test]
    fn autobind_source_and_datagram_boundaries() {
        let a = endpoint();
        let b = endpoint();
        b.ops().bind(&b, &address(45203)).unwrap();
        send(&a, b"first", Some(address(45203))).unwrap();
        send(&a, b"next", Some(address(45203))).unwrap();
        let (n, msg) = recv(&b, 8, 0);
        assert_eq!(n, Ok(5));
        assert_eq!(msg.data, b"first");
        assert!(matches!(msg.from,Some(SockAddr::Inet{addr:[127,0,0,1],port}) if port>=49152));
        assert_eq!(recv(&b, 8, 0).1.data, b"next");
    }
    #[test]
    fn truncation_and_peek_do_not_split_datagrams() {
        let a = endpoint();
        let b = endpoint();
        b.ops().bind(&b, &address(45204)).unwrap();
        send(&a, b"abcdef", Some(address(45204))).unwrap();
        let (n, msg) = recv(&b, 2, 2 | 32);
        assert_eq!(n, Ok(6));
        assert_eq!(msg.data, b"ab");
        assert_eq!(msg.out_flags, 32);
        let (n, msg) = recv(&b, 3, 0);
        assert_eq!(n, Ok(3));
        assert_eq!(msg.data, b"abc");
        assert_eq!(msg.out_flags, 32);
        assert_eq!(recv(&b, 8, 0).0, Err(11));
    }
    #[test]
    fn empty_datagram_is_a_message() {
        let a = endpoint();
        let b = endpoint();
        b.ops().bind(&b, &address(45205)).unwrap();
        send(&a, b"", Some(address(45205))).unwrap();
        send(&a, b"x", Some(address(45205))).unwrap();
        assert_eq!(recv(&b, 8, 0).0, Ok(0));
        assert_eq!(recv(&b, 8, 0).0, Ok(1));
        assert_eq!(recv(&b, 8, 0).0, Err(11));
    }
    #[test]
    fn connect_peer_filters_and_unspec_disconnects() {
        let a = endpoint();
        a.ops().bind(&a, &address(45206)).unwrap();
        let b = endpoint();
        b.ops().bind(&b, &address(45207)).unwrap();
        let c = endpoint();
        c.ops().bind(&c, &address(45208)).unwrap();
        a.ops().connect(&a, &address(45207), false).unwrap();
        assert_eq!(a.ops().peer_name(&a), Ok(address(45207)));
        send(&c, b"third", Some(address(45206))).unwrap();
        send(&b, b"peer", Some(address(45206))).unwrap();
        assert_eq!(recv(&a, 8, 0).1.data, b"peer");
        assert_eq!(recv(&a, 8, 0).0, Err(11));
        send(&a, b"conn", None).unwrap();
        assert_eq!(recv(&b, 8, 0).1.data, b"conn");
        a.ops().connect(&a, &SockAddr::Unspec, false).unwrap();
        assert_eq!(a.ops().peer_name(&a), Err(107));
        assert_eq!(send(&a, b"x", None), Err(89));
    }
    #[test]
    fn icmp_error_read_clear_and_poll() {
        let a = endpoint();
        a.ops().connect(&a, &address(45209), false).unwrap();
        assert_eq!(send(&a, b"x", None), Ok(1));
        assert_ne!(a.ops().poll(&a) & 8, 0);
        assert_eq!(recv(&a, 8, 0).0, Err(111));
        assert_eq!(a.ops().poll(&a) & 8, 0);
        assert_eq!(recv(&a, 8, 0).0, Err(11));
    }
    #[test]
    fn oversize_and_broadcast_permission() {
        let a = endpoint();
        assert_eq!(send(&a, &vec![0; 65508], Some(address(45210))), Err(90));
        let broad = SockAddr::Inet {
            addr: [255; 4],
            port: 45210,
        };
        assert_eq!(send(&a, b"x", Some(broad.clone())), Err(13));
        a.common.broadcast.store(true, Ordering::Release);
        assert_eq!(send(&a, b"x", Some(broad)), Ok(1));
    }
    #[test]
    fn full_receive_queue_preserves_accepted_messages_and_bounds_empty_messages() {
        let a = endpoint();
        let b = endpoint();
        b.ops().bind(&b, &address(45211)).unwrap();
        b.common.rcvbuf.store(45, Ordering::Release);
        send(&a, b"first", Some(address(45211))).unwrap();
        send(&a, b"second", Some(address(45211))).unwrap();
        assert_eq!(recv(&b, 16, 0).1.data, b"first");
        assert_eq!(recv(&b, 16, 0).0, Err(11));
        for _ in 0..100 {
            send(&a, b"", Some(address(45211))).unwrap();
        }
        assert_eq!(recv(&b, 0, 0).0, Ok(0));
        assert_eq!(recv(&b, 0, 0).0, Err(11));
    }
}

pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}
pub mod arch_impl {
    pub mod traits {
        pub trait CpuOps {
            fn without_interrupts<R>(f: impl FnOnce() -> R) -> R;
        }
    }
    pub mod aarch64 {
        pub struct Aarch64Cpu;
        impl super::traits::CpuOps for Aarch64Cpu {
            fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
                f()
            }
        }
    }
    pub mod x86_64 {
        pub struct X86Cpu;
        impl super::traits::CpuOps for X86Cpu {
            fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
                f()
            }
        }
    }
}
