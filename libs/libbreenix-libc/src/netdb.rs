//! Clean-room IPv4 name and service translation (POSIX, RFC 3493 and RFC 6761).
//! EAI/AI/NI values and addrinfo layout follow the Linux ABI.

use crate::inet::{c_bytes, decimal, format_ipv4, parse_inet_addr, parse_ipv4, AF_INET};
use core::{mem, ptr};
use libbreenix::errno::Errno;
const O_CLOEXEC: i32 = 0x80000;

const EAI_BADFLAGS: i32 = -1;
const EAI_NONAME: i32 = -2;
// Internal distinction: getaddrinfo maps this to NONAME; hostent uses NO_DATA.
const NO_ADDRESS: i32 = -100;
const EAI_AGAIN: i32 = -3;
const EAI_FAIL: i32 = -4;
const EAI_FAMILY: i32 = -6;
const EAI_SOCKTYPE: i32 = -7;
const EAI_SERVICE: i32 = -8;
const EAI_MEMORY: i32 = -10;
const EAI_SYSTEM: i32 = -11;
const EAI_OVERFLOW: i32 = -12;
const AI_PASSIVE: i32 = 1;
const AI_CANONNAME: i32 = 2;
const AI_NUMERICHOST: i32 = 4;
const AI_NUMERICSERV: i32 = 0x400;
const AI_KNOWN: i32 = 0x3f | AI_NUMERICSERV;
const NI_NUMERICHOST: i32 = 1;
const NI_NUMERICSERV: i32 = 2;
const NI_NOFQDN: i32 = 4;
const NI_NAMEREQD: i32 = 8;
const NI_DGRAM: i32 = 16;

#[repr(C)]
pub struct AddrInfo {
    pub flags: i32,
    pub family: i32,
    pub socktype: i32,
    pub protocol: i32,
    pub addrlen: u32,
    pub addr: *mut u8,
    pub canonname: *mut u8,
    pub next: *mut AddrInfo,
}

#[repr(C)]
struct Entry {
    info: AddrInfo,
    addr: [u8; 16],
    name: [u8; 256],
}

