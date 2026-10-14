//! DHCP client for boot-time interface configuration (RFC 2131, RFC 2132).
//!
//! Like Linux's kernel IP autoconfiguration (`net/ipv4/ipconfig.c`), this runs
//! once while the network stack initializes: DISCOVER, OFFER, REQUEST, ACK.
//! Every request is broadcast from 0.0.0.0 with the BROADCAST flag set, so the
//! server's replies reach an interface that has no address yet. The result is
//! the address, netmask, router and nameserver; the lease is not renewed, as
//! ipconfig does not renew it either.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use spin::Mutex;

use super::{ethernet, ipv4, udp};

const CLIENT_PORT: u16 = 68;
const SERVER_PORT: u16 = 67;

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
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_PARAMETER_LIST: u8 = 55;
const OPT_END: u8 = 255;

const DHCPDISCOVER: u8 = 1;
const DHCPOFFER: u8 = 2;
const DHCPREQUEST: u8 = 3;
const DHCPACK: u8 = 5;
const DHCPNAK: u8 = 6;

/// How long each attempt waits for the OFFER, and then for the ACK. Three
/// attempts bound a network with no server to 10 s of boot time.
const ATTEMPT_TIMEOUTS_MS: [u64; 3] = [2_000, 4_000, 4_000];

/// The reply the running exchange waits for; zero while none runs, so the
/// receive hook costs one atomic load on every other boot.
static AWAITED: AtomicU8 = AtomicU8::new(0);
static XID: AtomicU32 = AtomicU32::new(0);
static CLIENT_MAC: Mutex<[u8; 6]> = Mutex::new([0; 6]);
static REPLY: Mutex<Option<Reply>> = Mutex::new(None);
static LEASE: Mutex<Option<Lease>> = Mutex::new(None);

/// The configuration a server's ACK assigned.
#[derive(Clone, Copy, Debug)]
pub struct Lease {
    pub ip_addr: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway: [u8; 4],
    pub nameserver: Option<[u8; 4]>,
    pub server: [u8; 4],
    pub lease_secs: Option<u32>,
}

#[derive(Clone, Copy)]
struct Reply {
    kind: u8,
    yiaddr: [u8; 4],
    subnet_mask: Option<[u8; 4]>,
    router: Option<[u8; 4]>,
    nameserver: Option<[u8; 4]>,
    server: Option<[u8; 4]>,
    lease_secs: Option<u32>,
}

/// Obtain a lease for the interface whose hardware address is `mac`.
/// Returns `None` when no server completed the exchange.
pub fn acquire(mac: [u8; 6]) -> Option<Lease> {
    *CLIENT_MAC.lock() = mac;
    let seed = crate::time::tsc::nanoseconds_since_base().unwrap_or(0) as u32;
    XID.store(
        u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]) ^ seed,
        Ordering::Relaxed,
    );

    let lease = ATTEMPT_TIMEOUTS_MS
        .iter()
        .find_map(|&timeout_ms| attempt(mac, timeout_ms));
    AWAITED.store(0, Ordering::Release);
    *LEASE.lock() = lease;
    lease
}

/// The lease the boot-time exchange obtained, if any.
pub fn lease() -> Option<Lease> {
    *LEASE.lock()
}

fn attempt(mac: [u8; 6], timeout_ms: u64) -> Option<Lease> {
    let offer = exchange(mac, DHCPDISCOVER, None, DHCPOFFER, timeout_ms)?;
    let server = offer.server?;
    let ack = exchange(
        mac,
        DHCPREQUEST,
        Some((offer.yiaddr, server)),
        DHCPACK,
        timeout_ms,
    )?;
    if ack.kind != DHCPACK {
        return None;
    }
    Some(Lease {
        ip_addr: ack.yiaddr,
        subnet_mask: ack.subnet_mask.unwrap_or([255, 255, 255, 0]),
        gateway: ack.router?,
        nameserver: ack.nameserver,
        server,
        lease_secs: ack.lease_secs,
    })
}

