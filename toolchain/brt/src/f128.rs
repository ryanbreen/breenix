//! Integer arithmetic on IEEE 754-2019 binary128 encodings (§3.4).
//!
//! Operations return the result bits using roundTiesToEven (§4.3), with a
//! default quiet NaN for NaN inputs and invalid operations (§§6–7).
//! The interface exposes result bits and has no floating-point status flags.

#![forbid(unsafe_code)]

use core::cmp::Ordering;
use core::mem::swap;

const FRACTION_BITS: u32 = 112;
const HIDDEN_BIT: u128 = 1 << FRACTION_BITS;
const FRACTION_MASK: u128 = HIDDEN_BIT - 1;
const SIGN: u128 = 1 << 127;
const INF: u128 = 0x7fff << FRACTION_BITS;
const NAN: u128 = INF | (HIDDEN_BIT >> 1);
const BIAS: i32 = 16383;
const MIN_EXPONENT: i32 = 1 - BIAS;
const MAX_EXPONENT: i32 = BIAS;
const ROUND_BITS: u32 = 3;
const EXTENDED_HIDDEN_BIT: u128 = HIDDEN_BIT << ROUND_BITS;

struct Finite {
    // Value of the magnitude is significand * 2^(exponent - FRACTION_BITS).
    // Even for a subnormal input, the leading one is at FRACTION_BITS.
    significand: u128,
    exponent: i32,
}

impl Finite {
    fn decode_nonzero(magnitude: u128) -> Self {
        let field = (magnitude >> FRACTION_BITS) as i32;
        let fraction = magnitude & FRACTION_MASK;
        if field == 0 {
            let shift = fraction.leading_zeros() - HIDDEN_BIT.leading_zeros();
            Self {
                significand: fraction << shift,
                exponent: MIN_EXPONENT - shift as i32,
            }
        } else {
            Self {
                significand: HIDDEN_BIT | fraction,
                exponent: field - BIAS,
            }
        }
    }
}

// Summarize the discarded low bits in bit 0 so exact halfway cases can
// still be distinguished after alignment or the shift into the subnormal range.
fn shift_right_sticky(value: u128, shift: u32) -> u128 {
    match shift {
        0 => value,
        1..128 => (value >> shift) | u128::from(value << (128 - shift) != 0),
        _ => u128::from(value != 0),
    }
}

// The leading one is at EXTENDED_HIDDEN_BIT and the low bits carry the
// guard, round and sticky information. Round once, including at underflow.
fn round_and_pack(sign: u128, mut exponent: i32, mut extended: u128) -> u128 {
    if exponent < MIN_EXPONENT {
        extended = shift_right_sticky(extended, (MIN_EXPONENT - exponent) as u32);
        exponent = MIN_EXPONENT;
    }
    let mut significand = extended >> ROUND_BITS;
    let tail = extended & ((1 << ROUND_BITS) - 1);
    let halfway = 1 << (ROUND_BITS - 1);
    if tail > halfway || (tail == halfway && significand & 1 != 0) {
        significand += 1;
    }
    if significand == HIDDEN_BIT << 1 {
        significand >>= 1;
        exponent += 1;
    }
    if exponent > MAX_EXPONENT {
        return sign | INF;
    }
    let field = if significand >= HIDDEN_BIT {
        (exponent + BIAS) as u128
    } else {
        0
    };
    sign | (field << FRACTION_BITS) | (significand & FRACTION_MASK)
}

/// Add two binary128 encodings.
pub fn add(mut a: u128, mut b: u128) -> u128 {
    // Absolute-value encodings are ordered by magnitude for numeric inputs.
    if a & !SIGN < b & !SIGN {
        swap(&mut a, &mut b);
    }
    let a_mag = a & !SIGN;
    let b_mag = b & !SIGN;
    let opposite = (a ^ b) & SIGN != 0;
    if a_mag > INF || (a_mag == INF && b_mag == INF && opposite) {
        return NAN;
    }
    if a_mag == INF {
        return a;
    }
    if b_mag == 0 {
        return if a_mag == 0 { a & b } else { a };
    }
    if opposite && a_mag == b_mag {
        return 0;
    }
    let sign = a & SIGN;
    let a = Finite::decode_nonzero(a_mag);
    let b = Finite::decode_nonzero(b_mag);
    let mut exponent = a.exponent;
    let a_extended = a.significand << ROUND_BITS;
    let b_extended = shift_right_sticky(
        b.significand << ROUND_BITS,
        (a.exponent - b.exponent) as u32,
    );
    let extended = if opposite {
        let difference = a_extended - b_extended;
        // Substantial cancellation is possible only when the exponents are
        // close, where alignment leaves the subtrahend exact.
        let shift = difference.leading_zeros() - EXTENDED_HIDDEN_BIT.leading_zeros();
        exponent -= shift as i32;
        difference << shift
    } else {
        let sum = a_extended + b_extended;
        if sum >= EXTENDED_HIDDEN_BIT << 1 {
            exponent += 1;
            shift_right_sticky(sum, 1)
        } else {
            sum
        }
    };
    round_and_pack(sign, exponent, extended)
}

