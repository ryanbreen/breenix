//! Original vectors from IEEE 754-2019 §§3.4, 4.3, 5.11, 6 and 7.
//! The seeded oracle compares exact rationals to the midpoints between encodings.

use crate::f128::{self, Comparison};
use core::cmp::Ordering;

const SIGN: u128 = 1 << 127;
const FRACTION_UNIT: u128 = 1 << 112;
const FRACTION_MASK: u128 = FRACTION_UNIT - 1;
const INF: u128 = 0x7fff << 112;
const NAN: u128 = INF | (1 << 111);
const MAX: u128 = INF - 1;
const MIN_NORMAL: u128 = FRACTION_UNIT;
const ONE: u128 = power(0);
const TWO: u128 = power(1);
const THREE: u128 = TWO | (1 << 111);
const HALF: u128 = power(-1);
const ONE_AND_HALF: u128 = ONE | (1 << 111);
const HALF_ULP: u128 = power(-113);

const fn power(exponent: i32) -> u128 {
    ((exponent + 16383) as u128) << 112
}

#[test]
fn f128_add_rounds_per_direction() {
    use f128::{Rounding, INEXACT};

    // IEEE 754-2019 §§4.3, 6.3 and 7.6: ties, their negative mirrors, and exact zero.
    for (rounding, positive, negative, zero) in [
        (Rounding::NearestEven, ONE, SIGN | ONE, 0),
        (Rounding::Upward, ONE + 1, SIGN | ONE, 0),
        (Rounding::Downward, ONE, SIGN | (ONE + 1), SIGN),
        (Rounding::TowardZero, ONE, SIGN | ONE, 0),
    ] {
        assert_eq!(
            f128::add_rounded(ONE, HALF_ULP, rounding),
            (positive, INEXACT)
        );
        assert_eq!(
            f128::add_rounded(SIGN | ONE, SIGN | HALF_ULP, rounding),
            (negative, INEXACT),
        );
        assert_eq!(f128::add_rounded(ONE, SIGN | ONE, rounding), (zero, 0));
        assert_eq!(f128::add_rounded(SIGN, 0, rounding), (zero, 0));
        assert_eq!(f128::add_rounded(SIGN, SIGN, rounding), (SIGN, 0));
    }
}

#[test]
fn f128_raises_ieee_flags() {
    use f128::{Rounding::*, DIVIDE_BY_ZERO, INEXACT, INVALID, OVERFLOW, UNDERFLOW};

    // IEEE 754-2019 §§7.2–7.6: overflow, tiny inexact vs exact, and special operands.
    for (rounding, overflow, negative_overflow, tiny) in [
        (NearestEven, INF, SIGN | INF, 0),
        (Upward, INF, SIGN | MAX, 1),
        (Downward, MAX, SIGN | INF, 0),
        (TowardZero, MAX, SIGN | MAX, 0),
    ] {
        assert_eq!(
            f128::mul_rounded(MAX, TWO, rounding),
            (overflow, OVERFLOW | INEXACT)
        );
        assert_eq!(
            f128::mul_rounded(SIGN | MAX, TWO, rounding),
            (negative_overflow, OVERFLOW | INEXACT),
        );
        assert_eq!(
            f128::div_rounded(1, TWO, rounding),
            (tiny, UNDERFLOW | INEXACT)
        );
        assert_eq!(
            f128::div_rounded(MIN_NORMAL, TWO, rounding),
            (MIN_NORMAL / 2, 0)
        );
        assert_eq!(f128::div_rounded(ONE, 0, rounding), (INF, DIVIDE_BY_ZERO));
        assert_eq!(f128::div_rounded(INF, 0, rounding), (INF, 0));
        assert_eq!(f128::div_rounded(0, 0, rounding), (NAN, INVALID));
        assert_eq!(f128::add_rounded(INF | 1, ONE, rounding), (NAN, INVALID));
        assert_eq!(f128::add_rounded(NAN, ONE, rounding), (NAN, 0));
    }
    assert_eq!(
        f128::mul_rounded(MIN_NORMAL, ONE - 1, NearestEven),
        (MIN_NORMAL, UNDERFLOW | INEXACT)
    );
    assert_eq!(f128::add_rounded(MAX, ONE, TowardZero), (MAX, INEXACT));
}

