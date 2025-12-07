//! Ethernet frame parsing and construction
//!
//! Implements IEEE 802.3 Ethernet II frame format.

use alloc::vec::Vec;

/// Ethernet frame header size (without VLAN tag)
pub const ETHERNET_HEADER_SIZE: usize = 14;

/// Minimum Ethernet frame size (excluding FCS)
pub const ETHERNET_MIN_SIZE: usize = 60;

/// Maximum Ethernet payload size (MTU)
#[allow(dead_code)] // Standard constant for reference
pub const ETHERNET_MTU: usize = 1500;

/// Broadcast MAC address
pub const BROADCAST_MAC: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// EtherType for IPv4
pub const ETHERTYPE_IPV4: u16 = 0x0800;

/// EtherType for ARP
pub const ETHERTYPE_ARP: u16 = 0x0806;

/// EtherType for IPv6
#[allow(dead_code)]
pub const ETHERTYPE_IPV6: u16 = 0x86DD;

/// Parsed Ethernet frame
#[derive(Debug)]
#[allow(dead_code)] // Protocol fields - all are part of Ethernet frame
pub struct EthernetFrame<'a> {
    /// Destination MAC address
    pub dst_mac: [u8; 6],
    /// Source MAC address
    pub src_mac: [u8; 6],
    /// EtherType field
    pub ethertype: u16,
    /// Frame payload
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Parse an Ethernet frame from raw bytes
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < ETHERNET_HEADER_SIZE {
            return None;
        }

        let dst_mac = [data[0], data[1], data[2], data[3], data[4], data[5]];
        let src_mac = [data[6], data[7], data[8], data[9], data[10], data[11]];
        let ethertype = u16::from_be_bytes([data[12], data[13]]);

        Some(EthernetFrame {
            dst_mac,
            src_mac,
            ethertype,
            payload: &data[ETHERNET_HEADER_SIZE..],
        })
    }

    /// Build an Ethernet frame
    pub fn build(src_mac: &[u8; 6], dst_mac: &[u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(ETHERNET_HEADER_SIZE + payload.len());

        // Destination MAC
        frame.extend_from_slice(dst_mac);
        // Source MAC
        frame.extend_from_slice(src_mac);
        // EtherType
        frame.extend_from_slice(&ethertype.to_be_bytes());
        // Payload
        frame.extend_from_slice(payload);

        // Pad to minimum frame size if needed
        while frame.len() < ETHERNET_MIN_SIZE {
            frame.push(0);
        }

        frame
    }
}

/// Check if a MAC address is broadcast
#[allow(dead_code)]
pub fn is_broadcast(mac: &[u8; 6]) -> bool {
    *mac == BROADCAST_MAC
}

/// Check if a MAC address is multicast
#[allow(dead_code)]
pub fn is_multicast(mac: &[u8; 6]) -> bool {
    mac[0] & 0x01 != 0
}
