//! <string.h> and <strings.h> (ISO C17 7.24, POSIX.1-2024), plus the GNU and BSD
//! extensions BusyBox expects.
//!
//! These are strong definitions; compiler_builtins' weak copies of memcpy, memmove,
//! memset, memcmp, bcmp and strlen lose to them at link time. memcpy, memmove and memset
//! are assembly (mem_asm.rs) so the optimizer cannot recognise their loops as calls to
//! themselves. The remaining loops compare or search, which LLVM does not turn into calls.

mod errtext;
mod mem_asm;

pub(crate) use errtext::error_text;
pub(crate) use mem_asm::{memcpy, memset};

use core::ptr::null_mut;

#[inline]
unsafe fn byte(p: *const u8, i: usize) -> u8 {
    *p.add(i)
}

fn lower(c: u8) -> u8 {
    if c.is_ascii_uppercase() { c + 32 } else { c }
}

// ----------------------------------------------------------------------------
// Memory blocks
// ----------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let (x, y) = (byte(a, i), byte(b, i));
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    memcmp(a, b, n)
}

#[no_mangle]
pub unsafe extern "C" fn memchr(s: *const u8, c: i32, n: usize) -> *mut u8 {
    let c = c as u8;
    let mut i = 0;
    while i < n {
        if byte(s, i) == c {
            return s.add(i) as *mut u8;
        }
        i += 1;
    }
    null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn memrchr(s: *const u8, c: i32, n: usize) -> *mut u8 {
    let c = c as u8;
    let mut i = n;
    while i > 0 {
        i -= 1;
        if byte(s, i) == c {
            return s.add(i) as *mut u8;
        }
    }
    null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn memccpy(dst: *mut u8, src: *const u8, c: i32, n: usize) -> *mut u8 {
    let c = c as u8;
    let mut i = 0;
    while i < n {
        let b = byte(src, i);
        *dst.add(i) = b;
        i += 1;
        if b == c {
            return dst.add(i);
        }
    }
    null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn mempcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    mem_asm::memcpy(dst, src, n).add(n)
}

#[no_mangle]
pub unsafe extern "C" fn memmem(h: *const u8, hn: usize, n: *const u8, nn: usize) -> *mut u8 {
    if nn == 0 {
        return h as *mut u8;
    }
    if nn > hn {
        return null_mut();
    }
    let mut i = 0;
    while i + nn <= hn {
        if byte(h, i) == byte(n, 0) && memcmp(h.add(i), n, nn) == 0 {
            return h.add(i) as *mut u8;
        }
        i += 1;
    }
    null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn bzero(s: *mut u8, n: usize) {
    mem_asm::memset(s, 0, n);
}

#[no_mangle]
pub unsafe extern "C" fn explicit_bzero(s: *mut u8, n: usize) {
    mem_asm::memset(s, 0, n);
    // The stores must happen even though nothing reads them afterwards.
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

#[no_mangle]
pub unsafe extern "C" fn bcopy(src: *const u8, dst: *mut u8, n: usize) {
    mem_asm::memmove(dst, src, n);
}

// ----------------------------------------------------------------------------
// Lengths, copies and concatenation
// ----------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut i = 0;
    while byte(s, i) != 0 {
        i += 1;
    }
    i
}

#[no_mangle]
pub unsafe extern "C" fn strnlen(s: *const u8, max: usize) -> usize {
    let mut i = 0;
    while i < max && byte(s, i) != 0 {
        i += 1;
    }
    i
}

#[no_mangle]
pub unsafe extern "C" fn stpcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    let n = strlen(src);
    mem_asm::memcpy(dst, src, n + 1);
    dst.add(n)
}

#[no_mangle]
pub unsafe extern "C" fn strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    stpcpy(dst, src);
    dst
}

#[no_mangle]
pub unsafe extern "C" fn stpncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let len = strnlen(src, n);
    mem_asm::memcpy(dst, src, len);
    mem_asm::memset(dst.add(len), 0, n - len);
    dst.add(len)
}

