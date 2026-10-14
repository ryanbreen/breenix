//! Trigonometric functions (ISO C17 7.12.4, F.10.1).
//!
//! Arguments beyond pi/4 are reduced exactly: x * 2/pi is formed as an integer product of
//! x's 53-bit significand with the 256 bits of 2/pi that can affect the quadrant and the
//! fraction (Payne and Hanek, "Radian reduction for trigonometric functions", SIGNUM
//! Newsletter 18, 1983). The fraction, scaled by pi/2, is the reduced argument in
//! double-double, and the kernels sum Taylor series there.

use super::basic::{copysign, decompose, fabs};
use super::consts::{INV_FACT, INV_ODD, PI, PI_2, PI_4, TWO_OVER_PI};
use super::dd::{two_sum, DD};
use super::{domain, quiet};

/// 256 bits of 2/pi starting at bit index `s` (1-based weight 2^-s), most significant
/// word first.
fn window(s: usize) -> [u64; 4] {
    let mut out = [0u64; 4];
    for (j, w) in out.iter_mut().enumerate() {
        let o = s - 1 + 64 * j;
        let (i, sh) = (o / 64, o % 64);
        let hi = TWO_OVER_PI[i] << sh;
        let lo = if sh == 0 { 0 } else { TWO_OVER_PI[i + 1] >> (64 - sh) };
        *w = hi | lo;
    }
    out
}

/// Bits [lo, lo + n) of a 320-bit number held most significant word first, n <= 128.
fn bits(p: &[u64; 5], lo: usize, n: usize) -> u128 {
    let mut v = 0u128;
    for k in (0..n).rev() {
        let b = lo + k;
        let word = p[4 - b / 64];
        v = (v << 1) | ((word >> (b % 64)) & 1) as u128;
    }
    v
}

/// For finite |x| > pi/4: x = (4j + n) pi/2 + r with |r| <= pi/4, as (n, r).
fn reduce_big(x: f64) -> (u32, DD) {
    let (m, q) = decompose(x);
    // Bits of weight 2^-i with i < q - 1 only add multiples of 4 to x * 2/pi.
    let s = (q - 1).max(1) as usize;
    let b = window(s);
    // p = m * b, a 320-bit product.
    let mut p = [0u64; 5];
    let mut carry = 0u128;
    for j in (0..4).rev() {
        let v = m as u128 * b[j] as u128 + carry;
        p[j + 1] = v as u64;
        carry = v >> 64;
    }
    p[0] = carry as u64;
    // x * 2/pi = p * 2^(q - s - 255): the binary point sits above bit L of p.
    let l = (s as i64 + 255 - q as i64) as usize;
    let mut n = bits(&p, l, 2) as u32;
    let f = bits(&p, l - 128, 128);
    let (neg, mag) = if f >> 127 != 0 {
        n = (n + 1) & 3;
        (true, f.wrapping_neg())
    } else {
        (false, f)
    };
    let hi = mag as f64;
    let rest = mag as i128 - hi as u128 as i128;
    let frac = two_sum(hi, rest as f64).scale(-128);
    let r = frac.mul(PI_2);
    (n, if neg { r.neg() } else { r })
}

fn reduce(a: f64) -> (u32, DD) {
    if a <= PI_4.hi {
        (0, DD::from(a))
    } else {
        reduce_big(a)
    }
}

/// sin r for |r| <= pi/4.
fn sin_k(r: DD) -> DD {
    let s = r.sqr();
    let mut sum = DD::from(0.0);
    for k in (0..14).rev() {
        let c = INV_FACT[2 * k + 1];
        sum = sum.mul(s).add(if k & 1 == 1 { c.neg() } else { c });
    }
    sum.mul(r)
}

/// cos r for |r| <= pi/4.
fn cos_k(r: DD) -> DD {
    let s = r.sqr();
    let mut sum = DD::from(0.0);
    for k in (0..14).rev() {
        let c = INV_FACT[2 * k];
        sum = sum.mul(s).add(if k & 1 == 1 { c.neg() } else { c });
    }
    sum
}

#[no_mangle]
pub extern "C" fn sin(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x.is_infinite() {
        return domain();
    }
    if x == 0.0 {
        return x;
    }
    let (n, r) = reduce(fabs(x));
    let v = match n {
        0 => sin_k(r),
        1 => cos_k(r),
        2 => sin_k(r).neg(),
        _ => cos_k(r).neg(),
    };
    copysign(1.0, x) * v.to_f64()
}

#[no_mangle]
pub extern "C" fn cos(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x.is_infinite() {
        return domain();
    }
    let (n, r) = reduce(fabs(x));
    let v = match n {
        0 => cos_k(r),
        1 => sin_k(r).neg(),
        2 => cos_k(r).neg(),
        _ => sin_k(r),
    };
    v.to_f64()
}

