//! Error and gamma functions (ISO C17 7.12.8, F.10.5; signgam from POSIX.1-2024).
//!
//! erf sums the everywhere-positive series
//!     erf x = (2/sqrt(pi)) e^(-x^2) sum_n 2^n x^(2n+1) / (1 3 5 ... (2n+1)),
//! and erfc beyond x = 4 evaluates Laplace's continued fraction
//!     erfc x = (e^(-x^2)/sqrt(pi)) / (x + (1/2)/(x + 1/(x + (3/2)/(x + ...)))).
//! ln Gamma uses Stirling's series once the argument is at least 30, the recurrence
//! Gamma(x + 1) = x Gamma(x) below that, the reflection formula for negative arguments,
//! and the first terms of the Taylor series of ln Gamma about 1 and 2 right next to its
//! zeros there, where the general path would cancel. All in double-double.

use super::basic::{copysign, fabs, scale, trunc};
use super::consts::{EULER, HALF_LN_2PI, INV_SQRT_PI, LN_PI, PI_SQ_12, STIRLING, TWO_OVER_SQRT_PI};
use super::dd::{two_prod, two_sum, DD};
use super::explog::{exp_dd, log_dd};
use super::trig::sinpi_dd;
use super::{domain, overflow, pole, underflow};
use core::hint::black_box;

/// e^(-x^2) for x >= 0 as m 2^k.
fn exp_neg_sq(x: f64) -> (DD, i32) {
    exp_dd(two_prod(x, x).neg())
}

/// erf x for 0 <= x <= 6.
fn erf_dd(x: f64) -> DD {
    let x2 = two_prod(x, x).mul_f(2.0);
    let mut term = DD::from(x);
    let mut sum = term;
    let mut n = 0.0;
    loop {
        n += 1.0;
        term = term.mul(x2).div_f(2.0 * n + 1.0);
        sum = sum.add(term);
        if term.hi < sum.hi * 1e-34 || n > 400.0 {
            break;
        }
    }
    let (m, k) = exp_neg_sq(x);
    m.scale(k).mul(sum).mul(TWO_OVER_SQRT_PI)
}

/// erfc x for x > 4, as m 2^k.
fn erfc_cf(x: f64) -> (DD, i32) {
    let depth = 40 + (3000.0 / (x * x)) as i32;
    let mut t = DD::from(x);
    for n in (1..=depth).rev() {
        t = DD::from(n as f64 * 0.5).div(t).add_f(x);
    }
    let (m, k) = exp_neg_sq(x);
    (m.mul(INV_SQRT_PI).div(t), k)
}

#[no_mangle]
pub extern "C" fn erf(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        return x + x;
    }
    let a = fabs(x);
    if a > 6.0 {
        return copysign(1.0 - black_box(f64::MIN_POSITIVE), x);
    }
    copysign(erf_dd(a).to_f64(), x)
}

#[no_mangle]
pub extern "C" fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x.is_infinite() {
        return if x > 0.0 { 0.0 } else { 2.0 };
    }
    let a = fabs(x);
    if x < 0.0 {
        if a > 6.0 {
            return 2.0 - black_box(f64::MIN_POSITIVE);
        }
        return erf_dd(a).add_f(1.0).to_f64();
    }
    if a <= 4.0 {
        return erf_dd(a).neg().add_f(1.0).to_f64();
    }
    if a > 27.3 {
        return underflow(false);
    }
    let (m, k) = erfc_cf(a);
    scale(m.to_f64(), k as i64)
}

// ----------------------------------------------------------------------------
// Gamma
// ----------------------------------------------------------------------------

/// Stirling's series for ln Gamma z, z >= 30.
fn stirling(z: DD) -> DD {
    let inv = z.recip();
    let inv2 = inv.sqr();
    let mut sum = DD::from(0.0);
    for c in STIRLING[..14].iter().rev() {
        sum = sum.mul(inv2).add(*c);
    }
    let lz = log_dd(z);
    z.add_f(-0.5).mul(lz).sub(z).add(HALF_LN_2PI).add(sum.mul(inv))
}

