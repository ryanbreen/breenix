//! IPv4 packet parsing and construction
//!
//! Implements basic IPv4 packet handling (RFC 791).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use super::ethernet::EthernetFrame;
use super::icmp;

/// IPv4 header minimum size (no options)
pub const IPV4_HEADER_MIN_SIZE: usize = 20;

/// IPv4 protocol number for ICMP
pub const PROTOCOL_ICMP: u8 = 1;

/// IPv4 protocol number for TCP
#[allow(dead_code)]
pub const PROTOCOL_TCP: u8 = 6;

/// IPv4 protocol number for UDP
#[allow(dead_code)]
pub const PROTOCOL_UDP: u8 = 17;

/// Default TTL for outgoing packets
pub const DEFAULT_TTL: u8 = 64;

/// Parsed IPv4 packet
#[derive(Debug)]
#[allow(dead_code)] // Protocol fields - all are part of IPv4 header specification
pub struct Ipv4Packet<'a> {
    /// Version (should be 4)
    pub version: u8,
    /// Header length in 32-bit words
    pub ihl: u8,
    /// Type of service / DSCP
    pub tos: u8,
    /// Total length
    pub total_length: u16,
    /// Identification
    pub identification: u16,
    /// Flags and fragment offset
    pub flags_fragment: u16,
    /// Time to live
    pub ttl: u8,
    /// Protocol
    pub protocol: u8,
    /// Header checksum
    pub checksum: u16,
    /// Source IP address
    pub src_ip: [u8; 4],
    /// Destination IP address
    pub dst_ip: [u8; 4],
    /// Payload (after header)
    pub payload: &'a [u8],
}

impl<'a> Ipv4Packet<'a> {
    /// Parse an IPv4 packet from raw bytes
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < IPV4_HEADER_MIN_SIZE {
            return None;
        }

        let version = (data[0] >> 4) & 0x0F;
        let ihl = data[0] & 0x0F;

        // Validate version
        if version != 4 {
            return None;
        }

        // Validate header length (minimum 5 = 20 bytes)
        if ihl < 5 {
            return None;
        }

        let header_len = (ihl as usize) * 4;
        if data.len() < header_len {
            return None;
        }

        let tos = data[1];
        let total_length = u16::from_be_bytes([data[2], data[3]]);
        let identification = u16::from_be_bytes([data[4], data[5]]);
        let flags_fragment = u16::from_be_bytes([data[6], data[7]]);
        let ttl = data[8];
        let protocol = data[9];
        let checksum = u16::from_be_bytes([data[10], data[11]]);
        let src_ip = [data[12], data[13], data[14], data[15]];
        let dst_ip = [data[16], data[17], data[18], data[19]];

        // Validate total length
        if (total_length as usize) > data.len() {
            return None;
        }

        let payload = &data[header_len..(total_length as usize)];

        Some(Ipv4Packet {
            version,
            ihl,
            tos,
            total_length,
            identification,
            flags_fragment,
            ttl,
            protocol,
            checksum,
            src_ip,
            dst_ip,
            payload,
        })
    }

    /// Build an IPv4 packet
    pub fn build(src_ip: [u8; 4], dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Vec<u8> {
        let total_length = (IPV4_HEADER_MIN_SIZE + payload.len()) as u16;

        let mut packet = Vec::with_capacity(total_length as usize);

        // Version (4) + IHL (5 = 20 bytes)
        packet.push(0x45);
        // TOS
        packet.push(0);
        // Total length
        packet.extend_from_slice(&total_length.to_be_bytes());
        // Identification (use a simple counter)
        static PACKET_ID: AtomicU16 = AtomicU16::new(0);
        let id = PACKET_ID.fetch_add(1, Ordering::Relaxed);
        packet.extend_from_slice(&id.to_be_bytes());
        // Flags (Don't Fragment) + Fragment offset (0)
        packet.extend_from_slice(&0x4000u16.to_be_bytes());
        // TTL
        packet.push(DEFAULT_TTL);
        // Protocol
        packet.push(protocol);
        // Checksum (placeholder - will calculate after)
        packet.extend_from_slice(&[0, 0]);
        // Source IP
        packet.extend_from_slice(&src_ip);
        // Destination IP
        packet.extend_from_slice(&dst_ip);

        // Calculate and insert checksum
        let checksum = internet_checksum(&packet[..IPV4_HEADER_MIN_SIZE]);
        packet[10] = (checksum >> 8) as u8;
        packet[11] = (checksum & 0xFF) as u8;

        // Payload
        packet.extend_from_slice(payload);

        packet
    }
}

/// Handle an incoming IPv4 packet
pub fn handle_ipv4(eth_frame: &EthernetFrame, ip: &Ipv4Packet) {
    let config = super::config();

    // Check if this packet is for us (accept our IP or loopback addresses)
    if ip.dst_ip != config.ip_addr && ip.dst_ip[0] != 127 {
        // Not for us, ignore (we don't do routing)
        return;
    }

    // Verify checksum
    // Note: In a production system, we'd verify the checksum here
    // For now, we trust the NIC's checksum offload

    match ip.protocol {
        PROTOCOL_ICMP => {
            if let Some(icmp_packet) = icmp::IcmpPacket::parse(ip.payload) {
                icmp::handle_icmp(eth_frame, ip, &icmp_packet);
            }
        }
        #[cfg(target_arch = "x86_64")]
        PROTOCOL_TCP => {
            super::tcp::handle_tcp(ip, ip.payload);
        }
        #[cfg(target_arch = "x86_64")]
        PROTOCOL_UDP => {
            super::udp::handle_udp(ip, ip.payload);
        }
        _ => {
            #[cfg(target_arch = "x86_64")]
            log::debug!("IPv4: Unknown protocol {}", ip.protocol);
        }
    }
}

/// Calculate the Internet checksum (RFC 1071)
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Sum 16-bit words
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }

    // Add odd byte if present
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    // Fold 32-bit sum to 16 bits
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // One's complement
    !(sum as u16)
}