#[no_mangle]
pub unsafe extern "C" fn strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    stpncpy(dst, src, n);
    dst
}

#[no_mangle]
pub unsafe extern "C" fn strcat(dst: *mut u8, src: *const u8) -> *mut u8 {
    stpcpy(dst.add(strlen(dst)), src);
    dst
}

#[no_mangle]
pub unsafe extern "C" fn strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let end = dst.add(strlen(dst));
    let len = strnlen(src, n);
    mem_asm::memcpy(end, src, len);
    *end.add(len) = 0;
    dst
}

/// Copies at most size-1 bytes and always terminates when size > 0; returns strlen(src).
#[no_mangle]
pub unsafe extern "C" fn strlcpy(dst: *mut u8, src: *const u8, size: usize) -> usize {
    let len = strlen(src);
    if size > 0 {
        let n = len.min(size - 1);
        mem_asm::memcpy(dst, src, n);
        *dst.add(n) = 0;
    }
    len
}

/// Appends to a terminated dst of buffer size `size`; returns the length it tried to make.
#[no_mangle]
pub unsafe extern "C" fn strlcat(dst: *mut u8, src: *const u8, size: usize) -> usize {
    let dlen = strnlen(dst, size);
    if dlen == size {
        return size + strlen(src);
    }
    dlen + strlcpy(dst.add(dlen), src, size - dlen)
}

#[no_mangle]
pub unsafe extern "C" fn strdup(s: *const u8) -> *mut u8 {
    strndup(s, usize::MAX)
}

#[no_mangle]
pub unsafe extern "C" fn strndup(s: *const u8, n: usize) -> *mut u8 {
    let len = strnlen(s, n);
    let p = crate::stdlib::malloc::malloc(len + 1);
    if !p.is_null() {
        mem_asm::memcpy(p, s, len);
        *p.add(len) = 0;
    }
    p
}

// ----------------------------------------------------------------------------
// Comparison
// ----------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn strcmp(a: *const u8, b: *const u8) -> i32 {
    strncmp(a, b, usize::MAX)
}

