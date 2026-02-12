//! HTTP/1.1 client library for Breenix
//!
//! Provides simple HTTP GET requests over TCP sockets.
//!
//! # Example
//!
//! ```rust,ignore
//! use libbreenix::http::{http_get, HttpResponse};
//!
//! // Fetch a web page
//! match http_get("http://example.com/") {
//!     Ok(response) => {
//!         println!("Status: {}", response.status_code);
//!         println!("Body length: {} bytes", response.body_len);
//!     }
//!     Err(e) => println!("HTTP error: {:?}", e),
//! }
//! ```

use crate::dns::{resolve, DnsError, SLIRP_DNS};
use crate::error::Error;
use crate::socket::{connect_inet, recv, send, socket, AF_INET, SOCK_STREAM, SockAddrIn};
use crate::syscall::{nr, raw};
use crate::types::Fd;

// ============================================================================
// Constants
// ============================================================================

/// Default HTTP port
pub const HTTP_PORT: u16 = 80;

/// Maximum URL length
pub const MAX_URL_LEN: usize = 2048;

/// Maximum hostname length
pub const MAX_HOST_LEN: usize = 255;

/// Maximum path length
pub const MAX_PATH_LEN: usize = 1024;

/// Maximum response size (8KB)
pub const MAX_RESPONSE_SIZE: usize = 8192;

/// HTTP request buffer size
pub const REQUEST_BUF_SIZE: usize = 512;

/// CRLF sequence
pub const CRLF: &[u8] = b"\r\n";

// ============================================================================
// Error Types
// ============================================================================

/// HTTP client error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    /// URL is too long
    UrlTooLong,
    /// Invalid URL format (missing scheme, host, etc.)
    InvalidUrl,
    /// DNS resolution failed
    DnsError(DnsError),
    /// Failed to create socket
    SocketError,
    /// Failed to connect to server
    ConnectError,
    /// Failed to send request
    SendError,
    /// Failed to receive response
    RecvError,
    /// Connection timed out
    Timeout,
    /// Response too large
    ResponseTooLarge,
    /// Failed to parse response
    ParseError,
    /// HTTP scheme required (HTTPS not supported)
    HttpsNotSupported,
}

impl From<Error> for HttpError {
    fn from(_e: Error) -> Self {
        // Map OS errors to the closest HttpError variant.
        HttpError::SocketError
    }
}

// ============================================================================
// Response Types
// ============================================================================

/// Parsed HTTP response
#[derive(Clone, Copy)]
pub struct HttpResponse {
    /// HTTP status code (e.g., 200, 404)
    pub status_code: u16,
    /// Total bytes received
    pub total_len: usize,
    /// Body start offset in buffer
    pub body_offset: usize,
    /// Body length
    pub body_len: usize,
}


// ============================================================================
// URL Parsing
// ============================================================================

/// Parsed URL components
struct ParsedUrl<'a> {
    /// Hostname
    host: &'a str,
    /// Port (default 80)
    port: u16,
    /// Path (default "/")
    path: &'a str,
}

/// Parse an HTTP URL
///
/// Supports: http://host/path or http://host:port/path
fn parse_url(url: &str) -> Result<ParsedUrl<'_>, HttpError> {
    // Check for http:// prefix
    let url = if url.starts_with("http://") {
        &url[7..]
    } else if url.starts_with("https://") {
        return Err(HttpError::HttpsNotSupported);
    } else {
        // Assume bare hostname
        url
    };

    // Find end of host (first / or end of string)
    let (host_port, path) = match url.find('/') {
        Some(idx) => (&url[..idx], &url[idx..]),
        None => (url, "/"),
    };

    // Check for port
    let (host, port) = match host_port.rfind(':') {
        Some(idx) => {
            let port_str = &host_port[idx + 1..];
            let port = parse_port(port_str).ok_or(HttpError::InvalidUrl)?;
            (&host_port[..idx], port)
        }
        None => (host_port, HTTP_PORT),
    };

    // Validate
    if host.is_empty() {
        return Err(HttpError::InvalidUrl);
    }
    if host.len() > MAX_HOST_LEN {
        return Err(HttpError::UrlTooLong);
    }
    if path.len() > MAX_PATH_LEN {
        return Err(HttpError::UrlTooLong);
    }

    Ok(ParsedUrl { host, port, path })
}

/// Parse a port number from string
fn parse_port(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 {
        return None;
    }

    let mut port: u32 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        port = port * 10 + (b - b'0') as u32;
        if port > 65535 {
            return None;
        }
    }

    Some(port as u16)
}

// ============================================================================
// Request Building
// ============================================================================

/// Build an HTTP/1.1 GET request
///
/// Returns number of bytes written to buf, or 0 on error.
fn build_request(host: &str, path: &str, buf: &mut [u8]) -> usize {
    let mut pos = 0;

    // GET /path HTTP/1.1\r\n
    let parts: [&[u8]; 5] = [b"GET ", path.as_bytes(), b" HTTP/1.1\r\n", b"Host: ", host.as_bytes()];
    for part in &parts {
        if pos + part.len() > buf.len() {
            return 0;
        }
        buf[pos..pos + part.len()].copy_from_slice(part);
        pos += part.len();
    }

    // \r\nConnection: close\r\n\r\n
    let trailer = b"\r\nConnection: close\r\nUser-Agent: Breenix/1.0\r\n\r\n";
    if pos + trailer.len() > buf.len() {
        return 0;
    }
    buf[pos..pos + trailer.len()].copy_from_slice(trailer);
    pos += trailer.len();

    pos
}

// ============================================================================
// Response Parsing
// ============================================================================

