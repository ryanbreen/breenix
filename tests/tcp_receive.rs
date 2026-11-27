//! Host wire checks for the production TCP receiver with an injected clock and NIC.
extern crate alloc;
extern crate self as log;
extern crate self as net;
extern crate self as spin;

use std::cell::{Cell, RefCell};

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { panic!($($arg)*) };
}

pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[path = "../kernel/src/syscall/errno.rs"]
pub mod errno;
pub mod syscall {
    pub use super::errno;
}
pub mod process {
    pub mod process {
        pub type ProcessId = u64;
    }
}
pub mod per_cpu {
    pub fn preempt_disable() {}
    pub fn preempt_enable() {}
}
pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}
struct TimerStep;
static TIMER_ENTRY: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

pub fn arch_halt_with_interrupts() {
    std::panic::resume_unwind(Box::new(TimerStep));
}

fn step_timer() {
    let stopped = std::panic::catch_unwind(*TIMER_ENTRY.get().unwrap()).unwrap_err();
    assert!(stopped.is::<TimerStep>());
}
pub mod task {
    pub mod scheduler {
        #[derive(PartialEq)]
        pub enum WakeOutcome {
            NoTarget,
            Woken,
        }
        pub fn wake_thread_any_context(_: u64) -> WakeOutcome {
            WakeOutcome::Woken
        }
        pub fn unblock_thread(_: u64) {}
        pub fn current_thread_id() -> Option<u64> {
            Some(1)
        }
        pub fn yield_current() {}
        pub struct Scheduler;
        impl Scheduler {
            pub fn block_current(&mut self) {}
            pub fn block_current_for_io(&mut self) {
                unreachable!()
            }
            pub fn block_current_for_timer(&mut self, _: u64) {}
        }
        pub fn with_current<R>(f: impl FnOnce(&mut Scheduler) -> R) -> Option<R> {
            Some(f(&mut Scheduler))
        }
    }
    pub mod kthread {
        pub struct Handle;
        impl Handle {
            pub fn tid(&self) -> u64 {
                1
            }
        }
        pub fn kthread_run(entry: fn(), _: &str) -> Result<Handle, ()> {
            assert!(crate::TIMER_ENTRY.set(entry).is_ok());
            Ok(Handle)
        }
    }
}
pub mod ipc {
    pub mod poll {
        pub mod events {
            pub const POLLIN: i16 = 1;
            pub const POLLPRI: i16 = 2;
            pub const POLLOUT: i16 = 4;
            pub const POLLERR: i16 = 8;
            pub const POLLHUP: i16 = 16;
            pub const POLLRDHUP: i16 = 0x2000;
        }
    }
}
pub mod socket {
    pub mod object {
        pub struct Socket {
            pub common: Common,
        }
        pub struct Common;
        impl Common {
            pub fn notify(&self) {}
            pub fn notify_readable(&self) {}
            pub fn notify_writable(&self) {}
            pub fn set_error(&self, _: i32) {}
        }
    }
}
pub mod time {
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        let ms = super::CLOCK.get();
        (ms / 1000, ms % 1000 * 1_000_000)
    }
}
pub mod ipv4 {
    pub const DEFAULT_TTL: u8 = 64;
    pub const PROTOCOL_TCP: u8 = 6;
    pub struct Ipv4Packet {
        pub src_ip: [u8; 4],
        pub dst_ip: [u8; 4],
    }
    impl Ipv4Packet {
        pub fn try_build(_: [u8; 4], _: [u8; 4], _: u8, _: u8, _: &[u8]) -> Result<Vec<u8>, ()> {
            unreachable!("test flows use loopback output")
        }
    }
}
pub mod ethernet {
    pub const ETHERTYPE_IPV4: u16 = 0x0800;
}
pub fn send_ethernet(_: &[u8; 6], _: u16, _: &[u8]) -> Result<(), ()> {
    unreachable!("test flows use loopback output")
}
pub fn net_lock_guard() {}
pub fn loopback_work_queued() {}
pub fn loopback_work_done() {}
pub fn drain_loopback_queue() {}
pub fn current_packet_src_mac() -> [u8; 6] {
    [0; 6]
}
pub struct Config {
    pub ip_addr: [u8; 4],
}
pub fn config() -> Config {
    Config { ip_addr: LOCAL }
}
pub fn local_ip_of_inbound(_: &Config, dst: [u8; 4]) -> [u8; 4] {
    dst
}
pub fn source_ip_in(config: &Config, _: [u8; 4]) -> [u8; 4] {
    config.ip_addr
}

