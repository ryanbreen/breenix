//! <math.h>'s long double functions, and nexttoward and nexttowardf (ISO C17 7.12,
//! Annex F), written from the standard.
//!
//! long double is IEEE binary128 on aarch64 and the x87 80-bit format on x86-64. The
//! functions below work on a value's encoding in the low bits of a u128, in the format
//! they are given, so the host tests check both; the C entry points at the end pass the
//! native one. Exact functions (rounding, decomposition, remainders, fma, sqrt, hypot,
//! the neighbours) work on the integer significands and round once through
//! stdlib::floatconv. The others evaluate in ext.rs's extended double-double and round
//! once: within an ulp of the x87 format and within 2^12 ulps of binary128 (a relative
//! error near 2^-100), except powl, whose error grows with |y log x| to about 2^-92 for
//! results near the ends of the range. Results the format holds exactly (integral powers,
//! perfect cubes, logarithms of exact powers, factorials) are exact.

use super::ext::{
    acosh_x, as_integer, asinh_x, atan2_x, atan_x, atanh_x, domain, erf_series, erfc_cf, exp_exact, exp_x,
    expm1_x, is_integer, is_odd_integer, lgamma_pos_with, lgamma_pos_x, ln2, log1p_x, log_x, mul_wide, overflow,
    pi, pi_2, pole, quiet_nan, round_parts, round_x, sign_bit, sincos, sincos_pi, sinh_cosh_x, sub_int, tanh_x,
    underflow, unpack, x_of, Val, X,
};
use super::fenv;
use crate::stdlib::floatconv::{self, Big, Format, Round};
use core::cmp::Ordering;

type Ld = u128;

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

fn is_nan(v: Val) -> bool {
    matches!(v, Val::Nan { .. })
}

/// The quiet NaN for an operation with these operands, if any is a NaN.
fn nan_of(a: Val, b: Val, f: &Format) -> Option<Ld> {
    if matches!(b, Val::Nan { signaling: true, .. }) {
        fenv::raise(fenv::FE_INVALID);
    }
    if is_nan(a) {
        Some(quiet_nan(a, f))
    } else if is_nan(b) {
        Some(quiet_nan(b, f))
    } else {
        None
    }
}

fn exact(neg: bool, sig: u128, exp: i64, f: &Format) -> Ld {
    round_parts(neg, sig, exp, false, f)
}

fn integer(neg: bool, k: u128, f: &Format) -> Ld {
    if k == 0 { f.zero(neg) } else { exact(neg, k, 0, f) }
}

fn neg_of(v: Val) -> bool {
    match v {
        Val::Inf(n) | Val::Zero(n) | Val::Fin(n, _, _) => n,
        Val::Nan { .. } => false,
    }
}

/// The order of two values that are not NaNs, of the same format or not.
fn cmp_val(a: Val, b: Val) -> Ordering {
    // Magnitude class: zero < finite < infinite.
    let key = |v: Val| match v {
        Val::Zero(_) => (0, 0i64, 0u128),
        Val::Fin(_, sig, exp) => {
            let lz = sig.leading_zeros();
            (1, exp + 128 - lz as i64, sig << lz)
        }
        _ => (2, 0, 0),
    };
    let (an, bn) = (neg_of(a) && !matches!(a, Val::Zero(_)), neg_of(b) && !matches!(b, Val::Zero(_)));
    match (an, bn) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => key(a).cmp(&key(b)),
        (true, true) => key(b).cmp(&key(a)),
    }
}

/// The current rounding direction.
fn direction() -> Round {
    floatconv::current_round()
}

// ----------------------------------------------------------------------------
// Rounding to integers
// ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Trunc,
    Floor,
    Ceil,
    HalfAway,
    Current,
}

/// |x| = sig 2^exp with exp < 0, rounded to an integer by `mode`, and whether that
/// changed it.
fn int_of(neg: bool, sig: u128, exp: i64, mode: Mode) -> (u128, bool) {
    let sh = -exp;
    let (int, rem, half) = if sh >= 128 {
        (0, sig, None)
    } else {
        (sig >> sh, sig & ((1u128 << sh) - 1), Some(1u128 << (sh - 1)))
    };
    let above_half = |strict: bool| match half {
        Some(h) => if strict { rem > h } else { rem >= h },
        None => false,
    };
    let up = rem != 0
        && match mode {
            Mode::Trunc => false,
            Mode::Floor => neg,
            Mode::Ceil => !neg,
            Mode::HalfAway => above_half(false),
            Mode::Current => match direction() {
                Round::Nearest => above_half(true) || (half == Some(rem) && int & 1 == 1),
                Round::Up => !neg,
                Round::Down => neg,
                Round::Zero => false,
            },
        };
    (int + up as u128, rem != 0)
}

fn round_to_int(x: Ld, f: &Format, mode: Mode, signal_inexact: bool) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Fin(neg, sig, exp) if exp < 0 => {
            let (int, inexact) = int_of(neg, sig, exp, mode);
            if inexact && signal_inexact {
                fenv::raise(fenv::FE_INEXACT);
            }
            integer(neg, int, f)
        }
        _ => x,
    }
}

pub(crate) fn trunc(x: Ld, f: &Format) -> Ld {
    round_to_int(x, f, Mode::Trunc, false)
}

pub(crate) fn floor(x: Ld, f: &Format) -> Ld {
    round_to_int(x, f, Mode::Floor, false)
}

pub(crate) fn ceil(x: Ld, f: &Format) -> Ld {
    round_to_int(x, f, Mode::Ceil, false)
}

pub(crate) fn round(x: Ld, f: &Format) -> Ld {
    round_to_int(x, f, Mode::HalfAway, false)
}

pub(crate) fn rint(x: Ld, f: &Format) -> Ld {
    round_to_int(x, f, Mode::Current, true)
}

pub(crate) fn nearbyint(x: Ld, f: &Format) -> Ld {
    round_to_int(x, f, Mode::Current, false)
}

/// C17 7.12.9: a value outside long's range is a domain error.
fn to_long(x: Ld, f: &Format, mode: Mode, signal_inexact: bool) -> i64 {
    let invalid = || {
        crate::pthread::set_errno(crate::stdlib::EDOM);
        fenv::raise(fenv::FE_INVALID);
        i64::MIN
    };
    let (neg, int, inexact) = match unpack(x, f) {
        Val::Zero(_) => return 0,
        Val::Fin(neg, sig, exp) if exp < 0 => {
            let (int, inexact) = int_of(neg, sig, exp, mode);
            (neg, int, inexact)
        }
        Val::Fin(neg, sig, exp) => match as_integer(sig, exp) {
            Some(int) => (neg, int, false),
            None => return invalid(),
        },
        _ => return invalid(),
    };
    let limit = 1u128 << 63;
    if int > limit || (int == limit && !neg) {
        return invalid();
    }
    if inexact && signal_inexact {
        fenv::raise(fenv::FE_INEXACT);
    }
    if neg { (int as i64).wrapping_neg() } else { int as i64 }
}

pub(crate) fn lrint(x: Ld, f: &Format) -> i64 {
    to_long(x, f, Mode::Current, true)
}

pub(crate) fn lround(x: Ld, f: &Format) -> i64 {
    to_long(x, f, Mode::HalfAway, false)
}

// ----------------------------------------------------------------------------
// Sign, decomposition and scaling
// ----------------------------------------------------------------------------

pub(crate) fn fabs(x: Ld, f: &Format) -> Ld {
    x & !sign_bit(f)
}

pub(crate) fn copysign(x: Ld, y: Ld, f: &Format) -> Ld {
    (x & !sign_bit(f)) | (y & sign_bit(f))
}

pub(crate) fn frexp(x: Ld, e: &mut i32, f: &Format) -> Ld {
    *e = 0;
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Fin(neg, sig, exp) => {
            *e = (exp + f.p as i64) as i32;
            exact(neg, sig, -(f.p as i64), f)
        }
        _ => x,
    }
}

pub(crate) fn scalbn(x: Ld, n: i64, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Fin(neg, sig, exp) => round_parts(neg, sig, exp + n.clamp(-(1 << 20), 1 << 20), false, f),
        _ => x,
    }
}

pub(crate) fn ilogb(x: Ld, f: &Format) -> i32 {
    let r = match unpack(x, f) {
        Val::Fin(_, _, exp) => return (exp + f.p as i64 - 1) as i32,
        Val::Nan { .. } => super::basic::FP_ILOGBNAN,
        Val::Inf(_) => i32::MAX,
        Val::Zero(_) => super::basic::FP_ILOGB0,
    };
    super::domain();
    r
}

pub(crate) fn logb(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(_) => f.infinity(false),
        Val::Zero(_) => pole(true, f),
        Val::Fin(_, _, exp) => {
            let k = exp + f.p as i64 - 1;
            integer(k < 0, k.unsigned_abs() as u128, f)
        }
    }
}

pub(crate) fn modf(x: Ld, ip: &mut Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => {
            *ip = quiet_nan(v, f);
            *ip
        }
        Val::Inf(neg) => {
            *ip = x;
            f.zero(neg)
        }
        Val::Zero(_) => {
            *ip = x;
            x
        }
        Val::Fin(neg, sig, exp) => {
            if exp >= 0 {
                *ip = x;
                return f.zero(neg);
            }
            let sh = -exp;
            if sh >= 128 {
                *ip = f.zero(neg);
                return x;
            }
            let frac = sig & ((1u128 << sh) - 1);
            *ip = integer(neg, sig >> sh, f);
            if frac == 0 { f.zero(neg) } else { exact(neg, frac, exp, f) }
        }
    }
}

pub(crate) fn nan(f: &Format) -> Ld {
    f.quiet_nan(false)
}

// ----------------------------------------------------------------------------
// Wide integers: exact sums, square roots
// ----------------------------------------------------------------------------

const WORDS: usize = 16;

/// A little-endian unsigned integer of 1024 bits.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Wide([u64; WORDS]);

impl Wide {
    fn zero() -> Wide {
        Wide([0; WORDS])
    }

    /// (hi * 2^128 + lo) << sh, for values that fit.
    fn from(hi: u128, lo: u128, sh: usize) -> Wide {
        let mut w = Wide::zero();
        let words = [lo as u64, (lo >> 64) as u64, hi as u64, (hi >> 64) as u64];
        for (i, &v) in words.iter().enumerate() {
            if v == 0 {
                continue;
            }
            let b = i * 64 + sh;
            let (k, s) = (b / 64, b % 64);
            w.0[k] |= v << s;
            if s != 0 && k + 1 < WORDS {
                w.0[k + 1] |= v >> (64 - s);
            }
        }
        w
    }

    fn bit(&self, b: i64) -> bool {
        b >= 0 && (b as usize) < WORDS * 64 && self.0[b as usize / 64] >> (b % 64) & 1 == 1
    }

