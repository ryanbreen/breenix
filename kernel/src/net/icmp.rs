//! ICMP (Internet Control Message Protocol) implementation
//!
//! Implements ICMP echo (ping) request and reply (RFC 792).

use alloc::vec::Vec;

use super::ipv4::{self, Ipv4Packet, PROTOCOL_ICMP};

/// ICMP type: Echo Reply
pub const ICMP_ECHO_REPLY: u8 = 0;

/// ICMP type: Destination Unreachable
pub const ICMP_DEST_UNREACHABLE: u8 = 3;

/// ICMP type: Echo Request
pub const ICMP_ECHO_REQUEST: u8 = 8;

/// ICMP header size
pub const ICMP_HEADER_SIZE: usize = 8;

/// Parsed ICMP packet
#[derive(Debug)]
#[allow(dead_code)] // Protocol fields - all are part of ICMP specification
pub struct IcmpPacket<'a> {
    /// ICMP type
    pub icmp_type: u8,
    /// ICMP code
    pub code: u8,
    /// Checksum
    pub checksum: u16,
    /// Identifier (for echo request/reply)
    pub identifier: u16,
    /// Sequence number (for echo request/reply)
    pub sequence: u16,
    /// Payload data
    pub payload: &'a [u8],
}

impl<'a> IcmpPacket<'a> {
    /// Parse an ICMP packet from raw bytes
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < ICMP_HEADER_SIZE {
            return None;
        }

        let icmp_type = data[0];
        let code = data[1];
        let checksum = u16::from_be_bytes([data[2], data[3]]);
        let identifier = u16::from_be_bytes([data[4], data[5]]);
        let sequence = u16::from_be_bytes([data[6], data[7]]);
        let payload = &data[ICMP_HEADER_SIZE..];

        Some(IcmpPacket {
            icmp_type,
            code,
            checksum,
            identifier,
            sequence,
            payload,
        })
    }

    /// Build an ICMP echo request packet
    pub fn echo_request(identifier: u16, sequence: u16, payload: &[u8]) -> Vec<u8> {
        Self::build_echo(ICMP_ECHO_REQUEST, identifier, sequence, payload)
    }

    /// Build an ICMP echo reply packet
    pub fn echo_reply(identifier: u16, sequence: u16, payload: &[u8]) -> Vec<u8> {
        Self::build_echo(ICMP_ECHO_REPLY, identifier, sequence, payload)
    }

    /// Build an ICMP echo packet (request or reply)
    fn build_echo(icmp_type: u8, identifier: u16, sequence: u16, payload: &[u8]) -> Vec<u8> {
        let mut packet = Vec::with_capacity(ICMP_HEADER_SIZE + payload.len());

        // Type
        packet.push(icmp_type);
        // Code (0 for echo)
        packet.push(0);
        // Checksum (placeholder)
        packet.extend_from_slice(&[0, 0]);
        // Identifier
        packet.extend_from_slice(&identifier.to_be_bytes());
        // Sequence
        packet.extend_from_slice(&sequence.to_be_bytes());
        // Payload
        packet.extend_from_slice(payload);

        // Calculate and insert checksum
        let checksum = ipv4::internet_checksum(&packet);
        packet[2] = (checksum >> 8) as u8;
        packet[3] = (checksum & 0xFF) as u8;

        packet
    }
}

/// Handle an incoming ICMP packet
pub fn handle_icmp(ip: &Ipv4Packet, icmp: &IcmpPacket) {
    if ipv4::internet_checksum(ip.payload) != 0 {
        return;
    }
    match icmp.icmp_type {
        ICMP_ECHO_REQUEST => {
            let mut reply = Vec::new();
            if reply
                .try_reserve_exact(ICMP_HEADER_SIZE + icmp.payload.len())
                .is_err()
            {
                return;
            }
            reply.extend_from_slice(&[ICMP_ECHO_REPLY, 0, 0, 0]);
            reply.extend_from_slice(&icmp.identifier.to_be_bytes());
            reply.extend_from_slice(&icmp.sequence.to_be_bytes());
            reply.extend_from_slice(icmp.payload);
            let checksum = ipv4::internet_checksum(&reply);
            reply[2..4].copy_from_slice(&checksum.to_be_bytes());
            let _ = super::ip_output(
                ip.dst_ip,
                ip.src_ip,
                PROTOCOL_ICMP,
                ipv4::DEFAULT_TTL,
                &reply,
            );
        }
        ICMP_ECHO_REPLY => {
            // Preserve the networking boot stage without taking the logger's
            // locks in NetRx. The idle/thread writer emits this actual event.
            if ip.src_ip == super::config().gateway && icmp.identifier == 1 && icmp.sequence == 1 {
                static ECHO_REPLY_NOTICE: crate::log_notice::Node = crate::log_notice::Node::new();
                ECHO_REPLY_NOTICE.post_again("NET: ICMP echo reply received from gateway");
            }
        }
        ICMP_DEST_UNREACHABLE => {
            if let Some((src, src_port, dst, dst_port, errno)) = udp_error_quote(icmp) {
                super::udp::udp_icmp_error(src, src_port, dst, dst_port, errno);
            }
        }
        _ => {}
    }
}

