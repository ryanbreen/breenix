//! The math.h functions whose results are exact or depend only on the rounding direction
//! (ISO C17 7.12.6 to 7.12.13 and Annex F): sqrt, fabs, copysign, frexp, ldexp, scalbn,
//! logb, ilogb, modf, the rounding functions, fmod, remainder, remquo, fma, fmax, fmin,
//! fdim, nextafter and nan.
//!
//! Operations whose result depends on the dynamic rounding direction use the hardware's
//! own instructions where the target has them (frintx, frinti, fcvtzs, fsqrt, fmadd on
//! aarch64; sqrtsd, cvtsd2si on x86-64 with SSE2). Results that need a rounding no
//! instruction provides go through stdlib::floatconv, which rounds exactly once.

use super::{domain, fenv, pole, report};
#[cfg(target_arch = "x86_64")]
use crate::stdlib::floatconv::Big;
use crate::stdlib::floatconv::{self, Format, F32, F64};

const SIGN: u64 = 1 << 63;
const FRAC: u64 = (1 << 52) - 1;

pub(crate) fn pow2(n: i32) -> f64 {
    if n < -1074 { return 0.0; }
    if n < -1022 { return f64::from_bits(1u64 << (n + 1074)); }
    if n > 1023 { return f64::INFINITY; }
    f64::from_bits(((n + 1023) as u64) << 52)
}

/// For finite nonzero x: |x| = m * 2^e with m in [2^52, 2^53).
pub(crate) fn decompose(x: f64) -> (u64, i32) {
    let b = x.to_bits() & !SIGN;
    let be = (b >> 52) as i32;
    if be == 0 {
        let shift = b.leading_zeros() as i32 - 11;
        (b << shift, -1074 - shift)
    } else {
        ((b & FRAC) | (1 << 52), be - 1075)
    }
}

/// m * 2^e (m < 2^64) rounded to `f` in the current direction, with errno and flags.
pub(crate) fn round_binary(neg: bool, m: u128, e: i64, f: &Format) -> u128 {
    let c = floatconv::from_binary(neg, m, e, false, f, floatconv::current_round());
    report(&c);
    c.bits
}

// ----------------------------------------------------------------------------
// Square root
// ----------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
pub(crate) fn sqrt_exact(x: f64) -> f64 {
    let r: f64;
    unsafe { core::arch::asm!("fsqrt {0:d}, {1:d}", out(vreg) r, in(vreg) x, options(pure, nomem, nostack)) };
    r
}

