//! The number scanners shared by strto* (here), the scanf family (stdio) and wcsto*
//! (locale): the subject sequences of ISO C17 7.22.1.3 and 7.22.1.4.
//!
//! A scanner reads through `Chars`, which yields characters as code points, so narrow and
//! wide sources share it. Neither scanner skips leading white space; callers do, because
//! scanf does not count it against the field width. `width` bounds the characters a
//! scanner consumes (usize::MAX for none).
//!
//! When a scanner has consumed a prefix of a longer form that then fails ("0x" with no hex
//! digit, "1e+" with no exponent digit, "nan(" with no closing parenthesis) it rewinds to
//! the end of the shorter valid subject. A source that cannot rewind (a stream) returns
//! false, and the scan then fails as a matching failure, as scanf requires.

#[allow(unused_imports)] // Published cross-lane path (stdio's perror, regerror); consumers land separately.
pub(crate) use crate::string::error_text;

use super::floatconv::{self, Converted, Format};
use super::EINVAL;

pub(crate) trait Chars {
    fn peek(&mut self) -> Option<u32>;
    fn bump(&mut self);
    fn pos(&self) -> usize;
    fn rewind(&mut self, pos: usize) -> bool;
}

pub(crate) struct IntScan {
    /// The magnitude, saturated at u64::MAX when `overflow` is set.
    pub magnitude: u64,
    pub negative: bool,
    pub overflow: bool,
    /// False when no subject sequence was found.
    pub digits: bool,
}

pub(crate) struct FloatScan {
    pub value: f64,
    /// The result overflowed, or underflowed (tiny and inexact): ERANGE for strtod.
    pub range_error: bool,
}

/// A C string as a source of characters.
pub(crate) struct CStrChars {
    p: *const u8,
    i: usize,
}

impl CStrChars {
    /// # Safety
    /// `p` must point to a NUL-terminated string that outlives the scanner.
    pub(crate) unsafe fn new(p: *const u8) -> Self {
        CStrChars { p, i: 0 }
    }
}

impl Chars for CStrChars {
    fn peek(&mut self) -> Option<u32> {
        let c = unsafe { *self.p.add(self.i) };
        if c == 0 { None } else { Some(c as u32) }
    }
    fn bump(&mut self) {
        self.i += 1;
    }
    fn pos(&self) -> usize {
        self.i
    }
    fn rewind(&mut self, pos: usize) -> bool {
        self.i = pos;
        true
    }
}

/// White space in the C, POSIX and C.UTF-8 locales: space and \t \n \v \f \r.
pub(crate) fn is_space(c: u32) -> bool {
    c == 0x20 || (0x09..=0x0d).contains(&c)
}

pub(crate) fn skip_space(s: &mut dyn Chars) {
    while s.peek().is_some_and(is_space) {
        s.bump();
    }
}

fn digit_value(c: u32) -> u32 {
    match c {
        0x30..=0x39 => c - 0x30,
        0x41..=0x5a => c - 0x41 + 10,
        0x61..=0x7a => c - 0x61 + 10,
        _ => 99,
    }
}

/// A source limited to `width` characters.
struct Field<'a> {
    s: &'a mut dyn Chars,
    left: usize,
}

impl Field<'_> {
    fn peek(&mut self) -> Option<u32> {
        if self.left == 0 { None } else { self.s.peek() }
    }
    fn bump(&mut self) {
        self.s.bump();
        self.left -= 1;
    }
    fn take(&mut self, c: u8) -> bool {
        if self.peek() == Some(c as u32) {
            self.bump();
            true
        } else {
            false
        }
    }
    /// Takes c in either case.
    fn take_ci(&mut self, c: u8) -> bool {
        self.take(c) || self.take(c.to_ascii_uppercase())
    }
    fn pos(&self) -> usize {
        self.s.pos()
    }
    fn rewind(&mut self, pos: usize, left: usize) -> bool {
        if self.s.rewind(pos) {
            self.left = left;
            true
        } else {
            false
        }
    }
}

