//! DHCP configuration and lease maintenance (RFC 2131, RFC 2132).
//! The e1000 receive thread maintains leases; readers independently enforce
//! expiry so delayed maintenance can never keep an expired address usable.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use super::{ethernet, ipv4, udp};

pub(crate) const CLIENT_PORT: u16 = 68;
pub(crate) const SERVER_PORT: u16 = 67;

/// RFC 2131 section 3: the options field starts with this cookie.
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
/// op through file: the fixed BOOTP header in front of the cookie.
const FIXED_LEN: usize = 236;
/// RFC 1542 section 2.1: relay agents may drop BOOTP messages shorter than this.
const MIN_MESSAGE_LEN: usize = 300;
/// RFC 2131 section 2: ask servers and relays to broadcast their replies.
const FLAG_BROADCAST: u16 = 0x8000;

const OPT_PAD: u8 = 0;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_OVERLOAD: u8 = 52;
const OPT_RENEWAL: u8 = 58;
const OPT_REBINDING: u8 = 59;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_PARAMETER_LIST: u8 = 55;
const OPT_END: u8 = 255;

const DHCPDISCOVER: u8 = 1;
const DHCPOFFER: u8 = 2;
const DHCPREQUEST: u8 = 3;
const DHCPDECLINE: u8 = 4;
const DHCPACK: u8 = 5;
const DHCPNAK: u8 = 6;

/// How long each attempt waits for the OFFER, and then for the ACK. Three
/// attempts bound a network with no server to 10 s of boot time. Attempts
/// retransmit the DISCOVER in one transaction, so an OFFER that answers an
/// earlier copy after that copy's wait still counts.
const ATTEMPT_TIMEOUTS_MS: [u64; 3] = [2_000, 4_000, 4_000];

/// The reply the running exchange waits for; zero while none runs, so the
/// receive hook costs one atomic load on every other boot.
static AWAITED: AtomicU8 = AtomicU8::new(0);
static XID: AtomicU32 = AtomicU32::new(0);
static REQUEST_SENT: AtomicU64 = AtomicU64::new(0);
/// The interface's hardware address, in the low 48 bits.
static CLIENT_MAC: AtomicU64 = AtomicU64::new(0);
static REPLY: Mutex<Option<Reply>> = Mutex::new(None);
static LEASE: Mutex<Option<Lease>> = Mutex::new(None);
static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(0);
static CARRIER: AtomicBool = AtomicBool::new(true);
static PROBING: AtomicBool = AtomicBool::new(false);
static PROBE_IP: AtomicU32 = AtomicU32::new(0);
static CONFLICT: AtomicBool = AtomicBool::new(false);

fn now() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// The configuration a server's ACK assigned.
#[derive(Clone, Copy, Debug)]
pub struct Lease {
    pub ip_addr: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway: [u8; 4],
    pub nameservers: [Option<[u8; 4]>; 3],
    pub server: [u8; 4],
    pub expires_at: u64,
    pub renew_at: u64,
    pub rebind_at: u64,
}

#[derive(Clone, Copy)]
struct Reply {
    kind: u8,
    yiaddr: [u8; 4],
    subnet_mask: Option<[u8; 4]>,
    router: Option<[u8; 4]>,
    nameservers: [Option<[u8; 4]>; 3],
    server: Option<[u8; 4]>,
    lease_secs: Option<u32>,
    renewal: Option<u32>,
    rebinding: Option<u32>,
}

/// Obtain a lease for the interface whose hardware address is `mac`.
/// Returns `None` when no server completed the exchange.
pub fn acquire(mac: [u8; 6]) -> Option<Lease> {
    CLIENT_MAC.store(mac_to_u64(mac), Ordering::Relaxed);
    let seed = crate::time::tsc::nanoseconds_since_base().unwrap_or(0) as u32;
    XID.store(
        u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]) ^ seed,
        Ordering::Relaxed,
    );

    let lease = ATTEMPT_TIMEOUTS_MS
        .iter()
        .find_map(|&timeout_ms| attempt(mac, timeout_ms));
    AWAITED.store(0, Ordering::Release);
    let _guard = super::net_lock_guard();
    *LEASE.lock() = lease;
    lease
}

