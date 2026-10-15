//! Extended-range double-double arithmetic, shared by the long double functions
//! (long_double.rs) and the complex ones (complex.rs).
//!
//! An X is m * 2^e with m a double-double whose high part lies in [1, 2) and e an
//! integer far wider than any format's exponent, so intermediate results neither
//! overflow nor underflow; each function rounds its X once, into its own format, through
//! stdlib::floatconv, which honours the rounding direction and raises the exceptions.
//! The double-double kernels of explog.rs, trig.rs and special.rs evaluate the
//! functions on m, about 106 bits: enough for the x87 format's 64-bit significand, and a
//! relative error near 2^-100 in binary128's 113 bits.
//!
//! Trigonometric arguments are reduced from their exact significand with as many bits
//! of 2/pi as the widest exponent needs (Payne and Hanek, "Radian reduction for
//! trigonometric functions", SIGNUM Newsletter 18, 1983), the table generated from
//! Machin's formula pi = 16 atan(1/5) - 4 atan(1/239).

use super::basic::{decompose, round};
use super::consts::{HALF_LN_2PI, INV_FACT, INV_LN2, INV_ODD, LGAMMA2, LN2, PI, PI_2, STIRLING};
use super::dd::{two_prod, two_sum, DD};
use super::explog::{exp_dd, log_dd};
use super::fenv;
use super::trig::{atan_dd, cos_k, sin_k};
use super::two_over_pi::TWO_OVER_PI_LONG;
use crate::stdlib::floatconv::{self, Format};
use core::cmp::Ordering;

// ----------------------------------------------------------------------------
// Values of a format
// ----------------------------------------------------------------------------

/// A value of a binary format, unpacked exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Val {
    /// A NaN, by its encoding. The x87 format's invalid encodings are signaling NaNs.
    Nan { bits: u128, signaling: bool },
    Inf(bool),
    Zero(bool),
    /// (-1)^neg * sig * 2^exp, with sig's top bit at bit p - 1.
    Fin(bool, u128, i64),
}

fn exp_bits(f: &Format) -> u32 {
    (f.emax as u32 + 1).trailing_zeros() + 1
}

/// The fraction field's width: p - 1, or p for the x87 format, which stores its lead bit.
fn frac_bits(f: &Format) -> u32 {
    if f.explicit_lead { f.p } else { f.p - 1 }
}

fn total_bits(f: &Format) -> u32 {
    if f.explicit_lead { 80 } else { 1 + exp_bits(f) + f.p - 1 }
}

pub(crate) fn unpack(bits: u128, f: &Format) -> Val {
    let fb = frac_bits(f);
    let eb = exp_bits(f);
    let neg = (bits >> (total_bits(f) - 1)) & 1 == 1;
    let be = ((bits >> fb) & ((1u128 << eb) - 1)) as i64;
    let frac = bits & ((1u128 << fb) - 1);
    let top = (1i64 << eb) - 1;
    let bias = f.emax as i64;
    let (sig, exp) = if f.explicit_lead {
        let lead = frac >> 63 & 1 == 1;
        if be == top {
            let rest = frac & ((1u128 << 63) - 1);
            if lead && rest == 0 {
                return Val::Inf(neg);
            }
            return Val::Nan { bits, signaling: !lead || frac >> 62 & 1 == 0 };
        }
        if be != 0 && !lead {
            // An unnormal: the x87 unit rejects it as an invalid operand.
            return Val::Nan { bits, signaling: true };
        }
        if frac == 0 {
            return Val::Zero(neg);
        }
        (frac, be.max(1) - bias - 63)
    } else {
        if be == top {
            if frac == 0 {
                return Val::Inf(neg);
            }
            return Val::Nan { bits, signaling: frac >> (fb - 1) & 1 == 0 };
        }
        if be == 0 {
            if frac == 0 {
                return Val::Zero(neg);
            }
            (frac, 1 - bias - fb as i64)
        } else {
            (frac | 1u128 << fb, be - bias - fb as i64)
        }
    };
    let shift = sig.leading_zeros() as i64 - (128 - f.p as i64);
    Val::Fin(neg, sig << shift, exp - shift)
}

/// The quiet form of a NaN's encoding, raising invalid for a signaling one.
pub(crate) fn quiet_nan(v: Val, f: &Format) -> u128 {
    let Val::Nan { bits, signaling } = v else { return default_nan(f) };
    if signaling {
        fenv::raise(fenv::FE_INVALID);
    }
    if f.explicit_lead {
        if bits >> 63 & 1 == 0 {
            return default_nan(f);
        }
        bits | 1u128 << 62
    } else {
        bits | 1u128 << (f.p - 2)
    }
}

/// The NaN an invalid operation produces: the x87 unit's is negative.
pub(crate) fn default_nan(f: &Format) -> u128 {
    f.quiet_nan(f.explicit_lead)
}