/// ln Gamma x for x > 0, with x given in double-double.
fn lgamma_pos(x: DD) -> DD {
    if x.hi >= 30.0 {
        return stirling(x);
    }
    // ln Gamma x = ln Gamma(x + n) - ln(x (x + 1) ... (x + n - 1)).
    let n = (30.0 - x.hi) as i32 + 1;
    let mut prod = x;
    for i in 1..n {
        prod = prod.mul(x.add_f(i as f64));
    }
    stirling(x.add_f(n as f64)).sub(log_dd(prod))
}

/// Near x = 1 and x = 2, where ln Gamma vanishes: ln Gamma(1 + t) = -gamma t +
/// (pi^2/12) t^2 - ..., and ln Gamma(2 + t) = (1 - gamma) t + (pi^2/12 - 1/2) t^2 - ...
/// For |t| < 2^-27 the cubic term is below half an ulp of the result.
fn lgamma_near_zero(x: f64) -> Option<f64> {
    const LIMIT: f64 = 7.450580596923828e-9;
    let t1 = x - 1.0;
    if fabs(t1) < LIMIT {
        return Some(EULER.neg().mul_f(t1).add(PI_SQ_12.mul_f(t1).mul_f(t1)).to_f64());
    }
    let t2 = x - 2.0;
    if fabs(t2) < LIMIT {
        let c1 = EULER.neg().add_f(1.0);
        let c2 = PI_SQ_12.add_f(-0.5);
        return Some(c1.mul_f(t2).add(c2.mul_f(t2).mul_f(t2)).to_f64());
    }
    None
}

fn is_integer(x: f64) -> bool {
    trunc(x) == x
}

/// ln |Gamma x| and the sign of Gamma x, for finite x that is not a pole.
fn lgamma_signed(x: f64) -> (DD, i32) {
    if x > 0.0 {
        return (lgamma_pos(DD::from(x)), 1);
    }
    // Gamma(x) Gamma(1 - x) = pi / sin(pi x).
    let s = sinpi_dd(x);
    let sign = if s.hi < 0.0 { -1 } else { 1 };
    let s = if s.hi < 0.0 { s.neg() } else { s };
    let v = LN_PI.sub(log_dd(s)).sub(lgamma_pos(two_sum(1.0, -x)));
    (v, sign)
}

/// lgamma's sign of Gamma(x), set by lgamma (XSI).
#[no_mangle]
pub static mut signgam: i32 = 0;

#[no_mangle]
pub unsafe extern "C" fn lgamma_r(x: f64, sign: *mut i32) -> f64 {
    *sign = 1;
    if x.is_nan() {
        return x + x;
    }
    if x.is_infinite() {
        return fabs(x);
    }
    if x <= 0.0 && is_integer(x) {
        if x.is_sign_negative() {
            *sign = -1;
        }
        return pole(false);
    }
    if x == 1.0 || x == 2.0 {
        return 0.0;
    }
    if fabs(x) > 2.5e305 {
        return overflow(false);
    }
    if let Some(v) = lgamma_near_zero(x) {
        return v;
    }
    let (v, s) = lgamma_signed(x);
    *sign = s;
    v.to_f64()
}

#[no_mangle]
pub unsafe extern "C" fn lgamma(x: f64) -> f64 {
    let mut s = 1;
    let v = lgamma_r(x, &mut s);
    signgam = s;
    v
}

#[no_mangle]
pub extern "C" fn tgamma(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x == 0.0 {
        return pole(x.is_sign_negative());
    }
    if x.is_infinite() {
        return if x > 0.0 { x } else { domain() };
    }
    if x < 0.0 && is_integer(x) {
        return domain();
    }
    if x > 171.7 {
        return overflow(false);
    }
    if x < -185.0 {
        // |Gamma x| < 2^-1075; its sign alternates between the poles.
        let neg = (trunc(x) as i64) & 1 == 0;
        return underflow(neg);
    }
    if is_integer(x) && x <= 30.0 {
        // (x - 1)! rounded once.
        let mut p = DD::from(1.0);
        let mut k = 2.0;
        while k < x {
            p = p.mul_f(k);
            k += 1.0;
        }
        return p.to_f64();
    }
    let (l, sign) = lgamma_signed(x);
    let (m, k) = exp_dd(l);
    let v = scale(m.to_f64(), k as i64);
    if sign < 0 { -v } else { v }
}
