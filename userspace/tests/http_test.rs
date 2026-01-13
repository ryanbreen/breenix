//! HTTP client userspace test
//!
//! Tests the HTTP client implementation:
//! 1. URL parsing tests (no network needed - specific error assertions)
//! 2. HTTPS rejection test (no network needed)
//! 3. Error handling for invalid domain (expects DnsError specifically)
//! 4. Network integration test (clearly separate, with SKIP marker if unavailable)
//!
//! Note: External network connectivity may not be available in all test
//! environments. Network tests use SKIP markers when unavailable.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::http::{http_get_status, http_get_with_buf, HttpError, MAX_RESPONSE_SIZE};
use libbreenix::io;
use libbreenix::process;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("HTTP Test: Starting\n");

    // ========================================================================
    // SECTION 1: URL PARSING TESTS (no network needed, specific error assertions)
    // ========================================================================

    // Test 1: Port out of range (>65535) should return InvalidUrl
    io::print("HTTP_TEST: testing port out of range...\n");
    match http_get_status("http://example.com:99999/") {
        Err(HttpError::InvalidUrl) => {
            io::print("HTTP_TEST: port_out_of_range OK\n");
        }
        Ok(_) => {
            io::print("HTTP_TEST: port_out_of_range FAILED (should reject port > 65535)\n");
            process::exit(1);
        }
        Err(e) => {
            io::print("HTTP_TEST: port_out_of_range FAILED wrong err=");
            print_error(e);
            io::print(" (expected InvalidUrl)\n");
            process::exit(1);
        }
    }

    // Test 2: Non-numeric port should return InvalidUrl
    io::print("HTTP_TEST: testing non-numeric port...\n");
    match http_get_status("http://example.com:abc/") {
        Err(HttpError::InvalidUrl) => {
            io::print("HTTP_TEST: port_non_numeric OK\n");
        }
        Ok(_) => {
            io::print("HTTP_TEST: port_non_numeric FAILED (should reject non-numeric port)\n");
            process::exit(2);
        }
        Err(e) => {
            io::print("HTTP_TEST: port_non_numeric FAILED wrong err=");
            print_error(e);
            io::print(" (expected InvalidUrl)\n");
            process::exit(2);
        }
    }

    // Test 3: Empty host (http:///) should return InvalidUrl
    io::print("HTTP_TEST: testing empty host...\n");
    match http_get_status("http:///path") {
        Err(HttpError::InvalidUrl) => {
            io::print("HTTP_TEST: empty_host OK\n");
        }
        Ok(_) => {
            io::print("HTTP_TEST: empty_host FAILED (should reject empty host)\n");
            process::exit(3);
        }
        Err(e) => {
            io::print("HTTP_TEST: empty_host FAILED wrong err=");
            print_error(e);
            io::print(" (expected InvalidUrl)\n");
            process::exit(3);
        }
    }

    // Test 4: URL too long should return UrlTooLong
    io::print("HTTP_TEST: testing URL too long...\n");
    // Create a URL > 2048 chars (MAX_URL_LEN)
    // Use a stack buffer with a long path
    let long_url = create_long_url();
    match http_get_status(long_url) {
        Err(HttpError::UrlTooLong) => {
            io::print("HTTP_TEST: url_too_long OK\n");
        }
        Ok(_) => {
            io::print("HTTP_TEST: url_too_long FAILED (should reject URL > 2048 chars)\n");
            process::exit(4);
        }
        Err(e) => {
            io::print("HTTP_TEST: url_too_long FAILED wrong err=");
            print_error(e);
            io::print(" (expected UrlTooLong)\n");
            process::exit(4);
        }
    }

    // ========================================================================
    // SECTION 2: HTTPS REJECTION TEST (no network needed)
    // ========================================================================

    // Test 5: HTTPS rejection (tests URL parsing - no network needed)
    io::print("HTTP_TEST: testing HTTPS rejection...\n");
    match http_get_status("https://example.com/") {
        Err(HttpError::HttpsNotSupported) => {
            io::print("HTTP_TEST: https_rejected OK\n");
        }
        Ok(_) => {
            io::print("HTTP_TEST: https_rejected FAILED (should not support HTTPS)\n");
            process::exit(5);
        }
        Err(e) => {
            io::print("HTTP_TEST: https_rejected FAILED wrong err=");
            print_error(e);
            io::print(" (expected HttpsNotSupported)\n");
            process::exit(5);
        }
    }

    // ========================================================================
    // SECTION 3: ERROR HANDLING FOR INVALID DOMAIN (expects DnsError)
    // ========================================================================

    // Test 6: Invalid domain should return DnsError
    // .invalid is a reserved TLD that should never resolve (RFC 2606)
    io::print("HTTP_TEST: testing error handling (invalid domain)...\n");
    match http_get_status("http://this.domain.does.not.exist.invalid/") {
        Err(HttpError::DnsError(_)) => {
            io::print("HTTP_TEST: invalid_domain OK\n");
        }
        Ok(code) => {
            // This should not happen - .invalid TLD should never resolve
            io::print("HTTP_TEST: invalid_domain FAILED (got status ");
            print_num(code as u64);
            io::print(" - .invalid TLD should never resolve)\n");
            process::exit(6);
        }
        Err(e) => {
            io::print("HTTP_TEST: invalid_domain FAILED wrong err=");
            print_error(e);
            io::print(" (expected DnsError)\n");
            process::exit(6);
        }
    }

    // ========================================================================
    // SECTION 4: NETWORK INTEGRATION TEST (with SKIP marker if unavailable)
    // ========================================================================

    // Test 7: Network integration - try to fetch example.com
    // If network works: verify response (status code 200, body contains HTML)
    // If network unavailable: print SKIP marker (NOT OK)
    io::print("HTTP_TEST: testing HTTP fetch (example.com)...\n");
    let mut buf = [0u8; MAX_RESPONSE_SIZE];
    match http_get_with_buf("http://example.com/", &mut buf) {
        Ok((response, total_len)) => {
            // Network is available - verify response properly
            io::print("HTTP_TEST: received ");
            print_num(total_len as u64);
            io::print(" bytes, status=");
            print_num(response.status_code as u64);
            io::print("\n");

            // Verify status code is 200 (or redirect 301/302)
            if response.status_code == 200 {
                // Check body contains HTML
                let body = &buf[response.body_offset..response.body_offset + response.body_len];
                if contains_html(body) {
                    io::print("HTTP_TEST: example_fetch OK (status 200, body contains HTML)\n");
                } else {
                    io::print("HTTP_TEST: example_fetch FAILED (status 200 but no HTML in body)\n");
                    process::exit(7);
                }
            } else if response.status_code == 301 || response.status_code == 302 {
                // Redirect is acceptable - server is responding
                io::print("HTTP_TEST: example_fetch OK (redirect ");
                print_num(response.status_code as u64);
                io::print(")\n");
            } else if response.status_code >= 200 && response.status_code < 400 {
                // Other 2xx/3xx status - acceptable
                io::print("HTTP_TEST: example_fetch OK (status ");
                print_num(response.status_code as u64);
                io::print(")\n");
            } else {
                // 4xx or 5xx is unexpected for example.com
                io::print("HTTP_TEST: example_fetch FAILED (unexpected status ");
                print_num(response.status_code as u64);
                io::print(")\n");
                process::exit(7);
            }
        }
        Err(HttpError::ConnectError(code)) => {
            // Network unreachable - SKIP, not OK
            io::print("HTTP_TEST: example_fetch SKIP (network unavailable - ConnectError ");
            print_num(code as u64);
            io::print(")\n");
        }
        Err(HttpError::Timeout) => {
            // Timeout - SKIP, not OK
            io::print("HTTP_TEST: example_fetch SKIP (network unavailable - Timeout)\n");
        }
        Err(HttpError::DnsError(_)) => {
            // DNS failed - SKIP, not OK
            io::print("HTTP_TEST: example_fetch SKIP (network unavailable - DNS unreachable)\n");
        }
        Err(e) => {
            // Other errors indicate actual bugs in the HTTP client
            io::print("HTTP_TEST: example_fetch FAILED err=");
            print_error(e);
            io::print("\n");
            process::exit(7);
        }
    }

    io::print("HTTP Test: All tests passed!\n");
    process::exit(0);
}