#[cfg(target_arch = "aarch64")]
fn sqrtf_exact(x: f32) -> f32 {
    let r: f32;
    unsafe { core::arch::asm!("fsqrt {0:s}, {1:s}", out(vreg) r, in(vreg) x, options(pure, nomem, nostack)) };
    r
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
pub(crate) fn sqrt_exact(x: f64) -> f64 {
    let r: f64;
    unsafe { core::arch::asm!("sqrtsd {0}, {1}", out(xmm_reg) r, in(xmm_reg) x, options(pure, nomem, nostack)) };
    r
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
fn sqrtf_exact(x: f32) -> f32 {
    let r: f32;
    unsafe { core::arch::asm!("sqrtss {0}, {1}", out(xmm_reg) r, in(xmm_reg) x, options(pure, nomem, nostack)) };
    r
}

/// The soft-float build: an integer square root rounded once by floatconv.
#[cfg(all(target_arch = "x86_64", not(target_feature = "sse2")))]
pub(crate) fn sqrt_exact(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() || x.is_infinite() {
        return x;
    }
    let (m, e) = decompose(x);
    // Make the exponent even and the radicand about 110 bits, so the root has 55.
    let shift = if e & 1 == 0 { 56 } else { 57 };
    let rad = (m as u128) << shift;
    let mut root = 0u128;
    let mut bit = 1u128 << 126;
    let mut rem = rad;
    while bit > rad {
        bit >>= 2;
    }
    while bit != 0 {
        if rem >= root + bit {
            rem -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    let c = floatconv::from_binary(false, root, (e as i64 - shift) / 2, rem != 0, &F64, floatconv::Round::Nearest);
    f64::from_bits(c.bits as u64)
}

#[cfg(all(target_arch = "x86_64", not(target_feature = "sse2")))]
fn sqrtf_exact(x: f32) -> f32 {
    // A float's square root rounded to double then to float is correctly rounded: the
    // double has more than 2 * 24 + 2 bits.
    sqrt_exact(x as f64) as f32
}

#[no_mangle]
pub extern "C" fn sqrt(x: f64) -> f64 {
    if x < 0.0 {
        return domain();
    }
    sqrt_exact(x)
}

#[no_mangle]
pub extern "C" fn sqrtf(x: f32) -> f32 {
    if x < 0.0 {
        return domain() as f32;
    }
    sqrtf_exact(x)
}

// ----------------------------------------------------------------------------
// Sign and exponent manipulation
// ----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn fabs(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & !SIGN)
}

#[no_mangle]
pub extern "C" fn fabsf(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & 0x7fff_ffff)
}

#[no_mangle]
pub extern "C" fn copysign(x: f64, y: f64) -> f64 {
    f64::from_bits((x.to_bits() & !SIGN) | (y.to_bits() & SIGN))
}

#[no_mangle]
pub extern "C" fn copysignf(x: f32, y: f32) -> f32 {
    f32::from_bits((x.to_bits() & 0x7fff_ffff) | (y.to_bits() & 0x8000_0000))
}

#[no_mangle]
pub unsafe extern "C" fn frexp(x: f64, exp: *mut i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        *exp = 0;
        return x + x;
    }
    let (m, e) = decompose(x);
    *exp = e + 53;
    f64::from_bits((x.to_bits() & SIGN) | (1022 << 52) | (m & FRAC))
}

#[no_mangle]
pub unsafe extern "C" fn frexpf(x: f32, exp: *mut i32) -> f32 {
    frexp(x as f64, exp) as f32
}

/// x * 2^n, exact unless the result leaves the normal range.
pub(crate) fn scale(x: f64, n: i64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x + x;
    }
    let (m, e) = decompose(x);
    let n = n.clamp(-5000, 5000);
    let top = e as i64 + 52 + n;
    if (-1022..=1023).contains(&top) {
        return f64::from_bits((x.to_bits() & SIGN) | (((top + 1023) as u64) << 52) | (m & FRAC));
    }
    f64::from_bits(round_binary(x < 0.0, m as u128, e as i64 + n, &F64) as u64)
}

pub(crate) fn scalef(x: f32, n: i64) -> f32 {
    if x == 0.0 || !x.is_finite() {
        return x + x;
    }
    let (m, e) = decompose(x as f64);
    let n = n.clamp(-5000, 5000);
    let top = e as i64 + 52 + n;
    if (-126..=127).contains(&top) {
        return scale(x as f64, n) as f32;
    }
    f32::from_bits(round_binary(x < 0.0, m as u128, e as i64 + n, &F32) as u32)
}

#[no_mangle]
pub extern "C" fn ldexp(x: f64, n: i32) -> f64 {
    scale(x, n as i64)
}

#[no_mangle]
pub extern "C" fn scalbn(x: f64, n: i32) -> f64 {
    scale(x, n as i64)
}

#[no_mangle]
pub extern "C" fn scalbln(x: f64, n: i64) -> f64 {
    scale(x, n)
}

#[no_mangle]
pub extern "C" fn ldexpf(x: f32, n: i32) -> f32 {
    scalef(x, n as i64)
}

#[no_mangle]
pub extern "C" fn scalbnf(x: f32, n: i32) -> f32 {
    scalef(x, n as i64)
}

#[no_mangle]
pub extern "C" fn scalblnf(x: f32, n: i64) -> f32 {
    scalef(x, n)
}

#[no_mangle]
pub extern "C" fn logb(x: f64) -> f64 {
    if x.is_nan() {
        return x + x;
    }
    if x.is_infinite() {
        return fabs(x);
    }
    if x == 0.0 {
        return pole(true);
    }
    (decompose(x).1 + 52) as f64
}

