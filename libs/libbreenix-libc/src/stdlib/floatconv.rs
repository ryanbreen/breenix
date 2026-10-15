//! Exact conversion of `num / den * 2^b2` to the nearest value of an IEEE 754 binary
//! format in the current rounding direction (ISO C17 7.22.1.3 and Annex F.5).
//!
//! The method is plain long division on big integers: scale the ratio so the quotient
//! holds exactly the format's significand bits, then round once using the remainder.
//! A decimal input D * 10^e becomes D * 5^e * 2^e (or D / 5^-e * 2^e), so no step rounds.

use core::cmp::Ordering;

/// 11600 digits need <38600 bits. At the lowest binary128 decimal exponent,
/// 5^(11600+4968) needs <38500 bits; division needs another 114 bits. 1280
/// 32-bit limbs (40960 bits) cover both, including shift carry space.
const LIMBS: usize = 1280;

#[derive(Clone)]
pub(crate) struct Big {
    len: usize,
    d: [u32; LIMBS],
}

impl Big {
    pub(crate) fn from_u64(v: u64) -> Self {
        let mut b = Big { len: 0, d: [0; LIMBS] };
        b.d[0] = v as u32;
        b.d[1] = (v >> 32) as u32;
        b.len = 2;
        b.trim();
        b
    }

    pub(crate) fn from_u128(v: u128) -> Self {
        let mut b = Big { len: 4, d: [0; LIMBS] };
        for i in 0..4 {
            b.d[i] = (v >> (32 * i)) as u32;
        }
        b.trim();
        b
    }

    fn trim(&mut self) {
        while self.len > 0 && self.d[self.len - 1] == 0 {
            self.len -= 1;
        }
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.len == 0
    }

    /// self = self * m + add.
    pub(crate) fn mul_add_small(&mut self, m: u32, add: u32) {
        let mut carry = add as u64;
        for i in 0..self.len {
            let v = self.d[i] as u64 * m as u64 + carry;
            self.d[i] = v as u32;
            carry = v >> 32;
        }
        if carry != 0 {
            self.d[self.len] = carry as u32;
            self.len += 1;
        }
    }

    pub(crate) fn mul_pow5(&mut self, mut k: u64) {
        const POW5_13: u32 = 1_220_703_125;
        while k >= 13 {
            self.mul_add_small(POW5_13, 0);
            k -= 13;
        }
        self.mul_add_small(5u32.pow(k as u32), 0);
    }

    pub(crate) fn shl(&mut self, bits: usize) {
        if self.len == 0 || bits == 0 {
            return;
        }
        let limbs = bits / 32;
        let sh = bits % 32;
        let old = self.len;
        let new_len = old + limbs + 1;
        self.d[new_len - 1] = 0;
        for i in (0..old).rev() {
            let v = self.d[i];
            if sh == 0 {
                self.d[i + limbs] = v;
            } else {
                self.d[i + limbs + 1] |= v >> (32 - sh);
                self.d[i + limbs] = v << sh;
            }
        }
        for i in 0..limbs {
            self.d[i] = 0;
        }
        self.len = new_len;
        self.trim();
    }

    pub(crate) fn shr1(&mut self) {
        for i in 0..self.len {
            let hi = if i + 1 < self.len { self.d[i + 1] << 31 } else { 0 };
            self.d[i] = (self.d[i] >> 1) | hi;
        }
        self.trim();
    }

    pub(crate) fn bitlen(&self) -> usize {
        if self.len == 0 {
            0
        } else {
            32 * self.len - self.d[self.len - 1].leading_zeros() as usize
        }
    }

