//! DNS client library for Breenix
//!
//! Provides hostname resolution using UDP queries to DNS servers.
//!
//! # Example
//!
//! ```rust,ignore
//! use libbreenix::dns::{resolve, SLIRP_DNS};
//!
//! // Resolve hostname using QEMU's DNS server
//! match resolve("www.google.com", SLIRP_DNS) {
//!     Ok(result) => println!("IP: {:?}", result.addr),
//!     Err(e) => println!("DNS error: {:?}", e),
//! }
//! ```

use crate::io::close;
use crate::process::yield_now;
use crate::socket::{bind, recvfrom, sendto, socket, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK};
use crate::time::now_monotonic;

// ============================================================================
// Constants
// ============================================================================

/// DNS port
pub const DNS_PORT: u16 = 53;

/// DNS record type: A (IPv4 address)
pub const TYPE_A: u16 = 1;

/// DNS record type: CNAME (canonical name)
pub const TYPE_CNAME: u16 = 5;

/// DNS class: Internet
pub const CLASS_IN: u16 = 1;

/// Maximum hostname length
pub const MAX_HOSTNAME_LEN: usize = 255;

/// DNS query/response buffer size (RFC 1035)
pub const DNS_BUF_SIZE: usize = 512;

/// Maximum number of answers to parse
pub const MAX_ANSWERS: usize = 8;

/// QEMU SLIRP's built-in DNS server
pub const SLIRP_DNS: [u8; 4] = [10, 0, 2, 3];

/// Google's public DNS server
pub const GOOGLE_DNS: [u8; 4] = [8, 8, 8, 8];

// ============================================================================
// Error Types
// ============================================================================

/// DNS resolution error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsError {
    /// Failed to create socket
    SocketError,
    /// Failed to bind socket
    BindError,
    /// Failed to send query
    SendError,
    /// Failed to receive response
    RecvError,
    /// Response timeout (no response received)
    Timeout,
    /// Failed to parse response
    ParseError,
    /// DNS server returned an error (RCODE in response)
    /// Common values: 1=FormatError, 2=ServerFailure, 3=NXDOMAIN
    ServerError(u8),
    /// No A record found in response
    NoAddress,
    /// Hostname too long
    HostnameTooLong,
    /// Invalid hostname format
    InvalidHostname,
}

// ============================================================================
// Result Types
// ============================================================================

/// DNS resolution result
#[derive(Debug, Clone, Copy)]
pub struct DnsResult {
    /// Resolved IPv4 address
    pub addr: [u8; 4],
    /// Time to live in seconds
    pub ttl: u32,
}

/// Parsed DNS answer record
#[derive(Debug, Clone, Copy)]
pub struct DnsAnswer {
    /// Record type (TYPE_A, TYPE_CNAME, etc.)
    pub rtype: u16,
    /// Record class (usually CLASS_IN)
    pub rclass: u16,
    /// Time to live in seconds
    pub ttl: u32,
    /// Data length
    pub rdlength: u16,
    /// IPv4 address (only valid if rtype == TYPE_A)
    pub ipv4: [u8; 4],
}

impl DnsAnswer {
    /// Create an empty answer
    const fn empty() -> Self {
        DnsAnswer {
            rtype: 0,
            rclass: 0,
            ttl: 0,
            rdlength: 0,
            ipv4: [0; 4],
        }
    }
}

/// Parsed DNS response
#[derive(Clone, Copy)]
pub struct DnsResponse {
    /// Transaction ID
    pub id: u16,
    /// Response code (0 = success, 3 = NXDOMAIN)
    pub rcode: u8,
    /// Number of answers parsed
    pub answer_count: usize,
    /// Parsed answers
    pub answers: [DnsAnswer; MAX_ANSWERS],
}

impl DnsResponse {
    /// Create an empty response
    fn empty() -> Self {
        DnsResponse {
            id: 0,
            rcode: 0,
            answer_count: 0,
            answers: [DnsAnswer::empty(); MAX_ANSWERS],
        }
    }
}

// ============================================================================
// DNS Header
// ============================================================================

