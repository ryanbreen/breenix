//! TCP (Transmission Control Protocol) implementation
//!
//! Implements TCP packet parsing, construction, and connection state machine (RFC 793).
//!
//! Each connection keeps every byte it has not seen acknowledged in a bounded send
//! buffer and transmits against the peer's advertised window. Bytes IP output could
//! not take stay queued and go out again from `drain_deferred_tx`. Received bytes go
//! into a bounded receive buffer whose free space is the advertised window.
//!
//! A reset leaves an errno on the connection the way Linux's `tcp_reset` does:
//! ECONNREFUSED in SYN_SENT, EPIPE in CLOSE_WAIT, ECONNRESET otherwise. A close
//! with unread data sends RST instead of FIN (RFC 2525 2.17, as Linux does), and so
//! does data that arrives for a connection whose owner has closed it.
//!
//! The active closer waits in TIME_WAIT for 60 s (Linux's `TCP_TIMEWAIT_LEN`).
//! Expired TIME_WAIT entries, orphaned FIN_WAIT_2 entries past 60 s and closed
//! orphans are reaped from the connection table whenever the table is consulted for
//! a port or a new segment, at most once a second.

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use super::ipv4::{Ipv4Packet, PROTOCOL_TCP};
use crate::syscall::errno::{EAGAIN, ECONNREFUSED, ECONNRESET, EPIPE, ETIMEDOUT};

// ============================================================================
// Deferred TX queue: packets queued during RX processing for later sending.
// Parallels' VirtIO silently drops TX packets submitted while the RX ring
// is being consumed. We queue them and drain after process_rx() completes.
// ============================================================================

struct DeferredTx {
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    dst_mac: Option<[u8; 6]>,
    tcp_segment: Vec<u8>,
}

static DEFERRED_TX_QUEUE: Mutex<Vec<DeferredTx>> = Mutex::new(Vec::new());
static TCP_WAKE_REJECTED: AtomicU64 = AtomicU64::new(0);
static TCP_ACCEPT_PUBLISH_RACE_RECOVERED: AtomicU64 = AtomicU64::new(0);

/// Set when some connection holds output it could not send: data or a FIN that IP
/// output refused, or data a newly opened peer window lets out. `drain_deferred_tx`
/// takes it and runs every connection's output again.
static TX_RETRY: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "boot_tests")]
static FORCED_CONNECTION_LOOKUP_MISSES: AtomicU64 = AtomicU64::new(0);

pub fn tcp_wake_rejected() -> u64 {
    TCP_WAKE_REJECTED.load(Ordering::Relaxed)
}

pub fn tcp_accept_publish_race_recovered() -> u64 {
    TCP_ACCEPT_PUBLISH_RACE_RECOVERED.load(Ordering::Relaxed)
}

#[cfg(feature = "boot_tests")]
pub fn arm_forced_connection_lookup_miss(local_port: u16, remote_port: u16, count: u32) {
    let state = if count == 0 {
        0
    } else {
        ((local_port as u64) << 48) | ((remote_port as u64) << 32) | count as u64
    };
    FORCED_CONNECTION_LOOKUP_MISSES.store(state, Ordering::Release);
}

#[cfg(feature = "boot_tests")]
#[inline]
fn take_forced_connection_lookup_miss(conn_id: &ConnectionId) -> bool {
    let ports = ((conn_id.local_port as u64) << 48) | ((conn_id.remote_port as u64) << 32);
    FORCED_CONNECTION_LOOKUP_MISSES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
            let remaining = state as u32;
            if remaining == 0 || state & 0xffff_ffff_0000_0000 != ports {
                return None;
            }

            let next = remaining - 1;
            Some(if next == 0 { 0 } else { ports | next as u64 })
        })
        .is_ok()
}

#[cfg(not(feature = "boot_tests"))]
#[inline(always)]
fn take_forced_connection_lookup_miss(_: &ConnectionId) -> bool {
    false
}

/// Thread-context and NetRx TCP paths both enqueue, so exclude softirq re-entry.
fn queue_deferred_tx_with_mac(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    dst_mac: Option<[u8; 6]>,
    tcp_segment: Vec<u8>,
) {
    let _guard = super::net_lock_guard();
    let mut queue = DEFERRED_TX_QUEUE.lock();
    queue.push(DeferredTx {
        src_ip,
        dst_ip,
        dst_mac,
        tcp_segment,
    });
    drop(queue);
}

/// Thread context and the NetRx softirq inspect this queue, so exclude re-entry.
pub fn deferred_tx_queue_len() -> usize {
    let _guard = super::net_lock_guard();
    let queue = DEFERRED_TX_QUEUE.lock();
    let len = queue.len();
    drop(queue);
    len
}

/// Drain the deferred TX queue, sending all queued packets, then run the output of
/// every connection that holds unsent data or an unsent FIN.
/// Called after process_rx() completes and after each round of loopback delivery.
/// The queue is filled from thread context and NetRx, so guard only the queue take.
pub fn drain_deferred_tx() {
    let packets: Vec<DeferredTx> = {
        let _guard = super::net_lock_guard();
        let mut queue = DEFERRED_TX_QUEUE.lock();
        let packets = core::mem::take(&mut *queue);
        drop(queue);
        packets
    };

    for pkt in packets {
        let config = super::config();
        let is_loopback = pkt.dst_ip == config.ip_addr || pkt.dst_ip[0] == 127;
        // A segment that fails here was a SYN+ACK; the peer's SYN retransmission
        // brings another.
        let _ = if is_loopback {
            super::send_ipv4_from(pkt.src_ip, pkt.dst_ip, PROTOCOL_TCP, &pkt.tcp_segment)
        } else if let Some(mac) = pkt.dst_mac {
            // Send directly to the known MAC (bypasses ARP, which can't work
            // during process_rx since the re-entrancy guard is held).
            let ip_packet = super::ipv4::Ipv4Packet::build(
                pkt.src_ip,
                pkt.dst_ip,
                PROTOCOL_TCP,
                &pkt.tcp_segment,
            );
            super::send_ethernet(&mac, super::ethernet::ETHERTYPE_IPV4, &ip_packet)
        } else {
            super::send_ipv4_from(pkt.src_ip, pkt.dst_ip, PROTOCOL_TCP, &pkt.tcp_segment)
        };
    }

    if TX_RETRY.swap(false, Ordering::AcqRel) {
        let stalled = with_tcp_connections(|connections| {
            let mut stalled = false;
            for conn in connections.values_mut() {
                if conn.has_output() && !conn.output() {
                    stalled = true;
                }
            }
            stalled
        });
        if stalled {
            TX_RETRY.store(true, Ordering::Release);
        }
    }
}

/// TCP header minimum size (without options)
pub const TCP_HEADER_MIN_SIZE: usize = 20;

/// TCP option kinds (RFC 793, RFC 9293 3.2).
const TCP_OPT_END: u8 = 0;
const TCP_OPT_NOP: u8 = 1;
const TCP_OPT_MSS: u8 = 2;

/// Maximum segment size over Ethernet: a 1500-byte MTU less the IPv4 and TCP headers.
pub const ETHERNET_MSS: u16 = 1460;
/// Maximum segment size on loopback. Loopback carries IPv4 packets of any size, so
/// this only balances per-segment cost against the window: four full segments fit
/// in the largest window a connection advertises.
pub const LOOPBACK_MSS: u16 = 16384;
/// The peer's MSS when its SYN carries no MSS option (RFC 9293 3.7.1).
const DEFAULT_PEER_MSS: u16 = 536;

/// Receive buffer of a connection: the largest window a 16-bit window field can
/// advertise without window scaling.
pub const DEFAULT_RCVBUF: usize = 65535;
/// Send buffer of a connection: unacknowledged and unsent bytes together.
pub const DEFAULT_SNDBUF: usize = 65536;

/// How long the active closer waits in TIME_WAIT: Linux's `TCP_TIMEWAIT_LEN`, 60 s.
pub const TIME_WAIT_MS: u64 = 60_000;
/// How long a connection whose owner has closed it may wait in FIN_WAIT_2 for the
/// peer's FIN: Linux's default `tcp_fin_timeout`, 60 s.
const ORPHAN_FIN_WAIT2_MS: u64 = 60_000;
/// The connection table is swept for expired entries at most this often.
const REAP_INTERVAL_MS: u64 = 1_000;

/// Linux's SOMAXCONN, the largest backlog a listener takes.
const MAX_BACKLOG: usize = 4096;

/// The retransmission timeout before any round trip is measured (RFC 6298 2.1).
const RTO_INITIAL_MS: u64 = 1_000;
/// Bounds on the retransmission timeout: Linux's `TCP_RTO_MIN` and `TCP_RTO_MAX`.
const RTO_MIN_MS: u64 = 200;
const RTO_MAX_MS: u64 = 120_000;
/// The clock granularity term of RFC 6298 2.2.
const RTO_GRANULARITY_MS: u64 = 1;
/// Timeouts tolerated before a connection gives up with ETIMEDOUT: Linux's
/// `tcp_retries2`, and `tcp_syn_retries` during the handshake.
const MAX_RETRIES: u32 = 15;
const MAX_SYN_RETRIES: u32 = 6;

static LAST_REAP_MS: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// Timers: one kernel thread, `ktcptimerd`, sleeps until the earliest deadline of
// any connection (a retransmission timeout, the end of TIME_WAIT, an orphan's
// FIN_WAIT_2 limit) and runs the timers that are due.
// ============================================================================

static TIMER_THREAD: spin::Once<u64> = spin::Once::new();
/// The deadline `ktcptimerd` sleeps until, u64::MAX when it sleeps with none.
static TIMER_SLEEP_UNTIL_MS: AtomicU64 = AtomicU64::new(u64::MAX);
/// Set when a deadline earlier than the one slept for was armed.
static TIMER_KICK: AtomicBool = AtomicBool::new(false);

/// Start `ktcptimerd` if it is not running. Thread context, no TCP lock held.
fn ensure_timer_thread() {
    TIMER_THREAD.call_once(|| {
        crate::task::kthread::kthread_run(tcp_timer_thread, "ktcptimerd")
            .map(|handle| handle.tid())
            .unwrap_or(0)
    });
}

/// A connection armed a timer for `deadline`: wake `ktcptimerd` if it sleeps
/// past it. Called with the connection table held, from thread or softirq.
fn note_timer_deadline(deadline: u64) {
    if deadline >= TIMER_SLEEP_UNTIL_MS.load(Ordering::Acquire) {
        return;
    }
    TIMER_KICK.store(true, Ordering::Release);
    if let Some(&tid) = TIMER_THREAD.get() {
        if tid != 0 {
            crate::task::scheduler::wake_thread_any_context(tid);
        }
    }
}