/// The integer subject sequence for `base` (0 or 2..=36); Err(EINVAL) for any other base.
pub(crate) fn scan_int(s: &mut dyn Chars, base: u32, width: usize) -> Result<IntScan, i32> {
    if base == 1 || base > 36 {
        return Err(EINVAL);
    }
    let mut f = Field { s, left: width };
    let mut r = IntScan { magnitude: 0, negative: false, overflow: false, digits: false };
    if f.take(b'-') {
        r.negative = true;
    } else {
        f.take(b'+');
    }
    let mut base = base;
    if (base == 0 || base == 16) && f.take(b'0') {
        r.digits = true;
        let (after_zero, left) = (f.pos(), f.left);
        if f.take_ci(b'x') {
            if f.peek().is_some_and(|c| digit_value(c) < 16) {
                base = 16;
            } else {
                // "0x" with no hex digit: the subject is the "0".
                if !f.rewind(after_zero, left) {
                    r.digits = false;
                }
                return Ok(r);
            }
        } else if base == 0 {
            base = 8;
        }
    } else if base == 0 {
        base = 10;
    }
    while let Some(c) = f.peek() {
        let d = digit_value(c);
        if d >= base {
            break;
        }
        f.bump();
        r.digits = true;
        match r.magnitude.checked_mul(base as u64).and_then(|m| m.checked_add(d as u64)) {
            Some(m) if !r.overflow => r.magnitude = m,
            _ => {
                r.overflow = true;
                r.magnitude = u64::MAX;
            }
        }
    }
    Ok(r)
}

/// Significant decimal digits kept; any further digit only matters through whether it
/// is zero. 800 exceeds the 767 digits that decide any double or float rounding.
const MAX_DIGITS: usize = 800;

/// Matches the rest of `word` (lowercase) case-insensitively.
fn take_word(f: &mut Field, word: &[u8]) -> bool {
    for &c in word {
        if !f.take_ci(c) {
            return false;
        }
    }
    true
}

/// A decimal exponent's digits, saturated far beyond any format's range.
fn exponent_digits(f: &mut Field) -> Option<i64> {
    let neg = f.take(b'-') || {
        f.take(b'+');
        false
    };
    let mut any = false;
    let mut v = 0i64;
    while let Some(c) = f.peek() {
        let d = digit_value(c);
        if d >= 10 {
            break;
        }
        f.bump();
        any = true;
        v = (v * 10 + d as i64).min(1_000_000_000);
    }
    if !any {
        return None;
    }
    Some(if neg { -v } else { v })
}

/// Scans a floating subject sequence and rounds it into `fmt`; None when there is none.
pub(crate) fn scan_float(s: &mut dyn Chars, width: usize, fmt: &Format) -> Option<Converted> {
    let mode = floatconv::current_round();
    let mut f = Field { s, left: width };
    let neg = f.take(b'-') || {
        f.take(b'+');
        false
    };
    let special = |bits| Some(Converted { bits, inexact: false, overflow: false, underflow: false });
    match f.peek() {
        Some(0x69) | Some(0x49) => {
            if !take_word(&mut f, b"inf") {
                return None;
            }
            let (pos, left) = (f.pos(), f.left);
            if f.peek().is_some_and(|c| c == 0x69 || c == 0x49) && !take_word(&mut f, b"inity") && !f.rewind(pos, left) {
                return None;
            }
            return special(fmt.infinity(neg));
        }
        Some(0x6e) | Some(0x4e) => {
            if !take_word(&mut f, b"nan") {
                return None;
            }
            let (pos, left) = (f.pos(), f.left);
            if f.take(b'(') {
                loop {
                    match f.peek() {
                        Some(c) if c == 0x5f || digit_value(c) < 36 => f.bump(),
                        Some(0x29) => {
                            f.bump();
                            break;
                        }
                        _ => {
                            if !f.rewind(pos, left) {
                                return None;
                            }
                            break;
                        }
                    }
                }
            }
            return special(fmt.quiet_nan(neg));
        }
        _ => {}
    }

    // Hexadecimal: "0x", then hex digits with an optional point, then an optional
    // binary exponent.
    if f.take(b'0') {
        let (after_zero, left) = (f.pos(), f.left);
        if f.take_ci(b'x') {
            if let Some(c) = scan_hex(&mut f, neg, fmt, mode) {
                return Some(c);
            }
            if !f.rewind(after_zero, left) {
                return None;
            }
            return special(fmt.zero(neg));
        }
        // The zero was a decimal digit; scan the rest as decimal with it counted.
        return scan_decimal(&mut f, neg, true, fmt, mode);
    }
    scan_decimal(&mut f, neg, false, fmt, mode)
}

