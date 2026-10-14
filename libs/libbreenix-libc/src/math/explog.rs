//! Exponentials, logarithms, powers, roots and hyperbolic functions (ISO C17 7.12.5,
//! 7.12.6, 7.12.7, F.10.2 to F.10.4).
//!
//! The cores work in double-double: exp reduces x = k ln 2 + r and sums the Taylor series
//! of e^r; log writes x = m 2^e with m in [sqrt(1/2), sqrt(2)) and sums
//! ln m = 2 atanh((m - 1)/(m + 1)). Each function rounds once at the end.

use super::basic::{copysign, decompose, fabs, scale};
use super::consts::{INV_FACT, INV_LN10, INV_LN2, INV_ODD, LN2};
use super::dd::{two_prod, two_sum, DD};
use super::{domain, overflow, pole, quiet, underflow};
use core::hint::black_box;

/// e^a = m * 2^k for |a| below about 746, with m in [0.7, 1.42].
pub(crate) fn exp_dd(a: DD) -> (DD, i32) {
    let k = (a.hi * INV_LN2.hi + if a.hi < 0.0 { -0.5 } else { 0.5 }) as i32;
    let r = a.sub(LN2.mul_f(k as f64));
    // |r| <= 0.35: 24 terms reach 2^-106.
    let mut e = INV_FACT[24];
    for n in (0..24).rev() {
        e = e.mul(r).add(INV_FACT[n]);
    }
    (e, k)
}

/// For a > 0: (e, ln m) with a = m 2^e, m in [sqrt(1/2), sqrt(2)).
fn log_parts(a: DD) -> (i32, DD) {
    let mut a = a;
    let mut extra = 0;
    if a.hi < 1e-300 {
        a = a.scale(600);
        extra = -600;
    }
    let (_, ex) = decompose(a.hi);
    let mut e = ex + 52;
    let mut m = a.scale(-e);
    // m>sqrt(2) iff m*m>2; derive the threshold without a library constant.
    if m.hi * m.hi > 2.0 {
        m = m.scale(-1);
        e += 1;
    }
    let f = m.add_f(-1.0).div(m.add_f(1.0));
    let s = f.sqr();
    // |f| <= 0.172, s <= 0.0295: 23 terms reach 2^-106.
    let mut sum = INV_ODD[23];
    for k in (0..23).rev() {
        sum = sum.mul(s).add(INV_ODD[k]);
    }
    (e + extra, f.mul(sum).mul_f(2.0))
}

/// ln a for a > 0.
pub(crate) fn log_dd(a: DD) -> DD {
    let (e, lm) = log_parts(a);
    LN2.mul_f(e as f64).add(lm)
}

/// e^a - 1 for |a| <= 64.
fn expm1_dd(a: f64) -> DD {
    let (e, k) = exp_dd(DD::from(a));
    e.scale(k).add_f(-1.0)
}

/// Rounds m * 2^k, with overflow and underflow reported by scale.
fn finish(m: DD, k: i32, neg: bool) -> f64 {
    // Apply the sign before either rounding, as required for directed modes.
    scale((if neg { m.neg() } else { m }).to_f64(), k as i64)
}

#[no_mangle]
pub extern "C" fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x > 710.0 {
        return if x.is_infinite() { x } else { overflow(false) };
    }
    if x < -746.0 {
        return if x.is_infinite() { 0.0 } else { underflow(false) };
    }
    let (m, k) = exp_dd(DD::from(x));
    finish(m, k, false)
}

#[no_mangle]
pub extern "C" fn exp2(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x >= 1024.0 {
        return if x.is_infinite() { x } else { overflow(false) };
    }
    if x < -1076.0 {
        return if x.is_infinite() { 0.0 } else { underflow(false) };
    }
    let n = super::basic::round(x);
    let r = x - n;
    let (m, k) = exp_dd(LN2.mul_f(r));
    finish(m, k + n as i32, false)
}

