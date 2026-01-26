//! ARP (Address Resolution Protocol) implementation
//!
//! Implements RFC 826 for IPv4-to-Ethernet address resolution.

use alloc::vec::Vec;
use spin::Mutex;

use super::ethernet::{self, EthernetFrame, BROADCAST_MAC, ETHERTYPE_ARP};

// Driver abstraction: use E1000 on x86_64, VirtIO net on ARM64
#[cfg(target_arch = "x86_64")]
use crate::drivers::e1000;
#[cfg(target_arch = "aarch64")]
use crate::drivers::virtio::net_mmio;

// Driver abstraction functions (local to this module)
#[cfg(target_arch = "x86_64")]
fn get_mac_address() -> Option<[u8; 6]> {
    e1000::mac_address()
}

#[cfg(target_arch = "aarch64")]
fn get_mac_address() -> Option<[u8; 6]> {
    net_mmio::mac_address()
}

#[cfg(target_arch = "x86_64")]
fn driver_transmit(data: &[u8]) -> Result<(), &'static str> {
    e1000::transmit(data)
}

#[cfg(target_arch = "aarch64")]
fn driver_transmit(data: &[u8]) -> Result<(), &'static str> {
    net_mmio::transmit(data)
}

/// ARP hardware type for Ethernet
pub const ARP_HTYPE_ETHERNET: u16 = 1;

/// ARP protocol type for IPv4
pub const ARP_PTYPE_IPV4: u16 = 0x0800;

/// ARP operation: request
pub const ARP_OP_REQUEST: u16 = 1;

/// ARP operation: reply
pub const ARP_OP_REPLY: u16 = 2;

/// ARP packet header size for Ethernet/IPv4
pub const ARP_PACKET_SIZE: usize = 28;

/// Maximum ARP cache entries
const ARP_CACHE_SIZE: usize = 16;

/// ARP cache entry
#[derive(Clone, Copy)]
struct ArpCacheEntry {
    ip: [u8; 4],
    mac: [u8; 6],
    valid: bool,
}

impl Default for ArpCacheEntry {
    fn default() -> Self {
        ArpCacheEntry {
            ip: [0; 4],
            mac: [0; 6],
            valid: false,
        }
    }
}

/// ARP cache
static ARP_CACHE: Mutex<[ArpCacheEntry; ARP_CACHE_SIZE]> =
    Mutex::new([ArpCacheEntry { ip: [0; 4], mac: [0; 6], valid: false }; ARP_CACHE_SIZE]);

/// Initialize ARP subsystem
pub fn init() {
    // Cache is already initialized with default values
    #[cfg(target_arch = "x86_64")]
    log::debug!("ARP: Cache initialized ({} entries)", ARP_CACHE_SIZE);
}

/// Parsed ARP packet
#[derive(Debug)]
#[allow(dead_code)] // Protocol fields - all are part of the ARP specification
pub struct ArpPacket {
    /// Hardware type (should be 1 for Ethernet)
    pub htype: u16,
    /// Protocol type (should be 0x0800 for IPv4)
    pub ptype: u16,
    /// Hardware address length (6 for Ethernet)
    pub hlen: u8,
    /// Protocol address length (4 for IPv4)
    pub plen: u8,
    /// Operation (1 = request, 2 = reply)
    pub operation: u16,
    /// Sender hardware address (MAC)
    pub sender_mac: [u8; 6],
    /// Sender protocol address (IP)
    pub sender_ip: [u8; 4],
    /// Target hardware address (MAC)
    pub target_mac: [u8; 6],
    /// Target protocol address (IP)
    pub target_ip: [u8; 4],
}

impl ArpPacket {
    /// Parse an ARP packet from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < ARP_PACKET_SIZE {
            return None;
        }

        let htype = u16::from_be_bytes([data[0], data[1]]);
        let ptype = u16::from_be_bytes([data[2], data[3]]);
        let hlen = data[4];
        let plen = data[5];
        let operation = u16::from_be_bytes([data[6], data[7]]);

        // Validate this is Ethernet/IPv4 ARP
        if htype != ARP_HTYPE_ETHERNET || ptype != ARP_PTYPE_IPV4 || hlen != 6 || plen != 4 {
            return None;
        }

        let sender_mac = [data[8], data[9], data[10], data[11], data[12], data[13]];
        let sender_ip = [data[14], data[15], data[16], data[17]];
        let target_mac = [data[18], data[19], data[20], data[21], data[22], data[23]];
        let target_ip = [data[24], data[25], data[26], data[27]];

        Some(ArpPacket {
            htype,
            ptype,
            hlen,
            plen,
            operation,
            sender_mac,
            sender_ip,
            target_mac,
            target_ip,
        })
    }

    /// Build an ARP packet
    pub fn build(
        operation: u16,
        sender_mac: &[u8; 6],
        sender_ip: &[u8; 4],
        target_mac: &[u8; 6],
        target_ip: &[u8; 4],
    ) -> Vec<u8> {
        let mut packet = Vec::with_capacity(ARP_PACKET_SIZE);

        // Hardware type (Ethernet = 1)
        packet.extend_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes());
        // Protocol type (IPv4 = 0x0800)
        packet.extend_from_slice(&ARP_PTYPE_IPV4.to_be_bytes());
        // Hardware address length (6)
        packet.push(6);
        // Protocol address length (4)
        packet.push(4);
        // Operation
        packet.extend_from_slice(&operation.to_be_bytes());
        // Sender MAC
        packet.extend_from_slice(sender_mac);
        // Sender IP
        packet.extend_from_slice(sender_ip);
        // Target MAC
        packet.extend_from_slice(target_mac);
        // Target IP
        packet.extend_from_slice(target_ip);

        packet
    }
}

