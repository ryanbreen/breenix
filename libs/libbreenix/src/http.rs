//! HTTP/1.1 client library for Breenix
//!
//! Provides HTTP and HTTPS requests over TCP sockets.
//!
//! # Example
//!
//! ```rust,ignore
//! use libbreenix::http::{http_get_with_buf, HttpResponse, MAX_RESPONSE_SIZE};
//!
//! let mut buf = [0u8; MAX_RESPONSE_SIZE];
//! match http_get_with_buf("http://example.com/", &mut buf) {
//!     Ok((response, len)) => {
//!         println!("Status: {}", response.status_code);
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

/// Default HTTPS port
pub const HTTPS_PORT: u16 = 443;

/// Maximum URL length
pub const MAX_URL_LEN: usize = 2048;

/// Maximum hostname length
pub const MAX_HOST_LEN: usize = 255;

/// Maximum path length
pub const MAX_PATH_LEN: usize = 1024;

/// Maximum response size (64KB for HTTPS)
pub const MAX_RESPONSE_SIZE: usize = 65536;

/// HTTP request buffer size
pub const REQUEST_BUF_SIZE: usize = 2048;

/// CRLF sequence
pub const CRLF: &[u8] = b"\r\n";

// ============================================================================
// HTTP Method
// ============================================================================

/// HTTP request method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Head,
    Put,
    Delete,
}

impl HttpMethod {
    fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Head => "HEAD",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
        }
    }
}

// ============================================================================
// Request Types
// ============================================================================

/// HTTP request specification
pub struct HttpRequest<'a> {
    /// HTTP method
    pub method: HttpMethod,
    /// Full URL (http:// or https://)
    pub url: &'a str,
    /// Additional headers ("Name: Value" format)
    pub headers: &'a [&'a str],
    /// Optional request body
    pub body: Option<&'a [u8]>,
    /// Skip TLS certificate validation
    pub insecure: bool,
    /// Print progress/debug info to stderr
    pub verbose: bool,
}

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
    /// TLS handshake or encryption error
    TlsError,
}

impl From<Error> for HttpError {
    fn from(_e: Error) -> Self {
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
    /// Port
    port: u16,
    /// Path (default "/")
    path: &'a str,
    /// Whether this is HTTPS
    is_tls: bool,
}

/// Parse an HTTP or HTTPS URL
fn parse_url(url: &str) -> Result<ParsedUrl<'_>, HttpError> {
    let (rest, is_tls, default_port) = if url.starts_with("https://") {
        (&url[8..], true, HTTPS_PORT)
    } else if url.starts_with("http://") {
        (&url[7..], false, HTTP_PORT)
    } else {
        // Assume bare hostname, plain HTTP
        (url, false, HTTP_PORT)
    };