    fn shl(&self, bits: usize) -> Wide {
        let mut r = Wide::zero();
        let (k, s) = (bits / 64, bits % 64);
        for i in (k..WORDS).rev() {
            let lo = self.0[i - k] << s;
            let carry = if s != 0 && i > k { self.0[i - k - 1] >> (64 - s) } else { 0 };
            r.0[i] = lo | carry;
        }
        r
    }

    fn top(&self) -> Option<i64> {
        (0..WORDS).rev().find(|&i| self.0[i] != 0).map(|i| i as i64 * 64 + 63 - self.0[i].leading_zeros() as i64)
    }

    fn add(&self, o: &Wide) -> Wide {
        let mut r = Wide::zero();
        let mut carry = 0u128;
        for i in 0..WORDS {
            let v = self.0[i] as u128 + o.0[i] as u128 + carry;
            r.0[i] = v as u64;
            carry = v >> 64;
        }
        r
    }

    /// self - o, for self >= o.
    fn sub(&self, o: &Wide) -> Wide {
        let mut r = Wide::zero();
        let mut borrow = 0i128;
        for i in 0..WORDS {
            let v = self.0[i] as i128 - o.0[i] as i128 - borrow;
            r.0[i] = v as u64;
            borrow = (v < 0) as i128;
        }
        r
    }

    fn cmp(&self, o: &Wide) -> Ordering {
        for i in (0..WORDS).rev() {
            match self.0[i].cmp(&o.0[i]) {
                Ordering::Equal => continue,
                c => return c,
            }
        }
        Ordering::Equal
    }

    /// The 128 bits from bit `lo` up, and whether any bit below `lo` is set.
    fn window(&self, lo: i64) -> (u128, bool) {
        let mut v = 0u128;
        for b in (lo..lo + 128).rev() {
            v = (v << 1) | self.bit(b) as u128;
        }
        let sticky = (0..lo.max(0)).any(|b| self.bit(b));
        (v, sticky)
    }
}

/// w * 2^e rounded into `f` (w nonzero).
fn round_wide(neg: bool, w: &Wide, e: i64, f: &Format) -> Ld {
    let top = w.top().unwrap_or(0);
    let lo = top - 127;
    let (v, sticky) = w.window(lo);
    round_parts(neg, v, e + lo, sticky, f)
}

/// A signed term (-1)^neg * hi:lo * 2^exp of an exact sum.
#[derive(Clone, Copy)]
struct Term {
    neg: bool,
    hi: u128,
    lo: u128,
    exp: i64,
}

impl Term {
    fn bits(&self) -> i64 {
        if self.hi != 0 { 256 - self.hi.leading_zeros() as i64 } else { 128 - self.lo.leading_zeros() as i64 }
    }
}

/// a + b, both nonzero, rounded once into `f`. An exact zero is +0, or -0 when rounding
/// downward (IEEE 754 6.3).
fn sum_round(a: Term, b: Term, f: &Format) -> Ld {
    // Order by the position of the top bit.
    let (big, small) = if a.exp + a.bits() >= b.exp + b.bits() { (a, b) } else { (b, a) };
    let gap = (big.exp + big.bits()) - (small.exp + small.bits());
    if gap > 400 {
        // The smaller term only nudges the larger: (2 big +- 1) 2^(exp - 1) with sticky.
        let w = Wide::from(big.hi, big.lo, 2);
        let one = Wide::from(0, 1, 0);
        let w = if big.neg == small.neg { w.add(&one) } else { w.sub(&one) };
        return round_wide_sticky(big.neg, &w, big.exp - 2, f);
    }
    let e0 = big.exp.min(small.exp);
    let wa = Wide::from(big.hi, big.lo, (big.exp - e0) as usize);
    let wb = Wide::from(small.hi, small.lo, (small.exp - e0) as usize);
    if big.neg == small.neg {
        return round_wide(big.neg, &wa.add(&wb), e0, f);
    }
    match wa.cmp(&wb) {
        Ordering::Greater => round_wide(big.neg, &wa.sub(&wb), e0, f),
        Ordering::Less => round_wide(small.neg, &wb.sub(&wa), e0, f),
        Ordering::Equal => f.zero(direction() == Round::Down),
    }
}

/// As round_wide, with nonzero bits known to lie below w.
fn round_wide_sticky(neg: bool, w: &Wide, e: i64, f: &Format) -> Ld {
    let top = w.top().unwrap_or(0);
    let lo = top - 127;
    let (v, _) = w.window(lo);
    round_parts(neg, v, e + lo, true, f)
}

/// sqrt(n 2^e) for even e, rounded once into `f`: the root's bits come a pair of n's
/// bits at a time, p + 2 of them, and the rest of n and the remainder are the sticky bit.
fn sqrt_round(n: &Wide, e: i64, f: &Format) -> Ld {
    let Some(top) = n.top() else { return f.zero(false) };
    let pairs = f.p as i64 + 2;
    // At least `pairs` pairs of bits: widen a short n by whole pairs.
    let short = pairs - (top / 2 + 1);
    if short > 0 {
        return sqrt_round(&n.shl(2 * short as usize), e - 2 * short, f);
    }
    let first = top / 2;
    let last = (first - pairs + 1).max(0);
    let (mut root, mut rem) = (0u128, 0u128);
    for i in (last..=first).rev() {
        let pair = (n.bit(2 * i + 1) as u128) << 1 | n.bit(2 * i) as u128;
        rem = (rem << 2) | pair;
        let trial = (root << 2) | 1;
        root <<= 1;
        if rem >= trial {
            rem -= trial;
            root |= 1;
        }
    }
    let sticky = rem != 0 || (0..2 * last).any(|b| n.bit(b));
    round_parts(false, root, last + e / 2, sticky, f)
}

pub(crate) fn sqrt(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) | Val::Inf(false) => x,
        Val::Inf(true) | Val::Fin(true, _, _) => domain(f),
        Val::Fin(false, sig, exp) => {
            let (n, e) = if exp & 1 != 0 { (Wide::from(0, sig, 1), exp - 1) } else { (Wide::from(0, sig, 0), exp) };
            sqrt_round(&n, e, f)
        }
    }
}

pub(crate) fn hypot(x: Ld, y: Ld, f: &Format) -> Ld {
    let (a, b) = (unpack(x, f), unpack(y, f));
    if matches!(a, Val::Inf(_)) || matches!(b, Val::Inf(_)) {
        return f.infinity(false);
    }
    if let Some(n) = nan_of(a, b, f) {
        return n;
    }
    let (sa, ea, sb, eb) = match (a, b) {
        (Val::Zero(_), Val::Zero(_)) => return f.zero(false),
        (Val::Zero(_), Val::Fin(_, s, e)) | (Val::Fin(_, s, e), Val::Zero(_)) => return exact(false, s, e, f),
        (Val::Fin(_, sa, ea), Val::Fin(_, sb, eb)) => (sa, ea, sb, eb),
        _ => unreachable!(),
    };
    let (sa, ea, sb, eb) = if ea >= eb { (sa, ea, sb, eb) } else { (sb, eb, sa, ea) };
    if ea - eb > f.p as i64 + 4 {
        // b^2 is below the last bit of a^2: |a| plus a positive sliver.
        return round_parts(false, sa << 1, ea - 1, true, f);
    }
    let d = (ea - eb) as usize;
    let (ah, al) = mul_wide(sa, sa);
    let (bh, bl) = mul_wide(sb, sb);
    let n = Wide::from(ah, al, 2 * d).add(&Wide::from(bh, bl, 0));
    sqrt_round(&n, 2 * eb, f)
}

pub(crate) fn fma(a: Ld, b: Ld, c: Ld, f: &Format) -> Ld {
    let (va, vb, vc) = (unpack(a, f), unpack(b, f), unpack(c, f));
    let signaling = |v: Val| matches!(v, Val::Nan { signaling: true, .. });
    if signaling(va) || signaling(vb) || signaling(vc) {
        return domain(f);
    }
    let zero = |v: Val| matches!(v, Val::Zero(_));
    let inf = |v: Val| matches!(v, Val::Inf(_));
    let pneg = neg_of(va) != neg_of(vb);
    let invalid_product = (zero(va) && inf(vb)) || (zero(vb) && inf(va));
    let infinite_product = (inf(va) || inf(vb)) && !is_nan(va) && !is_nan(vb);
    if invalid_product || (infinite_product && inf(vc) && pneg != neg_of(vc)) {
        return domain(f);
    }
    for v in [va, vb, vc] {
        if is_nan(v) {
            return quiet_nan(v, f);
        }
    }
    if inf(vc) {
        return c;
    }
    if infinite_product {
        return f.infinity(pneg);
    }
    match (va, vb, vc) {
        (Val::Fin(_, sa, ea), Val::Fin(_, sb, eb), vc) => {
            let (hi, lo) = mul_wide(sa, sb);
            let p = Term { neg: pneg, hi, lo, exp: ea + eb };
            match vc {
                Val::Fin(nc, sc, ec) => sum_round(p, Term { neg: nc, hi: 0, lo: sc, exp: ec }, f),
                _ => round_wide(pneg, &Wide::from(hi, lo, 0), ea + eb, f),
            }
        }
        // A zero product: the sum of two zeros, or c itself.
        (_, _, Val::Zero(nc)) => {
            if pneg == nc { f.zero(nc) } else { f.zero(direction() == Round::Down) }
        }
        _ => c,
    }
}

/// a^2 + b^2 - 1 for finite a and b (zeros allowed), formed exactly and kept to 106 bits:
/// clog's ln|z| near the unit circle, where a^2 + b^2 cancels against 1.
pub(crate) fn norm_minus_one(a: Val, b: Val) -> X {
    let sq = |v: Val| match v {
        Val::Fin(_, s, e) => {
            let (hi, lo) = mul_wide(s, s);
            Some((hi, lo, 2 * e))
        }
        _ => None,
    };
    let terms: [Option<(u128, u128, i64)>; 2] = [sq(a), sq(b)];
    // Everything at the smallest exponent, which is at least -2^15 * 2 for any format.
    let e0 = terms.iter().flatten().map(|t| t.2).min().unwrap_or(0).min(0);
    if e0 < -(WORDS as i64 * 64 - 300) {
        // A square far below 1 next to one near it: 106 bits of the sum suffice.
        let x = x_of(a).sqr().add(x_of(b).sqr());
        return x.add_f(-1.0);
    }
    let mut sum = Wide::zero();
    for &(hi, lo, e) in terms.iter().flatten() {
        if e - e0 > (WORDS as i64 * 64 - 260) {
            return x_of(a).sqr().add(x_of(b).sqr()).add_f(-1.0);
        }
        sum = sum.add(&Wide::from(hi, lo, (e - e0) as usize));
    }
    let one = Wide::from(0, 1, (-e0) as usize);
    let (neg, d) = match sum.cmp(&one) {
        Ordering::Equal => return X::ZERO,
        Ordering::Greater => (false, sum.sub(&one)),
        Ordering::Less => (true, one.sub(&sum)),
    };
    let top = d.top().unwrap_or(0);
    let (v, _) = d.window(top - 127);
    X::from_parts(neg, v, top - 127 + e0)
}

