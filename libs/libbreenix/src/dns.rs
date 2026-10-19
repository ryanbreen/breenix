//! DNS client library for Breenix
//!
//! Provides hostname resolution using UDP queries to DNS servers.
//!
//! # Example
//!
//! This example uses the Breenix syscall ABI and is compile-checked only; run it inside Breenix.
//!
//! ```rust,no_run
//! use libbreenix::dns::{resolve, SLIRP_DNS};
//!
//! // Resolve hostname using QEMU's DNS server
//! match resolve("www.google.com", SLIRP_DNS) {
//!     Ok(result) => println!("IP: {:?}", result.addr),
//!     Err(e) => println!("DNS error: {:?}", e),
//! }
//! ```

use crate::error::Error;
use crate::io::{poll, PollFd};
use crate::socket::{
    bind_inet, recvfrom, sendto, socket, SockAddrIn, AF_INET, SOCK_CLOEXEC, SOCK_DGRAM,
    SOCK_NONBLOCK,
};
use crate::syscall::{nr, raw};
use crate::time::now_monotonic;
use crate::types::Fd;
#[cfg(feature = "std")]
use crate::types::Timespec;

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

/// Independent budget for automatic resolution, including configuration discovery.
pub const RESOLUTION_DEADLINE_MS: u64 = 20_000;

/// QEMU SLIRP's built-in DNS server
pub const SLIRP_DNS: [u8; 4] = [10, 0, 2, 3];

/// Parallels Desktop shared networking DNS/gateway
pub const PARALLELS_DNS: [u8; 4] = [10, 211, 55, 1];

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
    /// The complete automatic resolution exhausted its own phase budget.
    ResolutionDeadline,
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