/// Run every connection timer that is due, and reap what has expired. Returns
/// the earliest deadline still pending, published under the table lock as the
/// one `ktcptimerd` sleeps until so that an earlier arm finds it.
fn run_timers() -> Option<u64> {
    with_tcp_connections(|connections| {
        reap_expired(connections, true);
        let now = now_ms();
        let mut next: Option<u64> = None;
        for conn in connections.values_mut() {
            if conn.rtx_deadline_ms != 0 && now >= conn.rtx_deadline_ms {
                conn.retransmit_timeout(now);
            }
            if let Some(deadline) = conn.next_deadline() {
                next = Some(next.map_or(deadline, |n: u64| n.min(deadline)));
            }
        }
        TIMER_SLEEP_UNTIL_MS.store(next.unwrap_or(u64::MAX), Ordering::Release);
        next
    })
}

fn tcp_timer_thread() {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    loop {
        TIMER_KICK.store(false, Ordering::Release);
        let next = run_timers();
        // Retransmissions over loopback are queued for delivery; deliver them
        // and send what the deferred queue holds.
        crate::net::drain_loopback_queue();

        crate::arch_without_interrupts(|| {
            crate::task::scheduler::with_scheduler(|sched| match next {
                Some(deadline) => {
                    sched.block_current_for_timer(deadline.saturating_mul(1_000_000))
                }
                None => sched.block_current(),
            });
            if TIMER_KICK.load(Ordering::Acquire) {
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.unblock(tid);
                });
            }
        });
        // A halting waiter keeps its CPU until it yields: yield before halting.
        crate::task::scheduler::yield_current();
        crate::arch_halt_with_interrupts();
    }
}

/// `a` precedes `b` in sequence space (RFC 9293 3.4).
#[inline]
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// Monotonic milliseconds, on the clock the scheduler's timed sleeps use.
#[inline]
fn now_ms() -> u64 {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs.saturating_mul(1_000).saturating_add(nanos / 1_000_000)
}

/// TCP flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpFlags {
    pub fin: bool,
    pub syn: bool,
    pub rst: bool,
    pub psh: bool,
    pub ack: bool,
    pub urg: bool,
}

impl TcpFlags {
    /// Parse flags from the flags byte
    pub fn from_byte(byte: u8) -> Self {
        TcpFlags {
            fin: byte & 0x01 != 0,
            syn: byte & 0x02 != 0,
            rst: byte & 0x04 != 0,
            psh: byte & 0x08 != 0,
            ack: byte & 0x10 != 0,
            urg: byte & 0x20 != 0,
        }
    }

    /// Convert flags to byte
    pub fn to_byte(&self) -> u8 {
        let mut byte = 0u8;
        if self.fin {
            byte |= 0x01;
        }
        if self.syn {
            byte |= 0x02;
        }
        if self.rst {
            byte |= 0x04;
        }
        if self.psh {
            byte |= 0x08;
        }
        if self.ack {
            byte |= 0x10;
        }
        if self.urg {
            byte |= 0x20;
        }
        byte
    }

    /// Create SYN flag set
    pub fn syn() -> Self {
        TcpFlags {
            fin: false,
            syn: true,
            rst: false,
            psh: false,
            ack: false,
            urg: false,
        }
    }

    /// Create SYN+ACK flag set
    pub fn syn_ack() -> Self {
        TcpFlags {
            fin: false,
            syn: true,
            rst: false,
            psh: false,
            ack: true,
            urg: false,
        }
    }

    /// Create ACK flag set
    pub fn ack() -> Self {
        TcpFlags {
            fin: false,
            syn: false,
            rst: false,
            psh: false,
            ack: true,
            urg: false,
        }
    }

    /// Create ACK+PSH flag set (for data)
    pub fn ack_psh() -> Self {
        TcpFlags {
            fin: false,
            syn: false,
            rst: false,
            psh: true,
            ack: true,
            urg: false,
        }
    }

    /// Create FIN+ACK flag set
    pub fn fin_ack() -> Self {
        TcpFlags {
            fin: true,
            syn: false,
            rst: false,
            psh: false,
            ack: true,
            urg: false,
        }
    }

    /// Create RST flag set
    pub fn rst() -> Self {
        TcpFlags {
            fin: false,
            syn: false,
            rst: true,
            psh: false,
            ack: false,
            urg: false,
        }
    }

    /// Create RST+ACK flag set: the reset a synchronized connection sends.
    pub fn rst_ack() -> Self {
        TcpFlags {
            fin: false,
            syn: false,
            rst: true,
            psh: false,
            ack: true,
            urg: false,
        }
    }
}

/// Parsed TCP header
///
/// All fields are parsed from the TCP header for completeness and protocol conformance.
#[derive(Debug)]
#[allow(dead_code)] // All TCP header fields are part of RFC 793 protocol structure
pub struct TcpHeader {
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
    /// Sequence number
    pub seq_num: u32,
    /// Acknowledgment number
    pub ack_num: u32,
    /// Data offset in 32-bit words (header length)
    pub data_offset: u8,
    /// TCP flags
    pub flags: TcpFlags,
    /// Window size
    pub window_size: u16,
    /// Checksum
    pub checksum: u16,
    /// Urgent pointer
    pub urgent_ptr: u16,
    /// The MSS option's value, when the segment carries one.
    pub mss_option: Option<u16>,
    /// Bytes of data after the header.
    pub payload_len: usize,
}

impl TcpHeader {
    /// Parse a TCP header from raw bytes
    pub fn parse(data: &[u8]) -> Option<(Self, &[u8])> {
        if data.len() < TCP_HEADER_MIN_SIZE {
            return None;
        }

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);
        let seq_num = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ack_num = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        // Data offset is upper 4 bits of byte 12, in 32-bit words
        let data_offset = (data[12] >> 4) & 0x0F;
        let header_len = (data_offset as usize) * 4;

        if header_len < TCP_HEADER_MIN_SIZE || header_len > data.len() {
            return None;
        }

        let flags = TcpFlags::from_byte(data[13]);
        let window_size = u16::from_be_bytes([data[14], data[15]]);
        let checksum = u16::from_be_bytes([data[16], data[17]]);
        let urgent_ptr = u16::from_be_bytes([data[18], data[19]]);
        let mss_option = parse_mss_option(&data[TCP_HEADER_MIN_SIZE..header_len]);

        let payload = &data[header_len..];

        Some((
            TcpHeader {
                src_port,
                dst_port,
                seq_num,
                ack_num,
                data_offset,
                flags,
                window_size,
                checksum,
                urgent_ptr,
                mss_option,
                payload_len: payload.len(),
            },
            payload,
        ))
    }
}

/// The MSS option among a header's options, if present and well formed.
fn parse_mss_option(mut options: &[u8]) -> Option<u16> {
    while let Some(&kind) = options.first() {
        match kind {
            TCP_OPT_END => return None,
            TCP_OPT_NOP => options = &options[1..],
            _ => {
                let len = *options.get(1)? as usize;
                if len < 2 || len > options.len() {
                    return None;
                }
                if kind == TCP_OPT_MSS && len == 4 {
                    return Some(u16::from_be_bytes([options[2], options[3]]));
                }
                options = &options[len..];
            }
        }
    }
    None
}

/// Build a TCP packet (header + payload)
pub fn build_tcp_packet(
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    flags: TcpFlags,
    window_size: u16,
    payload: &[u8],
) -> Vec<u8> {
    let header_len = TCP_HEADER_MIN_SIZE; // No options for now
    let data_offset = (header_len / 4) as u8;

    let mut packet = Vec::with_capacity(header_len + payload.len());

    // Source port
    packet.extend_from_slice(&src_port.to_be_bytes());
    // Destination port
    packet.extend_from_slice(&dst_port.to_be_bytes());
    // Sequence number
    packet.extend_from_slice(&seq_num.to_be_bytes());
    // Acknowledgment number
    packet.extend_from_slice(&ack_num.to_be_bytes());
    // Data offset (4 bits) + reserved (4 bits)
    packet.push(data_offset << 4);
    // Flags
    packet.push(flags.to_byte());
    // Window size
    packet.extend_from_slice(&window_size.to_be_bytes());
    // Checksum (placeholder, will be calculated)
    packet.extend_from_slice(&0u16.to_be_bytes());
    // Urgent pointer
    packet.extend_from_slice(&0u16.to_be_bytes());

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Add `data` to a ones'-complement sum as big-endian 16-bit words (RFC 1071).
fn checksum_words(mut sum: u64, data: &[u8]) -> u64 {
    let mut words = data.chunks_exact(2);
    for word in &mut words {
        sum += u16::from_be_bytes([word[0], word[1]]) as u64;
    }
    if let [last] = words.remainder() {
        sum += (*last as u64) << 8;
    }
    sum
}

/// Calculate TCP checksum with pseudo-header
pub fn tcp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], tcp_packet: &[u8]) -> u16 {
    let mut sum = checksum_words(0, &src_ip);
    sum = checksum_words(sum, &dst_ip);
    sum += PROTOCOL_TCP as u64;
    sum += tcp_packet.len() as u64 & 0xFFFF;
    sum = checksum_words(sum, tcp_packet);
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Build a TCP packet with correct checksum
pub fn build_tcp_packet_with_checksum(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    flags: TcpFlags,
    window_size: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = build_tcp_packet(
        src_port,
        dst_port,
        seq_num,
        ack_num,
        flags,
        window_size,
        payload,
    );

    // Calculate checksum
    let checksum = tcp_checksum(src_ip, dst_ip, &packet);

    // Insert checksum at offset 16-17
    packet[16] = (checksum >> 8) as u8;
    packet[17] = (checksum & 0xFF) as u8;

    packet
}

/// The addressing and control fields of one outgoing segment.
#[derive(Clone, Copy)]
struct Segment {
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: TcpFlags,
    window: u16,
}

/// Build `seg` with `options` (a multiple of four bytes) and the payload `parts`
/// joined, checksummed. None when the memory for it cannot be had: data segments
/// are built on paths a user drives, so their allocation is fallible.
fn try_build_segment(seg: &Segment, options: &[u8], parts: [&[u8]; 2]) -> Option<Vec<u8>> {
    let header_len = TCP_HEADER_MIN_SIZE + options.len();
    let mut packet = Vec::new();
    packet
        .try_reserve_exact(header_len + parts[0].len() + parts[1].len())
        .ok()?;
    packet.extend_from_slice(&seg.src_port.to_be_bytes());
    packet.extend_from_slice(&seg.dst_port.to_be_bytes());
    packet.extend_from_slice(&seg.seq.to_be_bytes());
    packet.extend_from_slice(&seg.ack.to_be_bytes());
    packet.push(((header_len / 4) as u8) << 4);
    packet.push(seg.flags.to_byte());
    packet.extend_from_slice(&seg.window.to_be_bytes());
    packet.extend_from_slice(&[0, 0, 0, 0]);
    packet.extend_from_slice(options);
    packet.extend_from_slice(parts[0]);
    packet.extend_from_slice(parts[1]);
    let checksum = tcp_checksum(seg.src_ip, seg.dst_ip, &packet);
    packet[16..18].copy_from_slice(&checksum.to_be_bytes());
    Some(packet)
}

/// Hand `seg` to IP output. False when it was not sent: no memory for it, or IP
/// output refused it.
fn transmit(seg: &Segment, options: &[u8], parts: [&[u8]; 2]) -> bool {
    match try_build_segment(seg, options, parts) {
        Some(packet) => {
            super::send_ipv4_from(seg.src_ip, seg.dst_ip, PROTOCOL_TCP, &packet).is_ok()
        }
        None => false,
    }
}

/// The MSS option bytes advertising `mss`.
fn mss_option(mss: u16) -> [u8; 4] {
    let [hi, lo] = mss.to_be_bytes();
    [TCP_OPT_MSS, 4, hi, lo]
}

/// The MSS this host offers for a connection with `remote_ip`.
fn local_mss(remote_ip: [u8; 4]) -> u16 {
    let config = super::config();
    if remote_ip[0] == 127 || remote_ip == config.ip_addr {
        LOOPBACK_MSS
    } else {
        ETHERNET_MSS
    }
}

/// The MSS to send with: this host's, capped by what the peer's SYN offered.
fn effective_mss(remote_ip: [u8; 4], peer_mss: Option<u16>) -> u16 {
    local_mss(remote_ip)
        .min(peer_mss.unwrap_or(DEFAULT_PEER_MSS))
        .max(1)
}

/// TCP connection state (RFC 793)
///
/// All variants are part of the complete TCP state machine as defined in RFC 793.
/// Some states may not be actively used yet as the implementation matures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // RFC 793 state machine - all states are part of the complete protocol
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// Connection identifier (4-tuple)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionId {
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
}

