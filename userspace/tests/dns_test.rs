//! DNS resolution userspace test
//!
//! Tests the DNS client from userspace:
//! 1. Resolve a well-known hostname (www.google.com)
//! 2. Verify we get a valid IPv4 address
//! 3. Test NXDOMAIN handling for nonexistent domains
//!
//! Requires QEMU SLIRP networking (10.0.2.3 is SLIRP's DNS server)

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::dns::{resolve, DnsError, SLIRP_DNS};
use libbreenix::io;
use libbreenix::process;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("DNS Test: Starting\n");

    // Test 1: Resolve www.google.com
    io::print("DNS_TEST: resolving www.google.com...\n");
    match resolve("www.google.com", SLIRP_DNS) {
        Ok(result) => {
            io::print("DNS_TEST: resolved ip=");
            print_ip(result.addr);
            io::print(" ttl=");
            print_num(result.ttl as u64);
            io::print("\n");

            // Verify it's a valid public IP (not 0.0.0.0 or 127.x.x.x)
            if result.addr[0] != 0 && result.addr[0] != 127 {
                io::print("DNS_TEST: google_resolve OK\n");
            } else {
                io::print("DNS_TEST: google_resolve FAILED (invalid IP)\n");
                process::exit(1);
            }
        }
        Err(e) => {
            io::print("DNS_TEST: google_resolve FAILED err=");
            print_error(e);
            io::print("\n");
            process::exit(1);
        }
    }

    // Test 2: Resolve example.com (another reliable domain)
    io::print("DNS_TEST: resolving example.com...\n");
    match resolve("example.com", SLIRP_DNS) {
        Ok(result) => {
            io::print("DNS_TEST: resolved ip=");
            print_ip(result.addr);
            io::print("\n");

            // example.com should resolve to a valid public IP
            if result.addr[0] != 0 && result.addr[0] != 127 {
                io::print("DNS_TEST: example_resolve OK\n");
            } else {
                io::print("DNS_TEST: example_resolve FAILED (invalid IP)\n");
                process::exit(2);
            }
        }
        Err(e) => {
            io::print("DNS_TEST: example_resolve FAILED err=");
            print_error(e);
            io::print("\n");
            process::exit(2);
        }
    }

    // Test 3: NXDOMAIN test - nonexistent domain should fail
    io::print("DNS_TEST: testing NXDOMAIN...\n");
    match resolve("this.domain.does.not.exist.invalid", SLIRP_DNS) {
        Err(DnsError::ServerError(3)) => {
            // RCODE 3 = NXDOMAIN - this is the expected result
            io::print("DNS_TEST: nxdomain OK (RCODE 3)\n");
        }
        Err(DnsError::NoAddress) => {
            // Also acceptable - no A records found
            io::print("DNS_TEST: nxdomain OK (no address)\n");
        }
        Err(DnsError::Timeout) => {
            // Timeout is acceptable for invalid TLD - some DNS servers don't respond
            io::print("DNS_TEST: nxdomain OK (timeout)\n");
        }
        Ok(_) => {
            io::print("DNS_TEST: nxdomain FAILED (should not resolve)\n");
            process::exit(3);
        }
        Err(e) => {
            // Other errors indicate bugs - fail the test
            io::print("DNS_TEST: nxdomain FAILED unexpected err=");
            print_error(e);
            io::print("\n");
            process::exit(3);
        }
    }

    io::print("DNS Test: All tests passed!\n");
    process::exit(0);
}

/// Print an IPv4 address
fn print_ip(ip: [u8; 4]) {
    print_num(ip[0] as u64);
    io::print(".");
    print_num(ip[1] as u64);
    io::print(".");
    print_num(ip[2] as u64);
    io::print(".");
    print_num(ip[3] as u64);
}

/// Print a DNS error
fn print_error(e: DnsError) {
    match e {
        DnsError::SocketError => io::print("SocketError"),
        DnsError::BindError => io::print("BindError"),
        DnsError::SendError => io::print("SendError"),
        DnsError::RecvError => io::print("RecvError"),
        DnsError::Timeout => io::print("Timeout"),
        DnsError::ParseError => io::print("ParseError"),
        DnsError::ServerError(c) => {
            io::print("ServerError(");
            print_num(c as u64);
            io::print(")");
        }
        DnsError::NoAddress => io::print("NoAddress"),
        DnsError::HostnameTooLong => io::print("HostnameTooLong"),
        DnsError::InvalidHostname => io::print("InvalidHostname"),
    }
}

/// Print a number (no formatting library available)
fn print_num(mut n: u64) {
    if n == 0 {
        io::print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }

    // Reverse and print
    while i > 0 {
        i -= 1;
        let ch = [buf[i]];
        if let Ok(s) = core::str::from_utf8(&ch) {
            io::print(s);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("DNS Test: PANIC!\n");
    process::exit(99);
}
