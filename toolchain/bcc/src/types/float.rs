//! Binary formats from AMD64 psABI 3.1.2 and AAPCS64 5.1 and 10.1.

use super::{Float, Kind, Target};

pub(crate) struct FloatFormat {
    pub(crate) fraction: u32,
    pub(crate) bias: i32,
    pub(crate) explicit_integer: bool,
}

impl FloatFormat {
    pub(crate) fn of(ty: &Kind, target: Target) -> Self {
        let (fraction, bias, explicit_integer) = match ty {
            Kind::Float(Float::Single) => (23, 127, false),
            Kind::Float(Float::Double) => (52, 1023, false),
            Kind::LongDouble if target == Target::X86_64 => (63, 16383, true),
            Kind::LongDouble => (112, 16383, false),
            _ => unreachable!(),
        };
        Self {
            fraction,
            bias,
            explicit_integer,
        }
    }

    pub(crate) fn exponent_shift(&self) -> u32 {
        self.fraction + u32::from(self.explicit_integer)
    }

    pub(crate) fn exponent_mask(&self) -> u128 {
        ((2 * self.bias + 1) as u128) << self.exponent_shift()
    }

    pub(crate) fn min_exponent(&self) -> i32 {
        1 - self.bias
    }

    pub(crate) fn min_subnormal_exponent(&self) -> i32 {
        self.min_exponent() - self.fraction as i32
    }

    pub(crate) fn decimal_digits(&self) -> i32 {
        floor_log10_pow2(self.fraction as i32)
    }

    pub(crate) fn roundtrip_digits(&self) -> i32 {
        2 + floor_log10_pow2(self.fraction as i32 + 1)
    }

    pub(crate) fn min_decimal_exponent(&self) -> i32 {
        -floor_log10_pow2(-self.min_exponent())
    }

    pub(crate) fn max_decimal_exponent(&self) -> i32 {
        // For these four formats, the largest finite number and the next
        // (unrepresentable) power of two lie in the same decimal decade.
        floor_log10_pow2(self.bias + 1)
    }
}

// At the precisions and exponents of these formats the fixed-point
// approximation (error below 0.00008 after scaling) gives the exact floor.
pub(crate) fn floor_log10_pow2(exponent: i32) -> i32 {
    exponent * 30_103 / 100_000
}
