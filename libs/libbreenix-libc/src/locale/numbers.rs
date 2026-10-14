//! Wide numeric input uses STRMATH's subject scanner and exact rounding core.
use super::{classify, set_errno};
use crate::stdlib::numscan::{scan_f32, scan_f64, scan_int, Chars};
struct WideChars {
    p: *const u32,
    i: usize,
}
impl Chars for WideChars {
    fn peek(&mut self) -> Option<u32> {
        let c = unsafe { *self.p.add(self.i) };
        if c == 0 {
            None
        } else {
            Some(c)
        }
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
impl WideChars {
    unsafe fn new(p: *const u32) -> Self {
        let mut i = 0;
        while classify::iswspace(*p.add(i)) != 0 {
            i += 1;
        }
        Self { p, i }
    }
}
unsafe fn integer(s: *const u32, end: *mut *mut u32, base: i32, signed: bool) -> u64 {
    let mut chars = WideChars::new(s);
    let result = scan_int(&mut chars, base as u32, usize::MAX);
    match result {
        Err(e) => {
            set_errno(e);
            if !end.is_null() {
                *end = s as *mut u32;
            }
            0
        }
        Ok(r) => {
            if !end.is_null() {
                *end = if r.digits {
                    s.add(chars.pos()) as *mut u32
                } else {
                    s as *mut u32
                };
            }
            if !r.digits {
                return 0;
            }
            let limit = if signed {
                if r.negative {
                    1u64 << 63
                } else {
                    i64::MAX as u64
                }
            } else {
                u64::MAX
            };
            if r.overflow || r.magnitude > limit {
                set_errno(34);
                return if signed && r.negative {
                    i64::MIN as u64
                } else {
                    limit
                };
            }
            if r.negative {
                0u64.wrapping_sub(r.magnitude)
            } else {
                r.magnitude
            }
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn wcstol(s: *const u32, end: *mut *mut u32, base: i32) -> i64 {
    integer(s, end, base, true) as i64
}
#[no_mangle]
pub unsafe extern "C" fn wcstoll(s: *const u32, end: *mut *mut u32, base: i32) -> i64 {
    wcstol(s, end, base)
}
#[no_mangle]
pub unsafe extern "C" fn wcstoul(s: *const u32, end: *mut *mut u32, base: i32) -> u64 {
    integer(s, end, base, false)
}
#[no_mangle]
pub unsafe extern "C" fn wcstoull(s: *const u32, end: *mut *mut u32, base: i32) -> u64 {
    wcstoul(s, end, base)
}
#[no_mangle]
pub unsafe extern "C" fn wcstod(s: *const u32, end: *mut *mut u32) -> f64 {
    let mut chars = WideChars::new(s);
    let Some(r) = scan_f64(&mut chars, usize::MAX) else {
        if !end.is_null() {
            *end = s as *mut u32;
        }
        return 0.0;
    };
    if !end.is_null() {
        *end = s.add(chars.pos()) as *mut u32;
    }
    if r.range_error {
        set_errno(34);
    }
    r.value
}
#[no_mangle]
pub unsafe extern "C" fn wcstof(s: *const u32, end: *mut *mut u32) -> f32 {
    let mut chars = WideChars::new(s);
    let Some((value, range)) = scan_f32(&mut chars, usize::MAX) else {
        if !end.is_null() {
            *end = s as *mut u32;
        }
        return 0.0;
    };
    if !end.is_null() {
        *end = s.add(chars.pos()) as *mut u32;
    }
    if range {
        set_errno(34);
    }
    value
}

unsafe extern "C" fn long_double_bits(s: *const u32, end: *mut *mut u32) -> u128 {
    let mut chars = WideChars::new(s);
    let Some((bits, range)) = crate::stdlib::numscan::scan_long_double(&mut chars, usize::MAX)
    else {
        if !end.is_null() {
            *end = s as *mut u32;
        }
        return 0;
    };
    if !end.is_null() {
        *end = s.add(chars.pos()) as *mut u32;
    }
    if range {
        set_errno(34);
    }
    bits
}
// Rust returns the encoding in integer registers. The C ABI requires q0 for
// AAPCS64 binary128 and st(0) for AMD64's 80-bit extended format.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn wcstold(_: *const u32, _: *mut *mut u32) {
    core::arch::naked_asm!(
        "stp x29, x30, [sp, #-16]!",
        "mov x29, sp",
        "bl {convert}",
        "fmov d0, x0",
        "ins v0.d[1], x1",
        "ldp x29, x30, [sp], #16",
        "ret",
        convert = sym long_double_bits,
    );
}
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn wcstold(_: *const u32, _: *mut *mut u32) {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "call {convert}",
        "mov [rsp], rax",
        "mov [rsp + 8], rdx",
        "fld tbyte ptr [rsp]",
        "add rsp, 24",
        "ret",
        convert = sym long_double_bits,
    );
}