/// TCP connection state
pub struct TcpConnection {
    pub id: ConnectionId,
    pub state: TcpState,
    /// Our sequence number (next byte to send)
    pub send_next: u32,
    /// Initial send sequence number (RFC 793 - needed for retransmission and RST validation)
    #[allow(dead_code)]
    // Part of RFC 793 state machine, needed for future retransmission logic
    pub send_initial: u32,
    /// Send unacknowledged (oldest unacked seq)
    pub send_unack: u32,
    /// Remote's sequence number (next byte expected)
    pub recv_next: u32,
    /// Initial receive sequence number
    pub recv_initial: u32,
    /// The window this end last advertised.
    pub recv_window: u16,
    /// Remote's window size
    pub send_window: u16,
    /// Pending data to receive.
    ///
    /// #693: private, and written only by `publish_rx`. The instant bytes enter
    /// this buffer is the readiness-publication instant a blocking `poll()` has
    /// to be scored against, so there is exactly one place that appends to it
    /// and exactly one place that stamps `rx_publish_ns`. Making the field
    /// private is what keeps those two facts from drifting apart: a new append
    /// site outside this module does not compile.
    rx_buffer: VecDeque<u8>,
    /// Monotonic nanoseconds at which `publish_rx` last put bytes into
    /// `rx_buffer`; `0` before the first publication on this connection.
    rx_publish_ns: u64,
    /// Every byte from `send_unack` on: the first `tx_sent` are on the wire
    /// unacknowledged, the rest are not yet sent.
    tx_buffer: VecDeque<u8>,
    /// Bytes at the front of `tx_buffer` that have been sent.
    tx_sent: usize,
    /// Maximum segment size
    pub mss: u16,
    /// Process ID that owns this connection (for cleanup on process exit)
    #[allow(dead_code)] // Needed for connection ownership tracking
    pub owner_pid: crate::process::process::ProcessId,
    /// True if SHUT_WR was called (no more sending)
    pub send_shutdown: bool,
    /// True if SHUT_RD was called (no more receiving)
    pub recv_shutdown: bool,
    /// Reference count for fork() support - connection is only closed when last fd is closed
    pub refcount: core::sync::atomic::AtomicUsize,
    /// Threads waiting for recv data or connection state change (connect/recv blocking)
    pub waiting_threads: Mutex<Vec<u64>>,
    /// The errno a reset left, reported once by the next operation that fails on it.
    pub error: Option<i32>,
    /// Bytes the receive buffer holds at most.
    rcv_cap: usize,
    /// Bytes the send buffer holds at most.
    snd_cap: usize,
    /// The owner has finished sending: a FIN follows the queued data.
    fin_queued: bool,
    /// The sequence number our FIN occupies, once it has been sent.
    fin_seq: Option<u32>,
    /// The peer acknowledged the FIN.
    fin_acked: bool,
    /// The highest sequence number sent: `send_next` falls back to `send_unack`
    /// when a retransmission timeout resends, and an ACK up to here is valid.
    send_max: u32,
    /// Retransmission timeout (RFC 6298).
    rto_ms: u64,
    /// Smoothed round-trip time and its variation, 0 before the first sample.
    srtt_ms: u64,
    rttvar_ms: u64,
    /// The segment being timed: the sequence number its ACK reaches, and when it
    /// was sent. Retransmitted segments are never timed (Karn's rule).
    rtt_probe: Option<(u32, u64)>,
    /// When the retransmission timer fires, 0 when it is not running.
    rtx_deadline_ms: u64,
    /// Consecutive timeouts without progress.
    retries: u32,
    /// The peer's FIN arrived: after the queued bytes, the stream is at its end.
    peer_fin: bool,
    /// No descriptor refers to the connection any more.
    orphaned: bool,
    /// Monotonic ms at which the connection entered TIME_WAIT, or at which an
    /// orphan began waiting in FIN_WAIT_2.
    timer_since_ms: u64,
}

impl TcpConnection {
    /// The one path by which received bytes enter `rx_buffer`, and the one
    /// place the publication instant is stamped.
    ///
    /// #693 turned on whether readiness had been published inside a blocking
    /// `poll()`'s window or only after it expired, and the kernel held no
    /// record that could answer it: the buffer records what arrived, not when.
    /// This records when, so `sys_poll` can state the answer from kernel state
    /// at the moment it gives up instead of leaving it to be reconstructed from
    /// two userspace stamps and the ordering of console prints.
    ///
    /// An empty payload is not a publication and does not move the stamp: TCP
    /// segments carrying only flags reach several of the call sites, and a
    /// stamp moved by one of those would report a readiness no byte created.
    ///
    /// Returns how many bytes went in: at most the free receive space, and fewer
    /// when memory for them cannot be had.
    pub fn publish_rx(&mut self, bytes: &[u8]) -> usize {
        let take = bytes.len().min(self.rcv_space());
        if take == 0 || self.rx_buffer.try_reserve(take).is_err() {
            return 0;
        }
        self.rx_buffer.extend(&bytes[..take]);
        let (secs, nanos) = crate::time::get_monotonic_time_ns();
        self.rx_publish_ns = secs.saturating_mul(1_000_000_000).saturating_add(nanos);
        take
    }

    /// True when the receive buffer holds no bytes. This is the predicate
    /// `poll()` reports `POLLIN` from.
    pub fn rx_is_empty(&self) -> bool {
        self.rx_buffer.is_empty()
    }

    /// Bytes currently held in the receive buffer.
    pub fn rx_len(&self) -> usize {
        self.rx_buffer.len()
    }

    /// Monotonic nanoseconds of the most recent `publish_rx`, or `0` before
    /// the first publication on this connection.
    pub fn rx_publish_ns(&self) -> u64 {
        self.rx_publish_ns
    }

    /// True once the peer's FIN has arrived.
    pub fn peer_fin(&self) -> bool {
        self.peer_fin
    }

    /// Bytes the send buffer can take now.
    pub fn tx_space(&self) -> usize {
        self.snd_cap.saturating_sub(self.tx_buffer.len())
    }

    /// Bytes queued in the send buffer, sent or not.
    pub fn tx_len(&self) -> usize {
        self.tx_buffer.len()
    }

    /// Set the receive and send buffer sizes, as SO_RCVBUF and SO_SNDBUF do.
    pub fn set_buffer_sizes(&mut self, rcvbuf: usize, sndbuf: usize) {
        self.rcv_cap = rcvbuf.max(1);
        self.snd_cap = sndbuf.max(1);
    }

    /// Create a new connection in SYN_SENT state (for connect)
    pub fn new_outgoing(
        id: ConnectionId,
        initial_seq: u32,
        owner_pid: crate::process::process::ProcessId,
    ) -> Self {
        TcpConnection {
            id,
            state: TcpState::SynSent,
            send_next: initial_seq.wrapping_add(1), // SYN consumes a sequence number
            send_initial: initial_seq,
            send_unack: initial_seq,
            recv_next: 0,
            recv_initial: 0,
            recv_window: DEFAULT_RCVBUF.min(u16::MAX as usize) as u16,
            send_window: 0,
            rx_buffer: VecDeque::new(),
            rx_publish_ns: 0,
            tx_buffer: VecDeque::new(),
            tx_sent: 0,
            mss: ETHERNET_MSS,
            owner_pid,
            send_shutdown: false,
            recv_shutdown: false,
            refcount: core::sync::atomic::AtomicUsize::new(1),
            waiting_threads: Mutex::new(Vec::new()),
            error: None,
            rcv_cap: DEFAULT_RCVBUF,
            snd_cap: DEFAULT_SNDBUF,
            fin_queued: false,
            fin_seq: None,
            fin_acked: false,
            send_max: initial_seq.wrapping_add(1),
            rto_ms: RTO_INITIAL_MS,
            srtt_ms: 0,
            rttvar_ms: 0,
            rtt_probe: None,
            rtx_deadline_ms: 0,
            retries: 0,
            peer_fin: false,
            orphaned: false,
            timer_since_ms: 0,
        }
    }

    /// Free space in the receive buffer.
    fn rcv_space(&self) -> usize {
        self.rcv_cap.saturating_sub(self.rx_buffer.len())
    }

    /// The window to advertise now: the free receive space, as far as 16 bits carry it.
    fn rcv_window(&self) -> u16 {
        self.rcv_space().min(u16::MAX as usize) as u16
    }