/// DNS header structure (12 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct DnsHeader {
    /// Transaction ID
    id: u16,
    /// Flags: QR(1) OPCODE(4) AA(1) TC(1) RD(1) RA(1) Z(3) RCODE(4)
    flags: u16,
    /// Number of questions
    qdcount: u16,
    /// Number of answer RRs
    ancount: u16,
    /// Number of authority RRs
    nscount: u16,
    /// Number of additional RRs
    arcount: u16,
}

impl DnsHeader {
    /// Create a new query header
    /// Values are stored in host byte order; to_bytes() handles network conversion
    fn new_query(id: u16) -> Self {
        DnsHeader {
            id,
            // QR=0 (query), OPCODE=0 (standard), RD=1 (recursion desired)
            flags: 0x0100,
            qdcount: 1,
            ancount: 0,
            nscount: 0,
            arcount: 0,
        }
    }

    /// Parse header from bytes
    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 12 {
            return None;
        }
        Some(DnsHeader {
            id: u16::from_be_bytes([buf[0], buf[1]]),
            flags: u16::from_be_bytes([buf[2], buf[3]]),
            qdcount: u16::from_be_bytes([buf[4], buf[5]]),
            ancount: u16::from_be_bytes([buf[6], buf[7]]),
            nscount: u16::from_be_bytes([buf[8], buf[9]]),
            arcount: u16::from_be_bytes([buf[10], buf[11]]),
        })
    }

    /// Write header to buffer, returns bytes written
    fn to_bytes(&self, buf: &mut [u8]) -> usize {
        if buf.len() < 12 {
            return 0;
        }
        buf[0..2].copy_from_slice(&self.id.to_be_bytes());
        buf[2..4].copy_from_slice(&self.flags.to_be_bytes());
        buf[4..6].copy_from_slice(&self.qdcount.to_be_bytes());
        buf[6..8].copy_from_slice(&self.ancount.to_be_bytes());
        buf[8..10].copy_from_slice(&self.nscount.to_be_bytes());
        buf[10..12].copy_from_slice(&self.arcount.to_be_bytes());
        12
    }

    /// Get response code (0 = no error, 3 = NXDOMAIN)
    /// Note: flags is already in host byte order from from_bytes()
    fn rcode(&self) -> u8 {
        (self.flags & 0x000F) as u8
    }

    /// Check if this is a response (QR=1)
    /// Note: flags is already in host byte order from from_bytes()
    fn is_response(&self) -> bool {
        (self.flags & 0x8000) != 0
    }

    /// Get answer count in host byte order
    /// Note: ancount is already in host byte order from from_bytes()
    fn answer_count(&self) -> u16 {
        self.ancount
    }

    /// Get question count in host byte order
    /// Note: qdcount is already in host byte order from from_bytes()
    fn question_count(&self) -> u16 {
        self.qdcount
    }
}

// ============================================================================
// Encoding Functions
// ============================================================================

/// Generate a pseudo-random transaction ID for DNS queries
///
/// Uses a simple hash of the hostname combined with a monotonic counter.
/// This isn't cryptographically secure but provides enough variation
/// to avoid transaction ID collisions and basic spoofing.
fn generate_txid(hostname: &str) -> u16 {
    use core::sync::atomic::{AtomicU16, Ordering};
    static COUNTER: AtomicU16 = AtomicU16::new(0);

    // Increment counter for each query
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);

    // Hash hostname bytes using simple multiply-add
    let hash: u16 = hostname
        .bytes()
        .fold(0u16, |acc, b| acc.wrapping_add(b as u16).wrapping_mul(31));

    // Combine hash, counter, and a constant for variation
    hash ^ counter ^ 0xBEEF
}

/// Encode a hostname as DNS wire format labels
///
/// Example: "www.google.com" -> "\x03www\x06google\x03com\x00"
///
/// Returns number of bytes written, or 0 on error.
fn encode_hostname(hostname: &str, buf: &mut [u8]) -> usize {
    if hostname.is_empty() || hostname.len() > MAX_HOSTNAME_LEN {
        return 0;
    }

    let mut pos = 0;

    for label in hostname.split('.') {
        let len = label.len();
        if len == 0 || len > 63 {
            return 0; // Invalid label (empty or too long)
        }
        if pos + 1 + len >= buf.len() {
            return 0; // Buffer too small
        }

        // Write length byte
        buf[pos] = len as u8;
        pos += 1;

        // Write label characters
        buf[pos..pos + len].copy_from_slice(label.as_bytes());
        pos += len;
    }

    // Null terminator
    if pos >= buf.len() {
        return 0;
    }
    buf[pos] = 0;
    pos += 1;

    pos
}

