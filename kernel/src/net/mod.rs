//! Network stack for Breenix
//!
//! Implements a minimal network stack with:
//! - Ethernet frame parsing and construction
//! - ARP for IPv4 address resolution
//! - IPv4 packet handling
//! - ICMP echo (ping) request/reply

extern crate alloc;

pub mod arp;
pub mod dhcp;
#[cfg(target_arch = "x86_64")]
pub(crate) mod dns_probe;
pub mod ethernet;
pub mod icmp;
pub mod ipv4;
pub(crate) mod loopback_pump;
#[cfg(target_arch = "aarch64")]
mod rx_poll;

// TCP and UDP protocol implementations - architecture-independent
// The socket syscall layer handles arch-specific details
pub mod tcp;
pub mod udp;

pub use loopback_pump::{
    init_loopback_pump, loopback_pump_passes, loopback_pump_rearms, loopback_pump_tid,
    loopback_pump_wake_already_awake, loopback_pump_wake_rejected, loopback_pump_wakes,
};
#[cfg(target_arch = "aarch64")]
pub use rx_poll::init_rx_poll;

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

// Driver abstraction functions

/// Get the MAC address from the network device
fn get_mac_address() -> Option<[u8; 6]> {
    #[cfg(target_arch = "x86_64")]
    {
        crate::drivers::bcm5720::mac_address().or_else(e1000::mac_address)
            .or_else(crate::drivers::virtio::net_legacy::mac_address)
            .or_else(crate::drivers::rtl8139::mac_address)
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
        if crate::drivers::bcm5720::is_initialized() {
            return crate::drivers::bcm5720::transmit(data);
        }
        if crate::drivers::virtio::net_legacy::is_initialized() {
            return crate::drivers::virtio::net_legacy::transmit(data);
        }
        if crate::drivers::rtl8139::is_initialized() {
            return crate::drivers::rtl8139::transmit(data);
        }
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
    /// Subnet mask for routing and directed broadcast addresses.
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

/// Bridged VMware starts unconfigured until DHCP assigns an address.
#[allow(dead_code)] // Used conditionally when e1000 is active on VMware
pub const VMWARE_CONFIG: NetConfig = NetConfig {
    ip_addr: [0; 4],
    subnet_mask: [255, 255, 255, 0],
    gateway: [0; 4],
};

/// Select network config based on compile-time feature or default to SLIRP
/// Use VMNET_CONFIG when BREENIX_NET_MODE=vmnet is set at build time
#[cfg(feature = "vmnet")]
pub const DEFAULT_CONFIG: NetConfig = VMNET_CONFIG;

#[cfg(not(feature = "vmnet"))]
pub const DEFAULT_CONFIG: NetConfig = SLIRP_CONFIG;

static NET_CONFIG: Mutex<NetConfig> = Mutex::new(DEFAULT_CONFIG);

/// Bound both payload storage and metadata, including minimum-size IP packets.
const MAX_LOOPBACK_QUEUE_BYTES: usize = 1024 * 1024;
const MAX_DRAIN_ROUNDS: usize = 16;
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
/// fifth of the one the 2 green x86 networking runs in the #767 round were
/// measured with (docs/planning/green-program/timekeeping/767-FIX-2026-09-05.md).
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

impl Drop for LoopbackPacket {
    fn drop(&mut self) {
        loopback_work_done();
    }
}

static LOOPBACK_QUEUE: Mutex<Vec<LoopbackPacket>> = Mutex::new(Vec::new());
/// Loopback work not yet finished: queued or in-delivery loopback packets and
/// deferred TCP segments taken but not yet sent.
///
/// A packet or segment is counted when it is queued and uncounted when it is
/// dropped, after its delivery or send. Whatever handling it produces is queued,
/// and so counted, before that drop, so the count reaches zero only once the
/// whole chain a packet starts has been delivered. A single read of zero is
/// therefore quiescence; reading the queue depth and the delivery owner one
/// after the other is not, since a context between the two can queue work and
/// let go.
static LOOPBACK_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
/// Single-writer-under-lock atomic mirror of `LOOPBACK_QUEUE.len()`.
/// Writers publish the new length immediately after each queue mutation.
static LOOPBACK_QUEUE_BYTES: AtomicUsize = AtomicUsize::new(0);
static LOOPBACK_QUEUE_DEPTH: AtomicUsize = AtomicUsize::new(0);
static LOOPBACK_DRAIN_TICKET: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_DRAIN_OWNER: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_DRAIN_CONTENDED: AtomicU64 = AtomicU64::new(0);
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

/// Count one loopback packet or deferred segment as queued.
pub(crate) fn loopback_work_queued() {
    LOOPBACK_IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
}

/// Uncount one loopback packet or deferred segment once it has been handled.
pub(crate) fn loopback_work_done() {
    LOOPBACK_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
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

fn enqueue_arp_pending_packet(next_hop: [u8; 4], ip_packet: Vec<u8>) -> Result<(), TxError> {
    let now_ms = crate::time::get_monotonic_time();
    let _guard = net_lock_guard();
    let mut queue = ARP_PENDING_QUEUE.lock();

    let cutoff_ms = now_ms.saturating_sub(ARP_PENDING_TTL_MS);
    queue.retain(|packet| packet.queued_at_ms >= cutoff_ms);
    if queue.len() >= MAX_ARP_PENDING_QUEUE_SIZE {
        return Err(TxError::NoBufs);
    }
    queue.try_reserve(1).map_err(|_| TxError::NoBufs)?;
    queue.push(PendingArpPacket {
        next_hop,
        queued_at_ms: now_ms,
        ip_packet,
    });

    drop(queue);
    Ok(())
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
        LOOPBACK_QUEUE_BYTES.store(0, Ordering::Relaxed);
        drop(queue);
        packets
    };

    if packets.is_empty() {
        return;
    }

    let cutoff_ms = now_ms.saturating_sub(ARP_PENDING_TTL_MS);
    // Partition in place: RX needs no extra allocation to retain other hops.
    let mut remaining = packets;
    remaining.retain(|packet| {
        if packet.queued_at_ms < cutoff_ms {
            return false;
        }
        if packet.next_hop == *next_hop {
            send_ethernet(mac, ethernet::ETHERTYPE_IPV4, &packet.ip_packet).is_err()
        } else {
            true
        }
    });

    if remaining.is_empty() {
        return;
    }

    let _guard = net_lock_guard();
    let mut queue = ARP_PENDING_QUEUE.lock();
    for packet in remaining {
        if queue.len() >= MAX_ARP_PENDING_QUEUE_SIZE || queue.try_reserve(1).is_err() {
            break;
        }
        queue.push(packet);
    }
    drop(queue);
}

/// Take every queued packet. Only the context delivering them takes, so the
/// batches leave the queue in the order they are delivered.
fn take_queued_loopback_packets() -> Vec<LoopbackPacket> {
    let net_guard = net_lock_guard();
    let mut queue = LOOPBACK_QUEUE.lock();
    let packets = core::mem::take(&mut *queue);
    LOOPBACK_QUEUE_BYTES.store(0, Ordering::Relaxed);
    LOOPBACK_QUEUE_DEPTH.store(queue.len(), Ordering::Release);
    drop(queue);
    drop(net_guard);
    packets
}

/// Deliver at most `max_rounds` rounds of queued loopback packets.
///
/// Returns true when work remains so callers can re-arm without doing
/// unbounded delivery work in a single pass.
pub(crate) fn drain_loopback_rounds(max_rounds: usize, source: LoopbackDrainSource) -> bool {
    // One context delivers at a time, so packets reach the stack in the order
    // they were queued. With only the take guarded, two contexts could deliver
    // consecutive batches at once and the later batch overtake the earlier;
    // TCP, which keeps no out-of-order queue, then dropped each segment that
    // arrived ahead of the bytes before it and waited out a retransmission
    // timeout to send it again. Preemption stays off while the guard is held,
    // as it is in the softirq, so its holder is never switched out with the
    // queue left to nobody. Paired with the fence after the release below: a
    // packet queued before a failed acquire is seen by the holder's re-check.
    crate::per_cpu::preempt_disable();
    core::sync::atomic::fence(Ordering::SeqCst);
    let Some(delivering) = LoopbackDrainGuard::acquire() else {
        crate::per_cpu::preempt_enable();
        // The delivering context owns what is queued: it looks at the queue
        // again once it lets go. Re-raising the softirq here would spin when
        // that context is the one this interrupt came in on top of.
        tcp::drain_deferred_tx();
        return false;
    };
    // A socket whose last reference a delivered notice drops is released
    // later, not here with preemption off, where the release could block.
    let defer = crate::socket::object::DeferRelease::enter();

    for _ in 0..max_rounds {
        let packets = take_queued_loopback_packets();
        if packets.is_empty() {
            break;
        }

        for packet in packets {
            note_loopback_residency(packet.queued_at_tick, source);
            if let Some(parsed_ip) = ipv4::Ipv4Packet::parse(&packet.data) {
                ipv4::handle_loopback_ipv4(&parsed_ip);
            }
        }

        tcp::drain_deferred_tx();
    }

    drop(defer);
    drop(delivering);
    crate::per_cpu::preempt_enable();
    core::sync::atomic::fence(Ordering::SeqCst);

    // An empty loopback queue does not mean there is no queued work: TCP parks
    // segments it could not send during RX processing on the deferred-TX
    // queue, and this drain is their only owner.
    tcp::drain_deferred_tx();
    // What was queued while this context delivered, or is left once the round
    // budget is spent, has no owner unless this call names one, so re-raise
    // the softirq that owns loopback delivery.
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
///
/// A syscall's segments are delivered before it returns, as Linux delivers a
/// loopback segment before the send returns: a close whose FIN and final ACK
/// were still queued would leave its port held, and a bind right after it
/// would fail with EADDRINUSE. So this drains until no loopback work is in
/// flight (`LOOPBACK_IN_FLIGHT`), which covers the packets another context
/// is delivering and the replies their delivery queues. Work is only ever
/// held unqueued by a context running with preemption off (the delivery
/// guard holder, or `drain_deferred_tx`), so on another CPU it finishes, and
/// it is never below this call on the same CPU: the callers are thread
/// context.
pub fn drain_loopback_queue() {
    loop {
        let _ = drain_loopback_rounds(MAX_DRAIN_ROUNDS, LoopbackDrainSource::Syscall);
        if LOOPBACK_IN_FLIGHT.load(Ordering::Acquire) == 0 {
            return;
        }
        while LOOPBACK_DRAIN_OWNER.load(Ordering::Acquire) != 0
            && LOOPBACK_IN_FLIGHT.load(Ordering::Acquire) != 0
        {
            core::hint::spin_loop();
        }
        core::hint::spin_loop();
    }
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
        "loopback: depth={} drain_contended={} drain_completed={} dropped_full={} max_residency_ticks={} slow_deliveries={} softirq_raises={} softirq_drains={} pump_tid={} pump_passes={} pump_rearms={} pump_rearm_from_sched={} pump_wakes={} pump_wake_rejected={} pump_wake_already_awake={} isr_wakeup_depth_cpu0={} isr_wakeup_buffer_full={} stalled_reclaimed={}",
        loopback_queue_depth(),
        loopback_drain_contended(),
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

    if let Some(mac) = get_mac_address() {
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
    #[cfg(target_arch = "x86_64")]
    if uses_dhcp() {
        arp::init();
        net_log!("Network stack initialized; awaiting DHCP after carrier");
        return;
    }
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

    #[cfg(target_arch = "aarch64")]
    let gw = if active_tx_driver_is_e1000() {
        configure_from_dhcp().unwrap_or([0; 4])
    } else {
        gw
    };

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

/// VMware's adapter is bridged to the host's network, whose addressing only its
/// DHCP server knows. Without a valid lease it remains unconfigured.
#[cfg(target_arch = "aarch64")]
fn configure_from_dhcp() -> Option<[u8; 4]> {
    let mac = get_mac_address()?;
    net_log!("NET: Requesting a DHCP lease");
    let Some(lease) = dhcp::acquire(mac) else {
        net_log!("NET: No DHCP lease; interface remains unconfigured");
        return None;
    };
    let [a, b, c, d] = lease.ip_addr;
    let [m0, m1, m2, m3] = lease.subnet_mask;
    let [g0, g1, g2, g3] = lease.gateway;
    let [s0, s1, s2, s3] = lease.server;
    net_log!(
        "NET: DHCP lease {}.{}.{}.{} mask {}.{}.{}.{} gateway {}.{}.{}.{} from {}.{}.{}.{}",
        a, b, c, d, m0, m1, m2, m3, g0, g1, g2, g3, s0, s1, s2, s3
    );
    Some(lease.gateway)
}

/// Physical x86 and bridged VMware obtain their addressing from DHCP.
pub fn uses_dhcp() -> bool {
    #[cfg(target_arch = "x86_64")]
    { !crate::drivers::fw_cfg::present() }
    #[cfg(target_arch = "aarch64")]
    { active_tx_driver_is_e1000() }
}

/// Current configuration; DHCP interfaces lose their address at lease expiry.
pub fn config() -> NetConfig {
    if uses_dhcp() {
        return dhcp::lease().map(|lease| NetConfig {
            ip_addr: lease.ip_addr, subnet_mask: lease.subnet_mask, gateway: lease.gateway,
        }).unwrap_or(VMWARE_CONFIG);
    }
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

/// x86_64: set while one CPU drains the receive ring.
///
/// NetRx runs on whichever CPU raised it: the e1000 interrupt raises it on the
/// CPU that takes the IRQ, and a loopback enqueue on the CPU that queued the
/// packet. Two CPUs draining the ring at once would hand one connection's
/// segments to TCP out of order and concurrently, so one drains at a time, as
/// a NAPI context does on Linux.
#[cfg(target_arch = "x86_64")]
static X86_RX_DRAINING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// x86_64: a drain was asked for while another CPU held `X86_RX_DRAINING`.
/// The holder checks it after releasing and drains again, so the frame that
/// prompted the request is not left in the ring.
#[cfg(target_arch = "x86_64")]
static X86_RX_REQUESTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// The next frame from whichever controller is attached, copied into `buffer`.
#[cfg(target_arch = "x86_64")]
fn x86_receive(buffer: &mut [u8]) -> Option<usize> {
    if crate::drivers::bcm5720::is_initialized() {
        return crate::drivers::bcm5720::receive(buffer);
    }
    if crate::drivers::virtio::net_legacy::is_initialized() {
        return crate::drivers::virtio::net_legacy::receive(buffer);
    }
    if crate::drivers::rtl8139::is_initialized() {
        return crate::drivers::rtl8139::receive(buffer);
    }
    if !e1000::can_receive() {
        return None;
    }
    e1000::receive(buffer).ok()
}

/// Process incoming packets up to `budget` frames.
///
/// Returns `InProgress` when another CPU is draining; that CPU drains again
/// for this request before it stops.
#[cfg(target_arch = "x86_64")]
pub fn process_rx_budgeted(budget: u32) -> PollOutcome {
    use core::sync::atomic::Ordering::SeqCst;

    // Request, then claim. The holder releases, then reads the request, so
    // with sequentially consistent ordering one of the two always sees the
    // other: either this claim succeeds or the holder drains again.
    X86_RX_REQUESTED.store(true, SeqCst);
    let mut buffer = [0u8; 2048];
    let mut remaining = budget;
    loop {
        if X86_RX_DRAINING
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_err()
        {
            return PollOutcome::InProgress;
        }
        X86_RX_REQUESTED.store(false, SeqCst);

        while remaining > 0 {
            let Some(len) = x86_receive(&mut buffer) else {
                break;
            };
            process_packet(&buffer[..len]);
            remaining -= 1;
        }

        X86_RX_DRAINING.store(false, SeqCst);
        if remaining == 0 {
            return PollOutcome::BudgetExhausted;
        }
        if !X86_RX_REQUESTED.load(SeqCst) {
            return PollOutcome::Drained;
        }
    }
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
                    #[cfg(target_arch = "x86_64")]
                    if ip_packet.protocol == ipv4::PROTOCOL_UDP && ip_packet.dst_ip == config().ip_addr
                        && ip_packet.flags_fragment & 0x3fff == 0
                        && ipv4::internet_checksum(ip_packet.header) == 0 {
                        if let Some((header, payload)) = udp::UdpHeader::parse(ip_packet.payload) {
                            if (header.checksum == 0 || udp::udp_checksum(ip_packet.src_ip, ip_packet.dst_ip,
                                &ip_packet.payload[..header.length as usize]) == 0)
                                && dns_probe::receive(&ip_packet, &header, payload) { return; }
                        }
                    }
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
    if ethertype == ethernet::ETHERTYPE_IPV4 && ipv4::Ipv4Packet::parse(payload).is_none() {
        return Err("Invalid IPv4 packet");
    }
    if uses_dhcp() && ethertype == ethernet::ETHERTYPE_IPV4 {
        let ip = ipv4::Ipv4Packet::parse(payload).ok_or("Invalid IPv4 packet")?;
        // Queued packets and established sockets cannot retain an expired or
        // replaced source address. Discovery/DECLINE alone use 0.0.0.0.
        let discovery = ip.src_ip == [0; 4] && ip.dst_ip == [255; 4]
            && ip.protocol == ipv4::PROTOCOL_UDP
            && udp::UdpHeader::parse(ip.payload).is_some_and(|(h, _)| h.src_port == 68 && h.dst_port == 67);
        if !discovery && !dhcp::lease().is_some_and(|l| l.ip_addr == ip.src_ip) {
            return Err("No DHCP address");
        }
    }
    if payload.len() > ethernet::ETHERNET_MTU {
        return Err("Message too large");
    }
    let src_mac = get_mac_address().ok_or("Network device not initialized")?;

    let frame = ethernet::EthernetFrame::build(&src_mac, dst_mac, ethertype, payload)?;
    driver_transmit(&frame)
}

/// The local address a new flow to `dst` is sent from under `config`:
/// 127.0.0.1 to the loopback network, as Linux's route lookup chooses, else the
/// interface's.
pub fn source_ip_in(config: &NetConfig, dst: [u8; 4]) -> [u8; 4] {
    if dst[0] == 127 {
        [127, 0, 0, 1]
    } else {
        config.ip_addr
    }
}

/// The local address of an inbound packet sent to `dst`: the loopback address
/// it was sent to, else the interface's.
pub fn local_ip_of_inbound(config: &NetConfig, dst: [u8; 4]) -> [u8; 4] {
    if dst[0] == 127 {
        dst
    } else {
        config.ip_addr
    }
}

/// Send an IPv4 packet from the address `source_ip_in` picks for `dst_ip`.
pub fn send_ipv4(dst_ip: [u8; 4], protocol: u8, payload: &[u8]) -> Result<(), &'static str> {
    send_ipv4_from(
        source_ip_in(&config(), dst_ip),
        dst_ip,
        protocol,
        ipv4::DEFAULT_TTL,
        payload,
    )
}

/// Send an IPv4 packet from `src_ip`, a local address: a connection's own, for
/// a reply on a loopback address other than 127.0.0.1. `ttl` is the socket's
/// IP_TTL.
/// Loopback packets are queued from both thread context and the NetRx softirq.
pub fn send_ipv4_from(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    protocol: u8,
    ttl: u8,
    payload: &[u8],
) -> Result<(), &'static str> {
    ip_output(src_ip, dst_ip, protocol, ttl, payload).map_err(TxError::description)
}

/// IP output failures, with Linux ABI errno values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxError {
    NoRoute,
    NoBufs,
    MsgSize,
}
impl TxError {
    pub fn errno(self) -> i32 {
        match self {
            Self::NoRoute => crate::syscall::errno::ENETUNREACH,
            Self::NoBufs => 105, // ENOBUFS (Linux).
            Self::MsgSize => crate::syscall::errno::EMSGSIZE,
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::NoRoute => "No route to host",
            Self::NoBufs => "No buffer space available",
            Self::MsgSize => "Message too large",
        }
    }
}

pub fn is_local_addr(addr: [u8; 4]) -> bool {
    is_local_addr_in(&config(), addr)
}

pub fn is_broadcast_addr(addr: [u8; 4]) -> bool {
    is_broadcast_addr_in(&config(), addr)
}

pub fn is_local_addr_in(cfg: &NetConfig, addr: [u8; 4]) -> bool {
    addr[0] == 127 || addr == cfg.ip_addr
}
pub fn is_broadcast_addr_in(cfg: &NetConfig, addr: [u8; 4]) -> bool {
    addr == [255; 4] || (0..4).all(|i| addr[i] == (cfg.ip_addr[i] | !cfg.subnet_mask[i]))
}

fn enqueue_local_packet(ip_packet: Vec<u8>) -> Result<(), TxError> {
    {
        let _guard = net_lock_guard();
        let mut queue = LOOPBACK_QUEUE.lock();
        let charge = ip_packet.len() + core::mem::size_of::<LoopbackPacket>();
        let bytes = LOOPBACK_QUEUE_BYTES.load(Ordering::Relaxed);
        if charge > MAX_LOOPBACK_QUEUE_BYTES.saturating_sub(bytes) {
            LOOPBACK_DROPPED_FULL.fetch_add(1, Ordering::Relaxed);
            return Err(TxError::NoBufs);
        }
        queue.try_reserve(1).map_err(|_| TxError::NoBufs)?;
        loopback_work_queued();
        queue.push(LoopbackPacket {
            data: ip_packet,
            queued_at_tick: crate::time::get_ticks(),
        });
        LOOPBACK_QUEUE_BYTES.store(bytes + charge, Ordering::Relaxed);
        LOOPBACK_QUEUE_DEPTH.store(queue.len(), Ordering::Release);
    }
    kick_loopback_delivery();
    crate::net::loopback_pump::wake_loopback_pump();
    Ok(())
}

/// Never evict loopback packets. Accepted ARP misses are asynchronous success.
/// Callers drain loopback only after releasing their protocol locks. `ttl` is
/// the time to live the packet leaves with: the sending socket's IP_TTL.
pub fn ip_output(
    src: [u8; 4],
    dst: [u8; 4],
    protocol: u8,
    ttl: u8,
    payload: &[u8],
) -> Result<(), TxError> {
    if payload.len() > u16::MAX as usize - ipv4::IPV4_HEADER_MIN_SIZE {
        return Err(TxError::MsgSize);
    }
    let cfg = config();
    if is_local_addr_in(&cfg, dst) {
        return enqueue_local_packet(ipv4::Ipv4Packet::try_build(
            src, dst, protocol, ttl, payload,
        )?);
    }
    let broadcast = is_broadcast_addr_in(&cfg, dst);
    // No fragmentation: use the interface MTU and return EMSGSIZE.
    if payload.len() > ethernet::ETHERNET_MTU - ipv4::IPV4_HEADER_MIN_SIZE {
        return Err(TxError::MsgSize);
    }
    let same_subnet =
        (0..4).all(|i| dst[i] & cfg.subnet_mask[i] == cfg.ip_addr[i] & cfg.subnet_mask[i]);
    let next_hop = if same_subnet { dst } else { cfg.gateway };
    let ip_packet = ipv4::Ipv4Packet::try_build(src, dst, protocol, ttl, payload)?;
    if broadcast {
        enqueue_local_packet(ipv4::Ipv4Packet::try_build(
            src, dst, protocol, ttl, payload,
        )?)?;
    }
    let mac = if broadcast {
        ethernet::BROADCAST_MAC
    } else if let Some(mac) = arp::lookup(&next_hop) {
        mac
    } else {
        if get_mac_address().is_none() || next_hop == [0; 4] {
            return Err(TxError::NoRoute);
        }
        enqueue_arp_pending_packet(next_hop, ip_packet)?;
        // The queued packet is accepted even if this initial request encounters
        // device backpressure. A later ARP response will flush it.
        let _ = arp::request(&next_hop);
        return Ok(());
    };
    send_ethernet(&mac, ethernet::ETHERTYPE_IPV4, &ip_packet).map_err(|_| TxError::NoBufs)
}

/// Send an ICMP echo request (ping)
#[allow(dead_code)] // Public API
pub fn ping(dst_ip: [u8; 4]) -> Result<(), &'static str> {
    let icmp_packet = icmp::IcmpPacket::echo_request(1, 1, b"breenix ping");
    send_ipv4(dst_ip, ipv4::PROTOCOL_ICMP, &icmp_packet)
}