#[no_mangle]
pub extern "C" fn logbf(x: f32) -> f32 {
    logb(x as f64) as f32
}

#[cfg(target_arch = "aarch64")]
pub(crate) const FP_ILOGB0: i32 = -i32::MAX;
#[cfg(target_arch = "aarch64")]
pub(crate) const FP_ILOGBNAN: i32 = i32::MAX;
#[cfg(target_arch = "x86_64")]
pub(crate) const FP_ILOGB0: i32 = i32::MIN;
#[cfg(target_arch = "x86_64")]
pub(crate) const FP_ILOGBNAN: i32 = i32::MIN;

/// ilogb of 0, an infinity or a NaN is a domain error (C17 7.12.6.5, F.10.3.5).
#[no_mangle]
pub extern "C" fn ilogb(x: f64) -> i32 {
    let r = if x.is_nan() {
        FP_ILOGBNAN
    } else if x.is_infinite() {
        i32::MAX
    } else if x == 0.0 {
        FP_ILOGB0
    } else {
        return decompose(x).1 + 52;
    };
    domain();
    r
}

#[no_mangle]
pub extern "C" fn ilogbf(x: f32) -> i32 {
    ilogb(x as f64)
}

#[no_mangle]
pub unsafe extern "C" fn modf(x: f64, ip: *mut f64) -> f64 {
    if x.is_nan() {
        *ip = x;
        return x + x;
    }
    if x.is_infinite() {
        *ip = x;
        return copysign(0.0, x);
    }
    let t = trunc(x);
    *ip = t;
    copysign(x - t, x)
}

#[no_mangle]
pub unsafe extern "C" fn modff(x: f32, ip: *mut f32) -> f32 {
    let mut i = 0.0f64;
    let r = modf(x as f64, &mut i);
    *ip = i as f32;
    r as f32
}

// ----------------------------------------------------------------------------
// Rounding to integers
// ----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn trunc(x: f64) -> f64 {
    let b = x.to_bits();
    let e = ((b >> 52) & 0x7ff) as i32 - 1023;
    if e >= 52 {
        return super::quiet(x); // integral already, or Inf or NaN
    }
    if e < 0 {
        return f64::from_bits(b & SIGN);
    }
    f64::from_bits(b & !(FRAC >> e))
}

#[no_mangle]
pub extern "C" fn floor(x: f64) -> f64 {
    let t = trunc(x);
    if x < 0.0 && t != x { t - 1.0 } else { t }
}

#[no_mangle]
pub extern "C" fn ceil(x: f64) -> f64 {
    let t = trunc(x);
    if x > 0.0 && t != x { t + 1.0 } else { t }
}

/// Halfway cases away from zero.
#[no_mangle]
pub extern "C" fn round(x: f64) -> f64 {
    if !x.is_finite() { return super::quiet(x); }
    let t = trunc(x);
    if fabs(x - t) >= 0.5 { t + copysign(1.0, x) } else { t }
}

#[no_mangle]
pub extern "C" fn truncf(x: f32) -> f32 {
    trunc(x as f64) as f32
}

#[no_mangle]
pub extern "C" fn floorf(x: f32) -> f32 {
    floor(x as f64) as f32
}

#[no_mangle]
pub extern "C" fn ceilf(x: f32) -> f32 {
    ceil(x as f64) as f32
}

#[no_mangle]
pub extern "C" fn roundf(x: f32) -> f32 {
    round(x as f64) as f32
}

#[cfg(target_arch = "aarch64")]
mod round_dyn {
    pub(super) fn rint(x: f64) -> f64 {
        let r: f64;
        unsafe { core::arch::asm!("frintx {0:d}, {1:d}", out(vreg) r, in(vreg) x, options(nomem, nostack)) };
        r
    }
    pub(super) fn nearbyint(x: f64) -> f64 {
        let r: f64;
        unsafe { core::arch::asm!("frinti {0:d}, {1:d}", out(vreg) r, in(vreg) x, options(nomem, nostack)) };
        r
    }
    pub(super) fn rintf(x: f32) -> f32 {
        let r: f32;
        unsafe { core::arch::asm!("frintx {0:s}, {1:s}", out(vreg) r, in(vreg) x, options(nomem, nostack)) };
        r
    }
    pub(super) fn nearbyintf(x: f32) -> f32 {
        let r: f32;
        unsafe { core::arch::asm!("frinti {0:s}, {1:s}", out(vreg) r, in(vreg) x, options(nomem, nostack)) };
        r
    }

}

