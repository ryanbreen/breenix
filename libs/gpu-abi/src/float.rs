//! IEEE-754 binary32 bit patterns built with integer arithmetic.
//!
//! Neither kernel target uses the FPU (`docs/gui/gpu.md` section 1.3), yet VirGL takes
//! vertex positions, viewport transforms and colour values as binary32. Each function
//! returns the bit pattern IEEE 754's round-to-nearest-even conversion gives for its
//! exact real value, so the device sees the same floats a host computing natively would.

/// The binary32 nearest `value`.
pub const fn int_bits(value: i64) -> u32 {
    if value == 0 {
        return 0;
    }
    let sign = if value < 0 { 1u32 << 31 } else { 0 };
    let magnitude = value.unsigned_abs();
    sign | magnitude_bits(magnitude as u128, 0)
}

/// The binary32 nearest `value / 2^shift`.
pub const fn scaled_int_bits(value: i64, shift: u32) -> u32 {
    if value == 0 {
        return 0;
    }
    let sign = if value < 0 { 1u32 << 31 } else { 0 };
    sign | magnitude_bits(value.unsigned_abs() as u128, shift as i32)
}

/// The binary32 nearest `num / den`, for `den > 0` and `num <= den`.
pub const fn ratio_bits(num: u32, den: u32) -> u32 {
    if num == 0 || den == 0 {
        return 0;
    }
    // Find e with 2^e <= num/den < 2^(e+1); e is in -32..=0 for 32-bit operands.
    let mut e: i32 = 0;
    while (num as u128) << ((-e) as u32) < den as u128 {
        e -= 1;
    }
    // The 24-bit significand floor(num * 2^(23-e) / den) and its remainder.
    let shift = (23 - e) as u32;
    let scaled = (num as u128) << shift;
    let mut q = scaled / den as u128;
    let r = scaled % den as u128;
    let twice = 2 * r;
    if twice > den as u128 || (twice == den as u128 && q & 1 == 1) {
        q += 1;
    }
    let mut exponent = e;
    if q == 1 << 24 {
        q = 1 << 23;
        exponent += 1;
    }
    (((exponent + 127) as u32) << 23) | (q as u32 & 0x7F_FFFF)
}

/// The binary32 magnitude bits of `m / 2^shift` for `m > 0`, rounded to nearest even.
const fn magnitude_bits(m: u128, shift: i32) -> u32 {
    // Position of the leading one.
    let top = 127 - m.leading_zeros() as i32;
    let exponent = top - shift;
    // Keep 24 significant bits, rounding what falls off.
    let (mut sig, round_up) = if top > 23 {
        let drop = (top - 23) as u32;
        let kept = m >> drop;
        let rest = m & ((1u128 << drop) - 1);
        let half = 1u128 << (drop - 1);
        (kept, rest > half || (rest == half && kept & 1 == 1))
    } else {
        (m << (23 - top) as u32, false)
    };
    let mut exponent = exponent;
    if round_up {
        sig += 1;
        if sig == 1 << 24 {
            sig = 1 << 23;
            exponent += 1;
        }
    }
    (((exponent + 127) as u32) << 23) | (sig as u32 & 0x7F_FFFF)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn integers_match_native_conversion() {
        for v in [-16_777_217i64, -16_777_216, -8192, -1, 1, 2, 3, 255, 8191, 8192, 16_384, 16_777_215, 16_777_216, 16_777_217, 1 << 31] {
            assert_eq!(int_bits(v), (v as f32).to_bits(), "{v}");
        }
        assert_eq!(int_bits(0), 0);
    }

    #[test]
    fn scaled_positions_are_exact() {
        for x in 0..=16_384i64 {
            let i = x - 8192;
            let want = (i as f32 / 8192.0).to_bits();
            assert_eq!(scaled_int_bits(i, 13), if i == 0 { 0 } else { want }, "{x}");
        }
    }

    #[test]
    fn ratios_match_native_division() {
        for den in [1u32, 3, 7, 255, 256, 1000, 65_535, 16_777_217, u32::MAX] {
            for num in [0u32, 1, 2, 3, 5, 127, 128, 254, 255] {
                if num > den {
                    continue;
                }
                let want = (num as f64 / den as f64) as f32;
                assert_eq!(ratio_bits(num, den), want.to_bits(), "{num}/{den}");
            }
        }
        for k in 0..=255u32 {
            let want = (k as f32 / 255.0).to_bits();
            assert_eq!(ratio_bits(k, 255), want, "{k}/255");
        }
    }
}