/// Send RFC 792 type 3/code 3 with the original IP header and first eight L4 bytes.
/// Broadcasts, multicast, noninitial fragments and invalid sources never draw errors.
pub fn send_port_unreachable(ip: &Ipv4Packet, l4: &[u8]) {
    if l4.len() < 8
        || ip.flags_fragment & 0x1fff != 0
        || super::is_broadcast_addr(ip.dst_ip)
        || ip.dst_ip[0] >= 224
        || ip.src_ip == [0; 4]
        || super::is_broadcast_addr(ip.src_ip)
        || ip.src_ip[0] >= 224
    {
        return;
    }
    // Bound error output globally to ten packets per second. Linux exempts loopback.
    if ip.src_ip[0] != 127 {
        static NEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let now = crate::time::get_ticks();
        let next = NEXT.load(core::sync::atomic::Ordering::Relaxed);
        if now < next
            || NEXT
                .compare_exchange(
                    next,
                    now.saturating_add(100),
                    core::sync::atomic::Ordering::Relaxed,
                    core::sync::atomic::Ordering::Relaxed,
                )
                .is_err()
        {
            return;
        }
    }
    // IHL is at most 60, so this error packet has a fixed stack bound.
    let mut packet = [0u8; ICMP_HEADER_SIZE + 60 + 8];
    packet[0] = ICMP_DEST_UNREACHABLE;
    packet[1] = 3;
    let len = ICMP_HEADER_SIZE + ip.header.len() + 8;
    packet[ICMP_HEADER_SIZE..ICMP_HEADER_SIZE + ip.header.len()].copy_from_slice(ip.header);
    packet[len - 8..len].copy_from_slice(&l4[..8]);
    let checksum = ipv4::internet_checksum(&packet[..len]);
    packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    let _ = super::ip_output(
        ip.dst_ip,
        ip.src_ip,
        PROTOCOL_ICMP,
        ipv4::DEFAULT_TTL,
        &packet[..len],
    );
}

/// Extract an original UDP flow from an ICMP error. RFC 792 quotations may be
/// shorter than the original IP total length, so a full IPv4 parse is inappropriate.
pub fn udp_error_quote(icmp: &IcmpPacket) -> Option<([u8; 4], u16, [u8; 4], u16, i32)> {
    if icmp.icmp_type != ICMP_DEST_UNREACHABLE {
        return None;
    }
    let data = icmp.payload;
    if data.len() < 20 || data[0] >> 4 != 4 || data[0] & 15 < 5 || data[9] != ipv4::PROTOCOL_UDP {
        return None;
    }
    let header_len = (data[0] as usize & 15) * 4;
    if data.len() < header_len + 8
        || (u16::from_be_bytes([data[2], data[3]]) as usize) < header_len + 8
        || u16::from_be_bytes([data[6], data[7]]) & 0x1fff != 0
    {
        return None;
    }
    let udp_len = u16::from_be_bytes([data[header_len + 4], data[header_len + 5]]) as usize;
    let ip_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if udp_len < 8 || udp_len > ip_len - header_len {
        return None;
    }
    use crate::syscall::errno::*;
    let errno = match icmp.code {
        2 => ENOPROTOOPT,
        3 => ECONNREFUSED,
        4 => EMSGSIZE,
        6 | 9 => ENETUNREACH,
        7 => EHOSTDOWN,
        8 => ENONET,
        10 | 13 | 14 | 15 => EHOSTUNREACH,
        _ => return None, // Linux ignores transient errors without IP_RECVERR.
    };
    Some((
        [data[12], data[13], data[14], data[15]],
        u16::from_be_bytes([data[header_len], data[header_len + 1]]),
        [data[16], data[17], data[18], data[19]],
        u16::from_be_bytes([data[header_len + 2], data[header_len + 3]]),
        errno,
    ))
}