impl From<Error> for DnsError {
    fn from(_e: Error) -> Self {
        // Map OS errors to the closest DnsError variant.
        // Since DnsError predates the unified Error type, we map generically.
        DnsError::SocketError
    }
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

/// Draw transaction IDs from the pool without waiting for credited entropy at boot.
/// IDs may be predictable before the pool is ready.
fn generate_txid() -> Result<u16, DnsError> {
    let mut bytes = [0; 2];
    let mut used = 0;
    while used < bytes.len() {
        let n = unsafe {
            raw::syscall3(
                nr::GETRANDOM,
                bytes[used..].as_mut_ptr() as u64,
                (bytes.len() - used) as u64,
                crate::syscall::GRND_INSECURE as u64,
            )
        } as i64;
        if n <= 0 || n as usize > bytes.len() - used {
            return Err(DnsError::SocketError);
        }
        used += n as usize;
    }
    Ok(u16::from_ne_bytes(bytes))
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
/// This example uses the Breenix syscall ABI and is compile-checked only; run it inside Breenix.
///
/// ```rust,no_run
/// use libbreenix::dns::{resolve, SLIRP_DNS};
/// # fn main() -> Result<(), libbreenix::dns::DnsError> {
///
/// let result = resolve("example.com", SLIRP_DNS)?;
/// println!("IP: {}.{}.{}.{}", result.addr[0], result.addr[1], result.addr[2], result.addr[3]);
/// # Ok(())
/// # }
/// ```
/// All bounded A results and the final CNAME target from one validated reply.
pub struct DnsLookup {
    pub answers: [DnsResult; MAX_ANSWERS],
    pub len: usize,
    pub canonical: [u8; 256],
    pub canonical_len: usize,
}

// Decode compressed names with a strict hop and output bound, including backward pointers.
fn decode_name(buf: &[u8], mut pos: usize) -> Option<([u8; 256], usize, usize)> {
    let mut name = [0; 256];
    let mut used = 0;
    let mut end = None;
    for _ in 0..128 {
        let n = *buf.get(pos)? as usize;
        if n == 0 {
            return Some((name, used, end.unwrap_or(pos + 1)));
        }
        if n & 0xc0 == 0xc0 {
            let target = ((n & 0x3f) << 8) | *buf.get(pos + 1)? as usize;
            if target >= pos {
                return None;
            }
            end.get_or_insert(pos + 2);
            pos = target;
            continue;
        }
        if n > 63 {
            return None;
        }
        if used != 0 {
            name[used] = b'.';
            used += 1;
        }
        if used + n > 255 {
            return None;
        }
        name[used..used + n].copy_from_slice(buf.get(pos + 1..pos + 1 + n)?);
        used += n as usize;
        pos += n + 1;
    }
    None
}

fn expected_source(source: &SockAddrIn, server: [u8; 4]) -> bool {
    source.family == AF_INET as u16 && source.addr == server && source.port == DNS_PORT.to_be()
}

fn checked_reply(buf: &[u8], id: u16, hostname: &str) -> Option<Result<DnsLookup, DnsError>> {
    let header = DnsHeader::from_bytes(buf)?;
    if header.id != id || !header.is_response() || header.flags & 0x7a00 != 0 || header.qdcount != 1
    {
        return None;
    }
    let (question, qlen, mut at) = decode_name(buf, 12)?;
    if !question[..qlen].eq_ignore_ascii_case(hostname.as_bytes())
        || buf.get(at..at + 4)? != [0, 1, 0, 1]
    {
        return None;
    }
    at += 4;
    if header.rcode() != 0 {
        return Some(Err(DnsError::ServerError(header.rcode())));
    }
    // Keep the complete answer section bounded. Never use unrelated additional A records.
    if header.ancount as usize > MAX_ANSWERS {
        return Some(Err(DnsError::ParseError));
    }
    let mut owners = [[0; 256]; MAX_ANSWERS];
    let mut owner_lens = [0; MAX_ANSWERS];
    let mut targets = [[0; 256]; MAX_ANSWERS];
    let mut target_lens = [0; MAX_ANSWERS];
    let mut records = [DnsAnswer::empty(); MAX_ANSWERS];
    for i in 0..header.ancount as usize {
        let (owner, len, next) = decode_name(buf, at)?;
        owners[i] = owner;
        owner_lens[i] = len;
        let rr = buf.get(next..next + 10)?;
        let kind = u16::from_be_bytes([rr[0], rr[1]]);
        let class = u16::from_be_bytes([rr[2], rr[3]]);
        let ttl = u32::from_be_bytes([rr[4], rr[5], rr[6], rr[7]]);
        let size = u16::from_be_bytes([rr[8], rr[9]]) as usize;
        at = next + 10;
        let data = buf.get(at..at + size)?;
        records[i] = DnsAnswer {
            rtype: kind,
            rclass: class,
            ttl,
            rdlength: size as u16,
            ipv4: if kind == TYPE_A && size == 4 {
                [data[0], data[1], data[2], data[3]]
            } else {
                [0; 4]
            },
        };
        if kind == TYPE_CNAME && class == CLASS_IN {
            let (target, len, end) = decode_name(buf, at)?;
            if end != at + size {
                return None;
            }
            targets[i] = target;
            target_lens[i] = len;
        }
        at += size;
    }
    let mut canonical = question;
    let mut canonical_len = qlen;
    for hop in 0..=MAX_ANSWERS {
        let cname = (0..header.ancount as usize).find(|&i| {
            records[i].rtype == TYPE_CNAME
                && records[i].rclass == CLASS_IN
                && owners[i][..owner_lens[i]].eq_ignore_ascii_case(&canonical[..canonical_len])
        });
        let Some(i) = cname else {
            break;
        };
        if hop == MAX_ANSWERS {
            return None;
        }
        canonical = targets[i];
        canonical_len = target_lens[i];
    }
    let mut result = DnsLookup {
        answers: [DnsResult {
            addr: [0; 4],
            ttl: 0,
        }; MAX_ANSWERS],
        len: 0,
        canonical,
        canonical_len,
    };
    for i in 0..header.ancount as usize {
        if records[i].rtype == TYPE_A
            && records[i].rclass == CLASS_IN
            && records[i].rdlength == 4
            && owners[i][..owner_lens[i]].eq_ignore_ascii_case(&canonical[..canonical_len])
        {
            result.answers[result.len] = DnsResult {
                addr: records[i].ipv4,
                ttl: records[i].ttl,
            };
            result.len += 1;
        }
    }
    Some(if result.len == 0 {
        Err(DnsError::NoAddress)
    } else {
        Ok(result)
    })
}

pub fn resolve(hostname: &str, dns_server: [u8; 4]) -> Result<DnsResult, DnsError> {
    resolve_all(hostname, dns_server).map(|reply| reply.answers[0])
}

pub fn resolve_all(hostname: &str, dns_server: [u8; 4]) -> Result<DnsLookup, DnsError> {
    resolve_all_until(hostname, dns_server, u64::MAX)
}

fn monotonic_ms() -> Result<u64, DnsError> {
    now_monotonic()
        .map(|t| (t.tv_sec.max(0) as u64).saturating_mul(1000)
            .saturating_add(t.tv_nsec.max(0) as u64 / 1_000_000))
        .map_err(|_| DnsError::RecvError)
}

fn check_resolution_deadline(deadline: u64) -> Result<(), DnsError> {
    if monotonic_ms()? >= deadline {
        Err(DnsError::ResolutionDeadline)
    } else {
        Ok(())
    }
}

fn resolve_all_until(hostname: &str, dns_server: [u8; 4], resolution_deadline: u64)
    -> Result<DnsLookup, DnsError> {
    if hostname.is_empty() {
        return Err(DnsError::InvalidHostname);
    }
    if hostname.len() > MAX_HOSTNAME_LEN {
        return Err(DnsError::HostnameTooLong);
    }
    let txid = generate_txid()?;
    let mut query = [0; DNS_BUF_SIZE];
    let query_len = encode_query(hostname, txid, &mut query);
    if query_len == 0 {
        return Err(DnsError::InvalidHostname);
    }
    let fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0)
        .map_err(|_| DnsError::SocketError)?;
    let result = (|| {
        bind_inet(fd, &SockAddrIn::new([0; 4], 0)).map_err(|_| DnsError::BindError)?;
        let server = SockAddrIn::new(dns_server, DNS_PORT);
        let mut buf = [0; DNS_BUF_SIZE];
        for _ in 0..3 {
            check_resolution_deadline(resolution_deadline)?;
            sendto(fd, &query[..query_len], &server).map_err(|_| DnsError::SendError)?;
            let deadline = monotonic_ms()?.saturating_add(500).min(resolution_deadline);
            loop {
                check_resolution_deadline(resolution_deadline)?;
                let remaining = deadline.saturating_sub(monotonic_ms()?);
                if remaining == 0 {
                    break;
                }
                let mut ready = [PollFd::new(fd, 1)];
                match poll(&mut ready, remaining as i32) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(Error::Os(e)) if e == crate::errno::Errno::EINTR => continue,
                    Err(_) => return Err(DnsError::RecvError),
                }
                let mut source = SockAddrIn::default();
                let n = match recvfrom(fd, &mut buf, Some(&mut source)) {
                    Ok(n) => n,
                    Err(Error::Os(e))
                        if e == crate::errno::Errno::EAGAIN || e == crate::errno::Errno::EINTR =>
                    {
                        continue
                    }
                    Err(_) => return Err(DnsError::RecvError),
                };
                if !expected_source(&source, dns_server) {
                    continue;
                }
                if let Some(reply) = checked_reply(&buf[..n], txid, hostname) {
                    return reply;
                }
            }
        }
        Err(DnsError::Timeout)
    })();
    close_fd(fd);
    result
}