#[derive(Clone, Copy)]
struct Name {
    bytes: [u8; 256],
    len: usize,
}
impl Name {
    fn new(text: &[u8]) -> Result<Self, i32> {
        if text.len() > 255 {
            return Err(EAI_NONAME);
        }
        let mut name = Self {
            bytes: [0; 256],
            len: text.len(),
        };
        name.bytes[..text.len()].copy_from_slice(text);
        Ok(name)
    }
    fn text(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Read configuration as bounded lines, without caching files applications can edit.
/// Oversized lines are discarded whole, never parsed as partial records.
fn lines(path: &[u8], mut visit: impl FnMut(&[u8]) -> Result<(), i32>) -> Result<(), i32> {
    unsafe {
        let fd = crate::open(path.as_ptr(), O_CLOEXEC, 0);
        if fd < 0 {
            return if matches!(*crate::__errno_location(), e if e == Errno::ENOENT as i32 || e == Errno::EACCES as i32)
            {
                Ok(())
            } else {
                Err(EAI_SYSTEM)
            };
        }
        let mut chunk = [0; 512];
        let mut line = [0; 1024];
        let mut used = 0;
        let mut oversized = false;
        let result = (|| loop {
            let n = crate::read(fd, chunk.as_mut_ptr(), chunk.len());
            if n < 0 {
                if *crate::__errno_location() == Errno::EINTR as i32 {
                    continue;
                }
                return Err(EAI_SYSTEM);
            }
            if n == 0 {
                if used != 0 && !oversized {
                    visit(&line[..used])?;
                }
                return Ok(());
            }
            for b in &chunk[..n as usize] {
                if *b == b'\n' {
                    if !oversized {
                        visit(&line[..used])?;
                    }
                    used = 0;
                    oversized = false;
                } else if used < line.len() {
                    line[used] = *b;
                    used += 1;
                } else {
                    oversized = true;
                }
            }
        })();
        let saved_errno = *crate::__errno_location();
        crate::close(fd);
        *crate::__errno_location() = saved_errno;
        result
    }
}

fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    let end = line.iter().position(|b| *b == b'#').unwrap_or(line.len());
    line[..end]
        .split(|b| b.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
}
fn equal_name(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}
fn reserved(name: &[u8], suffix: &[u8]) -> bool {
    let name = name.strip_suffix(b".").unwrap_or(name);
    equal_name(name, suffix)
        || (name.len() > suffix.len()
            && name[name.len() - suffix.len() - 1] == b'.'
            && equal_name(&name[name.len() - suffix.len()..], suffix))
}

struct Addresses {
    bytes: [[u8; 4]; 64],
    len: usize,
    canon: Name,
}
impl Addresses {
    fn new(canon: &[u8]) -> Result<Self, i32> {
        Ok(Self {
            bytes: [[0; 4]; 64],
            len: 0,
            canon: Name::new(canon)?,
        })
    }
    fn add(&mut self, addr: [u8; 4]) -> Result<(), i32> {
        if self.bytes[..self.len].contains(&addr) {
            return Ok(());
        }
        if self.len == self.bytes.len() {
            return Err(EAI_MEMORY);
        }
        self.bytes[self.len] = addr;
        self.len += 1;
        Ok(())
    }
}

fn hosts_line(line: &[u8]) -> Option<([u8; 4], &[u8])> {
    let mut f = fields(line);
    Some((parse_ipv4(f.next()?)?, f.next()?))
}
fn hosts_forward(name: &[u8], out: &mut Addresses) -> Result<(), i32> {
    lines(b"/etc/hosts\0", |line| {
        if let Some((addr, canon)) = hosts_line(line) {
            if fields(line).skip(1).any(|alias| equal_name(alias, name)) {
                if out.len == 0 {
                    if let Ok(valid) = Name::new(canon) {
                        out.canon = valid;
                    }
                }
                out.add(addr)?;
            }
        }
        Ok(())
    })
}
fn hosts_reverse(addr: [u8; 4]) -> Result<Option<Name>, i32> {
    let mut found = None;
    lines(b"/etc/hosts\0", |line| {
        if found.is_none() {
            if let Some((ip, canon)) = hosts_line(line) {
                if ip == addr {
                    found = Name::new(canon).ok();
                }
            }
        }
        Ok(())
    })?;
    Ok(found)
}

fn dns_lookup(name: &[u8], out: &mut Addresses) -> Result<(), i32> {
    let hostname = core::str::from_utf8(name).map_err(|_| EAI_NONAME)?;
    let hostname = hostname.strip_suffix('.').unwrap_or(hostname);
    let mut servers = [[0; 4]; 3];
    let mut count = 0;
    lines(b"/etc/resolv.conf\0", |line| {
        let mut f = fields(line);
        if f.next() == Some(b"nameserver".as_slice()) {
            if let Some(addr) = f.next().and_then(parse_ipv4) {
                if count < servers.len() {
                    servers[count] = addr;
                    count += 1;
                }
            }
        }
        Ok(())
    })?;
    if count == 0 {
        servers[0] = [127, 0, 0, 1];
        count = 1;
    }
    let mut failure = EAI_AGAIN;
    for server in &servers[..count] {
        use libbreenix::dns::DnsError;
        match libbreenix::dns::resolve_all(hostname, *server) {
            Ok(answer) => {
                for record in &answer.answers[..answer.len] {
                    out.add(record.addr)?;
                }
                out.canon = Name::new(&answer.canonical[..answer.canonical_len])?;
                return Ok(());
            }
            Err(
                DnsError::ServerError(3)
                | DnsError::InvalidHostname
                | DnsError::HostnameTooLong,
            ) => return Err(EAI_NONAME),
            Err(DnsError::NoAddress) => return Err(NO_ADDRESS),
            Err(DnsError::Timeout | DnsError::ResolutionDeadline | DnsError::ServerError(2)) => failure = EAI_AGAIN,
            Err(
                DnsError::SocketError
                | DnsError::BindError
                | DnsError::SendError
                | DnsError::RecvError,
            ) => failure = EAI_AGAIN,
            Err(DnsError::ParseError | DnsError::ServerError(_)) => failure = EAI_FAIL,
        }
    }
    Err(failure)
}

fn node_addresses(node: Option<&[u8]>, flags: i32) -> Result<Addresses, i32> {
    let mut out = Addresses::new(node.unwrap_or(b""))?;
    match node {
        None => out.add(if flags & AI_PASSIVE != 0 {
            [0; 4]
        } else {
            [127, 0, 0, 1]
        })?,
        Some(name) => {
            if let Some(addr) = parse_inet_addr(name) {
                out.add(addr)?;
            } else {
                if flags & AI_NUMERICHOST != 0 || name.is_empty() || reserved(name, b"invalid") {
                    return Err(EAI_NONAME);
                }
                hosts_forward(name, &mut out)?;
                if out.len == 0 {
                    if reserved(name, b"localhost") {
                        out.canon = Name::new(b"localhost")?;
                        out.add([127, 0, 0, 1])?;
                    } else {
                        dns_lookup(name, &mut out)?;
                    }
                }
            }
        }
    }
    Ok(out)
}

fn numeric_port(text: &[u8]) -> Option<u16> {
    if text.is_empty() {
        return None;
    }
    let mut port = 0u16;
    for b in text {
        if !b.is_ascii_digit() {
            return None;
        }
        port = port.checked_mul(10)?.checked_add((b - b'0') as u16)?;
    }
    Some(port)
}
// Snapshot both protocols in one read, independent of the number of addresses.
fn service_ports(name: &[u8]) -> Result<[Option<u16>; 3], i32> {
    let mut found = [None; 3];
    lines(b"/etc/services\0", |line| {
        let mut f = fields(line);
        let Some(canon) = f.next() else {
            return Ok(());
        };
        let Some(spec) = f.next() else {
            return Ok(());
        };
        let mut parts = spec.split(|b| *b == b'/');
        let Some(port) = parts.next().and_then(numeric_port) else {
            return Ok(());
        };
        let index = match parts.next() {
            Some(b"tcp") => 0,
            Some(b"udp") => 1,
            Some(b"sctp") => 2,
            _ => return Ok(()),
        };
        if parts.next().is_none()
            && found[index].is_none()
            && (canon == name || f.any(|a| a == name))
        {
            found[index] = Some(port);
        }
        Ok(())
    })?;
    Ok(found)
}
fn service_name(port: u16, protocol: i32) -> Result<Option<Name>, i32> {
    let mut found = None;
    lines(b"/etc/services\0", |line| {
        let mut f = fields(line);
        let Some(canon) = f.next() else {
            return Ok(());
        };
        let Some(spec) = f.next() else {
            return Ok(());
        };
        let mut parts = spec.split(|b| *b == b'/');
        if parts.next().and_then(numeric_port) == Some(port)
            && parts.next() == Some(if protocol == 17 { b"udp" } else { b"tcp" })
            && found.is_none()
        {
            found = Name::new(canon).ok();
        }
        Ok(())
    })?;
    Ok(found)
}

#[no_mangle]
pub unsafe extern "C" fn freeaddrinfo(mut info: *mut AddrInfo) {
    while !info.is_null() {
        let next = (*info).next;
        crate::free(info as *mut u8);
        info = next;
    }
}

unsafe fn lookup(
    node: *const u8,
    service: *const u8,
    hints: *const AddrInfo,
) -> Result<*mut AddrInfo, i32> {
    let (flags, family, kind, protocol) = if hints.is_null() {
        (0, 0, 0, 0)
    } else {
        (
            (*hints).flags,
            (*hints).family,
            (*hints).socktype,
            (*hints).protocol,
        )
    };
    if node.is_null() && service.is_null() {
        return Err(EAI_NONAME);
    }
    if flags & !AI_KNOWN != 0 || (flags & AI_CANONNAME != 0 && node.is_null()) {
        return Err(EAI_BADFLAGS);
    }
    if family != 0 && family != AF_INET {
        return Err(EAI_FAMILY);
    }
    if kind != 0 && kind != 1 && kind != 2 && kind != 5 {
        return Err(EAI_SOCKTYPE);
    }
    if protocol != 0 && protocol != 6 && protocol != 17 && protocol != 132 {
        return Err(if kind != 0 { EAI_SOCKTYPE } else { EAI_SERVICE });
    }
    if (kind == 1 && protocol == 17)
        || (kind == 2 && protocol != 0 && protocol != 17)
        || (kind == 5 && protocol != 0 && protocol != 132)
    {
        return Err(EAI_SOCKTYPE);
    }
    let node = if node.is_null() {
        None
    } else {
        Some(c_bytes(node, 255).ok_or(EAI_NONAME)?)
    };
    let service = if service.is_null() {
        None
    } else {
        Some(c_bytes(service, 255).ok_or(EAI_SERVICE)?)
    };
    let numeric = service.and_then(numeric_port);
    if flags & AI_NUMERICSERV != 0 && service.is_some() && numeric.is_none() {
        return Err(EAI_NONAME);
    }
    let ports = match (service, numeric) {
        (Some(name), None) => service_ports(name)?,
        _ => [numeric; 3],
    };
    let addresses = node_addresses(node, flags).map_err(|e| if e == NO_ADDRESS { EAI_NONAME } else { e })?;
    let mut head: *mut AddrInfo = ptr::null_mut();
    let mut tail: *mut AddrInfo = ptr::null_mut();
    for addr in &addresses.bytes[..addresses.len] {
        for (ty, proto) in [(1, 6), (2, 17), (1, 132), (5, 132)] {
            if protocol == 0 && kind != 5 && proto == 132 {
                continue;
            }
            if (kind != 0 && kind != ty) || (protocol != 0 && protocol != proto) {
                continue;
            }
            let port = if service.is_none() {
                0
            } else {
                match ports[match proto {
                    6 => 0,
                    17 => 1,
                    _ => 2,
                }] {
                    Some(port) => port,
                    None => continue,
                }
            };
            let entry = crate::calloc(1, mem::size_of::<Entry>()) as *mut Entry;
            if entry.is_null() {
                freeaddrinfo(head);
                return Err(EAI_MEMORY);
            }
            let entry = &mut *entry;
            entry.addr[..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
            entry.addr[2..4].copy_from_slice(&port.to_be_bytes());
            entry.addr[4..8].copy_from_slice(addr);
            entry.info = AddrInfo {
                flags,
                family: AF_INET,
                socktype: ty,
                protocol: proto,
                addrlen: 16,
                addr: entry.addr.as_mut_ptr(),
                canonname: ptr::null_mut(),
                next: ptr::null_mut(),
            };
            if flags & AI_CANONNAME != 0 && head.is_null() {
                entry.name = addresses.canon.bytes;
                entry.info.canonname = entry.name.as_mut_ptr();
            }
            let info = &mut entry.info as *mut AddrInfo;
            if tail.is_null() {
                head = info;
            } else {
                (*tail).next = info;
            }
            tail = info;
        }
    }
    if head.is_null() {
        Err(EAI_SERVICE)
    } else {
        Ok(head)
    }
}

#[no_mangle]
pub unsafe extern "C" fn getaddrinfo(
    node: *const u8,
    service: *const u8,
    hints: *const AddrInfo,
    result: *mut *mut AddrInfo,
) -> i32 {
    if result.is_null() {
        *crate::__errno_location() = Errno::EFAULT as i32;
        return EAI_SYSTEM;
    }
    *result = ptr::null_mut();
    match lookup(node, service, hints) {
        Ok(info) => {
            *result = info;
            0
        }
        Err(e) => e,
    }
}

unsafe fn output(text: &[u8], dest: *mut u8, cap: u32) -> Result<(), i32> {
    if text.len() >= cap as usize {
        return Err(EAI_OVERFLOW);
    }
    ptr::copy_nonoverlapping(text.as_ptr(), dest, text.len());
    *dest.add(text.len()) = 0;
    Ok(())
}
/// NI_NOFQDN only shortens names in the machine's local DNS domain.
unsafe fn local_name_end(text: &[u8]) -> usize {
    let mut identity = mem::zeroed::<crate::utsname>();
    let saved_errno = *crate::__errno_location();
    let status = crate::uname(&mut identity);
    *crate::__errno_location() = saved_errno;
    if status != 0 {
        return text.len();
    }
    let hostname = &identity.nodename;
    let end = hostname
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(hostname.len());
    let hostname = &hostname[..end];
    if let Some(dot) = hostname.iter().position(|b| *b == b'.') {
        let suffix = &hostname[dot..];
        if suffix.len() > 1
            && text.len() > suffix.len()
            && equal_name(&text[text.len() - suffix.len()..], suffix)
        {
            return text.len() - suffix.len();
        }
    }
    text.len()
}

unsafe fn reverse(
    addr: *const u8,
    addrlen: u32,
    host: *mut u8,
    hostlen: u32,
    service: *mut u8,
    servicelen: u32,
    flags: i32,
) -> Result<(), i32> {
    if flags & !31 != 0 {
        return Err(EAI_BADFLAGS);
    }
    if addr.is_null() || addrlen < 16 || ptr::read_unaligned(addr as *const u16) != AF_INET as u16 {
        return Err(EAI_FAMILY);
    }
    let want_host = !host.is_null() && hostlen != 0;
    let want_service = !service.is_null() && servicelen != 0;
    if !want_host && !want_service {
        return Err(EAI_NONAME);
    }
    let ip = ptr::read_unaligned(addr.add(4) as *const [u8; 4]);
    let port = u16::from_be_bytes([*addr.add(2), *addr.add(3)]);
    if want_host {
        let name = if flags & NI_NUMERICHOST == 0 {
            hosts_reverse(ip)?
        } else {
            None
        };
        if let Some(name) = name {
            let text = name.text();
            let end = if flags & NI_NOFQDN != 0 {
                local_name_end(text)
            } else {
                text.len()
            };
            output(&text[..end], host, hostlen)?;
        } else {
            if flags & NI_NAMEREQD != 0 {
                return Err(EAI_NONAME);
            }
            let mut text = [0; 16];
            let n = format_ipv4(ip, &mut text);
            output(&text[..n], host, hostlen)?;
        }
    }
    if want_service {
        let name = if flags & NI_NUMERICSERV == 0 {
            service_name(port, if flags & NI_DGRAM != 0 { 17 } else { 6 })?
        } else {
            None
        };
        if let Some(name) = name {
            output(name.text(), service, servicelen)?;
        } else {
            let mut text = [0; 5];
            output(decimal(port, &mut text), service, servicelen)?;
        }
    }
    Ok(())
}

#[no_mangle]
pub unsafe extern "C" fn getnameinfo(
    addr: *const u8,
    addrlen: u32,
    host: *mut u8,
    hostlen: u32,
    service: *mut u8,
    servicelen: u32,
    flags: i32,
) -> i32 {
    match reverse(addr, addrlen, host, hostlen, service, servicelen, flags) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

#[no_mangle]
pub extern "C" fn gai_strerror(code: i32) -> *const u8 {
    let text: &[u8] = match code {
        0 => b"Success\0",
        EAI_BADFLAGS => b"Invalid resolver flags\0",
        EAI_NONAME => b"Name or service not known\0",
        EAI_AGAIN => b"Temporary name resolution failure\0",
        EAI_FAIL => b"Name resolution failure\0",
        EAI_FAMILY => b"Unsupported address family\0",
        EAI_SOCKTYPE => b"Unsupported socket type\0",
        EAI_SERVICE => b"Service unavailable for socket type\0",
        EAI_MEMORY => b"Resolver allocation failed\0",
        EAI_SYSTEM => b"System error\0",
        EAI_OVERFLOW => b"Resolver output buffer too small\0",
        _ => b"Unknown resolver error\0",
    };
    text.as_ptr()
}

// gethostbyname is the obsolete, non-reentrant hostent interface. Its result is
// overwritten by the next call; h_errno itself is per thread. Resolution shares
// the existing bounded IPv4 resolver rather than introducing another DNS path.
#[repr(C)]
pub struct HostEnt {
    pub name: *mut u8,
    pub aliases: *mut *mut u8,
    pub addrtype: i32,
    pub length: i32,
    pub addr_list: *mut *mut u8,
}
struct HostStorage {
    entry: HostEnt,
    name: [u8; 256],
    alias_names: [[u8; 256]; 64],
    aliases: [*mut u8; 65],
    addresses: [[u8; 4]; 64],
    pointers: [*mut u8; 65],
}
static mut HOST: HostStorage = HostStorage {
    entry: HostEnt { name: ptr::null_mut(), aliases: ptr::null_mut(), addrtype: AF_INET,
                     length: 4, addr_list: ptr::null_mut() },
    name: [0; 256], alias_names: [[0; 256]; 64], aliases: [ptr::null_mut(); 65], addresses: [[0; 4]; 64],
    pointers: [ptr::null_mut(); 65],
};
#[no_mangle]
pub unsafe extern "C" fn gethostbyname(name: *const u8) -> *mut HostEnt {
    let saved_errno = *crate::__errno_location();
    let error = crate::pthread::__h_errno_location();
    let mut alias_names = [[0u8; 256]; 64];
    let mut alias_count = 0;
    let result = if name.is_null() { Err(EAI_NONAME) }
                 else { c_bytes(name, 256).ok_or(EAI_NONAME).and_then(|text| {
                     let addresses = node_addresses(Some(text), 0)?;
                     let mut add_alias = |alias: &[u8]| -> Result<(), i32> {
                         if equal_name(alias, addresses.canon.text()) || alias_names[..alias_count].iter()
                             .any(|n| equal_name(&n[..n.iter().position(|b| *b == 0).unwrap_or(256)], alias)) {
                             return Ok(());
                         }
                         if alias_count == alias_names.len() { return Err(EAI_MEMORY); }
                         alias_names[alias_count] = Name::new(alias)?.bytes;
                         alias_count += 1;
                         Ok(())
                     };
                     // Hosts aliases are available even when the query was the canonical name.
                     lines(b"/etc/hosts\0", |line| {
                         if let Some((addr, canon)) = hosts_line(line) {
                             if equal_name(canon, addresses.canon.text()) && addresses.bytes[..addresses.len].contains(&addr) {
                                 for alias in fields(line).skip(2) { add_alias(alias)?; }
                             }
                         }
                         Ok(())
                     })?;
                     // A DNS CNAME query is also an alias of its final canonical target.
                     add_alias(text)?;
                     Ok(addresses)
                 }) };
    match result {
        Ok(addresses) => {
            let storage = ptr::addr_of_mut!(HOST);
            (*storage).name = addresses.canon.bytes;
            (*storage).addresses = addresses.bytes;
            (*storage).alias_names = alias_names;
            for i in 0..alias_count {
                (*storage).aliases[i] = ptr::addr_of_mut!((*storage).alias_names[i]).cast();
            }
            (*storage).aliases[alias_count] = ptr::null_mut();
            for i in 0..addresses.len {
                (*storage).pointers[i] = ptr::addr_of_mut!((*storage).addresses[i]).cast();
            }
            (*storage).pointers[addresses.len] = ptr::null_mut();
            (*storage).entry.name = ptr::addr_of_mut!((*storage).name).cast();
            (*storage).entry.aliases = ptr::addr_of_mut!((*storage).aliases).cast();
            (*storage).entry.addr_list = ptr::addr_of_mut!((*storage).pointers).cast();
            *error = 0;
            *crate::__errno_location() = saved_errno;
            ptr::addr_of_mut!((*storage).entry)
        }
        Err(e) => {
            *error = match e { EAI_NONAME => 1, EAI_AGAIN => 2, NO_ADDRESS => 4, _ => 3 };
            if e == EAI_MEMORY { *crate::__errno_location() = Errno::ENOMEM as i32; }
            else if e != EAI_SYSTEM { *crate::__errno_location() = saved_errno; }
            ptr::null_mut()
        }
    }
}
#[no_mangle]
pub extern "C" fn hstrerror(error: i32) -> *const u8 {
    match error {
        0 => b"Resolver success\0".as_ptr(),
        1 => b"Host not found\0".as_ptr(),
        2 => b"Temporary resolver failure\0".as_ptr(),
        3 => b"Unrecoverable resolver failure\0".as_ptr(),
        4 => b"No address data\0".as_ptr(),
        _ => b"Unknown resolver error\0".as_ptr(),
    }
}