const LOCAL: [u8; 4] = std::net::Ipv4Addr::LOCALHOST.octets();
const PEER: [u8; 4] = LOCAL;
const MSS: [u8; 4] = [2, 4, 5, 116];

thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(1000) };
    static REFUSE_NEXT_OUTPUT: Cell<bool> = const { Cell::new(false) };
    static OUTPUT: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}
pub fn send_ipv4_from(_: [u8; 4], _: [u8; 4], _: u8, _: u8, data: &[u8]) -> Result<(), ()> {
    if REFUSE_NEXT_OUTPUT.replace(false) {
        return Err(());
    }
    OUTPUT.with_borrow_mut(|packets| packets.push(data.to_vec()));
    Ok(())
}

#[allow(dead_code)]
pub mod icmp {
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct TcpMtuQuote {
        pub src: [u8; 4],
        pub dst: [u8; 4],
        pub src_port: u16,
        pub dst_port: u16,
    }
    impl TcpMtuQuote {
        pub(crate) fn smaller_mss(&self, _: u16, _: u32, _: usize) -> Option<u16> {
            None
        }
    }
}

#[allow(dead_code)]
#[path = "../kernel/src/net/tcp.rs"]
pub mod tcp;
use tcp::{ConnectionId, TcpFlags, TcpHeader};

// The production TCP table and timer are global even though the wire and
// clock belong to the test driving them.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
}

fn take_output() -> Vec<Vec<u8>> {
    OUTPUT.with_borrow_mut(std::mem::take)
}

fn receive(id: ConnectionId, seq: u32, ack: u32, flags: TcpFlags, options: &[u8], payload: &[u8]) {
    let mut packet = tcp::build_tcp_packet(
        id.remote_port,
        id.local_port,
        seq,
        ack,
        flags,
        60000,
        payload,
    );
    packet.splice(20..20, options.iter().copied());
    packet[12] = ((20 + options.len()) / 4) as u8 * 16;
    let checksum = tcp::tcp_checksum(PEER, LOCAL, &packet);
    packet[16..18].copy_from_slice(&checksum.to_be_bytes());
    tcp::handle_tcp(
        &ipv4::Ipv4Packet {
            src_ip: PEER,
            dst_ip: LOCAL,
        },
        &packet,
    );
}

fn handshake(port: u16, seq: u32, options: &[u8]) -> (ConnectionId, u32, Vec<u8>) {
    let id = ConnectionId {
        local_ip: LOCAL,
        local_port: port,
        remote_ip: PEER,
        remote_port: 44000,
    };
    let listener =
        tcp::tcp_listen(LOCAL, port, 4, 1, false, ipv4::DEFAULT_TTL, 212_992, None).unwrap();
    receive(id, seq.wrapping_sub(1), 0, TcpFlags::syn(), options, &[]);
    tcp::drain_deferred_tx();
    let syn_ack = take_output().pop().unwrap();
    let header = TcpHeader::parse(&syn_ack).unwrap().0;
    assert!(header.flags.syn && header.flags.ack);
    let ack = header.seq_num.wrapping_add(1);
    receive(id, seq, ack, TcpFlags::ack(), &[], &[]);
    assert_eq!(tcp::tcp_accept(listener), Some(id));
    tcp::tcp_unlisten(listener);
    (id, ack, syn_ack)
}

fn ack_fields(packet: &[u8]) -> (u32, u16, &[u8]) {
    let header_len = (packet[12] >> 4) as usize * 4;
    let header = TcpHeader::parse(packet).unwrap().0;
    assert!(header.flags.ack);
    (header.ack_num, header.window_size, &packet[20..header_len])
}

