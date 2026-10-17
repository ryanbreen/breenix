//! Bounded boot connectivity query, from RFC 1035 sections 4.1.1-4.1.4.
//! Uses DHCP's nameserver; this is independent of userspace resolver sockets.
use super::{dhcp::Lease, ipv4::Ipv4Packet, udp};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

const PORT: u16 = 49151;
const QUESTION: &[u8] = b"\x07example\x03com\0\0\x01\0\x01";
static SERVER: AtomicU32 = AtomicU32::new(0);
static ID: AtomicU16 = AtomicU16::new(0);
static ANSWER: AtomicBool = AtomicBool::new(false);

pub fn run(lease: Lease) {
    let Some(server) = lease.nameservers.into_iter().flatten().next() else {
        return;
    };
    let id = crate::arch_impl::x86_64::timer::rdtsc() as u16;
    ID.store(id, Ordering::Relaxed);
    ANSWER.store(false, Ordering::Relaxed);
    SERVER.store(u32::from_be_bytes(server), Ordering::Release);
    let mut query = [0u8; 29];
    query[..2].copy_from_slice(&id.to_be_bytes());
    query[2] = 1; // recursion desired
    query[5] = 1; // one question
    query[12..].copy_from_slice(QUESTION);
    for _ in 0..5 {
        // Retry gateway discovery too: a newly forwarding link may lose ARP.
        let _ = super::arp::request(&lease.gateway);
        let _ = super::ping(lease.gateway);
        if let Ok(datagram) = udp::try_build_udp_packet(lease.ip_addr, server, PORT, 53, &query) {
            let _ = super::send_ipv4(server, super::ipv4::PROTOCOL_UDP, &datagram);
        }
        for _ in 0..200 {
            if ANSWER.load(Ordering::Acquire) && super::icmp::gateway_echo_received() {
                break;
            }
            let until = crate::time::get_monotonic_time_ns();
            let start = until.0 * 1_000_000_000 + until.1;
            while {
                let t = crate::time::get_monotonic_time_ns();
                t.0 * 1_000_000_000 + t.1 - start < 10_000_000
            } {
                crate::arch_halt_with_interrupts();
            }
        }
        if ANSWER.load(Ordering::Acquire) && super::icmp::gateway_echo_received() {
            log::info!("[dns] answer received");
            break;
        }
    }
    SERVER.store(0, Ordering::Release);
}

fn word(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

// Validate compressed names with a hop bound, without recursive traversal.
fn name_end(bytes: &[u8], mut at: usize) -> Option<usize> {
    let mut end = None;
    let mut expanded = 0;
    for _ in 0..128 {
        let n = *bytes.get(at)? as usize;
        if n == 0 {
            return Some(end.unwrap_or(at + 1));
        }
        if n & 0xc0 == 0xc0 {
            let target = (word(bytes, at)? & 0x3fff) as usize;
            if target >= at {
                return None;
            }
            end.get_or_insert(at + 2);
            at = target;
        } else {
            if n > 63 {
                return None;
            }
            expanded += n + 1;
            if expanded > 254 {
                return None;
            }
            at += n + 1;
            bytes.get(..at)?;
        }
    }
    None
}

fn valid_answer(bytes: &[u8]) -> Option<()> {
    if word(bytes, 0)? != ID.load(Ordering::Relaxed)
        || word(bytes, 2)? & 0xfa0f != 0x8000 // response, standard query, no truncation/error
        || word(bytes, 4)? != 1 || bytes.get(12..29)? != QUESTION
    {
        return None;
    }
    let mut at = 29;
    for _ in 0..word(bytes, 6)? {
        at = name_end(bytes, at)?;
        let kind = word(bytes, at)?;
        let class = word(bytes, at + 2)?;
        let len = word(bytes, at + 8)? as usize;
        at += 10;
        let data = bytes.get(at..at + len)?;
        if kind == 1 && class == 1 && data.len() == 4 && data[0] != 0 && data[0] < 224 {
            return Some(());
        }
        at += len;
    }
    None
}

pub fn receive(ip: &Ipv4Packet, header: &udp::UdpHeader, bytes: &[u8]) -> bool {
    let server = SERVER.load(Ordering::Acquire);
    if server == 0
        || ip.src_ip != server.to_be_bytes()
        || header.src_port != 53
        || header.dst_port != PORT
    {
        return false;
    }
    if valid_answer(bytes).is_some() {
        ANSWER.store(true, Ordering::Release);
    }
    true
}
