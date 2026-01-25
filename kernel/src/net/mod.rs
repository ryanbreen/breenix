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

// TCP and UDP require process/socket/ipc modules which are x86_64-only for now
#[cfg(target_arch = "x86_64")]
pub mod tcp;
#[cfg(target_arch = "x86_64")]
pub mod udp;

use alloc::vec::Vec;
use spin::Mutex;

// Use E1000 on x86_64, VirtIO net on ARM64
#[cfg(target_arch = "x86_64")]
use crate::drivers::e1000;
#[cfg(target_arch = "aarch64")]
use crate::drivers::virtio::net_mmio;

#[cfg(target_arch = "x86_64")]
use crate::task::softirqd::{register_softirq_handler, SoftirqType};

// Logging macros that work on both architectures
#[cfg(target_arch = "x86_64")]
macro_rules! net_log {
    ($($arg:tt)*) => { log::info!($($arg)*) };
}

#[cfg(target_arch = "aarch64")]
macro_rules! net_log {
    ($($arg:tt)*) => { crate::serial_println!($($arg)*) };
}

#[cfg(target_arch = "x86_64")]
macro_rules! net_warn {
    ($($arg:tt)*) => { log::warn!($($arg)*) };
}

#[cfg(target_arch = "aarch64")]
macro_rules! net_warn {
    ($($arg:tt)*) => { crate::serial_println!($($arg)*) };
}

#[cfg(target_arch = "x86_64")]
macro_rules! net_debug {
    ($($arg:tt)*) => { log::debug!($($arg)*) };
}

#[cfg(target_arch = "aarch64")]
macro_rules! net_debug {
    ($($arg:tt)*) => { /* No-op on ARM64 for now */ };
}

// Driver abstraction functions

/// Get the MAC address from the network device
fn get_mac_address() -> Option<[u8; 6]> {
    #[cfg(target_arch = "x86_64")]
    { e1000::mac_address() }
    #[cfg(target_arch = "aarch64")]
    { net_mmio::mac_address() }
}

/// Transmit a raw Ethernet frame
fn driver_transmit(data: &[u8]) -> Result<(), &'static str> {
    #[cfg(target_arch = "x86_64")]
    { e1000::transmit(data) }
    #[cfg(target_arch = "aarch64")]
    { net_mmio::transmit(data) }
}

/// Network interface configuration
#[derive(Clone, Copy, Debug)]
pub struct NetConfig {
    /// Our IPv4 address
    pub ip_addr: [u8; 4],
    /// Subnet mask (for routing decisions - not yet used but required for complete config)
    #[allow(dead_code)] // Part of complete network config API
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
            let src_mac = get_mac_address().unwrap_or([0; 6]);
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

/// Softirq handler for network RX processing
/// Called from softirq context when NetRx softirq is raised by e1000 interrupt handler
#[cfg(target_arch = "x86_64")]
fn net_rx_softirq_handler(_softirq: SoftirqType) {
    process_rx();
}

/// Re-register the network softirq handler.
/// This is needed after tests that override the handler for testing purposes.
#[cfg(target_arch = "x86_64")]
pub fn register_net_softirq() {
    register_softirq_handler(SoftirqType::NetRx, net_rx_softirq_handler);
}

/// Re-register the network softirq handler (no-op on ARM64).
#[cfg(target_arch = "aarch64")]
pub fn register_net_softirq() {
    // ARM64 uses polling for now, no softirq registration needed
}

/// Initialize the network stack
#[cfg(target_arch = "x86_64")]
pub fn init() {
    // Register NET_RX softirq handler FIRST - before any network operations
    // This ensures the handler is ready before e1000 can raise the softirq
    register_softirq_handler(SoftirqType::NetRx, net_rx_softirq_handler);

    log::info!("NET: Initializing network stack...");

    if let Some(mac) = e1000::mac_address() {
        log::info!(
            "NET: MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    }

    init_common();
}

/// Initialize the network stack (ARM64 version)
#[cfg(target_arch = "aarch64")]
pub fn init() {
    crate::serial_println!("[net] Initializing network stack...");

    if let Some(mac) = net_mmio::mac_address() {
        crate::serial_println!(
            "[net] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    }

    init_common();
}

/// Common initialization logic for both architectures
fn init_common() {
    #[cfg(target_arch = "x86_64")]
    let mac_available = e1000::mac_address().is_some();
    #[cfg(target_arch = "aarch64")]
    let mac_available = net_mmio::mac_address().is_some();

    if !mac_available {
        #[cfg(target_arch = "x86_64")]
        log::warn!("NET: No network device available");
        #[cfg(target_arch = "aarch64")]
        crate::serial_println!("[net] No network device available");
        return;
    }

    let config = NET_CONFIG.lock();
    net_log!("NET: IP address: {}.{}.{}.{}",
        config.ip_addr[0], config.ip_addr[1], config.ip_addr[2], config.ip_addr[3]
    );
    net_log!("NET: Gateway: {}.{}.{}.{}",
        config.gateway[0], config.gateway[1], config.gateway[2], config.gateway[3]
    );

    // Initialize ARP cache
    arp::init();

    net_log!("Network stack initialized");

    // Send ARP request for gateway to test network connectivity
    let gateway = config.gateway;
    drop(config); // Release lock before calling arp::request
    net_log!("NET: Sending ARP request for gateway {}.{}.{}.{}",
        gateway[0], gateway[1], gateway[2], gateway[3]);
    if let Err(e) = arp::request(&gateway) {
        net_log!("NET: Failed to send ARP request: {}", e);
        return;
    }
    net_log!("ARP request sent successfully");

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
            net_log!("NET: ARP resolved gateway MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                gateway_mac[0], gateway_mac[1], gateway_mac[2],
                gateway_mac[3], gateway_mac[4], gateway_mac[5]);
            break;
        }
    }

    // Check if ARP resolved the gateway
    if arp::lookup(&gateway).is_none() {
        net_log!("NET: Gateway ARP not resolved, skipping ping test");
        return;
    }

    // Send ICMP echo request (ping) to gateway
    net_log!("NET: Sending ICMP echo request to gateway {}.{}.{}.{}",
        gateway[0], gateway[1], gateway[2], gateway[3]);
    if let Err(e) = ping(gateway) {
        net_log!("NET: Failed to send ping: {}", e);
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

    net_log!("NET: Network initialization complete");
}

/// Get the current network configuration
pub fn config() -> NetConfig {
    *NET_CONFIG.lock()
}

/// Process incoming packets (called from interrupt handler or polling loop)
#[cfg(target_arch = "x86_64")]
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

/// Process incoming packets (ARM64 - polling based)
#[cfg(target_arch = "aarch64")]
pub fn process_rx() {
    // VirtIO net driver returns borrowed slice
    while let Some(data) = net_mmio::receive() {
        process_packet(data);
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
    let src_mac = get_mac_address().ok_or("Network device not initialized")?;

    let frame = ethernet::EthernetFrame::build(&src_mac, dst_mac, ethertype, payload);
    driver_transmit(&frame)
}

/// Send an IPv4 packet
pub fn send_ipv4(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Result<(), &'static str> {
    let config = config();

    // Check for loopback - sending to ourselves or to 127.x.x.x network
    if dst_ip == config.ip_addr || dst_ip[0] == 127 {
        net_debug!("NET: Loopback detected, queueing packet for deferred delivery");

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
            net_warn!("NET: Loopback queue full, dropped oldest packet");
        }

        queue.push(LoopbackPacket { data: ip_packet });
        net_debug!("NET: Loopback packet queued (queue size: {})", queue.len());

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
