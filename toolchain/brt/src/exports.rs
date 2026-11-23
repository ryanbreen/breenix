//! C entry points for bcc's binary128 operations.
//!
//! AAPCS64 2025Q1 §§5.1, 6.8.2 (C.10–C.11) and 6.9 govern the 16-byte
//! unsigned integer ABI: x0:x1 and x2:x3 for inputs, x0:x1 for the result.

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