/// The first nameserver the kernel's boot-time DHCP lease named, read from
/// `/proc/net/pnp` (Linux ipconfig's format, one `nameserver a.b.c.d` per line).
/// `None` when no lease configured the interface.
pub fn dhcp_nameserver() -> Option<[u8; 4]> {
    dhcp_configuration().ok()?.1.into_iter().flatten().next()
}

/// DHCP management remains authoritative while discovery or renewal is pending.
/// Static virtual-network fallbacks are only for interfaces without DHCP policy.
fn dhcp_configuration() -> Result<(bool, [Option<[u8; 4]>; 3]), DnsError> {
    let fd = match crate::fs::open("/proc/net/pnp", crate::fs::O_RDONLY) {
        Ok(fd) => fd,
        Err(crate::error::Error::Os(crate::errno::Errno::ENOENT)) => return Ok((false, [None; 3])),
        Err(_) => return Err(DnsError::RecvError),
    };
    let mut buf = [0u8; 256];
    let read = crate::fs::read(fd, &mut buf);
    close_fd(fd);
    let size = read.map_err(|_| DnsError::RecvError)?;
    let text = core::str::from_utf8(&buf[..size]).map_err(|_| DnsError::ParseError)?;
    Ok(parse_dhcp_configuration(text))
}

fn parse_dhcp_configuration(text: &str) -> (bool, [Option<[u8; 4]>; 3]) {
    let mut servers = [None; 3];
    let required = text.lines().any(|line| line == "#PROTO: DHCP");
    for (slot, server) in servers.iter_mut().zip(text.lines().filter_map(|line|
        parse_ipv4(line.strip_prefix("nameserver ")?.trim()))) {
        *slot = Some(server);
    }
    (required, servers)
}

