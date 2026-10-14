//! UDP (User Datagram Protocol) implementation
//!
//! Implements UDP packet parsing and construction (RFC 768).

use alloc::vec::Vec;

use super::ipv4::{internet_checksum, Ipv4Packet};

/// UDP header size
pub const UDP_HEADER_SIZE: usize = 8;

/// Parsed UDP header
#[derive(Debug)]
pub struct UdpHeader {
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
    /// Length (header + data), bounds the received datagram.
    pub length: u16,
    /// Checksum, validated on receive unless zero (IPv4 checksum disabled).
    pub checksum: u16,
}

impl UdpHeader {
    /// Parse a UDP header from raw bytes
    pub fn parse(data: &[u8]) -> Option<(Self, &[u8])> {
        if data.len() < UDP_HEADER_SIZE {
            return None;
        }

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);
        let length = u16::from_be_bytes([data[4], data[5]]);
        let checksum = u16::from_be_bytes([data[6], data[7]]);

        // Validate length
        if (length as usize) < UDP_HEADER_SIZE || (length as usize) > data.len() {
            return None;
        }

        let payload = &data[UDP_HEADER_SIZE..(length as usize)];

        Some((
            UdpHeader {
                src_port,
                dst_port,
                length: length,
                checksum: checksum,
            },
            payload,
        ))
    }
}

/// Legacy builder for packet fixtures that supply IP addresses separately.
/// Zero checksum is valid for IPv4 UDP; refused construction is an empty Vec.
pub fn build_udp_packet(src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    match try_build_udp_packet([0; 4], [0; 4], src_port, dst_port, payload) {
        Ok(mut packet) => {
            packet[6..8].fill(0);
            packet
        }
        Err(_) => Vec::new(),
    }
}

/// Build a whole datagram fallibly, before any user length can narrow to u16.
pub fn try_build_udp_packet(
    src: [u8; 4],
    dst: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Result<Vec<u8>, super::TxError> {
    if payload.len() > 65507 {
        return Err(super::TxError::MsgSize);
    }
    let len = UDP_HEADER_SIZE + payload.len();
    let mut packet = Vec::new();
    packet
        .try_reserve_exact(len)
        .map_err(|_| super::TxError::NoBufs)?;
    packet.extend_from_slice(&src_port.to_be_bytes());
    packet.extend_from_slice(&dst_port.to_be_bytes());
    packet.extend_from_slice(&(len as u16).to_be_bytes());
    packet.extend_from_slice(&[0; 2]);
    packet.extend_from_slice(payload);
    let checksum = udp_checksum(src, dst, &packet);
    // RFC 768 transmits a computed zero as all ones; zero disables checksums.
    packet[6..8].copy_from_slice(&(if checksum == 0 { 0xffff } else { checksum }).to_be_bytes());
    Ok(packet)
}

/// RFC 768 pseudo-header checksum. No receive-path allocation is required.
pub fn udp_checksum(src: [u8; 4], dst: [u8; 4], packet: &[u8]) -> u16 {
    let mut pseudo = [0u8; 12];
    pseudo[..4].copy_from_slice(&src);
    pseudo[4..8].copy_from_slice(&dst);
    pseudo[9] = super::ipv4::PROTOCOL_UDP;
    pseudo[10..12].copy_from_slice(&(packet.len() as u16).to_be_bytes());
    let mut sum = u32::from(!internet_checksum(&pseudo)) + u32::from(!internet_checksum(packet));
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Deliver through Weak socket bindings; no descriptor lookup or PM hold.
pub fn handle_udp(ip: &Ipv4Packet, data: &[u8]) {
    let Some((header, payload)) = UdpHeader::parse(data) else {
        return;
    };
    let l4 = &data[..header.length as usize];
    if header.checksum != 0 && udp_checksum(ip.src_ip, ip.dst_ip, l4) != 0 {
        return;
    }
    if !crate::socket::udp::deliver(
        ip.dst_ip,
        header.dst_port,
        ip.src_ip,
        header.src_port,
        payload,
    ) {
        super::icmp::send_port_unreachable(ip, l4);
    }
}

/// ICMP's original UDP four-tuple identifies the socket whose error is pending.
pub fn udp_icmp_error(src: [u8; 4], src_port: u16, dst: [u8; 4], dst_port: u16, errno: i32) {
    crate::socket::udp::udp_icmp_error(src, src_port, dst, dst_port, errno);
}
