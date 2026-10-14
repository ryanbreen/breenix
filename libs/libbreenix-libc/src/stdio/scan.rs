//! Formatted input from C17 7.21.6.2 and 7.29.2.2 / POSIX fscanf, using STRMATH's
//! number cores. One engine serves narrow and wide formats.
use super::{
    file::{self, File},
    format::{store_count, Format, Length, Text},
};
use crate::stdlib::numscan::{scan_f32, scan_f64, scan_int, scan_long_double, Chars};
use core::{ffi::VaList, ptr};
fn whitespace(c: u32, wide_input: bool) -> bool {
    // C17 7.29.2.2: wide scanf uses the locale's iswspace classification.
    if wide_input { crate::locale::classify::iswspace(c) != 0 }
    else { c == 32 || (9..=13).contains(&c) }
}
struct StringChars {
    p: *const u8,
    n: usize,
}
impl Chars for StringChars {
    fn peek(&mut self) -> Option<u32> {
        let c = unsafe { *self.p.add(self.n) };
        if c == 0 {
            None
        } else {
            Some(c as u32)
        }
    }
    fn bump(&mut self) {
        if self.peek().is_some() {
            self.n += 1;
        }
    }
    fn pos(&self) -> usize {
        self.n
    }
    // scanf consumes the longest initial subsequence, even when incomplete ("1e+").
    fn rewind(&mut self, _pos: usize) -> bool {
        false
    }
}
struct StreamChars {
    f: *mut File,
    n: usize,
    look: Option<Option<u32>>,
}
impl Chars for StreamChars {
    fn peek(&mut self) -> Option<u32> {
        if self.look.is_none() {
            let c = unsafe { file::getc_locked(self.f) };
            self.look = Some(if c < 0 { None } else { Some(c as u32) });
        }
        self.look.unwrap()
    }
    fn bump(&mut self) {
        if self.peek().is_some() {
            self.look = None;
            self.n += 1;
        }
    }
    fn pos(&self) -> usize {
        self.n
    }
    fn rewind(&mut self, _pos: usize) -> bool {
        false
    }
}
impl Drop for StreamChars {
    fn drop(&mut self) {
        if let Some(Some(c)) = self.look {
            unsafe {
                file::ungetc_locked(c as i32, self.f);
            }
        }
    }
}
unsafe fn length(fmt: &Format, i: &mut usize) -> Length {
    let c = fmt.at(*i);
    let l = match c {
        104 => Length::H,
        108 => Length::L,
        106 => Length::J,
        122 => Length::Z,
        116 => Length::T,
        76 => Length::BigL,
        _ => return Length::None,
    };
    *i += 1;
    if c == 104 && fmt.at(*i) == 104 {
        *i += 1;
        Length::Hh
    } else if c == 108 && fmt.at(*i) == 108 {
        *i += 1;
        Length::Ll
    } else {
        l
    }
}
/// Whether c is a member of the scanset written at fmt[start..end].
unsafe fn in_set(fmt: &Format, start: usize, end: usize, c: u32) -> bool {
    let mut i = start;
    while i < end {
        let first = fmt.at(i);
        if i + 2 < end && fmt.at(i + 1) == 45 {
            let last = fmt.at(i + 2);
            if first <= last {
                if (first..=last).contains(&c) {
                    return true;
                }
            } else if c == first || c == 45 || c == last {
                return true;
            }
            i += 3;
        } else {
            if c == first {
                return true;
            }
            i += 1;
        }
    }
    false
}
fn skip(s: &mut dyn Chars, wide_input: bool) {
    while s.peek().is_some_and(|c| whitespace(c, wide_input)) {
        s.bump();
    }
}
fn stopped(assigned: i32, completed: bool, input: bool) -> i32 {
    if assigned == 0 && !completed && input {
        -1
    } else {
        assigned
    }
}
unsafe fn destination(ap: &mut VaList<'_, '_>, position: usize) -> *mut u8 {
    if position == 0 {
        return ap.arg::<*mut u8>();
    }
    let mut copy = (**ap).clone();
    let mut list = copy.as_va_list();
    for _ in 1..position {
        list.arg::<*mut u8>();
    }
    list.arg::<*mut u8>()
}
unsafe fn scan_error(stream: *mut File, errno: i32) {
    crate::pthread::set_errno(errno);
    if !stream.is_null() {
        file::set_error(stream);
    }
}
/// The scanf engine. With a wide format, `s` yields wide characters (LOCALE's wide
/// sources), and %c, %s and %[ without l store their multibyte encoding.
pub(crate) unsafe fn vscan(
    s: &mut dyn Chars,
    stream: *mut File,
    fmt: &Format,
    mut ap: VaList<'_, '_>,
) -> i32 {
    let wide_input = fmt.is_wide();
    let mut i = 0;
    let mut assigned = 0;
    let mut completed = false;
    while fmt.at(i) != 0 {
        let c = fmt.at(i);
        i += 1;
        if whitespace(c, wide_input) {
            skip(s, wide_input);
            continue;
        }
        if c != 37 {
            match s.peek() {
                Some(k) if k == c => s.bump(),
                other => return stopped(assigned, completed, other.is_none()),
            }
            continue;
        }
        if fmt.at(i) == 37 {
            i += 1;
            // %% is a conversion specification, so leading white space is skipped
            // (C17 7.21.6.2p8).
            skip(s, wide_input);
            match s.peek() {
                Some(37) => s.bump(),
                other => return stopped(assigned, completed, other.is_none()),
            }
            continue;
        }
        let initial = i;
        let mut position = 0usize;
        while (48..=57).contains(&fmt.at(i)) {
            let Some(n) = position
                .checked_mul(10)
                .and_then(|n| n.checked_add((fmt.at(i) - 48) as usize))
            else {
                scan_error(stream, 22);
                return assigned;
            };
            position = n;
            i += 1;
        }
        if fmt.at(i) == 36 {
            if position == 0 || position > super::args::LIMIT {
                scan_error(stream, 22);
                return assigned;
            }
            i += 1;
        } else {
            position = 0;
            i = initial;
        }
        let suppressed = fmt.at(i) == 42;
        if suppressed {
            i += 1;
        }
        let mut width = 0usize;
        while (48..=57).contains(&fmt.at(i)) {
            let Some(n) = width
                .checked_mul(10)
                .and_then(|n| n.checked_add((fmt.at(i) - 48) as usize))
            else {
                scan_error(stream, 75);
                return stopped(assigned, completed, true);
            };
            width = n;
            i += 1;
        }
        if width == 0 {
            width = usize::MAX;
        }
        let allocated = fmt.at(i) == 109;
        if allocated {
            i += 1;
        }
        let mut l = length(fmt, &mut i);
        let Ok(mut c) = u8::try_from(fmt.at(i)) else {
            scan_error(stream, 22);
            return assigned;
        };
        i += 1;
        // XSI: %C is %lc and %S is %ls.
        if c == b'C' || c == b'S' {
            l = Length::L;
            c = c.to_ascii_lowercase();
        }
        if c == b'n' {
            if !suppressed {
                store_count(destination(&mut ap, position), l, s.pos());
            }
            completed = true;
            continue;
        }
        let mut invert = false;
        let (mut set_start, mut set_end) = (0, 0);
        if c == b'[' {
            if fmt.at(i) == 94 {
                invert = true;
                i += 1;
            }
            set_start = i;
            if fmt.at(i) == 93 {
                i += 1;
            }
            while fmt.at(i) != 0 && fmt.at(i) != 93 {
                i += 1;
            }
            if fmt.at(i) != 93 {
                scan_error(stream, 22);
                return assigned;
            }
            set_end = i;
            i += 1;
        }
        if c != b'c' && c != b'[' {
            skip(s, wide_input);
        }
        let input_missing = s.peek().is_none();
        if input_missing {
            return stopped(assigned, completed, true);
        }
        match c {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' => {
                let base = match c {
                    b'i' => 0,
                    b'o' => 8,
                    b'x' | b'X' | b'p' => 16,
                    _ => 10,
                };
                let Ok(r) = scan_int(s, base, width) else {
                    return assigned;
                };
                if !r.digits {
                    return stopped(assigned, completed, false);
                }
                if r.overflow {
                    crate::pthread::set_errno(34);
                }
                if !suppressed {
                    let p = destination(&mut ap, position);
                    let value = if r.negative {
                        r.magnitude.wrapping_neg()
                    } else {
                        r.magnitude
                    };
                    if c == b'p' {
                        *(p as *mut *mut u8) = value as *mut u8;
                    } else {
                        store_count(p, l, value as usize);
                    }
                    assigned += 1;
                }
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                let p = if suppressed {
                    ptr::null_mut()
                } else {
                    destination(&mut ap, position)
                };
                let range = if l == Length::BigL {
                    let Some((bits, range)) = scan_long_double(s, width) else {
                        return stopped(assigned, completed, false);
                    };
                    if !suppressed {
                        ptr::write_unaligned(p as *mut u128, bits);
                    }
                    range
                } else if l == Length::L {
                    let Some(r) = scan_f64(s, width) else {
                        return stopped(assigned, completed, false);
                    };
                    if !suppressed {
                        *(p as *mut f64) = r.value;
                    }
                    r.range_error
                } else {
                    let Some((v, range)) = scan_f32(s, width) else {
                        return stopped(assigned, completed, false);
                    };
                    if !suppressed {
                        *(p as *mut f32) = v;
                    }
                    range
                };
                if range {
                    crate::pthread::set_errno(34);
                }
                if !suppressed {
                    assigned += 1;
                }
            }
            b'c' | b's' | b'[' => {
                if c == b'c' && width == usize::MAX {
                    width = 1;
                }
                let wide = l == Length::L;
                let destination = if suppressed {
                    ptr::null_mut()
                } else {
                    destination(&mut ap, position)
                };
                let mut body = Text::new();
                let mut units = 0;
                let mut st = core::mem::zeroed();
                while units < width {
                    let Some(k) = s.peek() else { break };
                    let matches = match c {
                        b's' => !whitespace(k, wide_input),
                        b'[' => in_set(fmt, set_start, set_end, k) != invert,
                        _ => true,
                    };
                    if !matches {
                        break;
                    }
                    // Narrow input stored wide is decoded; wide input stored narrow is
                    // encoded (C17 7.29.2.2p12: as if by wcrtomb).
                    let wc = if wide_input {
                        s.bump();
                        k
                    } else if wide {
                        loop {
                            let Some(k) = s.peek() else {
                                scan_error(stream, 84);
                                return stopped(assigned, completed, true);
                            };
                            s.bump();
                            match crate::locale::decode(&[k as u8], &mut st) {
                                crate::locale::Decoded::Char(wc, _) => break wc,
                                crate::locale::Decoded::Incomplete(_) => continue,
                                crate::locale::Decoded::Invalid => {
                                    scan_error(stream, 84);
                                    return stopped(assigned, completed, true);
                                }
                            }
                        }
                    } else {
                        s.bump();
                        k
                    };
                    if !suppressed {
                        let mut encoded = [0u8; 4];
                        let bytes: &[u8] = if wide {
                            encoded = wc.to_ne_bytes();
                            &encoded
                        } else if wide_input {
                            let Some(n) = crate::locale::encode(wc, &mut encoded) else {
                                scan_error(stream, 84);
                                return stopped(assigned, completed, true);
                            };
                            &encoded[..n]
                        } else {
                            encoded[0] = wc as u8;
                            &encoded[..1]
                        };
                        if let Err(e) = body.bytes(bytes) {
                            scan_error(stream, e);
                            return stopped(assigned, completed, true);
                        }
                    }
                    units += 1;
                }
                if c == b'c' && units < width {
                    return stopped(assigned, completed, true);
                }
                if units == 0 {
                    return stopped(assigned, completed, s.peek().is_none());
                }
                if !suppressed {
                    if c != b'c' {
                        let zeros = [0u8; 4];
                        if let Err(e) = body.bytes(&zeros[..if wide { 4 } else { 1 }]) {
                            scan_error(stream, e);
                            return stopped(assigned, completed, true);
                        }
                    }
                    if allocated {
                        *(destination as *mut *mut u8) = body.p;
                        body.p = ptr::null_mut();
                    } else {
                        ptr::copy_nonoverlapping(body.p, destination, body.n);
                    }
                    assigned += 1;
                }
            }
            _ => {
                scan_error(stream, 22);
                return assigned;
            }
        }
        completed = true;
    }
    assigned
}
#[no_mangle]
pub unsafe extern "C" fn vsscanf(input: *const u8, fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    vscan(
        &mut StringChars { p: input, n: 0 },
        ptr::null_mut(),
        &Format::Narrow(fmt),
        ap,
    )
}
#[no_mangle]
pub unsafe extern "C" fn sscanf(input: *const u8, fmt: *const u8, mut ap: ...) -> i32 {
    vsscanf(input, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vfscanf(f: *mut File, fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    file::lock(f);
    file::orient(f, -1);
    let mut source = StreamChars {
        f,
        n: 0,
        look: None,
    };
    let r = vscan(&mut source, f, &Format::Narrow(fmt), ap);
    drop(source);
    file::unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fscanf(f: *mut File, fmt: *const u8, mut ap: ...) -> i32 {
    vfscanf(f, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vscanf(fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    vfscanf(file::stdin, fmt, ap)
}
#[no_mangle]
pub unsafe extern "C" fn scanf(fmt: *const u8, mut ap: ...) -> i32 {
    vscanf(fmt, ap.as_va_list())
}
