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

// TCP and UDP protocol implementations - architecture-independent
// The socket syscall layer handles arch-specific details
pub mod tcp;
pub mod udp;

use alloc::vec::Vec;
use spin::Mutex;

// Use E1000 on x86_64, VirtIO net on ARM64 (MMIO for QEMU, PCI for Parallels)
// On VMware ARM64, e1000 is used (Intel 82574L emulation)
use crate::drivers::e1000;
#[cfg(target_arch = "aarch64")]
use crate::drivers::virtio::net_mmio;
#[cfg(target_arch = "aarch64")]
use crate::drivers::virtio::net_pci;

use crate::task::softirqd::{register_softirq_handler, SoftirqType};

const E1000_CARRIER_WAIT_MS: u32 = 5000;
const E1000_CARRIER_POLL_MS: u32 = 50;

/// Disable IRQs and return saved DAIF state. Prevents timer interrupt →
/// softirq → process_rx from deadlocking on shared locks (ARP_CACHE,
/// NET_CONFIG) that the interrupted thread may hold.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn irq_save() -> u64 {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
    }
    daif
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(crate) fn irq_restore(saved: u64) {
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) saved, options(nomem, nostack));
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) fn irq_save() -> u64 {
    0
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(crate) fn irq_restore(_: u64) {}

/// Re-entrancy guard for process_rx() on aarch64. Prevents nested RX drains
/// when interrupt-driven NetRx preempts another RX processing context.
#[cfg(target_arch = "aarch64")]
static RX_PROCESSING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

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
    ($($arg:tt)*) => {
        /* No-op on ARM64 for now */
    };
}

// Driver abstraction functions

/// Get the MAC address from the network device
fn get_mac_address() -> Option<[u8; 6]> {
    #[cfg(target_arch = "x86_64")]
    {
        e1000::mac_address()
    }
    #[cfg(target_arch = "aarch64")]
    {
        // Try VirtIO MMIO (QEMU), VirtIO PCI (Parallels), then e1000 (VMware)
        net_mmio::mac_address()
            .or_else(|| net_pci::mac_address())
            .or_else(|| e1000::mac_address())
    }
}

/// Transmit a raw Ethernet frame
fn driver_transmit(data: &[u8]) -> Result<(), &'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        if !e1000::link_up() {
            return Err("e1000 link down");
        }
        e1000::transmit(data)
    }
    #[cfg(target_arch = "aarch64")]
    {
        if net_pci::is_initialized() {
            net_pci::transmit(data)
        } else if e1000::is_initialized() {
            if !e1000::link_up() {
                return Err("e1000 link down");
            }
            e1000::transmit(data)
        } else {
            net_mmio::transmit(data)
        }
    }
}

fn active_tx_driver_is_e1000() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        e1000::is_initialized()
    }
    #[cfg(target_arch = "aarch64")]
    {
        !net_pci::is_initialized() && e1000::is_initialized()
    }
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
    ip_addr: [10, 0, 2, 15], // Guest IP
    subnet_mask: [255, 255, 255, 0],
    gateway: [10, 0, 2, 2], // QEMU gateway
};

/// Network configuration for macOS vmnet/bridge networking
/// socket_vmnet daemon uses 192.168.105.x (configured via --vmnet-gateway in plist)
/// The daemon runs DHCP but we use static IP to avoid waiting for DHCP
#[allow(dead_code)] // Used conditionally with vmnet feature
pub const VMNET_CONFIG: NetConfig = NetConfig {
    ip_addr: [192, 168, 105, 100], // Static guest IP (avoiding DHCP conflicts)
    subnet_mask: [255, 255, 255, 0],
    gateway: [192, 168, 105, 1], // vmnet gateway (socket_vmnet default)
};

/// Network configuration for Parallels Desktop shared networking (NAT)
/// Parallels shared network uses 10.211.55.x with gateway at 10.211.55.1
#[allow(dead_code)] // Used conditionally when PCI net is active
pub const PARALLELS_CONFIG: NetConfig = NetConfig {
    ip_addr: [10, 211, 55, 100], // Static guest IP (avoiding DHCP conflicts)
    subnet_mask: [255, 255, 255, 0],
    gateway: [10, 211, 55, 1], // Parallels shared network gateway
};