/// Create a URL longer than MAX_URL_LEN (2048)
/// Returns a static string that's > 2048 chars
fn create_long_url() -> &'static str {
    // This URL is exactly 2100 chars: "http://x.com/" (13) + 2087 'a' chars
    // We need > 2048, so 2100 is sufficient
    const LONG_URL: &str = concat!(
        "http://x.com/",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 100
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 200
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 300
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 400
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 500
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 600
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 700
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 800
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 900
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1000
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1100
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1200
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1300
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1400
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1500
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1600
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1700
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1800
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 1900
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 2000
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 2100
    );
    LONG_URL
}

/// Check if response body contains HTML markers
fn contains_html(body: &[u8]) -> bool {
    // Look for common HTML markers (case-insensitive search via lowercase check)
    let html_markers: [&[u8]; 4] = [b"<html", b"<HTML", b"<!doctype", b"<!DOCTYPE"];
    for marker in &html_markers {
        if contains_bytes(body, marker) {
            return true;
        }
    }
    false
}

/// Check if haystack contains needle
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

/// Print an HTTP error
fn print_error(e: HttpError) {
    match e {
        HttpError::UrlTooLong => io::print("UrlTooLong"),
        HttpError::InvalidUrl => io::print("InvalidUrl"),
        HttpError::DnsError(_) => io::print("DnsError"),
        HttpError::SocketError => io::print("SocketError"),
        HttpError::ConnectError(code) => {
            io::print("ConnectError(");
            print_num(code as u64);
            io::print(")");
        }
        HttpError::SendError => io::print("SendError"),
        HttpError::RecvError => io::print("RecvError"),
        HttpError::Timeout => io::print("Timeout"),
        HttpError::ResponseTooLarge => io::print("ResponseTooLarge"),
        HttpError::ParseError => io::print("ParseError"),
        HttpError::HttpsNotSupported => io::print("HttpsNotSupported"),
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
    io::print("HTTP Test: PANIC!\n");
    process::exit(99);
}