#[test]
fn f128_conversion_vectors() {
    // IEEE 754-2019 §§3.4 and 5.4.1: these integers fit in 113 bits exactly.
    assert_eq!(f128::from_i32(i32::MIN), SIGN | power(31));
    assert_eq!(f128::from_i64(i64::MIN), SIGN | power(63));
    assert_eq!(f128::from_u32(u32::MAX), power(32) - (1 << 81));
    assert_eq!(f128::from_u64(1 << 63), power(63));
    let uint64_max = power(64) - (1 << 49);
    assert_eq!(f128::from_u64(u64::MAX), uint64_max);
    assert_eq!(f128::to_u64(uint64_max), u64::MAX);
    assert_eq!(f128::to_i64(SIGN | power(63)), i64::MIN);
    assert_eq!(f128::to_i32(SIGN | power(31)), i32::MIN);

    // Truncate before checking the range (IEEE §5.8, N1570 §6.3.1.4).
    assert_eq!(f128::to_i32(SIGN | ONE_AND_HALF), -1);
    assert_eq!(f128::to_i64(ONE_AND_HALF), 1);
    assert_eq!(f128::to_u32(power(32) - 1), u32::MAX);
    assert_eq!(f128::to_u32(ONE_AND_HALF), 1);
    assert_eq!(f128::to_i64(SIGN | (power(63) + 1)), i64::MIN);
    assert_eq!(f128::to_u64(SIGN | HALF), 0);
    for bits in [NAN, INF, SIGN | INF, power(64), power(16383)] {
        assert_eq!(f128::to_i32(bits), 0);
        assert_eq!(f128::to_i64(bits), 0);
        assert_eq!(f128::to_u32(bits), 0);
        assert_eq!(f128::to_u64(bits), 0);
    }
    assert_eq!(f128::to_i64(power(63)), 0);
    assert_eq!(f128::to_u32(power(32)), 0);
    assert_eq!(f128::to_u64(SIGN | ONE), 0);

    // IEEE §§3.4, 5.4.2 and 6.3: widening preserves each finite value and sign.
    assert_eq!(f128::from_f32_bits(0x8000_0000), SIGN);
    assert_eq!(f128::from_f64_bits(0x8000_0000_0000_0000), SIGN);
    assert_eq!(f128::to_f32_bits(SIGN), 0x8000_0000);
    assert_eq!(f128::to_f64_bits(SIGN), 0x8000_0000_0000_0000);
    assert_eq!(f128::from_f32_bits(1), power(-149));
    assert_eq!(f128::from_f64_bits(1), power(-1074));
    assert_eq!(f128::from_f32_bits(0x3fc0_0000), ONE_AND_HALF);
    assert_eq!(f128::from_f64_bits(0xfff0_0000_0000_0000), SIGN | INF);
    assert_eq!(f128::to_f32_bits(SIGN | INF), 0xff80_0000);

    // IEEE §§4.3.1, 7.4 and 7.5: halfway cases round once, including at both ends.
    let f64_overflow = power(1024) - (1 << 59);
    assert_eq!(f128::to_f64_bits(f64_overflow - 1), 0x7fef_ffff_ffff_ffff);
    assert_eq!(f128::to_f64_bits(f64_overflow), 0x7ff0_0000_0000_0000);
    assert_eq!(
        f128::to_f64_bits(SIGN | power(-1075)),
        0x8000_0000_0000_0000
    );
    assert_eq!(f128::to_f64_bits(power(-1075) + 1), 1);
    assert_eq!(
        f128::to_f64_bits(power(-1022) - (1 << 59)),
        0x0010_0000_0000_0000
    );
    assert_eq!(f128::to_f32_bits(power(-150)), 0);
    assert_eq!(f128::to_f32_bits(power(-150) + 1), 1);
    assert_eq!(f128::to_f32_bits(ONE + (1 << 88)), 0x3f80_0000);
    assert_eq!(f128::to_f32_bits(ONE + (3 << 88)), 0x3f80_0002);
    assert_eq!(f128::to_f32_bits(power(128) - (1 << 88)), 0x7f80_0000);

    // Quiet signaling NaNs and retain the sign and representable payload (§6.2).
    assert_eq!(f128::to_f32_bits(SIGN | INF | 1), 0xffc0_0001);
    assert_eq!(f128::to_f64_bits(INF | (1 << 110)), 0x7ff8_0000_0000_0000);
    assert_eq!(f128::from_f64_bits(0xfff8_0000_0000_0001), SIGN | NAN | 1);

    let mut bits = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..1024 {
        bits ^= bits << 13;
        bits ^= bits >> 7;
        bits ^= bits << 17;
        if bits & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000 {
            continue;
        }
        let wide = f128::from_f64_bits(bits);
        let narrow = f128::to_f64_bits(wide);
        assert_eq!(narrow, bits, "binary64 {bits:#018x}");
        assert_eq!(f128::from_f64_bits(narrow), wide);
    }
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    fn evaluate(self, a: u128, b: u128) -> u128 {
        match self {
            Self::Add => f128::add(a, b),
            Self::Sub => f128::sub(a, b),
            Self::Mul => f128::mul(a, b),
            Self::Div => f128::div(a, b),
        }
    }
}