#[test]
fn receive_path_releases_queued_bytes_and_fin_across_sequence_wrap() {
    let _serial = serial();
    let start = u32::MAX - 1;
    let (id, ack, syn_ack) = handshake(40001, start, &MSS);
    assert_eq!(ack_fields(&syn_ack).2, &[2, 4, 64, 0]);
    receive(id, 1, ack, TcpFlags::fin_ack(), &[], b"def");
    let mut output = [0; 16];
    assert!(!tcp::tcp_has_data(&id));
    let gap_ack = take_output();
    assert_eq!(ack_fields(&gap_ack[0]).0, start);
    assert!(ack_fields(&gap_ack[0]).2.is_empty());
    receive(id, start, ack, TcpFlags::ack(), &[], b"abc");
    assert_eq!(tcp::tcp_recv(&id, &mut output), Ok(6));
    assert_eq!(&output[..6], b"abcdef");
    assert_eq!(tcp::tcp_recv(&id, &mut output), Ok(0));
    assert_eq!(ack_fields(&take_output()[0]).0, 5);
}

#[test]
fn receive_path_negotiates_sack_and_prioritizes_newest_merged_block() {
    let _serial = serial();
    let options = [2, 4, 5, 116, 4, 2, 0, 0];
    let (id, ack, syn_ack) = handshake(40002, 100, &options);
    assert_eq!(ack_fields(&syn_ack).2, &[2, 4, 64, 0, 4, 2, 0, 0]);
    for (seq, data) in [(106, b"ghi"), (112, b"mno"), (109, b"jkl")] {
        receive(id, seq, ack, TcpFlags::ack(), &[], data);
        let packets = take_output();
        assert_eq!(ack_fields(&packets[0]).0, 100);
        if seq == 106 {
            let options = ack_fields(&packets[0]).2;
            assert_eq!(
                options.get(..10),
                Some(&[5, 10, 0, 0, 0, 106, 0, 0, 0, 109][..])
            );
        }
        if seq == 109 {
            let options = ack_fields(&packets[0]).2;
            assert_eq!(&options[..10], &[5, 10, 0, 0, 0, 106, 0, 0, 0, 115]);
        }
    }
    receive(id, 100, ack, TcpFlags::ack(), &[], b"abcdef");
    assert_eq!(ack_fields(&take_output()[0]).0, 115);
    let mut output = [0; 15];
    assert_eq!(tcp::tcp_recv(&id, &mut output), Ok(15));
    assert_eq!(&output, b"abcdefghijklmno");
}

#[test]
fn receive_path_scales_only_after_syn_and_reopens_a_closed_window() {
    let _serial = serial();
    let options = [2, 4, 5, 116, 3, 3, 5, 0];
    let (id, ack, syn_ack) = handshake(40003, 1000, &options);
    let (_, field, options) = ack_fields(&syn_ack);
    assert_eq!(field, u16::MAX);
    let option = options.windows(3).find(|option| option[..2] == [3, 3]);
    let shift = option.expect("SYN+ACK must echo the offer")[2];
    assert!(shift > 0 && shift <= 14);
    tcp::tcp_set_options(&id, || tcp::ConnOptions {
        rcvbuf: 212_992,
        sndbuf: 212_992,
        keepalive: false,
        reuse: false,
        ttl: 64,
    });
    receive(id, 1000, ack, TcpFlags::ack(), &[], &vec![0; 1396]);
    receive(id, 2396, ack, TcpFlags::ack(), &[], &vec![0; 1396]);
    let packets = take_output();
    let (_, window, _) = ack_fields(packets.last().unwrap());
    let remaining = (window as usize) << shift;
    assert!(remaining > 65_535);
    let capacity = 106_496;
    assert!(remaining <= capacity - 2792 && capacity - 2792 - remaining < 1 << shift);

    let mut seq = 3792;
    for chunk in vec![0; remaining].chunks(1396) {
        receive(id, seq, ack, TcpFlags::ack(), &[], chunk);
        seq += chunk.len() as u32;
    }
    assert_eq!(ack_fields(take_output().last().unwrap()).1, 0);
    let mut buffer = [0; 1396];
    assert_eq!(tcp::tcp_recv(&id, &mut buffer), Ok(1396));
    let update = take_output();
    assert_eq!(update.len(), 1, "window must reopen when the app reads");
    assert!(ack_fields(&update[0]).1 > 0);
}