/// Find the end of HTTP headers (double CRLF)
fn find_header_end(buf: &[u8]) -> Option<usize> {
    for i in 0..buf.len().saturating_sub(3) {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i + 4);
        }
    }
    None
}

/// Parse HTTP status line "HTTP/1.x NNN ..."
fn parse_status_line(buf: &[u8]) -> Option<u16> {
    // Find first CRLF
    let line_end = buf.iter().position(|&b| b == b'\r')?;
    let line = &buf[..line_end];

    // Format: "HTTP/1.x NNN Reason"
    // We need at least "HTTP/1.x NNN" = 12 chars
    if line.len() < 12 {
        return None;
    }

    // Check HTTP prefix
    if !line.starts_with(b"HTTP/1.") {
        return None;
    }

    // Find status code (after space)
    let space_idx = line.iter().position(|&b| b == b' ')?;
    let after_space = &line[space_idx + 1..];

    // Parse 3-digit status code
    if after_space.len() < 3 {
        return None;
    }

    let d0 = (after_space[0] as char).to_digit(10)? as u16;
    let d1 = (after_space[1] as char).to_digit(10)? as u16;
    let d2 = (after_space[2] as char).to_digit(10)? as u16;

    Some(d0 * 100 + d1 * 10 + d2)
}

/// Parse HTTP response
fn parse_response(buf: &[u8], len: usize) -> Option<HttpResponse> {
    if len < 12 {
        return None;
    }

    let status_code = parse_status_line(&buf[..len])?;
    let header_end = find_header_end(&buf[..len])?;

    Some(HttpResponse {
        status_code,
        total_len: len,
        body_offset: header_end,
        body_len: len - header_end,
    })
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Close a file descriptor (used internally for cleanup).
fn close_fd(fd: Fd) {
    unsafe {
        raw::syscall1(nr::CLOSE, fd.raw());
    }
}

// ============================================================================
// High-Level API
// ============================================================================

/// Perform an HTTP GET request
///
/// # Arguments
/// * `url` - URL to fetch (e.g., "http://example.com/" or "http://host:port/path")
/// * `response_buf` - Buffer to receive response (should be MAX_RESPONSE_SIZE bytes)
///
/// # Returns
/// * `Ok((HttpResponse, usize))` - Response info and number of bytes in buffer
/// * `Err(HttpError)` - On failure
///
/// # Example
/// ```rust,ignore
/// use libbreenix::http::{http_get_with_buf, MAX_RESPONSE_SIZE};
///
/// let mut buf = [0u8; MAX_RESPONSE_SIZE];
/// match http_get_with_buf("http://example.com/", &mut buf) {
///     Ok((response, len)) => {
///         println!("Status: {}", response.status_code);
///         let body = &buf[response.body_offset..response.body_offset + response.body_len];
///         println!("Body: {:?}", core::str::from_utf8(body));
///     }
///     Err(e) => println!("Error: {:?}", e),
/// }
/// ```
pub fn http_get_with_buf(url: &str, response_buf: &mut [u8]) -> Result<(HttpResponse, usize), HttpError> {
    // Validate URL length
    if url.len() > MAX_URL_LEN {
        return Err(HttpError::UrlTooLong);
    }

    // Parse URL
    let parsed = parse_url(url)?;

    // Resolve hostname to IP
    let dns_result = resolve(parsed.host, SLIRP_DNS).map_err(HttpError::DnsError)?;
    let ip = dns_result.addr;

    // Create TCP socket
    let fd = socket(AF_INET, SOCK_STREAM, 0).map_err(|_| HttpError::SocketError)?;

    // Connect to server
    let server_addr = SockAddrIn::new(ip, parsed.port);
    if let Err(_e) = connect_inet(fd, &server_addr) {
        close_fd(fd);
        return Err(HttpError::ConnectError);
    }

    // Build request
    let mut request_buf = [0u8; REQUEST_BUF_SIZE];
    let request_len = build_request(parsed.host, parsed.path, &mut request_buf);
    if request_len == 0 {
        close_fd(fd);
        return Err(HttpError::InvalidUrl);
    }

    // Send request
    match send(fd, &request_buf[..request_len]) {
        Ok(written) if written == request_len => {}
        _ => {
            close_fd(fd);
            return Err(HttpError::SendError);
        }
    }

    // Receive response
    let mut total_received = 0usize;
    let max_read = response_buf.len();

    // Read until connection closes or buffer full
    for _ in 0..100 {
        // Safety limit on iterations
        if total_received >= max_read {
            close_fd(fd);
            return Err(HttpError::ResponseTooLarge);
        }

        match recv(fd, &mut response_buf[total_received..]) {
            Ok(0) => break, // Connection closed
            Ok(n) => {
                total_received += n;
                // Check if we have complete headers
                if find_header_end(&response_buf[..total_received]).is_some() {
                    // For Connection: close, keep reading until EOF
                    // We'll break when read returns 0
                }
            }
            Err(_) => break, // Error or connection closed
        }
    }

    close_fd(fd);

    if total_received == 0 {
        return Err(HttpError::Timeout);
    }

    // Parse response
    let response = parse_response(response_buf, total_received).ok_or(HttpError::ParseError)?;

    Ok((response, total_received))
}

/// Simple HTTP GET that returns just the status code
///
/// Useful for basic connectivity tests.
pub fn http_get_status(url: &str) -> Result<u16, HttpError> {
    let mut buf = [0u8; MAX_RESPONSE_SIZE];
    let (response, _) = http_get_with_buf(url, &mut buf)?;
    Ok(response.status_code)
}

/// Check if a URL is reachable (returns 2xx status)
pub fn http_ping(url: &str) -> bool {
    match http_get_status(url) {
        Ok(code) => (200..300).contains(&code),
        Err(_) => false,
    }
}
