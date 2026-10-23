//! DHCP configuration and lease maintenance (RFC 2131, RFC 2132).
//! Network maintenance workers maintain leases; readers independently enforce
//! expiry even when maintenance is delayed.

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
/// attempts bound a network with no server to 10 s waiting for offers. Attempts
/// retransmit the DISCOVER in one transaction, so an OFFER that answers an
/// earlier copy after that copy's wait still counts.
const ATTEMPT_TIMEOUTS_MS: [u64; 3] = [4_000, 4_000, 2_000];

/// DHCP policy remains in force before acquisition and after lease expiry.
static REQUIRED: AtomicBool = AtomicBool::new(false);
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
// Seven requests allow the 4, 8, 16, 32, 64, 64 second backoff to reach its cap.
const REBOOT_REQUEST_LIMIT: u8 = 7;
static REBOOT_REQUESTS: AtomicU8 = AtomicU8::new(0);
static RETRY_RANDOM: AtomicU32 = AtomicU32::new(0);
// 0 = bound, 1 = INIT-REBOOT, 2 = renewing, 3 = rebinding.
static MAINTENANCE_STATE: AtomicU8 = AtomicU8::new(0);
static REVALIDATE: AtomicBool = AtomicBool::new(false);
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

/// Mark an interface as DHCP-managed before carrier and discovery finish.
pub fn require_configuration() {
    REQUIRED.store(true, Ordering::Release);
}

pub fn configuration_required() -> bool {
    REQUIRED.load(Ordering::Acquire)
}

/// Obtain a lease for the interface whose hardware address is `mac`.
/// Returns `None` when no server completed the exchange.
pub fn acquire(mac: [u8; 6]) -> Option<Lease> {
    require_configuration();
    CLIENT_MAC.store(mac_to_u64(mac), Ordering::Relaxed);
    let seed = crate::time::tsc::nanoseconds_since_base().unwrap_or(0) as u32;
    XID.store(
        u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]) ^ seed,
        Ordering::Relaxed,
    );

    RETRY_RANDOM.store(seed, Ordering::Relaxed);
    MAINTENANCE_STATE.store(0, Ordering::Release);
    let lease = ATTEMPT_TIMEOUTS_MS
        .iter()
        .find_map(|&timeout_ms| attempt(mac, timeout_ms));
    AWAITED.store(0, Ordering::Release);
    {
        let _guard = super::net_lock_guard();
        *LEASE.lock() = lease;
    }
    if let Some(lease) = lease {
        lease_event("lease acquired", lease);
    } else {
        log::warn!("[dhcp] acquisition unanswered or refused; address=0.0.0.0");
    }
    lease
}

fn lease_event(event: &str, lease: Lease) {
    let [a, b, c, d] = lease.ip_addr;
    log::info!(
        "[dhcp] {event}: address={a}.{b}.{c}.{d} t1={} t2={} expires={} now={} ns",
        lease.renew_at,
        lease.rebind_at,
        lease.expires_at,
        now()
    );
}

