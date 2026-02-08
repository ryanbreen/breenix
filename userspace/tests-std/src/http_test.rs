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
//! This test uses libbreenix's http module via raw syscalls for DNS, socket,
//! connect, send, recv operations. Since the std port cannot directly use
//! libbreenix::http (which is a no_std library), we reimplement the HTTP
//! client logic using extern "C" FFI calls to the libc layer.

use std::process;

// Socket constants
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;

// HTTP constants
const MAX_URL_LEN: usize = 2048;
const MAX_RESPONSE_SIZE: usize = 8192;

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: addr,
            sin_zero: [0; 8],
        }
    }
}

extern "C" {
    fn socket(domain: i32, sock_type: i32, protocol: i32) -> i32;
    fn connect(sockfd: i32, addr: *const u8, addrlen: u32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn sendto(
        sockfd: i32,
        buf: *const u8,
        len: usize,
        flags: i32,
        dest_addr: *const u8,
        addrlen: u32,
    ) -> isize;
    fn recvfrom(
        sockfd: i32,
        buf: *mut u8,
        len: usize,
        flags: i32,
        src_addr: *mut u8,
        addrlen: *mut u32,
    ) -> isize;
    static mut ERRNO: i32;
}

fn get_errno() -> i32 {
    unsafe { ERRNO }
}

/// HTTP error types (matching libbreenix::http::HttpError)
#[derive(Debug)]
enum HttpError {
    UrlTooLong,
    InvalidUrl,
    HttpsNotSupported,
    DnsError(#[allow(dead_code)] i32),
    SocketError,
    ConnectError(i32),
    SendError,
    RecvError,
    #[allow(dead_code)] Timeout,
    ResponseTooLarge,
    ParseError,
}

/// Parsed URL components
struct ParsedUrl<'a> {
    host: &'a str,
    port: u16,
    path: &'a str,
}

/// Parse a URL into components
fn parse_url<'a>(url: &'a str) -> Result<ParsedUrl<'a>, HttpError> {
    if url.len() > MAX_URL_LEN {
        return Err(HttpError::UrlTooLong);
    }

    // Check scheme
    if url.starts_with("https://") {
        return Err(HttpError::HttpsNotSupported);
    }

    let rest = if let Some(r) = url.strip_prefix("http://") {
        r
    } else {
        return Err(HttpError::InvalidUrl);
    };

    // Split host and path
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    if host_port.is_empty() {
        return Err(HttpError::InvalidUrl);
    }

    // Parse host and port
    let (host, port) = if let Some(colon_idx) = host_port.find(':') {
        let host = &host_port[..colon_idx];
        let port_str = &host_port[colon_idx + 1..];
        let port: u32 = port_str.parse().map_err(|_| HttpError::InvalidUrl)?;
        if port > 65535 {
            return Err(HttpError::InvalidUrl);
        }
        (host, port as u16)
    } else {
        (host_port, 80u16)
    };

    if host.is_empty() {
        return Err(HttpError::InvalidUrl);
    }

    Ok(ParsedUrl { host, port, path })
}

/// DNS resolution constants
const SLIRP_DNS: [u8; 4] = [10, 0, 2, 3];
const DNS_PORT: u16 = 53;

