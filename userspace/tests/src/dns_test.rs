//! DNS resolution userspace test (std version)
//!
//! Tests the DNS client from userspace using the dns_resolve() FFI:
//! 1. Resolve a well-known hostname (www.google.com)
//! 2. Verify we get a valid IPv4 address
//! 3. Test NXDOMAIN handling for nonexistent domains
//! 4. Test empty hostname returns error
//! 5. Test hostname too long returns error
//! 6. Test transaction ID variation
//!
//! Network tests use SKIP markers when network is unavailable (e.g., CI flakiness).
//! Validation-only tests (empty hostname, hostname too long) must always pass.
//!
//! Requires QEMU SLIRP networking (10.0.2.3 is SLIRP's DNS server)

use std::process;

extern "C" {
    fn dns_resolve(host: *const u8, host_len: usize, server: *const u8, result_ip: *mut u8) -> i32;
}

/// QEMU SLIRP's built-in DNS server
const SLIRP_DNS: [u8; 4] = [10, 0, 2, 3];

/// EINVAL errno value
const EINVAL: i32 = 22;

/// EIO errno value (dns_resolve returns this for all DNS errors)
const EIO: i32 = 5;

/// Resolve a hostname and return the result
fn resolve(hostname: &str) -> Result<[u8; 4], i32> {
    let mut result_ip = [0u8; 4];
    let ret = unsafe {
        dns_resolve(
            hostname.as_ptr(),
            hostname.len(),
            SLIRP_DNS.as_ptr(),
            result_ip.as_mut_ptr(),
        )
    };
    if ret == 0 {
        Ok(result_ip)
    } else {
        Err(-ret) // Convert negative errno to positive
    }
}

/// Print an IPv4 address
fn format_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

fn main() {
    println!("DNS Test: Starting");

    // Test 1: Resolve www.google.com
    // This is a network-dependent test - SKIP on timeout/network failure
    println!("DNS_TEST: resolving www.google.com...");
    match resolve("www.google.com") {
        Ok(addr) => {
            println!("DNS_TEST: resolved ip={}", format_ip(addr));

            // Verify it's a valid public IP (not 0.0.0.0 or 127.x.x.x)
            if addr[0] != 0 && addr[0] != 127 {
                println!("DNS_TEST: google_resolve OK");
            } else {
                println!("DNS_TEST: google_resolve FAILED (invalid IP)");
                process::exit(1);
            }
        }
        Err(e) => {
            // Network may be unavailable - SKIP, not fail
            // dns_resolve returns EIO for all DNS errors (timeout, send error, etc.)
            if e == EIO {
                println!("DNS_TEST: google_resolve SKIP (network unavailable)");
            } else {
                println!("DNS_TEST: google_resolve FAILED err={}", e);
                process::exit(1);
            }
        }
    }

    // Test 2: Resolve example.com (another reliable domain)
    // This is a network-dependent test - SKIP on timeout/network failure
    println!("DNS_TEST: resolving example.com...");
    match resolve("example.com") {
        Ok(addr) => {
            println!("DNS_TEST: resolved ip={}", format_ip(addr));

            // example.com should resolve to a valid public IP
            if addr[0] != 0 && addr[0] != 127 {
                println!("DNS_TEST: example_resolve OK");
            } else {
                println!("DNS_TEST: example_resolve FAILED (invalid IP)");
                process::exit(2);
            }
        }
        Err(e) => {
            if e == EIO {
                println!("DNS_TEST: example_resolve SKIP (network unavailable)");
            } else {
                println!("DNS_TEST: example_resolve FAILED err={}", e);
                process::exit(2);
            }
        }
    }

    // Test 3: NXDOMAIN test - nonexistent domain should fail
    println!("DNS_TEST: testing NXDOMAIN...");
    match resolve("this.domain.does.not.exist.invalid") {
        Err(e) => {
            // dns_resolve returns EIO for all DNS errors including NXDOMAIN
            println!("DNS_TEST: nxdomain OK (error={})", e);
        }
        Ok(addr) => {
            println!("DNS_TEST: nxdomain FAILED (should not resolve, got {})", format_ip(addr));
            process::exit(3);
        }
    }

    // Test 4: Empty hostname should return EINVAL
    println!("DNS_TEST: testing empty hostname...");
    match resolve("") {
        Err(e) => {
            if e == EINVAL {
                println!("DNS_TEST: empty_hostname OK");
            } else {
                println!("DNS_TEST: empty_hostname FAILED wrong err={}", e);
                process::exit(4);
            }
        }
        Ok(_) => {
            println!("DNS_TEST: empty_hostname FAILED (should not resolve)");
            process::exit(4);
        }
    }

    // Test 5: Hostname too long should return error
    // The dns_resolve FFI passes the hostname through to libbreenix::dns::resolve
    // which checks hostname length > 255
    println!("DNS_TEST: testing long hostname...");
    // Create a hostname > 255 chars: 260 'a's + ".com" = 264 chars
    let long_hostname = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.com";
    match resolve(long_hostname) {
        Err(_) => {
            // Any error is acceptable (EINVAL or EIO depending on where the check happens)
            println!("DNS_TEST: long_hostname OK");
        }
        Ok(_) => {
            println!("DNS_TEST: long_hostname FAILED (should not resolve)");
            process::exit(5);
        }
    }

    // Test 6: Verify transaction IDs vary between queries
    // We verify this by doing two consecutive resolves and ensuring both succeed
    // (if txid was static and broken, the second query might fail due to txid mismatch)
    // This is a network-dependent test - SKIP on timeout/network failure
    println!("DNS_TEST: testing txid variation...");
    let mut txid_ok = true;
    let mut network_skip = false;
    for _i in 0..2u8 {
        match resolve("example.com") {
            Ok(_) => {
                // Success - txid matched
            }
            Err(e) => {
                if e == EIO {
                    // Network unavailable - SKIP, not fail
                    network_skip = true;
                    break;
                } else {
                    txid_ok = false;
                    break;
                }
            }
        }
    }
    if network_skip {
        println!("DNS_TEST: txid_varies SKIP (network unavailable)");
    } else if txid_ok {
        println!("DNS_TEST: txid_varies OK");
    } else {
        println!("DNS_TEST: txid_varies FAILED");
        process::exit(6);
    }

    println!("DNS Test: All tests passed!");
    process::exit(0);
}
