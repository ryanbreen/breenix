//! Address conversion written from the POSIX interface and RFC 3493.

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
        *crate::__errno_location() = 97;
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
        *crate::__errno_location() = 97;
        return core::ptr::null();
    }
    let mut text = [0; 16];
    let n = format_ipv4(core::ptr::read_unaligned(src as *const [u8; 4]), &mut text);
    if size as usize <= n {
        *crate::__errno_location() = 28;
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