#[test]
fn f128_vectors() {
    use Op::*;
    let arithmetic = [
        // Exact results and rounding at the 113-bit boundary (§4.3.1).
        (Add, ONE, TWO, THREE),
        (Sub, TWO, HALF, ONE_AND_HALF),
        (Mul, ONE_AND_HALF, TWO, THREE),
        (Div, THREE, TWO, ONE_AND_HALF),
        (Div, ONE, THREE, power(-2) | (FRACTION_MASK / 3)),
        (Add, ONE, HALF_ULP, ONE),
        (Add, ONE + 1, HALF_ULP, ONE + 2),
        (Add, ONE, HALF_ULP + 1, ONE + 1),
        (Add, ONE, HALF_ULP - 1, ONE),
        (Add, ONE, 1, ONE),
        (Sub, TWO, HALF_ULP, TWO),
        (Sub, TWO, HALF_ULP + 1, TWO - 1),
        (Sub, ONE + 1, ONE, power(-112)),
        (Mul, ONE + 1, ONE_AND_HALF, ONE_AND_HALF + 2),
        (Mul, ONE + 3, ONE_AND_HALF, ONE_AND_HALF + 4),
        // Gradual underflow, halfway results and crossover to normal (§§4.3, 7.5).
        (Add, MIN_NORMAL - 1, 1, MIN_NORMAL),
        (Sub, MIN_NORMAL, 1, MIN_NORMAL - 1),
        (Mul, MIN_NORMAL, ONE - 2, MIN_NORMAL - 1),
        (Mul, MIN_NORMAL, ONE - 1, MIN_NORMAL),
        (Mul, SIGN | 1, HALF, SIGN),
        (Mul, 1, HALF + 1, 1),
        (Div, 1, TWO, 0),
        (Div, 3, TWO, 2),
        (Div, 5, TWO, 2),
        (Div, SIGN | MIN_NORMAL, TWO, SIGN | (MIN_NORMAL >> 1)),
        (Div, MIN_NORMAL, MAX, 0),
        // Overflow starts halfway above the last finite number (§§4.3.1, 7.4).
        (Add, MAX, power(16269), MAX),
        (Add, MAX, power(16270), INF),
        (Mul, SIGN | MAX, TWO, SIGN | INF),
        (Div, MAX, HALF, INF),
        // The sign rules also apply to exact zeros and infinities (§6.3).
        (Add, 0, SIGN, 0),
        (Add, SIGN, SIGN, SIGN),
        (Add, ONE, SIGN | ONE, 0),
        (Sub, SIGN, 0, SIGN),
        (Sub, SIGN, SIGN, 0),
        (Sub, SIGN | ONE, SIGN | ONE, 0),
        (Mul, SIGN, SIGN | ONE, 0),
        (Mul, INF, SIGN | ONE, SIGN | INF),
        (Div, ONE, SIGN | INF, SIGN),
        (Div, SIGN | INF, SIGN | ONE, INF),
        // Default results for invalid operations and division by 0 (§§7.2, 7.3).
        (Add, INF, SIGN | INF, NAN),
        (Sub, INF, INF, NAN),
        (Mul, 0, INF, NAN),
        (Mul, SIGN | INF, SIGN, NAN),
        (Div, 0, SIGN, NAN),
        (Div, INF, SIGN | INF, NAN),
        (Div, ONE, SIGN, SIGN | INF),
        (Div, SIGN | ONE, SIGN, INF),
        // Both NaN kinds produce the runtime's default quiet NaN (§6.2).
        (Add, NAN | 3, ONE, NAN),
        (Sub, ONE, SIGN | NAN | 3, NAN),
        (Mul, INF | 1, 0, NAN),
        (Div, ONE, SIGN | INF | 1, NAN),
    ];
    for (op, a, b, expected) in arithmetic {
        assert_eq!(op.evaluate(a, b), expected, "{op:?}({a:032x}, {b:032x})");
    }

    use Comparison::*;
    let comparisons = [
        (0, SIGN, Equal),
        (SIGN, 0, Equal),
        (SIGN | 1, SIGN, Less),
        (1, 0, Greater),
        (ONE, ONE, Equal),
        (ONE + 1, ONE, Greater),
        (SIGN | (ONE + 1), SIGN | ONE, Less),
        (SIGN | INF, SIGN | MAX, Less),
        (INF, MAX, Greater),
        (INF, INF, Equal),
        (NAN, NAN, Unordered),
        (ONE, INF | 1, Unordered),
        (SIGN | NAN, SIGN | INF, Unordered),
    ];
    for (a, b, expected) in comparisons {
        assert_eq!(f128::compare(a, b), expected, "compare({a:032x}, {b:032x})");
    }
}

