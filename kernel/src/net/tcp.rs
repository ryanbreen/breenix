//! TCP (Transmission Control Protocol) implementation
//!
//! Implements TCP packet parsing, construction, and connection state machine (RFC 793).
//!
//! Each connection keeps every byte it has not seen acknowledged in a bounded send
//! buffer and transmits against the peer's advertised window. Bytes IP output could
//! not take stay queued and go out again from `drain_deferred_tx`, or from the
//! retransmission timer when nothing else runs the connection's output. Received
//! bytes go into a bounded receive buffer whose free space is the advertised window.
//!
//! A SYN to a listener makes a child connection in SYN_RECEIVED, which retransmits
//! its SYN+ACK on its own timer and is dropped by a reset or when its retries run
//! out. accept takes only children whose handshake has finished, oldest first.
//!
//! A segment is checked in the order RFC 9293 3.10.7.4 gives: its checksum, then
//! whether its sequence numbers fall in the receive window, then RST (RFC 5961:
//! only an exact match resets, another in-window RST draws a challenge ACK), SYN
//! and the acknowledgement, and only then its data.
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
//!
//! Every table lives behind one lock, `TCP_CONNECTIONS`, and grows only through
//! fallible reservations: a segment or call that finds no memory is dropped or
//! fails with ENOMEM rather than panicking the kernel.

use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use super::ipv4::{Ipv4Packet, PROTOCOL_TCP};
use crate::socket::object::Socket;
use crate::syscall::errno::{
    EADDRINUSE, EADDRNOTAVAIL, EAGAIN, ECONNREFUSED, ECONNRESET, EINVAL, ENOMEM, ENOTCONN,
    EPIPE, ETIMEDOUT,
};

// ============================================================================
// Deferred TX queue: packets queued during RX processing for later sending.
// Parallels' VirtIO silently drops TX packets submitted while the RX ring
// is being consumed. We queue them and drain after process_rx() completes.
// ============================================================================

struct DeferredTx {
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    dst_mac: Option<[u8; 6]>,
    ttl: u8,
    tcp_segment: Vec<u8>,
}

static DEFERRED_TX_QUEUE: Mutex<Vec<DeferredTx>> = Mutex::new(Vec::new());
static TCP_WAKE_REJECTED: AtomicU64 = AtomicU64::new(0);

/// Set when some connection holds output it could not send: data or a FIN that IP
/// output refused, or data a newly opened peer window lets out. `drain_deferred_tx`
/// takes it and runs every connection's output again.
static TX_RETRY: AtomicBool = AtomicBool::new(false);

pub fn tcp_wake_rejected() -> u64 {
    TCP_WAKE_REJECTED.load(Ordering::Relaxed)
}

