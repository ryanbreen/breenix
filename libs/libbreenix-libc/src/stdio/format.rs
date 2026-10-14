//! Shared narrow/wide formatting, from C17 7.21.6.1 and 7.29.2.1 and POSIX fprintf.
//! Conversions are written straight to the sink with their lengths measured first, so
//! formatting itself never allocates.
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
    pub(super) fn is_wide(&self) -> bool {
        matches!(self, Self::Wide(_))
    }
}
/// A growable byte buffer for the functions whose result is allocated (asprintf,
/// scanf's %m).
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
}
impl Drop for Text {
    fn drop(&mut self) {
        unsafe {
            crate::free(self.p);
        }
    }
}
fn emit(out: &mut dyn Sink, b: &[u8]) -> Result<(), i32> {
    if b.is_empty() || out.bytes(b) {
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
pub(super) unsafe fn store_count(p: *mut u8, l: Length, n: usize) {
    match l {
        Length::Hh => *(p as *mut i8) = n as i8,
        Length::H => *(p as *mut i16) = n as i16,
        Length::L | Length::Ll | Length::J | Length::Z | Length::T => *(p as *mut i64) = n as i64,
        _ => *(p as *mut i32) = n as i32,
    }
}
/// The text of one conversion after its prefix, written twice: once to measure it
/// and once to emit it.
pub(super) trait Body {
    fn put(&mut self, b: &[u8]) -> Result<(), i32>;
    fn zeros(&mut self, n: usize) -> Result<(), i32>;
}
struct Measure(usize);
impl Measure {
    fn add(&mut self, n: usize) -> Result<(), i32> {
        self.0 = self
            .0
            .checked_add(n)
            .filter(|&n| n <= i32::MAX as usize)
            .ok_or(75)?;
        Ok(())
    }
}
impl Body for Measure {
    fn put(&mut self, b: &[u8]) -> Result<(), i32> {
        self.add(b.len())
    }
    fn zeros(&mut self, n: usize) -> Result<(), i32> {
        self.add(n)
    }
}
struct Emit<'a>(&'a mut dyn Sink);
impl Body for Emit<'_> {
    fn put(&mut self, b: &[u8]) -> Result<(), i32> {
        emit(self.0, b)
    }
    fn zeros(&mut self, n: usize) -> Result<(), i32> {
        padding(self.0, n, b'0')
    }
}
#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub width: usize,
    pub left: bool,
    pub zero: bool,
}
/// Writes prefix and body padded to the field width (in bytes, which equal characters
/// for the ASCII text of numeric conversions).
pub(super) fn field(
    out: &mut dyn Sink,
    layout: Layout,
    prefix: &[u8],
    body: &mut dyn FnMut(&mut dyn Body) -> Result<(), i32>,
) -> Result<(), i32> {
    let mut size = Measure(prefix.len());
    body(&mut size)?;
    let pad = layout.width.saturating_sub(size.0);
    if !layout.left && !layout.zero {
        padding(out, pad, b' ')?
    }
    emit(out, prefix)?;
    if !layout.left && layout.zero {
        padding(out, pad, b'0')?
    }
    body(&mut Emit(out))?;
    if layout.left {
        padding(out, pad, b' ')?
    }
    Ok(())
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
#[derive(Clone, Copy)]
enum WideItem {
    Char(u32, bool),
    Wide(*const u32),
    Narrow(*const u8),
}
/// Visits the wide characters a wide-format %c, %lc, %s or %ls writes, reading no
/// array element beyond those the precision allows (C17 7.29.2.1p8).
unsafe fn each_wide(
    item: WideItem,
    limit: usize,
    f: &mut dyn FnMut(u32) -> Result<(), i32>,
) -> Result<(), i32> {
    match item {
        WideItem::Char(wc, true) => f(wc),
        WideItem::Char(c, false) => {
            let mut state = core::mem::zeroed();
            match crate::locale::decode(&[c as u8], &mut state) {
                crate::locale::Decoded::Char(wc, _) => f(wc),
                _ => Err(84),
            }
        }
        WideItem::Wide(mut p) => {
            let mut n = 0;
            while n < limit && *p != 0 {
                f(*p)?;
                p = p.add(1);
                n += 1;
            }
            Ok(())
        }
        WideItem::Narrow(mut p) => {
            let mut state = core::mem::zeroed();
            let mut n = 0;
            while n < limit && *p != 0 {
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
                f(wc)?;
                n += 1;
            }
            Ok(())
        }
    }
}
unsafe fn integer(
    out: &mut dyn Sink,
    mut layout: Layout,
    spec: &super::args::Spec,
    args: &mut super::args::Arguments<'_, '_>,
) -> Result<(), i32> {
    let c = spec.conversion;
    let signed = c == b'd' || c == b'i';
    let v = if c == b'p' {
        args.pointer(spec) as u64
    } else {
        args.integer(spec)
    };
    let negative = signed && (v as i64) < 0;
    let mut magnitude = if negative { v.wrapping_neg() } else { v };
    let mut prefix = [0u8; 3];
    let mut plen = 0;
    if negative || (signed && (spec.plus || spec.space)) {
        prefix[0] = if negative {
            b'-'
        } else if spec.plus {
            b'+'
        } else {
            b' '
        };
        plen = 1;
    }
    let base = match c {
        b'o' => 8,
        b'x' | b'X' | b'p' => 16,
        _ => 10,
    };
    if c == b'p' || (spec.alternate && magnitude != 0 && base == 16) {
        prefix[plen..plen + 2].copy_from_slice(if c == b'X' { b"0X" } else { b"0x" });
        plen += 2;
    }
    let alphabet = if c == b'X' {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut digits = [0u8; 64];
    let mut start = digits.len();
    if magnitude != 0 || spec.precision != Some(0) {
        loop {
            start -= 1;
            digits[start] = alphabet[(magnitude % base) as usize];
            magnitude /= base;
            if magnitude == 0 {
                break;
            }
        }
    }
    let digits = &digits[start..];
    let mut zeros = spec.precision.unwrap_or(1).saturating_sub(digits.len());
    if spec.alternate && base == 8 && zeros == 0 && digits.first() != Some(&b'0') {
        zeros = 1;
    }
    if spec.precision.is_some() {
        layout.zero = false;
    }
    field(out, layout, &prefix[..plen], &mut |b| {
        b.zeros(zeros)?;
        b.put(digits)
    })
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
    loop {
        let start = i;
        while fmt.at(i) != 0 && fmt.at(i) != 37 {
            if fmt.is_wide() && !out.wide(fmt.at(i)) {
                return Err(crate::pthread::errno());
            }
            i += 1;
        }
        if let Format::Narrow(p) = fmt {
            emit(&mut out, slice::from_raw_parts(p.add(start), i - start))?;
        }
        if fmt.at(i) == 0 {
            return Ok(());
        }
        i += 1;
        if fmt.at(i) == 37 {
            i += 1;
            emit(&mut out, b"%")?;
            continue;
        }
        let mut spec = super::args::Spec::parse(&fmt, &mut i)?;
        args.widths(&mut spec);
        if spec.width > i32::MAX as usize {
            return Err(75);
        }
        let layout = Layout {
            width: spec.width,
            left: spec.left,
            zero: spec.zero,
        };
        let text = Layout {
            zero: false,
            ..layout
        };
        let precision = spec.precision;
        let limit = precision.unwrap_or(usize::MAX);
        let l = spec.length;
        let c = spec.conversion;
        match c {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' => {
                integer(&mut out, layout, &spec, &mut args)?;
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                let v = args.floating(&spec);
                let sign: &[u8] = if v.negative {
                    b"-"
                } else if spec.plus {
                    b"+"
                } else if spec.space {
                    b" "
                } else {
                    b""
                };
                super::float::convert(&mut out, layout, sign, v, c, precision, spec.alternate)?;
            }
            b'c' | b's' if fmt.is_wide() => {
                let item = if c == b'c' {
                    WideItem::Char(args.integer(&spec) as u32, l == Length::L)
                } else if l == Length::L {
                    WideItem::Wide(args.pointer(&spec) as *const u32)
                } else {
                    let p = args.pointer(&spec) as *const u8;
                    WideItem::Narrow(if p.is_null() {
                        c"(null)".as_ptr() as *const u8
                    } else {
                        p
                    })
                };
                let mut count = 0usize;
                each_wide(item, limit, &mut |_| {
                    count += 1;
                    Ok(())
                })?;
                let pad = spec.width.saturating_sub(count);
                if !spec.left {
                    padding(&mut out, pad, b' ')?
                }
                each_wide(item, limit, &mut |wc| {
                    if out.wide(wc) {
                        Ok(())
                    } else {
                        Err(crate::pthread::errno())
                    }
                })?;
                if spec.left {
                    padding(&mut out, pad, b' ')?
                }
            }
            b'c' => {
                let wc = args.integer(&spec) as u32;
                let mut b = [wc as u8; 4];
                let n = if l == Length::L {
                    crate::locale::encode(wc, &mut b).ok_or(84)?
                } else {
                    1
                };
                field(&mut out, text, b"", &mut |body| body.put(&b[..n]))?;
            }
            b's' if l == Length::L => {
                let p = args.pointer(&spec) as *const u32;
                field(&mut out, text, b"", &mut |body| {
                    let mut q = p;
                    let mut used = 0;
                    while used < limit && *q != 0 {
                        let mut b = [0; 4];
                        let n = crate::locale::encode(*q, &mut b).ok_or(84)?;
                        if n > limit - used {
                            break;
                        }
                        body.put(&b[..n])?;
                        used += n;
                        q = q.add(1);
                    }
                    Ok(())
                })?;
            }
            b's' => {
                let mut p = args.pointer(&spec) as *const u8;
                if p.is_null() {
                    p = c"(null)".as_ptr() as *const u8;
                }
                let mut n = 0;
                while n < limit && *p.add(n) != 0 {
                    n += 1;
                }
                let s = slice::from_raw_parts(p, n);
                field(&mut out, text, b"", &mut |body| body.put(s))?;
            }
            b'n' => store_count(args.pointer(&spec), l, out.n),
            _ => return Err(22),
        }
    }
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
        encoded(self, wc)
    }
}
fn encoded(out: &mut dyn Sink, wc: u32) -> bool {
    let mut b = [0; 4];
    match crate::locale::encode(wc, &mut b) {
        Some(n) => out.bytes(&b[..n]),
        None => {
            crate::pthread::set_errno(84);
            false
        }
    }
}
/// Output of one call on a locked stream. An unbuffered stream gets the call's text
/// staged here, so it reaches the file in one write per 512 bytes instead of one per
/// piece (C17 7.21.3p3 lets unbuffered characters be transmitted as a block).
pub(crate) struct StreamOut {
    f: *mut File,
    staged: bool,
    buf: [u8; 512],
    len: usize,
    pub(crate) written: usize,
    failed: bool,
}
impl StreamOut {
    pub(crate) unsafe fn new(f: *mut File) -> Self {
        Self {
            f,
            staged: file::unbuffered(f),
            buf: [0; 512],
            len: 0,
            written: 0,
            failed: false,
        }
    }
    fn pass(&mut self, b: &[u8]) -> bool {
        let n = unsafe { file::write_locked(self.f, b) };
        self.written += n;
        if n < b.len() {
            self.failed = true;
        }
        !self.failed
    }
    /// Hands the staged text to the stream; false once any byte was refused.
    pub(crate) fn finish(&mut self) -> bool {
        if self.len > 0 && !self.failed {
            let len = self.len;
            self.len = 0;
            let buf = self.buf;
            self.pass(&buf[..len]);
        }
        !self.failed
    }
}
impl Sink for StreamOut {
    fn bytes(&mut self, b: &[u8]) -> bool {
        if self.failed {
            return false;
        }
        if !self.staged {
            return self.pass(b);
        }
        if self.len + b.len() > self.buf.len() && !self.finish() {
            return false;
        }
        if b.len() >= self.buf.len() {
            return self.pass(b);
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        true
    }
    fn wide(&mut self, wc: u32) -> bool {
        encoded(self, wc)
    }
}
/// dprintf's output, written in 512-byte blocks.
struct FdSink {
    fd: i32,
    buf: [u8; 512],
    len: usize,
    n: usize,
}
impl FdSink {
    fn drain(&mut self) -> bool {
        let mut i = 0;
        while i < self.len {
            let n = unsafe { crate::write(self.fd, self.buf.as_ptr().add(i), self.len - i) };
            if n < 0 && crate::pthread::errno() == 4 {
                continue;
            }
            if n <= 0 {
                if n == 0 {
                    crate::pthread::set_errno(5);
                }
                return false;
            }
            i += n as usize;
            self.n += n as usize;
        }
        self.len = 0;
        true
    }
}
impl Sink for FdSink {
    fn bytes(&mut self, b: &[u8]) -> bool {
        for chunk in b.chunks(self.buf.len()) {
            if self.len + chunk.len() > self.buf.len() && !self.drain() {
                return false;
            }
            self.buf[self.len..self.len + chunk.len()].copy_from_slice(chunk);
            self.len += chunk.len();
        }
        true
    }
    fn wide(&mut self, wc: u32) -> bool {
        encoded(self, wc)
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
        encoded(self, wc)
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
    let mut out = StreamOut::new(f);
    let mut r = super::vformat(&mut out, super::Format::Narrow(fmt), ap);
    if !out.finish() && r.is_ok() {
        r = Err(crate::pthread::errno());
    }
    if r.is_err() {
        file::set_error(f);
    }
    file::unlock(f);
    result(r, out.written)
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
    let mut out = FdSink {
        fd,
        buf: [0; 512],
        len: 0,
        n: 0,
    };
    let mut r = super::vformat(&mut out, super::Format::Narrow(fmt), ap);
    if r.is_ok() && !out.drain() {
        r = Err(crate::pthread::errno());
    }
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
