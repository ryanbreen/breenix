//! The fixed clock, the record of what TCP sent, and a scripted peer.
//!
//! TCP keeps its connections in one kernel-wide table, so the checks take
//! `lock` and run one at a time, each on its own local port.

use crate::net::tcp::{self, ConnectionId, SenderInfo, TcpHeader};
use crate::net::LOCAL_IP;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

pub const PEER_IP: [u8; 4] = [10, 0, 2, 2];
pub const PEER_PORT: u16 = 5001;
/// The MSS both ends offer, and so the connection's SMSS.
pub const SMSS: u32 = 1460;
const PEER_WINDOW: u16 = 65535;

static SERIAL: Mutex<()> = Mutex::new(());
static CLOCK_MS: AtomicU64 = AtomicU64::new(0);
static REFUSE: AtomicBool = AtomicBool::new(false);
static WIRE: Mutex<Vec<Sent>> = Mutex::new(Vec::new());
/// The largest TCP payload the route's device segments in one batch, if
/// the route has segmentation offload (by default it has not).
static BATCH_LIMIT: Mutex<Option<usize>> = Mutex::new(None);
/// Batches the device was given: (local port, sequence number, payload
/// length, MSS).
static BATCHES: Mutex<Vec<(u16, u32, usize, u16)>> = Mutex::new(Vec::new());

/// Run one check at a time, with the clock at `start_ms` and the record empty.
pub fn lock(start_ms: u64) -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    CLOCK_MS.store(start_ms, Ordering::SeqCst);
    REFUSE.store(false, Ordering::SeqCst);
    WIRE.lock().unwrap().clear();
    *BATCH_LIMIT.lock().unwrap() = None;
    BATCHES.lock().unwrap().clear();
    guard
}

/// Give the route a device that segments batches of up to `limit` payload
/// bytes, or take it away.
pub fn offer_batches(limit: Option<usize>) {
    *BATCH_LIMIT.lock().unwrap() = limit;
}

/// The batches sent from `local_port` since the last look, as (sequence
/// number, payload length, MSS).
pub fn take_batches(local_port: u16) -> Vec<(u32, usize, u16)> {
    let mut batches = BATCHES.lock().unwrap();
    let (mine, rest): (Vec<_>, Vec<_>) = batches.drain(..).partition(|b| b.0 == local_port);
    *batches = rest;
    mine.into_iter().map(|(_, seq, len, mss)| (seq, len, mss)).collect()
}

pub fn now_ms() -> u64 {
    CLOCK_MS.load(Ordering::SeqCst)
}

pub fn set_clock(ms: u64) {
    CLOCK_MS.store(ms, Ordering::SeqCst);
}

pub fn advance(ms: u64) {
    CLOCK_MS.fetch_add(ms, Ordering::SeqCst);
}

/// While set, IP output refuses each segment, as a full device queue does.
pub fn refuse_output(refuse: bool) {
    REFUSE.store(refuse, Ordering::SeqCst);
}

/// One segment TCP handed to IP output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub len: usize,
}

const SYN: u8 = 0x02;
const ACK: u8 = 0x10;

pub fn ip_output(src: [u8; 4], dst: [u8; 4], segment: &[u8]) -> Result<(), &'static str> {
    if REFUSE.load(Ordering::SeqCst) {
        return Err("No buffer space");
    }
    assert_eq!((src, dst), (LOCAL_IP, PEER_IP), "a segment left for an address no check uses");
    assert_eq!(tcp::tcp_checksum(src, dst, segment), 0, "TCP sent a segment with a bad checksum");
    let (header, payload) = TcpHeader::parse(segment).expect("TCP sent an unparseable segment");
    WIRE.lock().unwrap().push(Sent {
        src_port: header.src_port,
        dst_port: header.dst_port,
        seq: header.seq_num,
        ack: header.ack_num,
        flags: header.flags.to_byte(),
        len: payload.len(),
    });
    Ok(())
}

/// A connection this host opened to the scripted peer, which acknowledges what
/// the check tells it to and sends no data.
pub struct Flow {
    pub id: ConnectionId,
    /// The first sequence number of our data.
    pub data_start: u32,
    peer_seq: u32,
    window: u16,
}

impl Flow {
    /// connect() from `local_port`, answered with a SYN+ACK offering an MSS
    /// of `SMSS` and a 65535-byte window.
    pub fn connect(local_port: u16) -> Flow {
        Flow::open(local_port, SMSS as u16, false)
    }