/// Network configuration for VMware Fusion NAT networking
/// VMware NAT (vmnet8) uses 172.16.45.x with gateway at 172.16.45.2
#[allow(dead_code)] // Used conditionally when e1000 is active on VMware
pub const VMWARE_CONFIG: NetConfig = NetConfig {
    ip_addr: [172, 16, 45, 100], // Static guest IP (avoiding DHCP conflicts)
    subnet_mask: [255, 255, 255, 0],
    gateway: [172, 16, 45, 2], // VMware NAT gateway
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

/// Softirq handler for network RX processing.
/// Called from softirq context when NetRx softirq is raised by the network IRQ path.
///
/// PCI VirtIO uses a NAPI-shaped completion path: the IRQ handler suppresses
/// device callbacks and raises NetRx; this handler drains a bounded packet
/// budget and either re-enables callbacks or re-raises NetRx for more work.
fn net_rx_softirq_handler(_softirq: SoftirqType) {
    let outcome = process_rx_budgeted(64);
    if outcome == PollOutcome::BudgetExhausted {
        crate::tracing::providers::counters::NET_RX_BUDGET_EXHAUSTED.increment();
    }

    #[cfg(target_arch = "aarch64")]
    if net_pci::is_initialized() {
        match outcome {
            PollOutcome::Drained => {
                if net_pci::reenable_and_check_race() {
                    crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
                }
            }
            PollOutcome::BudgetExhausted => {
                crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
            }
        }
    }
}

/// Re-register the network softirq handler.
/// This is needed after tests that override the handler for testing purposes.
pub fn register_net_softirq() {
    register_softirq_handler(SoftirqType::NetRx, net_rx_softirq_handler);
}

/// Initialize the network stack
#[cfg(target_arch = "x86_64")]
pub fn init() {
    // Register NET_RX softirq handler FIRST - before any network operations
    // This ensures the handler is ready before e1000 can raise the softirq
    register_net_softirq();

    log::info!("NET: Initializing network stack...");

    if let Some(mac) = e1000::mac_address() {
        log::info!(
            "NET: MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );
    }

    init_common();
}

/// Initialize the network stack (ARM64 version)
#[cfg(target_arch = "aarch64")]
pub fn init() {
    // Register NET_RX softirq handler FIRST - before any network operations
    // This ensures the handler is ready before virtio-net can raise the softirq
    register_net_softirq();

    crate::serial_println!("[net] Initializing network stack...");

    // Auto-detect platform: PCI net = Parallels, e1000 = VMware, MMIO net = QEMU
    if net_pci::is_initialized() {
        crate::serial_println!("[net] Using VirtIO net PCI driver (Parallels)");
        let saved = irq_save();
        let mut config = NET_CONFIG.lock();
        *config = PARALLELS_CONFIG;
        drop(config);
        irq_restore(saved);
    } else if e1000::is_initialized() {
        crate::serial_println!("[net] Using Intel e1000 driver (VMware)");
        let saved = irq_save();
        let mut config = NET_CONFIG.lock();
        *config = VMWARE_CONFIG;
        drop(config);
        irq_restore(saved);
    }

    if let Some(mac) = get_mac_address() {
        crate::serial_println!(
            "[net] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );
    }

    init_common();
}

/// Common initialization logic for both architectures
fn init_common() {
    let mac_available = get_mac_address().is_some();

    if !mac_available {
        #[cfg(target_arch = "x86_64")]
        log::warn!("NET: No network device available");
        #[cfg(target_arch = "aarch64")]
        crate::serial_println!("[net] No network device available");
        return;
    }

    let saved = irq_save();
    let config = NET_CONFIG.lock();
    let ip = config.ip_addr;
    let gw = config.gateway;
    drop(config);
    irq_restore(saved);

    net_log!("NET: IP address: {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    net_log!("NET: Gateway: {}.{}.{}.{}", gw[0], gw[1], gw[2], gw[3]);

    // Initialize ARP cache
    arp::init();

    net_log!("Network stack initialized");

    if active_tx_driver_is_e1000() {
        // Linux e1000e only wakes TX/carrier after link is confirmed:
        // drivers/net/ethernet/intel/e1000e/netdev.c:5197-5304.
        let mut elapsed_ms = 0;
        while !e1000::link_up() && elapsed_ms < E1000_CARRIER_WAIT_MS {
            for _ in 0..2_500_000u32 {
                core::hint::spin_loop();
            }
            elapsed_ms += E1000_CARRIER_POLL_MS;
        }

        if e1000::link_up() {
            net_log!(
                "[net] e1000 link up after {}ms -- proceeding with ARP",
                elapsed_ms
            );
        } else {
            net_log!(
                "[net] e1000 link did not come up after {}ms -- skipping ARP (carrier-gated)",
                E1000_CARRIER_WAIT_MS
            );
            net_log!("NET: Network initialization complete");
            return;
        }
    }

    // Send ARP request for gateway to test network connectivity
    let gateway = gw;
    net_log!(
        "NET: Sending ARP request for gateway {}.{}.{}.{}",
        gateway[0],
        gateway[1],
        gateway[2],
        gateway[3]
    );
    if let Err(e) = arp::request(&gateway) {
        net_log!("NET: Failed to send ARP request: {}", e);
    }
    net_log!("ARP request sent successfully");

    // ARP resolution completes through interrupt-driven RX after init. Do not
    // spin-poll here; that hides whether MSI-X/softirq networking works.
    if arp::lookup(&gateway).is_none() {
        net_log!("NET: Gateway ARP not resolved during init; will resolve via IRQ path");
    }

    // Send ICMP echo request (ping) to gateway
    net_log!(
        "NET: Sending ICMP echo request to gateway {}.{}.{}.{}",
        gateway[0],
        gateway[1],
        gateway[2],
        gateway[3]
    );
    if let Err(e) = ping(gateway) {
        net_log!("NET: Failed to send ping: {}", e);
    }

    net_log!("NET: Network initialization complete");

    // Enable network device interrupt unconditionally at the end of init.
    // All post-init RX must flow through IRQ -> NetRx softirq, not init polling.
    #[cfg(target_arch = "aarch64")]
    {
        if net_pci::is_initialized() {
            // Enable MSI-X SPI at GIC now that init has completed.
            net_pci::enable_msi_spi();
        } else {
            net_mmio::enable_net_irq();
        }

        // Substep 4 bootstrap plus Substep 6 hardening: synchronously clear
        // virtio RX callback suppression so the next inbound MSI can fire even
        // if softirqd has not run its first NetRx dispatch yet. The softirq
        // raise remains as a redundant path for any RX state already present.
        if net_pci::is_initialized() {
            let _ = net_pci::reenable_and_check_race();
            net_log!("NET: synchronously cleared virtio callback suppression");
        }
        crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
        net_log!("NET: pre-primed NetRx softirq for bootstrap callback re-enable");
    }
}

/// Get the current network configuration.
/// IRQ-safe: disables interrupts to prevent deadlock with softirq handler.
pub fn config() -> NetConfig {
    let saved = irq_save();
    let c = *NET_CONFIG.lock();
    irq_restore(saved);
    c
}

/// Result of a bounded network RX poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    Drained,
    BudgetExhausted,
}

#[cfg(target_arch = "aarch64")]
fn reclaim_driver_tx_completed() {
    if net_pci::is_initialized() {
        let _ = net_pci::reclaim_tx_completed();
    } else if !e1000::is_initialized() {
        let _ = net_mmio::reclaim_tx_completed();
    }
}

/// Process incoming packets (called from interrupt handler or polling loop)
#[cfg(target_arch = "x86_64")]
pub fn process_rx() {
    let _ = process_rx_budgeted(u32::MAX);
}

/// Process incoming packets up to `budget` frames.
#[cfg(target_arch = "x86_64")]
pub fn process_rx_budgeted(budget: u32) -> PollOutcome {
    let mut buffer = [0u8; 2048];
    let mut remaining = budget;

    while remaining > 0 {
        if !e1000::can_receive() {
            return PollOutcome::Drained;
        }
        match e1000::receive(&mut buffer) {
            Ok(len) => {
                process_packet(&buffer[..len]);
                remaining -= 1;
            }
            Err(_) => return PollOutcome::Drained,
        }
    }

    PollOutcome::BudgetExhausted
}

/// Process incoming packets (ARM64 - polling or interrupt driven)
///
/// Protected by RX_PROCESSING atomic to prevent re-entrancy. When MSI-X is
/// active, the softirq handler can preempt another RX drain and try to call
/// process_rx() re-entrantly; the guard skips the nested call.
#[cfg(target_arch = "aarch64")]
pub fn process_rx() {
    let _ = process_rx_budgeted(u32::MAX);
}

/// Process incoming packets up to `budget` frames.
#[cfg(target_arch = "aarch64")]
pub fn process_rx_budgeted(budget: u32) -> PollOutcome {
    // Re-entrancy guard: if MSI-X -> softirq -> process_rx preempts another RX
    // drain, skip this nested call.
    use core::sync::atomic::Ordering;
    if RX_PROCESSING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return PollOutcome::Drained;
    }

    reclaim_driver_tx_completed();

    let mut remaining = budget;
    let mut outcome = PollOutcome::Drained;

    // Try PCI driver first (Parallels), then e1000 (VMware), then MMIO (QEMU)
    if net_pci::is_initialized() {
        let mut processed = false;
        while remaining > 0 {
            let Some(data) = net_pci::receive() else {
                break;
            };
            process_packet(data);
            processed = true;
            remaining -= 1;
        }
        if processed {
            net_pci::recycle_rx_buffers();
        }
    } else if e1000::is_initialized() {
        let mut buffer = [0u8; 2048];
        while remaining > 0 {
            if !e1000::can_receive() {
                break;
            }
            match e1000::receive(&mut buffer) {
                Ok(len) => {
                    process_packet(&buffer[..len]);
                    remaining -= 1;
                }
                Err(_) => break,
            }
        }
    } else {
        let mut processed = false;
        while remaining > 0 {
            let Some(data) = net_mmio::receive() else {
                break;
            };
            process_packet(data);
            processed = true;
            remaining -= 1;
        }
        if processed {
            net_mmio::recycle_rx_buffers();
        }
    }

    if remaining == 0 {
        outcome = PollOutcome::BudgetExhausted;
    }

    // Drain deferred TX queue — packets queued during RX processing (e.g., TCP
    // SYN-ACK responses) can now be sent safely since RX processing is complete.
    tcp::drain_deferred_tx();

    // Do NOT re-enable SPI here — the softirq handler does it after process_rx
    // returns, regardless of whether we processed packets or bailed on re-entrancy.
    // This avoids re-enabling from multiple code paths.

    RX_PROCESSING.store(false, Ordering::Release);
    outcome
}

/// Source MAC of the packet currently being processed (for response routing).
/// Set during process_packet, used by TCP to route SYN-ACK to the correct MAC.
static CURRENT_PACKET_SRC_MAC: Mutex<[u8; 6]> = Mutex::new([0; 6]);

/// Get the source MAC of the current incoming packet.
pub fn current_packet_src_mac() -> [u8; 6] {
    *CURRENT_PACKET_SRC_MAC.lock()
}

/// Process a received Ethernet frame
fn process_packet(data: &[u8]) {
    if let Some(frame) = ethernet::EthernetFrame::parse(data) {
        // Save source MAC so TCP can use it for SYN-ACK routing
        *CURRENT_PACKET_SRC_MAC.lock() = frame.src_mac;
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
pub fn send_ethernet(
    dst_mac: &[u8; 6],
    ethertype: u16,
    payload: &[u8],
) -> Result<(), &'static str> {
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
        let ip_packet = ipv4::Ipv4Packet::build(config.ip_addr, dst_ip, protocol, payload);

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

    // Determine the next-hop MAC address.
    // If the destination is on the same /24 subnet, ARP for it directly.
    // Otherwise, route through the gateway (standard IP routing).
    let same_subnet = dst_ip[0] == config.ip_addr[0]
        && dst_ip[1] == config.ip_addr[1]
        && dst_ip[2] == config.ip_addr[2];
    let next_hop = if same_subnet { dst_ip } else { config.gateway };
    let dst_mac = match arp::lookup(&next_hop) {
        Some(mac) => mac,
        None => {
            // ARP resolution is asynchronous: request the next-hop MAC and let
            // IRQ-driven NetRx populate the cache. Callers should retry on
            // ArpMiss; higher layers can rely on normal retransmission.
            net_log!(
                "NET: ARP cache miss for {}.{}.{}.{}, sending ARP request",
                next_hop[0],
                next_hop[1],
                next_hop[2],
                next_hop[3]
            );
            if let Err(e) = arp::request(&next_hop) {
                net_warn!("NET: ARP request failed after cache miss: {}", e);
                return Err("ARP request failed");
            }
            return Err("ArpMiss: reply will populate cache via IRQ");
        }
    };

    // Build IP packet
    let ip_packet = ipv4::Ipv4Packet::build(config.ip_addr, dst_ip, protocol, payload);

    send_ethernet(&dst_mac, ethernet::ETHERTYPE_IPV4, &ip_packet)
}

/// Send an ICMP echo request (ping)
#[allow(dead_code)] // Public API
pub fn ping(dst_ip: [u8; 4]) -> Result<(), &'static str> {
    let icmp_packet = icmp::IcmpPacket::echo_request(1, 1, b"breenix ping");
    send_ipv4(dst_ip, ipv4::PROTOCOL_ICMP, &icmp_packet)
}