/// The current unexpired lease, including renewed or revalidated leases.
pub fn lease() -> Option<Lease> {
    let _guard = super::net_lock_guard();
    let lease = *LEASE.lock();
    lease.filter(|l| now() < l.expires_at)
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
        log::warn!(
            "[dhcp] offer refused: invalid server or address={:?}",
            offer.yiaddr
        );
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
        let [a, b, c, d] = offer.yiaddr;
        log::warn!(
            "[dhcp] acquisition request unanswered or refused: address={a}.{b}.{c}.{d} reply={:?}",
            ack.map(|r| r.kind)
        );
        // A new transaction cannot accept a delayed ACK from this one.
        XID.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    if !probe(mac, lease.ip_addr) {
        lease_event("lease refused: address probe failed; DECLINE", lease);
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
    if now() >= lease.expires_at {
        lease_event("lease dropped: expired during probe", lease);
        None
    } else {
        Some(lease)
    }
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

/// Called by network maintenance workers in thread context.
/// RFC 2131 sections 3.7, 4.4.2 and 4.4.5: carrier loss keeps an unexpired
/// lease; return requests the cached address in INIT-REBOOT. T1 renews unicast,
/// T2 rebinds broadcast. Only NAK or expiry invalidates a held address.
pub fn maintain(mac: [u8; 6], carrier: bool) {
    let changed = CARRIER.swap(carrier, Ordering::AcqRel) != carrier;
    let mut current = {
        let _guard = super::net_lock_guard();
        let saved = *LEASE.lock();
        saved
    };
    if changed {
        if let Some(lease) = current {
            lease_event(
                if carrier {
                    "carrier up; lease retained"
                } else {
                    "carrier down; lease retained"
                },
                lease,
            );
        } else {
            log::info!(
                "[dhcp] carrier {}; address=0.0.0.0",
                if carrier { "up" } else { "down" }
            );
        }
        if carrier {
            REBOOT_REQUESTS.store(0, Ordering::Release);
            REVALIDATE.store(current.is_some(), Ordering::Release);
            NEXT_ATTEMPT.store(0, Ordering::Release);
        }
    }
    // Expiry is checked before carrier and retry deadlines, including while down.
    if let Some(lease) = current.filter(|l| now() >= l.expires_at) {
        drop_lease("lease dropped: expired", lease);
        current = None;
    }
    if !carrier || now() < NEXT_ATTEMPT.load(Ordering::Acquire) {
        return;
    }
    let Some(current) = current else {
        let next = acquire(mac)
            .map(|l| l.renew_at)
            .unwrap_or_else(|| now().saturating_add(10_000_000_000));
        NEXT_ATTEMPT.store(next, Ordering::Release);
        return;
    };
    let reboot = REVALIDATE.load(Ordering::Acquire) && (changed || now() < current.renew_at);
    if !reboot {
        REVALIDATE.store(false, Ordering::Release);
    }
    if !reboot && now() < current.renew_at {
        return;
    }
    let rebind = !reboot && now() >= current.rebind_at;
    let event = if reboot {
        "INIT-REBOOT request"
    } else if rebind {
        "rebind request"
    } else {
        "renew request"
    };
    let state = if reboot {
        1
    } else if rebind {
        3
    } else {
        2
    };
    if MAINTENANCE_STATE.swap(state, Ordering::AcqRel) != state {
        lease_event(event, current);
    }
    XID.fetch_add(1, Ordering::Relaxed);
    let _ = take_reply();
    let request_at = now();
    REQUEST_SENT.store(request_at, Ordering::Relaxed);
    AWAITED.store(DHCPACK, Ordering::Release);
    let mut message = build_message(DHCPREQUEST, XID.load(Ordering::Relaxed), mac, None);
    if reboot {
        // RFC 2131 Table 4: option 50, omit server identifier, ciaddr/source=0.
        message[243..249].copy_from_slice(&[
            OPT_REQUESTED_IP,
            4,
            current.ip_addr[0],
            current.ip_addr[1],
            current.ip_addr[2],
            current.ip_addr[3],
        ]);
        message[249..258].copy_from_slice(&[
            OPT_PARAMETER_LIST,
            6,
            OPT_SUBNET_MASK,
            OPT_ROUTER,
            OPT_DNS,
            OPT_LEASE_TIME,
            OPT_RENEWAL,
            OPT_REBINDING,
            OPT_END,
        ]);
    } else {
        message[12..16].copy_from_slice(&current.ip_addr);
        // RFC 2131 4.1: a configured client can receive unicast replies,
        // including while its request is broadcast in REBINDING.
        message[10..12].fill(0);
    }
    let src = if reboot { [0; 4] } else { current.ip_addr };
    let dst = if reboot || rebind {
        [255; 4]
    } else {
        current.server
    };
    let reply = send_message(&message, src, dst).and_then(|()| wait_reply(1_000));
    AWAITED.store(0, Ordering::Release);
    if now() >= current.expires_at {
        drop_lease("lease dropped: expired during request", current);
        return;
    }
    let server = if reboot || rebind {
        None
    } else {
        Some(current.server)
    };
    if let Some(reply) = reply {
        if reply.kind == DHCPNAK
            && reply
                .server
                .is_some_and(|s| valid_address(s) && server.is_none_or(|wanted| wanted == s))
        {
            drop_lease("lease dropped: NAK", current);
            return;
        }
        if let Some(renewed) =
            lease_from_ack(reply, current.ip_addr, server).filter(|l| now() < l.expires_at)
        {
            {
                let _guard = super::net_lock_guard();
                *LEASE.lock() = Some(renewed);
            }
            MAINTENANCE_STATE.store(0, Ordering::Release);
            REVALIDATE.store(false, Ordering::Release);
            NEXT_ATTEMPT.store(renewed.renew_at, Ordering::Release);
            lease_event(
                if reboot {
                    "lease revalidated"
                } else {
                    "lease renewed"
                },
                renewed,
            );
            return;
        }
    }
    let at = now();
    if reboot {
        let requests = REBOOT_REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
        if requests >= REBOOT_REQUEST_LIMIT {
            REVALIDATE.store(false, Ordering::Release);
            MAINTENANCE_STATE.store(0, Ordering::Release);
            NEXT_ATTEMPT.store(current.renew_at, Ordering::Release);
            lease_event(
                "INIT-REBOOT unanswered; normal renewal scheduled; lease retained",
                current,
            );
        } else {
            // Measure spacing from transmission, including time spent waiting.
            let delay = reboot_retry_delay(requests);
            NEXT_ATTEMPT.store(
                request_at.saturating_add(delay).min(current.renew_at),
                Ordering::Release,
            );
        }
        return;
    }
    // RFC 2131 4.4.5: half the time to T2/expiry, minimum 60 s, but
    // enter the next state as soon as its deadline arrives.
    let boundary = if rebind {
        current.expires_at
    } else {
        current.rebind_at
    };
    let delay = (boundary.saturating_sub(at) / 2).max(60_000_000_000);
    NEXT_ATTEMPT.store(at.saturating_add(delay).min(boundary), Ordering::Release);
}

// RFC 2131 4.1: double from 4 s with +/-1 s jitter, capped at 64 s.
fn reboot_retry_delay(requests: u8) -> u64 {
    let random = RETRY_RANDOM
        .load(Ordering::Relaxed)
        .wrapping_mul(1664525)
        .wrapping_add(1013904223);
    RETRY_RANDOM.store(random, Ordering::Relaxed);
    let base = (4_000_000_000u64 << (requests - 1).min(4)).min(64_000_000_000);
    (base - 1_000_000_000 + u64::from(random) % 2_000_000_001).min(64_000_000_000)
}

fn drop_lease(reason: &str, lease: Lease) {
    {
        let _guard = super::net_lock_guard();
        *LEASE.lock() = None;
    }
    super::arp::clear();
    MAINTENANCE_STATE.store(0, Ordering::Release);
    REVALIDATE.store(false, Ordering::Release);
    NEXT_ATTEMPT.store(0, Ordering::Release);
    lease_event(reason, lease);
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
        super::between_rx_polls();
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
    let message = if kind == DHCPDISCOVER {
        discover_message(XID.load(Ordering::Relaxed), mac)
    } else {
        build_message(kind, XID.load(Ordering::Relaxed), mac, request)
    };
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
        super::between_rx_polls();
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
#[cfg_attr(target_arch = "aarch64", allow(dead_code))] // The BCM5720 driver is x86-only.
pub(crate) fn discover_message(xid: u32, mac: [u8; 6]) -> [u8; MIN_MESSAGE_LEN] {
    build_message(DHCPDISCOVER, xid, mac, None)
}

/// Whether `message`, a UDP payload from the server port to the client port,
/// is an OFFER answering transaction `xid` for `mac`.
#[cfg_attr(target_arch = "aarch64", allow(dead_code))] // The BCM5720 driver is x86-only.
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
    if awaited == DHCPOFFER && !is_offer(message, XID.load(Ordering::Relaxed), mac) {
        return true;
    }
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