#[no_mangle]
pub extern "C" fn expm1(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x == 0.0 || fabs(x) < 5.551115123125783e-17 {
        return x;
    }
    if x > 40.0 {
        return exp(x);
    }
    if x == f64::NEG_INFINITY { return -1.0; }
    if x < -40.0 {
        // e^x is below half an ulp of 1.
        return black_box(f64::MIN_POSITIVE) - 1.0;
    }
    expm1_dd(x).to_f64()
}

#[no_mangle]
pub extern "C" fn log(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x == 0.0 {
        return pole(true);
    }
    if x < 0.0 {
        return domain();
    }
    if x.is_infinite() {
        return x;
    }
    log_dd(DD::from(x)).to_f64()
}

#[no_mangle]
pub extern "C" fn log2(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x == 0.0 {
        return pole(true);
    }
    if x < 0.0 {
        return domain();
    }
    if x.is_infinite() {
        return x;
    }
    let (e, lm) = log_parts(DD::from(x));
    lm.mul(INV_LN2).add_f(e as f64).to_f64()
}

#[no_mangle]
pub extern "C" fn log10(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x == 0.0 {
        return pole(true);
    }
    if x < 0.0 {
        return domain();
    }
    if x.is_infinite() {
        return x;
    }
    log_dd(DD::from(x)).mul(INV_LN10).to_f64()
}

#[no_mangle]
pub extern "C" fn log1p(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x == -1.0 {
        return pole(true);
    }
    if x < -1.0 {
        return domain();
    }
    if x.is_infinite() || x == 0.0 {
        return x;
    }
    // |log1p(x)-x| < x^2 for |x|<2^-54, below half an ulp.
    if fabs(x) < super::basic::pow2(-54) { return x; }
    log_dd(two_sum(1.0, x)).to_f64()
}

fn is_integer(y: f64) -> bool {
    super::basic::trunc(y) == y
}

fn is_odd_integer(y: f64) -> bool {
    is_integer(y) && fabs(y) < 9007199254740992.0 && (y as i64) & 1 == 1
}

/// pow, with the special cases of C17 F.10.4.4.
#[no_mangle]
pub extern "C" fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    let odd = is_odd_integer(y);
    if x == 0.0 {
        let neg = odd && x.is_sign_negative();
        return if y < 0.0 { pole(neg) } else if neg { -0.0 } else { 0.0 };
    }
    if y.is_infinite() {
        if x == -1.0 {
            return 1.0;
        }
        let big = fabs(x) > 1.0;
        return if big == (y > 0.0) { f64::INFINITY } else { 0.0 };
    }
    if x.is_infinite() {
        let neg = x < 0.0 && odd;
        let v = if y < 0.0 { 0.0 } else { f64::INFINITY };
        return if neg { -v } else { v };
    }
    if x < 0.0 && !is_integer(y) {
        return domain();
    }
    let neg = x < 0.0 && odd;
    let t = log_dd(DD::from(fabs(x))).mul_f(y);
    if t.hi > 710.0 {
        return overflow(neg);
    }
    if t.hi < -746.0 {
        return underflow(neg);
    }
    let (m, k) = exp_dd(t);
    finish(m, k, neg)
}

#[no_mangle]
pub extern "C" fn cbrt(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return quiet(x);
    }
    let t = log_dd(DD::from(fabs(x))).div_f(3.0);
    let (m, k) = exp_dd(t);
    finish(m, k, x < 0.0)
}

