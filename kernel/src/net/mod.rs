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
pub(crate) mod loopback_pump;

// TCP and UDP protocol implementations - architecture-independent
// The socket syscall layer handles arch-specific details
pub mod tcp;
pub mod udp;

pub use loopback_pump::{
    init_loopback_pump, loopback_pump_passes, loopback_pump_rearms, loopback_pump_tid,
    loopback_pump_wake_already_awake, loopback_pump_wake_rejected, loopback_pump_wakes,
};

use alloc::vec::Vec;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

/// Per-context exclusion for the network tables.
///
/// This guard protects `TCP_CONNECTIONS`, `TCP_LISTENERS`, `SEQ_COUNTER`,
/// `DEFERRED_TX_QUEUE`, `ARP_CACHE`, `NET_CONFIG`, `LOOPBACK_QUEUE`,
/// `ARP_PENDING_QUEUE`, and `CURRENT_PACKET_SRC_MAC` from same-CPU re-entry. On
/// x86_64 it disables bottom halves rather than clearing IF: the hardirq handlers
/// do not touch these tables, while e1000 hardirq handling only raises NetRx and
/// the NetRx softirq is the sole interrupt-context table user. Drop performs the
/// `local_bh_enable` equivalent, dispatching pending softirqs only when the
/// complete per-CPU preempt count reaches zero, exactly as `irq_exit` already
/// does. On aarch64 it preserves the existing DAIF IRQ-mask semantics.
#[must_use = "bind the guard as `let _guard = net_lock_guard();` so it remains active"]
pub(crate) struct NetLockGuard {
    #[cfg(target_arch = "x86_64")]
    bh_disabled: bool,
    #[cfg(target_arch = "aarch64")]
    saved_daif: u64,
    _not_send_or_sync: PhantomData<*const ()>,
}

#[inline(always)]
pub(crate) fn net_lock_guard() -> NetLockGuard {
    #[cfg(target_arch = "x86_64")]
    {
        let bh_disabled = crate::per_cpu::is_initialized();
        if bh_disabled {
            crate::per_cpu::bh_disable();
        }
        NetLockGuard {
            bh_disabled,
            _not_send_or_sync: PhantomData,
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        let saved_daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) saved_daif, options(nomem, nostack));
            core::arch::asm!("msr daifset, #2", options(nomem, nostack));
        }
        NetLockGuard {
            saved_daif,
            _not_send_or_sync: PhantomData,
        }
    }
}

impl Drop for NetLockGuard {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            if !self.bh_disabled {
                return;
            }
            crate::per_cpu::bh_enable();
            if crate::per_cpu::preempt_count() == 0 && crate::per_cpu::softirq_pending() != 0 {
                crate::per_cpu::do_softirq();
            }
        }

        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!(
                "msr daif, {}",
                in(reg) self.saved_daif,
                options(nomem, nostack)
            );
        }
    }
}

/// Re-entrancy guard for process_rx() on aarch64. Prevents nested RX drains
/// when interrupt-driven NetRx preempts another RX processing context.
#[cfg(target_arch = "aarch64")]
static RX_PROCESSING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(target_arch = "aarch64")]
static RX_PENDING_WHILE_PROCESSING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

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
        if false {
            let _ = core::format_args!($($arg)*);
        }
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
const MAX_DRAIN_ROUNDS: usize = 16;
const LOOPBACK_TAKE_ATTEMPTS: usize = 64;
const MAX_ARP_PENDING_QUEUE_SIZE: usize = 16;
/// How long an IP packet may sit on the pending-ARP queue before
/// `enqueue_arp_pending_packet` retains it away.
///
/// #767 (ruling R176). This was written as a flat `5_000` and compared against
/// `get_monotonic_time()` while that function returned the raw tick counter, so
/// the retention window it has actually enforced since it was introduced
/// (efc2af57, 2026-05-31, long after PIT_HZ became 200 at c16faca1) is 5000
/// ticks: 25 s on x86_64 and 5 s on aarch64. #767 made the producer return real
/// milliseconds; keeping the bare literal would have cut the x86 window to a
/// fifth of the one every green networking run on this tree was measured with.
/// Scaling by `MS_PER_TICK` holds each arch at its measured window. Shortening
/// it to the 5 s the name suggests is a defensible change, but it is a
/// behaviour change with its own evidence to produce, not a side effect of a
/// units repair.
const ARP_PENDING_TTL_MS: u64 = 5_000 * crate::time::timer::MS_PER_TICK;