#[no_mangle]
pub unsafe extern "C" fn strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let (x, y) = (byte(a, i), byte(b, i));
        if x != y || x == 0 {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn strcasecmp(a: *const u8, b: *const u8) -> i32 {
    strncasecmp(a, b, usize::MAX)
}

#[no_mangle]
pub unsafe extern "C" fn strncasecmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let (x, y) = (lower(byte(a, i)), lower(byte(b, i)));
        if x != y || x == 0 {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

/// In the C and POSIX locales collation is byte order; in C.UTF-8 it is code point order,
/// which UTF-8 preserves under byte comparison.
#[no_mangle]
pub unsafe extern "C" fn strcoll(a: *const u8, b: *const u8) -> i32 {
    strcmp(a, b)
}

/// The transformed string is the string itself, since strcoll is strcmp.
#[no_mangle]
pub unsafe extern "C" fn strxfrm(dst: *mut u8, src: *const u8, n: usize) -> usize {
    let len = strlen(src);
    if len < n {
        mem_asm::memcpy(dst, src, len + 1);
    }
    len
}

// ----------------------------------------------------------------------------
// Search
// ----------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn strchrnul(s: *const u8, c: i32) -> *mut u8 {
    let c = c as u8;
    let mut i = 0;
    loop {
        let b = byte(s, i);
        if b == c || b == 0 {
            return s.add(i) as *mut u8;
        }
        i += 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn strchr(s: *const u8, c: i32) -> *mut u8 {
    let p = strchrnul(s, c);
    if *p == c as u8 { p } else { null_mut() }
}

#[no_mangle]
pub unsafe extern "C" fn strrchr(s: *const u8, c: i32) -> *mut u8 {
    memrchr(s, c, strlen(s) + 1)
}

#[no_mangle]
pub unsafe extern "C" fn index(s: *const u8, c: i32) -> *mut u8 {
    strchr(s, c)
}

#[no_mangle]
pub unsafe extern "C" fn rindex(s: *const u8, c: i32) -> *mut u8 {
    strrchr(s, c)
}

/// A 256-bit membership set of the bytes of a terminated string.
struct ByteSet([u64; 4]);

impl ByteSet {
    unsafe fn of(set: *const u8) -> Self {
        let mut bits = [0u64; 4];
        let mut i = 0;
        loop {
            let b = byte(set, i);
            if b == 0 {
                break;
            }
            bits[(b >> 6) as usize] |= 1 << (b & 63);
            i += 1;
        }
        ByteSet(bits)
    }
    fn has(&self, b: u8) -> bool {
        self.0[(b >> 6) as usize] & (1 << (b & 63)) != 0
    }
}

#[no_mangle]
pub unsafe extern "C" fn strspn(s: *const u8, accept: *const u8) -> usize {
    let set = ByteSet::of(accept);
    let mut i = 0;
    while byte(s, i) != 0 && set.has(byte(s, i)) {
        i += 1;
    }
    i
}

#[no_mangle]
pub unsafe extern "C" fn strcspn(s: *const u8, reject: *const u8) -> usize {
    let set = ByteSet::of(reject);
    let mut i = 0;
    while byte(s, i) != 0 && !set.has(byte(s, i)) {
        i += 1;
    }
    i
}

#[no_mangle]
pub unsafe extern "C" fn strpbrk(s: *const u8, accept: *const u8) -> *mut u8 {
    let p = s.add(strcspn(s, accept));
    if *p == 0 { null_mut() } else { p as *mut u8 }
}

#[no_mangle]
pub unsafe extern "C" fn strstr(h: *const u8, n: *const u8) -> *mut u8 {
    memmem(h, strlen(h), n, strlen(n))
}

#[no_mangle]
pub unsafe extern "C" fn strcasestr(h: *const u8, n: *const u8) -> *mut u8 {
    let nn = strlen(n);
    let mut p = h;
    loop {
        if strncasecmp(p, n, nn) == 0 {
            return p as *mut u8;
        }
        if *p == 0 {
            return null_mut();
        }
        p = p.add(1);
    }
}

#[no_mangle]
pub unsafe extern "C" fn strtok_r(s: *mut u8, delim: *const u8, save: *mut *mut u8) -> *mut u8 {
    let mut p = if s.is_null() { *save } else { s };
    if p.is_null() {
        return null_mut();
    }
    p = p.add(strspn(p, delim));
    if *p == 0 {
        *save = null_mut();
        return null_mut();
    }
    let end = p.add(strcspn(p, delim));
    if *end == 0 {
        *save = null_mut();
    } else {
        *end = 0;
        *save = end.add(1);
    }
    p
}

static mut STRTOK_SAVE: *mut u8 = null_mut();

#[no_mangle]
pub unsafe extern "C" fn strtok(s: *mut u8, delim: *const u8) -> *mut u8 {
    strtok_r(s, delim, core::ptr::addr_of_mut!(STRTOK_SAVE))
}

/// BSD strsep: splits at the first delimiter, which may leave empty tokens.
#[no_mangle]
pub unsafe extern "C" fn strsep(sp: *mut *mut u8, delim: *const u8) -> *mut u8 {
    let s = *sp;
    if s.is_null() {
        return null_mut();
    }
    let end = s.add(strcspn(s, delim));
    if *end == 0 {
        *sp = null_mut();
    } else {
        *end = 0;
        *sp = end.add(1);
    }
    s
}

// ----------------------------------------------------------------------------
// strings.h bit search
// ----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn ffs(i: i32) -> i32 {
    ffsll(i as u32 as i64)
}

#[no_mangle]
pub extern "C" fn ffsl(i: i64) -> i32 {
    ffsll(i)
}

#[no_mangle]
pub extern "C" fn ffsll(i: i64) -> i32 {
    if i == 0 { 0 } else { i.trailing_zeros() as i32 + 1 }
}

// ----------------------------------------------------------------------------
// Error and signal text
// ----------------------------------------------------------------------------

/// Writes "Unknown error <n>" (or another prefix) into buf, terminated.
fn unknown_text(prefix: &[u8], n: i32, buf: &mut [u8; 32]) -> usize {
    let mut len = prefix.len();
    buf[..len].copy_from_slice(prefix);
    let mut digits = [0u8; 11];
    let mut d = 0;
    let mut v = (n as i64).unsigned_abs();
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        d += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if n < 0 {
        buf[len] = b'-';
        len += 1;
    }
    while d > 0 {
        d -= 1;
        buf[len] = digits[d];
        len += 1;
    }
    buf[len] = 0;
    len
}

static mut STRERROR_BUF: [u8; 32] = [0; 32];

#[no_mangle]
pub unsafe extern "C" fn strerror(errnum: i32) -> *mut u8 {
    match errtext::error_cstr(errnum) {
        Some(t) => t.as_ptr() as *mut u8,
        None => {
            let buf = &mut *core::ptr::addr_of_mut!(STRERROR_BUF);
            unknown_text(b"Unknown error ", errnum, buf);
            buf.as_mut_ptr()
        }
    }
}

/// XSI strerror_r: 0, EINVAL for an unknown number (the buffer still says so), or
/// ERANGE when the text does not fit (the buffer holds as much as fits, terminated).
#[no_mangle]
pub unsafe extern "C" fn strerror_r(errnum: i32, buf: *mut u8, len: usize) -> i32 {
    let mut tmp = [0u8; 32];
    let (text, status): (&[u8], i32) = match error_text(errnum) {
        Some(t) => (t.as_bytes(), 0),
        None => {
            let n = unknown_text(b"Unknown error ", errnum, &mut tmp);
            (&tmp[..n], super::stdlib::EINVAL)
        }
    };
    if len == 0 {
        return super::stdlib::ERANGE;
    }
    let n = text.len().min(len - 1);
    mem_asm::memcpy(buf, text.as_ptr(), n);
    *buf.add(n) = 0;
    if n < text.len() { super::stdlib::ERANGE } else { status }
}

/// The name glibc-targeted Rust std uses for the XSI strerror_r.
#[no_mangle]
pub unsafe extern "C" fn __xpg_strerror_r(errnum: i32, buf: *mut u8, len: usize) -> i32 {
    strerror_r(errnum, buf, len)
}

static mut STRSIGNAL_BUF: [u8; 32] = [0; 32];

#[no_mangle]
pub unsafe extern "C" fn strsignal(sig: i32) -> *mut u8 {
    match errtext::signal_text(sig) {
        Some(t) => t.as_ptr() as *mut u8,
        None => {
            let buf = &mut *core::ptr::addr_of_mut!(STRSIGNAL_BUF);
            unknown_text(b"Unknown signal ", sig, buf);
            buf.as_mut_ptr()
        }
    }
}

// ----------------------------------------------------------------------------
// The locale_t forms: every Breenix locale collates by code point, folds case only
// for ASCII, and reports errors in the same text, so the locale changes nothing.
// ----------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn strcoll_l(a: *const u8, b: *const u8, _loc: *mut core::ffi::c_void) -> i32 {
    strcoll(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn strxfrm_l(dst: *mut u8, src: *const u8, n: usize, _loc: *mut core::ffi::c_void) -> usize {
    strxfrm(dst, src, n)
}

#[no_mangle]
pub unsafe extern "C" fn strerror_l(errnum: i32, _loc: *mut core::ffi::c_void) -> *mut u8 {
    strerror(errnum)
}

#[no_mangle]
pub unsafe extern "C" fn strcasecmp_l(a: *const u8, b: *const u8, _loc: *mut core::ffi::c_void) -> i32 {
    strcasecmp(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn strncasecmp_l(a: *const u8, b: *const u8, n: usize, _loc: *mut core::ffi::c_void) -> i32 {
    strncasecmp(a, b, n)
}