/// An encoding of format `from` in format `to`: exact when `to` is the wider.
pub(crate) fn round_to(x: u128, from: &Format, to: &Format) -> u128 {
    match unpack(x, from) {
        Val::Nan { .. } => to.quiet_nan(x & sign_bit(from) != 0),
        Val::Inf(n) => to.infinity(n),
        Val::Zero(n) => to.zero(n),
        Val::Fin(n, sig, exp) => round_parts(n, sig, exp, false, to),
    }
}

/// An arithmetic operation: its exceptions, but errno as it was.
fn arithmetic(op: impl FnOnce() -> Ld) -> Ld {
    let saved = crate::pthread::errno();
    let r = op();
    crate::pthread::set_errno(saved);
    r
}

/// x * y rounded once, with IEEE 754's special values.
pub(crate) fn mul(x: Ld, y: Ld, f: &Format) -> Ld {
    // fma(x, y, -0) is x * y: -0 leaves every product, zero ones included, as it is.
    arithmetic(|| fma(x, y, f.zero(true), f))
}

/// x + y rounded once, with IEEE 754's special values.
pub(crate) fn add(x: Ld, y: Ld, f: &Format) -> Ld {
    arithmetic(|| fma(x, integer(false, 1, f), y, f))
}

/// x / y rounded once, with IEEE 754's special values.
pub(crate) fn div(x: Ld, y: Ld, f: &Format) -> Ld {
    arithmetic(|| divide(x, y, f))
}

fn divide(x: Ld, y: Ld, f: &Format) -> Ld {
    let (a, b) = (unpack(x, f), unpack(y, f));
    if let Some(n) = nan_of(a, b, f) {
        return n;
    }
    let neg = neg_of(a) != neg_of(b);
    match (a, b) {
        (Val::Inf(_), Val::Inf(_)) | (Val::Zero(_), Val::Zero(_)) => domain(f),
        (Val::Inf(_), _) => f.infinity(neg),
        (_, Val::Inf(_)) | (Val::Zero(_), _) => f.zero(neg),
        (Val::Fin(..), Val::Zero(_)) => {
            fenv::raise(fenv::FE_DIVBYZERO);
            f.infinity(neg)
        }
        (Val::Fin(_, sa, ea), Val::Fin(_, sb, eb)) => {
            let c = floatconv::convert(neg, &Big::from_u128(sa), &Big::from_u128(sb), ea - eb, false, f, direction());
            super::report(&c);
            c.bits
        }
        _ => unreachable!(),
    }
}

// ----------------------------------------------------------------------------
// Remainders
// ----------------------------------------------------------------------------

/// For finite nonzero x and y: |x| mod |y| as (significand, exponent), and the low bits
/// of the integer quotient floor(|x| / |y|).
fn rem_core(sx: u128, ex: i64, sy: u128, ey: i64) -> (u128, i64, u64) {
    if ex < ey || (ex == ey && sx < sy) {
        return (sx, ex, 0);
    }
    let mut q = (sx / sy) as u64;
    let mut r = sx % sy;
    let room = sy.leading_zeros() as i64 - 1;
    let mut d = ex - ey;
    while d > 0 {
        let s = d.min(room).min(63) as u32;
        let t = r << s;
        q = q.wrapping_shl(s).wrapping_add((t / sy) as u64);
        r = t % sy;
        d -= s as i64;
    }
    (r, ey, q)
}

fn fmod_special(a: Val, b: Val, x: Ld, f: &Format) -> Option<Ld> {
    if let Some(n) = nan_of(a, b, f) {
        return Some(n);
    }
    if matches!(a, Val::Inf(_)) || matches!(b, Val::Zero(_)) {
        return Some(domain(f));
    }
    if matches!(b, Val::Inf(_)) || matches!(a, Val::Zero(_)) {
        return Some(x);
    }
    None
}

pub(crate) fn fmod(x: Ld, y: Ld, f: &Format) -> Ld {
    let (a, b) = (unpack(x, f), unpack(y, f));
    if let Some(r) = fmod_special(a, b, x, f) {
        return r;
    }
    let (Val::Fin(neg, sx, ex), Val::Fin(_, sy, ey)) = (a, b) else { unreachable!() };
    let (r, e, _) = rem_core(sx, ex, sy, ey);
    if r == 0 { f.zero(neg) } else { exact(neg, r, e, f) }
}

/// The IEEE remainder x - n y, n the integer nearest x / y (ties to even), and n's low
/// bits with the sign of x / y.
pub(crate) fn remquo(x: Ld, y: Ld, quo: &mut i32, f: &Format) -> Ld {
    *quo = 0;
    let (a, b) = (unpack(x, f), unpack(y, f));
    if let Some(n) = nan_of(a, b, f) {
        return n;
    }
    if matches!(a, Val::Inf(_)) || matches!(b, Val::Zero(_)) {
        return domain(f);
    }
    if matches!(b, Val::Inf(_)) || matches!(a, Val::Zero(_)) {
        return x;
    }
    let (Val::Fin(nx, sx, ex), Val::Fin(ny, sy, ey)) = (a, b) else { unreachable!() };
    let (mut r, re, mut q) = rem_core(sx, ex, sy, ey);
    // Compare r 2^re with |y|/2 = sy 2^(ey - 1); r < |y| puts re at or below ey, and an
    // r far below |y| is not past its half.
    let (mut neg, mut rexp) = (nx, re);
    let beyond = if re < ey - 2 {
        false
    } else {
        let e = re.min(ey - 1);
        let r2 = Wide::from(0, r, (re - e) as usize);
        let half = Wide::from(0, sy, (ey - 1 - e) as usize);
        match r2.cmp(&half) {
            Ordering::Greater => true,
            Ordering::Equal => q & 1 == 1,
            Ordering::Less => false,
        }
    };
    if beyond {
        // |y| - r, both at exponent ey (re == ey whenever r < |y| came from a division).
        let (ry, rr) = if re == ey { (sy, r) } else { (sy << (ey - re), r) };
        r = ry - rr;
        rexp = re.min(ey);
        neg = !nx;
        q = q.wrapping_add(1);
    }
    let qv = (q & 0x7fff_ffff) as i32;
    *quo = if nx != ny { -qv } else { qv };
    if r == 0 { f.zero(nx) } else { exact(neg, r, rexp, f) }
}

pub(crate) fn remainder(x: Ld, y: Ld, f: &Format) -> Ld {
    remquo(x, y, &mut 0, f)
}

// ----------------------------------------------------------------------------
// Maximum, minimum, positive difference, neighbours
// ----------------------------------------------------------------------------

pub(crate) fn fmax(x: Ld, y: Ld, f: &Format) -> Ld {
    let (a, b) = (unpack(x, f), unpack(y, f));
    if is_nan(a) {
        return y;
    }
    if is_nan(b) || cmp_val(a, b) == Ordering::Greater || (cmp_val(a, b) == Ordering::Equal && neg_of(b)) {
        return x;
    }
    y
}

pub(crate) fn fmin(x: Ld, y: Ld, f: &Format) -> Ld {
    let (a, b) = (unpack(x, f), unpack(y, f));
    if is_nan(a) {
        return y;
    }
    if is_nan(b) || cmp_val(a, b) == Ordering::Less || (cmp_val(a, b) == Ordering::Equal && neg_of(a)) {
        return x;
    }
    y
}

pub(crate) fn fdim(x: Ld, y: Ld, f: &Format) -> Ld {
    let (a, b) = (unpack(x, f), unpack(y, f));
    if let Some(n) = nan_of(a, b, f) {
        return n;
    }
    if cmp_val(a, b) != Ordering::Greater {
        return f.zero(false);
    }
    match (a, b) {
        (Val::Inf(_), _) => x,
        (_, Val::Inf(_)) => f.infinity(false),
        (Val::Fin(na, sa, ea), Val::Fin(nb, sb, eb)) => {
            sum_round(Term { neg: na, hi: 0, lo: sa, exp: ea }, Term { neg: !nb, hi: 0, lo: sb, exp: eb }, f)
        }
        (Val::Fin(..), Val::Zero(_)) => x,
        (Val::Zero(_), _) => fabs(y, f),
        _ => unreachable!(),
    }
}

/// The neighbour of |x| above (`up`) or below it, as an encoding of `f` with x's sign.
fn neighbour(neg: bool, sig: u128, exp: i64, up: bool, f: &Format) -> Ld {
    let c = if up {
        floatconv::from_binary(neg, sig << 1, exp - 1, true, f, if neg { Round::Down } else { Round::Up })
    } else {
        floatconv::from_binary(neg, (sig << 1) - 1, exp - 1, true, f, Round::Zero)
    };
    c.bits
}

/// Overflow or underflow of a neighbour is a range error (F.10.8.3).
fn neighbour_report(r: Ld, f: &Format) {
    match unpack(r, f) {
        Val::Inf(_) => {
            crate::pthread::set_errno(crate::stdlib::ERANGE);
            fenv::raise(fenv::FE_OVERFLOW | fenv::FE_INEXACT);
        }
        Val::Zero(_) => {
            crate::pthread::set_errno(crate::stdlib::ERANGE);
            fenv::raise(fenv::FE_UNDERFLOW | fenv::FE_INEXACT);
        }
        Val::Fin(_, _, exp) if exp + (f.p as i64) - 1 < f.emin as i64 => {
            crate::pthread::set_errno(crate::stdlib::ERANGE);
            fenv::raise(fenv::FE_UNDERFLOW | fenv::FE_INEXACT);
        }
        _ => {}
    }
}

/// The next value after x (of format `fx`) toward y (of format `fy`).
pub(crate) fn next_toward(x: u128, fx: &Format, y: u128, fy: &Format) -> u128 {
    let (a, b) = (unpack(x, fx), unpack(y, fy));
    if is_nan(a) || is_nan(b) {
        if matches!(a, Val::Nan { signaling: true, .. }) || matches!(b, Val::Nan { signaling: true, .. }) {
            fenv::raise(fenv::FE_INVALID);
        }
        return match a {
            Val::Nan { .. } => quiet_nan(a, fx),
            _ => {
                // y's NaN, carried into x's format.
                let neg = y & sign_bit(fy) != 0;
                fx.quiet_nan(neg)
            }
        };
    }
    let order = cmp_val(a, b);
    if order == Ordering::Equal {
        // y converted to x's type: equal values, so x's magnitude with y's sign.
        return copysign(x, if neg_of(b) { sign_bit(fx) } else { 0 }, fx);
    }
    let toward_up = order == Ordering::Less;
    let r = match a {
        // The smallest subnormal, whose encoding is 1, with the direction's sign.
        Val::Zero(_) => 1 | if toward_up { 0 } else { sign_bit(fx) },
        Val::Inf(neg) => {
            // Only toward a finite value: the largest finite of x's sign.
            floatconv::from_binary(neg, 1, 1 << 20, false, fx, Round::Zero).bits
        }
        Val::Fin(neg, sig, exp) => neighbour(neg, sig, exp, toward_up != neg, fx),
        Val::Nan { .. } => unreachable!(),
    };
    neighbour_report(r, fx);
    r
}