pub(crate) fn sign_bit(f: &Format) -> u128 {
    1u128 << (total_bits(f) - 1)
}

/// A domain error: EDOM, invalid, and the default NaN.
pub(crate) fn domain(f: &Format) -> u128 {
    crate::pthread::set_errno(crate::stdlib::EDOM);
    fenv::raise(fenv::FE_INVALID);
    default_nan(f)
}

/// A pole error: ERANGE, divide-by-zero, and an infinity.
pub(crate) fn pole(neg: bool, f: &Format) -> u128 {
    crate::pthread::set_errno(crate::stdlib::ERANGE);
    fenv::raise(fenv::FE_DIVBYZERO);
    f.infinity(neg)
}

/// m * 2^e (m < 2^128, `sticky` for nonzero bits below m) rounded to `f` in the current
/// direction, with errno for overflow and underflow and the exceptions raised.
pub(crate) fn round_parts(neg: bool, m: u128, e: i64, sticky: bool, f: &Format) -> u128 {
    let c = floatconv::from_binary(neg, m, e.clamp(-(1 << 40), 1 << 40), sticky, f, floatconv::current_round());
    super::report(&c);
    c.bits
}

/// An overflowing result of the given sign.
pub(crate) fn overflow(neg: bool, f: &Format) -> u128 {
    round_parts(neg, 1, 1 << 30, false, f)
}

/// An underflowing nonzero result of the given sign.
pub(crate) fn underflow(neg: bool, f: &Format) -> u128 {
    round_parts(neg, 1, -(1 << 30), true, f)
}

// ----------------------------------------------------------------------------
// X: m * 2^e
// ----------------------------------------------------------------------------

const LIMIT: i64 = 1 << 40;

#[derive(Clone, Copy, Debug)]
pub(crate) struct X {
    pub(crate) m: DD,
    pub(crate) e: i64,
}

impl X {
    pub(crate) const ZERO: X = X { m: DD::from(0.0), e: 0 };
    pub(crate) const ONE: X = X { m: DD::from(1.0), e: 0 };

    /// m * 2^e, renormalized so |m.hi| lies in [1, 2).
    pub(crate) fn new(m: DD, e: i64) -> X {
        if m.hi == 0.0 || !m.hi.is_finite() {
            return X::ZERO;
        }
        let (_, ex) = decompose(m.hi);
        let k = ex + 52;
        let m = if k == 0 { m } else { m.scale(-k) };
        X { m, e: (e + k as i64).clamp(-LIMIT, LIMIT) }
    }

    pub(crate) fn from_dd(m: DD) -> X {
        X::new(m, 0)
    }

    pub(crate) fn from_f64(x: f64) -> X {
        X::new(DD::from(x), 0)
    }

    /// (-1)^neg * sig * 2^exp to about 107 bits: sig's top 53 bits rounded to nearest, and
    /// the signed rest rounded to nearest, so a value a double-double holds, such as
    /// 1 - 2^-113, stays exact.
    pub(crate) fn from_parts(neg: bool, sig: u128, exp: i64) -> X {
        if sig == 0 {
            return X::ZERO;
        }
        // s: sig with its top bit at bit 125 (a 127- or 128-bit sig loses its lowest bits).
        let lz = sig.leading_zeros() as i64;
        let s = if lz >= 2 { sig << (lz - 2) } else { sig >> (2 - lz) };
        let hi = (s >> 64) as u64 as f64;
        let lo = (s as i128 - ((hi as u64 as i128) << 64)) as f64 * super::basic::pow2(-64);
        let m = two_sum(hi, lo);
        let m = if neg { m.neg() } else { m };
        X::new(m, exp + 66 - lz)
    }

    pub(crate) fn is_zero(self) -> bool {
        self.m.hi == 0.0
    }

    pub(crate) fn is_neg(self) -> bool {
        self.m.hi < 0.0
    }

    pub(crate) fn neg(self) -> X {
        X { m: self.m.neg(), e: self.e }
    }

    pub(crate) fn abs(self) -> X {
        if self.is_neg() { self.neg() } else { self }
    }

    pub(crate) fn scale(self, k: i64) -> X {
        if self.is_zero() { self } else { X { m: self.m, e: (self.e + k).clamp(-LIMIT, LIMIT) } }
    }

    pub(crate) fn add(self, o: X) -> X {
        if self.is_zero() {
            return o;
        }
        if o.is_zero() {
            return self;
        }
        let d = self.e - o.e;
        if d > 110 {
            self
        } else if d < -110 {
            o
        } else if d >= 0 {
            X::new(self.m.add(o.m.scale(-d as i32)), self.e)
        } else {
            X::new(self.m.scale(d as i32).add(o.m), o.e)
        }
    }

    pub(crate) fn sub(self, o: X) -> X {
        self.add(o.neg())
    }

    pub(crate) fn add_f(self, b: f64) -> X {
        self.add(X::from_f64(b))
    }

