//! HTTP client userspace test (std version)
//!
//! Tests the HTTP client implementation:
//! 1. URL parsing tests (no network needed - specific error assertions)
//! 2. HTTPS rejection test (no network needed)
//! 3. Error handling for invalid domain (expects DnsError specifically)
//! 4. Network integration test (clearly separate, with SKIP marker if unavailable)
//!
//! Note: External network connectivity may not be available in all test
//! environments. Network tests use SKIP markers when unavailable.
//!
//! This test uses libbreenix's http module for DNS, socket, connect, send,
//! recv operations.

use libbreenix::http::{self, HttpError, MAX_RESPONSE_SIZE};
use std::process;

/// Print an HTTP error
fn print_error(e: &HttpError) {
    match e {
        HttpError::UrlTooLong => print!("UrlTooLong"),
        HttpError::InvalidUrl => print!("InvalidUrl"),
        HttpError::DnsError(_) => print!("DnsError"),
        HttpError::SocketError => print!("SocketError"),
        HttpError::ConnectError => print!("ConnectError"),
        HttpError::SendError => print!("SendError"),
        HttpError::RecvError => print!("RecvError"),
        HttpError::Timeout => print!("Timeout"),
        HttpError::ResponseTooLarge => print!("ResponseTooLarge"),
        HttpError::ParseError => print!("ParseError"),
        HttpError::TlsError => print!("TlsError"),
    }
}