#[no_mangle]
pub extern "C" fn hypot(x: f64, y: f64) -> f64 {
    if x.is_infinite() || y.is_infinite() {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    let (mut a, mut b) = (fabs(x), fabs(y));
    if a < b {
        core::mem::swap(&mut a, &mut b);
    }
    if b == 0.0 {
        return a;
    }
    let ea = decompose(a).1 + 52;
    if ea - (decompose(b).1 + 52) > 60 {
        // The correction is positive but smaller than half an ulp. Adding a
        // positive value below half an ulp supplies directed rounding, including
        // overflow at DBL_MAX under FE_UPWARD, without a spurious nearest overflow.
        let before = super::fenv::flags();
        super::fenv::set_flags(super::fenv::FE_OVERFLOW, 0);
        let r = black_box(a) + black_box(b);
        let r = black_box(r);
        let raised = super::fenv::flags();
        super::fenv::set_flags(super::fenv::FE_OVERFLOW, (before | raised) & super::fenv::FE_OVERFLOW);
        if raised & super::fenv::FE_OVERFLOW != 0 { crate::pthread::set_errno(crate::stdlib::ERANGE); }
        return r;
    }
    let (sa, sb) = (scale(a, -ea as i64), scale(b, -ea as i64));
    let s = two_prod(sa, sa).add(two_prod(sb, sb)).sqrt();
    scale(s.to_f64(), ea as i64)
}

// ----------------------------------------------------------------------------
// Hyperbolic functions
// ----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn tanh(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x == 0.0 || fabs(x) < 3.725290298461914e-9 {
        return x;
    }
    if x.is_infinite() { return copysign(1.0, x); }
    if fabs(x) > 22.0 {
        return copysign(1.0 - black_box(f64::MIN_POSITIVE), x);
    }
    let e = expm1_dd(2.0 * fabs(x));
    copysign(e.div(e.add_f(2.0)).to_f64(), x)
}

#[no_mangle]
pub extern "C" fn sinh(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return quiet(x);
    }
    let a = fabs(x);
    if a < 3.725290298461914e-9 {
        return x;
    }
    if a > 22.0 {
        if a > 711.0 {
            return overflow(x < 0.0);
        }
        let (m, k) = exp_dd(DD::from(a));
        return finish(m, k - 1, x < 0.0);
    }
    let e = expm1_dd(a);
    copysign(e.add(e.div(e.add_f(1.0))).mul_f(0.5).to_f64(), x)
}

#[no_mangle]
pub extern "C" fn cosh(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    let a = fabs(x);
    if a.is_infinite() {
        return a;
    }
    if a > 22.0 {
        if a > 711.0 {
            return overflow(false);
        }
        let (m, k) = exp_dd(DD::from(a));
        return finish(m, k - 1, false);
    }
    let e = expm1_dd(a);
    e.sqr().div(e.add_f(1.0).mul_f(2.0)).add_f(1.0).to_f64()
}

#[no_mangle]
pub extern "C" fn asinh(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return quiet(x);
    }
    let a = fabs(x);
    // asinh(x)=x-x^3/6+...; below 2^-27 the correction is < half an ulp.
    if a < super::basic::pow2(-27) { return x; }
    let r = if a > 268435456.0 {
        log_dd(DD::from(a)).add(LN2)
    } else {
        // ln(a + sqrt(a^2 + 1)) = log1p(a + a^2 / (1 + sqrt(a^2 + 1))).
        let t = two_prod(a, a);
        let v = t.div(t.add_f(1.0).sqrt().add_f(1.0)).add_f(a);
        log_dd(v.add_f(1.0))
    };
    copysign(r.to_f64(), x)
}

#[no_mangle]
pub extern "C" fn acosh(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x < 1.0 {
        return domain();
    }
    if x == 1.0 {
        return 0.0;
    }
    if x.is_infinite() {
        return x;
    }
    if x > 268435456.0 {
        return log_dd(DD::from(x)).add(LN2).to_f64();
    }
    let s = two_prod(x, x).add_f(-1.0).sqrt();
    log_dd(s.add_f(x)).to_f64()
}

#[no_mangle]
pub extern "C" fn atanh(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    let a = fabs(x);
    if a > 1.0 {
        return domain();
    }
    if a == 1.0 {
        return pole(x < 0.0);
    }
    if x == 0.0 || a < 3.725290298461914e-9 {
        return x;
    }
    let r = two_sum(1.0, a).div(two_sum(1.0, -a));
    copysign(log_dd(r).mul_f(0.5).to_f64(), x)
}