    pub(crate) fn cmp(&self, o: &Big) -> Ordering {
        if self.len != o.len {
            return self.len.cmp(&o.len);
        }
        for i in (0..self.len).rev() {
            if self.d[i] != o.d[i] {
                return self.d[i].cmp(&o.d[i]);
            }
        }
        Ordering::Equal
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn add(&mut self, o: &Big) {
        let n = self.len.max(o.len);
        let mut carry = 0u64;
        for i in 0..n {
            let v = carry + if i < self.len { self.d[i] as u64 } else { 0 } + if i < o.len { o.d[i] as u64 } else { 0 };
            self.d[i] = v as u32;
            carry = v >> 32;
        }
        self.len = n;
        if carry != 0 {
            self.d[n] = carry as u32;
            self.len += 1;
        }
    }

    /// self -= o, where self >= o.
    pub(crate) fn sub(&mut self, o: &Big) {
        let mut borrow = 0i64;
        for i in 0..self.len {
            let v = self.d[i] as i64 - borrow - if i < o.len { o.d[i] as i64 } else { 0 };
            self.d[i] = v as u32;
            borrow = (v < 0) as i64;
        }
        self.trim();
    }
}

/// An IEEE 754 binary interchange format, or the x87 80-bit extended format.
pub(crate) struct Format {
    /// Significand bits, including the leading bit.
    pub p: u32,
    pub emin: i32,
    pub emax: i32,
    /// The x87 format stores its leading bit; the others imply it.
    pub explicit_lead: bool,
    /// Decimal exponents of the leading digit beyond which a value surely overflows,
    /// and below which it surely lies under half the smallest subnormal.
    pub dec_high: i64,
    pub dec_low: i64,
}

pub(crate) const F16: Format = Format { p: 11, emin: -14, emax: 15, explicit_lead: false, dec_high: 5, dec_low: -9 };
pub(crate) const F32: Format = Format { p: 24, emin: -126, emax: 127, explicit_lead: false, dec_high: 39, dec_low: -47 };
pub(crate) const F64: Format = Format { p: 53, emin: -1022, emax: 1023, explicit_lead: false, dec_high: 309, dec_low: -326 };
pub(crate) const F128: Format =
    Format { p: 113, emin: -16382, emax: 16383, explicit_lead: false, dec_high: 4933, dec_low: -4968 };
#[cfg_attr(target_arch = "aarch64", allow(dead_code))]
pub(crate) const X87: Format =
    Format { p: 64, emin: -16382, emax: 16383, explicit_lead: true, dec_high: 4933, dec_low: -4953 };

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Round {
    Nearest,
    Up,
    Down,
    Zero,
}

pub(crate) fn current_round() -> Round {
    use crate::math::fenv;
    match fenv::get_round() {
        fenv::FE_UPWARD => Round::Up,
        fenv::FE_DOWNWARD => Round::Down,
        fenv::FE_TOWARDZERO => Round::Zero,
        _ => Round::Nearest,
    }
}

pub(crate) struct Converted {
    /// The value's encoding in the low bits.
    pub bits: u128,
    pub inexact: bool,
    pub overflow: bool,
    /// Tiny and inexact: IEEE 754 underflow.
    pub underflow: bool,
}

impl Format {
    fn total_bits(&self) -> u32 {
        if self.explicit_lead { 80 } else { 1 + self.exp_bits() + self.p - 1 }
    }
    fn exp_bits(&self) -> u32 {
        // emax = 2^(w-1) - 1.
        (self.emax as u32 + 1).trailing_zeros() + 1
    }
    fn sign(&self, neg: bool) -> u128 {
        if neg { 1u128 << (self.total_bits() - 1) } else { 0 }
    }
    fn frac_bits(&self) -> u32 {
        if self.explicit_lead { self.p } else { self.p - 1 }
    }
    pub(crate) fn infinity(&self, neg: bool) -> u128 {
        let all = (1u128 << self.exp_bits()) - 1;
        let lead = if self.explicit_lead { 1u128 << (self.p - 1) } else { 0 };
        self.sign(neg) | (all << self.frac_bits()) | lead
    }
    pub(crate) fn quiet_nan(&self, neg: bool) -> u128 {
        let quiet = if self.explicit_lead { 3u128 << (self.p - 2) } else { 1u128 << (self.p - 2) };
        self.infinity(neg) | quiet
    }
    pub(crate) fn zero(&self, neg: bool) -> u128 {
        self.sign(neg)
    }
    fn max_finite(&self, neg: bool) -> u128 {
        let all = (1u128 << self.exp_bits()) - 1;
        let frac = (1u128 << self.frac_bits()) - 1;
        self.sign(neg) | ((all - 1) << self.frac_bits()) | frac
    }
    /// Encodes q * 2^(e - (p - 1)), where q < 2^p, and q < 2^(p-1) only when e == emin.
    fn encode(&self, neg: bool, e: i32, q: u128) -> u128 {
        if self.explicit_lead {
            let biased = if q >> (self.p - 1) != 0 { (e - self.emin + 1) as u128 } else { 0 };
            self.sign(neg) | (biased << 64) | q
        } else {
            // The leading bit of q carries into the exponent field, which also turns a
            // subnormal that rounded up to 2^(p-1) into the smallest normal.
            self.sign(neg) | ((((e - self.emin) as u128) << (self.p - 1)) + q)
        }
    }
}

#[derive(PartialEq, Eq)]
enum Rem {
    Zero,
    Below,
    Half,
    Above,
}

/// floor(num * 2^s / den) for a quotient below 2^128, and where the remainder lies.
fn divide(num: &Big, den: &Big, s: i64) -> (u128, Rem) {
    let mut r = num.clone();
    let mut d = den.clone();
    if s >= 0 {
        r.shl(s as usize);
    } else {
        d.shl((-s) as usize);
    }
    let mut q = 0u128;
    if r.cmp(&d) != Ordering::Less {
        // Shift the divisor up to the remainder, then back down one bit per quotient bit,
        // which leaves it as it was.
        let top = r.bitlen() - d.bitlen();
        d.shl(top);
        for i in (0..=top).rev() {
            if r.cmp(&d) != Ordering::Less {
                r.sub(&d);
                q |= 1u128 << i;
            }
            if i > 0 {
                d.shr1();
            }
        }
    }
    if r.is_zero() {
        return (q, Rem::Zero);
    }
    r.shl(1);
    let rem = match r.cmp(&d) {
        Ordering::Less => Rem::Below,
        Ordering::Equal => Rem::Half,
        Ordering::Greater => Rem::Above,
    };
    (q, rem)
}

/// Rounds num / den * 2^b2 (num nonzero) into `f`. `sticky` says the true value is a
/// little above num / den (input digits that were dropped and were not all zero).
pub(crate) fn convert(neg: bool, num: &Big, den: &Big, b2: i64, sticky: bool, f: &Format, mode: Round) -> Converted {
    let p = f.p as i64;
    let mut s = p - (num.bitlen() as i64 - den.bitlen() as i64);
    let (mut q, mut rem) = divide(num, den, s);
    if q >> f.p != 0 {
        s -= 1;
        (q, rem) = divide(num, den, s);
    }
    let mut e = b2 - s + p - 1;
    let tiny = e < f.emin as i64;
    if tiny {
        s = b2 - f.emin as i64 + p - 1;
        (q, rem) = divide(num, den, s);
        e = f.emin as i64;
    }
    if sticky {
        rem = match rem {
            Rem::Zero => Rem::Below,
            Rem::Half => Rem::Above,
            r => r,
        };
    }
    let inexact = rem != Rem::Zero;
    let up = match mode {
        Round::Nearest => rem == Rem::Above || (rem == Rem::Half && q & 1 == 1),
        Round::Up => inexact && !neg,
        Round::Down => inexact && neg,
        Round::Zero => false,
    };
    if up {
        q += 1;
        if q >> f.p != 0 {
            q >>= 1;
            e += 1;
        }
    }
    if e > f.emax as i64 {
        let to_max = match mode {
            Round::Nearest => false,
            Round::Zero => true,
            Round::Up => neg,
            Round::Down => !neg,
        };
        let bits = if to_max { f.max_finite(neg) } else { f.infinity(neg) };
        return Converted { bits, inexact: true, overflow: true, underflow: false };
    }
    Converted { bits: f.encode(neg, e as i32, q), inexact, overflow: false, underflow: tiny && inexact }
}

/// The value of the decimal digits `digits` (0..=9 each) times 10^e10.
pub(crate) fn from_decimal(neg: bool, digits: &[u8], e10: i64, sticky: bool, f: &Format, mode: Round) -> Converted {
    let lead = digits.iter().position(|&d| d != 0);
    let Some(lead) = lead else {
        return Converted { bits: f.zero(neg), inexact: false, overflow: false, underflow: false };
    };
    let digits = &digits[lead..];
    let leading_exp = e10 + digits.len() as i64 - 1;
    // Values far outside the format keep their rounding behaviour with a small stand-in.
    let one = [1u8];
    let (digits, e10, sticky) = if leading_exp > f.dec_high {
        (&one[..], f.dec_high + 1, false)
    } else if leading_exp < f.dec_low {
        (&one[..], f.dec_low - 1, true)
    } else {
        (digits, e10, sticky)
    };
    let mut num = Big::from_u64(0);
    for chunk in digits.chunks(9) {
        let mut v = 0u32;
        for &d in chunk {
            v = v * 10 + d as u32;
        }
        num.mul_add_small(10u32.pow(chunk.len() as u32), v);
    }
    let mut den = Big::from_u64(1);
    if e10 >= 0 {
        num.mul_pow5(e10 as u64);
    } else {
        den.mul_pow5((-e10) as u64);
    }
    convert(neg, &num, &den, e10, sticky, f, mode)
}

/// The value of mant * 2^e2.
pub(crate) fn from_binary(neg: bool, mant: u128, e2: i64, sticky: bool, f: &Format, mode: Round) -> Converted {
    if mant == 0 {
        return Converted { bits: f.zero(neg), inexact: false, overflow: false, underflow: false };
    }
    let top = e2 + 127 - mant.leading_zeros() as i64;
    let (mant, e2, sticky) = if top > f.emax as i64 + 1 {
        (1, f.emax as i64 + 2, false)
    } else if top < f.emin as i64 - f.p as i64 - 1 {
        (1, f.emin as i64 - f.p as i64 - 2, true)
    } else {
        (mant, e2, sticky)
    };
    convert(neg, &Big::from_u128(mant), &Big::from_u64(1), e2, sticky, f, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str, e10: i64) -> u64 {
        let d: [u8; 80] = core::array::from_fn(|i| s.as_bytes().get(i).map_or(0, |c| c - b'0'));
        from_decimal(false, &d[..s.len()], e10, false, &F64, Round::Nearest).bits as u64
    }

    #[test]
    fn decimal_doubles() {
        assert_eq!(dec("325", 0), 325f64.to_bits());
        assert_eq!(dec("1", -1), 0.1f64.to_bits());
        assert_eq!(dec("17976931348623157", 292), f64::MAX.to_bits());
        assert_eq!(dec("49406564584124654", -340), 1);
        assert_eq!(dec("22250738585072014", -324), f64::MIN_POSITIVE.to_bits());
        // 1 + 2^-53 lies halfway between 1 and 1 + 2^-52 and rounds to even; a digit
        // beyond it rounds up.
        assert_eq!(dec("100000000000000011102230246251565404236316680908203125", -53), 1f64.to_bits());
        assert_eq!(dec("1000000000000000111022302462515654042363166809082031251", -54), (1.0 + f64::EPSILON).to_bits());
        assert_eq!(dec("9007199254740993", 0), 9007199254740992f64.to_bits());
        assert_eq!(dec("1", 999), f64::INFINITY.to_bits());
        assert_eq!(dec("1", -999), 0);
    }

    #[test]
    fn modes_and_floats() {
        let third = |m| from_decimal(false, &[3; 30], -30, false, &F64, m).bits as u64;
        assert!(third(Round::Up) > third(Round::Down));
        assert_eq!(third(Round::Zero), third(Round::Down));
        assert_eq!(from_decimal(false, &[1], -1, false, &F32, Round::Nearest).bits as u32, 0.1f32.to_bits());
        assert_eq!(from_binary(false, 3, -1, false, &F64, Round::Nearest).bits as u64, 1.5f64.to_bits());
        assert_eq!(from_binary(true, 1, -1074, false, &F64, Round::Nearest).bits as u64, (-5e-324f64).to_bits());
    }
}