/// Handle an incoming ARP packet
pub fn handle_arp(eth_frame: &EthernetFrame, arp: &ArpPacket) {
    let config = super::config();
    let our_mac = match get_mac_address() {
        Some(mac) => mac,
        None => return,
    };

    // Always learn from ARP packets (update cache with sender info)
    update_cache(&arp.sender_ip, &arp.sender_mac);

    // Check if this ARP is for us
    if arp.target_ip != config.ip_addr {
        return;
    }

    match arp.operation {
        ARP_OP_REQUEST => {
            // Send ARP reply
            #[cfg(target_arch = "x86_64")]
            log::debug!(
                "ARP: Request from {}.{}.{}.{} for our IP",
                arp.sender_ip[0], arp.sender_ip[1], arp.sender_ip[2], arp.sender_ip[3]
            );

            let reply = ArpPacket::build(
                ARP_OP_REPLY,
                &our_mac,
                &config.ip_addr,
                &arp.sender_mac,
                &arp.sender_ip,
            );

            let frame = ethernet::EthernetFrame::build(
                &our_mac,
                &eth_frame.src_mac,
                ETHERTYPE_ARP,
                &reply,
            );

            if let Err(_e) = driver_transmit(&frame) {
                #[cfg(target_arch = "x86_64")]
                log::warn!("ARP: Failed to send reply: {}", _e);
            } else {
                #[cfg(target_arch = "x86_64")]
                log::debug!("ARP: Sent reply");
            }
        }
        ARP_OP_REPLY => {
            #[cfg(target_arch = "x86_64")]
            log::debug!(
                "ARP: Reply from {}.{}.{}.{} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                arp.sender_ip[0], arp.sender_ip[1], arp.sender_ip[2], arp.sender_ip[3],
                arp.sender_mac[0], arp.sender_mac[1], arp.sender_mac[2],
                arp.sender_mac[3], arp.sender_mac[4], arp.sender_mac[5]
            );
            // Already updated cache above
        }
        _ => {}
    }
}

/// Update the ARP cache with a new entry
fn update_cache(ip: &[u8; 4], mac: &[u8; 6]) {
    let mut cache = ARP_CACHE.lock();

    // First, check if entry already exists
    for entry in cache.iter_mut() {
        if entry.valid && entry.ip == *ip {
            entry.mac = *mac;
            return;
        }
    }

    // Find an empty slot
    for entry in cache.iter_mut() {
        if !entry.valid {
            entry.ip = *ip;
            entry.mac = *mac;
            entry.valid = true;
            return;
        }
    }

    // Cache full - replace first entry (simple replacement policy)
    cache[0].ip = *ip;
    cache[0].mac = *mac;
    cache[0].valid = true;
}

/// Look up a MAC address in the ARP cache
pub fn lookup(ip: &[u8; 4]) -> Option<[u8; 6]> {
    let cache = ARP_CACHE.lock();

    for entry in cache.iter() {
        if entry.valid && entry.ip == *ip {
            return Some(entry.mac);
        }
    }

    None
}

/// Send an ARP request for an IP address
pub fn request(target_ip: &[u8; 4]) -> Result<(), &'static str> {
    let config = super::config();
    let our_mac = get_mac_address().ok_or("Network device not initialized")?;

    let arp_packet = ArpPacket::build(
        ARP_OP_REQUEST,
        &our_mac,
        &config.ip_addr,
        &[0, 0, 0, 0, 0, 0], // Unknown target MAC
        target_ip,
    );

    let frame = ethernet::EthernetFrame::build(
        &our_mac,
        &BROADCAST_MAC,
        ETHERTYPE_ARP,
        &arp_packet,
    );

    driver_transmit(&frame)?;

    #[cfg(target_arch = "x86_64")]
    log::debug!(
        "ARP: Sent request for {}.{}.{}.{}",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3]
    );

    Ok(())
}