/// The lease the boot-time exchange obtained, if any.
pub fn lease() -> Option<Lease> {
    let _guard = super::net_lock_guard();
    let lease = *LEASE.lock();
    lease.filter(|l| now() < l.expires_at && CARRIER.load(Ordering::Acquire))
}

fn mac_to_u64(mac: [u8; 6]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes[2..].copy_from_slice(&mac);
    u64::from_be_bytes(bytes)
}

fn attempt(mac: [u8; 6], timeout_ms: u64) -> Option<Lease> {
    let offer = exchange(mac, DHCPDISCOVER, None, DHCPOFFER, timeout_ms)?;
    let server = offer.server?;
    if !valid_address(server) || !valid_address(offer.yiaddr) {
        return None;
    }
    let ack = exchange(
        mac,
        DHCPREQUEST,
        Some((offer.yiaddr, server)),
        DHCPACK,
        timeout_ms,
    );
    let Some(lease) = ack.and_then(|ack| lease_from_ack(ack, offer.yiaddr, Some(server))) else {
        // A new transaction cannot accept a delayed ACK from this one.
        XID.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    if !probe(mac, lease.ip_addr) {
        let message = build_message(
            DHCPDECLINE,
            XID.load(Ordering::Relaxed),
            mac,
            Some((lease.ip_addr, server)),
        );
        let _ = send_message(&message, [0; 4], [255; 4]);
        XID.fetch_add(1, Ordering::Relaxed);
        // RFC 2131: wait at least ten seconds after DECLINE before restarting.
        wait_until(now().saturating_add(10_000_000_000));
        return None;
    }
    (now() < lease.expires_at).then_some(lease)
}

fn valid_address(ip: [u8; 4]) -> bool {
    ip[0] != 0 && ip[0] != 127 && ip[0] < 224 && ip != [255; 4]
}

fn lease_from_ack(ack: Reply, requested: [u8; 4], server: Option<[u8; 4]>) -> Option<Lease> {
    let selected = ack.server?;
    if ack.kind != DHCPACK
        || !valid_address(ack.yiaddr)
        || ack.yiaddr != requested
        || !valid_address(selected)
        || server.is_some_and(|s| s != selected)
    {
        return None;
    }
    let seconds = ack.lease_secs?;
    if seconds == 0 {
        return None;
    }
    // RFC 2131: ACK transit time must not extend the server's lease.
    let start = REQUEST_SENT.load(Ordering::Relaxed);
    let infinite = seconds == u32::MAX;
    let duration = u64::from(seconds) * 1_000_000_000;
    let t1 = ack
        .renewal
        .map(|s| u64::from(s) * 1_000_000_000)
        .unwrap_or(duration / 2);
    let t2 = ack
        .rebinding
        .map(|s| u64::from(s) * 1_000_000_000)
        .unwrap_or(duration - duration / 8);
    if !infinite && (t1 == 0 || t1 >= t2 || t2 >= duration) {
        return None;
    }
    Some(Lease {
        ip_addr: ack.yiaddr,
        subnet_mask: ack.subnet_mask?,
        gateway: ack.router.unwrap_or([0; 4]),
        nameservers: ack.nameservers,
        server: selected,
        expires_at: if infinite {
            u64::MAX
        } else {
            start.saturating_add(duration)
        },
        renew_at: if infinite {
            u64::MAX
        } else {
            start.saturating_add(t1)
        },
        rebind_at: if infinite {
            u64::MAX
        } else {
            start.saturating_add(t2)
        },
    })
}

fn send_message(message: &[u8], src: [u8; 4], dst: [u8; 4]) -> Option<()> {
    let datagram = udp::try_build_udp_packet(src, dst, CLIENT_PORT, SERVER_PORT, message).ok()?;
    if dst == [255; 4] {
        let packet =
            ipv4::Ipv4Packet::try_build(src, dst, ipv4::PROTOCOL_UDP, ipv4::DEFAULT_TTL, &datagram)
                .ok()?;
        super::send_ethernet(&ethernet::BROADCAST_MAC, ethernet::ETHERTYPE_IPV4, &packet).ok()?;
    } else {
        super::ip_output(src, dst, ipv4::PROTOCOL_UDP, ipv4::DEFAULT_TTL, &datagram).ok()?;
    }
    Some(())
}

/// Called in thread context, never in the interrupt or syscall entry paths.
/// Renew unicast at T1, rebind broadcast at T2, and discard on carrier loss,
/// NAK or expiry. A failed exchange waits before retrying rather than spinning.
pub fn maintain(mac: [u8; 6], carrier: bool) {
    let changed = CARRIER.swap(carrier, Ordering::AcqRel) != carrier;
    if changed || !carrier {
        let _guard = super::net_lock_guard();
        *LEASE.lock() = None;
        super::arp::clear();
        NEXT_ATTEMPT.store(0, Ordering::Release);
    }
    if !carrier || now() < NEXT_ATTEMPT.load(Ordering::Acquire) {
        return;
    }
    let current = lease();
    if let Some(current) = current {
        if now() < current.renew_at {
            return;
        }
        let rebind = now() >= current.rebind_at;
        // A new transaction cannot accept a delayed ACK from an earlier one.
        XID.fetch_add(1, Ordering::Relaxed);
        let _ = take_reply();
        REQUEST_SENT.store(now(), Ordering::Relaxed);
        AWAITED.store(DHCPACK, Ordering::Release);
        let mut message = build_message(DHCPREQUEST, XID.load(Ordering::Relaxed), mac, None);
        message[12..16].copy_from_slice(&current.ip_addr);
        if !rebind {
            message[10..12].fill(0);
        }
        let dst = if rebind { [255; 4] } else { current.server };
        let reply = send_message(&message, current.ip_addr, dst).and_then(|()| wait_reply(1_000));
        AWAITED.store(0, Ordering::Release);
        if let Some(reply) = reply.filter(|_| now() < current.expires_at) {
            let server = if rebind { None } else { Some(current.server) };
            if reply.kind == DHCPNAK && reply.server.is_some_and(|s| rebind || s == current.server)
            {
                let _guard = super::net_lock_guard();
                *LEASE.lock() = None;
                super::arp::clear();
            } else if let Some(renewed) = lease_from_ack(reply, current.ip_addr, server) {
                let _guard = super::net_lock_guard();
                *LEASE.lock() = Some(renewed);
            }
        }
        let remaining = current.expires_at.saturating_sub(now());
        NEXT_ATTEMPT.store(
            now().saturating_add((remaining / 2).clamp(100_000_000, 60_000_000_000)),
            Ordering::Release,
        );
    } else {
        // Readers already see no address even while discovery is running.
        {
            let _guard = super::net_lock_guard();
            *LEASE.lock() = None;
        }
        super::arp::clear();
        let _ = acquire(mac);
        NEXT_ATTEMPT.store(now().saturating_add(10_000_000_000), Ordering::Release);
    }
}

/// Observe ARP conflicts, including another host's simultaneous probe.
pub fn observe_arp(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) {
    if !PROBING.load(Ordering::Acquire)
        || mac_to_u64(sender_mac) == CLIENT_MAC.load(Ordering::Relaxed)
    {
        return;
    }
    let ip = PROBE_IP.load(Ordering::Relaxed).to_be_bytes();
    if sender_ip == ip || (sender_ip == [0; 4] && target_ip == ip) {
        CONFLICT.store(true, Ordering::Release);
    }
}

fn wait_until(deadline: u64) {
    while now() < deadline {
        let _ = super::process_rx_budgeted(64);
        core::hint::spin_loop();
    }
}

fn probe(mac: [u8; 6], ip: [u8; 4]) -> bool {
    PROBE_IP.store(u32::from_be_bytes(ip), Ordering::Relaxed);
    CONFLICT.store(false, Ordering::Relaxed);
    PROBING.store(true, Ordering::Release);
    // RFC 5227: random initial delay, three probes 1-2 seconds apart, then
    // two seconds before using the address. No source address is claimed yet.
    let mut random = XID.load(Ordering::Relaxed);
    wait_until(now().saturating_add(u64::from(random % 1_000_000_000)));
    let mut sent = true;
    for _ in 0..3 {
        if CONFLICT.load(Ordering::Acquire) {
            break;
        }
        let packet =
            super::arp::ArpPacket::build(super::arp::ARP_OP_REQUEST, &mac, &[0; 4], &[0; 6], &ip);
        if super::send_ethernet(&ethernet::BROADCAST_MAC, ethernet::ETHERTYPE_ARP, &packet).is_err()
        {
            sent = false;
            break;
        }
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        wait_until(now().saturating_add(1_000_000_000 + u64::from(random % 1_000_000_000)));
    }
    wait_until(now().saturating_add(2_000_000_000));
    PROBING.store(false, Ordering::Release);
    sent && !CONFLICT.load(Ordering::Acquire)
}

/// Broadcast one message and poll the receive ring until the awaited reply
/// (or, for a REQUEST, a NAK) arrives or `timeout_ms` passes. A retransmission
/// keeps a reply that arrived for an earlier copy after that copy's wait.
fn exchange(
    mac: [u8; 6],
    kind: u8,
    request: Option<([u8; 4], [u8; 4])>,
    awaited: u8,
    timeout_ms: u64,
) -> Option<Reply> {
    if AWAITED.swap(awaited, Ordering::AcqRel) != awaited {
        let _ = take_reply();
    }
    REQUEST_SENT.store(now(), Ordering::Relaxed);
    let message = build_message(kind, XID.load(Ordering::Relaxed), mac, request);
    send_message(&message, [0; 4], [255; 4])?;
    wait_reply(timeout_ms)
}

fn wait_reply(timeout_ms: u64) -> Option<Reply> {
    let deadline = now().saturating_add(timeout_ms * 1_000_000);
    loop {
        let _ = super::process_rx_budgeted(64);
        if now() >= deadline {
            return None;
        }
        if let Some(reply) = take_reply() {
            return Some(reply);
        }
        core::hint::spin_loop();
    }
}

fn take_reply() -> Option<Reply> {
    let _guard = super::net_lock_guard();
    let reply = REPLY.lock().take();
    reply
}

/// A DHCPDISCOVER from `mac` in transaction `xid`, for a driver that checks
/// its first frame reaches the network and is answered. It is the client's
/// own message; the exchange that would take a lease is not run.
#[cfg(target_arch = "x86_64")]
pub(crate) fn discover_message(xid: u32, mac: [u8; 6]) -> [u8; MIN_MESSAGE_LEN] {
    build_message(DHCPDISCOVER, xid, mac, None)
}

/// Whether `message`, a UDP payload from the server port to the client port,
/// is an OFFER answering transaction `xid` for `mac`.
#[cfg(target_arch = "x86_64")]
pub(crate) fn is_offer(message: &[u8], xid: u32, mac: [u8; 6]) -> bool {
    parse_reply(message, xid, mac).is_some_and(|reply| reply.kind == DHCPOFFER)
}

fn build_message(
    kind: u8,
    xid: u32,
    mac: [u8; 6],
    request: Option<([u8; 4], [u8; 4])>,
) -> [u8; MIN_MESSAGE_LEN] {
    let mut message = [0; MIN_MESSAGE_LEN];
    message[0] = 1;
    message[1] = 1;
    message[2] = 6;
    message[4..8].copy_from_slice(&xid.to_be_bytes());
    message[10..12].copy_from_slice(&FLAG_BROADCAST.to_be_bytes());
    message[28..34].copy_from_slice(&mac);
    message[236..240].copy_from_slice(&MAGIC_COOKIE);
    message[240..243].copy_from_slice(&[OPT_MESSAGE_TYPE, 1, kind]);
    let mut offset = 243;
    if let Some((requested_ip, server)) = request {
        message[offset..offset + 2].copy_from_slice(&[OPT_REQUESTED_IP, 4]);
        message[offset + 2..offset + 6].copy_from_slice(&requested_ip);
        message[offset + 6..offset + 8].copy_from_slice(&[OPT_SERVER_ID, 4]);
        message[offset + 8..offset + 12].copy_from_slice(&server);
        offset += 12;
    }
    message[offset..offset + 8].copy_from_slice(&[
        OPT_PARAMETER_LIST,
        6,
        OPT_SUBNET_MASK,
        OPT_ROUTER,
        OPT_DNS,
        OPT_LEASE_TIME,
        OPT_RENEWAL,
        OPT_REBINDING,
    ]);
    message[offset + 8] = OPT_END;
    message
}

/// Receive hook for IPv4 UDP payloads, ahead of the destination-address check
/// (the reply is addressed to the broadcast address or to the offered address,
/// neither of which is configured yet). Returns true when it consumed a reply
/// to the running exchange.
pub fn receive(ip: &ipv4::Ipv4Packet) -> bool {
    let awaited = AWAITED.load(Ordering::Acquire);
    if awaited == 0 {
        return false;
    }
    let Some((header, message)) = udp::UdpHeader::parse(ip.payload) else {
        return false;
    };
    if header.dst_port != CLIENT_PORT || header.src_port != SERVER_PORT {
        return false;
    }
    if header.checksum != 0
        && udp::udp_checksum(ip.src_ip, ip.dst_ip, &ip.payload[..header.length as usize]) != 0
    {
        return true;
    }
    let mac = CLIENT_MAC.load(Ordering::Relaxed).to_be_bytes();
    let mac = [mac[2], mac[3], mac[4], mac[5], mac[6], mac[7]];
    let Some(reply) = parse_reply(message, XID.load(Ordering::Relaxed), mac) else {
        return true;
    };
    if reply.kind == awaited || (awaited == DHCPACK && reply.kind == DHCPNAK) {
        let _guard = super::net_lock_guard();
        let mut slot = REPLY.lock();
        if slot.is_none() {
            *slot = Some(reply);
        }
    }
    true
}

fn parse_reply(message: &[u8], xid: u32, mac: [u8; 6]) -> Option<Reply> {
    if message.len() < FIXED_LEN + MAGIC_COOKIE.len()
        || message[0] != 2
        || message[1] != 1
        || message[2] != 6
        || message[4..8] != xid.to_be_bytes()
        || message[28..34] != mac
        || message[FIXED_LEN..FIXED_LEN + 4] != MAGIC_COOKIE
    {
        return None;
    }
    let mut reply = Reply {
        kind: 0,
        yiaddr: [message[16], message[17], message[18], message[19]],
        subnet_mask: None,
        router: None,
        nameservers: [None; 3],
        server: None,
        lease_secs: None,
        renewal: None,
        rebinding: None,
    };
    let mut overload = 0;
    parse_options(&message[FIXED_LEN + 4..], &mut reply, Some(&mut overload))?;
    if overload & 1 != 0 {
        parse_options(&message[108..236], &mut reply, None)?;
    }
    if overload & 2 != 0 {
        parse_options(&message[44..108], &mut reply, None)?;
    }
    (reply.kind != 0).then_some(reply)
}

fn parse_options(
    mut options: &[u8],
    reply: &mut Reply,
    mut overload: Option<&mut u8>,
) -> Option<()> {
    while let Some((&code, rest)) = options.split_first() {
        match code {
            OPT_PAD => {
                options = rest;
                continue;
            }
            OPT_END => return Some(()),
            _ => {}
        }
        let (&len, rest) = rest.split_first()?;
        let value = rest.get(..len as usize)?;
        options = &rest[len as usize..];
        let address = || (value.len() == 4).then(|| [value[0], value[1], value[2], value[3]]);
        match code {
            OPT_MESSAGE_TYPE => {
                if value.len() != 1 || reply.kind != 0 {
                    return None;
                }
                reply.kind = value[0];
            }
            OPT_OVERLOAD => {
                if value.len() != 1 || !(1..=3).contains(&value[0]) {
                    return None;
                }
                if let Some(field) = overload.as_deref_mut() {
                    *field = value[0];
                } else {
                    return None;
                }
            }
            OPT_SUBNET_MASK => reply.subnet_mask = Some(address()?),
            OPT_ROUTER => {
                if value.is_empty() || value.len() % 4 != 0 {
                    return None;
                }
                if reply.router.is_none() {
                    reply.router = Some([value[0], value[1], value[2], value[3]]);
                }
            }
            OPT_DNS => {
                if value.is_empty() || value.len() % 4 != 0 {
                    return None;
                }
                for chunk in value.chunks_exact(4) {
                    if let Some(slot) = reply.nameservers.iter_mut().find(|s| s.is_none()) {
                        *slot = Some([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    }
                }
            }
            OPT_SERVER_ID => reply.server = Some(address()?),
            OPT_LEASE_TIME => reply.lease_secs = Some(u32::from_be_bytes(address()?)),
            OPT_RENEWAL => reply.renewal = Some(u32::from_be_bytes(address()?)),
            OPT_REBINDING => reply.rebinding = Some(u32::from_be_bytes(address()?)),
            _ => {}
        }
    }
    Some(())
}