/// Loopback packet queue for deferred delivery
/// Packets sent to our own IP are queued here and delivered after the sender releases locks
struct LoopbackPacket {
    /// Raw IP packet data
    data: Vec<u8>,
    /// Monotonic timer tick at which this packet entered the queue.
    ///
    /// Delivery latency is the quantity #636 is about, so the queue carries the
    /// only timestamp from which it can be computed without a test harness.
    ///
    /// Ticks, not milliseconds, and read from `crate::time::get_ticks()` for
    /// that reason: on x86_64 the PIT runs at 200 Hz, so one tick is five
    /// milliseconds there and one millisecond on aarch64. Residency is
    /// reported in ticks and converted by the reader.
    ///
    /// #767 scaled `get_monotonic_time()` to real milliseconds. This field and
    /// its threshold are named in ticks and are read against the raw counter
    /// so that the number this queue reports keeps the meaning it had.
    queued_at_tick: u64,
}

static LOOPBACK_QUEUE: Mutex<Vec<LoopbackPacket>> = Mutex::new(Vec::new());
/// Single-writer-under-lock atomic mirror of `LOOPBACK_QUEUE.len()`.
/// Writers publish the new length immediately after each queue mutation.
static LOOPBACK_QUEUE_DEPTH: AtomicUsize = AtomicUsize::new(0);
static LOOPBACK_DRAIN_TICKET: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_DRAIN_OWNER: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_DRAIN_CONTENDED: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_DRAIN_TAKE_ABANDONED: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_DRAIN_COMPLETED: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_DROPPED_FULL: AtomicU64 = AtomicU64::new(0);
static IDLE_LOOPBACK_DRAIN_CALLS: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_PUMP_REARM_FROM_SCHED: AtomicU64 = AtomicU64::new(0);
/// Longest observed queue-to-delivery latency of a loopback packet, in ticks.
static LOOPBACK_MAX_RESIDENCY_TICKS: AtomicU64 = AtomicU64::new(0);
/// Deliveries whose residency exceeded `LOOPBACK_SLOW_DELIVERY_TICKS`.
static LOOPBACK_SLOW_DELIVERIES: AtomicU64 = AtomicU64::new(0);
/// Slow deliveries that have already emitted their census line.
static LOOPBACK_SLOW_REPORTS: AtomicU64 = AtomicU64::new(0);
/// Softirq drains that found the queue non-empty on entry.
static LOOPBACK_SOFTIRQ_DRAINS: AtomicU64 = AtomicU64::new(0);
/// Enqueues that raised the NetRx softirq to own their own delivery.
static LOOPBACK_SOFTIRQ_RAISES: AtomicU64 = AtomicU64::new(0);
/// Residency above which a delivery is reported rather than merely counted.
///
/// This is a diagnostic threshold, not a test bound: nothing fails because of
/// it, it only decides when the census line is emitted.
const LOOPBACK_SLOW_DELIVERY_TICKS: u64 = 50;
/// Number of slow deliveries that emit a census line before reporting goes
/// quiet, so a sustained stall cannot flood the serial line.
const LOOPBACK_SLOW_REPORT_LIMIT: u64 = 8;

/// Which context performed a drain, for slow-delivery attribution.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopbackDrainSource {
    Pump,
    Syscall,
    Idle,
    Softirq,
}

impl LoopbackDrainSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pump => "pump",
            Self::Syscall => "syscall",
            Self::Idle => "idle",
            Self::Softirq => "softirq",
        }
    }
}

pub fn loopback_max_residency_ticks() -> u64 {
    LOOPBACK_MAX_RESIDENCY_TICKS.load(Ordering::Relaxed)
}

pub fn loopback_softirq_drains() -> u64 {
    LOOPBACK_SOFTIRQ_DRAINS.load(Ordering::Relaxed)
}

pub fn loopback_softirq_raises() -> u64 {
    LOOPBACK_SOFTIRQ_RAISES.load(Ordering::Relaxed)
}

/// Hand the loopback queue to the NetRx softirq, which owns its delivery.
///
/// This is the enqueue's own kick, and the one that does not depend on any
/// scheduling decision: `irq_exit()` runs pending softirqs on the way out of
/// the next interrupt whenever the interrupted context was preemptible, so a
/// queued loopback packet is delivered within a timer tick of being queued
/// rather than whenever some unrelated thread next happens to drain.
#[inline]
pub(crate) fn kick_loopback_delivery() {
    LOOPBACK_SOFTIRQ_RAISES.fetch_add(1, Ordering::Relaxed);
    crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
}