#[cfg(target_arch = "x86_64")]
mod round_dyn {
    use super::super::fenv;

    /// Adding and removing 2^52 rounds a smaller magnitude to an integer in the current
    /// direction (raising inexact when it changes the value).
    pub(super) fn rint(x: f64) -> f64 {
        if !(super::fabs(x) < 4503599627370496.0) {
            return super::super::quiet(x);
        }
        let c = super::copysign(4503599627370496.0, x);
        let r = core::hint::black_box(x + c) - c;
        super::copysign(r, x)
    }
    pub(super) fn nearbyint(x: f64) -> f64 {
        let before = fenv::flags() & fenv::FE_INEXACT;
        let r = rint(x);
        fenv::set_flags(fenv::FE_INEXACT, before);
        r
    }
    pub(super) fn rintf(x: f32) -> f32 {
        if !(super::fabsf(x) < 8388608.0) {
            return if x.is_nan() { x + x } else { x };
        }
        let c = super::copysignf(8388608.0, x);
        let r = core::hint::black_box(x + c) - c;
        super::copysignf(r, x)
    }
    pub(super) fn nearbyintf(x: f32) -> f32 {
        let before = fenv::flags() & fenv::FE_INEXACT;
        let r = rintf(x);
        fenv::set_flags(fenv::FE_INEXACT, before);
        r
    }

}

#[no_mangle]
pub extern "C" fn rint(x: f64) -> f64 {
    round_dyn::rint(x)
}

#[no_mangle]
pub extern "C" fn nearbyint(x: f64) -> f64 {
    round_dyn::nearbyint(x)
}

#[no_mangle]
pub extern "C" fn rintf(x: f32) -> f32 {
    round_dyn::rintf(x)
}

#[no_mangle]
pub extern "C" fn nearbyintf(x: f32) -> f32 {
    round_dyn::nearbyintf(x)
}

// C17 7.12.9: an invalid integer conversion is a domain error. Test the
// rounded floating result before casting; i64::MAX rounds to 2^63 in binary64.
fn integer_result(r: f64) -> i64 {
    if r >= -9223372036854775808.0 && r < 9223372036854775808.0 { r as i64 }
    else {
        crate::pthread::set_errno(crate::stdlib::EDOM);
        fenv::raise(fenv::FE_INVALID);
        i64::MIN
    }
}
#[no_mangle]
pub extern "C" fn lrint(x: f64) -> i64 { integer_result(round_dyn::rint(x)) }
#[no_mangle]
pub extern "C" fn llrint(x: f64) -> i64 { lrint(x) }
#[no_mangle]
pub extern "C" fn lround(x: f64) -> i64 { integer_result(round(x)) }
#[no_mangle]
pub extern "C" fn llround(x: f64) -> i64 { lround(x) }
#[no_mangle]
pub extern "C" fn lrintf(x: f32) -> i64 { integer_result(round_dyn::rintf(x) as f64) }
#[no_mangle]
pub extern "C" fn llrintf(x: f32) -> i64 { lrintf(x) }
#[no_mangle]
pub extern "C" fn lroundf(x: f32) -> i64 { lround(x as f64) }
#[no_mangle]
pub extern "C" fn llroundf(x: f32) -> i64 { lroundf(x) }

// ----------------------------------------------------------------------------
// Remainders
// ----------------------------------------------------------------------------