pub(crate) fn next_after(x: Ld, y: Ld, f: &Format) -> Ld {
    next_toward(x, f, y, f)
}

// ----------------------------------------------------------------------------
// Exponentials and logarithms
// ----------------------------------------------------------------------------

/// The value as an X, for finite values.
fn xv(v: Val) -> X {
    x_of(v)
}

/// x's odd part and its power of two: x = o 2^t with o odd.
fn odd_part(sig: u128, exp: i64) -> (u128, i64) {
    let t = sig.trailing_zeros();
    (sig >> t, exp + t as i64)
}

pub(crate) fn exp(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(neg) => if neg { f.zero(false) } else { x },
        Val::Zero(_) => integer(false, 1, f),
        v @ Val::Fin(neg, sig, exp) => {
            let a = xv(v);
            if a.cmp(X::from_f64(12000.0)) == Ordering::Greater {
                return overflow(false, f);
            }
            if a.cmp(X::from_f64(-12000.0)) == Ordering::Less {
                return underflow(false, f);
            }
            round_x(exp_exact(neg, sig, exp), false, f)
        }
    }
}

pub(crate) fn exp2(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(neg) => if neg { f.zero(false) } else { x },
        Val::Zero(_) => integer(false, 1, f),
        Val::Fin(neg, sig, exp) => {
            if exp + (f.p as i64) > 15 {
                return if neg { underflow(false, f) } else { overflow(false, f) };
            }
            // x = n + r with n an integer and |r| < 1, both exact.
            let (n, frac) = if exp >= 0 {
                ((sig << exp) as i64, 0)
            } else if -exp >= 128 {
                (0, sig)
            } else {
                ((sig >> -exp) as i64, sig & ((1u128 << -exp) - 1))
            };
            let n = if neg { -n } else { n };
            if frac == 0 {
                return round_parts(false, 1, n, false, f);
            }
            let r = X::from_parts(neg, frac, exp);
            round_x(exp_x(r.mul(ln2())).scale(n), false, f)
        }
    }
}

pub(crate) fn expm1(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(neg) => if neg { integer(true, 1, f) } else { x },
        Val::Zero(_) => x,
        v => {
            let a = xv(v);
            if a.cmp(X::from_f64(12000.0)) == Ordering::Greater {
                return overflow(false, f);
            }
            if a.cmp(X::from_f64(-80.0)) == Ordering::Less {
                // e^x < 2^-115, below a quarter ulp of 1: e^x - 1 lies just above -1.
                return round_parts(true, (1u128 << (f.p + 1)) - 1, -(f.p as i64 + 1), true, f);
            }
            round_x(expm1_x(a), false, f)
        }
    }
}

/// ln x for finite x = sig 2^exp > 0: near 1, ln(1 + t) of the exact t = x - 1.
fn ln_parts(sig: u128, exp: i64) -> X {
    let t = sub_int(false, sig, exp, 1);
    if t.is_zero() || t.e < -2 { log1p_x(t) } else { log_x(X::from_parts(false, sig, exp)) }
}

/// A positive finite value's (sig, exp), or the result of a special operand.
fn log_operand(x: Ld, f: &Format) -> Result<(u128, i64), Ld> {
    match unpack(x, f) {
        v @ Val::Nan { .. } => Err(quiet_nan(v, f)),
        Val::Zero(_) => Err(pole(true, f)),
        Val::Inf(false) => Err(x),
        Val::Inf(true) | Val::Fin(true, _, _) => Err(domain(f)),
        Val::Fin(false, sig, exp) => Ok((sig, exp)),
    }
}

pub(crate) fn log(x: Ld, f: &Format) -> Ld {
    match log_operand(x, f) {
        Err(r) => r,
        Ok((sig, exp)) => round_x(ln_parts(sig, exp), false, f),
    }
}

pub(crate) fn log2(x: Ld, f: &Format) -> Ld {
    let (sig, exp) = match log_operand(x, f) {
        Err(r) => return r,
        Ok(a) => a,
    };
    let (o, t) = odd_part(sig, exp);
    if o == 1 {
        return integer(t < 0, t.unsigned_abs() as u128, f);
    }
    round_x(ln_parts(sig, exp).mul_dd(super::consts::INV_LN2), false, f)
}

pub(crate) fn log10(x: Ld, f: &Format) -> Ld {
    let (sig, exp) = match log_operand(x, f) {
        Err(r) => return r,
        Ok(a) => a,
    };
    // 10^k = 5^k 2^k exactly.
    let (o, t) = odd_part(sig, exp);
    let mut p = 1u128;
    let mut k = 0i64;
    while p < o {
        p *= 5;
        k += 1;
    }
    if p == o && t == k {
        return integer(false, k as u128, f);
    }
    round_x(ln_parts(sig, exp).mul_dd(super::consts::INV_LN10), false, f)
}

pub(crate) fn log1p(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) | Val::Inf(false) => x,
        Val::Inf(true) => domain(f),
        v @ Val::Fin(neg, sig, exp) => {
            let a = xv(v);
            if a.e < -2 {
                return round_x(log1p_x(a), false, f);
            }
            // 1 + x exactly: near x = -1 it keeps the bits an X of x drops.
            let s = sub_int(neg, sig, exp, -1);
            if s.is_zero() {
                pole(true, f)
            } else if s.is_neg() {
                domain(f)
            } else {
                round_x(log_x(s), false, f)
            }
        }
    }
}

/// (-1)^neg o^n 2^(t n), o odd, rounded once when o^|n| fits in 128 bits.
fn exact_power(neg: bool, o: u128, t: i64, n: i64, f: &Format) -> Option<Ld> {
    let mut p = 1u128;
    if o != 1 {
        for _ in 0..n.unsigned_abs() {
            p = p.checked_mul(o)?;
        }
    }
    let e = t.checked_mul(n)?;
    if n >= 0 {
        return Some(round_parts(neg, p, e, false, f));
    }
    if p == 1 {
        return Some(round_parts(neg, 1, e, false, f));
    }
    let c = floatconv::convert(neg, &Big::from_u64(1), &Big::from_u128(p), e, false, f, direction());
    super::report(&c);
    Some(c.bits)
}

/// pow, with the special cases of C17 F.10.4.4.
pub(crate) fn pow(x: Ld, y: Ld, f: &Format) -> Ld {
    let (a, b) = (unpack(x, f), unpack(y, f));
    let one = integer(false, 1, f);
    if matches!(b, Val::Zero(_)) || x == one {
        return one;
    }
    if let Some(n) = nan_of(a, b, f) {
        return n;
    }
    let (odd, int) = match b {
        Val::Fin(_, s, e) => (is_odd_integer(s, e), is_integer(s, e)),
        _ => (false, false),
    };
    let y_neg = neg_of(b);
    if let Val::Zero(xn) = a {
        let neg = odd && xn;
        return if y_neg { pole(neg, f) } else { f.zero(neg) };
    }
    if let Val::Inf(_) = b {
        if x == integer(true, 1, f) {
            return one;
        }
        let big = xv(a).cmp_abs(X::ONE) == Ordering::Greater || matches!(a, Val::Inf(_));
        return if big != y_neg { f.infinity(false) } else { f.zero(false) };
    }
    if let Val::Inf(xn) = a {
        let neg = xn && odd;
        return if y_neg { f.zero(neg) } else { f.infinity(neg) };
    }
    let (Val::Fin(xn, sx, ex), Val::Fin(_, sy, ey)) = (a, b) else { unreachable!() };
    if xn && !int {
        return domain(f);
    }
    let neg = xn && odd;
    if int && ey + (f.p as i64) <= 20 {
        let n = (as_integer(sy, ey).unwrap_or(0) as i64) * if y_neg { -1 } else { 1 };
        let (o, t) = odd_part(sx, ex);
        if let Some(r) = exact_power(neg, o, t, n, f) {
            return r;
        }
    }
    let t = ln_parts(sx, ex).mul(xv(b));
    if t.cmp(X::from_f64(12000.0)) == Ordering::Greater {
        return overflow(neg, f);
    }
    if t.cmp(X::from_f64(-12000.0)) == Ordering::Less {
        return underflow(neg, f);
    }
    let r = exp_x(t);
    round_x(if neg { r.neg() } else { r }, neg, f)
}

/// The integer cube root of n when n is a perfect cube.
fn exact_cube_root(n: u128) -> Option<u128> {
    let (mut lo, mut hi) = (0u128, 1u128 << 43);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        match mid.checked_mul(mid).and_then(|m| m.checked_mul(mid)) {
            Some(c) if c <= n => lo = mid,
            _ => hi = mid - 1,
        }
    }
    (lo * lo * lo == n).then_some(lo)
}

pub(crate) fn cbrt(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Fin(neg, sig, exp) => {
            let (o, t) = odd_part(sig, exp);
            if t % 3 == 0 {
                if let Some(c) = exact_cube_root(o) {
                    return exact(neg, c, t / 3, f);
                }
            }
            let r = exp_x(log_x(X::from_parts(false, sig, exp)).div(X::from_f64(3.0)));
            round_x(if neg { r.neg() } else { r }, neg, f)
        }
        _ => x,
    }
}

// ----------------------------------------------------------------------------
// Trigonometric functions
// ----------------------------------------------------------------------------

fn trig(x: Ld, f: &Format, which: u8) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(_) => domain(f),
        Val::Zero(_) => if which == 1 { integer(false, 1, f) } else { x },
        Val::Fin(neg, sig, exp) => {
            let (s, c) = sincos(neg, sig, exp);
            let r = match which {
                0 => s,
                1 => c,
                _ => s.div(c),
            };
            round_x(r, neg, f)
        }
    }
}

pub(crate) fn sin(x: Ld, f: &Format) -> Ld {
    trig(x, f, 0)
}

pub(crate) fn cos(x: Ld, f: &Format) -> Ld {
    trig(x, f, 1)
}

pub(crate) fn tan(x: Ld, f: &Format) -> Ld {
    trig(x, f, 2)
}

pub(crate) fn atan(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) => x,
        Val::Inf(neg) => round_x(if neg { pi_2().neg() } else { pi_2() }, neg, f),
        v @ Val::Fin(neg, _, _) => {
            let r = atan_x(xv(v).abs());
            round_x(if neg { r.neg() } else { r }, neg, f)
        }
    }
}