fn scan_hex(f: &mut Field, neg: bool, fmt: &Format, mode: floatconv::Round) -> Option<Converted> {
    let mut mant = 0u128;
    let mut e2 = 0i64;
    let mut sticky = false;
    let mut any = false;
    let mut point = false;
    loop {
        match f.peek() {
            Some(0x2e) if !point => {
                point = true;
                f.bump();
            }
            Some(c) if digit_value(c) < 16 => {
                f.bump();
                any = true;
                let d = digit_value(c);
                if mant >> 124 == 0 {
                    mant = mant << 4 | d as u128;
                    if point {
                        e2 -= 4;
                    }
                } else {
                    sticky |= d != 0;
                    if !point {
                        e2 += 4;
                    }
                }
            }
            _ => break,
        }
    }
    if !any {
        return None;
    }
    let (pos, left) = (f.pos(), f.left);
    if f.take_ci(b'p') {
        match exponent_digits(f) {
            Some(e) => e2 += e,
            None => {
                if !f.rewind(pos, left) {
                    return None;
                }
            }
        }
    }
    Some(floatconv::from_binary(neg, mant, e2, sticky, fmt, mode))
}

fn scan_decimal(f: &mut Field, neg: bool, had_zero: bool, fmt: &Format, mode: floatconv::Round) -> Option<Converted> {
    let mut digits = [0u8; MAX_DIGITS];
    let mut n = 0;
    let mut e10 = 0i64;
    let mut sticky = false;
    let mut any = had_zero;
    let mut point = false;
    loop {
        match f.peek() {
            Some(0x2e) if !point => {
                point = true;
                f.bump();
            }
            Some(c) if digit_value(c) < 10 => {
                f.bump();
                any = true;
                let d = digit_value(c) as u8;
                if n == 0 && d == 0 {
                    // A leading zero only moves the point.
                    if point {
                        e10 -= 1;
                    }
                } else if n < MAX_DIGITS {
                    digits[n] = d;
                    n += 1;
                    if point {
                        e10 -= 1;
                    }
                } else {
                    sticky |= d != 0;
                    if !point {
                        e10 += 1;
                    }
                }
            }
            _ => break,
        }
    }
    if !any {
        return None;
    }
    let (pos, left) = (f.pos(), f.left);
    if f.take_ci(b'e') {
        match exponent_digits(f) {
            Some(e) => e10 += e,
            None => {
                if !f.rewind(pos, left) {
                    return None;
                }
            }
        }
    }
    Some(floatconv::from_decimal(neg, &digits[..n], e10, sticky, fmt, mode))
}

pub(crate) fn scan_f64(s: &mut dyn Chars, width: usize) -> Option<FloatScan> {
    let c = scan_float(s, width, &floatconv::F64)?;
    Some(FloatScan { value: f64::from_bits(c.bits as u64), range_error: c.overflow || c.underflow })
}

pub(crate) fn scan_f32(s: &mut dyn Chars, width: usize) -> Option<(f32, bool)> {
    let c = scan_float(s, width, &floatconv::F32)?;
    Some((f32::from_bits(c.bits as u32), c.overflow || c.underflow))
}

/// The native long double encoding (binary128 on aarch64, x87 extended on x86-64) in the
/// low bits, and whether the result is a range error.
pub(crate) fn scan_long_double(s: &mut dyn Chars, width: usize) -> Option<(u128, bool)> {
    #[cfg(target_arch = "aarch64")]
    let fmt = &floatconv::F128;
    #[cfg(target_arch = "x86_64")]
    let fmt = &floatconv::X87;
    let c = scan_float(s, width, fmt)?;
    Some((c.bits, c.overflow || c.underflow))
}
