//! DHCP exchanges against a deterministic wire peer, using production parsers.
extern crate alloc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
#[path = "../kernel/src/net/dhcp.rs"]
pub mod dhcp;
#[path = "../kernel/src/net/ethernet.rs"]
pub mod ethernet;
#[path = "../kernel/src/net/ipv4.rs"]
pub mod ipv4;
#[path = "../kernel/src/net/udp.rs"]
pub mod udp;
#[derive(Debug)]
pub enum TxError {
    NoBufs,
    MsgSize,
}
static TCP_DELIVERIES: AtomicU64 = AtomicU64::new(0);
pub mod tcp {
    pub fn handle_tcp(_: &crate::ipv4::Ipv4Packet, _: &[u8]) {
        super::TCP_DELIVERIES.fetch_add(1, super::Ordering::Relaxed);
    }
}
pub mod icmp {
    pub struct IcmpPacket;
    impl IcmpPacket {
        pub fn parse(_: &[u8]) -> Option<Self> {
            None
        }
    }
    pub fn handle_icmp(_: &crate::ipv4::Ipv4Packet, _: &IcmpPacket) {}
    pub fn send_port_unreachable(_: &crate::ipv4::Ipv4Packet, _: &[u8]) {}
}
pub mod socket {
    pub mod udp {
        pub fn deliver(_: [u8; 4], _: u16, _: [u8; 4], _: u16, _: &[u8]) -> bool {
            false
        }
        pub fn udp_icmp_error(_: [u8; 4], _: u16, _: [u8; 4], _: u16, _: i32) {}
    }
}
pub mod arp {
    pub const ARP_OP_REQUEST: u16 = 1;
    pub struct ArpPacket;
    impl ArpPacket {
        pub fn build(_: u16, mac: &[u8; 6], src: &[u8; 4], _: &[u8; 6], ip: &[u8; 4]) -> [u8; 28] {
            let mut p = [0; 28];
            p[8..14].copy_from_slice(mac);
            p[14..18].copy_from_slice(src);
            p[24..28].copy_from_slice(ip);
            p
        }
    }
    pub fn clear() {}
}
static CLOCK_STEP: AtomicU64 = AtomicU64::new(100_000_000);
static CLOCK: AtomicU64 = AtomicU64::new(1_000_000_000);
pub mod time {
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        let t = super::CLOCK.load(super::Ordering::Relaxed);
        (t / 1_000_000_000, t % 1_000_000_000)
    }
    pub mod tsc {
        pub fn nanoseconds_since_base() -> Option<u64> {
            Some(super::super::CLOCK.load(super::super::Ordering::Relaxed))
        }
    }
}
pub fn net_lock_guard() {}
pub struct Config {
    pub ip_addr: [u8; 4],
}
pub fn config() -> Config {
    Config {
        ip_addr: dhcp::lease().map(|l| l.ip_addr).unwrap_or([0; 4]),
    }
}
pub fn is_broadcast_addr_in(_: &Config, ip: [u8; 4]) -> bool {
    ip == [255; 4]
}
const MAC: [u8; 6] = [2, 3, 4, 5, 6, 7];
const ADDRESS: [u8; 4] = [192, 0, 2, 10];
const SERVER: [u8; 4] = [192, 0, 2, 1];
#[derive(Default)]
struct Peer {
    lease_seconds: Option<u32>,
    bad_ack: u8,
    corrupt: u8,
    overload: bool,
    conflict: bool,
    no_router: bool,
    silent: bool,
    no_buffers: bool,
    nak: bool,
    pending: Option<Vec<u8>>,
    sent: Vec<([u8; 4], Vec<u8>)>,
    declines: usize,
}
static PEER: Mutex<Peer> = Mutex::new(Peer {
    lease_seconds: None,
    bad_ack: 0,
    corrupt: 0,
    overload: false,
    conflict: false,
    no_router: false,
    silent: false,
    no_buffers: false,
    nak: false,
    pending: None,
    sent: Vec::new(),
    declines: 0,
});
fn options(p: &Peer, kind: u8) -> Vec<u8> {
    let server = if p.bad_ack == 1 && kind == 5 {
        [192, 0, 2, 99]
    } else {
        SERVER
    };
    let mut o = vec![53, 1, kind, 54, 4];
    o.extend(server);
    o.extend([1, 4, 255, 255, 255, 0, 51, 4]);
    o.extend(p.lease_seconds.unwrap_or(120).to_be_bytes());
    if p.lease_seconds.is_none() {
        o.extend([58, 4, 0, 0, 0, 60, 59, 4, 0, 0, 0, 105]);
    }
    if !p.no_router {
        o.extend([3, 4, 192, 0, 2, 1]);
    }
    o.extend([6, 8, 192, 0, 2, 53, 192, 0, 2, 54, 255]);
    o
}
fn send(src: [u8; 4], dst: [u8; 4], data: &[u8]) -> Result<(), &'static str> {
    let (_, msg) = udp::UdpHeader::parse(data).unwrap();
    let mut p = PEER.lock().unwrap();
    if p.no_buffers {
        return Err("No buffers");
    }
    let kind = msg[242];
    p.sent.push((dst, msg.to_vec()));
    if kind == 4 {
        p.declines += 1;
        return Ok(());
    }
    if p.silent {
        return Ok(());
    }
    let response_kind = if kind == 1 {
        2
    } else if p.nak {
        6
    } else {
        5
    };
    let mut reply = msg[..240].to_vec();
    reply[0] = 2;
    let address = match p.bad_ack {
        2 if response_kind == 5 => [192, 0, 2, 11],
        3 if response_kind == 5 => [0; 4],
        _ => ADDRESS,
    };
    reply[16..20].copy_from_slice(&address);
    let opts = options(&p, response_kind);
    if p.overload {
        reply[44..108].fill(0);
        reply[108..236].fill(0);
        reply[108..108 + opts.len()].copy_from_slice(&opts);
        reply.extend([52, 1, 3, 255]);
        reply[44] = 255;
    } else {
        reply.extend(opts);
    }
    let datagram = udp::try_build_udp_packet(SERVER, [255; 4], 67, 68, &reply).unwrap();
    let mut ip = ipv4::Ipv4Packet::try_build(SERVER, [255; 4], 17, &datagram).unwrap();
    match p.corrupt {
        1 => ip[10] ^= 1,
        2 => {
            *ip.last_mut().unwrap() ^= 1;
        }
        3 => {
            ip[6] = 0x20;
            ip[10..12].fill(0);
            let sum = ipv4::internet_checksum(&ip[..20]);
            ip[10..12].copy_from_slice(&sum.to_be_bytes());
        }
        _ => {}
    }
    p.pending = Some(ip);
    let _ = src;
    Ok(())
}
pub fn send_ethernet(_: &[u8; 6], protocol: u16, data: &[u8]) -> Result<(), &'static str> {
    if protocol == ethernet::ETHERTYPE_ARP {
        if PEER.lock().unwrap().conflict {
            dhcp::observe_arp([9; 6], [data[24], data[25], data[26], data[27]], [0; 4]);
        }
        return Ok(());
    }
    let ip = ipv4::Ipv4Packet::parse(data).unwrap();
    send(ip.src_ip, ip.dst_ip, ip.payload)
}
pub fn ip_output(src: [u8; 4], dst: [u8; 4], _: u8, data: &[u8]) -> Result<(), TxError> {
    send(src, dst, data).map_err(|_| TxError::NoBufs)
}
pub fn process_rx_budgeted(_: usize) {
    CLOCK.fetch_add(CLOCK_STEP.load(Ordering::Relaxed), Ordering::Relaxed);
    let packet = PEER.lock().unwrap().pending.take();
    if let Some(packet) = packet {
        let ip = ipv4::Ipv4Packet::parse(&packet).unwrap();
        let eth = ethernet::EthernetFrame {
            src_mac: [0; 6],
            dst_mac: MAC,
            ethertype: 0x800,
            payload: &packet,
        };
        ipv4::handle_ipv4(&eth, &ip);
    }
}
fn setup() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let guard = SERIAL.lock().unwrap();
    *PEER.lock().unwrap() = Peer::default();
    CLOCK.store(1_000_000_000, Ordering::Relaxed);
    CLOCK_STEP.store(100_000_000, Ordering::Relaxed);
    dhcp::maintain(MAC, false);
    dhcp::maintain(MAC, true);
    PEER.lock().unwrap().sent.clear();
    guard
}
#[test]
fn overload_dns_and_gatewayless_lan() {
    let _guard = setup();
    {
        let mut p = PEER.lock().unwrap();
        p.overload = true;
        p.no_router = true;
    }
    let l = dhcp::acquire(MAC).unwrap();
    assert_eq!(l.gateway, [0; 4]);
    assert_eq!(
        l.nameservers,
        [Some([192, 0, 2, 53]), Some([192, 0, 2, 54]), None]
    );
}
#[test]
fn mismatched_server_address_and_zero_ack_are_refused() {
    let _guard = setup();
    for bad in 1..=3 {
        PEER.lock().unwrap().bad_ack = bad;
        assert!(dhcp::acquire(MAC).is_none());
    }
}
#[test]
fn corrupt_checksums_and_fragments_cannot_configure_interface() {
    let _guard = setup();
    for corrupt in 1..=3 {
        PEER.lock().unwrap().corrupt = corrupt;
        assert!(dhcp::acquire(MAC).is_none());
    }
}
#[test]
fn conflict_declines_and_never_installs_address() {
    let _guard = setup();
    PEER.lock().unwrap().conflict = true;
    assert!(dhcp::acquire(MAC).is_none());
    assert_eq!(PEER.lock().unwrap().declines, 3);
    assert!(dhcp::lease().is_none());
}
#[test]
fn renew_rebind_expire_and_carrier_handoff() {
    let _guard = setup();
    let l = dhcp::acquire(MAC).unwrap();
    CLOCK.store(l.renew_at, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    {
        let p = PEER.lock().unwrap();
        let (dst, msg) = p.sent.last().unwrap();
        assert_eq!(*dst, SERVER);
        assert_eq!(&msg[12..16], &ADDRESS);
        assert_eq!(&msg[10..12], &[0, 0]);
    }
    let l = dhcp::lease().unwrap();
    CLOCK.store(l.rebind_at, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert_eq!(PEER.lock().unwrap().sent.last().unwrap().0, [255; 4]);
    let l = dhcp::lease().unwrap();
    CLOCK.store(l.expires_at, Ordering::Relaxed);
    assert!(dhcp::lease().is_none());
    PEER.lock().unwrap().silent = true;
    dhcp::maintain(MAC, true);
    assert!(dhcp::lease().is_none());
    PEER.lock().unwrap().silent = false;
    let _ = dhcp::acquire(MAC).unwrap();
    dhcp::maintain(MAC, false);
    assert!(dhcp::lease().is_none());
}
#[test]
fn renewal_nak_removes_address() {
    let _guard = setup();
    let l = dhcp::acquire(MAC).unwrap();
    PEER.lock().unwrap().nak = true;
    CLOCK.store(l.renew_at, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert!(dhcp::lease().is_none());
}
#[test]
fn bounded_discovery_and_transmit_allocation_failure() {
    let _guard = setup();
    PEER.lock().unwrap().silent = true;
    let start = CLOCK.load(Ordering::Relaxed);
    assert!(dhcp::acquire(MAC).is_none());
    assert!(CLOCK.load(Ordering::Relaxed) - start <= 11_000_000_000);
    {
        let mut p = PEER.lock().unwrap();
        p.silent = false;
        p.no_buffers = true;
    }
    assert!(dhcp::acquire(MAC).is_none());
}

// Compile the actual procfs snapshot code, including its allocation failures.
pub mod net {
    pub use crate::dhcp;
}
#[path = "../kernel/src/fs/procfs/net.rs"]
mod proc_net;
std::thread_local! { static FAIL_ALLOC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
struct FailingAllocator;
unsafe impl std::alloc::GlobalAlloc for FailingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if FAIL_ALLOC.try_with(|f| f.get()).unwrap_or(false) {
            core::ptr::null_mut()
        } else {
            std::alloc::System.alloc(layout)
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        std::alloc::System.dealloc(ptr, layout);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        if FAIL_ALLOC.try_with(|f| f.get()).unwrap_or(false) {
            core::ptr::null_mut()
        } else {
            std::alloc::System.realloc(ptr, layout, size)
        }
    }
}
#[global_allocator]
static ALLOCATOR: FailingAllocator = FailingAllocator;
#[test]
fn procfs_secondary_dns_and_enomem() {
    let _guard = setup();
    dhcp::acquire(MAC).unwrap();
    let text = proc_net::generate_pnp().unwrap();
    assert!(text.contains("nameserver 192.0.2.53\nnameserver 192.0.2.54\n"));
    assert_eq!(proc_net::list_entries().unwrap(), ["pnp"]);
    FAIL_ALLOC.with(|f| f.set(true));
    let pnp = proc_net::generate_pnp();
    let entries = proc_net::list_entries();
    let lease = dhcp::acquire(MAC);
    FAIL_ALLOC.with(|f| f.set(false));
    assert_eq!(pnp, Err(-12));
    assert_eq!(entries, Err(-12));
    assert!(
        lease.is_none(),
        "DHCP allocation failure must be a bounded failure"
    );
}

#[test]
fn a_late_renewal_ack_cannot_resurrect_an_expired_address() {
    let _guard = setup();
    let lease = dhcp::acquire(MAC).unwrap();
    CLOCK.store(lease.expires_at - 500_000_000, Ordering::Relaxed);
    CLOCK_STEP.store(2_000_000_000, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert!(dhcp::lease().is_none());
}

#[test]
fn long_and_infinite_leases_keep_valid_default_deadlines() {
    let _guard = setup();
    PEER.lock().unwrap().lease_seconds = Some(u32::MAX);
    let lease = dhcp::acquire(MAC).unwrap();
    assert_eq!(lease.expires_at, u64::MAX);
    assert_eq!(lease.renew_at, u64::MAX);
    assert_eq!(lease.rebind_at, u64::MAX);
    PEER.lock().unwrap().lease_seconds = Some(u32::MAX - 1);
    let lease = dhcp::acquire(MAC).unwrap();
    assert!(lease.renew_at < lease.rebind_at && lease.rebind_at < lease.expires_at);
    assert!(dhcp::lease().is_some());
}

#[test]
fn physical_ingress_cannot_reach_or_spoof_loopback_services() {
    let _guard = setup();
    dhcp::acquire(MAC).unwrap();
    TCP_DELIVERIES.store(0, Ordering::Relaxed);
    for (src, dst) in [(SERVER, [127, 0, 0, 1]), ([127, 0, 0, 1], ADDRESS)] {
        let mut syn = [0u8; 20];
        syn[..2].copy_from_slice(&40000u16.to_be_bytes());
        syn[2..4].copy_from_slice(&2323u16.to_be_bytes());
        syn[12] = 0x50;
        syn[13] = 2;
        let mut pseudo = [0u8; 12];
        pseudo[..4].copy_from_slice(&src);
        pseudo[4..8].copy_from_slice(&dst);
        pseudo[9] = 6;
        pseudo[11] = 20;
        let mut sum = u32::from(!ipv4::internet_checksum(&pseudo))
            + u32::from(!ipv4::internet_checksum(&syn));
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        syn[16..18].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        let packet = ipv4::Ipv4Packet::try_build(src, dst, 6, &syn).unwrap();
        let ip = ipv4::Ipv4Packet::parse(&packet).unwrap();
        let frame = ethernet::EthernetFrame {
            src_mac: [9; 6],
            dst_mac: MAC,
            ethertype: 0x800,
            payload: &packet,
        };
        ipv4::handle_ipv4(&frame, &ip);
    }
    assert_eq!(TCP_DELIVERIES.load(Ordering::Relaxed), 0);
    let packet = ipv4::Ipv4Packet::try_build([127, 0, 0, 1], [127, 0, 0, 2], 6, &[0; 20]).unwrap();
    let ip = ipv4::Ipv4Packet::parse(&packet).unwrap();
    ipv4::handle_loopback_ipv4(&ip);
    assert_eq!(
        TCP_DELIVERIES.load(Ordering::Relaxed),
        1,
        "trusted loopback remains deliverable"
    );
}