/// Build a DNS query packet for an A record lookup
///
/// Returns number of bytes written to buf, or 0 on error.
pub fn encode_query(hostname: &str, id: u16, buf: &mut [u8]) -> usize {
    if buf.len() < DNS_BUF_SIZE {
        return 0;
    }

    // Write header
    let header = DnsHeader::new_query(id);
    let mut pos = header.to_bytes(buf);
    if pos == 0 {
        return 0;
    }

    // Encode hostname (QNAME)
    let name_len = encode_hostname(hostname, &mut buf[pos..]);
    if name_len == 0 {
        return 0;
    }
    pos += name_len;

    // QTYPE = A (1)
    if pos + 4 > buf.len() {
        return 0;
    }
    buf[pos] = 0;
    buf[pos + 1] = TYPE_A as u8;
    pos += 2;

    // QCLASS = IN (1)
    buf[pos] = 0;
    buf[pos + 1] = CLASS_IN as u8;
    pos += 2;

    pos
}

// ============================================================================
// Parsing Functions
// ============================================================================

/// Skip a DNS name in the buffer (handles compression pointers)
///
/// Returns position after the name, or 0 on error.
fn skip_name(buf: &[u8], mut pos: usize) -> usize {
    let len = buf.len();
    let mut jumps = 0;

    while pos < len {
        let label_len = buf[pos] as usize;

        if label_len == 0 {
            // End of name
            return pos + 1;
        } else if (label_len & 0xC0) == 0xC0 {
            // Compression pointer - 2 bytes total, then we're done
            if pos + 1 >= len {
                return 0;
            }
            return pos + 2;
        } else if label_len > 63 {
            // Invalid label length
            return 0;
        } else {
            pos += 1 + label_len;
        }

        jumps += 1;
        if jumps > 128 {
            return 0; // Prevent infinite loops
        }
    }

    0 // Ran out of buffer
}

/// Parse a DNS response
pub fn parse_response(buf: &[u8]) -> Option<DnsResponse> {
    if buf.len() < 12 {
        return None;
    }

    let header = DnsHeader::from_bytes(buf)?;

    if !header.is_response() {
        return None; // Not a response
    }

    let mut response = DnsResponse::empty();
    response.id = header.id; // Already in host byte order from from_bytes()
    response.rcode = header.rcode();

    if response.rcode != 0 {
        return Some(response); // Error response, no answers to parse
    }

    let mut pos = 12;

    // Skip questions section
    let qdcount = header.question_count();
    for _ in 0..qdcount {
        pos = skip_name(buf, pos);
        if pos == 0 {
            return None;
        }
        pos += 4; // QTYPE + QCLASS
        if pos > buf.len() {
            return None;
        }
    }

    // Parse answers
    let ancount = header.answer_count();
    for i in 0..ancount.min(MAX_ANSWERS as u16) as usize {
        // Skip name
        pos = skip_name(buf, pos);
        if pos == 0 || pos + 10 > buf.len() {
            break;
        }

        let rtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let rclass = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]);
        let ttl = u32::from_be_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]);
        let rdlength = u16::from_be_bytes([buf[pos + 8], buf[pos + 9]]);
        pos += 10;

        if pos + rdlength as usize > buf.len() {
            break;
        }

        response.answers[i] = DnsAnswer {
            rtype,
            rclass,
            ttl,
            rdlength,
            ipv4: if rtype == TYPE_A && rdlength == 4 {
                [buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]
            } else {
                [0; 4]
            },
        };
        response.answer_count += 1;

        pos += rdlength as usize;
    }

    Some(response)
}

// ============================================================================
// High-Level API
// ============================================================================

