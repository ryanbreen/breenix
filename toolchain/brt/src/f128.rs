//! Integer arithmetic and conversions on IEEE 754-2019 binary128 encodings.
//!
//! IEEE 754-2019 §§3.4, 4.3, 5.4, 5.8 and 6–7 specify the encodings and results.
//! Arithmetic returns a default quiet NaN for NaN inputs and invalid operations.
//! The value-only interface uses roundTiesToEven for floating-point results.

#![forbid(unsafe_code)]

use core::cmp::Ordering;
use core::mem::swap;

const FRACTION_BITS: u32 = 112;
const HIDDEN_BIT: u128 = 1 << FRACTION_BITS;
const FRACTION_MASK: u128 = HIDDEN_BIT - 1;
const SIGN: u128 = 1 << 127;
const INF: u128 = 0x7fff << FRACTION_BITS;
const QUIET_BIT: u128 = HIDDEN_BIT >> 1;
const NAN: u128 = INF | QUIET_BIT;
const BIAS: i32 = 16383;
const MIN_EXPONENT: i32 = 1 - BIAS;
const ROUND_BITS: u32 = 3;
const EXTENDED_HIDDEN_BIT: u128 = HIDDEN_BIT << ROUND_BITS;

/// IEEE 754-2019 §4.3 rounding directions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rounding {
    NearestEven,
    Upward,
    Downward,
    TowardZero,
}

impl Rounding {
    fn rounds_away_from_zero(self, negative: bool) -> bool {
        matches!(
            (self, negative),
            (Self::Upward, false) | (Self::Downward, true)
        )
    }
}

// IEEE 754-2019 §7 flags, in the bit positions of Arm DDI 0487 M.b §C5.2.10.
pub const INVALID: u8 = 1 << 0;
pub const DIVIDE_BY_ZERO: u8 = 1 << 1;
pub const OVERFLOW: u8 = 1 << 2;
pub const UNDERFLOW: u8 = 1 << 3;
pub const INEXACT: u8 = 1 << 4;

/// A result and the cumulative exception bits raised by this operation.
pub type Outcome<T> = (T, u8);

// IEEE 754-2019 §7.2: a signaling NaN raises invalid, a quiet NaN propagates.
fn nan_flags(a: u128, b: u128) -> u8 {
    let signaling = |magnitude| magnitude > INF && magnitude & QUIET_BIT == 0;
    if signaling(a) || signaling(b) {
        INVALID
    } else {
        0
    }
}

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

fn round_significand(extended: u128, negative: bool, rounding: Rounding) -> Outcome<u128> {
    let significand = extended >> ROUND_BITS;
    let tail = extended & ((1 << ROUND_BITS) - 1);
    if tail == 0 {
        return (significand, 0);
    }
    let halfway = 1 << (ROUND_BITS - 1);
    let increment = if rounding == Rounding::NearestEven {
        tail > halfway || (tail == halfway && significand & 1 != 0)
    } else {
        rounding.rounds_away_from_zero(negative)
    };
    (significand + u128::from(increment), INEXACT)
}

// The leading one is at fraction_bits + ROUND_BITS. The low bits carry the
// guard, round and sticky information. Round once, including at underflow.
fn round_and_pack(
    negative: bool,
    exponent: i32,
    mut extended: u128,
    rounding: Rounding,
    fraction_bits: u32,
    bias: i32,
) -> Outcome<u128> {
    let min_exponent = 1 - bias;
    // IEEE §7.5 permits tininess before rounding. Use it for both arithmetic
    // and conversions, as Arm DDI 0487 M.b §J1.4.3.193 does with FPCR.AH clear.
    let tiny = exponent < min_exponent;
    if tiny {
        extended = shift_right_sticky(extended, (min_exponent - exponent) as u32);
    }
    let exponent = exponent.max(min_exponent);
    let (significand, mut flags) = round_significand(extended, negative, rounding);
    let infinity = ((2 * bias + 1) as u128) << fraction_bits;
    let sign = if negative {
        infinity + (1 << fraction_bits)
    } else {
        0
    };
    // Adding the significand encodes the hidden bit, with a carry advancing
    // the exponent when a subnormal becomes normal or a normal rounds up.
    let magnitude = (((exponent + bias - 1) as u128) << fraction_bits) + significand;
    if magnitude >= infinity {
        let magnitude =
            if rounding == Rounding::NearestEven || rounding.rounds_away_from_zero(negative) {
                infinity
            } else {
                infinity - 1
            };
        return (sign | magnitude, OVERFLOW | INEXACT);
    }
    if tiny && flags == INEXACT {
        flags |= UNDERFLOW;
    }
    (sign | magnitude, flags)
}