/// Broadcast one message and poll the receive ring until the awaited reply
/// (or, for a REQUEST, a NAK) arrives or `timeout_ms` passes.
fn exchange(
    mac: [u8; 6],
    kind: u8,
    request: Option<([u8; 4], [u8; 4])>,
    awaited: u8,
    timeout_ms: u64,
) -> Option<Reply> {
    let _ = take_reply();
    AWAITED.store(awaited, Ordering::Release);
    let message = build_message(kind, XID.load(Ordering::Relaxed), mac, request);
    let datagram = udp::build_udp_packet(CLIENT_PORT, SERVER_PORT, &message);
    let packet = ipv4::Ipv4Packet::build([0; 4], [255; 4], ipv4::PROTOCOL_UDP, &datagram);
    if super::send_ethernet(&ethernet::BROADCAST_MAC, ethernet::ETHERTYPE_IPV4, &packet).is_err() {
        return None;
    }

    let start = crate::time::tsc::nanoseconds_since_base()?;
    let deadline = start.saturating_add(timeout_ms * 1_000_000);
    loop {
        let _ = super::process_rx_budgeted(64);
        if let Some(reply) = take_reply() {
            return Some(reply);
        }
        if crate::time::tsc::nanoseconds_since_base()? >= deadline {
            return None;
        }
        core::hint::spin_loop();
    }
}

fn take_reply() -> Option<Reply> {
    let _guard = super::net_lock_guard();
    let reply = REPLY.lock().take();
    reply
}

fn build_message(
    kind: u8,
    xid: u32,
    mac: [u8; 6],
    request: Option<([u8; 4], [u8; 4])>,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(MIN_MESSAGE_LEN);
    message.resize(FIXED_LEN, 0);
    message[0] = 1; // op: BOOTREQUEST
    message[1] = 1; // htype: Ethernet
    message[2] = 6; // hlen
    message[4..8].copy_from_slice(&xid.to_be_bytes());
    message[10..12].copy_from_slice(&FLAG_BROADCAST.to_be_bytes());
    message[28..34].copy_from_slice(&mac);
    message.extend_from_slice(&MAGIC_COOKIE);
    message.extend_from_slice(&[OPT_MESSAGE_TYPE, 1, kind]);
    if let Some((requested_ip, server)) = request {
        message.extend_from_slice(&[OPT_REQUESTED_IP, 4]);
        message.extend_from_slice(&requested_ip);
        message.extend_from_slice(&[OPT_SERVER_ID, 4]);
        message.extend_from_slice(&server);
    }
    message.extend_from_slice(&[
        OPT_PARAMETER_LIST,
        4,
        OPT_SUBNET_MASK,
        OPT_ROUTER,
        OPT_DNS,
        OPT_LEASE_TIME,
    ]);
    message.push(OPT_END);
    if message.len() < MIN_MESSAGE_LEN {
        message.resize(MIN_MESSAGE_LEN, OPT_PAD);
    }
    message
}

/// Receive hook for IPv4 UDP payloads, ahead of the destination-address check
/// (the reply is addressed to the broadcast address or to the offered address,
/// neither of which is configured yet). Returns true when it consumed a reply
/// to the running exchange.
pub fn receive(ip_payload: &[u8]) -> bool {
    let awaited = AWAITED.load(Ordering::Acquire);
    if awaited == 0 {
        return false;
    }
    let Some((header, message)) = udp::UdpHeader::parse(ip_payload) else {
        return false;
    };
    if header.dst_port != CLIENT_PORT || header.src_port != SERVER_PORT {
        return false;
    }
    let Some(reply) = parse_reply(message, XID.load(Ordering::Relaxed), *CLIENT_MAC.lock()) else {
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
        nameserver: None,
        server: None,
        lease_secs: None,
    };
    let mut options = &message[FIXED_LEN + 4..];
    while let Some((&code, rest)) = options.split_first() {
        match code {
            OPT_PAD => {
                options = rest;
                continue;
            }
            OPT_END => break,
            _ => {}
        }
        let (&len, rest) = rest.split_first()?;
        let value = rest.get(..len as usize)?;
        options = &rest[len as usize..];
        let address = || value.get(..4).map(|a| [a[0], a[1], a[2], a[3]]);
        match code {
            OPT_MESSAGE_TYPE => reply.kind = *value.first()?,
            OPT_SUBNET_MASK => reply.subnet_mask = address(),
            // Both lists are in order of preference: the first entry is used.
            OPT_ROUTER => reply.router = address(),
            OPT_DNS => reply.nameserver = address(),
            OPT_SERVER_ID => reply.server = address(),
            OPT_LEASE_TIME => reply.lease_secs = address().map(u32::from_be_bytes),
            _ => {}
        }
    }
    (reply.kind != 0).then_some(reply)
}