#[test]
fn receive_path_clips_hole_and_fin_at_the_advertised_right_edge() {
    let _serial = serial();
    let (id, ack, _) = handshake(40004, 100, &MSS);
    // The SYN window is 65535. The last two bytes straddle its right edge;
    // neither the byte beyond the edge nor the FIN may be accepted yet.
    receive(id, 65634, ack, TcpFlags::fin_ack(), &[], b"xy");
    assert_eq!(ack_fields(&take_output()[0]).0, 100);
    let mut seq = 100;
    for chunk in vec![0; 65534].chunks(1396) {
        receive(id, seq, ack, TcpFlags::ack(), &[], chunk);
        seq += chunk.len() as u32;
    }
    assert_eq!(ack_fields(take_output().last().unwrap()).0, 65635);
    let mut output = vec![0; 65536];
    assert_eq!(tcp::tcp_recv(&id, &mut output), Ok(65535));
    assert_eq!(output[65534], b'x');
    assert!(!tcp::tcp_has_data(&id));
}

#[test]
fn receive_path_acknowledges_a_pair_or_flushes_at_the_timer_deadline() {
    let _serial = serial();
    let (id, ack, _) = handshake(40005, 500, &MSS);
    receive(id, 500, ack, TcpFlags::ack(), &[], b"abc");
    assert!(take_output().is_empty());
    receive(id, 503, ack, TcpFlags::ack(), &[], b"def");
    let output = take_output();
    assert_eq!(output.len(), 1);
    assert_eq!(ack_fields(&output[0]).0, 506);
    receive(id, 506, ack, TcpFlags::ack(), &[], b"ghi");
    assert!(take_output().is_empty());
    CLOCK.set(1009);
    step_timer();
    assert!(take_output().is_empty());
    CLOCK.set(1010);
    step_timer();
    let output = take_output();
    assert_eq!(output.len(), 1);
    assert_eq!(ack_fields(&output[0]).0, 509);
}

#[test]
fn receive_path_read_preserves_pending_ack_unless_window_closed() {
    let _serial = serial();
    let options = [2, 4, 5, 116, 3, 3, 0, 0];
    let (id, ack, _) = handshake(40006, 300, &options);
    tcp::tcp_set_options(&id, || tcp::ConnOptions {
        rcvbuf: 212_992,
        sndbuf: 212_992,
        keepalive: false,
        reuse: false,
        ttl: 64,
    });
    take_output();
    let payload = [0x5a; 1396];
    receive(id, 300, ack, TcpFlags::ack(), &[], &payload);
    assert!(take_output().is_empty());
    let mut output = [0; 1396];
    assert_eq!(tcp::tcp_recv(&id, &mut output), Ok(1396));
    assert_eq!(output, payload);
    assert!(take_output().is_empty());

    receive(id, 1696, ack, TcpFlags::ack(), &[], &payload);
    let packets = take_output();
    assert_eq!(packets.len(), 1);
    assert_eq!(ack_fields(&packets[0]).0, 3092);
}

#[test]
fn receive_path_retries_refused_immediate_ack_with_current_sack() {
    let _serial = serial();
    let options = [2, 4, 5, 116, 4, 2, 0, 0];
    let (id, ack, _) = handshake(40007, 500, &options);
    REFUSE_NEXT_OUTPUT.set(true);
    receive(id, 503, ack, TcpFlags::ack(), &[], b"def");
    assert!(take_output().is_empty());
    CLOCK.set(1010);
    step_timer();
    let packets = take_output();
    assert_eq!(packets.len(), 1);
    let (ack, _, options) = ack_fields(&packets[0]);
    assert_eq!(ack, 500);
    assert_eq!(&options[..10], &[5, 10, 0, 0, 1, 247, 0, 0, 1, 250]);
}