    /// A segment of this connection carrying `flags` at `seq`, acknowledging
    /// `recv_next` and advertising the current window.
    fn segment(&mut self, seq: u32, flags: TcpFlags) -> Segment {
        self.recv_window = self.rcv_window();
        Segment {
            src_ip: self.id.local_ip,
            dst_ip: self.id.remote_ip,
            src_port: self.id.local_port,
            dst_port: self.id.remote_port,
            seq,
            ack: self.recv_next,
            flags,
            window: self.recv_window,
        }
    }

    /// Acknowledge everything received so far.
    fn send_ack(&mut self) {
        let seg = self.segment(self.send_next, TcpFlags::ack());
        transmit(&seg, &[], [&[], &[]]);
    }

    /// Reset the connection from this end (RFC 9293 3.5.2): RST at `send_next`.
    fn send_reset(&mut self) {
        let seg = self.segment(self.send_next, TcpFlags::rst_ack());
        transmit(&seg, &[], [&[], &[]]);
    }

    /// True in the states in which this end may still send data.
    fn may_send_data(&self) -> bool {
        matches!(self.state, TcpState::Established | TcpState::CloseWait) && !self.fin_queued
    }

    /// True in the states in which the peer may still send data.
    fn may_receive_data(&self) -> bool {
        matches!(
            self.state,
            TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2
        )
    }

    /// True once the FIN is on the wire past `send_next`'s current value: sent and
    /// not yet due for retransmission.
    fn fin_in_flight(&self) -> bool {
        self.fin_seq.is_some_and(|fin| seq_lt(fin, self.send_next))
    }

    /// True when queued data or a queued FIN has not been sent.
    fn has_output(&self) -> bool {
        let sending = matches!(
            self.state,
            TcpState::Established
                | TcpState::CloseWait
                | TcpState::FinWait1
                | TcpState::Closing
                | TcpState::LastAck
        );
        sending
            && (self.tx_buffer.len() > self.tx_sent
                || (self.fin_queued && !self.fin_in_flight() && !self.fin_acked))
    }

    /// Queue as much of `data` as the send buffer takes and memory allows.
    fn queue_tx(&mut self, data: &[u8]) -> usize {
        let take = data.len().min(self.tx_space());
        if take == 0 || self.tx_buffer.try_reserve(take).is_err() {
            return 0;
        }
        self.tx_buffer.extend(&data[..take]);
        take
    }

    /// Send `len` queued bytes starting `offset` bytes after `send_unack`, at
    /// `send_next`. False when IP output refused them.
    fn send_data(&mut self, offset: usize, len: usize) -> bool {
        let seg = self.segment(self.send_next, TcpFlags::ack_psh());
        let (front, back) = self.tx_buffer.as_slices();
        let end = offset + len;
        let parts: [&[u8]; 2] = if end <= front.len() {
            [&front[offset..end], &[]]
        } else if offset >= front.len() {
            [&back[offset - front.len()..end - front.len()], &[]]
        } else {
            [&front[offset..], &back[..end - front.len()]]
        };
        transmit(&seg, &[], parts)
    }

    /// Note that sequence space up to `send_next` has been sent: time it if it is
    /// new and nothing else is being timed, and run the retransmission timer.
    fn sent_through_next(&mut self) {
        if seq_lt(self.send_max, self.send_next) {
            self.send_max = self.send_next;
            if self.rtt_probe.is_none() && self.retries == 0 {
                self.rtt_probe = Some((self.send_next, now_ms()));
            }
        }
        self.arm_retransmit();
    }

    /// Send what the peer's window allows of the unsent data, then the FIN once
    /// every byte before it is out. False when IP output refused a segment: the
    /// bytes stay queued and `drain_deferred_tx` sends them later.
    fn output(&mut self) -> bool {
        if !self.has_output() {
            return true;
        }
        while self.tx_sent < self.tx_buffer.len() {
            let room = (self.send_window as usize).saturating_sub(self.tx_sent);
            if room == 0 {
                // The peer's window is full. Its next ACK opens it and sets
                // TX_RETRY; the retransmission timer probes it meanwhile.
                self.arm_retransmit();
                return true;
            }
            let len = (self.tx_buffer.len() - self.tx_sent)
                .min(room)
                .min(self.mss as usize);
            if !self.send_data(self.tx_sent, len) {
                return false;
            }
            self.tx_sent += len;
            self.send_next = self.send_next.wrapping_add(len as u32);
            self.sent_through_next();
        }
        if self.fin_queued && !self.fin_in_flight() && !self.fin_acked {
            let seg = self.segment(self.send_next, TcpFlags::fin_ack());
            if !transmit(&seg, &[], [&[], &[]]) {
                return false;
            }
            self.fin_seq = Some(self.send_next);
            self.send_next = self.send_next.wrapping_add(1);
            self.sent_through_next();
        }
        true
    }

    /// Run `output`, asking `drain_deferred_tx` to finish what it could not send.
    fn push_output(&mut self) {
        if !self.output() {
            TX_RETRY.store(true, Ordering::Release);
        }
    }

    /// Stop sending: queue a FIN after the buffered data and move to the state
    /// that waits for its acknowledgement.
    fn queue_fin(&mut self) {
        self.send_shutdown = true;
        match self.state {
            TcpState::Established => self.state = TcpState::FinWait1,
            TcpState::CloseWait => self.state = TcpState::LastAck,
            _ => return,
        }
        self.fin_queued = true;
        self.push_output();
    }

    /// A valid RST arrived: record the errno Linux's `tcp_reset` leaves and close.
    fn reset_by_peer(&mut self) {
        self.error = match self.state {
            TcpState::SynSent => Some(ECONNREFUSED),
            TcpState::CloseWait => Some(EPIPE),
            TcpState::TimeWait | TcpState::Closed => None,
            _ => Some(ECONNRESET),
        };
        self.close_now();
    }

    /// Enter CLOSED, dropping whatever was waiting to be sent.
    fn close_now(&mut self) {
        self.state = TcpState::Closed;
        self.tx_buffer.clear();
        self.tx_sent = 0;
        self.send_shutdown = true;
        self.rtx_deadline_ms = 0;
        wake_connection_waiters(self);
    }

    fn enter_time_wait(&mut self) {
        self.state = TcpState::TimeWait;
        self.timer_since_ms = now_ms();
        self.rtx_deadline_ms = 0;
        note_timer_deadline(self.timer_since_ms + TIME_WAIT_MS);
    }

    /// Fold a round-trip sample into the timeout (RFC 6298 2.2-2.4).
    fn sample_rtt(&mut self, rtt: u64) {
        if self.srtt_ms == 0 {
            self.srtt_ms = rtt.max(1);
            self.rttvar_ms = rtt / 2;
        } else {
            let delta = self.srtt_ms.abs_diff(rtt);
            self.rttvar_ms = (3 * self.rttvar_ms + delta) / 4;
            self.srtt_ms = ((7 * self.srtt_ms + rtt) / 8).max(1);
        }
        self.rto_ms = (self.srtt_ms + (4 * self.rttvar_ms).max(RTO_GRANULARITY_MS))
            .clamp(RTO_MIN_MS, RTO_MAX_MS);
    }

    /// True while something sent awaits its acknowledgement, or the peer's
    /// window is shut on data waiting to go.
    fn needs_retransmit_timer(&self) -> bool {
        let outstanding = self.send_unack != self.send_max;
        let probing = self.send_window == 0 && self.tx_buffer.len() > self.tx_sent;
        outstanding || probing
    }

    /// Start the retransmission timer if it should run and is not running, or
    /// stop it if nothing needs it (RFC 6298 5.1).
    fn arm_retransmit(&mut self) {
        if !self.needs_retransmit_timer() {
            self.rtx_deadline_ms = 0;
        } else if self.rtx_deadline_ms == 0 {
            self.rtx_deadline_ms = now_ms() + self.rto_ms;
            note_timer_deadline(self.rtx_deadline_ms);
        }
    }

    /// Restart the retransmission timer: an ACK acknowledged new data
    /// (RFC 6298 5.3).
    fn restart_retransmit(&mut self) {
        self.rtx_deadline_ms = 0;
        self.arm_retransmit();
    }

    /// The retransmission timer fired: resend from the oldest unacknowledged
    /// byte with the timeout doubled (RFC 6298 5.4-5.6), or give up with
    /// ETIMEDOUT after as many tries as Linux makes.
    fn retransmit_timeout(&mut self, now: u64) {
        self.retries += 1;
        let limit = if matches!(self.state, TcpState::SynSent | TcpState::SynReceived) {
            MAX_SYN_RETRIES
        } else {
            MAX_RETRIES
        };
        if self.retries > limit {
            if !matches!(self.state, TcpState::SynSent) {
                self.send_reset();
            }
            self.error = Some(ETIMEDOUT);
            self.close_now();
            return;
        }
        self.rto_ms = (self.rto_ms * 2).min(RTO_MAX_MS);
        self.rtt_probe = None;
        self.rtx_deadline_ms = now + self.rto_ms;
        note_timer_deadline(self.rtx_deadline_ms);
        match self.state {
            TcpState::SynSent => {
                let seg = self.segment(self.send_initial, TcpFlags::syn());
                transmit(&seg, &mss_option(local_mss(self.id.remote_ip)), [&[], &[]]);
            }
            TcpState::SynReceived => {
                let seg = self.segment(self.send_initial, TcpFlags::syn_ack());
                transmit(&seg, &mss_option(local_mss(self.id.remote_ip)), [&[], &[]]);
            }
            _ if self.send_window == 0 && !self.tx_buffer.is_empty() => {
                // The peer's window is shut: probe it with the oldest unacknowledged
                // byte (RFC 9293 3.8.6.1). Its answer carries the window.
                self.send_next = self.send_unack;
                self.tx_sent = 0;
                if self.send_data(0, 1) {
                    self.tx_sent = 1;
                    self.send_next = self.send_unack.wrapping_add(1);
                    if seq_lt(self.send_max, self.send_next) {
                        self.send_max = self.send_next;
                    }
                }
            }
            _ if self.send_unack != self.send_max => {
                // Go back to the oldest unacknowledged byte and send again what
                // the window allows; the peer drops what it already holds.
                self.send_next = self.send_unack;
                self.tx_sent = 0;
                self.push_output();
            }
            _ => self.rtx_deadline_ms = 0,
        }
    }