// Exact, nonnegative integers in little-endian base 2^64. An empty vector
// represents 0, and a nonempty vector ends in a nonzero limb.
#[derive(Clone)]
struct Natural(Vec<u64>);

impl Natural {
    fn from_u128(value: u128) -> Self {
        Self::trim(vec![value as u64, (value >> 64) as u64])
    }

    fn trim(mut limbs: Vec<u64>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self(limbs)
    }

    fn bits(&self) -> i32 {
        self.0.last().map_or(0, |high| {
            (self.0.len() as i32) * 64 - high.leading_zeros() as i32
        })
    }

    fn compare(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }

    fn shift(&self, distance: i32) -> Self {
        if distance == 0 || self.0.is_empty() {
            return self.clone();
        }
        let mut result = vec![0; distance as usize / 64];
        let mut carry = 0;
        for &limb in &self.0 {
            let wide = (u128::from(limb) << (distance % 64)) | carry;
            result.push(wide as u64);
            carry = wide >> 64;
        }
        result.push(carry as u64);
        Self::trim(result)
    }

    fn add(&self, other: &Self) -> Self {
        let len = self.0.len().max(other.0.len());
        let mut result = Vec::with_capacity(len + 1);
        let mut carry = 0;
        for i in 0..len {
            let sum = u128::from(self.0.get(i).copied().unwrap_or(0))
                + u128::from(other.0.get(i).copied().unwrap_or(0))
                + carry;
            result.push(sum as u64);
            carry = sum >> 64;
        }
        result.push(carry as u64);
        Self::trim(result)
    }

    fn sub(&self, other: &Self) -> Self {
        let mut result = Vec::with_capacity(self.0.len());
        let mut borrow = false;
        for (i, &limb) in self.0.iter().enumerate() {
            let (difference, first) = limb.overflowing_sub(other.0.get(i).copied().unwrap_or(0));
            let (digit, second) = difference.overflowing_sub(u64::from(borrow));
            result.push(digit);
            borrow = first || second;
        }
        assert!(!borrow);
        Self::trim(result)
    }

    fn mul(&self, other: &Self) -> Self {
        let mut result = vec![0; self.0.len() + other.0.len()];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0;
            for (j, &b) in other.0.iter().enumerate() {
                let sum = u128::from(a) * u128::from(b) + u128::from(result[i + j]) + carry;
                result[i + j] = sum as u64;
                carry = sum >> 64;
            }
            result[i + other.0.len()] = carry as u64;
        }
        Self::trim(result)
    }
}

// A magnitude N * 2^scale. Exponent fields 0 and 1 use the same quantum.
// For the oracle's overflow midpoint only, the infinity encoding stands
// for the next unbounded-exponent value, 2^16384 (§4.3.1).
fn dyadic(bits: u128) -> (Natural, i32) {
    let exponent = ((bits & !SIGN) >> 112) as i32;
    let fraction = bits & FRACTION_MASK;
    let significand = if exponent == 0 {
        fraction
    } else {
        fraction + FRACTION_UNIT
    };
    (Natural::from_u128(significand), exponent.max(1) - 16495)
}

fn cmp_scaled(a: &Natural, ae: i32, b: &Natural, be: i32) -> Ordering {
    if a.0.is_empty() || b.0.is_empty() {
        return a.0.len().cmp(&b.0.len());
    }
    (a.bits() + ae).cmp(&(b.bits() + be)).then_with(|| {
        let common = ae.min(be);
        a.shift(ae - common).compare(&b.shift(be - common))
    })
}

