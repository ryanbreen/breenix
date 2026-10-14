//! Address conversion written from the POSIX interface and RFC 3493.

use libbreenix::errno::Errno;

pub(crate) const AF_INET: i32 = 2;

/// Strict dotted decimal: four octets, no whitespace, signs, or octal forms.
pub(crate) fn parse_ipv4(text: &[u8]) -> Option<[u8; 4]> {
    let mut addr = [0; 4];
    let mut parts = text.split(|b| *b == b'.');
    for octet in &mut addr {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || (part.len() > 1 && part[0] == b'0') {
            return None;
        }
        let mut n = 0u16;
        for b in part {
            if !b.is_ascii_digit() {
                return None;
            }
            n = n * 10 + (b - b'0') as u16;
        }
        if n > 255 {
            return None;
        }
        *octet = n as u8;
    }
    if parts.next().is_some() {
        None
    } else {
        Some(addr)
    }
}

/// inet_addr forms used by getaddrinfo: one to four parts, decimal/octal/hex.
pub(crate) fn parse_inet_addr(text: &[u8]) -> Option<[u8; 4]> {
    let mut parts = [0u32; 4];
    let mut count = 0;
    for part in text.split(|b| *b == b'.') {
        if count == 4 || part.is_empty() {
            return None;
        }
        let (base, digits) = if part.starts_with(b"0x") || part.starts_with(b"0X") {
            (16, &part[2..])
        } else if part.len() > 1 && part[0] == b'0' {
            (8, &part[1..])
        } else {
            (10, part)
        };
        if digits.is_empty() {
            return None;
        }
        let mut n = 0u32;
        for &b in digits {
            let d = match b {
                b'0'..=b'9' => (b - b'0') as u32,
                b'a'..=b'f' => (b - b'a' + 10) as u32,
                b'A'..=b'F' => (b - b'A' + 10) as u32,
                _ => return None,
            };
            if d >= base {
                return None;
            }
            n = n.checked_mul(base)?.checked_add(d)?;
        }
        parts[count] = n;
        count += 1;
    }
    let mut value = 0;
    for &part in &parts[..count - 1] {
        if part > 255 {
            return None;
        }
        value = (value << 8) | part;
    }
    let bits = 8 * (5 - count);
    let last = parts[count - 1];
    if bits < 32 && last >= 1u32 << bits {
        return None;
    }
    Some(
        if bits == 32 {
            last
        } else {
            (value << bits) | last
        }
        .to_be_bytes(),
    )
}

pub(crate) fn decimal(mut n: u16, out: &mut [u8; 5]) -> &[u8] {
    let mut start = out.len();
    loop {
        start -= 1;
        out[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &out[start..];
        }
    }
}

pub(crate) fn format_ipv4(addr: [u8; 4], out: &mut [u8; 16]) -> usize {
    let mut n = 0;
    for (i, octet) in addr.iter().enumerate() {
        if i != 0 {
            out[n] = b'.';
            n += 1;
        }
        let mut digits = [0; 5];
        let text = decimal(*octet as u16, &mut digits);
        out[n..n + text.len()].copy_from_slice(text);
        n += text.len();
    }
    out[n] = 0;
    n
}

pub(crate) unsafe fn c_bytes<'a>(ptr: *const u8, max: usize) -> Option<&'a [u8]> {
    for n in 0..=max {
        if *ptr.add(n) == 0 {
            return Some(core::slice::from_raw_parts(ptr, n));
        }
    }
    None
}

#[no_mangle]
pub unsafe extern "C" fn inet_pton(family: i32, src: *const u8, dst: *mut u8) -> i32 {
    if family != AF_INET {
        *crate::__errno_location() = Errno::EAFNOSUPPORT as i32;
        return -1;
    }
    let Some(text) = c_bytes(src, 15) else {
        return 0;
    };
    let Some(addr) = parse_ipv4(text) else {
        return 0;
    };
    core::ptr::copy_nonoverlapping(addr.as_ptr(), dst, 4);
    1
}

