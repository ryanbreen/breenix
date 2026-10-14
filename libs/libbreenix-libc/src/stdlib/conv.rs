//! strtol and friends, strtod and friends, and ato* (ISO C17 7.22.1, POSIX.1-2024).

use super::numscan::{self, CStrChars, Chars, IntScan};
use super::{set_errno, ERANGE};

/// Scans an integer subject sequence after leading white space and stores the end
/// pointer; None when there is no subject sequence or the base is invalid.
unsafe fn scan(nptr: *const u8, end: *mut *mut u8, base: i32) -> Option<IntScan> {
    let mut s = CStrChars::new(nptr);
    numscan::skip_space(&mut s);
    let base = if (0..=36).contains(&base) { base as u32 } else { 1 };
    let r = match numscan::scan_int(&mut s, base, usize::MAX) {
        Ok(r) if r.digits => r,
        Ok(_) => {
            if !end.is_null() {
                *end = nptr as *mut u8;
            }
            return None;
        }
        Err(e) => {
            set_errno(e);
            if !end.is_null() {
                *end = nptr as *mut u8;
            }
            return None;
        }
    };
    if !end.is_null() {
        *end = nptr.add(s.pos()) as *mut u8;
    }
    Some(r)
}

/// The value as a signed integer whose largest value is `max`, with ERANGE on overflow.
fn signed(r: IntScan, max: u64) -> i64 {
    if r.negative {
        if r.overflow || r.magnitude > max + 1 {
            set_errno(ERANGE);
            return -(max as i64) - 1;
        }
        (r.magnitude as i64).wrapping_neg()
    } else {
        if r.overflow || r.magnitude > max {
            set_errno(ERANGE);
            return max as i64;
        }
        r.magnitude as i64
    }
}

/// The value as an unsigned integer whose largest value is `max`; a minus sign negates
/// in the unsigned type, as 7.22.1.4 specifies.
fn unsigned(r: IntScan, max: u64) -> u64 {
    if r.overflow || r.magnitude > max {
        set_errno(ERANGE);
        return max;
    }
    if r.negative { r.magnitude.wrapping_neg() & max } else { r.magnitude }
}

#[no_mangle]
pub unsafe extern "C" fn strtol(nptr: *const u8, end: *mut *mut u8, base: i32) -> i64 {
    scan(nptr, end, base).map_or(0, |r| signed(r, i64::MAX as u64))
}

#[no_mangle]
pub unsafe extern "C" fn strtoll(nptr: *const u8, end: *mut *mut u8, base: i32) -> i64 {
    strtol(nptr, end, base)
}

#[no_mangle]
pub unsafe extern "C" fn strtoimax(nptr: *const u8, end: *mut *mut u8, base: i32) -> i64 {
    strtol(nptr, end, base)
}

#[no_mangle]
pub unsafe extern "C" fn strtoul(nptr: *const u8, end: *mut *mut u8, base: i32) -> u64 {
    scan(nptr, end, base).map_or(0, |r| unsigned(r, u64::MAX))
}

#[no_mangle]
pub unsafe extern "C" fn strtoull(nptr: *const u8, end: *mut *mut u8, base: i32) -> u64 {
    strtoul(nptr, end, base)
}

#[no_mangle]
pub unsafe extern "C" fn strtoumax(nptr: *const u8, end: *mut *mut u8, base: i32) -> u64 {
    strtoul(nptr, end, base)
}

#[no_mangle]
pub unsafe extern "C" fn atoi(nptr: *const u8) -> i32 {
    strtol(nptr, core::ptr::null_mut(), 10) as i32
}

#[no_mangle]
pub unsafe extern "C" fn atol(nptr: *const u8) -> i64 {
    strtol(nptr, core::ptr::null_mut(), 10)
}

#[no_mangle]
pub unsafe extern "C" fn atoll(nptr: *const u8) -> i64 {
    strtol(nptr, core::ptr::null_mut(), 10)
}

/// Runs a float scanner after leading white space, storing the end pointer.
unsafe fn float<T>(nptr: *const u8, end: *mut *mut u8, f: impl FnOnce(&mut dyn Chars) -> Option<(T, bool)>) -> Option<T> {
    let mut s = CStrChars::new(nptr);
    numscan::skip_space(&mut s);
    let r = f(&mut s);
    if !end.is_null() {
        *end = if r.is_some() { nptr.add(s.pos()) as *mut u8 } else { nptr as *mut u8 };
    }
    let (v, range) = r?;
    if range {
        set_errno(ERANGE);
    }
    Some(v)
}

#[no_mangle]
pub unsafe extern "C" fn strtod(nptr: *const u8, end: *mut *mut u8) -> f64 {
    float(nptr, end, |s| numscan::scan_f64(s, usize::MAX).map(|r| (r.value, r.range_error))).unwrap_or(0.0)
}

#[no_mangle]
pub unsafe extern "C" fn strtof(nptr: *const u8, end: *mut *mut u8) -> f32 {
    float(nptr, end, |s| numscan::scan_f32(s, usize::MAX)).unwrap_or(0.0)
}

#[no_mangle]
pub unsafe extern "C" fn atof(nptr: *const u8) -> f64 {
    strtod(nptr, core::ptr::null_mut())
}

/// The long double result's encoding, for strtold.
pub(crate) unsafe extern "C" fn strtold_bits(nptr: *const u8, end: *mut *mut u8) -> u128 {
    float(nptr, end, |s| numscan::scan_long_double(s, usize::MAX)).unwrap_or(0)
}

/// strtold on aarch64: long double is IEEE binary128, returned in v0. The helper
/// returns the encoding in x0:x1, which fmov moves into the two halves of v0.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn strtold(nptr: *const u8, end: *mut *mut u8) {
    core::arch::naked_asm!(
        "stp x29, x30, [sp, #-16]!",
        "mov x29, sp",
        "bl {bits}",
        "fmov d0, x0",
        "fmov v0.d[1], x1",
        "ldp x29, x30, [sp], #16",
        "ret",
        bits = sym strtold_bits,
    )
}

/// strtold on x86-64: long double is the x87 80-bit format, returned in st(0), which no
/// Rust type expresses; the helper writes the encoding to the stack and fld loads it.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn strtold(nptr: *const u8, end: *mut *mut u8) {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "call {bits}",
        "mov [rsp], rax",
        "mov [rsp + 8], rdx",
        "fld tbyte ptr [rsp]",
        "add rsp, 24",
        "ret",
        bits = sym strtold_bits,
    )
}
