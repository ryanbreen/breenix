//! ICMP (Internet Control Message Protocol) implementation
//!
//! Implements ICMP echo (ping) request and reply (RFC 792).

use alloc::vec::Vec;

use super::ethernet::{EthernetFrame, ETHERTYPE_IPV4};
use super::ipv4::{self, Ipv4Packet, PROTOCOL_ICMP};
use crate::drivers::e1000;

/// ICMP type: Echo Reply
pub const ICMP_ECHO_REPLY: u8 = 0;

/// ICMP type: Destination Unreachable
#[allow(dead_code)]
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
pub fn handle_icmp(eth_frame: &EthernetFrame, ip: &Ipv4Packet, icmp: &IcmpPacket) {
    match icmp.icmp_type {
        ICMP_ECHO_REQUEST => {
            log::info!(
                "ICMP: Echo request from {}.{}.{}.{} seq={}",
                ip.src_ip[0], ip.src_ip[1], ip.src_ip[2], ip.src_ip[3],
                icmp.sequence
            );

            // Send echo reply
            send_echo_reply(eth_frame, ip, icmp);
        }
        ICMP_ECHO_REPLY => {
            log::info!(
                "ICMP: Echo reply from {}.{}.{}.{} seq={} (RTT calculation not implemented)",
                ip.src_ip[0], ip.src_ip[1], ip.src_ip[2], ip.src_ip[3],
                icmp.sequence
            );
        }
        ICMP_DEST_UNREACHABLE => {
            log::warn!(
                "ICMP: Destination unreachable from {}.{}.{}.{} code={}",
                ip.src_ip[0], ip.src_ip[1], ip.src_ip[2], ip.src_ip[3],
                icmp.code
            );
        }
        _ => {
            log::debug!("ICMP: Unknown type {} from {}.{}.{}.{}",
                icmp.icmp_type,
                ip.src_ip[0], ip.src_ip[1], ip.src_ip[2], ip.src_ip[3]
            );
        }
    }
}

/// Send an ICMP echo reply
fn send_echo_reply(eth_frame: &EthernetFrame, ip: &Ipv4Packet, icmp: &IcmpPacket) {
    let our_mac = match e1000::mac_address() {
        Some(mac) => mac,
        None => return,
    };

    let config = super::config();

    // Build ICMP reply (same identifier, sequence, and payload as request)
    let icmp_reply = IcmpPacket::echo_reply(icmp.identifier, icmp.sequence, icmp.payload);

    // Build IP packet
    let ip_packet = Ipv4Packet::build(
        config.ip_addr,
        ip.src_ip,
        PROTOCOL_ICMP,
        &icmp_reply,
    );

    // Build Ethernet frame (reply to sender)
    let frame = super::ethernet::EthernetFrame::build(
        &our_mac,
        &eth_frame.src_mac,
        ETHERTYPE_IPV4,
        &ip_packet,
    );

    if let Err(e) = e1000::transmit(&frame) {
        log::warn!("ICMP: Failed to send echo reply: {}", e);
    } else {
        log::debug!("ICMP: Sent echo reply to {}.{}.{}.{}",
            ip.src_ip[0], ip.src_ip[1], ip.src_ip[2], ip.src_ip[3]
        );
    }
}