pub(crate) fn atan2(y: Ld, x: Ld, f: &Format) -> Ld {
    let (b, a) = (unpack(y, f), unpack(x, f));
    if let Some(n) = nan_of(a, b, f) {
        return n;
    }
    let (yn, xn) = (neg_of(b), neg_of(a));
    let quarter = |k: f64| pi().mul_f(k);
    let v = match (b, a) {
        (Val::Zero(_), _) => if xn { pi() } else { X::ZERO },
        (_, Val::Zero(_)) => pi_2(),
        (Val::Inf(_), Val::Inf(_)) => if xn { quarter(0.75) } else { quarter(0.25) },
        (Val::Inf(_), _) => pi_2(),
        (_, Val::Inf(_)) => if xn { pi() } else { X::ZERO },
        (vb, va) => atan2_x(xv(vb), xv(va), false, xn),
    };
    round_x(if yn { v.neg() } else { v }, yn, f)
}

/// asin (`cos` false) or acos.
fn arc(x: Ld, f: &Format, cos: bool) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(_) => domain(f),
        Val::Zero(_) => if cos { round_x(pi_2(), false, f) } else { x },
        v @ Val::Fin(neg, sig, exp) => {
            let a = xv(v);
            match a.cmp_abs(X::ONE) {
                Ordering::Greater => domain(f),
                Ordering::Equal => {
                    if cos {
                        if neg { round_x(pi(), false, f) } else { f.zero(false) }
                    } else {
                        round_x(if neg { pi_2().neg() } else { pi_2() }, neg, f)
                    }
                }
                Ordering::Less => {
                    let (one_m, one_p) = (sub_int(neg, sig, exp, 1).neg(), sub_int(neg, sig, exp, -1));
                    if cos {
                        // acos t = 2 atan(sqrt((1 - t) / (1 + t))).
                        round_x(atan_x(one_m.div(one_p).sqrt()).scale(1), false, f)
                    } else {
                        let r = atan_x(a.abs().div(one_m.mul(one_p).sqrt()));
                        round_x(if neg { r.neg() } else { r }, neg, f)
                    }
                }
            }
        }
    }
}

pub(crate) fn asin(x: Ld, f: &Format) -> Ld {
    arc(x, f, false)
}

pub(crate) fn acos(x: Ld, f: &Format) -> Ld {
    arc(x, f, true)
}

// ----------------------------------------------------------------------------
// Hyperbolic functions
// ----------------------------------------------------------------------------

pub(crate) fn sinh(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) | Val::Inf(_) => x,
        v @ Val::Fin(neg, sig, exp) => {
            let a = xv(v);
            if a.cmp_abs(X::from_f64(12000.0)) == Ordering::Greater {
                return overflow(neg, f);
            }
            if a.cmp_abs(X::from_f64(64.0)) == Ordering::Greater {
                // e^-|x| is below 2^-180 of e^|x|.
                let r = exp_exact(false, sig, exp).scale(-1);
                return round_x(if neg { r.neg() } else { r }, neg, f);
            }
            round_x(sinh_cosh_x(a).0, neg, f)
        }
    }
}

pub(crate) fn cosh(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) => integer(false, 1, f),
        Val::Inf(_) => f.infinity(false),
        v @ Val::Fin(_, sig, exp) => {
            let a = xv(v);
            if a.cmp_abs(X::from_f64(12000.0)) == Ordering::Greater {
                return overflow(false, f);
            }
            if a.cmp_abs(X::from_f64(64.0)) == Ordering::Greater {
                return round_x(exp_exact(false, sig, exp).scale(-1), false, f);
            }
            round_x(sinh_cosh_x(a).1, false, f)
        }
    }
}

pub(crate) fn tanh(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) => x,
        Val::Inf(neg) => integer(neg, 1, f),
        v @ Val::Fin(neg, _, _) => {
            let a = xv(v);
            if a.cmp_abs(X::from_f64(40.0)) == Ordering::Greater {
                // 1 - |tanh x| < 2e^-80, below a quarter ulp of 1: just inside +-1.
                return round_parts(neg, (1u128 << (f.p + 1)) - 1, -(f.p as i64 + 1), true, f);
            }
            round_x(tanh_x(a), neg, f)
        }
    }
}

pub(crate) fn asinh(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) | Val::Inf(_) => x,
        v @ Val::Fin(neg, _, _) => round_x(asinh_x(xv(v)), neg, f),
    }
}

pub(crate) fn acosh(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(false) => x,
        Val::Zero(_) | Val::Inf(true) | Val::Fin(true, _, _) => domain(f),
        Val::Fin(false, sig, exp) => {
            let u = sub_int(false, sig, exp, 1);
            if u.is_neg() {
                domain(f)
            } else if u.is_zero() {
                f.zero(false)
            } else {
                round_x(acosh_x(X::from_parts(false, sig, exp), u), false, f)
            }
        }
    }
}

pub(crate) fn atanh(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) => x,
        Val::Inf(_) => domain(f),
        v @ Val::Fin(neg, sig, exp) => {
            // 1 - |x| exactly.
            let d = sub_int(false, sig, exp, 1).neg();
            if d.is_neg() {
                domain(f)
            } else if d.is_zero() {
                pole(neg, f)
            } else {
                round_x(atanh_x(xv(v), d), neg, f)
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Error and gamma functions
// ----------------------------------------------------------------------------

pub(crate) fn erf(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) => x,
        Val::Inf(neg) => integer(neg, 1, f),
        v @ Val::Fin(neg, _, _) => {
            let a = xv(v).abs();
            let r = if a.cmp(X::from_f64(4.0)) != Ordering::Greater {
                erf_series(a)
            } else if a.cmp(X::from_f64(12.0)) == Ordering::Less {
                X::ONE.sub(erfc_cf(a))
            } else {
                // erf x lies just inside +-1.
                return round_parts(neg, (1u128 << (f.p + 1)) - 1, -(f.p as i64 + 1), true, f);
            };
            round_x(if neg { r.neg() } else { r }, neg, f)
        }
    }
}

pub(crate) fn erfc(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(_) => integer(false, 1, f),
        Val::Inf(neg) => if neg { integer(false, 2, f) } else { f.zero(false) },
        v @ Val::Fin(neg, _, _) => {
            let a = xv(v).abs();
            if neg {
                if a.cmp(X::from_f64(12.0)) == Ordering::Greater {
                    // erfc x lies just below 2.
                    return round_parts(false, (1u128 << (f.p + 1)) - 1, -(f.p as i64), true, f);
                }
                let e = if a.cmp(X::from_f64(4.0)) != Ordering::Greater { erf_series(a) } else { X::ONE.sub(erfc_cf(a)) };
                return round_x(X::ONE.add(e), false, f);
            }
            if a.cmp(X::from_f64(4.0)) != Ordering::Greater {
                return round_x(X::ONE.sub(erf_series(a)), false, f);
            }
            if a.cmp(X::from_f64(1.0e5)) == Ordering::Greater {
                return underflow(false, f);
            }
            round_x(erfc_cf(a), false, f)
        }
    }
}

/// ln |Gamma x| and the sign of Gamma x for finite x that is not a pole.
fn lgamma_signed(neg: bool, sig: u128, exp: i64) -> (X, bool) {
    let x = X::from_parts(neg, sig, exp);
    if !neg {
        return (lgamma_pos_with(x, |c| sub_int(false, sig, exp, c)), false);
    }
    // Gamma(x) Gamma(1 - x) = pi / sin(pi x).
    let (s, _) = sincos_pi(true, sig, exp);
    let sign_neg = s.is_neg();
    let v = log_x(pi()).sub(log_x(s.abs())).sub(lgamma_pos_x(X::ONE.sub(x)));
    (v, sign_neg)
}

pub(crate) fn lgamma(x: Ld, sign: &mut i32, f: &Format) -> Ld {
    *sign = 1;
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Inf(_) => fabs(x, f),
        Val::Zero(neg) => {
            if neg {
                *sign = -1;
            }
            pole(false, f)
        }
        Val::Fin(neg, sig, exp) => {
            if neg && is_integer(sig, exp) {
                return pole(false, f);
            }
            if !neg && (x == integer(false, 1, f) || x == integer(false, 2, f)) {
                return f.zero(false);
            }
            let (v, sneg) = lgamma_signed(neg, sig, exp);
            if sneg {
                *sign = -1;
            }
            round_x(v, false, f)
        }
    }
}