#[no_mangle]
pub unsafe extern "C" fn inet_ntop(
    family: i32,
    src: *const u8,
    dst: *mut u8,
    size: u32,
) -> *const u8 {
    if family != AF_INET {
        *crate::__errno_location() = Errno::EAFNOSUPPORT as i32;
        return core::ptr::null();
    }
    let mut text = [0; 16];
    let n = format_ipv4(core::ptr::read_unaligned(src as *const [u8; 4]), &mut text);
    if size as usize <= n {
        *crate::__errno_location() = Errno::ENOSPC as i32;
        return core::ptr::null();
    }
    core::ptr::copy_nonoverlapping(text.as_ptr(), dst, n + 1);
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_decimal_and_roundtrip() {
        for text in [
            b"1.2.3".as_slice(),
            b"1.2.3.4x",
            b"256.0.0.1",
            b"01.2.3.4",
            b"+1.2.3.4",
            b"1.2.3.4.",
            b"1..3.4",
        ] {
            assert_eq!(parse_ipv4(text), None);
        }
        for addr in [[0; 4], [255; 4], [192, 0, 2, 7]] {
            let mut text = [0; 16];
            let n = format_ipv4(addr, &mut text);
            assert_eq!(parse_ipv4(&text[..n]), Some(addr));
        }
    }
}

/// inet_aton: the one- to four-part forms (decimal, octal or hexadecimal parts) into
/// `*addr` in network byte order; 1 on success, 0 for an invalid string.
#[no_mangle]
pub unsafe extern "C" fn inet_aton(text: *const u8, addr: *mut u32) -> i32 {
    match c_bytes(text, 64).and_then(parse_inet_addr) {
        Some(bytes) => {
            if !addr.is_null() {
                *addr = u32::from_ne_bytes(bytes);
            }
            1
        }
        None => 0,
    }
}

/// inet_addr: as inet_aton, returning the address, or INADDR_NONE (all ones) for an
/// invalid string.
#[no_mangle]
pub unsafe extern "C" fn inet_addr(text: *const u8) -> u32 {
    match c_bytes(text, 64).and_then(parse_inet_addr) {
        Some(bytes) => u32::from_ne_bytes(bytes),
        None => u32::MAX,
    }
}

static mut NTOA_ONCE: i32 = 0;
static mut NTOA_KEY: u32 = 0;
static mut NTOA_KEY_ERROR: i32 = 0;

unsafe extern "C" fn free_ntoa(buffer: *mut u8) {
    crate::free(buffer);
}

extern "C" fn init_ntoa() {
    unsafe {
        NTOA_KEY_ERROR = crate::pthread::pthread_key_create(
            core::ptr::addr_of_mut!(NTOA_KEY), Some(free_ntoa));
    }
}

/// inet_ntoa: dotted decimal in storage the next call on this thread overwrites.
#[no_mangle]
pub unsafe extern "C" fn inet_ntoa(addr: u32) -> *mut u8 {
    crate::pthread::pthread_once(core::ptr::addr_of_mut!(NTOA_ONCE), init_ntoa);
    // pthread_once publishes the key and its creation result to every caller.
    if NTOA_KEY_ERROR != 0 {
        *crate::__errno_location() = NTOA_KEY_ERROR;
        return core::ptr::null_mut();
    }
    let mut buffer = crate::pthread::pthread_getspecific(NTOA_KEY);
    if buffer.is_null() {
        buffer = crate::calloc(1, 16);
        if buffer.is_null() {
            *crate::__errno_location() = Errno::ENOMEM as i32;
            return buffer;
        }
        let error = crate::pthread::pthread_setspecific(NTOA_KEY, buffer);
        if error != 0 {
            crate::free(buffer);
            *crate::__errno_location() = error;
            return core::ptr::null_mut();
        }
    }
    let out = &mut *(buffer as *mut [u8; 16]);
    format_ipv4(addr.to_ne_bytes(), out);
    buffer
}

#[no_mangle]
pub extern "C" fn htonl(v: u32) -> u32 {
    v.to_be()
}

#[no_mangle]
pub extern "C" fn htons(v: u16) -> u16 {
    v.to_be()
}

#[no_mangle]
pub extern "C" fn ntohl(v: u32) -> u32 {
    u32::from_be(v)
}

#[no_mangle]
pub extern "C" fn ntohs(v: u16) -> u16 {
    u16::from_be(v)
}

/// The IPv6 wildcard address (::) and loopback address (::1).
#[no_mangle]
pub static in6addr_any: [u8; 16] = [0; 16];
#[no_mangle]
pub static in6addr_loopback: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