    /// Process the acknowledgement field of a segment in a synchronized state.
    fn process_ack(&mut self, header: &TcpHeader) {
        let ack = header.ack_num;
        if seq_lt(self.send_max, ack) {
            // Acknowledges something never sent (RFC 9293 3.10.7.4): answer with
            // an ACK and otherwise ignore it.
            self.send_ack();
            return;
        }
        if seq_lt(ack, self.send_unack) {
            // An old duplicate: it carries no window information worth taking.
            return;
        }
        let window_was = self.send_window;
        self.send_window = header.window_size;
        if self.send_window == 0 {
            // The peer answers while its window stays shut: it is alive, so the
            // probes it answers do not count toward giving up.
            self.retries = 0;
        }
        let acked = ack.wrapping_sub(self.send_unack) as usize;
        let mut freed = false;
        if acked > 0 {
            let data = acked.min(self.tx_buffer.len());
            self.tx_buffer.drain(..data);
            self.send_unack = ack;
            if seq_lt(self.send_next, ack) {
                self.send_next = ack;
            }
            self.tx_sent = (self.send_next.wrapping_sub(self.send_unack) as usize)
                .min(self.tx_buffer.len());
            freed = data > 0;
            if self.fin_seq.is_some_and(|fin| seq_lt(fin, ack)) {
                self.fin_acked = true;
            }
            if let Some((end, sent)) = self.rtt_probe {
                if !seq_lt(ack, end) {
                    self.rtt_probe = None;
                    self.sample_rtt(now_ms().saturating_sub(sent));
                }
            }
            self.retries = 0;
            self.restart_retransmit();
        } else if self.send_window != window_was {
            self.arm_retransmit();
        }
        if self.fin_acked {
            match self.state {
                TcpState::FinWait1 => {
                    self.state = TcpState::FinWait2;
                    self.timer_since_ms = now_ms();
                    if self.orphaned {
                        note_timer_deadline(self.timer_since_ms + ORPHAN_FIN_WAIT2_MS);
                    }
                }
                TcpState::Closing => self.enter_time_wait(),
                TcpState::LastAck => self.close_now(),
                _ => {}
            }
        }
        if (freed || self.send_window > window_was) && self.has_output() {
            TX_RETRY.store(true, Ordering::Release);
        }
        if freed {
            wake_connection_waiters(self);
        }
    }

    /// The next instant a timer of this connection is due, if any runs.
    fn next_deadline(&self) -> Option<u64> {
        match self.state {
            TcpState::TimeWait if self.orphaned => Some(self.timer_since_ms + TIME_WAIT_MS),
            TcpState::FinWait2 if self.orphaned => {
                Some(self.timer_since_ms + ORPHAN_FIN_WAIT2_MS)
            }
            _ if self.rtx_deadline_ms != 0 => Some(self.rtx_deadline_ms),
            _ => None,
        }
    }

    /// True when the connection has served its purpose and can leave the table.
    fn expired(&self, now: u64) -> bool {
        match self.state {
            TcpState::Closed => self.orphaned,
            TcpState::TimeWait => {
                self.orphaned && now.saturating_sub(self.timer_since_ms) >= TIME_WAIT_MS
            }
            TcpState::FinWait2 => {
                self.orphaned && now.saturating_sub(self.timer_since_ms) >= ORPHAN_FIN_WAIT2_MS
            }
            _ => false,
        }
    }
}

/// Pending connection (from SYN received, waiting for accept)
#[derive(Clone)]
pub struct PendingConnection {
    /// The address the SYN was sent to: a loopback address, or the interface's.
    pub local_ip: [u8; 4],
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub recv_initial: u32,
    pub send_initial: u32,
    /// True if an acceptor has claimed this entry and is publishing it.
    pub claimed: bool,
    /// True if the final ACK of the 3-way handshake has been received
    pub ack_received: bool,
    /// Data received before accept() was called (buffered here until connection is created)
    pub early_data: Vec<u8>,
    /// Next expected sequence number for early data
    pub recv_next: u32,
    /// The MSS option of the peer's SYN.
    pub peer_mss: Option<u16>,
    /// The peer's window as of its last segment.
    pub peer_window: u16,
}

/// Listening socket info
pub struct ListenSocket {
    /// Local IP address for this listening socket (for binding to specific interfaces)
    #[allow(dead_code)] // Part of socket bind API, needed for interface-specific listening
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub backlog: usize,
    pub pending: VecDeque<PendingConnection>,
    pub owner_pid: crate::process::process::ProcessId,
    /// Threads waiting for incoming connections (accept() blocking)
    pub waiting_threads: Mutex<Vec<u64>>,
    /// Reference count - number of fds pointing to this listener (for fork support)
    /// When this reaches 0, the listener is removed from TCP_LISTENERS
    pub ref_count: core::sync::atomic::AtomicUsize,
}

/// Global TCP connection table
static TCP_CONNECTIONS: Mutex<BTreeMap<ConnectionId, TcpConnection>> = Mutex::new(BTreeMap::new());

/// Global listening socket table (by local port)
static TCP_LISTENERS: Mutex<BTreeMap<u16, ListenSocket>> = Mutex::new(BTreeMap::new());

/// Sequence number generator (simple counter, should be more random in production)
static SEQ_COUNTER: Mutex<u32> = Mutex::new(0x12345678);

pub(crate) fn with_tcp_connections<R>(
    f: impl FnOnce(&mut BTreeMap<ConnectionId, TcpConnection>) -> R,
) -> R {
    let _guard = super::net_lock_guard();
    let mut connections = TCP_CONNECTIONS.lock();
    let result = f(&mut connections);
    drop(connections);
    result
}

pub(crate) fn with_tcp_listeners<R>(f: impl FnOnce(&mut BTreeMap<u16, ListenSocket>) -> R) -> R {
    let _guard = super::net_lock_guard();
    let mut listeners = TCP_LISTENERS.lock();
    let result = f(&mut listeners);
    drop(listeners);
    result
}

/// Remove the connections whose time is up. `force` sweeps even if the last
/// sweep was less than a second ago.
fn reap_expired(connections: &mut BTreeMap<ConnectionId, TcpConnection>, force: bool) {
    let now = now_ms();
    if !force && now.saturating_sub(LAST_REAP_MS.load(Ordering::Relaxed)) < REAP_INTERVAL_MS {
        return;
    }
    LAST_REAP_MS.store(now, Ordering::Relaxed);
    connections.retain(|_, conn| !conn.expired(now));
}

/// True when a socket may not bind `port`: a listener holds it, or a connection
/// on it is in any state but TIME_WAIT. A connection in TIME_WAIT holds the port
/// only against a socket without SO_REUSEADDR (`reuse` false), as on Linux.
pub fn tcp_port_in_use(port: u16, reuse: bool) -> bool {
    if with_tcp_listeners(|listeners| listeners.contains_key(&port)) {
        return true;
    }
    with_tcp_connections(|connections| {
        reap_expired(connections, true);
        connections.values().any(|conn| {
            conn.id.local_port == port
                && match conn.state {
                    TcpState::TimeWait => !reuse,
                    TcpState::Closed => !conn.orphaned,
                    _ => true,
                }
        })
    })
}

/// Generate a new initial sequence number
pub fn generate_isn() -> u32 {
    let _guard = super::net_lock_guard();
    let mut counter = SEQ_COUNTER.lock();
    let isn = *counter;
    *counter = counter.wrapping_add(64000); // Increment by a large amount
    drop(counter);
    isn
}

/// Process a segment for an existing connection, then drop the connection from
/// the table if that left it closed with no owner. True if a connection matched.
fn deliver_to_connection(conn_id: &ConnectionId, header: &TcpHeader, payload: &[u8]) -> bool {
    with_tcp_connections(|connections| {
        reap_expired(connections, false);
        let Some(conn) = connections.get_mut(conn_id) else {
            return false;
        };
        handle_tcp_for_connection(conn, header, payload);
        if conn.expired(now_ms()) {
            connections.remove(conn_id);
        }
        true
    })
}

/// Handle an incoming TCP packet
pub fn handle_tcp(ip: &Ipv4Packet, data: &[u8]) {
    let Some((header, payload)) = TcpHeader::parse(data) else {
        return;
    };

    let config = super::config();
    let local_ip = super::local_ip_of_inbound(&config, ip.dst_ip);
    let conn_id = ConnectionId {
        local_ip,
        local_port: header.dst_port,
        remote_ip: ip.src_ip,
        remote_port: header.src_port,
    };

    // First, check for an existing connection
    if !take_forced_connection_lookup_miss(&conn_id)
        && deliver_to_connection(&conn_id, &header, payload)
    {
        return;
    }

    // No existing connection - check for listening socket
    let (listener_found, retry_connection_lookup) = with_tcp_listeners(|listeners| {
        if let Some(listener) = listeners.get_mut(&header.dst_port) {
            if header.flags.syn && !header.flags.ack {
                // SYN received on listening socket - add to pending queue
                handle_syn_for_listener(listener, local_ip, ip.src_ip, &header);
                (true, false)
            } else if !header.flags.syn {
                if let Some(pending) = listener.pending.iter_mut().find(|pending| {
                    pending.local_ip == local_ip
                        && pending.remote_ip == ip.src_ip
                        && pending.remote_port == header.src_port
                }) {
                    if header.flags.ack {
                        // ACK received - this completes the 3-way handshake.
                        // Verify ACK number matches our SYN+ACK (send_initial + 1)
                        if header.ack_num == pending.send_initial.wrapping_add(1) {
                            pending.ack_received = true;
                            pending.peer_window = header.window_size;
                        }
                        // Buffer any data that arrived with this packet, as far as
                        // the window the SYN+ACK offered reaches.
                        if !payload.is_empty() {
                            let room = DEFAULT_RCVBUF.saturating_sub(pending.early_data.len());
                            if header.seq_num == pending.recv_next {
                                let take = payload.len().min(room);
                                if pending.early_data.try_reserve(take).is_ok() {
                                    pending.early_data.extend_from_slice(&payload[..take]);
                                    pending.recv_next =
                                        pending.recv_next.wrapping_add(take as u32);
                                }
                            }
                            // Acknowledge what is held, so the peer does not
                            // resend it while the connection waits for accept.
                            let window = DEFAULT_RCVBUF
                                .saturating_sub(pending.early_data.len())
                                .min(u16::MAX as usize) as u16;
                            send_tcp_packet(
                                local_ip,
                                ip.src_ip,
                                header.dst_port,
                                header.src_port,
                                pending.send_initial.wrapping_add(1),
                                pending.recv_next,
                                TcpFlags::ack(),
                                window,
                                &[],
                            );
                        }
                    }

                    (true, false)
                } else {
                    (true, true)
                }
            } else {
                (true, false)
            }
        } else {
            (false, false)
        }
    });

    if retry_connection_lookup {
        // tcp_accept publishes the child under TCP_CONNECTIONS (release) before
        // removing its pending entry under TCP_LISTENERS (release). Observing the
        // pending entry gone after acquiring TCP_LISTENERS therefore makes the
        // child publication visible to this subsequent TCP_CONNECTIONS acquire.
        // One re-check is sufficient; no polling or retransmit delay is needed.
        let recovered = with_tcp_connections(|connections| {
            if let Some(conn) = connections.get_mut(&conn_id) {
                handle_tcp_for_connection(conn, &header, payload);
                if conn.expired(now_ms()) {
                    connections.remove(&conn_id);
                }
                true
            } else {
                false
            }
        });
        if recovered {
            TCP_ACCEPT_PUBLISH_RACE_RECOVERED.fetch_add(1, Ordering::Relaxed);
        }
    }

    if listener_found {
        return;
    }

    // RFC 793 reset-generation rule: if an unmatched segment has RST set,
    // drop it silently; an RST must never be answered with another RST.
    if header.flags.rst {
        return;
    }

    // No connection and no listener - send RST
    send_rst(local_ip, ip.src_ip, &header);
}