    /// connect() from `local_port`, answered with a SYN+ACK offering an MSS
    /// of `mss` and a 65535-byte window. With `lose_syn`, the first SYN is
    /// lost and the SYN+ACK answers the one the retransmission timer resends.
    pub fn open(local_port: u16, mss: u16, lose_syn: bool) -> Flow {
        let id = tcp::tcp_connect(
            [0; 4],
            local_port,
            PEER_IP,
            PEER_PORT,
            crate::process::process::ProcessId(1),
            64,
            tcp::DEFAULT_RCVBUF,
        )
        .expect("connect");
        let sent = take_sent(local_port);
        assert_eq!(sent.len(), 1, "connect sent {sent:?}");
        assert_eq!(sent[0].flags & SYN, SYN);
        let isn = sent[0].seq;
        if lose_syn {
            let deadline = tcp::tcp_sender_info(&id).expect("the connection").rtx_deadline_ms;
            set_clock(deadline);
            tcp::run_timers();
            let resent = take_sent(local_port);
            assert_eq!(resent.len(), 1, "the SYN's timeout sent {resent:?}");
            assert_eq!((resent[0].flags & SYN, resent[0].seq), (SYN, isn));
        }
        let mut flow = Flow {
            id,
            data_start: isn.wrapping_add(1),
            peer_seq: 0x4000_0000,
            window: PEER_WINDOW,
        };
        let [hi, lo] = mss.to_be_bytes();
        flow.deliver(flow.peer_seq, flow.data_start, SYN | ACK, &[2, 4, hi, lo]);
        flow.peer_seq = flow.peer_seq.wrapping_add(1);
        assert_eq!(tcp::tcp_get_state(&id), Some(tcp::TcpState::Established));
        let sent = take_sent(local_port);
        assert_eq!(sent.len(), 1, "the handshake's ACK: {sent:?}");
        flow
    }

    /// The window the peer's next ACKs advertise.
    pub fn set_window(&mut self, window: u16) {
        self.window = window;
    }

    /// The application writes `len` bytes.
    pub fn write(&self, len: usize) {
        let bytes = vec![0x5a; len];
        assert_eq!(tcp::tcp_sendmsg(&self.id, &bytes, false), Ok(len));
    }

    /// The peer sends a pure ACK of `ack`.
    pub fn ack(&self, ack: u32) {
        self.deliver(self.peer_seq, ack, ACK, &[]);
    }

    /// Our data's sequence number `n` SMSS in.
    pub fn at(&self, n: u32) -> u32 {
        self.data_start.wrapping_add(n * SMSS)
    }

    /// The segments sent since the last look.
    pub fn sent(&self) -> Vec<Sent> {
        take_sent(self.id.local_port)
    }

    pub fn info(&self) -> SenderInfo {
        tcp::tcp_sender_info(&self.id).expect("the connection is in the table")
    }

    /// Hand the segment to TCP as the receive path does, then run deferred
    /// output as the receive path does once it has processed its batch.
    fn deliver(&self, seq: u32, ack: u32, flags: u8, options: &[u8]) {
        let mut segment = Vec::new();
        segment.extend_from_slice(&PEER_PORT.to_be_bytes());
        segment.extend_from_slice(&self.id.local_port.to_be_bytes());
        segment.extend_from_slice(&seq.to_be_bytes());
        segment.extend_from_slice(&ack.to_be_bytes());
        segment.push((((20 + options.len()) / 4) as u8) << 4);
        segment.push(flags);
        segment.extend_from_slice(&self.window.to_be_bytes());
        segment.extend_from_slice(&[0, 0, 0, 0]);
        segment.extend_from_slice(options);
        let checksum = tcp::tcp_checksum(PEER_IP, LOCAL_IP, &segment);
        segment[16..18].copy_from_slice(&checksum.to_be_bytes());
        let ip = crate::net::ipv4::Ipv4Packet {
            src_ip: PEER_IP,
            dst_ip: LOCAL_IP,
            payload: &segment,
        };
        tcp::handle_tcp(&ip, &segment);
        tcp::drain_deferred_tx();
    }
}

fn take_sent(local_port: u16) -> Vec<Sent> {
    let mut wire = WIRE.lock().unwrap();
    let (mine, rest): (Vec<_>, Vec<_>) = wire.drain(..).partition(|s| s.src_port == local_port);
    *wire = rest;
    mine
}

/// Data segments in `sent`, as (sequence number, length).
pub fn data(sent: &[Sent]) -> Vec<(u32, usize)> {
    sent.iter().filter(|s| s.len != 0).map(|s| (s.seq, s.len)).collect()
}

/// A counter from /proc/net/tcp_stats.
pub fn stat(name: &str) -> u64 {
    let text = tcp::proc_net_tcp_stats().expect("tcp_stats");
    text.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(": ")?.parse().ok())
        .unwrap_or_else(|| panic!("/proc/net/tcp_stats has no {name}: {text}"))
}
