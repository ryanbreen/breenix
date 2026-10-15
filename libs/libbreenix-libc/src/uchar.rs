//! <uchar.h>: ISO C17 7.28 Unicode utilities, written from the standard over this
//! library's mbrtowc and wcrtomb.
//!
//! A wchar_t holds a Unicode code point in the UTF-8 locale (and a byte's own value in
//! the C locale), so the char32_t conversions are mbrtowc's and wcrtomb's own. A character
//! beyond the Basic Multilingual Plane is two char16_t: mbrtoc16 returns the high
//! surrogate first and keeps the low one in the state, which the next call returns as
//! (size_t)-3; c16rtomb keeps a high surrogate in the state until its low one arrives.

use core::ptr::{addr_of_mut, null_mut};

/// mbstate_t, as locale::MbState lays it out.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct State {
    value: u32,
    expected: u8,
    seen: u8,
    lead: u8,
    reserved: u8,
}

const _: () = assert!(core::mem::size_of::<State>() == core::mem::size_of::<crate::locale::MbState>());

const INITIAL: State = State { value: 0, expected: 0, seen: 0, lead: 0, reserved: 0 };
/// The halves of a surrogate pair waiting in a state. The decoder never sets these
/// `expected` values, and mbsinit reports a state holding one as not initial.
const PENDING_LOW: u8 = 0xff;
const PENDING_HIGH: u8 = 0xfe;

const EILSEQ: i32 = 84;
const INVALID: usize = usize::MAX;
const STORED: usize = usize::MAX - 2;

// ISO C gives each function its own internal state for a null `ps`.
static mut C16_READ: State = INITIAL;
static mut C16_WRITE: State = INITIAL;
static mut C32_READ: State = INITIAL;
static mut C32_WRITE: State = INITIAL;

extern "C" {
    fn mbrtowc(wc: *mut u32, s: *const u8, n: usize, ps: *mut State) -> usize;
    fn wcrtomb(s: *mut u8, wc: u32, ps: *mut State) -> usize;
}

#[no_mangle]
pub unsafe extern "C" fn mbrtoc16(pc16: *mut u16, s: *const u8, n: usize, ps: *mut State) -> usize {
    let st = if ps.is_null() { addr_of_mut!(C16_READ) } else { ps };
    // A null s is mbrtoc16(NULL, "", 1, ps).
    let (pc16, s, n) = if s.is_null() { (null_mut(), b"\0".as_ptr(), 1) } else { (pc16, s, n) };
    if (*st).expected == PENDING_LOW {
        let low = (*st).value as u16;
        *st = INITIAL;
        if !pc16.is_null() {
            *pc16 = low;
        }
        return STORED;
    }
    let mut wc = 0u32;
    let r = mbrtowc(&mut wc, s, n, st);
    if r >= usize::MAX - 1 {
        return r;
    }
    let unit = if wc > 0xffff {
        let v = wc - 0x10000;
        (*st).expected = PENDING_LOW;
        (*st).value = 0xdc00 + (v & 0x3ff);
        0xd800 + (v >> 10)
    } else {
        wc
    };
    if !pc16.is_null() {
        *pc16 = unit as u16;
    }
    r
}

#[no_mangle]
pub unsafe extern "C" fn c16rtomb(s: *mut u8, c16: u16, ps: *mut State) -> usize {
    let st = if ps.is_null() { addr_of_mut!(C16_WRITE) } else { ps };
    // A null s is c16rtomb(buf, 0, ps) with an internal buffer.
    let mut buf = [0u8; 4];
    let (s, c) = if s.is_null() { (buf.as_mut_ptr(), 0u32) } else { (s, c16 as u32) };
    let is_high = (0xd800..0xdc00).contains(&c);
    let is_low = (0xdc00..0xe000).contains(&c);
    if (*st).expected == PENDING_HIGH {
        let high = (*st).value;
        *st = INITIAL;
        if !is_low {
            crate::pthread::set_errno(EILSEQ);
            return INVALID;
        }
        let mut scratch = INITIAL;
        return wcrtomb(s, 0x10000 + ((high - 0xd800) << 10) + (c - 0xdc00), &mut scratch);
    }
    if is_high {
        (*st).expected = PENDING_HIGH;
        (*st).value = c;
        return 0;
    }
    if is_low {
        crate::pthread::set_errno(EILSEQ);
        return INVALID;
    }
    wcrtomb(s, c, st)
}

#[no_mangle]
pub unsafe extern "C" fn mbrtoc32(pc32: *mut u32, s: *const u8, n: usize, ps: *mut State) -> usize {
    mbrtowc(pc32, s, n, if ps.is_null() { addr_of_mut!(C32_READ) } else { ps })
}

#[no_mangle]
pub unsafe extern "C" fn c32rtomb(s: *mut u8, c32: u32, ps: *mut State) -> usize {
    wcrtomb(s, c32, if ps.is_null() { addr_of_mut!(C32_WRITE) } else { ps })
}