fn retransmit_timeouts() -> u64 {
    let text = tcp::proc_net_tcp_stats().unwrap();
    let value = text.strip_prefix("retransmit_timeouts: ").unwrap();
    value.trim_end().parse().unwrap()
}

#[test]
fn retransmission_timeouts_count_each_resend_and_nothing_else() {
    let _serial = serial();
    let before = retransmit_timeouts();
    let id = tcp::tcp_connect(LOCAL, 40012, PEER, 44012, 1, 64, 212_992).unwrap();
    let syn = take_output().remove(0);
    let syn_seq = TcpHeader::parse(&syn).unwrap().0.seq_num;

    // The SYN's timer runs out: it is sent again and counted.
    CLOCK.set(CLOCK.get() + 1000);
    step_timer();
    assert!(take_output().contains(&syn));
    assert_eq!(retransmit_timeouts(), before + 1);

    let ack = syn_seq.wrapping_add(1);
    receive(id, 7000 - 1, ack, TcpFlags::syn_ack(), &MSS, &[]);
    take_output();
    assert_eq!(tcp::tcp_sendmsg(&id, &[7; 100], false), Ok(100));
    assert_eq!(take_output().len(), 1);

    // Unacknowledged data: the timeout sends it again and counts once.
    CLOCK.set(CLOCK.get() + 10_000);
    step_timer();
    let resent = take_output();
    assert!(resent.iter().any(|packet| TcpHeader::parse(packet).unwrap().1 == [7; 100]));
    assert_eq!(retransmit_timeouts(), before + 2);

    // With the data acknowledged, a later timer pass leaves the count at two.
    receive(id, 7000, ack.wrapping_add(100), TcpFlags::ack(), &[], &[]);
    take_output();
    CLOCK.set(CLOCK.get() + 200_000);
    step_timer();
    assert_eq!(retransmit_timeouts(), before + 2);
}

#[test]
fn receive_path_uses_scaled_peer_window_only_after_active_handshake() {
    let _serial = serial();
    let id = tcp::tcp_connect(LOCAL, 40008, PEER, 44000, 1, 64, 212_992).unwrap();
    let packets = take_output();
    let syn = TcpHeader::parse(&packets[0]).unwrap().0;
    assert!(syn.flags.syn && !syn.flags.ack);
    assert_eq!(syn.window_size, u16::MAX);
    assert_eq!(
        &packets[0][20..],
        &[2, 4, 64, 0, 3, 3, 1, 4, 2, 0, 0, 0]
    );

    CLOCK.set(2000);
    step_timer();
    let retry = take_output();
    assert!(retry.iter().any(|packet| packet == &packets[0]));

    let start = 1200;
    let ack = syn.seq_num.wrapping_add(1);
    let options = [2, 4, 5, 116, 3, 3, 1, 0];
    receive(id, start - 1, ack, TcpFlags::syn_ack(), &options, &[]);
    let handshake_ack = take_output();
    let (_, window, options) = ack_fields(&handshake_ack[0]);
    assert_eq!(usize::from(window) << 1, 106_496);
    assert!(options.is_empty());

    tcp::tcp_set_options(&id, || tcp::ConnOptions {
        rcvbuf: 212_992,
        sndbuf: 212_992,
        keepalive: false,
        reuse: false,
        ttl: 64,
    });
    let data = vec![0; 85_000];
    assert_eq!(tcp::tcp_sendmsg(&id, &data, false), Ok(data.len()));
    let payload_len = |packets: Vec<Vec<u8>>| -> usize {
        packets
            .iter()
            .map(|p| TcpHeader::parse(p).unwrap().1.len())
            .sum()
    };
    assert_eq!(
        payload_len(take_output()),
        60_000,
        "SYN+ACK window is unscaled"
    );
    receive(id, start, ack, TcpFlags::ack(), &[], &[]);
    tcp::drain_deferred_tx();
    assert_eq!(
        payload_len(take_output()),
        25_000,
        "ordinary peer ACK scales the window"
    );
}