fn parse_ipv4(text: &str) -> Option<[u8; 4]> {
    let mut address = [0u8; 4];
    let mut octets = text.split('.');
    for octet in &mut address {
        *octet = octets.next()?.parse().ok()?;
    }
    octets.next().is_none().then_some(address)
}

fn resolver_candidates(dhcp_required: bool, lease_servers: [Option<[u8; 4]>; 3])
    -> [Option<([u8; 4], &'static str)>; 6] {
    [
        lease_servers[0].map(|server| (server, "the DHCP nameserver")),
        lease_servers[1].map(|server| (server, "the DHCP nameserver")),
        lease_servers[2].map(|server| (server, "the DHCP nameserver")),
        (!dhcp_required).then_some((GOOGLE_DNS, "8.8.8.8")),
        (!dhcp_required).then_some((PARALLELS_DNS, "10.211.55.1")),
        (!dhcp_required).then_some((SLIRP_DNS, "10.0.2.3")),
    ]
}

/// Resolve a hostname by trying multiple DNS servers automatically.
///
/// DHCP-managed interfaces use only their lease's nameservers. Other interfaces
/// try the established static DNS servers for their virtual network.
pub fn resolve_auto(hostname: &str) -> Result<DnsResult, DnsError> {
    let resolution_deadline = monotonic_ms()?.saturating_add(RESOLUTION_DEADLINE_MS);
    #[cfg(feature = "std")]
    let total_start = now_monotonic().unwrap_or(Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    });
    let configuration = dhcp_configuration();
    #[cfg(feature = "std")]
    eprintln!("[dns] configuration read ({}ms)", elapsed_ms(&total_start));
    check_resolution_deadline(resolution_deadline)?;
    let (required, lease_servers) = configuration?;
    let servers = resolver_candidates(required, lease_servers);

    let mut last_err = DnsError::Timeout;
    for (server, _name) in servers.iter().flatten() {
        #[cfg(feature = "std")]
        let attempt_start = now_monotonic().unwrap_or(Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        });

        check_resolution_deadline(resolution_deadline)?;
        let result = resolve_all_until(hostname, *server, resolution_deadline)
            .map(|reply| reply.answers[0]);
        check_resolution_deadline(resolution_deadline)?;
        match result {
            Ok(r) if r.addr[0] != 0 && r.addr[0] != 127 => {
                #[cfg(feature = "std")]
                {
                    let elapsed = elapsed_ms(&attempt_start);
                    let total = elapsed_ms(&total_start);
                    eprintln!(
                        "[dns] resolved '{}' via {} -> {}.{}.{}.{} ({}ms, total {}ms)",
                        hostname, _name, r.addr[0], r.addr[1], r.addr[2], r.addr[3], elapsed, total
                    );
                }
                return Ok(r);
            }
            Ok(_) => {
                #[cfg(feature = "std")]
                eprintln!(
                    "[dns] '{}' via {}: unusable address ({}ms)",
                    hostname,
                    _name,
                    elapsed_ms(&attempt_start)
                );
                continue;
            }
            Err(e) => {
                #[cfg(feature = "std")]
                eprintln!(
                    "[dns] '{}' via {}: {:?} ({}ms)",
                    hostname,
                    _name,
                    e,
                    elapsed_ms(&attempt_start)
                );
                if e == DnsError::ResolutionDeadline {
                    return Err(e);
                }
                last_err = e;
                continue;
            }
        }
    }
    #[cfg(feature = "std")]
    eprintln!(
        "[dns] '{}' FAILED all servers (total {}ms)",
        hostname,
        elapsed_ms(&total_start)
    );
    Err(last_err)
}

