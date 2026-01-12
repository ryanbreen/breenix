//! Network stack for Breenix
//!
//! Implements a minimal network stack with:
//! - Ethernet frame parsing and construction
//! - ARP for IPv4 address resolution
//! - IPv4 packet handling
//! - ICMP echo (ping) request/reply

extern crate alloc;

pub mod arp;
pub mod ethernet;
pub mod icmp;
pub mod ipv4;
pub mod tcp;
pub mod udp;

use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::e1000;

/// Network interface configuration
#[derive(Clone, Copy, Debug)]
pub struct NetConfig {
    /// Our IPv4 address
    pub ip_addr: [u8; 4],
    /// Subnet mask
    pub subnet_mask: [u8; 4],
    /// Default gateway
    pub gateway: [u8; 4],
}

/// Default network configuration for QEMU user-mode networking (SLIRP)
/// QEMU's default user-mode network uses 10.0.2.0/24 with gateway at 10.0.2.2
#[allow(dead_code)] // Used conditionally without vmnet feature
pub const SLIRP_CONFIG: NetConfig = NetConfig {
    ip_addr: [10, 0, 2, 15],      // Guest IP
    subnet_mask: [255, 255, 255, 0],
    gateway: [10, 0, 2, 2],       // QEMU gateway
};

/// Network configuration for macOS vmnet/bridge networking
/// socket_vmnet daemon uses 192.168.105.x (configured via --vmnet-gateway in plist)
/// The daemon runs DHCP but we use static IP to avoid waiting for DHCP
#[allow(dead_code)] // Used conditionally with vmnet feature
pub const VMNET_CONFIG: NetConfig = NetConfig {
    ip_addr: [192, 168, 105, 100], // Static guest IP (avoiding DHCP conflicts)
    subnet_mask: [255, 255, 255, 0],
    gateway: [192, 168, 105, 1],   // vmnet gateway (socket_vmnet default)
};

/// Select network config based on compile-time feature or default to SLIRP
/// Use VMNET_CONFIG when BREENIX_NET_MODE=vmnet is set at build time
#[cfg(feature = "vmnet")]
pub const DEFAULT_CONFIG: NetConfig = VMNET_CONFIG;

#[cfg(not(feature = "vmnet"))]
pub const DEFAULT_CONFIG: NetConfig = SLIRP_CONFIG;

static NET_CONFIG: Mutex<NetConfig> = Mutex::new(DEFAULT_CONFIG);

/// Maximum number of packets to queue in loopback queue
/// Prevents unbounded memory growth if drain_loopback_queue() is not called
const MAX_LOOPBACK_QUEUE_SIZE: usize = 32;

/// Loopback packet queue for deferred delivery
/// Packets sent to our own IP are queued here and delivered after the sender releases locks
struct LoopbackPacket {
    /// Raw IP packet data
    data: Vec<u8>,
}

static LOOPBACK_QUEUE: Mutex<Vec<LoopbackPacket>> = Mutex::new(Vec::new());

/// Drain the loopback queue, delivering any pending packets
/// Called after syscalls release their locks to avoid deadlock
pub fn drain_loopback_queue() {
    // Take all packets from the queue
    let packets: Vec<LoopbackPacket> = {
        let mut queue = LOOPBACK_QUEUE.lock();
        core::mem::take(&mut *queue)
    };

    // Deliver each packet
    for packet in packets {
        if let Some(parsed_ip) = ipv4::Ipv4Packet::parse(&packet.data) {
            let src_mac = e1000::mac_address().unwrap_or([0; 6]);
            let dummy_frame = ethernet::EthernetFrame {
                src_mac,
                dst_mac: src_mac,
                ethertype: ethernet::ETHERTYPE_IPV4,
                payload: &packet.data,
            };
            ipv4::handle_ipv4(&dummy_frame, &parsed_ip);
        }
    }
}

