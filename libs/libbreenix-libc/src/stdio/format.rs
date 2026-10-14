//! Shared narrow/wide formatting, from C17 7.21.6.1 and 7.29.2.1 and POSIX fprintf.
use super::file::{self, File};
use core::{ffi::VaList, ptr, slice};
pub(crate) trait Sink {
    fn bytes(&mut self, b: &[u8]) -> bool;
    fn wide(&mut self, wc: u32) -> bool;
}
#[allow(dead_code)] // Shared wide-format API intentionally consumed by the LOCALE lane.
pub(crate) enum Format {
    Narrow(*const u8),
    Wide(*const u32),
}
impl Format {
    pub(super) unsafe fn at(&self, i: usize) -> u32 {
        match *self {
            Self::Narrow(p) => *p.add(i) as u32,
            Self::Wide(p) => *p.add(i),
        }
    }
    fn is_wide(&self) -> bool {
        matches!(self, Self::Wide(_))
    }
}
pub(super) struct Text {
    pub p: *mut u8,
    pub n: usize,
    cap: usize,
}
impl Text {
    pub fn new() -> Self {
        Self {
            p: ptr::null_mut(),
            n: 0,
            cap: 0,
        }
    }
    pub fn byte(&mut self, b: u8) -> Result<(), i32> {
        self.bytes(&[b])
    }
    pub fn bytes(&mut self, b: &[u8]) -> Result<(), i32> {
        let need = self
            .n
            .checked_add(b.len())
            .filter(|&n| n <= i32::MAX as usize)
            .ok_or(75)?;
        if need > self.cap {
            let cap = core::cmp::max(need, self.cap.saturating_mul(2).max(64));
            let p = unsafe { crate::realloc(self.p, cap) };
            if p.is_null() {
                return Err(12);
            }
            self.p = p;
            self.cap = cap;
        }
        if !b.is_empty() {
            unsafe {
                ptr::copy_nonoverlapping(b.as_ptr(), self.p.add(self.n), b.len());
            }
        }
        self.n = need;
        Ok(())
    }
    pub fn as_bytes(&self) -> &[u8] {
        if self.n == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.p, self.n) }
        }
    }
}
impl Drop for Text {
    fn drop(&mut self) {
        unsafe {
            crate::free(self.p);
        }
    }
}
fn emit(out: &mut dyn Sink, b: &[u8]) -> Result<(), i32> {
    if out.bytes(b) {
        Ok(())
    } else {
        let e = crate::pthread::errno();
        Err(if e == 0 { 5 } else { e })
    }
}
fn padding(out: &mut dyn Sink, n: usize, c: u8) -> Result<(), i32> {
    let buf = [c; 64];
    let mut n = n;
    while n > 0 {
        let k = n.min(64);
        emit(out, &buf[..k])?;
        n -= k;
    }
    Ok(())
}
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Length {
    None,
    H,
    Hh,
    L,
    Ll,
    J,
    Z,
    T,
    BigL,
}
fn digits(
    out: &mut Text,
    mut v: u64,
    base: u64,
    upper: bool,
    precision: Option<usize>,
) -> Result<(), i32> {
    let mut b = [0; 64];
    let mut n = 0;
    let alphabet = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    if v != 0 || precision != Some(0) {
        loop {
            b[n] = alphabet[(v % base) as usize];
            n += 1;
            v /= base;
            if v == 0 {
                break;
            }
        }
    }
    for _ in n..precision.unwrap_or(1) {
        out.byte(b'0')?
    }
    for i in (0..n).rev() {
        out.byte(b[i])?
    }
    Ok(())
}
pub(super) unsafe fn store_count(p: *mut u8, l: Length, n: usize) {
    match l {
        Length::Hh => *(p as *mut i8) = n as i8,
        Length::H => *(p as *mut i16) = n as i16,
        Length::L | Length::Ll | Length::J | Length::Z | Length::T => *(p as *mut i64) = n as i64,
        _ => *(p as *mut i32) = n as i32,
    }
}
struct Counting<'a> {
    out: &'a mut dyn Sink,
    n: usize,
    wide: bool,
}
impl Counting<'_> {
    fn add(&mut self, n: usize) -> bool {
        match self.n.checked_add(n).filter(|&n| n <= i32::MAX as usize) {
            Some(n) => {
                self.n = n;
                true
            }
            None => {
                crate::pthread::set_errno(75);
                false
            }
        }
    }
}
impl super::Sink for Counting<'_> {
    fn bytes(&mut self, b: &[u8]) -> bool {
        let n = if self.wide {
            let mut st = unsafe { core::mem::zeroed() };
            let mut pos = 0;
            let mut count = 0;
            while pos < b.len() {
                match crate::locale::decode(&b[pos..], &mut st) {
                    crate::locale::Decoded::Char(_, used) => {
                        pos += used;
                        count += 1
                    }
                    _ => {
                        crate::pthread::set_errno(84);
                        return false;
                    }
                }
            }
            count
        } else {
            b.len()
        };
        self.add(n) && self.out.bytes(b)
    }
    fn wide(&mut self, wc: u32) -> bool {
        let n = if self.wide {
            1
        } else {
            let mut b = [0; 4];
            let Some(n) = crate::locale::encode(wc, &mut b) else {
                crate::pthread::set_errno(84);
                return false;
            };
            n
        };
        self.add(n) && self.out.wide(wc)
    }
}
pub(crate) unsafe fn vformat(
    out: &mut dyn Sink,
    fmt: Format,
    ap: VaList<'_, '_>,
) -> Result<(), i32> {
    let mut args = super::args::Arguments::new(ap, &fmt)?;
    let mut out = Counting {
        out,
        n: 0,
        wide: fmt.is_wide(),
    };
    let mut i = 0;
    while fmt.at(i) != 0 {
        let wc = fmt.at(i);
        i += 1;
        if wc != 37 {
            if fmt.is_wide() {
                if !out.wide(wc) {
                    return Err(crate::pthread::errno());
                }
            } else {
                emit(&mut out, &[wc as u8])?
            }
            continue;
        }
        if fmt.at(i) == 37 {
            i += 1;
            emit(&mut out, b"%")?;
            continue;
        }
        let mut spec = super::args::Spec::parse(&fmt, &mut i)?;
        args.widths(&mut spec);
        let (left, plus, space, alternate, mut zero) =
            (spec.left, spec.plus, spec.space, spec.alternate, spec.zero);
        let width = spec.width;
        if width > i32::MAX as usize {
            return Err(75);
        }
        let precision = spec.precision;
        let l = spec.length;
        let c = spec.conversion;
        if fmt.is_wide() && (c == b'c' || c == b's') {
            let mut words = Text::new();
            if c == b'c' {
                let mut wc = args.integer(&spec) as u32;
                if l != Length::L {
                    let mut state = core::mem::zeroed();
                    wc = match crate::locale::decode(&[wc as u8], &mut state) {
                        crate::locale::Decoded::Char(wc, _) => wc,
                        _ => return Err(84),
                    };
                }
                words.bytes(&wc.to_ne_bytes())?;
            } else if l == Length::L {
                let mut p = args.pointer(&spec) as *const u32;
                while *p != 0 && words.n / 4 < precision.unwrap_or(usize::MAX) {
                    words.bytes(&(*p).to_ne_bytes())?;
                    p = p.add(1);
                }
            } else {
                let mut p = args.pointer(&spec) as *const u8;
                if p.is_null() {
                    p = c"(null)".as_ptr() as *const u8;
                }
                let mut state = core::mem::zeroed();
                while *p != 0 && words.n / 4 < precision.unwrap_or(usize::MAX) {
                    let wc = loop {
                        if *p == 0 {
                            return Err(84);
                        }
                        let byte = *p;
                        p = p.add(1);
                        match crate::locale::decode(&[byte], &mut state) {
                            crate::locale::Decoded::Char(wc, _) => break wc,
                            crate::locale::Decoded::Incomplete(_) => continue,
                            crate::locale::Decoded::Invalid => return Err(84),
                        }
                    };
                    words.bytes(&wc.to_ne_bytes())?;
                }
            }
            let pad = width.saturating_sub(words.n / 4);
            if !left {
                padding(&mut out, pad, b' ')?
            }
            for chunk in words.as_bytes().chunks_exact(4) {
                let wc = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if !out.wide(wc) {
                    return Err(crate::pthread::errno());
                }
            }
            if left {
                padding(&mut out, pad, b' ')?
            }
            continue;
        }
        let mut body = Text::new();
        let mut prefix = Text::new();
        let mut numeric = false;
        let mut field_units = 0;
        match c {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' => {
                numeric = true;
                let signed = c == b'd' || c == b'i';
                let v = if c == b'p' {
                    args.pointer(&spec) as u64
                } else {
                    args.integer(&spec)
                };
                let negative = signed && (v as i64) < 0;
                let magnitude = if negative { v.wrapping_neg() } else { v };
                if negative {
                    prefix.byte(b'-')?
                } else if signed && plus {
                    prefix.byte(b'+')?
                } else if signed && space {
                    prefix.byte(b' ')?
                }
                let base = if c == b'o' {
                    8
                } else if c == b'x' || c == b'X' || c == b'p' {
                    16
                } else {
                    10
                };
                if c == b'p' || (alternate && magnitude != 0 && base == 16) {
                    prefix.bytes(if c == b'X' { b"0X" } else { b"0x" })?
                }
                digits(&mut body, magnitude, base, c == b'X', precision)?;
                if alternate && base == 8 && (body.n == 0 || *body.p != b'0') {
                    prefix.byte(b'0')?
                }
                if precision.is_some() {
                    zero = false
                }
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                let v = args.floating(&spec);
                numeric = true;
                if v.negative {
                    prefix.byte(b'-')?
                } else if plus {
                    prefix.byte(b'+')?
                } else if space {
                    prefix.byte(b' ')?
                }
                super::float::render(&mut body, v, c, precision, alternate)?;
                if v.special != 0 {
                    zero = false
                }
                if c.to_ascii_lowercase() == b'a' && v.special == 0 {
                    prefix.bytes(&body.as_bytes()[..2])?;
                    unsafe {
                        ptr::copy(body.p.add(2), body.p, body.n - 2);
                    }
                    body.n -= 2;
                }
            }
            b'c' | b's' => {
                let widearg = l == Length::L;
                if c == b'c' {
                    let wc = args.integer(&spec) as u32;
                    if widearg {
                        let mut b = [0; 4];
                        let n = crate::locale::encode(wc, &mut b).ok_or(84)?;
                        body.bytes(&b[..n])?;
                        field_units = if fmt.is_wide() { 1 } else { n };
                    } else {
                        body.byte(wc as u8)?;
                        field_units = 1;
                    }
                } else if widearg {
                    let mut p = args.pointer(&spec) as *const u32;
                    let limit = precision.unwrap_or(usize::MAX);
                    while *p != 0 {
                        let mut b = [0; 4];
                        let n = crate::locale::encode(*p, &mut b).ok_or(84)?;
                        let units = if fmt.is_wide() { 1 } else { n };
                        if field_units + units > limit {
                            break;
                        }
                        body.bytes(&b[..n])?;
                        field_units += units;
                        p = p.add(1);
                    }
                } else {
                    let mut p = args.pointer(&spec) as *const u8;
                    if p.is_null() {
                        p = c"(null)".as_ptr() as *const u8;
                    }
                    let limit = precision.unwrap_or(usize::MAX);
                    while *p != 0 && body.n < limit {
                        body.byte(*p)?;
                        p = p.add(1);
                    }
                    field_units = body.n;
                    if fmt.is_wide() {
                        let mut st = core::mem::zeroed();
                        let mut pos = 0;
                        field_units = 0;
                        while pos < body.n {
                            match crate::locale::decode(&body.as_bytes()[pos..], &mut st) {
                                crate::locale::Decoded::Char(_, n) => {
                                    pos += n;
                                    field_units += 1
                                }
                                _ => return Err(84),
                            }
                        }
                    }
                }
            }
            b'n' => {
                store_count(args.pointer(&spec), l, out.n);
                continue;
            }
            _ => return Err(22),
        }
        if numeric {
            field_units = prefix.n + body.n;
        }
        let pad = width.saturating_sub(field_units);
        if !left && !(numeric && zero) {
            padding(&mut out, pad, b' ')?
        }
        emit(&mut out, prefix.as_bytes())?;
        if !left && numeric && zero {
            padding(&mut out, pad, b'0')?
        }
        emit(&mut out, body.as_bytes())?;
        if left {
            padding(&mut out, pad, b' ')?
        }
    }
    Ok(())
}
struct BufferSink {
    p: *mut u8,
    cap: usize,
    n: usize,
}
impl Sink for BufferSink {
    fn bytes(&mut self, b: &[u8]) -> bool {
        let room = self.cap.saturating_sub(1).saturating_sub(self.n);
        let k = b.len().min(room);
        if k > 0 {
            unsafe {
                ptr::copy_nonoverlapping(b.as_ptr(), self.p.add(self.n), k);
            }
        }
        match self
            .n
            .checked_add(b.len())
            .filter(|&n| n <= i32::MAX as usize)
        {
            Some(n) => {
                self.n = n;
                true
            }
            None => {
                crate::pthread::set_errno(75);
                false
            }
        }
    }
    fn wide(&mut self, wc: u32) -> bool {
        let mut b = [0; 4];
        match crate::locale::encode(wc, &mut b) {
            Some(n) => self.bytes(&b[..n]),
            None => {
                crate::pthread::set_errno(84);
                false
            }
        }
    }
}
struct StreamSink {
    f: *mut File,
    n: usize,
}
impl Sink for StreamSink {
    fn bytes(&mut self, b: &[u8]) -> bool {
        let n = unsafe { file::write_locked(self.f, b) };
        self.n += n;
        n == b.len()
    }
    fn wide(&mut self, wc: u32) -> bool {
        let mut b = [0; 4];
        match crate::locale::encode(wc, &mut b) {
            Some(n) => self.bytes(&b[..n]),
            None => {
                crate::pthread::set_errno(84);
                false
            }
        }
    }
}
struct FdSink {
    fd: i32,
    n: usize,
}
impl Sink for FdSink {
    fn bytes(&mut self, b: &[u8]) -> bool {
        let mut i = 0;
        while i < b.len() {
            let n = unsafe { crate::write(self.fd, b.as_ptr().add(i), b.len() - i) };
            if n < 0 && crate::pthread::errno() == 4 {
                continue;
            }
            if n <= 0 {
                return false;
            }
            i += n as usize;
            self.n += n as usize;
        }
        true
    }
    fn wide(&mut self, wc: u32) -> bool {
        let mut b = [0; 4];
        match crate::locale::encode(wc, &mut b) {
            Some(n) => self.bytes(&b[..n]),
            None => {
                crate::pthread::set_errno(84);
                false
            }
        }
    }
}
struct AllocSink {
    body: Text,
}
impl Sink for AllocSink {
    fn bytes(&mut self, b: &[u8]) -> bool {
        if let Err(e) = self.body.bytes(b) {
            crate::pthread::set_errno(e);
            false
        } else {
            true
        }
    }
    fn wide(&mut self, wc: u32) -> bool {
        let mut b = [0; 4];
        match crate::locale::encode(wc, &mut b) {
            Some(n) => self.bytes(&b[..n]),
            None => {
                crate::pthread::set_errno(84);
                false
            }
        }
    }
}
fn result(r: Result<(), i32>, n: usize) -> i32 {
    match r {
        Ok(()) => match i32::try_from(n) {
            Ok(n) => n,
            Err(_) => {
                crate::pthread::set_errno(75);
                -1
            }
        },
        Err(e) => {
            crate::pthread::set_errno(e);
            -1
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn vsnprintf(
    p: *mut u8,
    n: usize,
    fmt: *const u8,
    ap: VaList<'_, '_>,
) -> i32 {
    let mut out = BufferSink { p, cap: n, n: 0 };
    let r = super::vformat(&mut out, super::Format::Narrow(fmt), ap);
    if n > 0 {
        *p.add(out.n.min(n - 1)) = 0;
    }
    result(r, out.n)
}
#[no_mangle]
pub unsafe extern "C" fn snprintf(p: *mut u8, n: usize, fmt: *const u8, mut ap: ...) -> i32 {
    vsnprintf(p, n, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vsprintf(p: *mut u8, fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    vsnprintf(p, usize::MAX, fmt, ap)
}
#[no_mangle]
pub unsafe extern "C" fn sprintf(p: *mut u8, fmt: *const u8, mut ap: ...) -> i32 {
    vsprintf(p, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vfprintf(f: *mut File, fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    file::lock(f);
    file::orient(f, -1);
    let mut out = StreamSink { f, n: 0 };
    let r = super::vformat(&mut out, super::Format::Narrow(fmt), ap);
    if r.is_err() {
        file::set_error(f);
    }
    file::unlock(f);
    result(r, out.n)
}
#[no_mangle]
pub unsafe extern "C" fn fprintf(f: *mut File, fmt: *const u8, mut ap: ...) -> i32 {
    vfprintf(f, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vprintf(fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    vfprintf(file::stdout, fmt, ap)
}
#[no_mangle]
pub unsafe extern "C" fn printf(fmt: *const u8, mut ap: ...) -> i32 {
    vprintf(fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vdprintf(fd: i32, fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    let mut out = FdSink { fd, n: 0 };
    let r = super::vformat(&mut out, super::Format::Narrow(fmt), ap);
    result(r, out.n)
}
#[no_mangle]
pub unsafe extern "C" fn dprintf(fd: i32, fmt: *const u8, mut ap: ...) -> i32 {
    vdprintf(fd, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vasprintf(p: *mut *mut u8, fmt: *const u8, ap: VaList<'_, '_>) -> i32 {
    *p = ptr::null_mut();
    let mut out = AllocSink { body: Text::new() };
    let r = super::vformat(&mut out, super::Format::Narrow(fmt), ap);
    let n = out.body.n;
    if let Err(e) = r {
        return result(Err(e), n);
    }
    if let Err(e) = out.body.byte(0) {
        return result(Err(e), n);
    }
    *p = out.body.p;
    out.body.p = ptr::null_mut();
    result(Ok(()), n)
}
#[no_mangle]
pub unsafe extern "C" fn asprintf(p: *mut *mut u8, fmt: *const u8, mut ap: ...) -> i32 {
    vasprintf(p, fmt, ap.as_va_list())
}
