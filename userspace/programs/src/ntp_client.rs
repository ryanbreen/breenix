//! NTP client for Breenix
//!
//! Queries an NTP server over UDP port 123 and adjusts the system clock
//! using clock_settime(CLOCK_REALTIME). Uses SNTPv4 (RFC 4330) — a
//! simplified subset of NTPv4 that does not require drift tracking or
//! peer selection.
//!
//! Usage: ntp_client [server_ip]
//!   If no server IP is given, queries pool.ntp.org via DNS.
//!   Falls back to Google NTP (216.239.35.0) if DNS fails.

use libbreenix::dns;
use libbreenix::process::yield_now;
use libbreenix::socket::{
    bind_inet, recvfrom, sendto, socket, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK,
};
use libbreenix::syscall::{nr, raw};
use libbreenix::time::{self, clock_settime, now_monotonic, CLOCK_REALTIME};
use libbreenix::types::{Fd, Timespec};

const NTP_PORT: u16 = 123;

// NTP epoch is 1900-01-01; Unix epoch is 1970-01-01.
// Difference: 70 years of seconds (including 17 leap years).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

// NTP packet is 48 bytes
const NTP_PACKET_SIZE: usize = 48;

// Timeout for NTP response
const TIMEOUT_SECS: u64 = 5;

// Google public NTP server
const GOOGLE_NTP: [u8; 4] = [216, 239, 35, 0];

fn close_fd(fd: Fd) {
    unsafe {
        raw::syscall1(nr::CLOSE, fd.raw());
    }
}

/// Build an SNTP request packet (client mode, version 4).
fn build_ntp_request(buf: &mut [u8; NTP_PACKET_SIZE]) {
    // Zero out
    *buf = [0u8; NTP_PACKET_SIZE];

    // LI=0 (no warning), VN=4 (NTPv4), Mode=3 (client)
    // Byte 0: LI(2) | VN(3) | Mode(3) = 0b00_100_011 = 0x23
    buf[0] = 0x23;
}

/// Parse the transmit timestamp from an NTP response.
/// Returns Unix timestamp as (seconds, fractional_nanoseconds).
fn parse_ntp_response(buf: &[u8]) -> Option<(u64, u64)> {
    if buf.len() < NTP_PACKET_SIZE {
        return None;
    }

    // Verify it's a server response: Mode should be 4 (server)
    let mode = buf[0] & 0x07;
    if mode != 4 {
        return None;
    }

    // Transmit timestamp is at bytes 40-47
    // Bytes 40-43: seconds since NTP epoch (1900-01-01)
    // Bytes 44-47: fractional seconds
    let ntp_secs = u32::from_be_bytes([buf[40], buf[41], buf[42], buf[43]]) as u64;
    let ntp_frac = u32::from_be_bytes([buf[44], buf[45], buf[46], buf[47]]) as u64;

    if ntp_secs == 0 {
        return None; // Kiss-of-death or invalid
    }

    // Convert NTP epoch to Unix epoch
    let unix_secs = ntp_secs.checked_sub(NTP_UNIX_OFFSET)?;

    // Convert fractional seconds to nanoseconds
    // frac / 2^32 * 10^9
    let nanos = (ntp_frac * 1_000_000_000) >> 32;

    Some((unix_secs, nanos))
}

/// Query a single NTP server by IP address. Returns Unix timestamp on success.
fn query_ntp_server(server_ip: [u8; 4]) -> Result<(u64, u64), &'static str> {
    let fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0)
        .map_err(|_| "socket failed")?;

    let local_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    if bind_inet(fd, &local_addr).is_err() {
        close_fd(fd);
        return Err("bind failed");
    }

    let mut request = [0u8; NTP_PACKET_SIZE];
    build_ntp_request(&mut request);

    let server_addr = SockAddrIn::new(server_ip, NTP_PORT);
    if sendto(fd, &request, &server_addr).is_err() {
        close_fd(fd);
        return Err("sendto failed");
    }

    // Poll for response with timeout
    let mut resp_buf = [0u8; NTP_PACKET_SIZE];
    let start = now_monotonic().unwrap_or(Timespec { tv_sec: 0, tv_nsec: 0 });
    let deadline = start.tv_sec as u64 + TIMEOUT_SECS;

    loop {
        match recvfrom(fd, &mut resp_buf, None) {
            Ok(len) if len >= NTP_PACKET_SIZE => {
                close_fd(fd);
                return parse_ntp_response(&resp_buf)
                    .ok_or("invalid NTP response");
            }
            _ => {
                let now = now_monotonic().unwrap_or(Timespec { tv_sec: 0, tv_nsec: 0 });
                if now.tv_sec as u64 >= deadline {
                    close_fd(fd);
                    return Err("timeout");
                }
                let _ = yield_now();
            }
        }
    }
}

/// Parse an IPv4 address from "a.b.c.d" format.
fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut parts = s.splitn(4, '.');
    let a = parts.next()?.parse::<u8>().ok()?;
    let b = parts.next()?.parse::<u8>().ok()?;
    let c = parts.next()?.parse::<u8>().ok()?;
    let d = parts.next()?.parse::<u8>().ok()?;
    Some([a, b, c, d])
}

fn main() {
    println!("[ntp] Breenix NTP client starting");

    // Get current time for comparison
    let before = time::now_realtime().unwrap_or(Timespec { tv_sec: 0, tv_nsec: 0 });
    println!("[ntp] Current wall clock: {} s", before.tv_sec);

    // Determine NTP server IP
    let args: Vec<String> = std::env::args().collect();
    let server_ip = if args.len() > 1 {
        match parse_ipv4(&args[1]) {
            Some(ip) => {
                println!("[ntp] Using server {}", args[1]);
                ip
            }
            None => {
                println!("[ntp] Invalid IP '{}', using Google NTP", args[1]);
                GOOGLE_NTP
            }
        }
    } else {
        // Try DNS resolution for pool.ntp.org
        println!("[ntp] Resolving pool.ntp.org...");
        match dns::resolve_auto("pool.ntp.org") {
            Ok(result) => {
                let ip = result.addr;
                println!("[ntp] Resolved to {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                ip
            }
            Err(_) => {
                println!("[ntp] DNS failed, using Google NTP (216.239.35.0)");
                GOOGLE_NTP
            }
        }
    };

    // Query NTP server
    println!(
        "[ntp] Querying {}.{}.{}.{}:{}...",
        server_ip[0], server_ip[1], server_ip[2], server_ip[3], NTP_PORT
    );

    match query_ntp_server(server_ip) {
        Ok((unix_secs, nanos)) => {
            println!("[ntp] NTP time: {} s, {} ns", unix_secs, nanos);

            // Calculate drift
            let drift = unix_secs as i64 - before.tv_sec;
            println!("[ntp] Clock drift: {} s", drift);

            // Set the system clock
            let new_time = Timespec {
                tv_sec: unix_secs as i64,
                tv_nsec: nanos as i64,
            };
            match clock_settime(CLOCK_REALTIME, &new_time) {
                Ok(()) => {
                    println!("[ntp] System clock updated successfully");

                    // Verify
                    let after = time::now_realtime()
                        .unwrap_or(Timespec { tv_sec: 0, tv_nsec: 0 });
                    println!("[ntp] Verified wall clock: {} s", after.tv_sec);
                    println!("[ntp] Adjustment applied: {} s", after.tv_sec - before.tv_sec);
                }
                Err(e) => {
                    println!("[ntp] clock_settime failed: {:?}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            println!("[ntp] NTP query failed: {}", e);
            std::process::exit(1);
        }
    }

    println!("[ntp] Done");
}