/// Thread-context and NetRx TCP paths both enqueue, so exclude softirq re-entry.
/// A segment there is no memory to queue is dropped: it is a SYN+ACK, which the
/// peer's SYN retransmission or the child's own timer sends again.
fn queue_deferred_tx_with_mac(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    dst_mac: Option<[u8; 6]>,
    ttl: u8,
    tcp_segment: Vec<u8>,
) {
    let _guard = super::net_lock_guard();
    let mut queue = DEFERRED_TX_QUEUE.lock();
    if queue.try_reserve(1).is_ok() {
        queue.push(DeferredTx {
            src_ip,
            dst_ip,
            dst_mac,
            ttl,
            tcp_segment,
        });
    }
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
        // A segment that fails here was a SYN+ACK; the child's retransmission
        // timer sends it again.
        let _ = if is_loopback {
            super::send_ipv4_from(
                pkt.src_ip,
                pkt.dst_ip,
                PROTOCOL_TCP,
                pkt.ttl,
                &pkt.tcp_segment,
            )
        } else if let Some(mac) = pkt.dst_mac {
            // Send directly to the known MAC (bypasses ARP, which can't work
            // during process_rx since the re-entrancy guard is held).
            let ip_packet = super::ipv4::Ipv4Packet::try_build(
                pkt.src_ip,
                pkt.dst_ip,
                PROTOCOL_TCP,
                pkt.ttl,
                &pkt.tcp_segment,
            )
            .unwrap_or_default();
            super::send_ethernet(&mac, super::ethernet::ETHERTYPE_IPV4, &ip_packet)
        } else {
            super::send_ipv4_from(
                pkt.src_ip,
                pkt.dst_ip,
                PROTOCOL_TCP,
                pkt.ttl,
                &pkt.tcp_segment,
            )
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

/// Keepalive under SO_KEEPALIVE, with Linux's defaults: the first probe after
/// `tcp_keepalive_time` of silence, then one every `tcp_keepalive_intvl`, and
/// ETIMEDOUT after `tcp_keepalive_probes` go unanswered.
const KEEPALIVE_TIME_MS: u64 = 7_200_000;
const KEEPALIVE_INTVL_MS: u64 = 75_000;
const KEEPALIVE_PROBES: u32 = 9;

static LAST_REAP_MS: AtomicU64 = AtomicU64::new(0);

// ============================================================================
// Timers: one kernel thread, `ktcptimerd`, sleeps until the earliest deadline of
// any connection (a retransmission timeout, a keepalive probe, the end of
// TIME_WAIT, an orphan's FIN_WAIT_2 limit) and runs the timers that are due.
// ============================================================================

/// `ktcptimerd`'s thread id, 0 until it runs.
static TIMER_TID: AtomicU64 = AtomicU64::new(0);
/// Held by the one caller that is starting `ktcptimerd`.
static TIMER_STARTING: AtomicBool = AtomicBool::new(false);
/// The deadline `ktcptimerd` sleeps until, u64::MAX when it sleeps with none.
static TIMER_SLEEP_UNTIL_MS: AtomicU64 = AtomicU64::new(u64::MAX);
/// Set when a deadline earlier than the one slept for was armed.
static TIMER_KICK: AtomicBool = AtomicBool::new(false);

/// Start `ktcptimerd` if it is not running. Thread context, no TCP lock held.
/// ENOMEM when the thread cannot be made; the next caller tries again.
fn ensure_timer_thread() -> Result<(), i32> {
    loop {
        if TIMER_TID.load(Ordering::Acquire) != 0 {
            return Ok(());
        }
        if TIMER_STARTING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let started = crate::task::kthread::kthread_run(tcp_timer_thread, "ktcptimerd");
            let result = match started {
                Ok(handle) => {
                    TIMER_TID.store(handle.tid(), Ordering::Release);
                    Ok(())
                }
                Err(_) => Err(ENOMEM),
            };
            TIMER_STARTING.store(false, Ordering::Release);
            return result;
        }
        // Another caller is starting it: wait for its outcome.
        crate::task::scheduler::yield_current();
    }
}

/// A connection armed a timer for `deadline`: wake `ktcptimerd` if it sleeps
/// past it. Called with the connection table held, from thread or softirq.
fn note_timer_deadline(deadline: u64) {
    if deadline >= TIMER_SLEEP_UNTIL_MS.load(Ordering::Acquire) {
        return;
    }
    TIMER_KICK.store(true, Ordering::Release);
    let tid = TIMER_TID.load(Ordering::Acquire);
    if tid != 0 {
        crate::task::scheduler::wake_thread_any_context(tid);
    }
}

/// Notices `run_timers` takes per pass of the table; a fuller batch runs another
/// pass at once, so a notice never needs memory.
const NOTICE_BATCH: usize = 16;

/// Run every connection timer that is due, and reap what has expired. Returns
/// the earliest deadline still pending, published under the table lock as the
/// one `ktcptimerd` sleeps until so that an earlier arm finds it.
fn run_timers() -> Option<u64> {
    loop {
        let mut notices: [Option<Notice>; NOTICE_BATCH] = Default::default();
        let mut taken = 0;
        let next = with_tcp_connections(|connections| {
            reap_expired(connections, true);
            let now = now_ms();
            let mut next: Option<u64> = None;
            for conn in connections.values_mut() {
                if conn.rtx_deadline_ms != 0 && now >= conn.rtx_deadline_ms {
                    conn.retransmit_timeout(now);
                }
                if conn.keepalive_due(now) {
                    conn.keepalive_timeout();
                }
                if conn.notify_pending != 0 && taken < NOTICE_BATCH {
                    if let Some(notice) = take_notice(conn) {
                        notices[taken] = Some(notice);
                        taken += 1;
                    }
                }
                if let Some(deadline) = conn.next_deadline() {
                    next = Some(next.map_or(deadline, |n: u64| n.min(deadline)));
                }
            }
            reap_expired(connections, true);
            TIMER_SLEEP_UNTIL_MS.store(next.unwrap_or(u64::MAX), Ordering::Release);
            next
        });
        for notice in notices.into_iter().flatten() {
            notice.deliver();
        }
        if taken < NOTICE_BATCH {
            return next;
        }
    }
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
    /// The urgent pointer, meaningful when `flags.urg` is set.
    urgent: u16,
    /// The IPv4 time to live it leaves with.
    ttl: u8,
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
    packet.extend_from_slice(&[0, 0]);
    packet.extend_from_slice(&seg.urgent.to_be_bytes());
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
            super::send_ipv4_from(seg.src_ip, seg.dst_ip, PROTOCOL_TCP, seg.ttl, &packet).is_ok()
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
    /// Initial send sequence number: the SYN or SYN+ACK this end sends again.
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
    /// Threads waiting for recv data or connection state change, for the boot
    /// fixtures that drive a connection without a socket.
    pub waiting_threads: Mutex<Vec<u64>>,
    /// The errno a reset left. With an owner it moves to the socket's SO_ERROR
    /// at the next notice; without one, the next operation reports it once.
    pub error: Option<i32>,
    /// The socket this connection belongs to, told of every change that can
    /// end a wait: data, space, a FIN, a reset, the handshake's end.
    owner: Option<Weak<Socket>>,
    /// Changes the owner has not been told of yet: `CHANGED_RX`, `CHANGED_TX`.
    notify_pending: u8,
    /// The listener this connection came from, until accept takes it.
    parent: Option<ListenerId>,
    /// The listening socket, told once the handshake finishes.
    parent_owner: Option<Weak<Socket>>,
    /// The order in which unaccepted children finished their handshakes.
    accept_order: u64,
    /// Bytes the receive buffer holds at most.
    rcv_cap: usize,
    /// Bytes the send buffer holds at most.
    snd_cap: usize,
    /// The owner set SO_REUSEADDR: a later bind with it may share the port.
    reuse: bool,
    /// The owner's IP_TTL: the time to live of every segment it sends.
    ttl: u8,
    /// The owner set SO_KEEPALIVE.
    keepalive: bool,
    /// When the last acceptable segment arrived, for keepalive.
    last_recv_ms: u64,
    /// Keepalive probes sent without an answer.
    keepalive_probes: u32,
    /// The owner has finished sending: a FIN follows the queued data.
    fin_queued: bool,
    /// The sequence number our FIN occupies, once it has been sent.
    fin_seq: Option<u32>,
    /// The peer acknowledged the FIN.
    fin_acked: bool,
    /// The highest sequence number sent: `send_next` falls back to `send_unack`
    /// when a retransmission timeout resends, and an ACK up to here is valid.
    send_max: u32,
    /// The largest window the peer has offered, which bounds how old an
    /// acceptable ACK may be (RFC 5961 5.2).
    max_send_window: u16,
    /// IP output refused a segment of queued output: the retransmission timer
    /// tries it again if nothing else does.
    tx_blocked: bool,
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
    /// Window probes sent since the peer's window shut, for their backoff.
    probes: u32,
    /// The sequence number after the last urgent byte sent (MSG_OOB), until the
    /// peer acknowledges it.
    snd_up: Option<u32>,
    /// The sequence number of an urgent byte the peer announced and that has
    /// not arrived yet.
    rcv_up: Option<u32>,
    /// The urgent byte, taken out of the stream, until recv(MSG_OOB) reads it.
    oob: Option<u8>,
    /// The peer's FIN arrived: after the queued bytes, the stream is at its end.
    peer_fin: bool,
    /// No descriptor refers to the connection any more.
    orphaned: bool,
    /// Monotonic ms at which the connection entered TIME_WAIT, or at which an
    /// orphan began waiting in FIN_WAIT_2.
    timer_since_ms: u64,
}

/// Orders the children whose handshakes finished, for accept.
static NEXT_ACCEPT_ORDER: AtomicU64 = AtomicU64::new(0);

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

    /// A connection in `state` whose first sequence number is `initial_seq`:
    /// SYN_SENT for connect, SYN_RECEIVED for a listener's child. The SYN or
    /// SYN+ACK at `initial_seq` is outstanding.
    pub fn new(
        id: ConnectionId,
        state: TcpState,
        initial_seq: u32,
        owner_pid: crate::process::process::ProcessId,
    ) -> Self {
        TcpConnection {
            id,
            state,
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
            waiting_threads: Mutex::new(Vec::new()),
            error: None,
            owner: None,
            notify_pending: 0,
            parent: None,
            parent_owner: None,
            accept_order: 0,
            rcv_cap: DEFAULT_RCVBUF,
            snd_cap: DEFAULT_SNDBUF,
            reuse: false,
            ttl: super::ipv4::DEFAULT_TTL,
            keepalive: false,
            last_recv_ms: now_ms(),
            keepalive_probes: 0,
            fin_queued: false,
            fin_seq: None,
            fin_acked: false,
            send_max: initial_seq.wrapping_add(1),
            max_send_window: 0,
            tx_blocked: false,
            rto_ms: RTO_INITIAL_MS,
            srtt_ms: 0,
            rttvar_ms: 0,
            rtt_probe: None,
            rtx_deadline_ms: 0,
            retries: 0,
            probes: 0,
            snd_up: None,
            rcv_up: None,
            oob: None,
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
            urgent: 0,
            ttl: self.ttl,
        }
    }

    /// Acknowledge everything received so far.
    fn send_ack(&mut self) {
        let seg = self.segment(self.send_next, TcpFlags::ack());
        transmit(&seg, &[], [&[], &[]]);
    }

    /// Send the SYN+ACK again: a duplicate SYN came, or its timer ran out.
    fn send_syn_ack(&mut self) {
        let seg = self.segment(self.send_initial, TcpFlags::syn_ack());
        transmit(&seg, &mss_option(local_mss(self.id.remote_ip)), [&[], &[]]);
    }

    /// Reset the connection from this end (RFC 9293 3.5.2): RST at `send_next`.
    fn send_reset(&mut self) {
        let seg = self.segment(self.send_next, TcpFlags::rst_ack());
        transmit(&seg, &[], [&[], &[]]);
    }

    /// Answer an unacceptable acknowledgement with <SEQ=SEG.ACK><CTL=RST>
    /// (RFC 9293 3.10.7.3 and 3.10.7.4).
    fn send_reset_at(&self, seq: u32) {
        send_tcp_packet(
            self.id.local_ip,
            self.id.remote_ip,
            self.id.local_port,
            self.id.remote_port,
            seq,
            0,
            TcpFlags::rst(),
            0,
            &[],
        );
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

    /// True for a listener's child that accept may take: its handshake has
    /// finished and it has not closed.
    fn acceptable(&self) -> bool {
        !matches!(self.state, TcpState::SynReceived | TcpState::Closed)
    }

    /// True when a segment occupying `len` sequence numbers from `seq` falls in
    /// the receive window (RFC 9293 3.10.7.4, the four cases of the
    /// acceptability test), judged against the window last advertised.
    fn segment_acceptable(&self, seq: u32, len: u32) -> bool {
        let window = self.recv_window as u32;
        let in_window = |s: u32| s.wrapping_sub(self.recv_next) < window;
        match (len, window) {
            (0, 0) => seq == self.recv_next,
            (0, _) => in_window(seq),
            (_, 0) => false,
            _ => in_window(seq) || in_window(seq.wrapping_add(len - 1)),
        }
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

    /// Queue as much of `data` as the send buffer takes: the count, 0 when the
    /// buffer is full, ENOMEM when there is room but no memory for it.
    fn queue_tx(&mut self, data: &[u8]) -> Result<usize, i32> {
        let take = data.len().min(self.tx_space());
        if take == 0 {
            return Ok(0);
        }
        self.tx_buffer.try_reserve(take).map_err(|_| ENOMEM)?;
        self.tx_buffer.extend(&data[..take]);
        Ok(take)
    }

    /// Send `len` queued bytes starting `offset` bytes after `send_unack`, at
    /// `send_next`, marked urgent while they precede the urgent pointer. False
    /// when IP output refused them.
    fn send_data(&mut self, offset: usize, len: usize) -> bool {
        let mut seg = self.segment(self.send_next, TcpFlags::ack_psh());
        if let Some(up) = self.snd_up {
            if seq_lt(seg.seq, up) {
                seg.flags.urg = true;
                seg.urgent = up.wrapping_sub(seg.seq).min(u16::MAX as u32) as u16;
            }
        }
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
    /// bytes stay queued, and `drain_deferred_tx` or, failing that, the
    /// retransmission timer sends them later.
    fn output(&mut self) -> bool {
        let sent = self.output_now();
        self.tx_blocked = !sent;
        if !sent {
            self.arm_retransmit();
        }
        sent
    }

    fn output_now(&mut self) -> bool {
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
        self.tx_blocked = false;
        self.rtx_deadline_ms = 0;
        wake_connection_waiters(self);
    }

    fn enter_time_wait(&mut self) {
        self.state = TcpState::TimeWait;
        self.timer_since_ms = now_ms();
        self.rtx_deadline_ms = 0;
        note_timer_deadline(self.timer_since_ms + TIME_WAIT_MS);
    }

    /// The handshake finished: an acceptable ACK of our SYN came in SYN_RECEIVED.
    /// A listener's child becomes one accept may take.
    fn establish(&mut self, header: &TcpHeader) {
        self.state = TcpState::Established;
        self.send_unack = header.ack_num;
        self.take_peer_window(header.window_size);
        self.retries = 0;
        self.restart_retransmit();
        if self.parent.is_some() {
            self.accept_order = NEXT_ACCEPT_ORDER.fetch_add(1, Ordering::Relaxed);
        }
        wake_connection_waiters(self);
    }

    fn take_peer_window(&mut self, window: u16) {
        self.send_window = window;
        self.max_send_window = self.max_send_window.max(window);
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

    /// True while something sent awaits its acknowledgement, the peer's window
    /// is shut on data waiting to go, or IP output refused queued output.
    fn needs_retransmit_timer(&self) -> bool {
        let outstanding = self.send_unack != self.send_max;
        let probing = self.send_window == 0 && self.tx_buffer.len() > self.tx_sent;
        outstanding || probing || self.tx_blocked
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
        if self.send_unack == self.send_max && self.tx_blocked && self.send_window != 0 {
            // Nothing is on the wire: IP output refused what was to go. Try
            // again, backing off as a retransmission would, without counting
            // it as one.
            self.rtx_deadline_ms = 0;
            self.rto_ms = (self.rto_ms * 2).min(RTO_MAX_MS);
            self.push_output();
            return;
        }
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
        self.rtt_probe = None;
        let established = !matches!(self.state, TcpState::SynSent | TcpState::SynReceived);
        if established && self.send_window == 0 && !self.tx_buffer.is_empty() {
            // The peer's window is shut (RFC 9293 3.8.6.1). Probe it as Linux's
            // tcp_xmit_probe_skb does, with a segment that carries no data at
            // send_unack - 1, which the peer must answer with an ACK carrying
            // its window. Whatever went past the shut window is sent again from
            // send_unack once it opens. Probes back off on their own count and
            // leave the retransmission timeout as the round trips set it.
            self.send_next = self.send_unack;
            self.tx_sent = 0;
            self.probes = self.probes.saturating_add(1);
            let backoff = self.rto_ms.saturating_mul(1 << self.probes.min(10));
            self.rtx_deadline_ms = now + backoff.min(RTO_MAX_MS);
            note_timer_deadline(self.rtx_deadline_ms);
            let seg = self.segment(self.send_unack.wrapping_sub(1), TcpFlags::ack());
            transmit(&seg, &[], [&[], &[]]);
            return;
        }
        self.rto_ms = (self.rto_ms * 2).min(RTO_MAX_MS);
        self.rtx_deadline_ms = now + self.rto_ms;
        note_timer_deadline(self.rtx_deadline_ms);
        match self.state {
            TcpState::SynSent => {
                let seg = self.segment(self.send_initial, TcpFlags::syn());
                transmit(&seg, &mss_option(local_mss(self.id.remote_ip)), [&[], &[]]);
            }
            TcpState::SynReceived => self.send_syn_ack(),
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

    /// Process the acknowledgement field of a segment in a synchronized state,
    /// once the caller has found it acceptable.
    fn process_ack(&mut self, header: &TcpHeader) {
        let ack = header.ack_num;
        if seq_lt(ack, self.send_unack) {
            // An old duplicate: it carries no window information worth taking.
            return;
        }
        let window_was = self.send_window;
        self.take_peer_window(header.window_size);
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
            if self.snd_up.is_some_and(|up| !seq_lt(ack, up)) {
                self.snd_up = None;
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
        if self.send_window > 0 {
            self.probes = 0;
            if window_was == 0 && self.send_unack != self.send_max {
                // The shut window dropped whatever was sent past it: send it
                // again from the oldest unacknowledged byte now that it opened.
                self.send_next = self.send_unack;
                self.tx_sent = 0;
            }
        }
        if self.fin_acked {
            match self.state {
                TcpState::FinWait1 => {
                    self.state = TcpState::FinWait2;
                    self.timer_since_ms = now_ms();
                    if self.orphaned {
                        note_timer_deadline(self.timer_since_ms + ORPHAN_FIN_WAIT2_MS);
                    }
                    // A close lingering on the FIN's acknowledgement may end.
                    wake_connection_waiters(self);
                }
                TcpState::Closing => {
                    self.enter_time_wait();
                    wake_connection_waiters(self);
                }
                TcpState::LastAck => self.close_now(),
                _ => {}
            }
        }
        if (freed || self.send_window > window_was) && self.has_output() {
            TX_RETRY.store(true, Ordering::Release);
        }
        if freed {
            wake_connection_writers(self);
        }
    }

    /// Take in-sequence `payload` at `seq` (== `recv_next`): the urgent byte, if
    /// it is among them, leaves the stream for recv(MSG_OOB) as on Linux without
    /// SO_OOBINLINE. Returns the sequence space taken, which ends short when the
    /// receive buffer fills.
    fn take_payload(&mut self, seq: u32, payload: &[u8]) -> usize {
        if let Some(up) = self.rcv_up {
            let at = up.wrapping_sub(seq) as usize;
            if at < payload.len() {
                let before = self.publish_rx(&payload[..at]);
                if before < at {
                    return before;
                }
                self.oob = Some(payload[at]);
                self.rcv_up = None;
                return at + 1 + self.publish_rx(&payload[at + 1..]);
            }
        }
        self.publish_rx(payload)
    }

    /// When the next keepalive probe is due, while SO_KEEPALIVE is on and the
    /// connection sits idle with nothing outstanding.
    fn keepalive_deadline(&self) -> Option<u64> {
        if !self.keepalive
            || !matches!(self.state, TcpState::Established | TcpState::CloseWait)
            || self.rtx_deadline_ms != 0
        {
            return None;
        }
        Some(
            self.last_recv_ms
                + KEEPALIVE_TIME_MS
                + u64::from(self.keepalive_probes) * KEEPALIVE_INTVL_MS,
        )
    }

    fn keepalive_due(&self, now: u64) -> bool {
        self.keepalive_deadline().is_some_and(|deadline| now >= deadline)
    }

    /// The keepalive timer fired: probe as Linux's tcp_write_wakeup does, with
    /// no data at send_unack - 1, which the peer must answer; give up with
    /// ETIMEDOUT once the probes go unanswered.
    fn keepalive_timeout(&mut self) {
        if self.keepalive_probes >= KEEPALIVE_PROBES {
            self.send_reset();
            self.error = Some(ETIMEDOUT);
            self.close_now();
            return;
        }
        self.keepalive_probes += 1;
        let seg = self.segment(self.send_unack.wrapping_sub(1), TcpFlags::ack());
        transmit(&seg, &[], [&[], &[]]);
    }

    /// The next instant a timer of this connection is due, if any runs.
    fn next_deadline(&self) -> Option<u64> {
        match self.state {
            TcpState::TimeWait if self.orphaned => Some(self.timer_since_ms + TIME_WAIT_MS),
            TcpState::FinWait2 if self.orphaned => {
                Some(self.timer_since_ms + ORPHAN_FIN_WAIT2_MS)
            }
            _ => {
                let rtx = (self.rtx_deadline_ms != 0).then_some(self.rtx_deadline_ms);
                match (rtx, self.keepalive_deadline()) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            }
        }
    }

    /// True when the connection has served its purpose and can leave the table:
    /// an orphan whose time is up, or a listener's child that closed before
    /// accept took it.
    fn expired(&self, now: u64) -> bool {
        match self.state {
            TcpState::Closed => self.orphaned || self.parent.is_some(),
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

/// A listener, named by the id `tcp_listen` returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerId(u64);

static NEXT_LISTENER_ID: AtomicU64 = AtomicU64::new(1);

/// Listening socket info
pub struct ListenSocket {
    pub id: ListenerId,
    /// The address it listens on; 0.0.0.0 takes every local address.
    pub local_ip: [u8; 4],
    pub local_port: u16,
    /// Children it holds at most, in SYN_RECEIVED or waiting for accept.
    pub backlog: usize,
    pub owner_pid: crate::process::process::ProcessId,
    /// The listening socket set SO_REUSEADDR; its children inherit it.
    reuse: bool,
    /// The listening socket's IP_TTL, which its SYN+ACKs and children take.
    ttl: u8,
    /// The listening socket, told when a connection is ready for accept.
    owner: Option<Weak<Socket>>,
}

/// A port a socket bound without listening or connecting yet.
struct BoundPort {
    addr: [u8; 4],
    port: u16,
    reuse: bool,
}

/// The connections, as `with_tcp_connections` hands them out.
pub(crate) struct ConnTable {
    conns: Vec<TcpConnection>,
}

impl ConnTable {
    const fn new() -> Self {
        ConnTable { conns: Vec::new() }
    }

    fn position(&self, id: &ConnectionId) -> Option<usize> {
        self.conns.iter().position(|conn| conn.id == *id)
    }

    pub fn get(&self, id: &ConnectionId) -> Option<&TcpConnection> {
        self.conns.iter().find(|conn| conn.id == *id)
    }

    pub fn get_mut(&mut self, id: &ConnectionId) -> Option<&mut TcpConnection> {
        self.conns.iter_mut().find(|conn| conn.id == *id)
    }

    pub fn contains_key(&self, id: &ConnectionId) -> bool {
        self.position(id).is_some()
    }

    pub fn values(&self) -> impl Iterator<Item = &TcpConnection> {
        self.conns.iter()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut TcpConnection> {
        self.conns.iter_mut()
    }

    /// Room for one more connection: ENOMEM when there is no memory for it.
    fn reserve_one(&mut self) -> Result<(), i32> {
        self.conns.try_reserve(1).map_err(|_| ENOMEM)
    }

    /// Add a connection after `reserve_one` made room for it.
    fn push_reserved(&mut self, conn: TcpConnection) {
        debug_assert!(self.conns.len() < self.conns.capacity());
        self.conns.push(conn);
    }

    fn remove(&mut self, id: &ConnectionId) -> Option<TcpConnection> {
        self.position(id).map(|index| self.conns.swap_remove(index))
    }

    fn retain(&mut self, keep: impl FnMut(&TcpConnection) -> bool) {
        self.conns.retain(keep);
    }
}

/// Every TCP table: connections, listeners and bound ports.
pub(crate) struct TcpTables {
    conns: ConnTable,
    listeners: Vec<ListenSocket>,
    bound: Vec<BoundPort>,
}

impl TcpTables {
    const fn new() -> Self {
        TcpTables {
            conns: ConnTable::new(),
            listeners: Vec::new(),
            bound: Vec::new(),
        }
    }

    fn listener(&mut self, id: ListenerId) -> Option<&mut ListenSocket> {
        self.listeners.iter_mut().find(|listener| listener.id == id)
    }

    /// The listener an inbound segment to `local_ip`:`port` reaches: one bound
    /// to that address, else one bound to INADDR_ANY.
    fn listener_for(&self, local_ip: [u8; 4], port: u16) -> Option<usize> {
        let on_port = |listener: &&ListenSocket| listener.local_port == port;
        self.listeners
            .iter()
            .position(|listener| on_port(&listener) && listener.local_ip == local_ip)
            .or_else(|| {
                self.listeners
                    .iter()
                    .position(|listener| on_port(&listener) && listener.local_ip == [0; 4])
            })
    }

    /// True when a socket with SO_REUSEADDR `reuse` may not bind `addr`:`port`,
    /// as Linux's `inet_csk_bind_conflict` decides: another socket on the port
    /// with an overlapping address holds it, unless both set SO_REUSEADDR and
    /// the other is not listening. A connection in TIME_WAIT keeps the
    /// SO_REUSEADDR its socket had.
    fn bind_conflict(&self, addr: [u8; 4], port: u16, reuse: bool) -> bool {
        let shared = |other: bool| reuse && other;
        self.listeners
            .iter()
            .any(|listener| listener.local_port == port && overlaps(listener.local_ip, addr))
            || self.bound.iter().any(|bound| {
                bound.port == port && overlaps(bound.addr, addr) && !shared(bound.reuse)
            })
            || self.conns.values().any(|conn| {
                conn.id.local_port == port
                    && overlaps(conn.id.local_ip, addr)
                    && !(conn.state == TcpState::Closed && conn.orphaned)
                    && !shared(conn.reuse)
            })
    }

    /// Give back the port a socket bound at `addr`.
    fn unbind(&mut self, addr: [u8; 4], port: u16) {
        if let Some(index) = self
            .bound
            .iter()
            .position(|bound| bound.addr == addr && bound.port == port)
        {
            self.bound.swap_remove(index);
        }
    }
}

/// True when two local addresses can receive the same segment: equal, or
/// either one INADDR_ANY.
fn overlaps(a: [u8; 4], b: [u8; 4]) -> bool {
    a == [0; 4] || b == [0; 4] || a == b
}

/// Every TCP table, under one lock.
static TCP_CONNECTIONS: Mutex<TcpTables> = Mutex::new(TcpTables::new());

/// Sequence number generator (simple counter, should be more random in production)
static SEQ_COUNTER: Mutex<u32> = Mutex::new(0x12345678);

fn with_tcp<R>(f: impl FnOnce(&mut TcpTables) -> R) -> R {
    let _guard = super::net_lock_guard();
    let mut tables = TCP_CONNECTIONS.lock();
    let result = f(&mut tables);
    drop(tables);
    result
}

pub(crate) fn with_tcp_connections<R>(f: impl FnOnce(&mut ConnTable) -> R) -> R {
    with_tcp(|tables| f(&mut tables.conns))
}

/// Remove the connections whose time is up. `force` sweeps even if the last
/// sweep was less than a second ago.
fn reap_expired(connections: &mut ConnTable, force: bool) {
    let now = now_ms();
    if !force && now.saturating_sub(LAST_REAP_MS.load(Ordering::Relaxed)) < REAP_INTERVAL_MS {
        return;
    }
    LAST_REAP_MS.store(now, Ordering::Relaxed);
    connections.retain(|conn| !conn.expired(now));
}

/// The first port an unbound socket is given: the bottom of Linux's default
/// `ip_local_port_range`, 32768-60999.
const EPHEMERAL_FIRST: u16 = 32768;
const EPHEMERAL_LAST: u16 = 60999;
static NEXT_EPHEMERAL: core::sync::atomic::AtomicU16 =
    core::sync::atomic::AtomicU16::new(EPHEMERAL_FIRST);

/// Take `addr`:`port` for a socket that binds it: EADDRINUSE when another
/// socket holds it (`TcpTables::bind_conflict`), ENOMEM when there is no
/// memory to record it.
pub fn tcp_bind_port(addr: [u8; 4], port: u16, reuse: bool) -> Result<(), i32> {
    with_tcp(|tables| {
        reap_expired(&mut tables.conns, true);
        if tables.bind_conflict(addr, port, reuse) {
            return Err(EADDRINUSE);
        }
        tables.bound.try_reserve(1).map_err(|_| ENOMEM)?;
        tables.bound.push(BoundPort { addr, port, reuse });
        Ok(())
    })
}

/// Give back a port `tcp_bind_port` or `tcp_bind_ephemeral` took at `addr`:
/// its socket closed without listening or connecting.
pub fn tcp_unbind_port(addr: [u8; 4], port: u16) {
    with_tcp(|tables| tables.unbind(addr, port));
}

/// Bind an unbound socket at `addr` to a free port from the ephemeral range,
/// as connect and listen do for a socket that was never bound. EADDRINUSE
/// when every port in the range is taken; connect reports that as Linux does,
/// with EADDRNOTAVAIL.
pub fn tcp_bind_ephemeral(addr: [u8; 4]) -> Result<u16, i32> {
    let span = (EPHEMERAL_LAST - EPHEMERAL_FIRST) as u32 + 1;
    for _ in 0..span {
        let candidate = NEXT_EPHEMERAL
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |port| {
                Some(if port >= EPHEMERAL_LAST { EPHEMERAL_FIRST } else { port + 1 })
            })
            .unwrap_or(EPHEMERAL_FIRST);
        match tcp_bind_port(addr, candidate, false) {
            Ok(()) => return Ok(candidate),
            Err(EADDRINUSE) => {}
            Err(error) => return Err(error),
        }
    }
    Err(EADDRINUSE)
}

/// What a socket's options mean for its connection.
#[derive(Clone, Copy)]
pub struct ConnOptions {
    /// SO_RCVBUF and SO_SNDBUF as they report, Linux's doubled values, of
    /// which half carries data.
    pub rcvbuf: usize,
    pub sndbuf: usize,
    pub keepalive: bool,
    pub reuse: bool,
    pub ttl: u8,
}

/// Bring a connection up to date with its socket's options.
pub fn tcp_set_options(conn_id: &ConnectionId, options: impl FnOnce() -> ConnOptions) {
    with_tcp_connections(|connections| {
        let Some(conn) = connections.get_mut(conn_id) else {
            return;
        };
        // Read under the table lock: an older setter must never apply a
        // snapshot after a newer setter has updated this connection.
        let options = options();
        conn.set_buffer_sizes(options.rcvbuf / 2, options.sndbuf / 2);
        conn.reuse = options.reuse;
        conn.ttl = options.ttl;
        if options.keepalive && !conn.keepalive {
            // Linux starts the keepalive time when the option is turned on.
            conn.last_recv_ms = now_ms();
            conn.keepalive_probes = 0;
        }
        conn.keepalive = options.keepalive;
        if let Some(deadline) = conn.keepalive_deadline() {
            note_timer_deadline(deadline);
        }
    });
}

/// The TTL inherited when the handshake created this connection.
pub fn tcp_ttl(conn_id: &ConnectionId) -> Option<u8> {
    with_tcp_connections(|connections| connections.get(conn_id).map(|conn| conn.ttl))
}

/// poll's view of a connection, as Linux's `tcp_poll` reports it: POLLHUP once
/// both directions are shut (or the connection is closed), POLLIN and POLLRDHUP
/// once the peer's FIN or SHUT_RD ends the input, POLLIN for queued bytes,
/// POLLPRI for an unread urgent byte, POLLOUT when the send buffer has room or
/// sending is shut (the call then fails at once), and POLLERR while a reset's
/// errno is pending on the connection. None for a connection the table no
/// longer holds.
pub fn tcp_poll_events(conn_id: &ConnectionId) -> Option<u16> {
    use crate::ipc::poll::events::{POLLERR, POLLHUP, POLLIN, POLLOUT, POLLPRI, POLLRDHUP};
    with_tcp_connections(|connections| {
        let conn = connections.get(conn_id)?;
        let closed = conn.state == TcpState::Closed;
        let input_done = conn.peer_fin || conn.recv_shutdown || closed;
        let output_done = conn.send_shutdown || closed;
        let mut bits: i16 = 0;
        if input_done && output_done {
            bits |= POLLHUP;
        }
        if input_done {
            bits |= POLLIN | POLLRDHUP;
        }
        if !matches!(conn.state, TcpState::SynSent | TcpState::SynReceived) {
            if !conn.rx_buffer.is_empty() {
                bits |= POLLIN;
            }
            if conn.oob.is_some() {
                bits |= POLLPRI;
            }
            // Linux's sk_stream_is_writeable: free space at least half of what
            // is queued.
            let space = conn.tx_space();
            if output_done || (space > 0 && space >= conn.tx_buffer.len() / 2) {
                bits |= POLLOUT;
            }
        }
        if conn.error.is_some() {
            bits |= POLLERR;
        }
        Some(bits as u16)
    })
}

/// True when a connection has finished its handshake and has not been reset:
/// it has a peer to name.
pub fn tcp_has_peer(conn_id: &ConnectionId) -> bool {
    with_tcp_connections(|connections| {
        connections.get(conn_id).is_some_and(|conn| {
            !matches!(
                conn.state,
                TcpState::SynSent | TcpState::SynReceived | TcpState::Closed | TcpState::Listen
            )
        })
    })
}

/// True when a socket without SO_REUSEADDR may not bind `port` on any address.
pub fn tcp_port_in_use(port: u16, reuse: bool) -> bool {
    with_tcp(|tables| {
        reap_expired(&mut tables.conns, true);
        tables.bind_conflict([0; 4], port, reuse)
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

/// A socket to wake once the TCP tables are released. A reset's errno is
/// already its SO_ERROR: `take_notice` stores it under the table lock, so that
/// no one sees the connection closed before the error is there to read.
pub(crate) struct Notice {
    socket: Arc<Socket>,
    changed: u8,
}

impl Notice {
    /// Wake the socket's waiters, telling edge-triggered epoll which side
    /// changed. Thread or softirq context, with no TCP lock held: dropping the
    /// reference here may release the socket, whose release takes the TCP lock.
    fn deliver(self) {
        match self.changed {
            CHANGED_RX => self.socket.common.notify_readable(),
            CHANGED_TX => self.socket.common.notify_writable(),
            _ => self.socket.common.notify(),
        }
    }
}

/// A connection change that can make its socket readable: data, a FIN, a
/// connection ready for accept.
const CHANGED_RX: u8 = 1;
/// A change that can make it writable: send space freed.
const CHANGED_TX: u8 = 2;
/// A change of state, which can do both.
const CHANGED_ANY: u8 = CHANGED_RX | CHANGED_TX;

/// The notice a connection owes its socket, if a change is pending: to its
/// owner, or, for a child accept has not taken, to the listener once accept
/// can take it. Called with the table lock held; the returned reference must
/// leave the lock before it is dropped.
fn take_notice(conn: &mut TcpConnection) -> Option<Notice> {
    let changed = conn.notify_pending;
    if changed == 0 {
        return None;
    }
    if let Some(owner) = conn.owner.as_ref() {
        conn.notify_pending = 0;
        let socket = owner.upgrade()?;
        if let Some(error) = conn.error.take() {
            socket.common.set_error(error);
        }
        return Some(Notice { socket, changed });
    }
    if conn.parent.is_some() && conn.acceptable() {
        conn.notify_pending = 0;
        let socket = conn.parent_owner.as_ref()?.upgrade()?;
        // A connection ready for accept is input to the listener.
        return Some(Notice {
            socket,
            changed: CHANGED_RX,
        });
    }
    None
}

/// Run `f` on a connection, then deliver the notice its socket is owed.
fn with_connection_notify<R>(
    conn_id: &ConnectionId,
    f: impl FnOnce(&mut TcpConnection) -> R,
) -> Option<R> {
    let (result, notice) = with_tcp_connections(|connections| {
        let conn = connections.get_mut(conn_id)?;
        let result = f(conn);
        Some((result, take_notice(conn)))
    })?;
    if let Some(notice) = notice {
        notice.deliver();
    }
    Some(result)
}

/// Tie a connection to the socket that carries it: from now on the socket's
/// wait queue hears of its changes and its SO_ERROR of its resets. A reset that
/// came before is handed over at once.
pub fn tcp_set_owner(conn_id: &ConnectionId, owner: Weak<Socket>) {
    with_connection_notify(conn_id, |conn| {
        conn.owner = Some(owner);
        conn.notify_pending = CHANGED_ANY;
    });
}

/// Tie a listener to its socket, which hears when a connection is ready.
pub fn tcp_set_listener_owner(id: ListenerId, owner: Weak<Socket>) {
    with_tcp(|tables| {
        if let Some(listener) = tables.listener(id) {
            listener.owner = Some(owner);
        }
    });
}

/// Handle an incoming TCP packet
pub fn handle_tcp(ip: &Ipv4Packet, data: &[u8]) {
    // A segment whose checksum does not hold over the pseudo-header is dropped
    // unread (RFC 9293 3.1).
    if tcp_checksum(ip.src_ip, ip.dst_ip, data) != 0 {
        return;
    }
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

    // A connection takes the segment if one matches it, else a listener.
    let (notice, listener_found) = with_tcp(|tables| {
        reap_expired(&mut tables.conns, false);
        if let Some(conn) = tables.conns.get_mut(&conn_id) {
            handle_tcp_for_connection(conn, &header, payload);
            let notice = take_notice(conn);
            if conn.expired(now_ms()) {
                tables.conns.remove(&conn_id);
            }
            return (notice, true);
        }
        let Some(index) = tables.listener_for(local_ip, header.dst_port) else {
            return (None, false);
        };
        handle_listener_segment(tables, index, local_ip, ip.src_ip, &header);
        (None, true)
    });
    if let Some(notice) = notice {
        notice.deliver();
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

/// Handle a segment that arrived in SYN_SENT (RFC 9293 3.10.7.3).
fn handle_syn_sent(conn: &mut TcpConnection, header: &TcpHeader) {
    if header.flags.ack && header.ack_num != conn.send_next {
        // An acknowledgement of something never sent draws a reset, unless it
        // is one.
        if !header.flags.rst {
            conn.send_reset_at(header.ack_num);
        }
        return;
    }
    if header.flags.rst {
        // A reset counts only with an acceptable ACK.
        if header.flags.ack {
            conn.reset_by_peer();
        }
        return;
    }
    if !header.flags.syn {
        return;
    }
    conn.recv_initial = header.seq_num;
    conn.recv_next = header.seq_num.wrapping_add(1);
    conn.take_peer_window(header.window_size);
    conn.mss = effective_mss(conn.id.remote_ip, header.mss_option);
    conn.last_recv_ms = now_ms();
    if !header.flags.ack {
        // Simultaneous open: the peer's SYN crossed ours. Acknowledge it with
        // our SYN again and wait in SYN_RECEIVED for its ACK.
        conn.state = TcpState::SynReceived;
        conn.send_syn_ack();
        return;
    }
    conn.send_unack = header.ack_num;
    conn.state = TcpState::Established;
    conn.retries = 0;
    conn.restart_retransmit();
    conn.send_ack();

    // Wake threads blocked in connect()
    wake_connection_waiters(conn);
}

/// Handle a segment for a connection in any state past SYN_SENT, in the order
/// of RFC 9293 3.10.7.4: sequence number, RST, SYN, ACK, URG, data, FIN.
fn handle_tcp_for_connection(conn: &mut TcpConnection, header: &TcpHeader, payload: &[u8]) {
    match conn.state {
        TcpState::SynSent => {
            handle_syn_sent(conn, header);
            return;
        }
        TcpState::Closed | TcpState::Listen => return,
        TcpState::SynReceived
            if header.flags.syn && header.seq_num.wrapping_add(1) == conn.recv_next =>
        {
            if !header.flags.ack {
                // The peer did not see our SYN+ACK: send it again.
                conn.send_syn_ack();
            } else if header.ack_num == conn.send_next {
                // Simultaneous open: the peer's SYN+ACK acknowledges our SYN,
                // and our SYN+ACK already acknowledged its SYN.
                conn.last_recv_ms = now_ms();
                conn.establish(header);
            }
            return;
        }
        _ => {}
    }

    let seg_len = payload.len() as u32 + header.flags.syn as u32 + header.flags.fin as u32;
    if !conn.segment_acceptable(header.seq_num, seg_len) {
        // Outside the window: acknowledge it, unless it is a reset, and
        // otherwise ignore it. A retransmitted FIN in TIME_WAIT means the
        // peer did not see our ACK of it: the wait starts again.
        if header.flags.rst {
            return;
        }
        if header.flags.fin && conn.state == TcpState::TimeWait {
            conn.timer_since_ms = now_ms();
        }
        conn.send_ack();
        return;
    }
    conn.last_recv_ms = now_ms();
    conn.keepalive_probes = 0;

    if header.flags.rst {
        // Only a reset at exactly the next expected sequence number ends the
        // connection; another in the window draws a challenge ACK, so that a
        // blind one cannot (RFC 5961 3.2).
        if header.seq_num == conn.recv_next {
            conn.reset_by_peer();
        } else {
            conn.send_ack();
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
    if !header.flags.ack {
        return;
    }

    if conn.state == TcpState::SynReceived {
        if header.ack_num != conn.send_next {
            conn.send_reset_at(header.ack_num);
            return;
        }
        conn.establish(header);
        // The ACK that completes the handshake may carry data or a FIN.
    } else {
        let ack = header.ack_num;
        let oldest = conn.send_unack.wrapping_sub(conn.max_send_window as u32);
        if seq_lt(conn.send_max, ack) || seq_lt(ack, oldest) {
            // It acknowledges something never sent, or is older than any window
            // the peer offered: answer with an ACK and drop it (RFC 9293
            // 3.10.7.4, RFC 5961 5.2).
            conn.send_ack();
            return;
        }
        conn.process_ack(header);
        if conn.state == TcpState::Closed {
            return;
        }
    }

    // The urgent pointer names the byte before it (BSD's reading, which Linux
    // keeps). A newer urgent byte replaces one not yet read.
    if header.flags.urg && header.urgent_ptr != 0 && conn.may_receive_data() {
        let urgent = header
            .seq_num
            .wrapping_add(header.urgent_ptr as u32)
            .wrapping_sub(1);
        let newer = conn.rcv_up.map_or(true, |up| seq_lt(up, urgent));
        if !seq_lt(urgent, conn.recv_next) && newer {
            conn.rcv_up = Some(urgent);
            conn.oob = None;
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
            let taken = conn.take_payload(seq, payload);
            conn.recv_next = conn.recv_next.wrapping_add(taken as u32);
            in_sequence = taken == payload.len();
            if taken > 0 {
                wake_connection_readers(conn);
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
            wake_connection_readers(conn);
        }
    }

    if need_ack {
        conn.send_ack();
    }
}

/// A segment for a listener with no connection of its own (RFC 9293 3.10.7.2):
/// a reset is ignored, an acknowledgement draws a reset, and a SYN makes a
/// child in SYN_RECEIVED.
fn handle_listener_segment(
    tables: &mut TcpTables,
    index: usize,
    local_ip: [u8; 4],
    src_ip: [u8; 4],
    header: &TcpHeader,
) {
    if header.flags.rst {
        return;
    }
    if header.flags.ack {
        send_tcp_packet(
            local_ip,
            src_ip,
            header.dst_port,
            header.src_port,
            header.ack_num,
            0,
            TcpFlags::rst(),
            0,
            &[],
        );
        return;
    }
    if header.flags.syn {
        handle_syn_for_listener(tables, index, local_ip, src_ip, header);
    }
}

/// A SYN for a listener: make the child connection in SYN_RECEIVED and send
/// its SYN+ACK, or refuse it with a reset when the listener already holds its
/// backlog of children. A SYN there is no memory for is dropped; the peer
/// sends it again.
fn handle_syn_for_listener(
    tables: &mut TcpTables,
    index: usize,
    local_ip: [u8; 4],
    src_ip: [u8; 4],
    header: &TcpHeader,
) {
    let listener = &tables.listeners[index];
    let id = listener.id;
    let children = tables
        .conns
        .values()
        .filter(|conn| conn.parent == Some(id))
        .count();
    if children >= listener.backlog {
        send_rst(local_ip, src_ip, header);
        return;
    }

    let conn_id = ConnectionId {
        local_ip,
        local_port: header.dst_port,
        remote_ip: src_ip,
        remote_port: header.src_port,
    };
    let isn = generate_isn();
    let mut child = TcpConnection::new(conn_id, TcpState::SynReceived, isn, listener.owner_pid);
    child.recv_initial = header.seq_num;
    child.recv_next = header.seq_num.wrapping_add(1);
    child.take_peer_window(header.window_size);
    child.mss = effective_mss(src_ip, header.mss_option);
    child.parent = Some(id);
    child.parent_owner = listener.owner.clone();
    child.reuse = listener.reuse;
    child.ttl = listener.ttl;
    let seg = child.segment(isn, TcpFlags::syn_ack());
    let Some(syn_ack) = try_build_segment(&seg, &mss_option(local_mss(src_ip)), [&[], &[]])
    else {
        return;
    };
    if tables.conns.reserve_one().is_err() {
        return;
    }
    // The SYN+ACK is outstanding until the final ACK comes; the child's timer
    // sends it again, and gives the child up after Linux's tcp_synack_retries.
    child.arm_retransmit();
    tables.conns.push_reserved(child);

    // Queue the SYN+ACK for deferred sending AFTER RX processing completes.
    // Sending TX packets during RX processing on Parallels' VirtIO causes the
    // TX packet to be silently dropped (the device doesn't process the TX ring
    // while the RX ring is being consumed).
    let src_mac = super::current_packet_src_mac();
    queue_deferred_tx_with_mac(local_ip, src_ip, Some(src_mac), seg.ttl, syn_ack);
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
        urgent: 0,
        ttl: super::ipv4::DEFAULT_TTL,
    };
    // A control segment that cannot be sent is recovered by the peer's
    // retransmission; there is no caller to report it to.
    transmit(&seg, &[], [payload, &[]]);
}

/// Open a connection from `bind_addr`:`local_port`, the address and port its
/// socket bound (an address of 0.0.0.0 lets the route choose), to
/// `remote_ip`:`remote_port`. The connection takes over the socket's binding.
/// EADDRNOTAVAIL when the four-tuple is in use, ENOMEM without memory for it.
pub fn tcp_connect(
    bind_addr: [u8; 4],
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    owner_pid: crate::process::process::ProcessId,
    ttl: u8,
) -> Result<ConnectionId, i32> {
    ensure_timer_thread()?;
    let config = super::config();

    // A connection to the loopback network is made from 127.0.0.1, so its
    // peer sees 127.0.0.1. The peer replies from the address the SYN was sent
    // to, any 127.x.x.x, which is this connection's remote address.
    let local_ip = if bind_addr == [0; 4] {
        super::source_ip_in(&config, remote_ip)
    } else {
        bind_addr
    };
    let conn_id = ConnectionId {
        local_ip,
        local_port,
        remote_ip,
        remote_port,
    };

    let isn = generate_isn();
    let mut conn = TcpConnection::new(conn_id, TcpState::SynSent, isn, owner_pid);
    conn.mss = local_mss(remote_ip);
    conn.ttl = ttl;
    let syn = conn.segment(isn, TcpFlags::syn());
    // The SYN is outstanding until the SYN+ACK comes; the timer resends it.
    conn.arm_retransmit();

    // An earlier incarnation of the same four-tuple that only waits out
    // TIME_WAIT with no owner gives way to the new one.
    with_tcp(|tables| {
        reap_expired(&mut tables.conns, true);
        if let Some(old) = tables.conns.get(&conn_id) {
            if !(old.orphaned && matches!(old.state, TcpState::TimeWait | TcpState::Closed)) {
                return Err(EADDRNOTAVAIL);
            }
        }
        tables.conns.reserve_one()?;
        tables.conns.remove(&conn_id);
        tables.conns.push_reserved(conn);
        tables.unbind(bind_addr, local_port);
        Ok(())
    })?;

    // Send SYN. One lost here is sent again by the retransmission timer.
    transmit(&syn, &mss_option(local_mss(remote_ip)), [&[], &[]]);

    Ok(conn_id)
}

/// Start listening on `addr`:`port`, which the socket bound, taking over its
/// binding. EADDRINUSE when another listener holds an overlapping address on
/// the port; ENOMEM without memory for the listener or its timer thread.
/// `owner` is told when a connection is ready for accept.
pub fn tcp_listen(
    addr: [u8; 4],
    port: u16,
    backlog: usize,
    owner_pid: crate::process::process::ProcessId,
    reuse: bool,
    ttl: u8,
    owner: Option<Weak<Socket>>,
) -> Result<ListenerId, i32> {
    // Children retransmit their SYN+ACKs; the timer thread must be there for them.
    ensure_timer_thread()?;
    let id = ListenerId(NEXT_LISTENER_ID.fetch_add(1, Ordering::Relaxed));
    with_tcp(|tables| {
        if tables
            .listeners
            .iter()
            .any(|listener| listener.local_port == port && overlaps(listener.local_ip, addr))
        {
            return Err(EADDRINUSE);
        }
        tables.listeners.try_reserve(1).map_err(|_| ENOMEM)?;
        tables.unbind(addr, port);
        tables.listeners.push(ListenSocket {
            id,
            local_ip: addr,
            local_port: port,
            backlog: listen_backlog(backlog),
            owner_pid,
            reuse,
            ttl,
            owner,
        });
        Ok(id)
    })
}

/// Linux takes one connection on a listener whose backlog is 0, and caps a
/// backlog at SOMAXCONN.
fn listen_backlog(backlog: usize) -> usize {
    backlog.clamp(1, MAX_BACKLOG)
}

/// IP_TTL set on a listening socket: the SYN+ACKs and children of later
/// connection requests take it.
pub fn tcp_set_listener_ttl(id: ListenerId, ttl: impl FnOnce() -> u8) {
    with_tcp(|tables| {
        if let Some(listener) = tables.listener(id) {
            listener.ttl = ttl();
        }
    });
}

/// A second listen on a listening socket: Linux takes its new backlog.
pub fn tcp_set_backlog(id: ListenerId, backlog: usize) {
    with_tcp(|tables| {
        if let Some(listener) = tables.listener(id) {
            listener.backlog = listen_backlog(backlog);
        }
    });
}

/// Stop listening: the listener goes, and so do the children accept has not
/// taken, each reset so that its peer learns of it rather than wait on a
/// connection nobody owns.
pub fn tcp_unlisten(id: ListenerId) {
    with_tcp(|tables| {
        if let Some(index) = tables.listeners.iter().position(|listener| listener.id == id) {
            tables.listeners.swap_remove(index);
        }
        for conn in tables.conns.values_mut() {
            if conn.parent == Some(id) {
                if conn.state != TcpState::Closed {
                    conn.send_reset();
                }
                conn.close_now();
                conn.parent = None;
                conn.parent_owner = None;
                conn.orphaned = true;
            }
        }
        reap_expired(&mut tables.conns, true);
    });
}

/// Take the child of listener `id` whose handshake finished first, if one is
/// ready. It belongs to no socket until the caller ties it to one.
pub fn tcp_accept(id: ListenerId) -> Option<ConnectionId> {
    with_tcp_connections(|connections| {
        let conn = connections
            .values_mut()
            .filter(|conn| conn.parent == Some(id) && conn.acceptable())
            .min_by_key(|conn| conn.accept_order)?;
        conn.parent = None;
        conn.parent_owner = None;
        Some(conn.id)
    })
}

/// Queue `data` on a connection and send what the peer's window allows, the
/// last byte urgent when `oob` (MSG_OOB). Never blocks: Ok with the count
/// queued, which is short when the send buffer fills; EAGAIN when it takes
/// nothing; ENOMEM when there is room but no memory; the pending reset errno
/// once, then EPIPE, after a reset or once sending has shut down (Linux's
/// `sk_stream_error`); ENOTCONN before the handshake has finished.
pub fn tcp_sendmsg(conn_id: &ConnectionId, data: &[u8], oob: bool) -> Result<usize, i32> {
    with_connection_notify(conn_id, |conn| {
        if conn.send_shutdown || conn.state == TcpState::Closed {
            return Err(conn.error.take().unwrap_or(EPIPE));
        }
        if !conn.may_send_data() {
            return Err(ENOTCONN);
        }
        let queued = conn.queue_tx(data)?;
        if queued == 0 && !data.is_empty() {
            return Err(EAGAIN);
        }
        if oob && queued > 0 {
            conn.snd_up = Some(conn.send_unack.wrapping_add(conn.tx_buffer.len() as u32));
        }
        conn.push_output();
        Ok(queued)
    })
    .unwrap_or(Err(EPIPE))
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

    match tcp_sendmsg(conn_id, data, false) {
        Ok(n) => Ok(n),
        Err(EPIPE) => Err("Connection shutdown for writing"),
        Err(EAGAIN) => Err("Send buffer full"),
        Err(_) => Err("Connection not established"),
    }
}

/// Where a receive puts the bytes it takes.
pub enum RecvInto<'a> {
    /// Copy them here.
    Copy(&'a mut [u8]),
    /// Take up to this many without copying them (TCP's MSG_TRUNC).
    Discard(usize),
}

impl RecvInto<'_> {
    fn len(&self) -> usize {
        match self {
            RecvInto::Copy(buf) => buf.len(),
            RecvInto::Discard(len) => *len,
        }
    }
}

/// What a receive found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvResult {
    /// This many bytes.
    Data(usize),
    /// End of file: the peer's FIN, or SHUT_RD.
    Eof,
    /// The connection is closed without a FIN: a reset or a timeout, whose
    /// errno, if still unread, is the socket's SO_ERROR.
    Closed,
    /// Nothing yet.
    Empty,
}

/// Take up to `into.len()` received bytes, leaving them queued when `peek`, and
/// only once at least `min` of them are queued or no more can come
/// (MSG_WAITALL with MSG_PEEK). Never blocks. Bytes come first, then the end of
/// the stream, then an errno still pending on the connection, as Linux's
/// tcp_recvmsg orders them.
pub fn tcp_recvmsg(
    conn_id: &ConnectionId,
    into: RecvInto<'_>,
    peek: bool,
    min: usize,
) -> Result<RecvResult, i32> {
    with_tcp_connections(|connections| {
        let Some(conn) = connections.get_mut(conn_id) else {
            return Ok(RecvResult::Closed);
        };
        let queued = conn.rx_buffer.len();
        let ended = conn.peer_fin || conn.recv_shutdown || conn.state == TcpState::Closed;
        if queued > 0 && (queued >= min.min(into.len()) || ended) {
            let len = into.len().min(queued);
            if let RecvInto::Copy(buf) = into {
                let (front, back) = conn.rx_buffer.as_slices();
                let first = len.min(front.len());
                buf[..first].copy_from_slice(&front[..first]);
                buf[first..len].copy_from_slice(&back[..len - first]);
            }
            if !peek {
                conn.rx_buffer.drain(..len);
                update_window(conn);
            }
            return Ok(RecvResult::Data(len));
        }
        if conn.peer_fin || conn.recv_shutdown {
            return Ok(RecvResult::Eof);
        }
        if let Some(error) = conn.error.take() {
            return Err(error);
        }
        if conn.state == TcpState::Closed {
            return Ok(RecvResult::Closed);
        }
        Ok(RecvResult::Empty)
    })
}

/// recv(MSG_OOB): the urgent byte, as Linux's tcp_recv_urg gives it. EINVAL
/// when there is none to read, ENOTCONN on a connection closed by a reset,
/// Ok(None) at the end of the stream, EAGAIN while the announced byte has not
/// arrived. Never blocks.
pub fn tcp_recv_oob(conn_id: &ConnectionId, peek: bool) -> Result<Option<u8>, i32> {
    with_tcp_connections(|connections| {
        let Some(conn) = connections.get_mut(conn_id) else {
            return Err(EINVAL);
        };
        if conn.oob.is_none() && conn.rcv_up.is_none() {
            return Err(EINVAL);
        }
        if conn.state == TcpState::Closed && !conn.peer_fin {
            return Err(ENOTCONN);
        }
        if let Some(byte) = conn.oob {
            if !peek {
                conn.oob = None;
            }
            return Ok(Some(byte));
        }
        if conn.state == TcpState::Closed || conn.recv_shutdown || conn.peer_fin {
            return Ok(None);
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
    match tcp_recvmsg(conn_id, RecvInto::Copy(buf), false, 0) {
        Ok(RecvResult::Data(n)) => Ok(n),
        Ok(RecvResult::Empty) => Err("No data available"),
        // End of file, and a reset, read as 0 on this path.
        Ok(RecvResult::Eof | RecvResult::Closed) | Err(_) => Ok(0),
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
    with_connection_notify(conn_id, |conn| {
        if shut_rd {
            conn.recv_shutdown = true;
        }
        if shut_wr && !conn.send_shutdown {
            // Send FIN, after the queued data, to signal we're done sending
            conn.queue_fin();
        }
        wake_connection_waiters(conn);
    });
}

/// Close a connection: its owner has let go of it. `reuse` is the owner's
/// SO_REUSEADDR, which a connection waiting in TIME_WAIT keeps.
///
/// It sends a FIN after the queued data, unless bytes the owner never read are
/// still queued or `abort` is set (SO_LINGER with a zero timeout): then it
/// resets the connection, as Linux does (RFC 2525 2.17, RFC 9293 3.6), and the
/// peer's next operation fails with ECONNRESET.
pub fn tcp_close_connection(
    conn_id: &ConnectionId,
    abort: bool,
    reuse: bool,
) -> Result<(), &'static str> {
    with_tcp_connections(|connections| {
        let conn = connections.get_mut(conn_id).ok_or("Connection not found")?;
        conn.orphaned = true;
        conn.owner = None;
        conn.recv_shutdown = true;
        conn.reuse = reuse;
        let reset = abort || !conn.rx_buffer.is_empty();
        match conn.state {
            TcpState::Established
            | TcpState::CloseWait
            | TcpState::FinWait1
            | TcpState::FinWait2
            | TcpState::Closing
            | TcpState::LastAck
                if reset =>
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
            TcpState::FinWait1 | TcpState::Closing | TcpState::LastAck | TcpState::Closed => {}
        }
        reap_expired(connections, true);

        Ok(())
    })
}

/// Close a connection gracefully: `tcp_close_connection` without abort.
pub fn tcp_close(conn_id: &ConnectionId) -> Result<(), &'static str> {
    tcp_close_connection(conn_id, false, false)
}

/// True while a closed connection's FIN has not been acknowledged: the
/// states SO_LINGER waits out (Linux's tcp_close).
pub fn tcp_close_pending(conn_id: &ConnectionId) -> bool {
    with_tcp_connections(|connections| {
        connections.get(conn_id).is_some_and(|conn| {
            matches!(
                conn.state,
                TcpState::FinWait1 | TcpState::Closing | TcpState::LastAck
            )
        })
    })
}

/// Check if there's a connection ready to accept
pub fn tcp_has_pending(id: ListenerId) -> bool {
    tcp_pending_accept_depth(id) > 0
}

/// Return the number of connections ready for accept on a listener.
pub fn tcp_pending_accept_depth(id: ListenerId) -> usize {
    with_tcp_connections(|connections| {
        connections
            .values()
            .filter(|conn| conn.parent == Some(id) && conn.acceptable())
            .count()
    })
}

/// Get connection state for debugging and introspection
pub fn tcp_get_state(conn_id: &ConnectionId) -> Option<TcpState> {
    with_tcp_connections(|connections| connections.get(conn_id).map(|c| c.state))
}

// ============================================================================
// Blocking I/O support for the boot fixtures that drive a connection without a
// socket: waiter registration and wakeup by thread id.
// ============================================================================

/// Register a thread as waiting for data/state change on a connection
/// (recv/connect). False when there is no connection or no memory to record it.
pub fn tcp_register_recv_waiter(conn_id: &ConnectionId, thread_id: u64) -> bool {
    with_tcp_connections(|connections| {
        let Some(conn) = connections.get(conn_id) else {
            return false;
        };
        let mut waiting = conn.waiting_threads.lock();
        if waiting.contains(&thread_id) {
            return true;
        }
        if waiting.try_reserve(1).is_err() {
            return false;
        }
        waiting.push(thread_id);
        true
    })
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

/// Wake all threads waiting on a connection (called when its state changes),
/// and mark the change for the owning socket's next notice.
fn wake_connection_waiters(conn: &mut TcpConnection) {
    wake_connection(conn, CHANGED_ANY);
}

/// `wake_connection_waiters` for data or a FIN that arrived.
fn wake_connection_readers(conn: &mut TcpConnection) {
    wake_connection(conn, CHANGED_RX);
}

/// `wake_connection_waiters` for send space the peer's ACK freed.
fn wake_connection_writers(conn: &mut TcpConnection) {
    wake_connection(conn, CHANGED_TX);
}

fn wake_connection(conn: &mut TcpConnection, changed: u8) {
    conn.notify_pending |= changed;
    let mut waiting = conn.waiting_threads.lock();
    wake_waiting_threads(&mut waiting);
}

/// Wake every thread in `waiting`, keeping only those the scheduler refused,
/// for the next wake. Allocates nothing: it runs in the NetRx softirq.
fn wake_waiting_threads(waiting: &mut Vec<u64>) {
    waiting.retain(|&thread_id| {
        let rejected = crate::task::scheduler::wake_thread_any_context(thread_id)
            == crate::task::scheduler::WakeOutcome::Rejected;
        if rejected {
            TCP_WAKE_REJECTED.fetch_add(1, Ordering::Relaxed);
        }
        rejected
    });
}