/// Handle a segment that arrived in SYN_SENT.
fn handle_syn_sent(conn: &mut TcpConnection, header: &TcpHeader) {
    let acceptable_ack = header.flags.ack && header.ack_num == conn.send_next;
    if header.flags.rst {
        // RFC 9293 3.10.7.3: a reset in SYN_SENT counts only with an
        // acceptable ACK.
        if acceptable_ack {
            conn.reset_by_peer();
        }
        return;
    }
    if !(header.flags.syn && acceptable_ack) {
        return;
    }
    conn.recv_initial = header.seq_num;
    conn.recv_next = header.seq_num.wrapping_add(1);
    conn.send_window = header.window_size;
    conn.send_unack = header.ack_num;
    conn.mss = effective_mss(conn.id.remote_ip, header.mss_option);
    conn.state = TcpState::Established;
    conn.retries = 0;
    conn.restart_retransmit();
    conn.send_ack();

    // Wake threads blocked in connect()
    wake_connection_waiters(conn);
}

/// Handle TCP packet for an established connection
fn handle_tcp_for_connection(
    conn: &mut TcpConnection,
    header: &TcpHeader,
    payload: &[u8],
) {
    match conn.state {
        TcpState::SynSent => {
            handle_syn_sent(conn, header);
            return;
        }
        TcpState::Closed | TcpState::Listen => return,
        TcpState::SynReceived => {
            if header.flags.rst {
                conn.reset_by_peer();
                return;
            }
            if header.flags.syn && !header.flags.ack {
                // The peer did not see our SYN+ACK: send it again.
                let seg = Segment {
                    src_ip: conn.id.local_ip,
                    dst_ip: conn.id.remote_ip,
                    src_port: conn.id.local_port,
                    dst_port: conn.id.remote_port,
                    seq: conn.send_initial,
                    ack: header.seq_num.wrapping_add(1),
                    flags: TcpFlags::syn_ack(),
                    window: conn.rcv_window(),
                };
                transmit(&seg, &mss_option(local_mss(conn.id.remote_ip)), [&[], &[]]);
                return;
            }
            if !header.flags.ack || header.ack_num != conn.send_next {
                return;
            }
            conn.state = TcpState::Established;
            conn.send_unack = header.ack_num;
            conn.send_window = header.window_size;
            conn.retries = 0;
            conn.restart_retransmit();
            wake_connection_waiters(conn);
            // The ACK that completes the handshake may carry data or a FIN.
        }
        _ => {}
    }

    // A synchronized state.
    if header.flags.rst {
        // Accept the reset only inside the receive window, so that a stale or
        // spoofed one cannot end the connection (RFC 9293 3.10.7.4).
        let seq_delta = header.seq_num.wrapping_sub(conn.recv_next);
        if seq_delta == 0 || seq_delta < conn.recv_window as u32 {
            conn.reset_by_peer();
        }
        return;
    }
    if header.flags.syn {
        // A SYN on a synchronized connection gets a challenge ACK (RFC 5961 4.2);
        // it is also how a peer that never saw our final ACK completes its
        // handshake when it sends its SYN+ACK again.
        conn.send_ack();
        return;
    }

    if header.flags.ack {
        conn.process_ack(header);
        if conn.state == TcpState::Closed {
            return;
        }
    }

    // Drop the front of a segment that repeats bytes already received: after a
    // retransmission, segment boundaries need not match the first sending.
    let mut seq = header.seq_num;
    let mut payload = payload;
    let behind = conn.recv_next.wrapping_sub(seq) as usize;
    if !payload.is_empty() && seq_lt(seq, conn.recv_next) && behind < payload.len() {
        payload = &payload[behind..];
        seq = conn.recv_next;
    }

    let mut need_ack = false;
    let mut in_sequence = payload.is_empty();
    if !payload.is_empty() {
        need_ack = true;
        if conn.may_receive_data() && seq == conn.recv_next {
            if conn.recv_shutdown
                && matches!(conn.state, TcpState::FinWait1 | TcpState::FinWait2)
            {
                // Data for an end that will never read it: reset, as Linux
                // does for a socket that has shut down both directions.
                conn.send_reset();
                conn.close_now();
                return;
            }
            let taken = conn.publish_rx(payload);
            conn.recv_next = conn.recv_next.wrapping_add(taken as u32);
            in_sequence = taken == payload.len();
            if taken > 0 {
                wake_connection_waiters(conn);
            }
        }
    }

    if header.flags.fin {
        need_ack = true;
        let fin_seq = seq.wrapping_add(payload.len() as u32);
        if in_sequence && fin_seq == conn.recv_next && !conn.peer_fin {
            conn.recv_next = conn.recv_next.wrapping_add(1); // FIN consumes sequence
            conn.peer_fin = true;
            match conn.state {
                TcpState::Established => conn.state = TcpState::CloseWait,
                TcpState::FinWait1 => {
                    if conn.fin_acked {
                        conn.enter_time_wait();
                    } else {
                        conn.state = TcpState::Closing;
                    }
                }
                TcpState::FinWait2 => conn.enter_time_wait(),
                _ => {}
            }
            // Wake threads blocked in recv() so they see EOF
            wake_connection_waiters(conn);
        } else if conn.state == TcpState::TimeWait {
            // The peer did not see our ACK of its FIN: acknowledge it again and
            // restart the wait (RFC 9293 3.10.7.4).
            conn.timer_since_ms = now_ms();
        }
    }

    if need_ack {
        conn.send_ack();
    }
}

/// Handle SYN packet for a listening socket
fn handle_syn_for_listener(
    listener: &mut ListenSocket,
    local_ip: [u8; 4],
    src_ip: [u8; 4],
    header: &TcpHeader,
) {
    let local_port = listener.local_port;
    let syn_ack = |send_isn: u32| {
        try_build_segment(
            &Segment {
                src_ip: local_ip,
                dst_ip: src_ip,
                src_port: local_port,
                dst_port: header.src_port,
                seq: send_isn,
                ack: header.seq_num.wrapping_add(1),
                flags: TcpFlags::syn_ack(),
                window: DEFAULT_RCVBUF.min(u16::MAX as usize) as u16,
            },
            &mss_option(local_mss(src_ip)),
            [&[], &[]],
        )
    };

    if let Some(pending) = listener.pending.iter().find(|p| {
        p.local_ip == local_ip && p.remote_ip == src_ip && p.remote_port == header.src_port
    }) {
        if let Some(segment) = syn_ack(pending.send_initial) {
            let src_mac = super::current_packet_src_mac();
            queue_deferred_tx_with_mac(local_ip, src_ip, Some(src_mac), segment);
        }
        return;
    }

    if listener.pending.len() >= listener.backlog {
        // Send RST to tell client the connection was refused
        send_rst(local_ip, src_ip, header);
        return;
    }

    let send_isn = generate_isn();
    let Some(segment) = syn_ack(send_isn) else {
        // No memory to answer: the peer's SYN retransmission tries again.
        return;
    };

    // Add to pending queue
    listener.pending.push_back(PendingConnection {
        local_ip,
        remote_ip: src_ip,
        remote_port: header.src_port,
        recv_initial: header.seq_num,
        send_initial: send_isn,
        claimed: false,
        ack_received: false,
        early_data: Vec::new(),
        recv_next: header.seq_num.wrapping_add(1), // +1 for SYN
        peer_mss: header.mss_option,
        peer_window: header.window_size,
    });

    // Queue the SYN+ACK for deferred sending AFTER RX processing completes.
    // Sending TX packets during RX processing on Parallels' VirtIO causes the
    // TX packet to be silently dropped (the device doesn't process the TX ring
    // while the RX ring is being consumed).
    let src_mac = super::current_packet_src_mac();
    queue_deferred_tx_with_mac(local_ip, src_ip, Some(src_mac), segment);

    // Wake threads blocked in accept() - connection is now pending
    wake_accept_waiters(listener);
}

/// Answer a segment no connection or listener takes with a reset
/// (RFC 9293 3.10.7.1).
fn send_rst(src_ip: [u8; 4], dst_ip: [u8; 4], header: &TcpHeader) {
    let (seq, ack, flags) = if header.flags.ack {
        (header.ack_num, 0, TcpFlags::rst())
    } else {
        let length =
            header.payload_len as u32 + header.flags.syn as u32 + header.flags.fin as u32;
        (0, header.seq_num.wrapping_add(length), TcpFlags::rst_ack())
    };

    send_tcp_packet(
        src_ip,
        dst_ip,
        header.dst_port,
        header.src_port,
        seq,
        ack,
        flags,
        0,
        &[],
    );
}

/// Send a TCP packet
/// `src_ip` is the connection's local address.
pub fn send_tcp_packet(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq_num: u32,
    ack_num: u32,
    flags: TcpFlags,
    window_size: u16,
    payload: &[u8],
) {
    let seg = Segment {
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        seq: seq_num,
        ack: ack_num,
        flags,
        window: window_size,
    };
    // A control segment that cannot be sent is recovered by the peer's
    // retransmission; there is no caller to report it to.
    transmit(&seg, &[], [payload, &[]]);
}