// The input has at most 64 significant bits. The scale for each input format
// keeps nonzero results normal in binary128, so packing is exact.
fn from_scaled_integer(sign: u128, magnitude: u64, scale: i32) -> u128 {
    if magnitude == 0 {
        return sign;
    }
    let leading_bit = u64::BITS - 1 - magnitude.leading_zeros();
    let field = (leading_bit as i32 + scale + BIAS) as u128;
    let significand = u128::from(magnitude) << (FRACTION_BITS - leading_bit);
    sign | (field << FRACTION_BITS) | (significand & FRACTION_MASK)
}

/// Convert a signed 32-bit integer exactly to binary128.
pub fn from_i32(value: i32) -> u128 {
    from_i64(i64::from(value))
}

/// Convert a signed 64-bit integer exactly to binary128.
pub fn from_i64(value: i64) -> u128 {
    let sign = if value < 0 { SIGN } else { 0 };
    from_scaled_integer(sign, value.unsigned_abs(), 0)
}

/// Convert an unsigned 32-bit integer exactly to binary128.
pub fn from_u32(value: u32) -> u128 {
    from_u64(u64::from(value))
}

/// Convert an unsigned 64-bit integer exactly to binary128.
pub fn from_u64(value: u64) -> u128 {
    from_scaled_integer(0, value, 0)
}

// i128 can hold both signs of a 64-bit magnitude. Larger magnitudes and
// nonfinite encodings are out of range for each of the integer destinations.
// N1570 §6.3.1.4 discards the fractional part. Report IEEE §5.8 exceptions.
pub(crate) fn to_integer<T: TryFrom<i128> + Default>(bits: u128) -> Outcome<T> {
    let magnitude = bits & !SIGN;
    let exponent = (magnitude >> FRACTION_BITS) as i32 - BIAS;
    if exponent >= 64 {
        return (T::default(), INVALID);
    }
    if exponent < 0 {
        return (T::default(), if magnitude != 0 { INEXACT } else { 0 });
    }
    let significand = HIDDEN_BIT | (magnitude & FRACTION_MASK);
    let shift = FRACTION_BITS - exponent as u32;
    let integer = (significand >> shift) as i128;
    let signed = if bits & SIGN != 0 { -integer } else { integer };
    match T::try_from(signed) {
        Ok(value) => {
            let fractional = significand & ((1 << shift) - 1) != 0;
            (value, if fractional { INEXACT } else { 0 })
        }
        Err(_) => (T::default(), INVALID),
    }
}

// N1570 §6.3.1.4 leaves unrepresentable results undefined. brt returns 0
// for an unrepresentable truncated value, infinity or NaN.

/// Convert binary128 to i32 with roundTowardZero. Invalid conversions return 0.
pub fn to_i32(bits: u128) -> i32 {
    to_integer(bits).0
}

/// Convert binary128 to i64 with roundTowardZero. Invalid conversions return 0.
pub fn to_i64(bits: u128) -> i64 {
    to_integer(bits).0
}

/// Convert binary128 to u32 with roundTowardZero. Invalid conversions return 0.
pub fn to_u32(bits: u128) -> u32 {
    to_integer(bits).0
}

/// Convert binary128 to u64 with roundTowardZero. Invalid conversions return 0.
pub fn to_u64(bits: u128) -> u64 {
    to_integer(bits).0
}