/// Subtract two binary128 encodings.
pub fn sub(a: u128, b: u128) -> u128 {
    add(a, b ^ SIGN)
}

// Form the 226-bit product in two u128 words using four 64-bit limb products.
fn significand_product(a: u128, b: u128) -> (u128, u128) {
    let (a_high, a_low) = (a >> 64, u128::from(a as u64));
    let (b_high, b_low) = (b >> 64, u128::from(b as u64));
    let low_product = a_low * b_low;
    let cross_a = a_low * b_high;
    let cross_b = a_high * b_low;
    let middle = (low_product >> 64) + u128::from(cross_a as u64) + u128::from(cross_b as u64);
    let high = a_high * b_high + (cross_a >> 64) + (cross_b >> 64) + (middle >> 64);
    let low = (middle << 64) | u128::from(low_product as u64);
    (high, low)
}

/// Multiply two binary128 encodings.
pub fn mul(a: u128, b: u128) -> u128 {
    let sign = (a ^ b) & SIGN;
    let a = a & !SIGN;
    let b = b & !SIGN;
    if a > INF || b > INF || (a == INF && b == 0) || (b == INF && a == 0) {
        return NAN;
    }
    if a == INF || b == INF {
        return sign | INF;
    }
    if a == 0 || b == 0 {
        return sign;
    }

    let a = Finite::decode_nonzero(a);
    let b = Finite::decode_nonzero(b);
    let (high, low) = significand_product(a.significand, b.significand);
    // The product's leading bit is at 2*FRACTION_BITS or one past it.
    let product_carry = u32::from(high >= 1 << (2 * FRACTION_BITS + 1 - 128));
    let shift = FRACTION_BITS - ROUND_BITS + product_carry;
    let from_high = high << (128 - shift);
    let from_low = low >> shift;
    let sticky = u128::from(low << (128 - shift) != 0);
    round_and_pack(
        sign,
        a.exponent + b.exponent + product_carry as i32,
        from_high | from_low | sticky,
    )
}

/// Divide two binary128 encodings.
pub fn div(a: u128, b: u128) -> u128 {
    let sign = (a ^ b) & SIGN;
    let a = a & !SIGN;
    let b = b & !SIGN;
    if a > INF || b > INF || (a == INF && b == INF) || (a == 0 && b == 0) {
        return NAN;
    }
    if a == INF || b == 0 {
        return sign | INF;
    }
    if a == 0 || b == INF {
        return sign;
    }

    let a = Finite::decode_nonzero(a);
    let b = Finite::decode_nonzero(b);
    // Make the quotient start with a one. Long division generates its
    // remaining bits, and the final remainder records any further ones.
    let scale = u32::from(a.significand < b.significand);
    let mut remainder = (a.significand << scale) - b.significand;
    let mut quotient = 1;
    for _ in 0..FRACTION_BITS + ROUND_BITS {
        remainder <<= 1;
        quotient <<= 1;
        if remainder >= b.significand {
            remainder -= b.significand;
            quotient |= 1;
        }
    }
    round_and_pack(
        sign,
        a.exponent - b.exponent - scale as i32,
        quotient | u128::from(remainder != 0),
    )
}

/// The four comparison outcomes in IEEE 754-2019 §5.11.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparison {
    Less,
    Equal,
    Greater,
    Unordered,
}

/// Compare two binary128 encodings, treating signed zeros as equal.
pub fn compare(a: u128, b: u128) -> Comparison {
    let a_mag = a & !SIGN;
    let b_mag = b & !SIGN;
    if a_mag > INF || b_mag > INF {
        return Comparison::Unordered;
    }
    if a_mag | b_mag == 0 {
        return Comparison::Equal;
    }

    let ordering = match (a & SIGN != 0, b & SIGN != 0) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => b_mag.cmp(&a_mag),
        (false, false) => a_mag.cmp(&b_mag),
    };
    match ordering {
        Ordering::Less => Comparison::Less,
        Ordering::Equal => Comparison::Equal,
        Ordering::Greater => Comparison::Greater,
    }
}