struct Rational {
    negative: bool,
    numerator: Natural,
    denominator: Natural,
    scale: i32,
}

impl Rational {
    fn exact(op: Op, a: u128, b: u128) -> Self {
        let a_negative = a & SIGN != 0;
        let b_negative = (b & SIGN != 0) ^ matches!(op, Op::Sub);
        let (a, ae) = dyadic(a);
        let (b, be) = dyadic(b);
        let one = Natural::from_u128(1);
        let (negative, numerator, denominator, scale) = match op {
            Op::Add | Op::Sub => {
                let scale = ae.min(be);
                let a = a.shift(ae - scale);
                let b = b.shift(be - scale);
                if a_negative == b_negative {
                    (a_negative, a.add(&b), one, scale)
                } else if a.compare(&b) == Ordering::Less {
                    (b_negative, b.sub(&a), one, scale)
                } else {
                    (
                        a_negative && a.compare(&b) != Ordering::Equal,
                        a.sub(&b),
                        one,
                        scale,
                    )
                }
            }
            Op::Mul => (a_negative ^ b_negative, a.mul(&b), one, ae + be),
            Op::Div => (a_negative ^ b_negative, a, b, ae - be),
        };
        Self {
            negative,
            numerator,
            denominator,
            scale,
        }
    }

    fn compare_dyadic(&self, value: &Natural, scale: i32) -> Ordering {
        cmp_scaled(
            &self.numerator,
            self.scale,
            &self.denominator.mul(value),
            scale,
        )
    }

    fn rounded(&self) -> u128 {
        let sign = if self.negative { SIGN } else { 0 };
        if self.numerator.0.is_empty() {
            return sign;
        }
        // Search the ordered positive encodings for the two neighbors.
        let (mut low, mut high) = (0, INF);
        while high - low > 1 {
            let mid = low + (high - low) / 2;
            let (value, scale) = dyadic(mid);
            match self.compare_dyadic(&value, scale) {
                Ordering::Less => high = mid,
                Ordering::Equal => return sign | mid,
                Ordering::Greater => low = mid,
            }
        }
        let (a, ae) = dyadic(low);
        let (b, be) = dyadic(high);
        let common = ae.min(be);
        let twice_midpoint = a.shift(ae - common).add(&b.shift(be - common));
        let order = self.compare_dyadic(&twice_midpoint, common - 1);
        let rounded = if order == Ordering::Greater || (order == Ordering::Equal && low & 1 != 0) {
            high
        } else {
            low
        };
        sign | rounded
    }
}

fn finite_compare(a: u128, b: u128) -> Comparison {
    let difference = Rational::exact(Op::Sub, a, b);
    if difference.numerator.0.is_empty() {
        Comparison::Equal
    } else if difference.negative {
        Comparison::Less
    } else {
        Comparison::Greater
    }
}

fn next_bits(state: &mut u128) -> u128 {
    *state ^= *state << 23;
    *state ^= *state >> 17;
    *state ^= *state << 26;
    *state
}

#[test]
fn f128_matches_exact_rationals() {
    let mut state = 0xa5c3_7d91_e09b_2468_ace1_3579_bdf0_642bu128;
    for i in 0..1024 {
        let x = next_bits(&mut state);
        let y = next_bits(&mut state);
        let exponent = |bits: u128| ((bits >> 112) & 0x7fff) % 0x7fff;
        let (a_exp, b_exp) = match i % 6 {
            0 => (exponent(x), exponent(y)),
            1 => (exponent(x), exponent(x)),
            2 => (0, exponent(y) % 5),
            3 => (0x7ffe - exponent(x) % 5, 16383 + exponent(y) % 4),
            4 => (exponent(x) % 5, 16380 + exponent(y) % 7),
            _ => (16383, 16265 + exponent(y) % 10),
        };
        let a = (x & (SIGN | FRACTION_MASK)) | (a_exp << 112);
        let b = ((y & (SIGN | FRACTION_MASK)) | (b_exp << 112)).max(1);
        for op in [Op::Add, Op::Sub, Op::Mul, Op::Div] {
            let expected = Rational::exact(op, a, b).rounded();
            assert_eq!(
                op.evaluate(a, b),
                expected,
                "seed index {i}: {op:?}({a:032x}, {b:032x})"
            );
        }
        assert_eq!(
            f128::compare(a, b),
            finite_compare(a, b),
            "seed index {i}: compare({a:032x}, {b:032x})"
        );
    }
}