    // Find end of host (first / or end of string)
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    // Check for port
    let (host, port) = match host_port.rfind(':') {
        Some(idx) => {
            let port_str = &host_port[idx + 1..];
            let port = parse_port(port_str).ok_or(HttpError::InvalidUrl)?;
            (&host_port[..idx], port)
        }
        None => (host_port, default_port),
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

    Ok(ParsedUrl {
        host,
        port,
        path,
        is_tls,
    })
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
// Connection Abstraction
// ============================================================================

/// Connection type: plain TCP or TLS-encrypted
#[cfg(feature = "std")]
enum Connection {
    Plain(Fd),
    Tls(crate::tls::stream::TlsStream),
}

#[cfg(not(feature = "std"))]
enum Connection {
    Plain(Fd),
}

impl Connection {
    fn send(&mut self, data: &[u8]) -> Result<usize, HttpError> {
        match self {
            Connection::Plain(fd) => send(*fd, data).map_err(|_| HttpError::SendError),
            #[cfg(feature = "std")]
            Connection::Tls(stream) => stream.write(data).map_err(|_| HttpError::TlsError),
        }
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        match self {
            Connection::Plain(fd) => recv(*fd, buf).map_err(|_| HttpError::RecvError),
            #[cfg(feature = "std")]
            Connection::Tls(stream) => stream.read(buf).map_err(|_| HttpError::TlsError),
        }
    }

    fn close(&mut self) {
        match self {
            Connection::Plain(fd) => close_fd(*fd),
            #[cfg(feature = "std")]
            Connection::Tls(stream) => {
                let _ = stream.close();
                close_fd(stream.fd());
            }
        }
    }
}

// ============================================================================
// Request Building
// ============================================================================

/// Build an HTTP/1.1 request with method, headers, and optional body
///
/// Returns number of bytes written to buf, or 0 on error.
fn build_full_request(
    method: &str,
    host: &str,
    path: &str,
    extra_headers: &[&str],
    body: Option<&[u8]>,
    buf: &mut [u8],
) -> usize {
    let mut pos = 0;

    // Request line: METHOD /path HTTP/1.1\r\n
    let parts: [&[u8]; 5] = [
        method.as_bytes(),
        b" ",
        path.as_bytes(),
        b" HTTP/1.1\r\n",
        b"Host: ",
    ];
    for part in &parts {
        if pos + part.len() > buf.len() {
            return 0;
        }
        buf[pos..pos + part.len()].copy_from_slice(part);
        pos += part.len();
    }

    // Host value + CRLF
    let host_bytes = host.as_bytes();
    if pos + host_bytes.len() + 2 > buf.len() {
        return 0;
    }
    buf[pos..pos + host_bytes.len()].copy_from_slice(host_bytes);
    pos += host_bytes.len();
    buf[pos..pos + 2].copy_from_slice(b"\r\n");
    pos += 2;

    // Default headers
    let defaults = b"Connection: close\r\nUser-Agent: burl/1.0 (Breenix)\r\nAccept-Encoding: identity\r\n";
    if pos + defaults.len() > buf.len() {
        return 0;
    }
    buf[pos..pos + defaults.len()].copy_from_slice(defaults);
    pos += defaults.len();

    // Extra headers
    for header in extra_headers {
        let hdr = header.as_bytes();
        if pos + hdr.len() + 2 > buf.len() {
            return 0;
        }
        buf[pos..pos + hdr.len()].copy_from_slice(hdr);
        pos += hdr.len();
        buf[pos..pos + 2].copy_from_slice(b"\r\n");
        pos += 2;
    }

    // Content-Length for body
    if let Some(body_data) = body {
        let cl = format_content_length(body_data.len());
        let cl_bytes = cl.as_bytes();
        if pos + cl_bytes.len() + 2 > buf.len() {
            return 0;
        }
        buf[pos..pos + cl_bytes.len()].copy_from_slice(cl_bytes);
        pos += cl_bytes.len();
        buf[pos..pos + 2].copy_from_slice(b"\r\n");
        pos += 2;
    }

    // End of headers
    if pos + 2 > buf.len() {
        return 0;
    }
    buf[pos..pos + 2].copy_from_slice(b"\r\n");
    pos += 2;

    // Body
    if let Some(body_data) = body {
        if pos + body_data.len() > buf.len() {
            return 0;
        }
        buf[pos..pos + body_data.len()].copy_from_slice(body_data);
        pos += body_data.len();
    }

    pos
}

/// Format Content-Length header value
fn format_content_length(len: usize) -> ContentLengthBuf {
    ContentLengthBuf::new(len)
}

/// Stack-allocated Content-Length header string
struct ContentLengthBuf {
    buf: [u8; 48],
    len: usize,
}

impl ContentLengthBuf {
    fn new(content_len: usize) -> Self {
        let prefix = b"Content-Length: ";
        let mut buf = [0u8; 48];
        buf[..prefix.len()].copy_from_slice(prefix);
        let mut pos = prefix.len();

        // Convert number to string
        if content_len == 0 {
            buf[pos] = b'0';
            pos += 1;
        } else {
            let mut digits = [0u8; 20];
            let mut n = content_len;
            let mut dpos = 0;
            while n > 0 {
                digits[dpos] = b'0' + (n % 10) as u8;
                n /= 10;
                dpos += 1;
            }
            for i in (0..dpos).rev() {
                buf[pos] = digits[i];
                pos += 1;
            }
        }

        ContentLengthBuf { buf, len: pos }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

// ============================================================================
// Response Parsing
// ============================================================================

/// Convert ASCII byte to lowercase.
fn to_lower(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' {
        b + 32
    } else {
        b
    }
}

/// Case-insensitive search for `needle` in `haystack` (ASCII only).
fn contains_bytes_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=haystack.len() - needle.len() {
        let mut matched = true;
        for k in 0..needle.len() {
            if to_lower(haystack[i + k]) != to_lower(needle[k]) {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

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
    if line.len() < 12 {
        return None;
    }

    if !line.starts_with(b"HTTP/1.") {
        return None;
    }

    let space_idx = line.iter().position(|&b| b == b' ')?;
    let after_space = &line[space_idx + 1..];

    if after_space.len() < 3 {
        return None;
    }

    let d0 = (after_space[0] as char).to_digit(10)? as u16;
    let d1 = (after_space[1] as char).to_digit(10)? as u16;
    let d2 = (after_space[2] as char).to_digit(10)? as u16;

    Some(d0 * 100 + d1 * 10 + d2)
}

/// Find Content-Length value in HTTP headers.
///
/// Scans header lines for a "Content-Length: N" header (case-insensitive)
/// and returns the parsed value. Only matches at line starts to avoid
/// false positives from header values containing the string.
fn find_content_length(buf: &[u8], header_end: usize) -> Option<usize> {
    let headers = &buf[..header_end];
    let cl = b"content-length:";

    let mut i = 0;
    while i + cl.len() <= headers.len() {
        // Only match at header line starts (i == 0 or preceded by \n)
        if i == 0 || headers[i - 1] == b'\n' {
            let mut matched = true;
            for k in 0..cl.len() {
                if to_lower(headers[i + k]) != cl[k] {
                    matched = false;
                    break;
                }
            }

            if matched {
                // Skip optional whitespace after ':'
                let mut j = i + cl.len();
                while j < headers.len() && headers[j] == b' ' {
                    j += 1;
                }
                // Parse digits
                let mut val = 0usize;
                let mut found = false;
                while j < headers.len() && headers[j] >= b'0' && headers[j] <= b'9' {
                    val = val * 10 + (headers[j] - b'0') as usize;
                    found = true;
                    j += 1;
                }
                if found {
                    return Some(val);
                }
            }
        }
        i += 1;
    }
    None
}

/// Check if Transfer-Encoding: chunked is present in headers.
///
/// Scans header lines for "Transfer-Encoding:" (case-insensitive) and checks
/// if the value contains "chunked". Only matches at line starts.
fn is_chunked_encoding(buf: &[u8], header_end: usize) -> bool {
    let headers = &buf[..header_end];
    let te = b"transfer-encoding:";

    let mut i = 0;
    while i + te.len() <= headers.len() {
        // Only match at header line starts (i == 0 or preceded by \n)
        if i == 0 || headers[i - 1] == b'\n' {
            let mut matched = true;
            for k in 0..te.len() {
                if to_lower(headers[i + k]) != te[k] {
                    matched = false;
                    break;
                }
            }

            if matched {
                // Find end of this header line
                let mut j = i + te.len();
                while j < headers.len() && headers[j] != b'\r' && headers[j] != b'\n' {
                    j += 1;
                }
                // Check if value contains "chunked"
                let value = &headers[i + te.len()..j];
                if contains_bytes_ci(value, b"chunked") {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// Check if a chunked response is complete (ends with "0\r\n\r\n" or "0\r\n...\r\n\r\n").
fn is_chunked_complete(buf: &[u8], body_start: usize, total_len: usize) -> bool {
    if total_len < body_start + 5 {
        return false;
    }
    let body = &buf[body_start..total_len];
    // The final chunk is "0\r\n\r\n" (possibly with trailer headers between the CRLFs)
    // Simple check: look for "\r\n0\r\n" near the end, or the body ending with "0\r\n\r\n"
    if body.len() >= 5 && &body[body.len() - 5..] == b"0\r\n\r\n" {
        return true;
    }
    // Also check for the pattern with trailing CRLF after headers
    if body.len() >= 7 && &body[body.len() - 7..] == b"\r\n0\r\n\r\n" {
        return true;
    }
    false
}

/// Decode a chunked transfer-encoded body in place.
///
/// Strips chunk size markers and CRLFs, leaving only the raw body data.
/// Returns the decoded body length.
fn decode_chunked_body(buf: &mut [u8], body_start: usize, body_end: usize) -> usize {
    let mut rp = body_start; // read position
    let mut wp = body_start; // write position

    loop {
        // Parse chunk size (hex digits)
        let mut chunk_size: usize = 0;
        let mut found_digit = false;

        while rp < body_end {
            let b = buf[rp];
            let digit = match b {
                b'0'..=b'9' => Some((b - b'0') as usize),
                b'a'..=b'f' => Some((b - b'a' + 10) as usize),
                b'A'..=b'F' => Some((b - b'A' + 10) as usize),
                _ => None,
            };

            if let Some(d) = digit {
                chunk_size = chunk_size * 16 + d;
                found_digit = true;
                rp += 1;
            } else {
                break;
            }
        }

        if !found_digit {
            break;
        }

        // Skip optional chunk extensions (after ';')
        while rp < body_end && buf[rp] != b'\r' && buf[rp] != b'\n' {
            rp += 1;
        }

        // Skip CRLF after chunk size
        if rp + 1 < body_end && buf[rp] == b'\r' && buf[rp + 1] == b'\n' {
            rp += 2;
        } else if rp < body_end && buf[rp] == b'\n' {
            rp += 1;
        } else {
            break; // Malformed
        }

        // Terminal chunk (size 0)
        if chunk_size == 0 {
            break;
        }

        // Copy chunk data (in-place safe: wp <= rp always)
        let data_end = rp + chunk_size;
        if data_end > body_end {
            // Truncated chunk - copy what we have
            let available = body_end - rp;
            buf.copy_within(rp..rp + available, wp);
            wp += available;
            break;
        }

        buf.copy_within(rp..data_end, wp);
        wp += chunk_size;
        rp = data_end;

        // Skip trailing CRLF after chunk data
        if rp + 1 < body_end && buf[rp] == b'\r' && buf[rp + 1] == b'\n' {
            rp += 2;
        } else if rp < body_end && buf[rp] == b'\n' {
            rp += 1;
        }
    }

    wp - body_start
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

/// Perform a general HTTP/HTTPS request
///
/// Supports all HTTP methods, custom headers, request bodies, and HTTPS.
pub fn http_request(
    request: &HttpRequest<'_>,
    response_buf: &mut [u8],
) -> Result<(HttpResponse, usize), HttpError> {
    let _verbose = request.verbose;
    #[cfg(feature = "std")]
    let verbose = _verbose;

    // Validate URL length
    if request.url.len() > MAX_URL_LEN {
        return Err(HttpError::UrlTooLong);
    }

    // Parse URL
    let parsed = parse_url(request.url)?;
    #[cfg(feature = "std")]
    if verbose {
        eprint!("* Parsed: host={} port={} path={} tls={}\n",
            parsed.host, parsed.port, parsed.path, parsed.is_tls);
    }

    // Resolve hostname to IP
    #[cfg(feature = "std")]
    if verbose { eprint!("* Resolving {}...\n", parsed.host); }
    let dns_result = resolve(parsed.host, SLIRP_DNS).map_err(HttpError::DnsError)?;
    let ip = dns_result.addr;
    #[cfg(feature = "std")]
    if verbose {
        eprint!("* Resolved to {}.{}.{}.{}\n", ip[0], ip[1], ip[2], ip[3]);
    }

    // Create TCP socket
    #[cfg(feature = "std")]
    if verbose { eprint!("* Creating TCP socket...\n"); }
    let fd = socket(AF_INET, SOCK_STREAM, 0).map_err(|_| HttpError::SocketError)?;
    #[cfg(feature = "std")]
    if verbose { eprint!("* Socket created (fd={})\n", fd.raw()); }

    // Connect to server
    #[cfg(feature = "std")]
    if verbose { eprint!("* Connecting to port {}...\n", parsed.port); }
    let server_addr = SockAddrIn::new(ip, parsed.port);
    if let Err(_e) = connect_inet(fd, &server_addr) {
        #[cfg(feature = "std")]
        if verbose { eprint!("* Connect failed\n"); }
        close_fd(fd);
        return Err(HttpError::ConnectError);
    }
    #[cfg(feature = "std")]
    if verbose { eprint!("* Connected\n"); }

    // Establish connection (plain or TLS)
    let mut conn = if parsed.is_tls {
        #[cfg(feature = "std")]
        {
            #[cfg(feature = "std")]
            if verbose { eprint!("* Starting TLS handshake...\n"); }
            match crate::tls::stream::TlsStream::connect_verbose(fd, parsed.host, request.insecure, verbose) {
                Ok(stream) => {
                    #[cfg(feature = "std")]
                    if verbose { eprint!("* TLS handshake complete\n"); }
                    Connection::Tls(stream)
                }
                Err(e) => {
                    #[cfg(feature = "std")]
                    if verbose { eprint!("* TLS handshake FAILED: {:?}\n", e); }
                    let _ = e;
                    close_fd(fd);
                    return Err(HttpError::TlsError);
                }
            }
        }
        #[cfg(not(feature = "std"))]
        {
            close_fd(fd);
            return Err(HttpError::TlsError);
        }
    } else {
        Connection::Plain(fd)
    };

    // Build request
    let mut request_buf = [0u8; REQUEST_BUF_SIZE];
    let request_len = build_full_request(
        request.method.as_str(),
        parsed.host,
        parsed.path,
        request.headers,
        request.body,
        &mut request_buf,
    );
    if request_len == 0 {
        conn.close();
        return Err(HttpError::InvalidUrl);
    }
    #[cfg(feature = "std")]
    if verbose { eprint!("* Sending {} request ({} bytes)...\n", request.method.as_str(), request_len); }

    // Send request
    match conn.send(&request_buf[..request_len]) {
        Ok(written) if written == request_len => {
            #[cfg(feature = "std")]
            if verbose { eprint!("* Request sent\n"); }
        }
        _ => {
            #[cfg(feature = "std")]
            if verbose { eprint!("* Send failed\n"); }
            conn.close();
            return Err(HttpError::SendError);
        }
    }

    // Receive response
    #[cfg(feature = "std")]
    if verbose { eprint!("* Waiting for response...\n"); }
    let mut total_received = 0usize;
    let max_read = response_buf.len();

    // Track whether we've found the header end and what the body completion condition is
    let mut header_end: Option<usize> = None;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;

    for _ in 0..1000 {
        if total_received >= max_read {
            conn.close();
            return Err(HttpError::ResponseTooLarge);
        }

        match conn.recv(&mut response_buf[total_received..]) {
            Ok(0) => break,
            Ok(n) => {
                total_received += n;
                #[cfg(feature = "std")]
                if verbose { eprint!("* Received {} bytes (total: {})\n", n, total_received); }

                // Once we have headers, determine body completion strategy
                if header_end.is_none() {
                    header_end = find_header_end(&response_buf[..total_received]);
                    if let Some(hend) = header_end {
                        content_length = find_content_length(&response_buf[..total_received], hend);
                        chunked = is_chunked_encoding(&response_buf[..total_received], hend);
                        #[cfg(feature = "std")]
                        if verbose {
                            if let Some(cl) = content_length {
                                eprint!("* Content-Length: {}\n", cl);
                            }
                            if chunked {
                                eprint!("* Transfer-Encoding: chunked\n");
                            }
                        }
                    }
                }

                // Check if response is complete
                if let Some(hend) = header_end {
                    if let Some(cl) = content_length {
                        // Content-Length: stop when we have all body bytes
                        let body_received = total_received - hend;
                        if body_received >= cl {
                            #[cfg(feature = "std")]
                            if verbose { eprint!("* Body complete ({}/{} bytes)\n", body_received, cl); }
                            break;
                        }
                    } else if chunked {
                        // Chunked: stop when we see the final chunk marker
                        if is_chunked_complete(response_buf, hend, total_received) {
                            #[cfg(feature = "std")]
                            if verbose { eprint!("* Chunked body complete\n"); }
                            break;
                        }
                    }
                    // No Content-Length and not chunked: read until EOF/error
                }
            }
            Err(_) => break,
        }
    }

    conn.close();

    if total_received == 0 {
        #[cfg(feature = "std")]
        if verbose { eprint!("* No data received\n"); }
        return Err(HttpError::Timeout);
    }

    #[cfg(feature = "std")]
    if verbose { eprint!("* Total received: {} bytes\n", total_received); }

    // Decode chunked transfer encoding if detected
    let decoded_body_len = if chunked {
        if let Some(hend) = header_end {
            let decoded = decode_chunked_body(response_buf, hend, total_received);
            #[cfg(feature = "std")]
            if verbose { eprint!("* Decoded chunked body: {} bytes\n", decoded); }
            Some(decoded)
        } else {
            None
        }
    } else {
        None
    };

    let mut response =
        parse_response(response_buf, total_received).ok_or(HttpError::ParseError)?;

    // Override body_len with decoded length if chunked
    if let Some(dbl) = decoded_body_len {
        response.body_len = dbl;
    }

    Ok((response, total_received))
}

/// Perform an HTTP GET request (convenience wrapper)
///
/// For HTTPS URLs, use `http_request` with an `HttpRequest` struct
/// to control TLS options.
pub fn http_get_with_buf(
    url: &str,
    response_buf: &mut [u8],
) -> Result<(HttpResponse, usize), HttpError> {
    let request = HttpRequest {
        method: HttpMethod::Get,
        url,
        headers: &[],
        body: None,
        insecure: false,
        verbose: false,
    };
    http_request(&request, response_buf)
}

/// Simple HTTP GET that returns just the status code
pub fn http_get_status(url: &str) -> Result<u16, HttpError> {
    let mut buf = [0u8; 8192];
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