fn widen(bits: u64, fraction_bits: u32, bias: i32) -> Outcome<u128> {
    let max_field = (2 * bias + 1) as u64;
    let exponent_and_sign = bits >> fraction_bits;
    let sign = if exponent_and_sign & (max_field + 1) != 0 {
        SIGN
    } else {
        0
    };
    let hidden_bit = 1 << fraction_bits;
    let field = exponent_and_sign & max_field;
    let fraction = bits & (hidden_bit - 1);
    if field == max_field {
        if fraction == 0 {
            return (sign | INF, 0);
        }
        let quiet_bit = hidden_bit >> 1;
        let payload = fraction & (quiet_bit - 1);
        let flags = if fraction & quiet_bit == 0 {
            INVALID
        } else {
            0
        };
        return (sign | NAN | u128::from(payload), flags);
    }
    let result = if field == 0 {
        from_scaled_integer(sign, fraction, 1 - bias - fraction_bits as i32)
    } else {
        from_scaled_integer(
            sign,
            hidden_bit | fraction,
            field as i32 - bias - fraction_bits as i32,
        )
    };
    (result, 0)
}

/// Widen binary32 bits exactly to binary128, quieting NaNs and preserving their payloads.
pub fn from_f32_bits(bits: u32) -> u128 {
    from_f32_bits_with_flags(bits).0
}

/// Widen binary64 bits exactly to binary128, quieting NaNs and preserving their payloads.
pub fn from_f64_bits(bits: u64) -> u128 {
    from_f64_bits_with_flags(bits).0
}

pub(crate) fn from_f32_bits_with_flags(bits: u32) -> Outcome<u128> {
    widen(u64::from(bits), 23, 127)
}

pub(crate) fn from_f64_bits_with_flags(bits: u64) -> Outcome<u128> {
    widen(bits, 52, 1023)
}

fn narrow(bits: u128, fraction_bits: u32, bias: i32, rounding: Rounding) -> Outcome<u64> {
    let max_field = (2 * bias + 1) as u64;
    let negative = bits & SIGN != 0;
    let sign = if negative {
        (max_field + 1) << fraction_bits
    } else {
        0
    };
    let infinity = max_field << fraction_bits;
    let magnitude = bits & !SIGN;
    if magnitude > INF {
        let quiet_bit = 1 << (fraction_bits - 1);
        // Keep the low payload bits, including the entire payload when it fits.
        let payload = magnitude as u64 & (quiet_bit - 1);
        return (
            sign | infinity | quiet_bit | payload,
            nan_flags(magnitude, 0),
        );
    }
    if magnitude == INF {
        return (sign | infinity, 0);
    }
    if magnitude == 0 {
        return (sign, 0);
    }

    let finite = Finite::decode_nonzero(magnitude);
    let shift = FRACTION_BITS - fraction_bits - ROUND_BITS;
    let (rounded, flags) = round_and_pack(
        negative,
        finite.exponent,
        shift_right_sticky(finite.significand, shift),
        rounding,
        fraction_bits,
        bias,
    );
    (rounded as u64, flags)
}

/// Narrow binary128 to binary32 bits with roundTiesToEven and quiet NaNs.
pub fn to_f32_bits(bits: u128) -> u32 {
    to_f32_bits_rounded(bits, Rounding::NearestEven).0
}

/// Narrow binary128 to binary64 bits with roundTiesToEven and quiet NaNs.
pub fn to_f64_bits(bits: u128) -> u64 {
    to_f64_bits_rounded(bits, Rounding::NearestEven).0
}

/// Narrow binary128 to binary32 with the supplied rounding direction.
pub fn to_f32_bits_rounded(bits: u128, rounding: Rounding) -> Outcome<u32> {
    let (rounded, flags) = narrow(bits, 23, 127, rounding);
    (rounded as u32, flags)
}

/// Narrow binary128 to binary64 with the supplied rounding direction.
pub fn to_f64_bits_rounded(bits: u128, rounding: Rounding) -> Outcome<u64> {
    narrow(bits, 52, 1023, rounding)
}

/// Add two binary128 encodings.
pub fn add(a: u128, b: u128) -> u128 {
    add_rounded(a, b, Rounding::NearestEven).0
}

