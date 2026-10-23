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
    pub fn handle_icmp(_: &crate::ipv4::Ipv4Packet, _: &IcmpPacket) -> crate::ipv4::Handled {
        crate::ipv4::Handled::Done
    }
    pub fn send_port_unreachable(_: &crate::ipv4::Ipv4Packet, _: &[u8]) {}
}
pub mod socket {
    pub mod udp {
        pub enum Delivery {
            Taken,
            NoReceiver,
            RegistryBusy,
        }
        pub fn deliver(_: [u8; 4], _: u16, _: [u8; 4], _: u16, _: &[u8]) -> Delivery {
            Delivery::NoReceiver
        }
        pub fn udp_icmp_error(_: [u8; 4], _: u16, _: [u8; 4], _: u16, _: i32) -> Delivery {
            Delivery::Taken
        }
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
static ACK_DELAY: AtomicU64 = AtomicU64::new(0);
static REQUEST_TIME: AtomicU64 = AtomicU64::new(0);
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
    drop_first_discover: bool,
    no_buffers: bool,
    nak: bool,
    pending: Option<Vec<u8>>,
    sent: Vec<([u8; 4], Vec<u8>)>,
    declines: usize,
    sources: Vec<[u8; 4]>,
    sent_at: Vec<u64>,
}
static PEER: Mutex<Peer> = Mutex::new(Peer {
    lease_seconds: None,
    bad_ack: 0,
    corrupt: 0,
    overload: false,
    conflict: false,
    no_router: false,
    silent: false,
    drop_first_discover: false,
    no_buffers: false,
    nak: false,
    pending: None,
    sent: Vec::new(),
    declines: 0,
    sources: Vec::new(),
    sent_at: Vec::new(),
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
    o.extend([6, 8, 192, 0, 2, 53, 192, 0, 2, 54, 42, 8, 192, 0, 2, 123, 192, 0, 2, 124, 255]);
    o
}
fn send(src: [u8; 4], dst: [u8; 4], data: &[u8]) -> Result<(), &'static str> {
    let (_, msg) = udp::UdpHeader::parse(data).unwrap();
    let mut p = PEER.lock().unwrap();
    if p.no_buffers {
        return Err("No buffers");
    }
    let kind = msg[242];
    if kind == 3 {
        REQUEST_TIME.store(CLOCK.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    p.sent.push((dst, msg.to_vec()));
    p.sources.push(src);
    p.sent_at.push(CLOCK.load(Ordering::Relaxed));
    if kind == 4 {
        p.declines += 1;
        return Ok(());
    }
    if p.drop_first_discover
        && kind == 1
        && p.sent.iter().filter(|(_, msg)| msg[242] == 1).count() == 1
    {
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
    let mut ip =
        ipv4::Ipv4Packet::try_build(SERVER, [255; 4], 17, ipv4::DEFAULT_TTL, &datagram).unwrap();
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
pub fn ip_output(src: [u8; 4], dst: [u8; 4], _: u8, _: u8, data: &[u8]) -> Result<(), TxError> {
    send(src, dst, data).map_err(|_| TxError::NoBufs)
}
pub fn process_rx_budgeted(_: usize) {
    CLOCK.fetch_add(CLOCK_STEP.load(Ordering::Relaxed), Ordering::Relaxed);
    let packet = PEER.lock().unwrap().pending.take();
    if let Some(packet) = packet {
        if packet.get(20 + 8 + 242) == Some(&5) {
            CLOCK.fetch_add(ACK_DELAY.load(Ordering::Relaxed), Ordering::Relaxed);
        }
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
pub fn between_rx_polls() {}
fn setup() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let guard = SERIAL.lock().unwrap();
    *PEER.lock().unwrap() = Peer::default();
    CLOCK.store(u64::MAX, Ordering::Relaxed);
    dhcp::maintain(MAC, false);
    CLOCK.store(1_000_000_000, Ordering::Relaxed);
    CLOCK_STEP.store(100_000_000, Ordering::Relaxed);
    ACK_DELAY.store(0, Ordering::Relaxed);
    REQUEST_TIME.store(0, Ordering::Relaxed);
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
    {
        let peer = PEER.lock().unwrap();
        let (dst, msg) = peer.sent.last().unwrap();
        assert_eq!(*dst, [255; 4]);
        assert_eq!(&msg[10..12], &[0, 0]);
    }
    let l = dhcp::lease().unwrap();
    CLOCK.store(l.expires_at, Ordering::Relaxed);
    assert!(dhcp::lease().is_none());
    PEER.lock().unwrap().silent = true;
    dhcp::maintain(MAC, true);
    assert!(dhcp::lease().is_none());
    PEER.lock().unwrap().silent = false;
    let _ = dhcp::acquire(MAC).unwrap();
    dhcp::maintain(MAC, false);
    assert_eq!(dhcp::lease().unwrap().ip_addr, ADDRESS);
}
#[test]
fn unexpired_link_bounce_keeps_address_and_requests_without_discovery_or_probe() {
    let _guard = setup();
    let original = dhcp::acquire(MAC).unwrap();
    PEER.lock().unwrap().sent.clear();
    dhcp::maintain(MAC, false);
    assert_eq!(dhcp::lease().unwrap().ip_addr, original.ip_addr);
    let start = CLOCK.load(Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert_eq!(dhcp::lease().unwrap().ip_addr, original.ip_addr);
    let peer = PEER.lock().unwrap();
    assert_eq!(peer.sent.len(), 1);
    let (dst, msg) = &peer.sent[0];
    assert_eq!(*peer.sources.last().unwrap(), [0; 4]);
    assert_eq!(*dst, [255; 4]);
    assert_eq!(msg[242], 3);
    assert_eq!(&msg[12..16], &[0; 4]);
    assert_eq!(&msg[243..249], &[50, 4, 192, 0, 2, 10]);
    assert_eq!(msg[249], 55, "INIT-REBOOT omits server identifier");
    assert!(
        CLOCK.load(Ordering::Relaxed) - start < 1_000_000_000,
        "an existing lease does not run a fresh address probe"
    );
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
    assert!(text.contains("ntpserver 192.0.2.123\nntpserver 192.0.2.124\n"));
    assert_eq!(dhcp::lease().unwrap().ntp_servers, [Some([192, 0, 2, 123]), Some([192, 0, 2, 124]), None]);
    assert_eq!(proc_net::list_entries().unwrap(), ["pnp", "tcp"]);
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
        let packet = ipv4::Ipv4Packet::try_build(src, dst, 6, ipv4::DEFAULT_TTL, &syn).unwrap();
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
    let packet = ipv4::Ipv4Packet::try_build(
        [127, 0, 0, 1],
        [127, 0, 0, 2],
        6,
        ipv4::DEFAULT_TTL,
        &[0; 20],
    )
    .unwrap();
    let ip = ipv4::Ipv4Packet::parse(&packet).unwrap();
    ipv4::handle_loopback_ipv4(&ip);
    assert_eq!(
        TCP_DELIVERIES.load(Ordering::Relaxed),
        1,
        "trusted loopback remains deliverable"
    );
}

#[test]
fn delayed_ack_does_not_add_time_to_initial_or_renewed_lease() {
    let _guard = setup();
    PEER.lock().unwrap().lease_seconds = Some(60);
    ACK_DELAY.store(1_500_000_000, Ordering::Relaxed);
    let lease = dhcp::acquire(MAC).unwrap();
    assert_eq!(
        lease.expires_at,
        REQUEST_TIME.load(Ordering::Relaxed) + 60_000_000_000
    );
    CLOCK.store(lease.renew_at, Ordering::Relaxed);
    ACK_DELAY.store(200_000_000, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    let renewed = dhcp::lease().unwrap();
    assert_eq!(
        renewed.expires_at,
        REQUEST_TIME.load(Ordering::Relaxed) + 60_000_000_000
    );
}

#[test]
fn a_bring_up_discover_is_answered_only_by_its_own_offer() {
    let mac = [0x02, 0, 0, 0, 0, 0x5a];
    let xid = 0x1234_5678;
    let discover = dhcp::discover_message(xid, mac);
    assert_eq!(discover[0], 1, "BOOTREQUEST");
    assert_eq!(&discover[4..8], &xid.to_be_bytes());
    assert_eq!(&discover[28..34], &mac);
    assert_eq!(
        &discover[236..243],
        &[99, 130, 83, 99, 53, 1, 1],
        "cookie, then DHCPDISCOVER"
    );
    assert!(
        !dhcp::is_offer(&discover, xid, mac),
        "a request is not a reply"
    );

    let mut offer = discover;
    offer[0] = 2;
    offer[242] = 2;
    assert!(dhcp::is_offer(&offer, xid, mac));
    assert!(!dhcp::is_offer(&offer, xid ^ 1, mac), "another transaction");
    assert!(
        !dhcp::is_offer(&offer, xid, [0x02, 0, 0, 0, 0, 0x5b]),
        "another client"
    );
    let mut ack = offer;
    ack[242] = 5;
    assert!(!dhcp::is_offer(&ack, xid, mac), "an ACK is not an offer");
}

#[test]
fn lost_first_discover_retransmits_after_four_seconds_and_acquires_lease() {
    let _guard = setup();
    PEER.lock().unwrap().drop_first_discover = true;
    let start = CLOCK.load(Ordering::Relaxed);
    let lease = dhcp::acquire(MAC).expect("the second DISCOVER must complete DORA");
    let peer = PEER.lock().unwrap();
    let discovers: Vec<_> = peer
        .sent
        .iter()
        .filter(|(_, message)| message[242] == 1)
        .collect();
    assert_eq!(discovers.len(), 2);
    assert_eq!(&discovers[0].1[4..8], &discovers[1].1[4..8]);
    assert!(REQUEST_TIME.load(Ordering::Relaxed) - start >= 4_000_000_000);
    assert_eq!(lease.ip_addr, ADDRESS);
    assert_eq!(lease.nameservers[0], Some([192, 0, 2, 53]));
}

#[test]
fn procfs_marks_dhcp_policy_while_no_lease_is_usable() {
    let _guard = setup();
    dhcp::require_configuration();
    CLOCK.store(u64::MAX, Ordering::Relaxed);
    dhcp::maintain(MAC, false);
    assert_eq!(proc_net::generate_pnp().unwrap(), "#PROTO: DHCP\n");
}

#[test]
fn unanswered_renewal_reaches_t2_and_expiry_without_losing_address_early() {
    let _guard = setup();
    let lease = dhcp::acquire(MAC).unwrap();
    PEER.lock().unwrap().silent = true;
    CLOCK.store(lease.renew_at, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert_eq!(dhcp::lease().unwrap().expires_at, lease.expires_at);
    assert_eq!(PEER.lock().unwrap().sent.last().unwrap().0, SERVER);
    CLOCK.store(lease.rebind_at, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert_eq!(PEER.lock().unwrap().sent.last().unwrap().0, [255; 4]);
    assert_eq!(dhcp::lease().unwrap().ip_addr, ADDRESS);
    CLOCK.store(lease.expires_at - 1, Ordering::Relaxed);
    assert_eq!(dhcp::lease().unwrap().ip_addr, ADDRESS);
    CLOCK.store(lease.expires_at, Ordering::Relaxed);
    assert!(dhcp::lease().is_none());
    dhcp::maintain(MAC, false);
    assert!(dhcp::lease().is_none());
}

#[test]
fn unanswered_reboot_retains_original_expiry_and_nak_invalidates_it() {
    let _guard = setup();
    let original = dhcp::acquire(MAC).unwrap();
    dhcp::maintain(MAC, false);
    PEER.lock().unwrap().silent = true;
    dhcp::maintain(MAC, true);
    assert_eq!(dhcp::lease().unwrap().expires_at, original.expires_at);
    {
        let mut peer = PEER.lock().unwrap();
        peer.silent = false;
        peer.nak = true;
    }
    CLOCK.fetch_add(5_000_000_000, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert!(dhcp::lease().is_none());
}

#[test]
fn request_wait_crossing_t2_does_not_postpone_broadcast_rebind() {
    let _guard = setup();
    let lease = dhcp::acquire(MAC).unwrap();
    PEER.lock().unwrap().silent = true;
    CLOCK.store(lease.rebind_at - 500_000_000, Ordering::Relaxed);
    CLOCK_STEP.store(2_000_000_000, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert_eq!(PEER.lock().unwrap().sent.last().unwrap().0, SERVER);
    assert_eq!(dhcp::lease().unwrap().ip_addr, ADDRESS);
    dhcp::maintain(MAC, true);
    assert_eq!(PEER.lock().unwrap().sent.last().unwrap().0, [255; 4]);
    assert_eq!(dhcp::lease().unwrap().ip_addr, ADDRESS);
}

#[test]
fn lease_expired_while_down_discovers_instead_of_rebooting_the_old_address() {
    let _guard = setup();
    let lease = dhcp::acquire(MAC).unwrap();
    dhcp::maintain(MAC, false);
    CLOCK.store(lease.expires_at, Ordering::Relaxed);
    dhcp::maintain(MAC, false);
    assert!(dhcp::lease().is_none());
    PEER.lock().unwrap().sent.clear();
    dhcp::maintain(MAC, true);
    assert_eq!(dhcp::lease().unwrap().ip_addr, ADDRESS);
    let peer = PEER.lock().unwrap();
    assert_eq!(peer.sent.len(), 2);
    assert_eq!(peer.sent[0].1[242], 1);
    assert_eq!(peer.sent[1].1[242], 3);
}

#[test]
fn reboot_retries_back_off_to_cap_then_wait_for_normal_renewal() {
    let _guard = setup();
    PEER.lock().unwrap().lease_seconds = Some(86_400);
    let lease = dhcp::acquire(MAC).unwrap();
    dhcp::maintain(MAC, false);
    PEER.lock().unwrap().silent = true;
    let first = PEER.lock().unwrap().sent.len();
    dhcp::maintain(MAC, true);
    let mut jitters = Vec::new();
    for base_seconds in [4, 8, 16, 32, 64, 64] {
        let count = PEER.lock().unwrap().sent.len();
        let previous = *PEER.lock().unwrap().sent_at.last().unwrap();
        let earliest = previous + (base_seconds - 1) * 1_000_000_000;
        CLOCK.store(earliest - 1, Ordering::Relaxed);
        dhcp::maintain(MAC, true);
        assert_eq!(PEER.lock().unwrap().sent.len(), count);
        // Poll a deterministic clock through the allowed jitter window.
        for _ in 0..=21 {
            let latest = previous + (base_seconds + 1).min(64) * 1_000_000_000;
            CLOCK.store(
                (CLOCK.load(Ordering::Relaxed) + 100_000_000).min(latest),
                Ordering::Relaxed,
            );
            dhcp::maintain(MAC, true);
            if PEER.lock().unwrap().sent.len() != count {
                break;
            }
        }
        let peer = PEER.lock().unwrap();
        assert_eq!(peer.sent.len(), count + 1);
        let spacing = peer.sent_at.last().unwrap() - previous;
        assert!(spacing >= (base_seconds - 1) * 1_000_000_000);
        assert!(spacing <= (base_seconds + 1).min(64) * 1_000_000_000);
        if base_seconds < 64 {
            jitters.push(spacing as i64 - base_seconds as i64 * 1_000_000_000);
        }
        assert_eq!(*peer.sources.last().unwrap(), [0; 4]);
    }
    assert!(
        jitters.windows(2).any(|pair| pair[0] != pair[1]),
        "retry jitter varies"
    );
    assert_eq!(PEER.lock().unwrap().sent.len(), first + 7);
    let retained = dhcp::lease().unwrap();
    assert_eq!(retained.expires_at, lease.expires_at);
    CLOCK.store(lease.renew_at - 1, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    assert_eq!(PEER.lock().unwrap().sent.len(), first + 7);
    CLOCK.store(lease.renew_at, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    let peer = PEER.lock().unwrap();
    assert_eq!(peer.sent.last().unwrap().0, SERVER);
    assert_eq!(*peer.sources.last().unwrap(), ADDRESS);
}

#[test]
fn renew_and_rebind_retry_wait_at_least_sixty_seconds() {
    let _guard = setup();
    // With less than 120 seconds to T2/expiry, half the remaining
    // time must be rounded up to the 60-second retry minimum.
    PEER.lock().unwrap().lease_seconds = Some(800);
    let lease = dhcp::acquire(MAC).unwrap();
    PEER.lock().unwrap().silent = true;
    CLOCK.store(lease.rebind_at - 100_000_000_000, Ordering::Relaxed);
    dhcp::maintain(MAC, true);
    for state_start in [
        CLOCK.load(Ordering::Relaxed),
        lease.rebind_at + 1_000_000_000,
    ] {
        CLOCK.store(state_start + 59_000_000_000, Ordering::Relaxed);
        let count = PEER.lock().unwrap().sent.len();
        dhcp::maintain(MAC, true);
        assert_eq!(PEER.lock().unwrap().sent.len(), count);
        CLOCK.store(state_start + 60_000_000_000, Ordering::Relaxed);
        dhcp::maintain(MAC, true);
        assert_eq!(PEER.lock().unwrap().sent.len(), count + 1);
        if state_start < lease.rebind_at {
            CLOCK.store(lease.rebind_at, Ordering::Relaxed);
            dhcp::maintain(MAC, true);
        }
    }
}