    pub(crate) fn mul(self, o: X) -> X {
        X::new(self.m.mul(o.m), self.e + o.e)
    }

    pub(crate) fn mul_f(self, b: f64) -> X {
        X::new(self.m.mul_f(b), self.e)
    }

    pub(crate) fn mul_dd(self, b: DD) -> X {
        X::new(self.m.mul(b), self.e)
    }

    pub(crate) fn sqr(self) -> X {
        self.mul(self)
    }

    /// self / o for nonzero o.
    pub(crate) fn div(self, o: X) -> X {
        X::new(self.m.div(o.m), self.e - o.e)
    }

    pub(crate) fn recip(self) -> X {
        X::ONE.div(self)
    }

    /// The square root of a non-negative value.
    pub(crate) fn sqrt(self) -> X {
        if self.is_zero() {
            return X::ZERO;
        }
        let (m, e) = if self.e & 1 != 0 { (self.m.mul_f(2.0), self.e - 1) } else { (self.m, self.e) };
        X::new(m.sqrt(), e / 2)
    }

    /// The value as a double-double, for |e| below about 1000.
    pub(crate) fn to_dd(self) -> DD {
        if self.is_zero() { DD::from(0.0) } else { self.m.scale(self.e as i32) }
    }

    pub(crate) fn cmp_abs(self, o: X) -> Ordering {
        match (self.is_zero(), o.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        let (a, b) = (self.abs(), o.abs());
        if a.e != b.e {
            return a.e.cmp(&b.e);
        }
        let d = a.m.sub(b.m);
        d.hi.partial_cmp(&0.0).unwrap_or(Ordering::Equal)
    }

    pub(crate) fn cmp(self, o: X) -> Ordering {
        let d = self.sub(o);
        if d.is_zero() { Ordering::Equal } else if d.is_neg() { Ordering::Less } else { Ordering::Greater }
    }
}

/// An X rounded once into `f`, with the sign of a zero result given by `neg_zero`.
pub(crate) fn round_x(x: X, neg_zero: bool, f: &Format) -> u128 {
    if x.is_zero() {
        return f.zero(neg_zero);
    }
    let neg = x.is_neg();
    let m = if neg { x.m.neg() } else { x.m };
    let (mh, eh) = decompose(m.hi);
    if m.lo == 0.0 {
        return round_parts(neg, mh as u128, eh as i64 + x.e, false, f);
    }
    let (ml, el) = decompose(m.lo);
    let shift = (eh - el) as i64;
    let (s, base, sticky) = if shift <= 74 {
        let hi = (mh as u128) << shift;
        (if m.lo < 0.0 { hi - ml as u128 } else { hi + ml as u128 }, el as i64, false)
    } else {
        let hi = (mh as u128) << 74;
        (if m.lo < 0.0 { hi - 1 } else { hi }, eh as i64 - 74, true)
    };
    round_parts(neg, s, base + x.e, sticky, f)
}

/// A finite value of a format as an X.
pub(crate) fn x_of(v: Val) -> X {
    match v {
        Val::Fin(neg, sig, exp) => X::from_parts(neg, sig, exp),
        _ => X::ZERO,
    }
}

/// x - c for x = (-1)^neg sig 2^exp and an integer |c| <= 32, from the exact difference:
/// near c the subtraction cancels the bits an X of x keeps and leaves those it dropped.
pub(crate) fn sub_int(neg: bool, sig: u128, exp: i64, c: i64) -> X {
    debug_assert!(c.abs() <= 32);
    if (-120..=0).contains(&exp) && sig >> 120 == 0 {
        let x = if neg { -(sig as i128) } else { sig as i128 };
        let d = x - ((c as i128) << -exp);
        return X::from_parts(d < 0, d.unsigned_abs(), exp);
    }
    // Here x lies above 2^p or below 2^-7: far from c, nothing cancels.
    X::from_parts(neg, sig, exp).add_f(-(c as f64))
}

// ----------------------------------------------------------------------------
// Constants
// ----------------------------------------------------------------------------

pub(crate) fn pi() -> X {
    X::from_dd(PI)
}

pub(crate) fn pi_2() -> X {
    X::from_dd(PI_2)
}

pub(crate) fn ln2() -> X {
    X::from_dd(LN2)
}

// ----------------------------------------------------------------------------
// Exponentials and logarithms
// ----------------------------------------------------------------------------

/// e^a, for |a| below 2^40 (beyond which every format overflows or underflows).
pub(crate) fn exp_x(a: X) -> X {
    if a.is_zero() {
        return X::ONE;
    }
    if a.e < -1000 {
        return X::ONE.add(a);
    }
    let ad = a.to_dd();
    let k = round(ad.hi * INV_LN2.hi);
    let r = ad.sub(LN2.mul_f(k));
    let (m, k2) = exp_dd(r);
    X::new(m, k as i64 + k2 as i64)
}

/// ln 2 - (LN2.hi + LN2.lo), the third word of ln 2: about 2^-110.
const LN2_LO2: f64 = f64::from_bits(0x3907b57a079a1934);

/// x = (-1)^neg sig 2^exp exactly as the sum of three doubles, largest first.
pub(crate) fn split3(neg: bool, sig: u128, exp: i64) -> [f64; 3] {
    let lz = sig.leading_zeros();
    let s = sig << lz;
    let e = exp - lz as i64;
    let word = |shift: u32, bits: u32| -> f64 {
        let v = ((s >> shift) & ((1u128 << bits) - 1)) as u64 as f64;
        let v = v * scale_pow2(e + shift as i64);
        if neg { -v } else { v }
    };
    [word(75, 53), word(22, 53), word(0, 22)]
}

/// 2^n, for n in a double's range.
fn scale_pow2(n: i64) -> f64 {
    super::basic::pow2(n.clamp(-1074, 1023) as i32)
}

/// e^x for x = (-1)^neg sig 2^exp with |x| below 2^14: the whole argument, all of its
/// bits, is reduced by k ln 2 with ln 2 to about 160 bits, so a long double argument
/// near the top of the range keeps its precision.
pub(crate) fn exp_exact(neg: bool, sig: u128, exp: i64) -> X {
    if exp < -900 {
        // e^x = 1 + x to far beyond 106 bits.
        return X::ONE.add(X::from_parts(neg, sig, exp));
    }
    let [p0, p1, p2] = split3(neg, sig, exp);
    let k = round(p0 * INV_LN2.hi);
    let kh = two_prod(LN2.hi, k);
    let kl = two_prod(LN2.lo, k);
    let r = two_sum(p0, -kh.hi)
        .add_f(-kh.lo)
        .add_f(p1)
        .add_f(p2)
        .add(kl.neg())
        .add_f(-LN2_LO2 * k);
    let (m, k2) = exp_dd(r);
    X::new(m, k as i64 + k2 as i64)
}

/// e^a - 1, without cancellation for small |a|.
pub(crate) fn expm1_x(a: X) -> X {
    if a.is_zero() {
        return a;
    }
    if a.e < -1000 {
        return a.add(a.sqr().scale(-1));
    }
    if a.e >= -1 {
        return exp_x(a).add_f(-1.0);
    }
    // |a| < 1/2: the Taylor series sum a^n / n!, n >= 1.
    let r = a.to_dd();
    let mut e = INV_FACT[30];
    for n in (1..30).rev() {
        e = e.mul(r).add(INV_FACT[n]);
    }
    X::from_dd(e.mul(r))
}

/// ln a for a > 0.
pub(crate) fn log_x(a: X) -> X {
    if a.e.abs() < 1000 {
        X::from_dd(log_dd(a.to_dd()))
    } else {
        X::from_dd(LN2.mul_f(a.e as f64).add(log_dd(a.m)))
    }
}

/// ln(1 + t) for t > -1, without cancellation for small |t|.
pub(crate) fn log1p_x(t: X) -> X {
    if t.is_zero() {
        return t;
    }
    if t.e < -1000 {
        return t.sub(t.sqr().scale(-1));
    }
    if t.e >= -2 {
        return log_x(t.add_f(1.0));
    }
    // |t| < 1/4: ln(1 + t) = 2 atanh(f), f = t / (2 + t), |f| < 0.143.
    let f = t.div(t.add_f(2.0)).to_dd();
    let s = f.sqr();
    let mut sum = INV_ODD[23];
    for k in (0..23).rev() {
        sum = sum.mul(s).add(INV_ODD[k]);
    }
    X::from_dd(f.mul(sum).mul_f(2.0))
}

// ----------------------------------------------------------------------------
// Trigonometric functions
// ----------------------------------------------------------------------------

/// Bit `b` of a little-endian multi-word number.
fn bit(p: &[u64], b: usize) -> u64 {
    p.get(b / 64).map_or(0, |w| w >> (b % 64) & 1)
}

/// Bits [lo, lo + 128) of a little-endian multi-word number; bits below 0 read as zero.
fn bits128(p: &[u64], lo: i64) -> u128 {
    let mut v = 0u128;
    for k in (0..128).rev() {
        let b = lo + k;
        v = (v << 1) | if b < 0 { 0 } else { bit(p, b as usize) as u128 };
    }
    v
}

const WINDOW_WORDS: usize = 6;

/// For x = sig * 2^exp >= pi/4: x = (4j + n) pi/2 + r, |r| <= pi/4, as (n, r).
pub(crate) fn reduce_pio2(sig: u128, exp: i64) -> (u32, X) {
    // Bits of 2/pi of weight 2^-i with i < exp - 1 only add multiples of 4.
    let s = (exp - 1).max(1) as usize;
    // b: WINDOW_WORDS words of 2/pi from bit s, little-endian.
    let mut b = [0u64; WINDOW_WORDS];
    for (j, w) in b.iter_mut().rev().enumerate() {
        let o = s - 1 + 64 * j;
        let (i, sh) = (o / 64, o % 64);
        let hi = TWO_OVER_PI_LONG[i] << sh;
        let lo = if sh == 0 { 0 } else { TWO_OVER_PI_LONG[i + 1] >> (64 - sh) };
        *w = hi | lo;
    }
    // p = sig * b.
    let a = [sig as u64, (sig >> 64) as u64];
    let mut p = [0u64; WINDOW_WORDS + 2];
    for (i, &ai) in a.iter().enumerate() {
        let mut carry = 0u128;
        for (j, &bj) in b.iter().enumerate() {
            let v = ai as u128 * bj as u128 + p[i + j] as u128 + carry;
            p[i + j] = v as u64;
            carry = v >> 64;
        }
        p[i + WINDOW_WORDS] = carry as u64;
    }
    // x * 2/pi = p * 2^(exp - s - 64 WINDOW_WORDS + 1): the binary point sits above bit l.
    let l = s as i64 + 64 * WINDOW_WORDS as i64 - 1 - exp;
    let mut n = (bit(&p, l as usize) | bit(&p, l as usize + 1) << 1) as u32;
    let neg = bit(&p, l as usize - 1) == 1;
    if neg {
        // The fraction is at least 1/2: take 1 - fraction, a two's complement below l.
        n = (n + 1) & 3;
        let mut borrow = 1u128;
        for w in 0..p.len() {
            let v = (!p[w]) as u128 + borrow;
            p[w] = v as u64;
            borrow = v >> 64;
        }
        for b in l as usize..64 * p.len() {
            p[b / 64] &= !(1u64 << (b % 64));
        }
    }
    let top = (0..l).rev().find(|&b| bit(&p, b as usize) == 1);
    let r = match top {
        None => X::ZERO,
        Some(t) => X::from_parts(neg, bits128(&p, t - 127), t - 127 - l).mul(pi_2()),
    };
    (n, r)
}

/// (sin r, cos r) for |r| <= pi/4.
fn sincos_kernel(r: X) -> (X, X) {
    if r.is_zero() {
        return (r, X::ONE);
    }
    if r.e < -500 {
        return (r, X::ONE.sub(r.sqr().scale(-1)));
    }
    let d = r.to_dd();
    (X::from_dd(sin_k(d)), X::from_dd(cos_k(d)))
}

/// (sin x, cos x) for finite x = (-1)^neg sig 2^exp.
pub(crate) fn sincos(neg: bool, sig: u128, exp: i64) -> (X, X) {
    let x = X::from_parts(false, sig, exp);
    let (n, r) = if x.cmp_abs(X::from_dd(super::consts::PI_4)) != Ordering::Greater {
        (0, x)
    } else {
        reduce_pio2(sig, exp)
    };
    let (s, c) = sincos_kernel(r);
    let (s, c) = match n {
        0 => (s, c),
        1 => (c, s.neg()),
        2 => (s.neg(), c.neg()),
        _ => (c.neg(), s),
    };
    (if neg { s.neg() } else { s }, c)
}

/// sin(pi x) and cos(pi x) for finite x, exact at integers and half-integers.
pub(crate) fn sincos_pi(neg: bool, sig: u128, exp: i64) -> (X, X) {
    // x mod 2 exactly: drop the bits of weight 2 and above.
    let r = if exp >= 1 {
        0
    } else if exp <= -128 {
        sig
    } else {
        let keep = (1 - exp) as u32; // bits below weight 2
        if keep >= 128 { sig } else { sig & ((1u128 << keep) - 1) }
    };
    // r * 2^exp in [0, 2): quarter = floor(4 (r 2^exp)), rest in [0, 1/4).
    let (quarter, rest) = if exp >= 1 || r == 0 {
        (0u32, X::ZERO)
    } else {
        let q_shift = -exp - 2; // r >> q_shift = floor(4 r 2^exp)
        let (q, low) = if q_shift >= 128 {
            (0, r)
        } else if q_shift <= 0 {
            ((r << (-q_shift)) as u32 & 7, 0)
        } else {
            ((r >> q_shift) as u32 & 7, r & ((1u128 << q_shift) - 1))
        };
        (q, X::from_parts(false, low, exp))
    };
    // pi x = quarter pi/4 + pi rest.
    let t = rest.mul(pi());
    let (s, c) = sincos_kernel(t);
    // Rotate by quarter * pi/4: odd quarters mix sine and cosine by sqrt(1/2).
    let h = X::from_f64(0.5).sqrt();
    let (ss, cc) = match quarter & 7 {
        0 => (s, c),
        1 => (s.add(c).mul(h), c.sub(s).mul(h)),
        2 => (c, s.neg()),
        3 => (c.sub(s).mul(h), s.add(c).mul(h).neg()),
        4 => (s.neg(), c.neg()),
        5 => (s.add(c).mul(h).neg(), s.sub(c).mul(h)),
        6 => (c.neg(), s),
        _ => (s.sub(c).mul(h), s.add(c).mul(h)),
    };
    (if neg { ss.neg() } else { ss }, cc)
}

/// atan t for t >= 0.
pub(crate) fn atan_x(t: X) -> X {
    if t.is_zero() {
        return t;
    }
    if t.e < -60 {
        return t.sub(t.mul(t.sqr()).mul_f(1.0 / 3.0));
    }
    if t.e > 60 {
        return pi_2().sub(t.recip());
    }
    X::from_dd(atan_dd(t.to_dd()))
}

/// The angle of (x, y) in (-pi, pi], for finite (x, y) not both zero; zeros carry their
/// signs as C17 F.10.1.4 requires.
pub(crate) fn atan2_x(y: X, x: X, y_neg: bool, x_neg: bool) -> X {
    let a = if y.is_zero() {
        X::ZERO
    } else if x.is_zero() {
        pi_2()
    } else if y.cmp_abs(x) == Ordering::Greater {
        pi_2().sub(atan_x(x.abs().div(y.abs())))
    } else {
        atan_x(y.abs().div(x.abs()))
    };
    let a = if x_neg { pi().sub(a) } else { a };
    if y_neg { a.neg() } else { a }
}

/// asinh t.
pub(crate) fn asinh_x(t: X) -> X {
    let a = t.abs();
    let r = if a.e < -60 {
        a.sub(a.mul(a.sqr()).mul_f(1.0 / 6.0))
    } else if a.e > 60 {
        log_x(a).add(ln2())
    } else {
        // ln(a + sqrt(a^2 + 1)) = log1p(a + a^2 / (1 + sqrt(1 + a^2))).
        let a2 = a.sqr();
        log1p_x(a.add(a2.div(a2.add_f(1.0).sqrt().add_f(1.0))))
    };
    if t.is_neg() { r.neg() } else { r }
}

/// acosh t for t >= 1, given u = t - 1 (exactly, near 1).
pub(crate) fn acosh_x(t: X, u: X) -> X {
    if t.e > 60 {
        return log_x(t).add(ln2());
    }
    log1p_x(u.add(u.mul(t.add_f(1.0)).sqrt()))
}

/// atanh t for |t| < 1, given d = 1 - |t| (exactly, near 1).
pub(crate) fn atanh_x(t: X, d: X) -> X {
    let a = t.abs();
    let r = if a.e < -60 {
        a.add(a.mul(a.sqr()).mul_f(1.0 / 3.0))
    } else {
        log1p_x(a.scale(1).div(d)).scale(-1)
    };
    if t.is_neg() { r.neg() } else { r }
}

/// (sinh a, cosh a) for |a| below 2^40.
pub(crate) fn sinh_cosh_x(a: X) -> (X, X) {
    let x = a.abs();
    let (s, c) = if x.e < -60 {
        (x.add(x.mul(x.sqr()).mul_f(1.0 / 6.0)), X::ONE.add(x.sqr().scale(-1)))
    } else if x.e < 0 {
        // em = e^x - 1: sinh x = (em + em / (em + 1)) / 2, cosh x = 1 + em^2 / (2 (em + 1)).
        let em = expm1_x(x);
        let ep = em.add_f(1.0);
        (em.add(em.div(ep)).scale(-1), X::ONE.add(em.sqr().div(ep).scale(-1)))
    } else {
        let e = exp_x(x);
        let ei = e.recip();
        (e.sub(ei).scale(-1), e.add(ei).scale(-1))
    };
    (if a.is_neg() { s.neg() } else { s }, c)
}

/// tanh a.
pub(crate) fn tanh_x(a: X) -> X {
    let x = a.abs();
    let r = if x.e < -60 {
        x.sub(x.mul(x.sqr()).mul_f(1.0 / 3.0))
    } else if x.e >= 7 {
        // Beyond 128, tanh x = 1 - 2 e^(-2x) + ... is 1 to far beyond 106 bits.
        X::ONE.sub(exp_x(x.scale(1).neg()).scale(1))
    } else {
        let em = expm1_x(x.scale(1));
        em.div(em.add_f(2.0))
    };
    if a.is_neg() { r.neg() } else { r }
}

// ----------------------------------------------------------------------------
// Error and gamma functions
// ----------------------------------------------------------------------------

/// erf x for 0 <= x <= 6: (2/sqrt(pi)) e^(-x^2) sum 2^n x^(2n+1) / (1 3 5 ... (2n+1)).
pub(crate) fn erf_series(x: X) -> X {
    if x.e < -500 {
        return x.mul_dd(super::consts::TWO_OVER_SQRT_PI);
    }
    let xd = x.to_dd();
    let x2 = xd.sqr().mul_f(2.0);
    let mut term = xd;
    let mut sum = term;
    let mut n = 0.0;
    loop {
        n += 1.0;
        term = term.mul(x2).div_f(2.0 * n + 1.0);
        sum = sum.add(term);
        if term.hi < sum.hi * 1e-34 || n > 600.0 {
            break;
        }
    }
    exp_x(X::from_dd(xd.sqr().neg())).mul_dd(sum).mul_dd(super::consts::TWO_OVER_SQRT_PI)
}

/// erfc x for x > 4, by Laplace's continued fraction
/// erfc x = (e^(-x^2)/sqrt(pi)) / (x + (1/2)/(x + 1/(x + (3/2)/(x + ...)))).
pub(crate) fn erfc_cf(x: X) -> X {
    let xx = x.approx();
    let depth = 40 + (4000.0 / (xx * xx)) as i32;
    let mut t = x;
    for n in (1..=depth).rev() {
        t = X::from_f64(n as f64 * 0.5).div(t).add(x);
    }
    exp_x(x.sqr().neg()).mul_dd(super::consts::INV_SQRT_PI).div(t)
}

impl X {
    /// The nearest double, saturating to the infinities and zero.
    pub(crate) fn approx(self) -> f64 {
        if self.is_zero() {
            0.0
        } else if self.e > 1023 {
            if self.is_neg() { f64::NEG_INFINITY } else { f64::INFINITY }
        } else if self.e < -1074 {
            0.0
        } else {
            self.to_dd().hi
        }
    }
}

/// Stirling's series for ln Gamma z, z >= 30.
fn stirling_x(z: X) -> X {
    let inv = z.recip();
    let lz = log_x(z);
    let base = z.add_f(-0.5).mul(lz).sub(z).add(X::from_dd(HALF_LN_2PI));
    if inv.e < -500 {
        return base;
    }
    let inv = inv.to_dd();
    let inv2 = inv.sqr();
    let mut sum = DD::from(0.0);
    for c in STIRLING[..14].iter().rev() {
        sum = sum.mul(inv2).add(*c);
    }
    base.add(X::from_dd(sum.mul(inv)))
}

/// ln Gamma(2 + t) for |t| <= 1/2: (1 - euler) t + sum (-1)^k (zeta(k) - 1) / k t^k, whose
/// terms past t^60 lie below 2^-120 of the sum.
fn lgamma2_series(t: X) -> X {
    let c1 = X::ONE.sub(X::from_dd(super::consts::EULER));
    if t.is_zero() || t.e < -500 {
        return c1.mul(t);
    }
    let d = t.to_dd();
    let mut sum = DD::from(0.0);
    for c in LGAMMA2.iter().rev() {
        sum = sum.add(*c).mul(d);
    }
    X::from_dd(sum).add(c1).mul(t)
}

/// ln Gamma x for x > 0, with `minus(c)` giving x - c for a small integer c. Near the zeros
/// at 1 and 2 the result is about that difference times a constant, so a caller holding
/// x's exact bits passes the exact difference.
pub(crate) fn lgamma_pos_with(x: X, minus: impl Fn(i64) -> X) -> X {
    if x.e < -1000 {
        return log_x(x).neg();
    }
    let a = x.approx();
    if a >= 30.0 {
        return stirling_x(x);
    }
    if a < 0.5 {
        // Gamma x = Gamma(2 + x) / (x (1 + x)).
        return lgamma2_series(x).sub(log1p_x(x)).sub(log_x(x));
    }
    if a < 1.5 {
        // Gamma x = Gamma(2 + t) / (1 + t), t = x - 1.
        let t = minus(1);
        return lgamma2_series(t).sub(log1p_x(t));
    }
    // Gamma x = Gamma(2 + t) (x - 1) (x - 2) ... (x - n), t = x - 2 - n in [-1/2, 1/2].
    let n = (a - 1.5) as i64;
    let mut prod = X::ONE;
    for i in 1..=n {
        prod = prod.mul(x.add_f(-(i as f64)));
    }
    let r = lgamma2_series(minus(n + 2));
    if n == 0 { r } else { r.add(log_x(prod)) }
}

/// ln Gamma x for x > 0.
pub(crate) fn lgamma_pos_x(x: X) -> X {
    lgamma_pos_with(x, |c| x.add_f(-(c as f64)))
}

/// The value of an exact rational sig * 2^exp as a u128 integer, when it is one and fits.
pub(crate) fn as_integer(sig: u128, exp: i64) -> Option<u128> {
    if exp >= 0 {
        if exp >= sig.leading_zeros() as i64 { None } else { Some(sig << exp) }
    } else if -exp >= 128 {
        if sig == 0 { Some(0) } else { None }
    } else if sig & ((1u128 << -exp) - 1) == 0 {
        Some(sig >> -exp)
    } else {
        None
    }
}

/// Whether sig * 2^exp (sig > 0) is an integer.
pub(crate) fn is_integer(sig: u128, exp: i64) -> bool {
    exp >= 0 || (-exp < 128 && sig & ((1u128 << -exp) - 1) == 0)
}

/// Whether sig * 2^exp (sig > 0) is an odd integer.
pub(crate) fn is_odd_integer(sig: u128, exp: i64) -> bool {
    if exp > 0 {
        return false;
    }
    if exp == 0 {
        return sig & 1 == 1;
    }
    -exp < 128 && is_integer(sig, exp) && (sig >> -exp) & 1 == 1
}

// ----------------------------------------------------------------------------
// Exact integer arithmetic
// ----------------------------------------------------------------------------

/// a * b as (high, low) 128-bit words.
pub(crate) fn mul_wide(a: u128, b: u128) -> (u128, u128) {
    let (a1, a0) = (a >> 64, a & u64::MAX as u128);
    let (b1, b0) = (b >> 64, b & u64::MAX as u128);
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let mid = (p00 >> 64) + (p01 & u64::MAX as u128) + (p10 & u64::MAX as u128);
    let lo = (p00 & u64::MAX as u128) | (mid << 64);
    let hi = p11 + (p01 >> 64) + (p10 >> 64) + (mid >> 64);
    (hi, lo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::floatconv::{F128, F64, X87};

    fn f64_of(x: X) -> f64 {
        f64::from_bits(round_x(x, false, &F64) as u64)
    }

    #[test]
    fn unpack_round_trips() {
        for &v in &[1.0f64, -2.5, 1e-310, f64::MAX, f64::MIN_POSITIVE] {
            let u = unpack(v.to_bits() as u128, &F64);
            let Val::Fin(neg, sig, exp) = u else { panic!("{v}") };
            assert_eq!(round_parts(neg, sig, exp, false, &F64) as u64, v.to_bits());
        }
        // 1.0 in binary128 and in the x87 format.
        assert_eq!(unpack(0x3fff_0000_0000_0000_0000_0000_0000_0000, &F128), Val::Fin(false, 1u128 << 112, -112));
        assert_eq!(unpack(0x3fff_8000_0000_0000_0000, &X87), Val::Fin(false, 1u128 << 63, -63));
        assert!(matches!(unpack(0x7fff_8000_0000_0000_0000, &X87), Val::Inf(false)));
        assert!(matches!(unpack(0x7fff_c000_0000_0000_0000, &X87), Val::Nan { signaling: false, .. }));
        assert!(matches!(unpack(0x3fff_0000_0000_0000_0001, &X87), Val::Nan { signaling: true, .. }));
    }

    #[test]
    fn x_arithmetic() {
        let a = X::from_f64(3.0);
        let b = X::from_f64(4.0);
        assert_eq!(f64_of(a.sqr().add(b.sqr()).sqrt()), 5.0);
        assert_eq!(f64_of(a.div(b)), 0.75);
        assert_eq!(f64_of(X::from_f64(1e300).mul(X::from_f64(1e300)).div(X::from_f64(1e300))), 1e300);
        assert_eq!(f64_of(exp_x(X::ZERO)), 1.0);
        assert_eq!(f64_of(log_x(X::ONE)), 0.0);
        assert_eq!(f64_of(exp_x(X::ONE)), core::f64::consts::E);
        assert_eq!(f64_of(log_x(X::from_f64(10.0))), core::f64::consts::LN_10);
        assert_eq!(f64_of(atan_x(X::ONE).scale(2)), core::f64::consts::PI);
    }

    #[test]
    fn reduction_matches_double() {
        for &x in &[1.0f64, 2.0, 10.0, 1e10, 1e22, 1e300, 5.0e-1, 3.0e5] {
            let Val::Fin(_, sig, exp) = unpack(x.to_bits() as u128, &F64) else { panic!() };
            let (s, c) = sincos(false, sig, exp);
            assert_eq!(f64_of(s), super::super::trig::sin(x), "sin {x}");
            assert_eq!(f64_of(c), super::super::trig::cos(x), "cos {x}");
        }
    }

    #[test]
    fn sinpi_exact_points() {
        let one = Val::Fin(false, 1u128 << 52, -52);
        let Val::Fin(_, sig, exp) = one else { panic!() };
        let (s, c) = sincos_pi(false, sig, exp);
        assert!(s.is_zero());
        assert_eq!(f64_of(c), -1.0);
        let (s, _) = sincos_pi(false, 1u128 << 52, -53); // 1/2
        assert_eq!(f64_of(s), 1.0);
        let (s, _) = sincos_pi(false, 1u128 << 52, -54); // 1/4
        assert_eq!(f64_of(s), core::f64::consts::FRAC_1_SQRT_2);
    }

    #[test]
    fn integer_helpers() {
        let (hi, lo) = mul_wide(u128::MAX, 2);
        assert_eq!((hi, lo), (1, u128::MAX - 1));
    }
}
