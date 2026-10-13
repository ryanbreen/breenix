//! ARP (Address Resolution Protocol) implementation
//!
//! Implements RFC 826 for IPv4-to-Ethernet address resolution.

use spin::Mutex;

use super::ethernet::{self, EthernetFrame, BROADCAST_MAC, ETHERTYPE_ARP};

// Use parent module's driver abstraction
fn get_mac_address() -> Option<[u8; 6]> {
    super::get_mac_address()
}

fn driver_transmit(data: &[u8]) -> Result<(), &'static str> {
    super::driver_transmit(data)
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
static ARP_CACHE: Mutex<[ArpCacheEntry; ARP_CACHE_SIZE]> = Mutex::new(
    [ArpCacheEntry {
        ip: [0; 4],
        mac: [0; 6],
        valid: false,
    }; ARP_CACHE_SIZE],
);

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
    ) -> [u8; ARP_PACKET_SIZE] {
        let mut packet = [0; ARP_PACKET_SIZE];
        packet[0..2].copy_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes());
        packet[2..4].copy_from_slice(&ARP_PTYPE_IPV4.to_be_bytes());
        packet[4] = 6;
        packet[5] = 4;
        packet[6..8].copy_from_slice(&operation.to_be_bytes());
        packet[8..14].copy_from_slice(sender_mac);
        packet[14..18].copy_from_slice(sender_ip);
        packet[18..24].copy_from_slice(target_mac);
        packet[24..28].copy_from_slice(target_ip);
        packet
    }
}

/// Handle an incoming ARP packet
pub fn handle_arp(eth_frame: &EthernetFrame, arp: &ArpPacket) {
    crate::tracing::providers::net_rx::count_arp();

    let config = super::config();
    let our_mac = match get_mac_address() {
        Some(mac) => mac,
        None => return,
    };

    // Always learn from ARP packets (update cache with sender info)
    update_cache(&arp.sender_ip, &arp.sender_mac);
    super::flush_arp_pending_packets(&arp.sender_ip, &arp.sender_mac);

    // Check if this ARP is for us
    if arp.target_ip != config.ip_addr {
        return;
    }

    match arp.operation {
        ARP_OP_REQUEST => {
            let reply = ArpPacket::build(
                ARP_OP_REPLY,
                &our_mac,
                &config.ip_addr,
                &arp.sender_mac,
                &arp.sender_ip,
            );

            let frame =
                ethernet::EthernetFrame::build(&our_mac, &eth_frame.src_mac, ETHERTYPE_ARP, &reply);

            if let Ok(frame) = frame {
                let _ = driver_transmit(&frame);
            }
        }
        ARP_OP_REPLY => {
            if arp.sender_ip == config.gateway {
                static NOTICE: crate::log_notice::Node = crate::log_notice::Node::new();
                static POSTED: core::sync::atomic::AtomicBool =
                    core::sync::atomic::AtomicBool::new(false);
                if !POSTED.swap(true, core::sync::atomic::Ordering::Relaxed) {
                    let mut line = *b"NET: ARP resolved gateway MAC: 00:00:00:00:00:00";
                    let hex = b"0123456789abcdef";
                    for (i, byte) in arp.sender_mac.iter().enumerate() {
                        line[31 + i * 3] = hex[(byte >> 4) as usize];
                        line[32 + i * 3] = hex[(byte & 15) as usize];
                    }
                    if let Ok(text) = core::str::from_utf8(&line) {
                        NOTICE.post_again(text);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Update the ARP cache with a new entry.
/// Uses bottom-half exclusion on x86_64 and IRQ masking on aarch64 because the
/// network softirq also calls this through process_rx → handle_arp.
fn update_cache(ip: &[u8; 4], mac: &[u8; 6]) {
    let _guard = super::net_lock_guard();
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

/// Look up a MAC address in the ARP cache.
/// Uses bottom-half exclusion on x86_64 and IRQ masking on aarch64 to prevent
/// same-CPU re-entry from the network softirq.
pub fn lookup(ip: &[u8; 4]) -> Option<[u8; 6]> {
    let _guard = super::net_lock_guard();
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

    let frame =
        ethernet::EthernetFrame::build(&our_mac, &BROADCAST_MAC, ETHERTYPE_ARP, &arp_packet)?;

    driver_transmit(&frame)?;

    Ok(())
}
