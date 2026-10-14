//! Formatted input from C17 7.21.6.2 / POSIX fscanf, using STRMATH's number cores.
use super::{
    file::{self, File},
    format::{store_count, Length, Text},
};
use crate::stdlib::numscan::{scan_f32, scan_f64, scan_int, scan_long_double, Chars};
use core::{ffi::VaList, ptr};
fn whitespace(c: u32) -> bool {
    c == 32 || (9..=13).contains(&c)
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
unsafe fn length(fmt: *const u8, i: &mut usize) -> Length {
    let c = *fmt.add(*i);
    let l = match c {
        b'h' => Length::H,
        b'l' => Length::L,
        b'j' => Length::J,
        b'z' => Length::Z,
        b't' => Length::T,
        b'L' => Length::BigL,
        _ => return Length::None,
    };
    *i += 1;
    if c == b'h' && *fmt.add(*i) == b'h' {
        *i += 1;
        Length::Hh
    } else if c == b'l' && *fmt.add(*i) == b'l' {
        *i += 1;
        Length::Ll
    } else {
        l
    }
}
fn skip(s: &mut dyn Chars) {
    while s.peek().is_some_and(whitespace) {
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
unsafe fn vscan(
    s: &mut dyn Chars,
    stream: *mut File,
    fmt: *const u8,
    mut ap: VaList<'_, '_>,
) -> i32 {
    let mut i = 0;
    let mut assigned = 0;
    let mut completed = false;
    while *fmt.add(i) != 0 {
        let c = *fmt.add(i);
        i += 1;
        if whitespace(c as u32) {
            skip(s);
            continue;
        }
        if c != b'%' {
            match s.peek() {
                Some(k) if k == c as u32 => s.bump(),
                other => return stopped(assigned, completed, other.is_none()),
            }
            continue;
        }
        if *fmt.add(i) == b'%' {
            i += 1;
            // %% is a conversion specification, so leading white space is skipped
            // (C17 7.21.6.2p8).
            skip(s);
            match s.peek() {
                Some(37) => s.bump(),
                other => return stopped(assigned, completed, other.is_none()),
            }
            continue;
        }
        let initial = i;
        let mut position = 0usize;
        while (*fmt.add(i)).is_ascii_digit() {
            let Some(n) = position
                .checked_mul(10)
                .and_then(|n| n.checked_add((*fmt.add(i) - b'0') as usize))
            else {
                scan_error(stream, 22);
                return assigned;
            };
            position = n;
            i += 1;
        }
        if *fmt.add(i) == b'$' {
            if position == 0 || position > super::args::LIMIT {
                scan_error(stream, 22);
                return assigned;
            }
            i += 1;
        } else {
            position = 0;
            i = initial;
        }
        let suppressed = *fmt.add(i) == b'*';
        if suppressed {
            i += 1;
        }
        let mut width = 0usize;
        while (*fmt.add(i)).is_ascii_digit() {
            let Some(n) = width
                .checked_mul(10)
                .and_then(|n| n.checked_add((*fmt.add(i) - b'0') as usize))
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
        let allocated = *fmt.add(i) == b'm';
        if allocated {
            i += 1;
        }
        let mut l = length(fmt, &mut i);
        let mut c = *fmt.add(i);
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
        let mut set = [false; 256];
        let mut invert = false;
        if c == b'[' {
            if *fmt.add(i) == b'^' {
                invert = true;
                i += 1;
            }
            if *fmt.add(i) == b']' {
                set[b']' as usize] = true;
                i += 1;
            }
            while *fmt.add(i) != 0 && *fmt.add(i) != b']' {
                let start = *fmt.add(i);
                i += 1;
                if *fmt.add(i) == b'-' && *fmt.add(i + 1) != 0 && *fmt.add(i + 1) != b']' {
                    let end = *fmt.add(i + 1);
                    if start <= end {
                        for ch in start..=end {
                            set[ch as usize] = true;
                        }
                    } else {
                        set[start as usize] = true;
                        set[b'-' as usize] = true;
                        set[end as usize] = true;
                    }
                    i += 2;
                } else {
                    set[start as usize] = true;
                }
            }
            if *fmt.add(i) != b']' {
                scan_error(stream, 22);
                return assigned;
            }
            i += 1;
        }
        if c != b'c' && c != b'[' {
            skip(s);
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
                        b's' => !whitespace(k),
                        b'[' => set[k as usize] != invert,
                        _ => true,
                    };
                    if !matches {
                        break;
                    }
                    if wide {
                        let wc = loop {
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
                        };
                        if !suppressed {
                            if let Err(e) = body.bytes(&wc.to_ne_bytes()) {
                                scan_error(stream, e);
                                return stopped(assigned, completed, true);
                            }
                        }
                    } else {
                        s.bump();
                        if !suppressed {
                            if let Err(e) = body.byte(k as u8) {
                                scan_error(stream, e);
                                return stopped(assigned, completed, true);
                            }
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
        fmt,
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
    let r = vscan(&mut source, f, fmt, ap);
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