pub fn loopback_slow_deliveries() -> u64 {
    LOOPBACK_SLOW_DELIVERIES.load(Ordering::Relaxed)
}

/// Record one packet's queue-to-delivery latency and report the outliers.
fn note_loopback_residency(queued_at_tick: u64, source: LoopbackDrainSource) {
    let now_tick = crate::time::get_ticks();
    let residency_ticks = now_tick.saturating_sub(queued_at_tick);

    LOOPBACK_MAX_RESIDENCY_TICKS.fetch_max(residency_ticks, Ordering::Relaxed);
    if residency_ticks <= LOOPBACK_SLOW_DELIVERY_TICKS {
        return;
    }

    LOOPBACK_SLOW_DELIVERIES.fetch_add(1, Ordering::Relaxed);
    if LOOPBACK_SLOW_REPORTS.fetch_add(1, Ordering::Relaxed) >= LOOPBACK_SLOW_REPORT_LIMIT {
        return;
    }

    crate::serial_println!(
        "LOOPBACK_SLOW_DELIVERY: residency_ticks={} source={} slow={}",
        residency_ticks,
        source.as_str(),
        LOOPBACK_SLOW_DELIVERIES.load(Ordering::Relaxed),
    );
    dump_loopback_state();
}

struct LoopbackDrainGuard {
    owner: u64,
}