/// Resolve a hostname to an IPv4 address
///
/// # Arguments
/// * `hostname` - The hostname to resolve (e.g., "www.google.com")
/// * `dns_server` - DNS server IP address (use SLIRP_DNS for QEMU)
///
/// # Returns
/// * `Ok(DnsResult)` with the IPv4 address on success
/// * `Err(DnsError)` on failure
///
/// # Example
/// ```rust,ignore
/// use libbreenix::dns::{resolve, SLIRP_DNS};
///
/// let result = resolve("example.com", SLIRP_DNS)?;
/// println!("IP: {}.{}.{}.{}", result.addr[0], result.addr[1], result.addr[2], result.addr[3]);
/// ```
pub fn resolve(hostname: &str, dns_server: [u8; 4]) -> Result<DnsResult, DnsError> {
    if hostname.is_empty() {
        return Err(DnsError::InvalidHostname);
    }
    if hostname.len() > MAX_HOSTNAME_LEN {
        return Err(DnsError::HostnameTooLong);
    }

    // Create UDP socket with non-blocking mode
    // CRITICAL: Must use SOCK_NONBLOCK because UDP recvfrom now blocks by default.
    // Without this, the DNS resolver would hang forever waiting for a response.
    let fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0).map_err(|_| DnsError::SocketError)?;

    // Bind to ephemeral port (port 0 = kernel assigns)
    let local_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    if bind(fd, &local_addr).is_err() {
        let _ = close(fd as u64);
        return Err(DnsError::BindError);
    }

    // Build query packet
    let mut query_buf = [0u8; DNS_BUF_SIZE];

    // Generate pseudo-random transaction ID based on hostname and counter
    let txid: u16 = generate_txid(hostname);

    let query_len = encode_query(hostname, txid, &mut query_buf);
    if query_len == 0 {
        let _ = close(fd as u64);
        return Err(DnsError::InvalidHostname);
    }

    // Send query to DNS server
    let dns_addr = SockAddrIn::new(dns_server, DNS_PORT);
    if sendto(fd, &query_buf[..query_len], &dns_addr).is_err() {
        let _ = close(fd as u64);
        return Err(DnsError::SendError);
    }

    // Receive response with 5-second timeout
    let mut resp_buf = [0u8; DNS_BUF_SIZE];
    let mut received = false;
    let mut resp_len = 0;

    // Network packets arrive via interrupt → softirq → process_rx().
    // We poll recvfrom() with yield_now() between attempts.
    // DNS resolution via QEMU SLIRP forwards to host DNS, which can take time.
    const TIMEOUT_SECS: u64 = 5;
    let start = now_monotonic();
    let deadline_secs = start.tv_sec as u64 + TIMEOUT_SECS;

    loop {
        match recvfrom(fd, &mut resp_buf, None) {
            Ok(len) if len > 0 => {
                resp_len = len;
                received = true;
                break;
            }
            _ => {
                // Check timeout
                let now = now_monotonic();
                if now.tv_sec as u64 >= deadline_secs {
                    break; // Timeout
                }
                // Yield to scheduler - allows timer interrupt to fire and process softirqs
                yield_now();
            }
        }
    }

    let _ = close(fd as u64);

    if !received {
        return Err(DnsError::Timeout);
    }

    // Parse response
    let response = parse_response(&resp_buf[..resp_len]).ok_or(DnsError::ParseError)?;

    // Verify transaction ID matches
    if response.id != txid {
        return Err(DnsError::ParseError);
    }

    // Check for server errors
    if response.rcode != 0 {
        return Err(DnsError::ServerError(response.rcode));
    }

    // Find first A record
    for i in 0..response.answer_count {
        let answer = &response.answers[i];
        if answer.rtype == TYPE_A && answer.rdlength == 4 {
            return Ok(DnsResult {
                addr: answer.ipv4,
                ttl: answer.ttl,
            });
        }
    }

    Err(DnsError::NoAddress)
}

/// Convenience function using QEMU SLIRP's DNS server
pub fn resolve_slirp(hostname: &str) -> Result<DnsResult, DnsError> {
    resolve(hostname, SLIRP_DNS)
}