/// Initiate a TCP connection (called from connect syscall)
pub fn tcp_connect(
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    owner_pid: crate::process::process::ProcessId,
) -> Result<ConnectionId, &'static str> {
    ensure_timer_thread();
    let config = super::config();

    // A connection to the loopback network is made from 127.0.0.1, so its
    // peer sees 127.0.0.1. The peer replies from the address the SYN was sent
    // to, any 127.x.x.x, which is this connection's remote address.
    let conn_id = ConnectionId {
        local_ip: super::source_ip_in(&config, remote_ip),
        local_port,
        remote_ip,
        remote_port,
    };

    let isn = generate_isn();
    let mut conn = TcpConnection::new_outgoing(conn_id, isn, owner_pid);
    conn.mss = local_mss(remote_ip);
    let syn = conn.segment(isn, TcpFlags::syn());
    // The SYN is outstanding until the SYN+ACK comes; the timer resends it.
    conn.arm_retransmit();

    // Add connection to table. An earlier incarnation of the same four-tuple
    // that only waits out TIME_WAIT with no owner gives way to the new one.
    let inserted = with_tcp_connections(|connections| {
        reap_expired(connections, true);
        if let Some(old) = connections.get(&conn_id) {
            if !(old.orphaned && matches!(old.state, TcpState::TimeWait | TcpState::Closed)) {
                return false;
            }
        }
        connections.insert(conn_id, conn);
        true
    });
    if !inserted {
        return Err("Connection already exists");
    }

    // Send SYN. One lost here is sent again by the retransmission timer.
    transmit(&syn, &mss_option(local_mss(remote_ip)), [&[], &[]]);

    Ok(conn_id)
}

/// Start listening on a port (called from listen syscall)
pub fn tcp_listen(
    local_port: u16,
    backlog: usize,
    owner_pid: crate::process::process::ProcessId,
) -> Result<(), &'static str> {
    let config = super::config();

    let inserted = with_tcp_listeners(|listeners| {
        if listeners.contains_key(&local_port) {
            return false;
        }

        listeners.insert(
            local_port,
            ListenSocket {
                local_ip: config.ip_addr,
                local_port,
                // Linux takes one connection on a listener whose backlog is 0, and
                // caps a backlog at SOMAXCONN.
                backlog: backlog.clamp(1, MAX_BACKLOG),
                pending: VecDeque::new(),
                owner_pid,
                waiting_threads: Mutex::new(Vec::new()),
                ref_count: core::sync::atomic::AtomicUsize::new(1),
            },
        );
        true
    });
    if !inserted {
        return Err("Port already in use");
    }

    Ok(())
}

/// Accept a pending connection (called from accept syscall)
pub fn tcp_accept(local_port: u16) -> Option<ConnectionId> {
    ensure_timer_thread();
    let pending = with_tcp_listeners(|listeners| {
        listeners.get_mut(&local_port).and_then(|listener| {
            listener
                .pending
                .iter_mut()
                .find(|pending| !pending.claimed)
                .map(|pending| {
                    pending.claimed = true;
                    pending.clone()
                })
        })
    })?;
    let copied_early_data_len = pending.early_data.len();

    let conn_id = ConnectionId {
        local_ip: pending.local_ip,
        local_port,
        remote_ip: pending.remote_ip,
        remote_port: pending.remote_port,
    };

    // Get owner PID from listener
    let owner_pid =
        with_tcp_listeners(|listeners| listeners.get(&local_port).map(|l| l.owner_pid))?;

    // Create connection - state depends on whether ACK was already received
    let mut conn = TcpConnection::new_outgoing(conn_id, pending.send_initial, owner_pid);
    conn.mss = effective_mss(pending.remote_ip, pending.peer_mss);
    conn.send_window = pending.peer_window;
    // send_next is past our SYN+ACK either way, so the ACK check in
    // handle_tcp_for_connection (SynReceived branch) matches:
    //   header.ack_num == conn.send_next  <-  ISN+1 == ISN+1
    conn.send_next = pending.send_initial.wrapping_add(1);
    if pending.ack_received {
        // 3-way handshake complete, connection is established
        conn.state = TcpState::Established;
        conn.send_unack = conn.send_next;
    } else {
        conn.state = TcpState::SynReceived;
        // The SYN+ACK is outstanding until the final ACK comes.
        conn.send_max = conn.send_next;
        conn.send_unack = pending.send_initial;
        conn.arm_retransmit();
    }
    conn.recv_initial = pending.recv_initial;
    // Use the recv_next from pending, which accounts for any early data
    conn.recv_next = pending.recv_next;
    // Copy any early data that arrived before accept()
    conn.publish_rx(&pending.early_data);

    // MUST precede pending removal: handle_tcp's final-ACK recovery relies on
    // the child being published before the listener entry disappears.
    with_tcp_connections(|connections| {
        connections.insert(conn_id, conn);
    });

    // MUST remain after the TCP_CONNECTIONS insert above. Once readers observe
    // this pending entry gone, handle_tcp re-checks the published child once.
    let final_pending = with_tcp_listeners(|listeners| {
        let listener = listeners.get_mut(&local_port)?;
        let position = listener.pending.iter().position(|entry| {
            entry.remote_ip == pending.remote_ip && entry.remote_port == pending.remote_port
        })?;
        listener.pending.remove(position)
    });

    if let Some(final_pending) = final_pending {
        with_tcp_connections(|connections| {
            if let Some(conn) = connections.get_mut(&conn_id) {
                if final_pending.ack_received && conn.state == TcpState::SynReceived {
                    conn.state = TcpState::Established;
                    conn.send_unack = conn.send_next;
                    conn.send_window = final_pending.peer_window;
                    conn.restart_retransmit();
                }

                if final_pending.early_data.len() > copied_early_data_len {
                    conn.publish_rx(&final_pending.early_data[copied_early_data_len..]);
                }

                let pending_advance = final_pending.recv_next.wrapping_sub(pending.recv_next);
                let connection_advance = conn.recv_next.wrapping_sub(pending.recv_next);
                if pending_advance > connection_advance {
                    conn.recv_next = final_pending.recv_next;
                }
            }
        });
    }

    Some(conn_id)
}

/// Queue `data` on a connection and send what the peer's window allows. Never
/// blocks: Ok with the count queued, which is short when the send buffer fills;
/// EAGAIN when it takes nothing; the pending reset errno once, then EPIPE, after a
/// reset or once sending has shut down (Linux's `sk_stream_error`); ENOTCONN before
/// the handshake has finished.
pub fn tcp_sendmsg(conn_id: &ConnectionId, data: &[u8]) -> Result<usize, i32> {
    with_tcp_connections(|connections| {
        let Some(conn) = connections.get_mut(conn_id) else {
            return Err(EPIPE);
        };
        if conn.send_shutdown || conn.state == TcpState::Closed {
            return Err(conn.error.take().unwrap_or(EPIPE));
        }
        if !conn.may_send_data() {
            return Err(crate::syscall::errno::ENOTCONN);
        }
        let queued = conn.queue_tx(data);
        if queued == 0 && !data.is_empty() {
            return Err(EAGAIN);
        }
        conn.push_output();
        Ok(queued)
    })
}

/// Send data on a connection
pub fn tcp_send(conn_id: &ConnectionId, data: &[u8]) -> Result<usize, &'static str> {
    match with_tcp_connections(|connections| {
        connections.get(conn_id).map(|conn| conn.send_shutdown)
    }) {
        None => return Err("Connection not found"),
        Some(true) => return Err("Connection shutdown for writing"),
        Some(false) => {}
    }

    // Wait for the connection to reach Established state.
    // After accept(), the connection may still be in SynReceived if the
    // client's final ACK hasn't been processed yet. Block until the softirq
    // handler transitions the state and wakes us via wake_connection_waiters.
    {
        let thread_id = crate::task::scheduler::current_thread_id().ok_or("no current thread")?;
        tcp_register_recv_waiter(conn_id, thread_id);

        for _ in 0..20 {
            let state =
                with_tcp_connections(|connections| connections.get(conn_id).map(|c| c.state));
            let Some(state) = state else {
                tcp_unregister_recv_waiter(conn_id, thread_id);
                return Err("Connection not found");
            };

            if matches!(state, TcpState::Established | TcpState::CloseWait) {
                break;
            }
            if matches!(state, TcpState::Closed | TcpState::TimeWait) {
                tcp_unregister_recv_waiter(conn_id, thread_id);
                return Err("Connection closed");
            }
            if !matches!(state, TcpState::SynReceived | TcpState::SynSent) {
                tcp_unregister_recv_waiter(conn_id, thread_id);
                return Err("Connection not established");
            }

            // Block until the softirq handler wakes us on state change.
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_io();
            });
            crate::task::scheduler::yield_current();
        }

        tcp_unregister_recv_waiter(conn_id, thread_id);
    }

    match tcp_sendmsg(conn_id, data) {
        Ok(n) => Ok(n),
        Err(EPIPE) => Err("Connection shutdown for writing"),
        Err(EAGAIN) => Err("Send buffer full"),
        Err(_) => Err("Connection not established"),
    }
}

/// Take up to `buf.len()` received bytes, leaving them queued when `peek`. Never
/// blocks. Ok(0) at end of file: after the peer's FIN, after SHUT_RD, or once the
/// connection is closed with no error to report. A reset's errno is reported once,
/// after any bytes that arrived before it. EAGAIN when nothing is queued yet.
pub fn tcp_recvmsg(conn_id: &ConnectionId, buf: &mut [u8], peek: bool) -> Result<usize, i32> {
    with_tcp_connections(|connections| {
        let Some(conn) = connections.get_mut(conn_id) else {
            return Ok(0);
        };
        if !conn.rx_buffer.is_empty() {
            let len = buf.len().min(conn.rx_buffer.len());
            let (front, back) = conn.rx_buffer.as_slices();
            let first = len.min(front.len());
            buf[..first].copy_from_slice(&front[..first]);
            buf[first..len].copy_from_slice(&back[..len - first]);
            if !peek {
                conn.rx_buffer.drain(..len);
                update_window(conn);
            }
            return Ok(len);
        }
        if conn.peer_fin || conn.recv_shutdown {
            return Ok(0);
        }
        if let Some(error) = conn.error.take() {
            return Err(error);
        }
        if conn.state == TcpState::Closed {
            return Ok(0);
        }
        Err(EAGAIN)
    })
}

/// After the reader freed receive space, tell the peer once the window has grown
/// by a full segment or by half the buffer (receiver-side silly-window avoidance,
/// RFC 9293 3.8.6.2.2).
fn update_window(conn: &mut TcpConnection) {
    if !conn.may_receive_data() {
        return;
    }
    let threshold = (conn.mss as usize).min(conn.rcv_cap / 2).max(1);
    if conn.rcv_window() as usize >= conn.recv_window as usize + threshold {
        conn.send_ack();
    }
}