impl LoopbackDrainGuard {
    fn acquire() -> Option<Self> {
        let owner = LOOPBACK_DRAIN_TICKET.fetch_add(1, Ordering::Relaxed) + 1;

        match LOOPBACK_DRAIN_OWNER.compare_exchange(0, owner, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => Some(Self { owner }),
            Err(_) => {
                LOOPBACK_DRAIN_CONTENDED.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }
}

impl Drop for LoopbackDrainGuard {
    fn drop(&mut self) {
        LOOPBACK_DRAIN_COMPLETED.fetch_add(1, Ordering::Relaxed);
        let _ = LOOPBACK_DRAIN_OWNER.compare_exchange(
            self.owner,
            0,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

pub fn loopback_drain_contended() -> u64 {
    LOOPBACK_DRAIN_CONTENDED.load(Ordering::Relaxed)
}

pub fn loopback_drain_completed() -> u64 {
    LOOPBACK_DRAIN_COMPLETED.load(Ordering::Relaxed)
}

pub fn loopback_drain_take_abandoned() -> u64 {
    LOOPBACK_DRAIN_TAKE_ABANDONED.load(Ordering::Relaxed)
}

pub fn loopback_dropped_full() -> u64 {
    LOOPBACK_DROPPED_FULL.load(Ordering::Relaxed)
}

fn loopback_queue_depth() -> usize {
    LOOPBACK_QUEUE_DEPTH.load(Ordering::Acquire)
}

pub fn loopback_queue_depth_for_test() -> usize {
    loopback_queue_depth()
}

/// True when the lock-free queue-depth mirror reports pending packets.
#[inline(always)]
pub(crate) fn loopback_queue_has_work() -> bool {
    LOOPBACK_QUEUE_DEPTH.load(Ordering::Acquire) != 0
}

/// True when the lock-free queue-depth mirror reports no pending packets.
pub(crate) fn loopback_queue_is_empty() -> bool {
    LOOPBACK_QUEUE_DEPTH.load(Ordering::Acquire) == 0
}

pub(crate) fn record_loopback_pump_rearm_from_sched() {
    LOOPBACK_PUMP_REARM_FROM_SCHED.fetch_add(1, Ordering::Relaxed);
}

pub fn loopback_pump_rearm_from_sched() -> u64 {
    LOOPBACK_PUMP_REARM_FROM_SCHED.load(Ordering::Relaxed)
}

/// IPv4 packets waiting for ARP resolution of their next hop.
struct PendingArpPacket {
    next_hop: [u8; 4],
    queued_at_ms: u64,
    ip_packet: Vec<u8>,
}

static ARP_PENDING_QUEUE: Mutex<Vec<PendingArpPacket>> = Mutex::new(Vec::new());

fn enqueue_arp_pending_packet(next_hop: [u8; 4], ip_packet: Vec<u8>) {
    let now_ms = crate::time::get_monotonic_time();
    let _guard = net_lock_guard();
    let mut queue = ARP_PENDING_QUEUE.lock();

    let cutoff_ms = now_ms.saturating_sub(ARP_PENDING_TTL_MS);
    queue.retain(|packet| packet.queued_at_ms >= cutoff_ms);
    if queue.len() >= MAX_ARP_PENDING_QUEUE_SIZE {
        queue.remove(0);
    }
    queue.push(PendingArpPacket {
        next_hop,
        queued_at_ms: now_ms,
        ip_packet,
    });

    drop(queue);
}

/// Send packets queued while `next_hop` was unresolved.
///
/// Called from ARP RX processing after the cache has been updated. The pending
/// queue is also touched by thread-context TX, so x86_64 disables bottom halves
/// and aarch64 masks IRQs to avoid same-CPU softirq re-entry deadlocks.
pub(crate) fn flush_arp_pending_packets(next_hop: &[u8; 4], mac: &[u8; 6]) {
    let now_ms = crate::time::get_monotonic_time();
    let packets = {
        let _guard = net_lock_guard();
        let mut queue = ARP_PENDING_QUEUE.lock();
        let packets = core::mem::take(&mut *queue);
        drop(queue);
        packets
    };

    if packets.is_empty() {
        return;
    }

    let cutoff_ms = now_ms.saturating_sub(ARP_PENDING_TTL_MS);
    let mut remaining = Vec::new();
    for packet in packets {
        if packet.queued_at_ms < cutoff_ms {
            continue;
        }

        if packet.next_hop == *next_hop {
            let _ = send_ethernet(mac, ethernet::ETHERTYPE_IPV4, &packet.ip_packet);
        } else {
            remaining.push(packet);
        }
    }

    if remaining.is_empty() {
        return;
    }

    let _guard = net_lock_guard();
    let mut queue = ARP_PENDING_QUEUE.lock();
    for packet in remaining {
        if queue.len() >= MAX_ARP_PENDING_QUEUE_SIZE {
            let mut oldest_idx = 0;
            for (idx, queued) in queue.iter().enumerate().skip(1) {
                if queued.queued_at_ms < queue[oldest_idx].queued_at_ms {
                    oldest_idx = idx;
                }
            }

            if queue[oldest_idx].queued_at_ms <= packet.queued_at_ms {
                queue.remove(oldest_idx);
            } else {
                continue;
            }
        }
        queue.push(packet);
    }
    drop(queue);
}

/// Result of one bounded attempt to take the queued loopback packets.
enum LoopbackTake {
    Packets(Vec<LoopbackPacket>),
    Empty,
    Contended,
}

/// Take one batch while excluding other takers. Delivery is deliberately
/// outside this microsecond-scale guarded window.
fn take_queued_loopback_packets() -> LoopbackTake {
    for _ in 0..LOOPBACK_TAKE_ATTEMPTS {
        if let Some(drain_guard) = LoopbackDrainGuard::acquire() {
            let net_guard = net_lock_guard();
            let mut queue = LOOPBACK_QUEUE.lock();
            let packets = core::mem::take(&mut *queue);
            LOOPBACK_QUEUE_DEPTH.store(queue.len(), Ordering::Release);
            drop(queue);
            drop(drain_guard);
            drop(net_guard);

            return if packets.is_empty() {
                LoopbackTake::Empty
            } else {
                LoopbackTake::Packets(packets)
            };
        }
        core::hint::spin_loop();
    }

    LOOPBACK_DRAIN_TAKE_ABANDONED.fetch_add(1, Ordering::Relaxed);
    LoopbackTake::Contended
}

/// Deliver at most `max_rounds` rounds of queued loopback packets.
///
/// Returns true when work remains so callers can re-arm without doing
/// unbounded delivery work in a single pass.
pub(crate) fn drain_loopback_rounds(max_rounds: usize, source: LoopbackDrainSource) -> bool {
    for _ in 0..max_rounds {
        let packets = match take_queued_loopback_packets() {
            LoopbackTake::Packets(packets) => packets,
            // An empty loopback queue does not mean there is no queued work:
            // TCP parks segments it could not send during RX processing on the
            // deferred-TX queue, and this drain is their only owner. Returning
            // early without flushing left them waiting for the next loopback
            // packet from somewhere else to carry them out.
            LoopbackTake::Empty => {
                tcp::drain_deferred_tx();
                return false;
            }
            // Another context is taking the batch. It will deliver what it took,
            // but anything queued behind it belongs to nobody once this call
            // returns, so hand it back to the softirq rather than dropping it.
            LoopbackTake::Contended => {
                tcp::drain_deferred_tx();
                return rearm_if_work_remains();
            }
        };

        for packet in packets {
            note_loopback_residency(packet.queued_at_tick, source);
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

        tcp::drain_deferred_tx();
    }

    // The round budget is spent. Whatever is still queued has no owner unless
    // this call names one, so re-raise the softirq that owns loopback delivery.
    rearm_if_work_remains()
}

/// Re-raise the delivery softirq when the queue still holds work.
///
/// Every path that stops draining with packets still queued goes through here,
/// so "work remains" and "someone is coming back for it" are the same fact.
fn rearm_if_work_remains() -> bool {
    if loopback_queue_is_empty() {
        return false;
    }
    kick_loopback_delivery();
    true
}

/// Drain the loopback queue, delivering any pending packets.
///
/// Called after syscalls release their locks to avoid deadlock. TCP loopback
/// can enqueue deferred replies (SYN+ACK, ACK) while delivering an earlier
/// packet, so drain bounded rounds until the local packet chain is quiescent.
pub fn drain_loopback_queue() {
    let _ = drain_loopback_rounds(MAX_DRAIN_ROUNDS, LoopbackDrainSource::Syscall);
}

/// Drain loopback delivery from a general thread-context idle backstop.
pub fn drain_loopback_from_idle() {
    IDLE_LOOPBACK_DRAIN_CALLS.fetch_add(1, Ordering::Relaxed);
    let _ = drain_loopback_rounds(MAX_DRAIN_ROUNDS, LoopbackDrainSource::Idle);
}

pub fn idle_loopback_drain_calls() -> u64 {
    IDLE_LOOPBACK_DRAIN_CALLS.load(Ordering::Relaxed)
}

/// Emit loopback queue, drain, pump, and ISR-buffer state for hang triage.
pub fn dump_loopback_state() {
    let pump_tid = loopback_pump_tid();
    crate::serial_println!(
        "loopback: depth={} drain_contended={} drain_take_abandoned={} drain_completed={} dropped_full={} max_residency_ticks={} slow_deliveries={} softirq_raises={} softirq_drains={} pump_tid={} pump_passes={} pump_rearms={} pump_rearm_from_sched={} pump_wakes={} pump_wake_rejected={} pump_wake_already_awake={} accept_publish_race_recovered={} isr_wakeup_depth_cpu0={} isr_wakeup_buffer_full={} stalled_reclaimed={}",
        loopback_queue_depth(),
        loopback_drain_contended(),
        loopback_drain_take_abandoned(),
        loopback_drain_completed(),
        loopback_dropped_full(),
        loopback_max_residency_ticks(),
        loopback_slow_deliveries(),
        loopback_softirq_raises(),
        loopback_softirq_drains(),
        pump_tid,
        loopback_pump_passes(),
        loopback_pump_rearms(),
        loopback_pump_rearm_from_sched(),
        loopback_pump_wakes(),
        loopback_pump_wake_rejected(),
        loopback_pump_wake_already_awake(),
        tcp::tcp_accept_publish_race_recovered(),
        crate::task::scheduler::isr_wakeup_depth(0),
        crate::task::scheduler::isr_wakeup_buffer_full(),
        crate::task::scheduler::enqueue_stalled_reclaimed(),
    );
    crate::task::scheduler::emit_wake_attribution_counters();
    if pump_tid != 0 {
        crate::task::scheduler::dump_thread_placement(pump_tid, "kloopbackd");
    }
}

/// Softirq handler for network RX processing.
/// Called from softirq context when NetRx softirq is raised by the network IRQ path.
///
/// PCI VirtIO uses a NAPI-shaped completion path: the IRQ handler suppresses
/// device callbacks and raises NetRx; this handler drains a bounded packet
/// budget and either re-enables callbacks or re-raises NetRx for more work.
fn net_rx_softirq_handler(_softirq: SoftirqType) {
    crate::tracing::providers::net_rx::count_softirq_entry();
    #[cfg(target_arch = "aarch64")]
    if net_pci::is_initialized() {
        net_pci::record_rx_softirq_entry_snapshot();
    }

    loop {
        let outcome = process_rx_budgeted(64);
        if outcome == PollOutcome::BudgetExhausted {
            crate::tracing::providers::counters::NET_RX_BUDGET_EXHAUSTED.increment();
        }

        #[cfg(target_arch = "aarch64")]
        if net_pci::is_initialized() {
            match outcome {
                PollOutcome::Drained => {
                    if net_pci::reenable_and_check_race() {
                        continue;
                    }
                }
                PollOutcome::BudgetExhausted => {
                    crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
                }
                PollOutcome::InProgress => {}
            }
        }

        #[cfg(target_arch = "aarch64")]
        if !net_pci::is_initialized() && outcome == PollOutcome::BudgetExhausted {
            crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
        }

        #[cfg(not(target_arch = "aarch64"))]
        match outcome {
            PollOutcome::Drained => {}
            PollOutcome::BudgetExhausted => {
                crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
            }
            PollOutcome::InProgress => {}
        }

        break;
    }

    // Loopback delivery is receive processing too, and this is the context that
    // owns it: the enqueue raises NetRx, and irq_exit() runs it on the way out
    // of the next interrupt. Draining here is what makes a queued loopback
    // packet's delivery scheduled by its own enqueue rather than by whichever
    // unrelated thread next reaches a syscall drain site.
    if loopback_queue_has_work() {
        LOOPBACK_SOFTIRQ_DRAINS.fetch_add(1, Ordering::Relaxed);
    }
    let _ = drain_loopback_rounds(MAX_DRAIN_ROUNDS, LoopbackDrainSource::Softirq);

    #[cfg(target_arch = "aarch64")]
    if net_pci::is_initialized() {
        net_pci::record_rx_softirq_exit_snapshot();
    }
    crate::tracing::providers::net_rx::count_softirq_exit();
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
        let _guard = net_lock_guard();
        let mut config = NET_CONFIG.lock();
        *config = PARALLELS_CONFIG;
        drop(config);
    } else if e1000::is_initialized() {
        crate::serial_println!("[net] Using Intel e1000 driver (VMware)");
        let _guard = net_lock_guard();
        let mut config = NET_CONFIG.lock();
        *config = VMWARE_CONFIG;
        drop(config);
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

    let _guard = net_lock_guard();
    let config = NET_CONFIG.lock();
    let ip = config.ip_addr;
    let gw = config.gateway;
    drop(config);
    drop(_guard);

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
            let bootstrap_outcome = process_rx_budgeted(64);
            if matches!(
                bootstrap_outcome,
                PollOutcome::BudgetExhausted | PollOutcome::InProgress
            ) {
                crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
            }
        } else {
            net_mmio::enable_net_irq();
        }

        // Substep 4 bootstrap plus Substep 6 hardening: synchronously clear
        // virtio RX callback suppression so the next inbound MSI can fire even
        // if softirqd has not run its first NetRx dispatch yet. The softirq
        // raise remains as a redundant path for any RX state already present.
        if net_pci::is_initialized() {
            for _ in 0..8 {
                if !net_pci::reenable_and_check_race() {
                    break;
                }
                match process_rx_budgeted(64) {
                    PollOutcome::Drained => {}
                    PollOutcome::BudgetExhausted | PollOutcome::InProgress => {
                        crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
                        break;
                    }
                }
            }
            net_log!("NET: synchronously cleared virtio callback suppression");
        }
        crate::task::softirqd::raise_softirq(SoftirqType::NetRx);
        net_log!("NET: pre-primed NetRx softirq for bootstrap callback re-enable");
    }
}

/// Get the current network configuration.
/// Uses bottom-half exclusion on x86_64 and IRQ masking on aarch64 to prevent
/// same-CPU re-entry from the network softirq.
pub fn config() -> NetConfig {
    let _guard = net_lock_guard();
    let c = *NET_CONFIG.lock();
    c
}

/// Result of a bounded network RX poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    Drained,
    BudgetExhausted,
    InProgress,
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

#[cfg(target_arch = "aarch64")]
pub fn rx_processing_held() -> bool {
    use core::sync::atomic::Ordering;
    RX_PROCESSING.load(Ordering::Acquire)
}

#[cfg(target_arch = "aarch64")]
pub fn rx_pending_while_processing() -> bool {
    use core::sync::atomic::Ordering;
    RX_PENDING_WHILE_PROCESSING.load(Ordering::Acquire)
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
        RX_PENDING_WHILE_PROCESSING.store(true, Ordering::Release);
        crate::tracing::providers::net_rx::count_reentrant_skip();
        return PollOutcome::InProgress;
    }

    reclaim_driver_tx_completed();

    let mut remaining = budget;
    let mut outcome = PollOutcome::Drained;

    // Try PCI driver first (Parallels), then e1000 (VMware), then MMIO (QEMU)
    if net_pci::is_initialized() {
        loop {
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
            if remaining == 0
                || (!RX_PENDING_WHILE_PROCESSING.swap(false, Ordering::AcqRel)
                    && !net_pci::rx_used_pending())
            {
                break;
            }
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

    crate::tracing::providers::net_rx::count_guard_release();
    RX_PROCESSING.store(false, Ordering::Release);
    outcome
}

/// Source MAC of the packet currently being processed (for response routing).
/// Set during process_packet, used by TCP to route SYN-ACK to the correct MAC.
static CURRENT_PACKET_SRC_MAC: Mutex<[u8; 6]> = Mutex::new([0; 6]);

/// Get the source MAC of the current incoming packet.
/// Thread-context TCP reads it while the NetRx softirq updates it, so exclude re-entry.
pub fn current_packet_src_mac() -> [u8; 6] {
    let _guard = net_lock_guard();
    let current_src_mac = CURRENT_PACKET_SRC_MAC.lock();
    let src_mac = *current_src_mac;
    drop(current_src_mac);
    src_mac
}

/// Process a received Ethernet frame
/// Source-MAC state is also read from thread context, so its NetRx write is guarded.
fn process_packet(data: &[u8]) {
    if let Some(frame) = ethernet::EthernetFrame::parse(data) {
        crate::tracing::providers::net_rx::count_frame();
        // Save source MAC so TCP can use it for SYN-ACK routing
        {
            let _guard = net_lock_guard();
            let mut current_src_mac = CURRENT_PACKET_SRC_MAC.lock();
            *current_src_mac = frame.src_mac;
            drop(current_src_mac);
        }
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
                crate::tracing::providers::net_rx::count_ethertype_other();
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
/// Loopback packets are queued from both thread context and the NetRx softirq.
pub fn send_ipv4(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Result<(), &'static str> {
    let config = config();

    // Check for loopback - sending to ourselves or to 127.x.x.x network
    if dst_ip == config.ip_addr || dst_ip[0] == 127 {
        net_debug!("NET: Loopback detected, queueing packet for deferred delivery");

        // Build IP packet
        let ip_packet = ipv4::Ipv4Packet::build(config.ip_addr, dst_ip, protocol, payload);

        // Queue for deferred delivery (to avoid deadlock with process manager lock)
        // The caller must call drain_loopback_queue() after releasing locks
        let (queue_len, dropped_oldest) = {
            let _guard = net_lock_guard();
            let mut queue = LOOPBACK_QUEUE.lock();

            // Drop oldest packet if queue is full to prevent unbounded memory growth
            let dropped_oldest = if queue.len() >= MAX_LOOPBACK_QUEUE_SIZE {
                queue.remove(0);
                LOOPBACK_DROPPED_FULL.fetch_add(1, Ordering::Relaxed);
                true
            } else {
                false
            };

            queue.push(LoopbackPacket {
                data: ip_packet,
                queued_at_tick: crate::time::get_ticks(),
            });
            let queue_len = queue.len();
            LOOPBACK_QUEUE_DEPTH.store(queue_len, Ordering::Release);
            drop(queue);
            (queue_len, dropped_oldest)
        };

        if dropped_oldest {
            net_warn!("NET: Loopback queue full, dropped oldest packet");
        }
        net_debug!("NET: Loopback packet queued (queue size: {})", queue_len);
        // Two kicks, deliberately: the softirq is the owner (it runs on the way
        // out of the next interrupt, needing no scheduling decision at all),
        // and kloopbackd remains the thread-context backstop for contexts the
        // softirq cannot reach.
        kick_loopback_delivery();
        crate::net::loopback_pump::wake_loopback_pump();

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
            let ip_packet = ipv4::Ipv4Packet::build(config.ip_addr, dst_ip, protocol, payload);
            enqueue_arp_pending_packet(next_hop, ip_packet);

            // ARP resolution is asynchronous: request the next-hop MAC and let
            // IRQ-driven NetRx populate the cache. The ARP handler flushes
            // packets queued above once the next-hop MAC is known.
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