/// Add two binary128 encodings with the supplied rounding direction.
pub fn add_rounded(mut a: u128, mut b: u128, rounding: Rounding) -> Outcome<u128> {
    // Absolute-value encodings are ordered by magnitude for numeric inputs.
    if a & !SIGN < b & !SIGN {
        swap(&mut a, &mut b);
    }
    let a_mag = a & !SIGN;
    let b_mag = b & !SIGN;
    let opposite = (a ^ b) & SIGN != 0;
    if a_mag > INF {
        return (NAN, nan_flags(a_mag, b_mag));
    }
    if a_mag == INF {
        return if b_mag == INF && opposite {
            (NAN, INVALID)
        } else {
            (a, 0)
        };
    }
    // IEEE §6.3 gives an exact cancellation the sign of roundTowardNegative.
    // Equal-sign zeros keep their sign.
    if opposite && a_mag == b_mag {
        return (
            if rounding == Rounding::Downward {
                SIGN
            } else {
                0
            },
            0,
        );
    }
    if b_mag == 0 {
        return (a, 0);
    }
    let negative = a & SIGN != 0;
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
    round_and_pack(negative, exponent, extended, rounding, FRACTION_BITS, BIAS)
}

/// Subtract two binary128 encodings.
pub fn sub(a: u128, b: u128) -> u128 {
    sub_rounded(a, b, Rounding::NearestEven).0
}

/// Subtract two binary128 encodings with the supplied rounding direction.
pub fn sub_rounded(a: u128, b: u128, rounding: Rounding) -> Outcome<u128> {
    add_rounded(a, b ^ SIGN, rounding)
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
    mul_rounded(a, b, Rounding::NearestEven).0
}

/// Multiply two binary128 encodings with the supplied rounding direction.
pub fn mul_rounded(a: u128, b: u128, rounding: Rounding) -> Outcome<u128> {
    let sign = (a ^ b) & SIGN;
    let a = a & !SIGN;
    let b = b & !SIGN;
    if a > INF || b > INF {
        return (NAN, nan_flags(a, b));
    }
    if (a == INF && b == 0) || (b == INF && a == 0) {
        return (NAN, INVALID);
    }
    if a == INF || b == INF {
        return (sign | INF, 0);
    }
    if a == 0 || b == 0 {
        return (sign, 0);
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
        sign != 0,
        a.exponent + b.exponent + product_carry as i32,
        from_high | from_low | sticky,
        rounding,
        FRACTION_BITS,
        BIAS,
    )
}

/// Divide two binary128 encodings.
pub fn div(a: u128, b: u128) -> u128 {
    div_rounded(a, b, Rounding::NearestEven).0
}

/// Divide two binary128 encodings with the supplied rounding direction.
pub fn div_rounded(a: u128, b: u128, rounding: Rounding) -> Outcome<u128> {
    let sign = (a ^ b) & SIGN;
    let a = a & !SIGN;
    let b = b & !SIGN;
    if a > INF || b > INF {
        return (NAN, nan_flags(a, b));
    }
    if (a == INF && b == INF) || (a == 0 && b == 0) {
        return (NAN, INVALID);
    }
    if a == INF {
        return (sign | INF, 0);
    }
    if b == 0 {
        return (sign | INF, DIVIDE_BY_ZERO);
    }
    if a == 0 || b == INF {
        return (sign, 0);
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
        sign != 0,
        a.exponent - b.exponent - scale as i32,
        quotient | u128::from(remainder != 0),
        rounding,
        FRACTION_BITS,
        BIAS,
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
    compare_with_flags(a, b).0
}

pub(crate) fn compare_with_flags(a: u128, b: u128) -> Outcome<Comparison> {
    let a_mag = a & !SIGN;
    let b_mag = b & !SIGN;
    if a_mag > INF || b_mag > INF {
        return (Comparison::Unordered, nan_flags(a_mag, b_mag));
    }
    if a_mag | b_mag == 0 {
        return (Comparison::Equal, 0);
    }

    let ordering = match (a & SIGN != 0, b & SIGN != 0) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => b_mag.cmp(&a_mag),
        (false, false) => a_mag.cmp(&b_mag),
    };
    let result = match ordering {
        Ordering::Less => Comparison::Less,
        Ordering::Equal => Comparison::Equal,
        Ordering::Greater => Comparison::Greater,
    };
    (result, 0)
}
