//! C entry points for bcc's binary128 arithmetic and conversions.
//!
//! AAPCS64 2025Q1 §§5.1, 6.8.2 (C.10–C.11) and 6.9 govern the 16-byte
//! unsigned integer ABI. A binary128 operand occupies x0:x1, with x2:x3
//! for a second operand. A binary128 result occupies x0:x1. The 32-bit and
//! 64-bit types use one GPR, including binary32 and binary64 as integer bits.

use crate::f128::{self, Comparison, Outcome, Rounding};

#[cfg(target_arch = "aarch64")]
fn rounding() -> Rounding {
    // Arm DDI 0487 M.b §C5.2.8: FPCR.RMode occupies bits [23:22].
    let fpcr: u64;
    // SAFETY: MRS reads the current thread's floating-point control register.
    unsafe {
        core::arch::asm!("mrs {fpcr}, fpcr", fpcr = out(reg) fpcr, options(nostack, preserves_flags))
    };
    match (fpcr >> 22) & 3 {
        1 => Rounding::Upward,
        2 => Rounding::Downward,
        3 => Rounding::TowardZero,
        _ => Rounding::NearestEven,
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn rounding() -> Rounding {
    Rounding::NearestEven
}

fn finish<T>((result, flags): Outcome<T>) -> T {
    #[cfg(target_arch = "aarch64")]
    if flags != 0 {
        // Arm DDI 0487 M.b §C5.2.10: accumulate in FPSR without clearing
        // earlier exceptions or changing the other status fields.
        let fpsr: u64;
        // SAFETY: MRS reads the current thread's floating-point status register.
        unsafe {
            core::arch::asm!("mrs {fpsr}, fpsr", fpsr = out(reg) fpsr, options(nostack, preserves_flags))
        };
        let updated = fpsr | u64::from(flags);
        // SAFETY: MSR only sets cumulative exception bits in this thread's FPSR.
        unsafe {
            core::arch::asm!("msr fpsr, {updated}", updated = in(reg) updated, options(nostack, preserves_flags))
        };
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = flags;
    result
}

#[no_mangle]
pub extern "C" fn __brt_tf_add(a: u128, b: u128) -> u128 {
    finish(f128::add_rounded(a, b, rounding()))
}

#[no_mangle]
pub extern "C" fn __brt_tf_sub(a: u128, b: u128) -> u128 {
    finish(f128::sub_rounded(a, b, rounding()))
}

#[no_mangle]
pub extern "C" fn __brt_tf_mul(a: u128, b: u128) -> u128 {
    finish(f128::mul_rounded(a, b, rounding()))
}

#[no_mangle]
pub extern "C" fn __brt_tf_div(a: u128, b: u128) -> u128 {
    finish(f128::div_rounded(a, b, rounding()))
}

/// Return unsigned encodings of -1 (less), 0 (equal), 1 (greater), or 2 (unordered).
#[no_mangle]
pub extern "C" fn __brt_tf_cmp(a: u128, b: u128) -> u128 {
    match finish(f128::compare_with_flags(a, b)) {
        Comparison::Less => u128::MAX,
        Comparison::Equal => 0,
        Comparison::Greater => 1,
        Comparison::Unordered => 2,
    }
}

#[no_mangle]
pub extern "C" fn __brt_tf_from_i32(value: i32) -> u128 {
    f128::from_i32(value)
}

#[no_mangle]
pub extern "C" fn __brt_tf_from_i64(value: i64) -> u128 {
    f128::from_i64(value)
}

#[no_mangle]
pub extern "C" fn __brt_tf_from_u32(value: u32) -> u128 {
    f128::from_u32(value)
}

#[no_mangle]
pub extern "C" fn __brt_tf_from_u64(value: u64) -> u128 {
    f128::from_u64(value)
}

#[no_mangle]
pub extern "C" fn __brt_tf_from_f32(bits: u32) -> u128 {
    finish(f128::from_f32_bits_with_flags(bits))
}

#[no_mangle]
pub extern "C" fn __brt_tf_from_f64(bits: u64) -> u128 {
    finish(f128::from_f64_bits_with_flags(bits))
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_i32(bits: u128) -> i32 {
    finish(f128::to_integer(bits))
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_i64(bits: u128) -> i64 {
    finish(f128::to_integer(bits))
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_u32(bits: u128) -> u32 {
    finish(f128::to_integer(bits))
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_u64(bits: u128) -> u64 {
    finish(f128::to_integer(bits))
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_f32(bits: u128) -> u32 {
    finish(f128::to_f32_bits_rounded(bits, rounding()))
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_f64(bits: u128) -> u64 {
    finish(f128::to_f64_bits_rounded(bits, rounding()))
}
