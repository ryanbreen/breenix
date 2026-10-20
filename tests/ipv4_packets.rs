//! Exercise the production wire parsers/builders without a guest or a NIC.
extern crate alloc;

#[path = "../kernel/src/log_notice.rs"]
pub mod log_notice;
pub mod serial {
    pub fn write_notices() {
        unreachable!("wire tests never drain notices")
    }
}

#[path = "../kernel/src/syscall/errno.rs"]
pub mod errno;
pub mod syscall {
    pub use crate::errno;
}

#[derive(Debug, PartialEq)]
pub enum TxError {
    NoBufs,
    MsgSize,
}
pub struct Config {
    pub ip_addr: [u8; 4],
    pub gateway: [u8; 4],
}
pub fn config() -> Config {
    Config {
        ip_addr: [10, 0, 2, 15],
        gateway: [10, 0, 2, 2],
    }
}
pub fn is_broadcast_addr(addr: [u8; 4]) -> bool {
    addr == [255; 4] || addr == [10, 0, 2, 255]
}
pub fn ip_output(_: [u8; 4], _: [u8; 4], _: u8, _: u8, _: &[u8]) -> Result<(), TxError> {
    unreachable!("wire tests never transmit")
}
pub mod tcp {
    pub fn handle_tcp(_: &crate::ipv4::Ipv4Packet, _: &[u8]) {
        unreachable!("wire tests never deliver TCP")
    }
}
pub mod udp {
    pub fn udp_icmp_error(_: [u8; 4], _: u16, _: [u8; 4], _: u16, _: i32) -> crate::ipv4::Handled {
        crate::ipv4::Handled::Done
    }
    pub fn handle_udp(_: &crate::ipv4::Ipv4Packet, _: &[u8]) -> crate::ipv4::Handled {
        unreachable!("wire tests never deliver UDP")
    }
}
pub mod dhcp {
    pub fn receive(_: &crate::ipv4::Ipv4Packet) -> bool {
        unreachable!("wire tests never deliver UDP")
    }
}
#[path = "../kernel/src/net/ethernet.rs"]
pub mod ethernet;
#[path = "../kernel/src/net/icmp.rs"]
pub mod icmp;
#[path = "../kernel/src/net/ipv4.rs"]
pub mod ipv4;

use ipv4::{internet_checksum, Ipv4Packet, PROTOCOL_UDP};

#[test]
fn total_length_cannot_underflow_header_or_exceed_input() {
    let mut packet = Ipv4Packet::try_build(
        [127, 0, 0, 1],
        [127, 0, 0, 2],
        PROTOCOL_UDP,
        ipv4::DEFAULT_TTL,
        &[0; 8],
    )
    .unwrap();
    for len in [0u16, 1, 19, 29, 65535] {
        packet[2..4].copy_from_slice(&len.to_be_bytes());
        assert!(
            Ipv4Packet::parse(&packet).is_none(),
            "accepted total_length={len}"
        );
    }
    packet[0] = 0x46;
    packet[2..4].copy_from_slice(&20u16.to_be_bytes());
    assert!(Ipv4Packet::parse(&packet).is_none());
}

#[test]
fn packets_carry_the_ttl_they_are_built_with() {
    let packet =
        Ipv4Packet::try_build([127, 0, 0, 1], [127, 0, 0, 2], PROTOCOL_UDP, 42, &[0; 8]).unwrap();
    let ip = Ipv4Packet::parse(&packet).unwrap();
    assert_eq!(ip.ttl, 42);
    assert_eq!(internet_checksum(ip.header), 0);
    let packet = Ipv4Packet::build([127, 0, 0, 1], [127, 0, 0, 2], PROTOCOL_UDP, &[0; 8]);
    assert_eq!(Ipv4Packet::parse(&packet).unwrap().ttl, ipv4::DEFAULT_TTL);
}

#[test]
fn maximum_ip_length_roundtrips_and_oversize_is_refused() {
    let payload = vec![0x5a; 65515];
    let packet = Ipv4Packet::try_build(
        [127, 0, 0, 1],
        [127, 0, 0, 2],
        PROTOCOL_UDP,
        ipv4::DEFAULT_TTL,
        &payload,
    )
    .unwrap();
    let ip = Ipv4Packet::parse(&packet).unwrap();
    assert_eq!(ip.total_length, 65535);
    assert_eq!(ip.payload, payload);
    assert_eq!(internet_checksum(ip.header), 0);
    assert_eq!(
        Ipv4Packet::try_build(
            ip.src_ip,
            ip.dst_ip,
            PROTOCOL_UDP,
            ipv4::DEFAULT_TTL,
            &vec![0; 65516]
        ),
        Err(TxError::MsgSize)
    );
    assert!(Ipv4Packet::build(ip.src_ip, ip.dst_ip, PROTOCOL_UDP, &vec![0; 65516]).is_empty());
}

