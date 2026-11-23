//! C entry points for bcc's binary128 arithmetic and conversions.
//!
//! AAPCS64 2025Q1 §§5.1, 6.8.2 (C.10–C.11) and 6.9 govern the 16-byte
//! unsigned integer ABI. A binary128 operand occupies x0:x1, with x2:x3
//! for a second operand. A binary128 result occupies x0:x1. The 32-bit and
//! 64-bit types use one GPR, including binary32 and binary64 as integer bits.

use crate::f128::{self, Comparison};

#[no_mangle]
pub extern "C" fn __brt_tf_add(a: u128, b: u128) -> u128 {
    f128::add(a, b)
}

#[no_mangle]
pub extern "C" fn __brt_tf_sub(a: u128, b: u128) -> u128 {
    f128::sub(a, b)
}

#[no_mangle]
pub extern "C" fn __brt_tf_mul(a: u128, b: u128) -> u128 {
    f128::mul(a, b)
}

#[no_mangle]
pub extern "C" fn __brt_tf_div(a: u128, b: u128) -> u128 {
    f128::div(a, b)
}

/// Return unsigned encodings of -1 (less), 0 (equal), 1 (greater), or 2 (unordered).
#[no_mangle]
pub extern "C" fn __brt_tf_cmp(a: u128, b: u128) -> u128 {
    match f128::compare(a, b) {
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
    f128::from_f32_bits(bits)
}

#[no_mangle]
pub extern "C" fn __brt_tf_from_f64(bits: u64) -> u128 {
    f128::from_f64_bits(bits)
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_i32(bits: u128) -> i32 {
    f128::to_i32(bits)
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_i64(bits: u128) -> i64 {
    f128::to_i64(bits)
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_u32(bits: u128) -> u32 {
    f128::to_u32(bits)
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_u64(bits: u128) -> u64 {
    f128::to_u64(bits)
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_f32(bits: u128) -> u32 {
    f128::to_f32_bits(bits)
}

#[no_mangle]
pub extern "C" fn __brt_tf_to_f64(bits: u128) -> u64 {
    f128::to_f64_bits(bits)
}