/// For finite x and nonzero finite y: |x| mod |y| (exact, non-negative) and the low 64
/// bits of the integer quotient floor(|x| / |y|).
fn rem_core(x: f64, y: f64) -> (f64, u64) {
    let (mx, ex) = decompose(x);
    let (my, ey) = decompose(y);
    if ex < ey || (ex == ey && mx < my) {
        return (fabs(x), 0);
    }
    let mut q = mx / my;
    let mut r = mx % my;
    let mut d = (ex - ey) as u32;
    while d > 0 {
        let s = d.min(63);
        let t = (r as u128) << s;
        q = q.wrapping_shl(s).wrapping_add((t / my as u128) as u64);
        r = (t % my as u128) as u64;
        d -= s;
    }
    // r * 2^ey is below |y|, so it is representable; scale cannot round.
    (scale(r as f64, ey as i64), q)
}

#[no_mangle]
pub extern "C" fn fmod(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x.is_infinite() || y == 0.0 {
        return domain();
    }
    if y.is_infinite() || x == 0.0 {
        return x;
    }
    copysign(rem_core(x, y).0, x)
}

#[no_mangle]
pub extern "C" fn fmodf(x: f32, y: f32) -> f32 {
    // The remainder of two floats is a float, so computing it in double is exact.
    fmod(x as f64, y as f64) as f32
}

/// The IEEE remainder x - n y, n the integer nearest x / y (ties to even), and n's low bits.
fn remainder_core(x: f64, y: f64) -> (f64, i32) {
    let (mut r, mut q) = rem_core(x, y);
    let ay = fabs(y);
    let beyond_half = if ay >= pow2(-1021) {
        let h = ay * 0.5;
        r > h || (r == h && q & 1 == 1)
    } else {
        let r2 = r * 2.0;
        r2 > ay || (r2 == ay && q & 1 == 1)
    };
    if beyond_half {
        r -= ay;
        q = q.wrapping_add(1);
    }
    let r = if r == 0.0 { copysign(0.0, x) } else if x < 0.0 { -r } else { r };
    let q = (q & 0x7fff_ffff) as i32;
    (r, if (x < 0.0) != (y < 0.0) { -q } else { q })
}

#[no_mangle]
pub extern "C" fn remainder(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x.is_infinite() || y == 0.0 {
        return domain();
    }
    if y.is_infinite() {
        return x;
    }
    remainder_core(x, y).0
}

#[no_mangle]
pub unsafe extern "C" fn remquo(x: f64, y: f64, quo: *mut i32) -> f64 {
    *quo = 0;
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x.is_infinite() || y == 0.0 {
        return domain();
    }
    if y.is_infinite() {
        return x;
    }
    let (r, q) = remainder_core(x, y);
    *quo = q;
    r
}

#[no_mangle]
pub extern "C" fn remainderf(x: f32, y: f32) -> f32 {
    remainder(x as f64, y as f64) as f32
}

#[no_mangle]
pub unsafe extern "C" fn remquof(x: f32, y: f32, quo: *mut i32) -> f32 {
    remquo(x as f64, y as f64, quo) as f32
}

// ----------------------------------------------------------------------------
// Fused multiply-add
// ----------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
/// x = (-1)^neg * m * 2^e for finite nonzero x of either width.
fn parts(x: f64) -> (bool, u64, i64) {
    let (m, e) = decompose(x);
    (x < 0.0, m, e as i64)
}