#[test]
fn truncated_icmp_quote_identifies_original_flow_and_rejects_malformed_headers() {
    let mut udp = vec![0; 100];
    udp[..2].copy_from_slice(&49152u16.to_be_bytes());
    udp[2..4].copy_from_slice(&53u16.to_be_bytes());
    udp[4..6].copy_from_slice(&100u16.to_be_bytes());
    let ip = Ipv4Packet::try_build(
        [127, 0, 0, 2],
        [127, 0, 0, 1],
        PROTOCOL_UDP,
        ipv4::DEFAULT_TTL,
        &udp,
    )
    .unwrap();
    let mut error = vec![3, 3, 0, 0, 0, 0, 0, 0];
    error.extend_from_slice(&ip[..28]);
    let parsed = icmp::IcmpPacket::parse(&error).unwrap();
    assert_eq!(
        icmp::udp_error_quote(&parsed),
        Some(([127, 0, 0, 2], 49152, [127, 0, 0, 1], 53, 111))
    );
    for len in 0..36 {
        if let Some(parsed) = icmp::IcmpPacket::parse(&error[..len]) {
            assert!(icmp::udp_error_quote(&parsed).is_none());
        }
    }
    error[8] = 0x4f;
    assert!(icmp::udp_error_quote(&icmp::IcmpPacket::parse(&error).unwrap()).is_none());
}

pub mod time {
    pub fn get_ticks() -> u64 {
        0
    }
}
pub fn is_broadcast_addr_in(_: &Config, addr: [u8; 4]) -> bool {
    is_broadcast_addr(addr)
}

#[test]
fn icmp_quote_uses_linux_fatal_codes_without_quoted_checksum_validation() {
    let mut udp = [0; 8];
    udp[..2].copy_from_slice(&49152u16.to_be_bytes());
    udp[2..4].copy_from_slice(&53u16.to_be_bytes());
    udp[4..6].copy_from_slice(&8u16.to_be_bytes());
    let ip = Ipv4Packet::try_build(
        [127, 0, 0, 1],
        [127, 0, 0, 2],
        PROTOCOL_UDP,
        ipv4::DEFAULT_TTL,
        &udp,
    )
    .unwrap();
    let mut error = vec![3, 3, 0, 0, 0, 0, 0, 0];
    error.extend_from_slice(&ip);
    error[18] ^= 1; // A quoted IP checksum is not validated by Linux.
    for (code, expected) in [
        (0, None),
        (1, None),
        (2, Some(92)),
        (3, Some(111)),
        (4, Some(90)),
        (5, None),
        (6, Some(101)),
        (7, Some(112)),
        (8, Some(64)),
        (9, Some(101)),
        (10, Some(113)),
        (11, None),
        (12, None),
        (13, Some(113)),
        (14, Some(113)),
        (15, Some(113)),
    ] {
        error[1] = code;
        assert_eq!(
            icmp::udp_error_quote(&icmp::IcmpPacket::parse(&error).unwrap()).map(|quote| quote.4),
            expected
        );
    }
}

#[test]
fn ethernet_build_preserves_header_and_minimum_padding() {
    let bytes = ethernet::EthernetFrame::build(&[1; 6], &[2; 6], ethernet::ETHERTYPE_IPV4, &[3; 8])
        .unwrap();
    assert_eq!(bytes.len(), ethernet::ETHERNET_MIN_SIZE);
    let frame = ethernet::EthernetFrame::parse(&bytes).unwrap();
    assert_eq!(frame.src_mac, [1; 6]);
    assert_eq!(frame.dst_mac, [2; 6]);
    assert_eq!(frame.ethertype, ethernet::ETHERTYPE_IPV4);
    assert_eq!(&frame.payload[..8], &[3; 8]);
    assert!(frame.payload[8..].iter().all(|b| *b == 0));
}
#[test]
fn broadcast_ipv4_is_not_dispatched_to_tcp_or_icmp() {
    for protocol in [ipv4::PROTOCOL_ICMP, ipv4::PROTOCOL_TCP] {
        let payload = icmp::IcmpPacket::echo_request(1, 1, b"broadcast");
        let packet = Ipv4Packet::try_build(
            [10, 0, 2, 2],
            [255; 4],
            protocol,
            ipv4::DEFAULT_TTL,
            &payload,
        )
        .unwrap();
        let frame = ethernet::EthernetFrame {
            src_mac: [1; 6],
            dst_mac: [255; 6],
            ethertype: ethernet::ETHERTYPE_IPV4,
            payload: &packet,
        };
        // Harness protocol output/dispatch panics if either forbidden path runs.
        ipv4::handle_ipv4(&frame, &Ipv4Packet::parse(&packet).unwrap());
    }
}