/// Receive data from a connection
pub fn tcp_recv(conn_id: &ConnectionId, buf: &mut [u8]) -> Result<usize, &'static str> {
    let exists = with_tcp_connections(|connections| connections.contains_key(conn_id));
    if !exists {
        return Err("Connection not found");
    }
    if with_tcp_connections(|connections| {
        connections.get(conn_id).is_some_and(|conn| conn.recv_shutdown)
    }) {
        return Ok(0); // EOF - user called SHUT_RD
    }
    match tcp_recvmsg(conn_id, buf, false) {
        Ok(n) => Ok(n),
        Err(EAGAIN) => Err("No data available"),
        // A reset reads as end of file on this path.
        Err(_) => Ok(0),
    }
}

/// Check if a connection is established (or was established before remote close)
///
/// Returns true for:
/// - Established: Connection is fully open
/// - CloseWait: Remote closed, but handshake completed (read data, then close)
/// - FinWait1/FinWait2: We closed, but handshake completed
/// - Closing/LastAck/TimeWait: Connection closing but was established
///
/// Returns false for:
/// - SynSent: Still waiting for SYN-ACK
/// - SynReceived: Server still completing handshake
/// - Closed/Listen: Not connected
pub fn tcp_is_established(conn_id: &ConnectionId) -> bool {
    with_tcp_connections(|connections| {
        connections.get(conn_id).is_some_and(|conn| {
            // Connection is "established" for connect() purposes if handshake completed.
            // This includes states where one or both sides have initiated close.
            matches!(
                conn.state,
                TcpState::Established
                    | TcpState::CloseWait
                    | TcpState::FinWait1
                    | TcpState::FinWait2
                    | TcpState::Closing
                    | TcpState::LastAck
                    | TcpState::TimeWait
            )
        })
    })
}

/// Check if a connection failed (reset or closed during handshake)
pub fn tcp_is_failed(conn_id: &ConnectionId) -> bool {
    with_tcp_connections(|connections| {
        if let Some(conn) = connections.get(conn_id) {
            matches!(conn.state, TcpState::Closed | TcpState::TimeWait)
        } else {
            true // Connection not found = failed
        }
    })
}

/// The errno a reset left on a connection, cleared by reading it.
pub fn tcp_take_error(conn_id: &ConnectionId) -> Option<i32> {
    with_tcp_connections(|connections| connections.get_mut(conn_id)?.error.take())
}

/// Shutdown part of a full-duplex connection
/// shut_rd: stop receiving
/// shut_wr: stop sending (also sends FIN to remote)
pub fn tcp_shutdown(conn_id: &ConnectionId, shut_rd: bool, shut_wr: bool) {
    with_tcp_connections(|connections| {
        if let Some(conn) = connections.get_mut(conn_id) {
            if shut_rd {
                conn.recv_shutdown = true;
            }
            if shut_wr && !conn.send_shutdown {
                // Send FIN, after the queued data, to signal we're done sending
                conn.queue_fin();
            }
            wake_connection_waiters(conn);
        }
    });
}

/// Increment reference count on a TCP connection (called during fork)
pub fn tcp_add_ref(conn_id: &ConnectionId) {
    with_tcp_connections(|connections| {
        if let Some(conn) = connections.get(conn_id) {
            conn.refcount
                .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }
    });
}

/// Close a connection (decrement refcount, only actually close when last reference dropped)
///
/// The last close sends a FIN after the queued data, unless bytes the owner never
/// read are still queued: then it resets the connection, as Linux does (RFC 2525
/// 2.17), and the peer's next operation fails with ECONNRESET.
pub fn tcp_close(conn_id: &ConnectionId) -> Result<(), &'static str> {
    with_tcp_connections(|connections| {
        let conn = connections.get_mut(conn_id).ok_or("Connection not found")?;

        // Decrement reference count
        let old_count = conn
            .refcount
            .fetch_sub(1, core::sync::atomic::Ordering::SeqCst);

        // Only actually close when last reference is dropped
        if old_count > 1 {
            return Ok(());
        }

        // Last reference - actually close the connection
        conn.orphaned = true;
        conn.recv_shutdown = true;
        let unread = !conn.rx_buffer.is_empty();
        match conn.state {
            TcpState::Established | TcpState::CloseWait if unread => {
                conn.send_reset();
                conn.close_now();
            }
            TcpState::FinWait1 | TcpState::FinWait2 | TcpState::Closing | TcpState::LastAck
                if unread =>
            {
                conn.send_reset();
                conn.close_now();
            }
            TcpState::Established | TcpState::CloseWait => conn.queue_fin(),
            TcpState::FinWait2 => {
                conn.timer_since_ms = now_ms();
                note_timer_deadline(conn.timer_since_ms + ORPHAN_FIN_WAIT2_MS);
            }
            TcpState::TimeWait => note_timer_deadline(conn.timer_since_ms + TIME_WAIT_MS),
            TcpState::SynReceived => {
                conn.send_reset();
                conn.close_now();
            }
            TcpState::SynSent | TcpState::Listen => conn.close_now(),
            TcpState::FinWait1
            | TcpState::Closing
            | TcpState::LastAck
            | TcpState::Closed => {}
        }
        reap_expired(connections, true);

        Ok(())
    })
}

/// Check if there's a pending connection to accept
pub fn tcp_has_pending(local_port: u16) -> bool {
    with_tcp_listeners(|listeners| {
        listeners
            .get(&local_port)
            .map(|l| !l.pending.is_empty())
            .unwrap_or(false)
    })
}

/// Return the number of connections waiting to be accepted on a listener.
pub fn tcp_pending_accept_depth(local_port: u16) -> usize {
    with_tcp_listeners(|listeners| {
        listeners
            .get(&local_port)
            .map(|listener| listener.pending.len())
            .unwrap_or(0)
    })
}

/// Get connection state for debugging and introspection
pub fn tcp_get_state(conn_id: &ConnectionId) -> Option<TcpState> {
    with_tcp_connections(|connections| connections.get(conn_id).map(|c| c.state))
}

// ============================================================================
// Blocking I/O support - waiter registration and wakeup
// ============================================================================

/// Register a thread as waiting for incoming connections on a listening socket (accept)
pub fn tcp_register_accept_waiter(local_port: u16, thread_id: u64) {
    with_tcp_listeners(|listeners| {
        if let Some(listener) = listeners.get(&local_port) {
            let mut waiting = listener.waiting_threads.lock();
            if !waiting.contains(&thread_id) {
                waiting.push(thread_id);
            }
        }
    });
}

/// Unregister a thread from waiting for incoming connections
pub fn tcp_unregister_accept_waiter(local_port: u16, thread_id: u64) {
    with_tcp_listeners(|listeners| {
        if let Some(listener) = listeners.get(&local_port) {
            let mut waiting = listener.waiting_threads.lock();
            waiting.retain(|&id| id != thread_id);
        }
    });
}

/// Register a thread as waiting for data/state change on a connection (recv/connect)
pub fn tcp_register_recv_waiter(conn_id: &ConnectionId, thread_id: u64) {
    with_tcp_connections(|connections| {
        if let Some(conn) = connections.get(conn_id) {
            let mut waiting = conn.waiting_threads.lock();
            if !waiting.contains(&thread_id) {
                waiting.push(thread_id);
            }
        }
    });
}

/// Unregister a thread from waiting for data on a connection
pub fn tcp_unregister_recv_waiter(conn_id: &ConnectionId, thread_id: u64) {
    with_tcp_connections(|connections| {
        if let Some(conn) = connections.get(conn_id) {
            let mut waiting = conn.waiting_threads.lock();
            waiting.retain(|&id| id != thread_id);
        }
    });
}

/// Check if a connection has data available for recv
pub fn tcp_has_data(conn_id: &ConnectionId) -> bool {
    with_tcp_connections(|connections| {
        connections
            .get(conn_id)
            .map(|c| !c.rx_buffer.is_empty())
            .unwrap_or(false)
    })
}

/// Wake all threads waiting on a listening socket (called when SYN arrives)
fn wake_accept_waiters(listener: &ListenSocket) {
    let readers: Vec<u64> = {
        let waiting = listener.waiting_threads.lock();
        waiting.iter().copied().collect()
    };

    if !readers.is_empty() {
        let rejected = wake_waiting_threads(&readers);
        let mut waiting = listener.waiting_threads.lock();
        waiting.retain(|tid| !readers.contains(tid) || rejected.contains(tid));
    }
}

/// Wake all threads waiting on a connection (called when data arrives or state changes)
fn wake_connection_waiters(conn: &TcpConnection) {
    let readers: Vec<u64> = {
        let waiting = conn.waiting_threads.lock();
        waiting.iter().copied().collect()
    };

    if !readers.is_empty() {
        let rejected = wake_waiting_threads(&readers);
        let mut waiting = conn.waiting_threads.lock();
        waiting.retain(|tid| !readers.contains(tid) || rejected.contains(tid));
    }
}

fn wake_waiting_threads(readers: &[u64]) -> Vec<u64> {
    let mut rejected = Vec::new();
    for thread_id in readers {
        if crate::task::scheduler::wake_thread_any_context(*thread_id)
            == crate::task::scheduler::WakeOutcome::Rejected
        {
            TCP_WAKE_REJECTED.fetch_add(1, Ordering::Relaxed);
            rejected.push(*thread_id);
        }
    }
    rejected
}

/// Increment the reference count for a TCP listener (called when fd is duplicated via fork)
pub fn tcp_listener_ref_inc(port: u16) {
    with_tcp_listeners(|listeners| {
        if let Some(listener) = listeners.get(&port) {
            listener
                .ref_count
                .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }
    });
}

/// Decrement the reference count for a TCP listener and remove if it reaches 0
/// Returns true if the listener was removed, false otherwise
///
/// Removing a listener resets the connections still waiting to be accepted, so
/// that their peers learn of it rather than wait on a connection nobody owns.
pub fn tcp_listener_ref_dec(port: u16) -> bool {
    let removed = with_tcp_listeners(|listeners| {
        let listener = listeners.get(&port)?;
        let old = listener
            .ref_count
            .fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
        if old == 1 {
            // Reference count reached 0, remove the listener
            return listeners.remove(&port);
        }
        None
    });
    let Some(listener) = removed else {
        return false;
    };
    for pending in &listener.pending {
        send_tcp_packet(
            pending.local_ip,
            pending.remote_ip,
            port,
            pending.remote_port,
            pending.send_initial.wrapping_add(1),
            pending.recv_next,
            TcpFlags::rst_ack(),
            0,
            &[],
        );
    }
    true
}