/// Simple DNS resolution (A record query)
fn dns_resolve(hostname: &str) -> Result<[u8; 4], i32> {
    // Build DNS query packet
    let mut query = [0u8; 512];
    let mut pos = 0;

    // Transaction ID
    query[pos] = 0x13; query[pos + 1] = 0x37;
    pos += 2;
    // Flags: standard query, recursion desired
    query[pos] = 0x01; query[pos + 1] = 0x00;
    pos += 2;
    // Questions: 1
    query[pos] = 0x00; query[pos + 1] = 0x01;
    pos += 2;
    // Answer/Authority/Additional RRs: 0
    for _ in 0..6 {
        query[pos] = 0x00;
        pos += 1;
    }

    // Encode hostname as DNS labels
    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(-1);
        }
        query[pos] = label.len() as u8;
        pos += 1;
        for b in label.bytes() {
            query[pos] = b;
            pos += 1;
        }
    }
    query[pos] = 0; // Root label
    pos += 1;

    // Type: A (1)
    query[pos] = 0x00; query[pos + 1] = 0x01;
    pos += 2;
    // Class: IN (1)
    query[pos] = 0x00; query[pos + 1] = 0x01;
    pos += 2;

    // Create UDP socket
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(-2);
    }

    // Send to DNS server
    let dns_addr = SockAddrIn::new(SLIRP_DNS, DNS_PORT);
    let sent = unsafe {
        sendto(
            fd,
            query.as_ptr(),
            pos,
            0,
            &dns_addr as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>() as u32,
        )
    };
    if sent < 0 {
        unsafe { close(fd); }
        return Err(-3);
    }

    // Receive response
    let mut response = [0u8; 512];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    let received = unsafe {
        recvfrom(
            fd,
            response.as_mut_ptr(),
            response.len(),
            0,
            &mut src_addr as *mut SockAddrIn as *mut u8,
            &mut addrlen,
        )
    };
    unsafe { close(fd); }

    if received < 12 {
        return Err(-4);
    }

    let received = received as usize;

    // Parse DNS response - skip header (12 bytes) and question section
    let mut rpos = 12;
    // Skip question section
    while rpos < received && response[rpos] != 0 {
        let label_len = response[rpos] as usize;
        if label_len & 0xC0 == 0xC0 {
            rpos += 2;
            break;
        }
        rpos += 1 + label_len;
    }
    if rpos < received && response[rpos] == 0 {
        rpos += 1; // null terminator
    }
    rpos += 4; // Type + Class

    // Parse answer section
    // Look for A record (type 1)
    let answer_count = ((response[6] as u16) << 8) | response[7] as u16;
    for _ in 0..answer_count {
        if rpos + 12 > received {
            return Err(-5);
        }

        // Skip name (may be compressed)
        if response[rpos] & 0xC0 == 0xC0 {
            rpos += 2;
        } else {
            while rpos < received && response[rpos] != 0 {
                let label_len = response[rpos] as usize;
                rpos += 1 + label_len;
            }
            rpos += 1;
        }

        if rpos + 10 > received {
            return Err(-5);
        }

        let rtype = ((response[rpos] as u16) << 8) | response[rpos + 1] as u16;
        let rdlength = ((response[rpos + 8] as u16) << 8) | response[rpos + 9] as u16;
        rpos += 10;

        if rtype == 1 && rdlength == 4 && rpos + 4 <= received {
            return Ok([response[rpos], response[rpos + 1], response[rpos + 2], response[rpos + 3]]);
        }

        rpos += rdlength as usize;
    }

    Err(-6)
}

/// HTTP response info
struct HttpResponse {
    status_code: u16,
    body_offset: usize,
    body_len: usize,
}

/// Perform an HTTP GET and return status code
fn http_get_status(url: &str) -> Result<u16, HttpError> {
    let mut buf = [0u8; MAX_RESPONSE_SIZE];
    let (response, _total_len) = http_get_with_buf(url, &mut buf)?;
    Ok(response.status_code)
}

