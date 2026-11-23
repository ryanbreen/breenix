//! N1570 6.4.4.2. Round the literal's exact ratio once, to nearest/even.
//! The Intel SDM Vol. 1 Table 4-3 and Fig. 8-13 specify the x87 encoding.
//! IEEE 754-2019 §3.4 and Table 3.5 specify AArch64's binary128 encoding.

use std::cmp::Ordering;

use super::{
    body::{canonical, constant_integer},
    Expression, ExpressionKind, Target, Unary,
};
use crate::types::{Float, Integer, Kind, Type};

// Static initialization uses the same rounding as literal translation.
pub(super) fn constant_bits(expr: &Expression<'_>, dest: &Kind, target: Target) -> Option<u128> {
    let format = Format::of(dest, target);
    let (negative, significand, exponent) = match &expr.ty.kind {
        Kind::Integer(integer) => {
            let bits = constant_integer(expr, target)
                .or_else(|| constant_to_integer(expr, *integer, target))?;
            let negative = integer.is_signed(target) && (bits as i64) < 0;
            (
                negative,
                Some(u128::from(if negative {
                    bits.wrapping_neg()
                } else {
                    bits
                })),
                0,
            )
        }
        source @ (Kind::Float(_) | Kind::LongDouble) => {
            let source_format = Format::of(source, target);
            let bits = match &expr.kind {
                ExpressionKind::Float(bits) => *bits,
                ExpressionKind::Unary(op @ (Unary::Plus | Unary::Minus), operand) => {
                    let operand = constant_bits(operand, source, target)?;
                    let sign = if *op == Unary::Minus {
                        source_format.sign_bit()
                    } else {
                        0
                    };
                    operand ^ sign
                }
                ExpressionKind::Cast(operand) => constant_bits(operand, source, target)?,
                ExpressionKind::Conditional { condition, yes, no } => {
                    let value = if constant_integer(condition, target)? != 0 {
                        yes
                    } else {
                        no
                    };
                    constant_bits(value, source, target)?
                }
                _ => return None,
            };
            if source == dest {
                return Some(bits);
            }
            source_format.unpack(bits)
        }
        _ => return None,
    };
    let bits = match significand {
        None => format.infinity(),
        Some(0) => 0,
        Some(value) => format.round(Big::from_integer(value), Big(vec![1]), exponent),
    };
    Some(bits | if negative { format.sign_bit() } else { 0 })
}

pub(super) fn constant_to_integer(
    expr: &Expression<'_>,
    dest: Integer,
    target: Target,
) -> Option<u64> {
    if !matches!(expr.ty.kind, Kind::Float(_) | Kind::LongDouble) {
        if let ExpressionKind::Cast(operand) = &expr.kind {
            let Kind::Integer(integer) = expr.ty.kind else {
                return None;
            };
            let value = constant_to_integer(operand, integer, target)?;
            let value = canonical(value, &expr.ty, target);
            return Some(canonical(value, &Type::new(Kind::Integer(dest)), target));
        }
        return None;
    }
    let bits = constant_bits(expr, &expr.ty.kind, target)?;
    let (negative, significand, exponent) = Format::of(&expr.ty.kind, target).unpack(bits);
    if dest == Integer::Bool {
        return Some(u64::from(significand != Some(0)));
    }
    let value = significand?;
    let truncated = if exponent < 0 {
        value.checked_shr(exponent.unsigned_abs()).unwrap_or(0)
    } else if exponent <= 63 && value <= u128::from(u64::MAX) >> exponent {
        value << exponent
    } else {
        return None;
    };
    let width = dest.layout().size * 8;
    let limit = if dest.is_signed(target) {
        (1_u128 << (width - 1)) - u128::from(!negative)
    } else if negative {
        0
    } else {
        (1_u128 << width) - 1
    };
    if truncated > limit {
        return None;
    }
    let value = truncated as u64;
    Some(if negative {
        value.wrapping_neg()
    } else {
        value
    })
}