/// Initialize the network stack
pub fn init() {
    log::info!("NET: Initializing network stack...");

    if let Some(mac) = e1000::mac_address() {
        log::info!(
            "NET: MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    }

    let config = NET_CONFIG.lock();
    log::info!(
        "NET: IP address: {}.{}.{}.{}",
        config.ip_addr[0], config.ip_addr[1], config.ip_addr[2], config.ip_addr[3]
    );
    log::info!(
        "NET: Gateway: {}.{}.{}.{}",
        config.gateway[0], config.gateway[1], config.gateway[2], config.gateway[3]
    );

    // Initialize ARP cache
    arp::init();

    log::info!("Network stack initialized");

    // Send ARP request for gateway to test network connectivity
    let gateway = config.gateway;
    drop(config); // Release lock before calling arp::request
    log::info!("NET: Sending ARP request for gateway {}.{}.{}.{}",
        gateway[0], gateway[1], gateway[2], gateway[3]);
    if let Err(e) = arp::request(&gateway) {
        log::warn!("NET: Failed to send ARP request: {}", e);
        return;
    }
    log::info!("ARP request sent successfully");

    // Wait for ARP reply (poll RX a few times to get the gateway MAC)
    // The reply comes via interrupt, so we just need to give it time to arrive
    for _ in 0..50 {
        process_rx();
        // Delay to let packets arrive and interrupts fire
        for _ in 0..500000 {
            core::hint::spin_loop();
        }
        // Check if we got the ARP reply yet
        if let Some(gateway_mac) = arp::lookup(&gateway) {
            log::info!("NET: ARP resolved gateway MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                gateway_mac[0], gateway_mac[1], gateway_mac[2],
                gateway_mac[3], gateway_mac[4], gateway_mac[5]);
            break;
        }
    }

    // Check if ARP resolved the gateway
    if arp::lookup(&gateway).is_none() {
        log::warn!("NET: Gateway ARP not resolved, skipping ping test");
        return;
    }

    // Send ICMP echo request (ping) to gateway
    log::info!("NET: Sending ICMP echo request to gateway {}.{}.{}.{}",
        gateway[0], gateway[1], gateway[2], gateway[3]);
    if let Err(e) = ping(gateway) {
        log::warn!("NET: Failed to send ping: {}", e);
        return;
    }

    // Poll for the ping reply (just process RX to handle incoming packets)
    for _ in 0..20 {
        process_rx();
        // Delay to let packets arrive and interrupts fire
        for _ in 0..500000 {
            core::hint::spin_loop();
        }
    }

    log::info!("NET: Network initialization complete");
}

/// Get the current network configuration
pub fn config() -> NetConfig {
    *NET_CONFIG.lock()
}

/// Process incoming packets (called from interrupt handler or polling loop)
pub fn process_rx() {
    let mut buffer = [0u8; 2048];

    while e1000::can_receive() {
        match e1000::receive(&mut buffer) {
            Ok(len) => {
                process_packet(&buffer[..len]);
            }
            Err(_) => break,
        }
    }
}

/// Process a received Ethernet frame
fn process_packet(data: &[u8]) {
    if let Some(frame) = ethernet::EthernetFrame::parse(data) {
        match frame.ethertype {
            ethernet::ETHERTYPE_ARP => {
                if let Some(arp_packet) = arp::ArpPacket::parse(frame.payload) {
                    arp::handle_arp(&frame, &arp_packet);
                }
            }
            ethernet::ETHERTYPE_IPV4 => {
                if let Some(ip_packet) = ipv4::Ipv4Packet::parse(frame.payload) {
                    ipv4::handle_ipv4(&frame, &ip_packet);
                }
            }
            _ => {
                // Unknown ethertype, ignore
            }
        }
    }
}

/// Send an Ethernet frame
pub fn send_ethernet(dst_mac: &[u8; 6], ethertype: u16, payload: &[u8]) -> Result<(), &'static str> {
    let src_mac = e1000::mac_address().ok_or("E1000 not initialized")?;

    let frame = ethernet::EthernetFrame::build(&src_mac, dst_mac, ethertype, payload);
    e1000::transmit(&frame)
}

/// Send an IPv4 packet
pub fn send_ipv4(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Result<(), &'static str> {
    let config = config();

    // Check for loopback - sending to ourselves or to 127.x.x.x network
    if dst_ip == config.ip_addr || dst_ip[0] == 127 {
        log::debug!("NET: Loopback detected, queueing packet for deferred delivery");

        // Build IP packet
        let ip_packet = ipv4::Ipv4Packet::build(
            config.ip_addr,
            dst_ip,
            protocol,
            payload,
        );

        // Queue for deferred delivery (to avoid deadlock with process manager lock)
        // The caller must call drain_loopback_queue() after releasing locks
        let mut queue = LOOPBACK_QUEUE.lock();

        // Drop oldest packet if queue is full to prevent unbounded memory growth
        if queue.len() >= MAX_LOOPBACK_QUEUE_SIZE {
            queue.remove(0);
            log::warn!("NET: Loopback queue full, dropped oldest packet");
        }

        queue.push(LoopbackPacket { data: ip_packet });
        log::debug!("NET: Loopback packet queued (queue size: {})", queue.len());

        return Ok(());
    }

    // Look up destination MAC in ARP cache
    // For QEMU SLIRP mode, always send through gateway since SLIRP doesn't have real
    // hosts on the virtual subnet - all services (DNS at 10.0.2.3, etc.) are emulated
    // by SLIRP and routed through the gateway MAC.
    // For real networks, we could try direct ARP for same-subnet destinations.
    let dst_mac = arp::lookup(&config.gateway).ok_or("ARP lookup failed - gateway not in cache")?;

    // Build IP packet
    let ip_packet = ipv4::Ipv4Packet::build(
        config.ip_addr,
        dst_ip,
        protocol,
        payload,
    );

    send_ethernet(&dst_mac, ethernet::ETHERTYPE_IPV4, &ip_packet)
}

/// Send an ICMP echo request (ping)
#[allow(dead_code)] // Public API
pub fn ping(dst_ip: [u8; 4]) -> Result<(), &'static str> {
    let icmp_packet = icmp::IcmpPacket::echo_request(1, 1, b"breenix ping");
    send_ipv4(dst_ip, ipv4::PROTOCOL_ICMP, &icmp_packet)
}