pub(crate) fn tgamma(x: Ld, f: &Format) -> Ld {
    match unpack(x, f) {
        v @ Val::Nan { .. } => quiet_nan(v, f),
        Val::Zero(neg) => pole(neg, f),
        Val::Inf(neg) => if neg { domain(f) } else { x },
        Val::Fin(neg, sig, exp) => {
            if neg && is_integer(sig, exp) {
                return domain(f);
            }
            let a = X::from_parts(neg, sig, exp);
            if !neg && a.cmp(X::from_f64(2000.0)) == Ordering::Greater {
                return overflow(false, f);
            }
            if neg && a.cmp(X::from_f64(-2000.0)) == Ordering::Less {
                // |Gamma x| underflows; its sign alternates between the poles.
                let (_, s) = lgamma_signed(neg, sig, exp);
                return underflow(s, f);
            }
            if !neg {
                if let Some(n) = as_integer(sig, exp) {
                    // (n - 1)! while it fits in 128 bits, rounded once.
                    let mut p = 1u128;
                    let mut ok = true;
                    for k in 2..n {
                        match p.checked_mul(k) {
                            Some(q) => p = q,
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        return round_parts(false, p, 0, false, f);
                    }
                }
            }
            let (l, s) = lgamma_signed(neg, sig, exp);
            let r = exp_x(l);
            round_x(if s { r.neg() } else { r }, s, f)
        }
    }
}

// ----------------------------------------------------------------------------
// C entry points
// ----------------------------------------------------------------------------

#[cfg(all(target_arch = "aarch64", not(test)))]
mod abi {
    //! binary128 travels in the vector registers, as Rust's f128 does.
    use super::*;
    use crate::stdlib::floatconv::F128;

    const F: &Format = &F128;

    fn b(x: f128) -> u128 {
        x.to_bits()
    }

    fn v(x: u128) -> f128 {
        f128::from_bits(x)
    }

    macro_rules! unary {
        ($($name:ident => $f:path),* $(,)?) => {$(
            #[no_mangle]
            pub extern "C" fn $name(x: f128) -> f128 {
                v($f(b(x), F))
            }
        )*};
    }

    macro_rules! binary {
        ($($name:ident => $f:path),* $(,)?) => {$(
            #[no_mangle]
            pub extern "C" fn $name(x: f128, y: f128) -> f128 {
                v($f(b(x), b(y), F))
            }
        )*};
    }

    unary!(
        acosl => acos, asinl => asin, atanl => atan, cosl => cos, sinl => sin, tanl => tan,
        acoshl => acosh, asinhl => asinh, atanhl => atanh, coshl => cosh, sinhl => sinh,
        tanhl => tanh, expl => exp, exp2l => exp2, expm1l => expm1, logl => log, log10l => log10,
        log1pl => log1p, log2l => log2, logbl => logb, cbrtl => cbrt, fabsl => fabs, sqrtl => sqrt,
        erfl => erf, erfcl => erfc, tgammal => tgamma, ceill => ceil, floorl => floor,
        nearbyintl => nearbyint, rintl => rint, roundl => round, truncl => trunc,
    );

    binary!(
        atan2l => atan2, powl => pow, copysignl => copysign, fdiml => fdim, fmaxl => fmax,
        fminl => fmin, fmodl => fmod, hypotl => hypot, remainderl => remainder,
        nextafterl => next_after, nexttowardl => next_after,
    );

    #[no_mangle]
    pub extern "C" fn nexttoward(x: f64, y: f128) -> f64 {
        f64::from_bits(next_toward(x.to_bits() as u128, &floatconv::F64, b(y), F) as u64)
    }

    #[no_mangle]
    pub extern "C" fn nexttowardf(x: f32, y: f128) -> f32 {
        f32::from_bits(next_toward(x.to_bits() as u128, &floatconv::F32, b(y), F) as u32)
    }

    #[no_mangle]
    pub extern "C" fn fmal(x: f128, y: f128, z: f128) -> f128 {
        v(fma(b(x), b(y), b(z), F))
    }

    #[no_mangle]
    pub extern "C" fn ldexpl(x: f128, n: i32) -> f128 {
        v(scalbn(b(x), n as i64, F))
    }

    #[no_mangle]
    pub extern "C" fn scalbnl(x: f128, n: i32) -> f128 {
        v(scalbn(b(x), n as i64, F))
    }

    #[no_mangle]
    pub extern "C" fn scalblnl(x: f128, n: i64) -> f128 {
        v(scalbn(b(x), n, F))
    }

    #[no_mangle]
    pub extern "C" fn ilogbl(x: f128) -> i32 {
        ilogb(b(x), F)
    }

    #[no_mangle]
    pub extern "C" fn lrintl(x: f128) -> i64 {
        lrint(b(x), F)
    }

    #[no_mangle]
    pub extern "C" fn llrintl(x: f128) -> i64 {
        lrint(b(x), F)
    }

    #[no_mangle]
    pub extern "C" fn lroundl(x: f128) -> i64 {
        lround(b(x), F)
    }

    #[no_mangle]
    pub extern "C" fn llroundl(x: f128) -> i64 {
        lround(b(x), F)
    }

    #[no_mangle]
    pub unsafe extern "C" fn frexpl(x: f128, e: *mut i32) -> f128 {
        v(frexp(b(x), &mut *e, F))
    }

    #[no_mangle]
    pub unsafe extern "C" fn modfl(x: f128, ip: *mut f128) -> f128 {
        let mut i = 0;
        let r = modf(b(x), &mut i, F);
        *ip = v(i);
        v(r)
    }

    #[no_mangle]
    pub unsafe extern "C" fn remquol(x: f128, y: f128, quo: *mut i32) -> f128 {
        v(remquo(b(x), b(y), &mut *quo, F))
    }

    #[no_mangle]
    pub unsafe extern "C" fn lgammal_r(x: f128, sign: *mut i32) -> f128 {
        v(lgamma(b(x), &mut *sign, F))
    }

    #[no_mangle]
    pub unsafe extern "C" fn lgammal(x: f128) -> f128 {
        let mut s = 1;
        let r = lgamma(b(x), &mut s, F);
        super::super::special::signgam = s;
        v(r)
    }

    #[no_mangle]
    pub extern "C" fn nanl(_tag: *const u8) -> f128 {
        v(nan(F))
    }
}

#[cfg(all(target_arch = "x86_64", not(test)))]
mod abi {
    //! The x87 format travels on the stack, in 16-byte slots, and comes back in st(0),
    //! which no Rust type expresses. Each entry point is a few instructions that move the
    //! arguments into registers for a Rust function on the encodings (a long double in
    //! two 64-bit registers, the low ten bytes meaningful) and load its result with fld.
    use super::*;
    use crate::stdlib::floatconv::X87;

    const F: &Format = &X87;

    /// The meaningful 80 bits of a stack slot.
    fn m(lo: u64, hi: u64) -> u128 {
        (lo as u128) | ((hi as u128 & 0xffff) << 64)
    }

    // A function on the encodings, with the C argument shape it serves.
    macro_rules! unary {
        ($($name:ident => $f:ident),* $(,)?) => {$(
            #[unsafe(naked)]
            #[no_mangle]
            pub unsafe extern "C" fn $name() {
                core::arch::naked_asm!(
                    "mov rdi, [rsp + 8]",
                    "mov rsi, [rsp + 16]",
                    "sub rsp, 24",
                    "call {f}",
                    "mov [rsp], rax",
                    "mov [rsp + 8], rdx",
                    "fld tbyte ptr [rsp]",
                    "add rsp, 24",
                    "ret",
                    f = sym $name::inner,
                )
            }
            mod $name {
                pub(super) extern "C" fn inner(lo: u64, hi: u64) -> u128 {
                    super::super::$f(super::m(lo, hi), super::F)
                }
            }
        )*};
    }

    macro_rules! binary {
        ($($name:ident => $f:ident),* $(,)?) => {$(
            #[unsafe(naked)]
            #[no_mangle]
            pub unsafe extern "C" fn $name() {
                core::arch::naked_asm!(
                    "mov rdi, [rsp + 8]",
                    "mov rsi, [rsp + 16]",
                    "mov rdx, [rsp + 24]",
                    "mov rcx, [rsp + 32]",
                    "sub rsp, 24",
                    "call {f}",
                    "mov [rsp], rax",
                    "mov [rsp + 8], rdx",
                    "fld tbyte ptr [rsp]",
                    "add rsp, 24",
                    "ret",
                    f = sym $name::inner,
                )
            }
            mod $name {
                pub(super) extern "C" fn inner(alo: u64, ahi: u64, blo: u64, bhi: u64) -> u128 {
                    super::super::$f(super::m(alo, ahi), super::m(blo, bhi), super::F)
                }
            }
        )*};
    }

    unary!(
        acosl => acos, asinl => asin, atanl => atan, cosl => cos, sinl => sin, tanl => tan,
        acoshl => acosh, asinhl => asinh, atanhl => atanh, coshl => cosh, sinhl => sinh,
        tanhl => tanh, expl => exp, exp2l => exp2, expm1l => expm1, logl => log, log10l => log10,
        log1pl => log1p, log2l => log2, logbl => logb, cbrtl => cbrt, fabsl => fabs, sqrtl => sqrt,
        erfl => erf, erfcl => erfc, tgammal => tgamma, ceill => ceil, floorl => floor,
        nearbyintl => nearbyint, rintl => rint, roundl => round, truncl => trunc,
    );

    binary!(
        atan2l => atan2, powl => pow, copysignl => copysign, fdiml => fdim, fmaxl => fmax,
        fminl => fmin, fmodl => fmod, hypotl => hypot, remainderl => remainder,
        nextafterl => next_after, nexttowardl => next_after,
    );

    #[unsafe(naked)]
    #[no_mangle]
    pub unsafe extern "C" fn fmal() {
        core::arch::naked_asm!(
            "mov rdi, [rsp + 8]",
            "mov rsi, [rsp + 16]",
            "mov rdx, [rsp + 24]",
            "mov rcx, [rsp + 32]",
            "mov r8, [rsp + 40]",
            "mov r9, [rsp + 48]",
            "sub rsp, 24",
            "call {f}",
            "mov [rsp], rax",
            "mov [rsp + 8], rdx",
            "fld tbyte ptr [rsp]",
            "add rsp, 24",
            "ret",
            f = sym inner_fmal,
        )
    }
    extern "C" fn inner_fmal(alo: u64, ahi: u64, blo: u64, bhi: u64, clo: u64, chi: u64) -> u128 {
        fma(m(alo, ahi), m(blo, bhi), m(clo, chi), F)
    }

    /// long double f(long double, integer or pointer): the second argument arrives in rdi.
    macro_rules! with_word {
        ($($name:ident($t:ty) => |$x:ident, $w:ident| $body:expr),* $(,)?) => {$(
            #[unsafe(naked)]
            #[no_mangle]
            pub unsafe extern "C" fn $name() {
                core::arch::naked_asm!(
                    "mov rdx, rdi",
                    "mov rdi, [rsp + 8]",
                    "mov rsi, [rsp + 16]",
                    "sub rsp, 24",
                    "call {f}",
                    "mov [rsp], rax",
                    "mov [rsp + 8], rdx",
                    "fld tbyte ptr [rsp]",
                    "add rsp, 24",
                    "ret",
                    f = sym $name::inner,
                )
            }
            mod $name {
                use super::super::*;
                use super::{m, F};
                #[allow(unused_unsafe)]
                pub(super) unsafe extern "C" fn inner(lo: u64, hi: u64, $w: $t) -> u128 {
                    let $x = m(lo, hi);
                    unsafe { $body }
                }
            }
        )*};
    }

    with_word!(
        ldexpl(i32) => |x, n| scalbn(x, n as i64, F),
        scalbnl(i32) => |x, n| scalbn(x, n as i64, F),
        scalblnl(i64) => |x, n| scalbn(x, n, F),
        frexpl(*mut i32) => |x, e| frexp(x, &mut *e, F),
        modfl(*mut u8) => |x, ip| {
            let mut i = 0;
            let r = modf(x, &mut i, F);
            core::ptr::write_unaligned(ip as *mut u64, i as u64);
            core::ptr::write_unaligned(ip.add(8) as *mut u16, (i >> 64) as u16);
            r
        },
        lgammal_r(*mut i32) => |x, s| lgamma(x, &mut *s, F),
    );

    /// An integer result of a long double: no x87 register is involved.
    macro_rules! to_integer {
        ($($name:ident -> $t:ty => $f:ident),* $(,)?) => {$(
            #[unsafe(naked)]
            #[no_mangle]
            pub unsafe extern "C" fn $name() {
                core::arch::naked_asm!(
                    "mov rdi, [rsp + 8]",
                    "mov rsi, [rsp + 16]",
                    "jmp {f}",
                    f = sym $name::inner,
                )
            }
            mod $name {
                pub(super) extern "C" fn inner(lo: u64, hi: u64) -> $t {
                    super::super::$f(super::m(lo, hi), super::F)
                }
            }
        )*};
    }

    to_integer!(
        ilogbl -> i32 => ilogb,
        lrintl -> i64 => lrint,
        llrintl -> i64 => lrint,
        lroundl -> i64 => lround,
        llroundl -> i64 => lround,
    );

    #[unsafe(naked)]
    #[no_mangle]
    pub unsafe extern "C" fn remquol() {
        core::arch::naked_asm!(
            "mov r8, rdi",
            "mov rdi, [rsp + 8]",
            "mov rsi, [rsp + 16]",
            "mov rdx, [rsp + 24]",
            "mov rcx, [rsp + 32]",
            "sub rsp, 24",
            "call {f}",
            "mov [rsp], rax",
            "mov [rsp + 8], rdx",
            "fld tbyte ptr [rsp]",
            "add rsp, 24",
            "ret",
            f = sym inner_remquol,
        )
    }
    unsafe extern "C" fn inner_remquol(alo: u64, ahi: u64, blo: u64, bhi: u64, quo: *mut i32) -> u128 {
        remquo(m(alo, ahi), m(blo, bhi), &mut *quo, F)
    }

    #[unsafe(naked)]
    #[no_mangle]
    pub unsafe extern "C" fn lgammal() {
        core::arch::naked_asm!(
            "mov rdi, [rsp + 8]",
            "mov rsi, [rsp + 16]",
            "sub rsp, 24",
            "call {f}",
            "mov [rsp], rax",
            "mov [rsp + 8], rdx",
            "fld tbyte ptr [rsp]",
            "add rsp, 24",
            "ret",
            f = sym inner_lgammal,
        )
    }
    unsafe extern "C" fn inner_lgammal(lo: u64, hi: u64) -> u128 {
        let mut s = 1;
        let r = lgamma(m(lo, hi), &mut s, F);
        super::super::special::signgam = s;
        r
    }

    #[unsafe(naked)]
    #[no_mangle]
    pub unsafe extern "C" fn nanl() {
        core::arch::naked_asm!(
            "sub rsp, 24",
            "call {f}",
            "mov [rsp], rax",
            "mov [rsp + 8], rdx",
            "fld tbyte ptr [rsp]",
            "add rsp, 24",
            "ret",
            f = sym inner_nanl,
        )
    }
    extern "C" fn inner_nanl() -> u128 {
        nan(F)
    }

    /// double nexttoward(double, long double) and float nexttowardf(float, long double): y
    /// on the stack, x in xmm0 with SSE and in rdi in the soft-float build. The inner function
    /// takes x and then y's two words, which go to the integer registers x leaves free; its
    /// result is already where the caller looks for it.
    macro_rules! toward {
        ($($name:ident => $inner:ident),* $(,)?) => {$(
            #[cfg(target_feature = "sse2")]
            #[unsafe(naked)]
            #[no_mangle]
            pub unsafe extern "C" fn $name() {
                core::arch::naked_asm!(
                    "mov rdi, [rsp + 8]",
                    "mov rsi, [rsp + 16]",
                    "jmp {f}",
                    f = sym $inner,
                )
            }
            #[cfg(not(target_feature = "sse2"))]
            #[unsafe(naked)]
            #[no_mangle]
            pub unsafe extern "C" fn $name() {
                core::arch::naked_asm!(
                    "mov rsi, [rsp + 8]",
                    "mov rdx, [rsp + 16]",
                    "jmp {f}",
                    f = sym $inner,
                )
            }
        )*};
    }

    toward!(nexttoward => inner_nexttoward, nexttowardf => inner_nexttowardf);

    extern "C" fn inner_nexttoward(x: f64, lo: u64, hi: u64) -> f64 {
        f64::from_bits(next_toward(x.to_bits() as u128, &floatconv::F64, m(lo, hi), F) as u64)
    }

    extern "C" fn inner_nexttowardf(x: f32, lo: u64, hi: u64) -> f32 {
        f32::from_bits(next_toward(x.to_bits() as u128, &floatconv::F32, m(lo, hi), F) as u32)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::floatconv::{F128, X87};

    // Generated by a Python script from the decimal module at 140 digits (pi by Machin's
    // formula), each value rounded to nearest into both formats.
    // (function, arguments, binary128 encoding, x87 encoding, exact)
    const REFS: &[(&str, &[f64], u128, u128, bool)] = &[
        ("exp", &[1.0], 0x40005bf0a8b1457695355fb8ac404e7a, 0x4000adf85458a2bb4a9b, false),
        ("exp", &[-10.5], 0x3fefcdfc263f6a0b9ed1ddd37af3ac70, 0x3fefe6fe131fb505cf69, false),
        ("exp", &[11000.0], 0x7dfc90712c344d655bb4c68a4e0ad481, 0x7dfcc838961a26b2adda, false),
        ("log", &[10.0], 0x400026bb1bbb5551582dd4adac5705a6, 0x4000935d8dddaaa8ac17, false),
        ("log", &[1e-300], 0xc0085963447f87fb5354ce1a6a649cb9, 0xc008acb1a23fc3fda9aa, false),
        ("log2", &[3.0], 0x3fff95c01a39fbd6879fa00b120a068c, 0x3fffcae00d1cfdeb43d0, false),
        ("log10", &[2.0], 0x3ffd34413509f79fef311f12b35816f9, 0x3ffd9a209a84fbcff799, false),
        ("log1p", &[1e-10], 0x3fddb7cdfd9d1d692bcdd20280995ed0, 0x3fdddbe6fece8eb495e7, false),
        ("expm1", &[1e-10], 0x3fddb7cdfd9dda4e3432568e17647173, 0x3fdddbe6feceed271a19, false),
        ("expm1", &[-0.75], 0xbffe0e25f8a081940d6881e1660074fc, 0xbffe8712fc5040ca06b4, false),
        ("sin", &[1.0], 0x3ffeaed548f090cee0418dd3d2138a1e, 0x3ffed76aa47848677021, false),
        ("cos", &[1.0], 0x3ffe14a280fb5068b923848cdb2ed0e3, 0x3ffe8a51407da8345c92, false),
        ("tan", &[1.0], 0x3fff8eb245cbee3a5b8acc7d41323141, 0x3fffc75922e5f71d2dc5, false),
        ("sin", &[1e22], 0xbffeb453ab76bf3970fa29bc83b9323e, 0xbffeda29d5bb5f9cb87d, false),
        ("cos", &[1e22], 0x3ffe0be2cef01c8f3934b90dff9cec2a, 0x3ffe85f167780e479c9a, false),
        ("sin", &[4.820814132776971e+60], 0xbffefea6b9eb7ec84cd9e617a980d9cb, 0xbffeff535cf5bf64266d, false),
        ("atan", &[1.0], 0x3ffe921fb54442d18469898cc51701b8, 0x3ffec90fdaa22168c235, false),
        ("atan", &[0.5], 0x3ffddac670561bb4f68adfc88bd97875, 0x3ffded63382b0dda7b45, false),
        ("asin", &[0.5], 0x3ffe0c152382d73658465bb32e0f567b, 0x3ffe860a91c16b9b2c23, false),
        ("acos", &[0.5], 0x3fff0c152382d73658465bb32e0f567b, 0x3fff860a91c16b9b2c23, false),
        ("acos", &[-1.0], 0x4000921fb54442d18469898cc51701b8, 0x4000c90fdaa22168c235, false),
        ("sinh", &[1.0], 0x3fff2cd9fc44eb9825a80249487f0650, 0x3fff966cfe2275cc12d4, false),
        ("cosh", &[2.0], 0x4000e18fa0df2d9bc29327f717774d0c, 0x4000f0c7d06f96cde14a, false),
        ("tanh", &[0.5], 0x3ffdd9353d7568af365128ee21c65b09, 0x3ffdec9a9ebab4579b29, false),
        ("asinh", &[1.0], 0x3ffec34366179d426cc1b1f33d1ba4a3, 0x3ffee1a1b30bcea13661, false),
        ("acosh", &[2.0], 0x3fff51242719804349be684bd0188d53, 0x3fffa892138cc021a4df, false),
        ("atanh", &[0.5], 0x3ffe193ea7aad030a976a4198d55053b, 0x3ffe8c9f53d5681854bb, false),
        ("erf", &[1.0], 0x3ffeaf767a741088ac6d0110fdbb0d27, 0x3ffed7bb3d3a08445637, false),
        ("erfc", &[1.0], 0x3ffc4226162fbddd4e4bfbbc0913cb64, 0x3ffca1130b17deeea726, false),
        ("tgamma", &[0.5], 0x3fffc5bf891b4ef6aa79c3b0520d5db9, 0x3fffe2dfc48da77b553d, false),
        ("lgamma", &[10.0], 0x400299a8921a7f7cf4b07d758b4c2bc1, 0x4002ccd4490d3fbe7a58, false),
        ("lgamma", &[1.0000000037252903], 0xbfe22788cfaca9bb2dc3c25432a08536, 0xbfe293c467d654dd96e2, false),
        ("lgamma", &[0.9999999990686774], 0x3fe02788cfcd8fcb27eb37dbf5dd9d3a, 0x3fe093c467e6c7e593f6, false),
        ("lgamma", &[2.0000000000009095], 0x3fd5b0ee60720a8716390367b7685c1b, 0x3fd5d877303905438b1d, false),
        ("lgamma", &[1.5], 0xbffbeeb95b094c1914d1a18fd62c1d35, 0xbffbf75cad84a60c8a69, false),
        ("lgamma", &[0.75], 0x3ffca051c372609edd816b9f5bec23c5, 0x3ffcd028e1b9304f6ec1, false),
        ("lgamma", &[0.25], 0x3fff49bbd81c16efb1e317135ab901c0, 0x3fffa4ddec0e0b77d8f2, false),
        ("lgamma", &[2.5], 0x3ffd2383e809a67e79f700737168fdd5, 0x3ffd91c1f404d33f3cfc, false),
        ("lgamma", &[3.0], 0x3ffe62e42fefa39ef35793c7673007e6, 0x3ffeb17217f7d1cf79ac, false),
        ("lgamma", &[7.5], 0x4001e233060e41f7f4f3d20ee266b83e, 0x4001f119830720fbfa7a, false),
        ("cbrt", &[2.0], 0x3fff428a2f98d728ae223ddab715be25, 0x3fffa14517cc6b945711, false),
        ("pow", &[2.0,0.5], 0x3fff6a09e667f3bcc908b2fb1366ea95, 0x3fffb504f333f9de6484, false),
        ("pow", &[10.0,-0.25], 0x3ffe1feb33c1c381e11737c93ff804f4, 0x3ffe8ff599e0e1c0f08c, false),
        ("atan2", &[1.0,-1.0], 0x40002d97c7f3321d234f272993d1414a, 0x400096cbe3f9990e91a8, false),
        ("sqrt", &[2.0], 0x3fff6a09e667f3bcc908b2fb1366ea95, 0x3fffb504f333f9de6484, true),
        ("hypot", &[1.0,1.0], 0x3fff6a09e667f3bcc908b2fb1366ea95, 0x3fffb504f333f9de6484, true),
        ("pow", &[3.0,-1.0], 0x3ffd5555555555555555555555555555, 0x3ffdaaaaaaaaaaaaaaab, true),
        ("cbrt", &[-27.0], 0xc0008000000000000000000000000000, 0xc000c000000000000000, true),
        ("pow", &[10.0,15.0], 0x4030c6bf526340000000000000000000, 0x4030e35fa931a0000000, true),
        ("log10", &[1000.0], 0x40008000000000000000000000000000, 0x4000c000000000000000, true),
        ("log2", &[0.125], 0xc0008000000000000000000000000000, 0xc000c000000000000000, true),
        ("exp2", &[10.0], 0x40090000000000000000000000000000, 0x40098000000000000000, true),
        ("tgamma", &[5.0], 0x40038000000000000000000000000000, 0x4003c000000000000000, true),
        ("hypot", &[3.0,4.0], 0x40014000000000000000000000000000, 0x4001a000000000000000, true),
    ];

    /// A double, exactly, in format f.
    fn ld(v: f64, f: &Format) -> Ld {
        match unpack(v.to_bits() as u128, &floatconv::F64) {
            Val::Fin(neg, sig, exp) => exact(neg, sig, exp, f),
            Val::Zero(neg) => f.zero(neg),
            Val::Inf(neg) => f.infinity(neg),
            Val::Nan { .. } => f.quiet_nan(false),
        }
    }

    fn call(name: &str, a: &[Ld], f: &Format) -> Ld {
        match name {
            "exp" => exp(a[0], f),
            "exp2" => exp2(a[0], f),
            "expm1" => expm1(a[0], f),
            "log" => log(a[0], f),
            "log2" => log2(a[0], f),
            "log10" => log10(a[0], f),
            "log1p" => log1p(a[0], f),
            "sin" => sin(a[0], f),
            "cos" => cos(a[0], f),
            "tan" => tan(a[0], f),
            "atan" => atan(a[0], f),
            "asin" => asin(a[0], f),
            "acos" => acos(a[0], f),
            "sinh" => sinh(a[0], f),
            "cosh" => cosh(a[0], f),
            "tanh" => tanh(a[0], f),
            "asinh" => asinh(a[0], f),
            "acosh" => acosh(a[0], f),
            "atanh" => atanh(a[0], f),
            "erf" => erf(a[0], f),
            "erfc" => erfc(a[0], f),
            "tgamma" => tgamma(a[0], f),
            "lgamma" => lgamma(a[0], &mut 0, f),
            "cbrt" => cbrt(a[0], f),
            "sqrt" => sqrt(a[0], f),
            "pow" => pow(a[0], a[1], f),
            "atan2" => atan2(a[0], a[1], f),
            "hypot" => hypot(a[0], a[1], f),
            _ => panic!("{name}"),
        }
    }

    /// The distance in ulps between two finite values of the same sign and binade.
    fn ulps(a: Ld, b: Ld, f: &Format) -> u128 {
        let (Val::Fin(na, sa, ea), Val::Fin(nb, sb, eb)) = (unpack(a, f), unpack(b, f)) else {
            return if a == b { 0 } else { u128::MAX };
        };
        if na != nb {
            return u128::MAX;
        }
        let e = ea.min(eb);
        let (sa, sb) = (sa << (ea - e), sb << (eb - e));
        sa.abs_diff(sb) >> (ea.max(eb) - e).min(1)
    }

    #[test]
    fn against_references() {
        for &(name, args, q, x, is_exact) in REFS {
            for (f, want, limit) in [(&F128, q, 1u128 << 12), (&X87, x, 1)] {
                let a: Vec<Ld> = args.iter().map(|&v| ld(v, f)).collect();
                let got = call(name, &a, f);
                let d = ulps(got, want, f);
                let limit = if is_exact { 0 } else { limit };
                assert!(d <= limit, "{name}{args:?} p={}: {got:#x} vs {want:#x}, {d} ulps", f.p);
            }
        }
    }

    #[test]
    fn exact_functions() {
        for f in [&F128, &X87] {
            let n = |v: f64| ld(v, f);
            assert_eq!(floor(n(-2.5), f), n(-3.0));
            assert_eq!(ceil(n(-2.5), f), n(-2.0));
            assert_eq!(trunc(n(-2.5), f), n(-2.0));
            assert_eq!(round(n(-2.5), f), n(-3.0));
            assert_eq!(rint(n(2.5), f), n(2.0));
            assert_eq!(rint(n(3.5), f), n(4.0));
            assert_eq!(floor(n(-0.25), f), n(-1.0));
            assert_eq!(ceil(n(-0.25), f), n(-0.0));
            assert_eq!(lround(n(-2.5), f), -3);
            assert_eq!(lrint(n(2.5), f), 2);
            assert_eq!(fmod(n(10.0), n(3.0), f), n(1.0));
            assert_eq!(fmod(n(-10.0), n(3.0), f), n(-1.0));
            assert_eq!(fmod(n(1e300), n(3.0), f), n(1e300 % 3.0));
            let mut q = 0;
            assert_eq!(remquo(n(10.0), n(3.0), &mut q, f), n(1.0));
            assert_eq!(q, 3);
            assert_eq!(remquo(n(11.0), n(3.0), &mut q, f), n(-1.0));
            assert_eq!(q, 4);
            assert_eq!(remainder(n(5.0), n(2.0), f), n(1.0));
            assert_eq!(remainder(n(7.0), n(2.0), f), n(-1.0));
            assert_eq!(fma(n(2.0), n(3.0), n(4.0), f), n(10.0));
            // fma(x, x, -x) for x = 1e300 = m 2^e: m^2 2^(2e) less x, which lies far below half
            // an ulp of it, is m^2 rounded to nearest with a tie broken downward.
            let Val::Fin(_, m, e) = unpack(1e300f64.to_bits() as u128, &floatconv::F64) else { unreachable!() };
            let sq = m * m;
            let drop = (128 - sq.leading_zeros()).saturating_sub(f.p);
            let (q, r) = (sq >> drop, sq & ((1u128 << drop) - 1));
            let q = if drop > 0 && r > 1u128 << (drop - 1) { q + 1 } else { q };
            assert_eq!(fma(n(1e300), n(1e300), n(-1e300), f), exact(false, q, 2 * e + drop as i64, f));
            assert_eq!(fdim(n(5.0), n(3.0), f), n(2.0));
            assert_eq!(fdim(n(3.0), n(5.0), f), n(0.0));
            assert_eq!(fmax(n(-0.0), n(0.0), f), n(0.0));
            assert_eq!(fmin(n(-0.0), n(0.0), f), n(-0.0));
            assert_eq!(copysign(n(2.0), n(-1.0), f), n(-2.0));
            assert_eq!(fabs(n(-2.0), f), n(2.0));
            let mut e = 0;
            assert_eq!(frexp(n(12.0), &mut e, f), n(0.75));
            assert_eq!(e, 4);
            assert_eq!(scalbn(n(0.75), 4, f), n(12.0));
            assert_eq!(logb(n(12.0), f), n(3.0));
            assert_eq!(ilogb(n(0.25), f), -2);
            let mut ip = 0;
            assert_eq!(modf(n(-3.25), &mut ip, f), n(-0.25));
            assert_eq!(ip, n(-3.0));
            assert_eq!(sqrt(n(16.0), f), n(4.0));
            assert_eq!(cbrt(n(0.125), f), n(0.5));
            assert_eq!(pow(n(2.0), n(-3.0), f), n(0.125));
            assert_eq!(pow(n(-2.0), n(3.0), f), n(-8.0));
            assert_eq!(log(n(1.0), f), n(0.0));
            assert_eq!(exp(n(0.0), f), n(1.0));
            assert_eq!(tgamma(n(1.0), f), n(1.0));
            assert_eq!(lgamma(n(2.0), &mut 0, f), n(0.0));
            // The neighbours of 1.
            let up = next_after(n(1.0), n(2.0), f);
            assert_eq!(unpack(up, f), Val::Fin(false, (1u128 << (f.p - 1)) + 1, -(f.p as i64 - 1)));
            let down = next_after(n(1.0), n(0.0), f);
            assert_eq!(unpack(down, f), Val::Fin(false, (1u128 << f.p) - 1, -(f.p as i64)));
            assert_eq!(next_after(down, n(2.0), f), n(1.0));
            assert_eq!(next_after(n(0.0), n(-1.0), f), 1 | sign_bit(f));
            // nexttoward(double, long double) steps the double.
            let d = next_toward(1f64.to_bits() as u128, &floatconv::F64, up, f);
            assert_eq!(f64::from_bits(d as u64), 1.0 + f64::EPSILON);
        }
    }

    /// Functions at the neighbours of 1 (1 - 2^-p below, 1 + 2^(1-p) above), where an X of the
    /// argument alone has lost what the result depends on. References from the decimal module
    /// at 140 digits, as for REFS: (function, binary128 encoding, x87 encoding).
    #[test]
    fn near_one() {
        const NEAR: &[(&str, u128, u128)] = &[
            ("atanh_down", 0x40043c133ab16db990b9ff9d97e6c709, 0x4003b437e057b116b792),
            ("acos_down", 0x3fc70000000000000000000000000000, 0x3fdfb504f333f9de6484),
            ("asin_down", 0x3fff921fb54442d18369898cc51701b8, 0x3fffc90fdaa16c63cf01),
            ("log_down", 0xbf8e0000000000000000000000000000, 0xbfbf8000000000000000),
            ("log_up", 0x3f8effffffffffffffffffffffffffff, 0x3fbfffffffffffffffff),
            ("log2_down", 0xbf8e71547652b82fe1777d0ffda0d23b, 0xbfbfb8aa3b295c17f0bc),
            ("log10_down", 0xbf8cbcb7b1526e50e32a6ab7555f5a68, 0xbfbdde5bd8a937287196),
            ("acosh_up", 0x3fc76a09e667f3bcc908b2fb1366ea95, 0x3fe08000000000000000),
            ("log1p_negdown", 0xc005394d72518e7252d35076091866f9, 0xc004b17217f7d1cf79ac),
            ("pow_down_2p", 0x3ffd78b56362cef37c6aeb7b1e0a4154, 0x3ffdbc5ab1b16779be35),
            ("lgamma_down", 0x3f8d2788cfc6fb618f49a37c7f0202a6, 0x3fbe93c467e37db0c7a6),
            ("lgamma_up", 0xbf8e2788cfc6fb618f49a37c7f0202a4, 0xbfbf93c467e37db0c7a3),
        ];
        for &(name, q, x) in NEAR {
            for (f, want, limit) in [(&F128, q, 1u128 << 12), (&X87, x, 1)] {
                let one = ld(1.0, f);
                let down = next_after(one, ld(0.0, f), f);
                let up = next_after(one, ld(2.0, f), f);
                let got = match name {
                    "atanh_down" => atanh(down, f),
                    "acos_down" => acos(down, f),
                    "asin_down" => asin(down, f),
                    "log_down" => log(down, f),
                    "log_up" => log(up, f),
                    "log2_down" => log2(down, f),
                    "log10_down" => log10(down, f),
                    "acosh_up" => acosh(up, f),
                    "log1p_negdown" => log1p(down | sign_bit(f), f),
                    "pow_down_2p" => pow(down, exact(false, 1, f.p as i64, f), f),
                    "lgamma_down" => lgamma(down, &mut 0, f),
                    "lgamma_up" => lgamma(up, &mut 0, f),
                    _ => panic!("{name}"),
                };
                let d = ulps(got, want, f);
                assert!(d <= limit, "{name} p={}: {got:#x} vs {want:#x}, {d} ulps", f.p);
            }
        }
    }
}