pub(super) fn literal(text: &str, target: Target) -> Option<(u128, Kind)> {
    let (text, ty) = if let Some(text) = text.strip_suffix(['f', 'F']) {
        (text, Kind::Float(Float::Single))
    } else if let Some(text) = text.strip_suffix(['l', 'L']) {
        (text, Kind::LongDouble)
    } else {
        (text, Kind::Float(Float::Double))
    };
    let format = Format::of(&ty, target);
    let hex = text.starts_with("0x") || text.starts_with("0X");
    let (mantissa, exponent) = if hex {
        let (mantissa, exponent) = text[2..].split_once(['p', 'P'])?;
        (mantissa, parse_exponent(exponent)?)
    } else if let Some((mantissa, exponent)) = text.split_once(['e', 'E']) {
        (mantissa, parse_exponent(exponent)?)
    } else if text.contains('.') {
        (text, 0)
    } else {
        return None;
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    if digits.is_empty() || digits.len() > 4096 {
        return None;
    }
    let radix = if hex { 16 } else { 10 };
    let mut numerator = Big::default();
    for ch in digits.chars() {
        numerator.multiply(radix, ch.to_digit(radix)?);
    }
    if numerator.0.is_empty() {
        return Some((0, ty));
    }

    let mut denominator = Big(vec![1]);
    let bits = if hex {
        exponent - 4 * fraction.len() as i32
    } else {
        let exponent = exponent - fraction.len() as i32;
        let decimal_order = digits.trim_start_matches('0').len() as i32 + exponent;
        if decimal_order > format.decimal_max {
            return Some((format.infinity(), ty));
        }
        if decimal_order < format.decimal_min {
            return Some((0, ty));
        }
        let scaled = if exponent < 0 {
            &mut denominator
        } else {
            &mut numerator
        };
        for _ in 0..exponent.unsigned_abs() {
            scaled.multiply(5, 0);
        }
        exponent
    };
    Some((format.round(numerator, denominator, bits), ty))
}

fn parse_exponent(text: &str) -> Option<i32> {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() {
        return None;
    }
    let mut magnitude = 0;
    for ch in digits.chars() {
        magnitude = (magnitude * 10 + ch.to_digit(10)? as i32).min(32768);
    }
    Some(if negative { -magnitude } else { magnitude })
}

struct Format {
    fraction: u32,
    bias: i32,
    explicit_integer: bool,
    // Conservative decimal-order bounds before constructing powers of five.
    decimal_min: i32,
    decimal_max: i32,
}

impl Format {
    fn of(ty: &Kind, target: Target) -> Self {
        match ty {
            Kind::Float(Float::Single) => Self {
                fraction: 23,
                bias: 127,
                explicit_integer: false,
                decimal_min: -46,
                decimal_max: 39,
            },
            Kind::Float(Float::Double) => Self {
                fraction: 52,
                bias: 1023,
                explicit_integer: false,
                decimal_min: -325,
                decimal_max: 310,
            },
            Kind::LongDouble if target == Target::Aarch64 => Self {
                fraction: 112,
                bias: 16383,
                explicit_integer: false,
                decimal_min: -4966,
                decimal_max: 4934,
            },
            Kind::LongDouble => Self {
                fraction: 63,
                bias: 16383,
                explicit_integer: true,
                decimal_min: -4952,
                decimal_max: 4934,
            },
            _ => unreachable!(),
        }
    }

    fn pack(&self, exponent: i32, significand: u128) -> u128 {
        let width = self.fraction + u32::from(self.explicit_integer);
        (exponent as u128) << width | (significand & ((1 << width) - 1))
    }

    fn sign_bit(&self) -> u128 {
        let exponent_width = self.bias.count_ones() + 1;
        1 << (self.fraction + u32::from(self.explicit_integer) + exponent_width)
    }

    // Decode a floating literal as a signed integer
    // significand times a power of two. A missing significand is infinity.
    fn unpack(&self, bits: u128) -> (bool, Option<u128>, i32) {
        let width = self.fraction + u32::from(self.explicit_integer);
        let exponent_mask = (2 * self.bias + 1) as u128;
        let exponent = ((bits >> width) & exponent_mask) as i32;
        let negative = bits & self.sign_bit() != 0;
        if exponent as u128 == exponent_mask {
            return (negative, None, 0);
        }
        let mut significand = bits & ((1 << width) - 1);
        if exponent != 0 && !self.explicit_integer {
            significand |= 1 << self.fraction;
        }
        (
            negative,
            Some(significand),
            exponent.max(1) - self.bias - self.fraction as i32,
        )
    }

    fn infinity(&self) -> u128 {
        self.pack(2 * self.bias + 1, 1 << self.fraction)
    }

    fn round(&self, mut numerator: Big, mut denominator: Big, bits: i32) -> u128 {
        let ratio = numerator.bits() - denominator.bits();
        let below_power = if ratio >= 0 {
            numerator.compare(&denominator.shift(ratio)) == Ordering::Less
        } else {
            numerator.shift(-ratio).compare(&denominator) == Ordering::Less
        };
        let mut exponent = bits + ratio - i32::from(below_power);
        let minimum = 1 - self.bias;
        if exponent > self.bias {
            return self.infinity();
        }
        if exponent < minimum - self.fraction as i32 - 1 {
            return 0;
        }
        let scale = bits + self.fraction as i32 - exponent.max(minimum);
        if scale >= 0 {
            numerator = numerator.shift(scale);
        } else {
            denominator = denominator.shift(-scale);
        }

        let mut significand = 0;
        for bit in (0..=self.fraction).rev() {
            let part = denominator.shift(bit as i32);
            if numerator.compare(&part) != Ordering::Less {
                numerator.subtract(&part);
                significand |= 1 << bit;
            }
        }
        match numerator.shift(1).compare(&denominator) {
            Ordering::Greater => significand += 1,
            Ordering::Equal if significand & 1 != 0 => significand += 1,
            _ => {}
        }

        let integer_bit = 1 << self.fraction;
        if exponent < minimum {
            // Rounded subnormals can carry into the smallest normal. The
            // extended format stores both the new exponent and integer bit.
            return self.pack(i32::from(significand == integer_bit), significand);
        }
        if significand == integer_bit << 1 {
            significand = integer_bit;
            exponent += 1;
        }
        self.pack(exponent + self.bias, significand)
    }
}

#[derive(Default)]
struct Big(Vec<u32>);

impl Big {
    fn from_integer(mut value: u128) -> Self {
        let mut words = Vec::new();
        while value != 0 {
            words.push(value as u32);
            value >>= 32;
        }
        Self(words)
    }

    fn multiply(&mut self, factor: u32, add: u32) {
        let mut carry = u64::from(add);
        for word in &mut self.0 {
            carry += u64::from(*word) * u64::from(factor);
            *word = carry as u32;
            carry >>= 32;
        }
        if carry != 0 {
            self.0.push(carry as u32);
        }
    }

    fn bits(&self) -> i32 {
        self.0.last().map_or(0, |last| {
            self.0.len() as i32 * 32 - last.leading_zeros() as i32
        })
    }

    fn compare(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }

    fn shift(&self, bits: i32) -> Self {
        if self.0.is_empty() {
            return Self::default();
        }
        let mut words = vec![0; (bits / 32) as usize];
        let mut carry = 0;
        for word in &self.0 {
            carry |= u64::from(*word) << (bits % 32);
            words.push(carry as u32);
            carry >>= 32;
        }
        if carry != 0 {
            words.push(carry as u32);
        }
        Self(words)
    }

    fn subtract(&mut self, other: &Self) {
        let mut borrow = 0;
        for (index, word) in self.0.iter_mut().enumerate() {
            let delta = i64::from(*word) - i64::from(*other.0.get(index).unwrap_or(&0)) - borrow;
            *word = delta as u32;
            borrow = i64::from(delta < 0);
        }
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }
}