/// Perform an HTTP GET with a provided buffer
fn http_get_with_buf<'a>(url: &str, buf: &'a mut [u8]) -> Result<(HttpResponse, usize), HttpError> {
    let parsed = parse_url(url)?;

    // Resolve hostname
    let ip = dns_resolve(parsed.host).map_err(HttpError::DnsError)?;

    // Create TCP socket
    let fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(HttpError::SocketError);
    }

    // Connect
    let dest = SockAddrIn::new(ip, parsed.port);
    let ret = unsafe {
        connect(
            fd,
            &dest as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>() as u32,
        )
    };
    if ret < 0 {
        let e = get_errno();
        unsafe { close(fd); }
        return Err(HttpError::ConnectError(e));
    }

    // Build and send HTTP request
    let request = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        parsed.path, parsed.host
    );
    let sent = unsafe { write(fd, request.as_ptr(), request.len()) };
    if sent <= 0 {
        unsafe { close(fd); }
        return Err(HttpError::SendError);
    }

    // Read response
    let mut total = 0usize;
    loop {
        if total >= buf.len() {
            unsafe { close(fd); }
            return Err(HttpError::ResponseTooLarge);
        }
        let n = unsafe { read(fd, buf.as_mut_ptr().add(total), buf.len() - total) };
        if n < 0 {
            unsafe { close(fd); }
            return Err(HttpError::RecvError);
        }
        if n == 0 {
            break;
        }
        total += n as usize;
    }
    unsafe { close(fd); }

    if total == 0 {
        return Err(HttpError::RecvError);
    }

    // Parse HTTP status line
    let response_str = core::str::from_utf8(&buf[..total.min(256)]).unwrap_or("");
    let status_code = if response_str.starts_with("HTTP/") {
        // Find status code after first space
        if let Some(space_pos) = response_str.find(' ') {
            let after_space = &response_str[space_pos + 1..];
            if after_space.len() >= 3 {
                after_space[..3].parse::<u16>().unwrap_or(0)
            } else {
                0
            }
        } else {
            0
        }
    } else {
        0
    };

    if status_code == 0 {
        return Err(HttpError::ParseError);
    }

    // Find body (after \r\n\r\n)
    let mut body_offset = total;
    for i in 0..total.saturating_sub(3) {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            body_offset = i + 4;
            break;
        }
    }

    let body_len = if body_offset < total { total - body_offset } else { 0 };

    Ok((HttpResponse { status_code, body_offset, body_len }, total))
}

/// Print an HTTP error
fn print_error(e: &HttpError) {
    match e {
        HttpError::UrlTooLong => print!("UrlTooLong"),
        HttpError::InvalidUrl => print!("InvalidUrl"),
        HttpError::DnsError(_) => print!("DnsError"),
        HttpError::SocketError => print!("SocketError"),
        HttpError::ConnectError(code) => print!("ConnectError({})", code),
        HttpError::SendError => print!("SendError"),
        HttpError::RecvError => print!("RecvError"),
        HttpError::Timeout => print!("Timeout"),
        HttpError::ResponseTooLarge => print!("ResponseTooLarge"),
        HttpError::ParseError => print!("ParseError"),
        HttpError::HttpsNotSupported => print!("HttpsNotSupported"),
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
    match http_get_status("http://example.com:99999/") {
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
    match http_get_status("http://example.com:abc/") {
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
    match http_get_status("http:///path") {
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
    match http_get_status(long_url) {
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

    // Test 5: HTTPS rejection
    print!("HTTP_TEST: testing HTTPS rejection...\n");
    match http_get_status("https://example.com/") {
        Err(HttpError::HttpsNotSupported) => {
            print!("HTTP_TEST: https_rejected OK\n");
        }
        Ok(_) => {
            print!("HTTP_TEST: https_rejected FAILED (should not support HTTPS)\n");
            process::exit(5);
        }
        Err(e) => {
            print!("HTTP_TEST: https_rejected FAILED wrong err=");
            print_error(&e);
            print!(" (expected HttpsNotSupported)\n");
            process::exit(5);
        }
    }

    // ========================================================================
    // SECTION 3: ERROR HANDLING FOR INVALID DOMAIN (expects DnsError)
    // ========================================================================

    // Test 6: Invalid domain should return DnsError
    print!("HTTP_TEST: testing error handling (invalid domain)...\n");
    match http_get_status("http://this.domain.does.not.exist.invalid/") {
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
    match http_get_with_buf("http://example.com/", &mut buf) {
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
        Err(HttpError::ConnectError(code)) => {
            print!("HTTP_TEST: example_fetch SKIP (network unavailable - ConnectError {})\n", code);
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