/// Compute elapsed milliseconds since a start time.
#[cfg(feature = "std")]
fn elapsed_ms(start: &Timespec) -> u64 {
    let now = now_monotonic().unwrap_or(Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    });
    let start_ms = start.tv_sec as u64 * 1000 + start.tv_nsec as u64 / 1_000_000;
    let now_ms = now.tv_sec as u64 * 1000 + now.tv_nsec as u64 / 1_000_000;
    now_ms.saturating_sub(start_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reply() -> ([u8; DNS_BUF_SIZE], usize) {
        let mut packet = [0; DNS_BUF_SIZE];
        let mut n = encode_query("alias.test", 0x1234, &mut packet);
        packet[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        packet[6..8].copy_from_slice(&3u16.to_be_bytes());
        // CNAME alias.test -> target.test, then two A records owned by target.test.
        packet[n..n + 12].copy_from_slice(&[0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 13]);
        n += 12;
        let target_at = n;
        n += encode_hostname("target.test", &mut packet[n..]);
        for ip in [[192, 0, 2, 1], [192, 0, 2, 2]] {
            packet[n..n + 12].copy_from_slice(&[
                0xc0,
                target_at as u8,
                0,
                1,
                0,
                1,
                0,
                0,
                0,
                60,
                0,
                4,
            ]);
            n += 12;
            packet[n..n + 4].copy_from_slice(&ip);
            n += 4;
        }
        (packet, n)
    }
    #[test]
    fn dhcp_policy_keeps_discovery_and_lease_dns_off_virtual_fallbacks() {
        let (required, addresses) = parse_dhcp_configuration("#PROTO: DHCP\n");
        assert!(resolver_candidates(required, addresses).iter().all(Option::is_none));
        let (required, addresses) = parse_dhcp_configuration(
            "#PROTO: DHCP\nnameserver 192.0.2.53\nnameserver 192.0.2.54\n");
        let chosen = resolver_candidates(required, addresses).map(|entry| entry.map(|(address, _)| address));
        assert_eq!(chosen, [Some([192, 0, 2, 53]), Some([192, 0, 2, 54]), None, None, None, None]);
        let (required, addresses) = parse_dhcp_configuration("#MANUAL\n");
        assert_eq!(resolver_candidates(required, addresses).into_iter().flatten().count(), 3);
    }

    #[test]
    fn rejects_wrong_source_and_unrelated_answer_owner() {
        let server = [192, 0, 2, 53];
        assert!(expected_source(&SockAddrIn::new(server, 53), server));
        assert!(!expected_source(
            &SockAddrIn::new([192, 0, 2, 54], 53),
            server
        ));
        assert!(!expected_source(&SockAddrIn::new(server, 54), server));
        let (mut packet, n) = reply();
        let owner = n - 32;
        // Both A owners now refer to alias.test, which the CNAME has replaced.
        packet[owner + 1] = 12;
        packet[owner + 17] = 12;
        assert!(matches!(
            checked_reply(&packet[..n], 0x1234, "alias.test"),
            Some(Err(DnsError::NoAddress))
        ));
    }
    #[test]
    fn validates_question_transaction_and_cname_owned_answers() {
        let (packet, n) = reply();
        assert!(checked_reply(&packet[..n], 0x5678, "alias.test").is_none());
        assert!(checked_reply(&packet[..n], 0x1234, "other.test").is_none());
        let result = checked_reply(&packet[..n], 0x1234, "alias.test")
            .unwrap()
            .unwrap();
        assert_eq!(result.len, 2);
        assert_eq!(&result.canonical[..result.canonical_len], b"target.test");
        assert_eq!(result.answers[0].addr, [192, 0, 2, 1]);
        assert_eq!(result.answers[1].addr, [192, 0, 2, 2]);
    }
    #[test]
    fn rejects_truncated_questions_and_compression_cycles() {
        let (mut packet, n) = reply();
        for end in 0..25 {
            assert!(checked_reply(&packet[..end], 0x1234, "alias.test").is_none());
        }
        packet[12..14].copy_from_slice(&[0xc0, 12]);
        assert!(checked_reply(&packet[..n], 0x1234, "alias.test").is_none());
    }
}