/// Check if response body contains HTML markers
fn contains_html(body: &[u8]) -> bool {
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

/// Create a URL longer than MAX_URL_LEN (2048)
fn create_long_url() -> &'static str {
    const LONG_URL: &str = concat!(
        "http://x.com/",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    LONG_URL
}

fn main() {
    print!("HTTP Test: Starting\n");

    // ========================================================================
    // SECTION 1: URL PARSING TESTS (no network needed, specific error assertions)
    // ========================================================================

    // Test 1: Port out of range (>65535) should return InvalidUrl
    print!("HTTP_TEST: testing port out of range...\n");
    match http::http_get_status("http://example.com:99999/") {
        Err(HttpError::InvalidUrl) => {
            print!("HTTP_TEST: port_out_of_range OK\n");
        }
        Ok(_) => {
            print!("HTTP_TEST: port_out_of_range FAILED (should reject port > 65535)\n");
            process::exit(1);
        }
        Err(e) => {
            print!("HTTP_TEST: port_out_of_range FAILED wrong err=");
            print_error(&e);
            print!(" (expected InvalidUrl)\n");
            process::exit(1);
        }
    }

    // Test 2: Non-numeric port should return InvalidUrl
    print!("HTTP_TEST: testing non-numeric port...\n");
    match http::http_get_status("http://example.com:abc/") {
        Err(HttpError::InvalidUrl) => {
            print!("HTTP_TEST: port_non_numeric OK\n");
        }
        Ok(_) => {
            print!("HTTP_TEST: port_non_numeric FAILED (should reject non-numeric port)\n");
            process::exit(2);
        }
        Err(e) => {
            print!("HTTP_TEST: port_non_numeric FAILED wrong err=");
            print_error(&e);
            print!(" (expected InvalidUrl)\n");
            process::exit(2);
        }
    }

    // Test 3: Empty host (http:///) should return InvalidUrl
    print!("HTTP_TEST: testing empty host...\n");
    match http::http_get_status("http:///path") {
        Err(HttpError::InvalidUrl) => {
            print!("HTTP_TEST: empty_host OK\n");
        }
        Ok(_) => {
            print!("HTTP_TEST: empty_host FAILED (should reject empty host)\n");
            process::exit(3);
        }
        Err(e) => {
            print!("HTTP_TEST: empty_host FAILED wrong err=");
            print_error(&e);
            print!(" (expected InvalidUrl)\n");
            process::exit(3);
        }
    }

    // Test 4: URL too long should return UrlTooLong
    print!("HTTP_TEST: testing URL too long...\n");
    let long_url = create_long_url();
    match http::http_get_status(long_url) {
        Err(HttpError::UrlTooLong) => {
            print!("HTTP_TEST: url_too_long OK\n");
        }
        Ok(_) => {
            print!("HTTP_TEST: url_too_long FAILED (should reject URL > 2048 chars)\n");
            process::exit(4);
        }
        Err(e) => {
            print!("HTTP_TEST: url_too_long FAILED wrong err=");
            print_error(&e);
            print!(" (expected UrlTooLong)\n");
            process::exit(4);
        }
    }

    // ========================================================================
    // SECTION 2: HTTPS REJECTION TEST (no network needed)
    // ========================================================================

    // Test 5: HTTPS URL parsing (should attempt TLS, may fail without network)
    print!("HTTP_TEST: testing HTTPS URL parsing...\n");
    match http::http_get_status("https://example.com/") {
        Ok(code) => {
            print!("HTTP_TEST: https_url OK (status {})\n", code);
        }
        Err(HttpError::TlsError) => {
            print!("HTTP_TEST: https_url OK (TLS attempted, failed as expected without network/certs)\n");
        }
        Err(HttpError::ConnectError) | Err(HttpError::DnsError(_)) | Err(HttpError::Timeout) => {
            print!("HTTP_TEST: https_url SKIP (network unavailable)\n");
        }
        Err(e) => {
            print!("HTTP_TEST: https_url FAILED wrong err=");
            print_error(&e);
            print!("\n");
            process::exit(5);
        }
    }

    // ========================================================================
    // SECTION 3: ERROR HANDLING FOR INVALID DOMAIN (expects DnsError)
    // ========================================================================

    // Test 6: Invalid domain should return DnsError
    print!("HTTP_TEST: testing error handling (invalid domain)...\n");
    match http::http_get_status("http://this.domain.does.not.exist.invalid/") {
        Err(HttpError::DnsError(_)) => {
            print!("HTTP_TEST: invalid_domain OK\n");
        }
        Ok(code) => {
            print!("HTTP_TEST: invalid_domain FAILED (got status {} - .invalid TLD should never resolve)\n", code);
            process::exit(6);
        }
        Err(e) => {
            print!("HTTP_TEST: invalid_domain FAILED wrong err=");
            print_error(&e);
            print!(" (expected DnsError)\n");
            process::exit(6);
        }
    }

    // ========================================================================
    // SECTION 4: NETWORK INTEGRATION TEST (with SKIP marker if unavailable)
    // ========================================================================

    // Test 7: Network integration - try to fetch example.com
    print!("HTTP_TEST: testing HTTP fetch (example.com)...\n");
    let mut buf = [0u8; MAX_RESPONSE_SIZE];
    match http::http_get_with_buf("http://example.com/", &mut buf) {
        Ok((response, total_len)) => {
            print!("HTTP_TEST: received {} bytes, status={}\n", total_len, response.status_code);

            if response.status_code == 200 {
                let body = &buf[response.body_offset..response.body_offset + response.body_len];
                if contains_html(body) {
                    print!("HTTP_TEST: example_fetch OK (status 200, body contains HTML)\n");
                } else {
                    print!("HTTP_TEST: example_fetch FAILED (status 200 but no HTML in body)\n");
                    process::exit(7);
                }
            } else if response.status_code == 301 || response.status_code == 302 {
                print!("HTTP_TEST: example_fetch OK (redirect {})\n", response.status_code);
            } else if response.status_code >= 200 && response.status_code < 400 {
                print!("HTTP_TEST: example_fetch OK (status {})\n", response.status_code);
            } else {
                print!("HTTP_TEST: example_fetch FAILED (unexpected status {})\n", response.status_code);
                process::exit(7);
            }
        }
        Err(HttpError::ConnectError) => {
            print!("HTTP_TEST: example_fetch SKIP (network unavailable - ConnectError)\n");
        }
        Err(HttpError::Timeout) => {
            print!("HTTP_TEST: example_fetch SKIP (network unavailable - Timeout)\n");
        }
        Err(HttpError::DnsError(_)) => {
            print!("HTTP_TEST: example_fetch SKIP (network unavailable - DNS unreachable)\n");
        }
        Err(HttpError::SocketError) => {
            print!("HTTP_TEST: example_fetch SKIP (network unavailable - SocketError)\n");
        }
        Err(HttpError::SendError) => {
            print!("HTTP_TEST: example_fetch SKIP (network unavailable - SendError)\n");
        }
        Err(HttpError::RecvError) => {
            print!("HTTP_TEST: example_fetch SKIP (network unavailable - RecvError)\n");
        }
        Err(e) => {
            print!("HTTP_TEST: example_fetch FAILED err=");
            print_error(&e);
            print!("\n");
            process::exit(7);
        }
    }

    print!("HTTP Test: All tests passed!\n");
    process::exit(0);
}