#[no_mangle]
pub extern "C" fn tan(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x.is_infinite() {
        return domain();
    }
    if x == 0.0 {
        return x;
    }
    let (n, r) = reduce(fabs(x));
    let (s, c) = (sin_k(r), cos_k(r));
    let v = if n & 1 == 0 { s.div(c) } else { c.div(s).neg() };
    copysign(1.0, x) * v.to_f64()
}

/// atan t for t >= 0 (finite).
pub(crate) fn atan_dd(t: DD) -> DD {
    let (mut a, inv) = if t.hi > 1.0 { (t.recip(), true) } else { (t, false) };
    // atan a = 2 atan(a / (1 + sqrt(1 + a^2))), three times: |a| <= tan(pi/32).
    for _ in 0..3 {
        a = a.div(a.sqr().add_f(1.0).sqrt().add_f(1.0));
    }
    let s = a.sqr();
    let mut sum = DD::from(0.0);
    for k in (0..18).rev() {
        let c = INV_ODD[k];
        sum = sum.mul(s).add(if k & 1 == 1 { c.neg() } else { c });
    }
    let v = sum.mul(a).mul_f(8.0);
    if inv { PI_2.sub(v) } else { v }
}

#[no_mangle]
pub extern "C" fn atan(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        return quiet(x);
    }
    let a = fabs(x);
    let v = if a.is_infinite() { PI_2.to_f64() } else { atan_dd(DD::from(a)).to_f64() };
    copysign(v, x)
}

/// atan(|y| / |x|) for finite nonzero x and y, without forming an overflowing ratio.
fn atan_ratio(ay: f64, ax: f64) -> DD {
    let r = ay / ax;
    if r > 1.152921504606847e18 {
        PI_2.sub(DD::from(ax).div_f(ay))
    } else if r < 8.673617379884035e-19 {
        DD::from(ay).div_f(ax)
    } else {
        atan_dd(DD::from(ay).div_f(ax))
    }
}

/// atan2, with the special cases of C17 F.10.1.4.
#[no_mangle]
pub extern "C" fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    let left = x < 0.0 || (x == 0.0 && x.is_sign_negative());
    let v = if y == 0.0 {
        if left { PI.to_f64() } else { 0.0 }
    } else if x == 0.0 {
        PI_2.to_f64()
    } else if y.is_infinite() {
        if x.is_infinite() {
            if left { PI_4.mul_f(3.0).to_f64() } else { PI_4.to_f64() }
        } else {
            PI_2.to_f64()
        }
    } else if x.is_infinite() {
        if left { PI.to_f64() } else { 0.0 }
    } else {
        let a = atan_ratio(fabs(y), fabs(x));
        if left { PI.sub(a).to_f64() } else { a.to_f64() }
    };
    copysign(v, y)
}

/// sqrt((1 - a)(1 + a)) for 0 <= a < 1.
fn cos_of_asin(a: f64) -> DD {
    two_sum(1.0, -a).mul(two_sum(1.0, a)).sqrt()
}

#[no_mangle]
pub extern "C" fn asin(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    let a = fabs(x);
    if a > 1.0 {
        return domain();
    }
    if x == 0.0 {
        return x;
    }
    if a == 1.0 {
        return copysign(PI_2.to_f64(), x);
    }
    copysign(atan_dd(DD::from(a).div(cos_of_asin(a))).to_f64(), x)
}

#[no_mangle]
pub extern "C" fn acos(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    let a = fabs(x);
    if a > 1.0 {
        return domain();
    }
    if x == 1.0 {
        return 0.0;
    }
    if x == -1.0 {
        return PI.to_f64();
    }
    if x == 0.0 {
        return PI_2.to_f64();
    }
    let t = atan_dd(cos_of_asin(a).div_f(a));
    if x > 0.0 { t.to_f64() } else { PI.sub(t).to_f64() }
}

/// sin(pi x) for finite x, in double-double; exact zeros at integers.
pub(crate) fn sinpi_dd(x: f64) -> DD {
    // x - 2 round(x / 2) is exact and lies in [-1, 1].
    let r = x - 2.0 * super::basic::round(x * 0.5);
    let (neg, a) = (r < 0.0, fabs(r));
    // sin(pi a) = sin(pi (1 - a)); fold to [0, 1/2], then to [0, 1/4] via cosine.
    let a = if a > 0.5 { 1.0 - a } else { a };
    let v = if a <= 0.25 {
        sin_k(PI.mul_f(a))
    } else {
        cos_k(PI.mul_f(0.5 - a))
    };
    if neg { v.neg() } else { v }
}