#[cfg(target_arch = "x86_64")]
/// a * b + c rounded once into `f`, for finite nonzero a, b and finite c, all values of
/// format `f` passed as doubles.
pub(crate) fn fma_exact(a: f64, b: f64, c: f64, f: &Format) -> u128 {
    let (na, ma, ea) = parts(a);
    let (nb, mb, eb) = parts(b);
    let np = na != nb;
    let ep = ea + eb;
    let mut prod = Big::from_u128(ma as u128 * mb as u128);
    if c == 0.0 {
        return round_big(np, &prod, ep, f);
    }
    let (nc, mc, ec) = parts(c);
    let mut add = Big::from_u64(mc);
    let e = ep.min(ec);
    prod.shl((ep - e) as usize);
    add.shl((ec - e) as usize);
    if np == nc {
        prod.add(&add);
        return round_big(np, &prod, e, f);
    }
    match prod.cmp(&add) {
        core::cmp::Ordering::Greater => {
            prod.sub(&add);
            round_big(np, &prod, e, f)
        }
        core::cmp::Ordering::Less => {
            add.sub(&prod);
            round_big(nc, &add, e, f)
        }
        // An exact zero sum is +0, or -0 when rounding downward (IEEE 754 6.3).
        core::cmp::Ordering::Equal => {
            let neg = fenv::get_round() == fenv::FE_DOWNWARD;
            if neg { f.zero(true) } else { f.zero(false) }
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn round_big(neg: bool, m: &Big, e: i64, f: &Format) -> u128 {
    let c = floatconv::convert(neg, m, &Big::from_u64(1), e, false, f, floatconv::current_round());
    report(&c);
    c.bits
}

#[cfg(target_arch = "x86_64")]
/// The cases fma_exact does not take: NaN, infinite or zero operands, where a * b + c
/// already rounds once (a zero product is exact; an infinity absorbs).
fn fma_special(a: f64, b: f64, c: f64) -> Option<f64> {
    // C17 F.10.9.2: classify the exact product, never a separately rounded
    // finite product (DBL_MAX*2 + -INF is -INF, with no invalid exception).
    let signaling = |x: f64| x.is_nan() && x.to_bits() & (1u64 << 51) == 0;
    if signaling(a) || signaling(b) || signaling(c) { return Some(domain()); }
    let invalid_product = (a == 0.0 && b.is_infinite()) || (b == 0.0 && a.is_infinite());
    let infinite_product = (a.is_infinite() || b.is_infinite()) && !a.is_nan() && !b.is_nan();
    let opposite = (a.is_sign_negative() != b.is_sign_negative()) != c.is_sign_negative();
    if invalid_product || (infinite_product && c.is_infinite() && opposite) {
        return Some(domain());
    }
    if a.is_nan() || b.is_nan() || c.is_nan() { return Some(a + b + c); }
    if c.is_infinite() { return Some(c); }
    if infinite_product { return Some(copysign(f64::INFINITY, if a.is_sign_negative() != b.is_sign_negative() { -1.0 } else { 1.0 })); }
    if a == 0.0 || b == 0.0 { return Some(a * b + c); }
    None
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub extern "C" fn fma(a: f64, b: f64, c: f64) -> f64 {
    let before = fenv::flags();
    fenv::set_flags(fenv::FE_ALL_EXCEPT, 0);
    let r: f64;
    unsafe {
        core::arch::asm!("fmadd {0:d}, {1:d}, {2:d}, {3:d}", out(vreg) r, in(vreg) a, in(vreg) b, in(vreg) c,
            options(nomem, nostack))
    };
    let raised = fenv::flags();
    fenv::set_flags(fenv::FE_ALL_EXCEPT, before | raised);
    if raised & fenv::FE_INVALID != 0 { crate::pthread::set_errno(crate::stdlib::EDOM); }
    else if raised & (fenv::FE_OVERFLOW | fenv::FE_UNDERFLOW) != 0 {
        crate::pthread::set_errno(crate::stdlib::ERANGE);
    }
    r
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub extern "C" fn fmaf(a: f32, b: f32, c: f32) -> f32 {
    let before = fenv::flags();
    fenv::set_flags(fenv::FE_ALL_EXCEPT, 0);
    let r: f32;
    unsafe {
        core::arch::asm!("fmadd {0:s}, {1:s}, {2:s}, {3:s}", out(vreg) r, in(vreg) a, in(vreg) b, in(vreg) c,
            options(nomem, nostack))
    };
    let raised = fenv::flags();
    fenv::set_flags(fenv::FE_ALL_EXCEPT, before | raised);
    if raised & fenv::FE_INVALID != 0 { crate::pthread::set_errno(crate::stdlib::EDOM); }
    else if raised & (fenv::FE_OVERFLOW | fenv::FE_UNDERFLOW) != 0 {
        crate::pthread::set_errno(crate::stdlib::ERANGE);
    }
    r
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn fma(a: f64, b: f64, c: f64) -> f64 {
    fma_special(a, b, c).unwrap_or_else(|| f64::from_bits(fma_exact(a, b, c, &F64) as u64))
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub extern "C" fn fmaf(a: f32, b: f32, c: f32) -> f32 {
    let signaling = |x: f32| x.is_nan() && x.to_bits() & (1u32 << 22) == 0;
    if signaling(a) || signaling(b) || signaling(c) { return domain() as f32; }
    let (a, b, c) = (a as f64, b as f64, c as f64);
    match fma_special(a, b, c) {
        Some(r) => r as f32,
        None => f32::from_bits(fma_exact(a, b, c, &F32) as u32),
    }
}

// ----------------------------------------------------------------------------
// Maximum, minimum, positive difference, neighbours, NaN
// ----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn fmax(x: f64, y: f64) -> f64 {
    if x.is_nan() {
        return y;
    }
    if y.is_nan() || x > y || (x == y && y.is_sign_negative()) {
        return x;
    }
    y
}

#[no_mangle]
pub extern "C" fn fmin(x: f64, y: f64) -> f64 {
    if x.is_nan() {
        return y;
    }
    if y.is_nan() || x < y || (x == y && x.is_sign_negative()) {
        return x;
    }
    y
}

#[no_mangle]
pub extern "C" fn fdim(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x > y {
        let before = fenv::flags();
        fenv::set_flags(fenv::FE_OVERFLOW, 0);
        let r = core::hint::black_box(x) - core::hint::black_box(y);
        let raised = fenv::flags();
        fenv::set_flags(fenv::FE_OVERFLOW, (before | raised) & fenv::FE_OVERFLOW);
        if x.is_finite() && y.is_finite() && raised & fenv::FE_OVERFLOW != 0 {
            crate::pthread::set_errno(crate::stdlib::ERANGE);
        }
        r
    } else {
        0.0
    }
}

#[no_mangle]
pub extern "C" fn fmaxf(x: f32, y: f32) -> f32 {
    fmax(x as f64, y as f64) as f32
}

#[no_mangle]
pub extern "C" fn fminf(x: f32, y: f32) -> f32 {
    fmin(x as f64, y as f64) as f32
}

#[no_mangle]
pub extern "C" fn fdimf(x: f32, y: f32) -> f32 {
    super::narrow(fdim(x as f64, y as f64))
}

/// The next value after x toward y; overflow and underflow are range errors (F.10.8.3).
#[no_mangle]
pub extern "C" fn nextafter(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x == y {
        return y;
    }
    let r = if x == 0.0 {
        copysign(f64::from_bits(1), y)
    } else {
        let b = x.to_bits();
        f64::from_bits(if (y > x) == (x > 0.0) { b + 1 } else { b - 1 })
    };
    neighbour_report(r.is_infinite(), r == 0.0 || fabs(r) < f64::MIN_POSITIVE);
    r
}

#[no_mangle]
pub extern "C" fn nextafterf(x: f32, y: f32) -> f32 {
    if x.is_nan() || y.is_nan() {
        return x + y;
    }
    if x == y {
        return y;
    }
    let r = if x == 0.0 {
        copysignf(f32::from_bits(1), y)
    } else {
        let b = x.to_bits();
        f32::from_bits(if (y > x) == (x > 0.0) { b + 1 } else { b - 1 })
    };
    neighbour_report(r.is_infinite(), r == 0.0 || fabsf(r) < f32::MIN_POSITIVE);
    r
}

fn neighbour_report(overflow: bool, tiny: bool) {
    if overflow {
        crate::pthread::set_errno(crate::stdlib::ERANGE);
        fenv::raise(fenv::FE_OVERFLOW | fenv::FE_INEXACT);
    } else if tiny {
        crate::pthread::set_errno(crate::stdlib::ERANGE);
        fenv::raise(fenv::FE_UNDERFLOW | fenv::FE_INEXACT);
    }
}

/// A quiet NaN; the tag selects nothing.
#[no_mangle]
pub extern "C" fn nan(_tag: *const u8) -> f64 {
    f64::from_bits(0x7ff8_0000_0000_0000)
}

#[no_mangle]
pub extern "C" fn nanf(_tag: *const u8) -> f32 {
    f32::from_bits(0x7fc0_0000)
}
