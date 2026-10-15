//! <complex.h> (ISO C17 7.3 and Annex G) for float, double and long double, and the
//! compiler's complex multiplication and division helpers, written from the standard.
//!
//! Each function works on the encodings of its operand's parts in the type's format, so
//! one implementation serves all three types. The special values are Annex G's, folded
//! by each function's symmetries (conjugation, and oddness or evenness) onto the first
//! quadrant. Finite operands are evaluated in ext.rs's extended double-double and each
//! part is rounded once: correctly rounded or nearly so for float and double, within an
//! ulp or so for the x87 format, and a relative error near 2^-100 for binary128. The
//! inverse functions use Kahan's formulas over principal square roots, which put the
//! branch cuts where Annex G does, signed zeros included (W. Kahan, "Branch Cuts for
//! Complex Elementary Functions", in The State of the Art in Numerical Analysis, 1987).
//!
//! __mul?c3 and __div?c3 are the calls a compiler makes for z * w and z / w; they follow
//! the example algorithms of C17 G.5.1, which recover the infinities a naive formula
//! turns into NaNs.

use super::ext::{
    asinh_x, atan2_x, exp_exact, exp_x, log1p_x, log_x, overflow, pi, pi_2, quiet_nan, round_x, sign_bit,
    sincos, sinh_cosh_x, underflow, unpack, x_of, Val, X,
};
use super::fenv;
use super::long_double as ld;
use crate::stdlib::floatconv::{Format, F64};
use core::cmp::Ordering;

type Bits = u128;

/// A complex operand: each part's value and encoding.
#[derive(Clone, Copy)]
struct Z {
    re: Val,
    im: Val,
    rb: Bits,
    ib: Bits,
}

fn z(rb: Bits, ib: Bits, f: &Format) -> Z {
    Z { re: unpack(rb, f), im: unpack(ib, f), rb, ib }
}

fn is_nan(v: Val) -> bool {
    matches!(v, Val::Nan { .. })
}

fn is_inf(v: Val) -> bool {
    matches!(v, Val::Inf(_))
}

fn is_zero(v: Val) -> bool {
    matches!(v, Val::Zero(_))
}

fn is_fin(v: Val) -> bool {
    matches!(v, Val::Fin(..) | Val::Zero(_))
}

fn neg_of(v: Val, bits: Bits, f: &Format) -> bool {
    match v {
        Val::Nan { .. } => bits & sign_bit(f) != 0,
        Val::Inf(n) | Val::Zero(n) | Val::Fin(n, _, _) => n,
    }
}

fn flip(b: Bits, f: &Format) -> Bits {
    b ^ sign_bit(f)
}

fn signed(b: Bits, neg: bool, f: &Format) -> Bits {
    if neg { flip(b, f) } else { b }
}

/// The NaN result of an operation on z: z's own NaN quieted, or the default one.
fn nan(z: &Z, f: &Format) -> Bits {
    if is_nan(z.re) {
        quiet_nan(z.re, f)
    } else if is_nan(z.im) {
        quiet_nan(z.im, f)
    } else {
        f.quiet_nan(false)
    }
}

/// A NaN result where Annex G requires the invalid exception.
fn invalid_nan(f: &Format) -> Bits {
    fenv::raise(fenv::FE_INVALID);
    f.quiet_nan(false)
}

fn inf(f: &Format) -> Bits {
    f.infinity(false)
}

fn zero(f: &Format) -> Bits {
    f.zero(false)
}

fn one(f: &Format) -> Bits {
    round_x(X::ONE, false, f)
}

fn val(x: X, f: &Format) -> Bits {
    round_x(x, false, f)
}

/// |v| as a value with its sign cleared.
fn abs(v: Val) -> Val {
    match v {
        Val::Inf(_) => Val::Inf(false),
        Val::Zero(_) => Val::Zero(false),
        Val::Fin(_, s, e) => Val::Fin(false, s, e),
        n => n,
    }
}

/// The exact parts of an X: its sign, significand and exponent (dropping any bits more
/// than 74 places below the high word, as round_x would treat them as sticky).
fn parts(x: X) -> (bool, u128, i64) {
    let neg = x.is_neg();
    let m = if neg { x.m.neg() } else { x.m };
    let (mh, eh) = super::basic::decompose(m.hi);
    if m.lo == 0.0 {
        return (neg, mh as u128, eh as i64 + x.e);
    }
    let (ml, el) = super::basic::decompose(m.lo);
    let shift = (eh - el) as i64;
    if shift <= 74 {
        let hi = (mh as u128) << shift;
        (neg, if m.lo < 0.0 { hi - ml as u128 } else { hi + ml as u128 }, el as i64 + x.e)
    } else {
        (neg, mh as u128, eh as i64 + x.e)
    }
}

/// (sin y, cos y) for a finite part y.
fn sc(v: Val) -> (X, X) {
    match v {
        Val::Fin(neg, s, e) => sincos(neg, s, e),
        _ => (X::ZERO, X::ONE),
    }
}

/// (sinh a, cosh a) for a finite a >= 0 below 12000, with the large arguments reduced
/// exactly.
fn sh_ch(a: Val) -> (X, X) {
    match a {
        Val::Fin(_, s, e) if x_of(a).cmp_abs(X::from_f64(64.0)) == Ordering::Greater => {
            let h = exp_exact(false, s, e).scale(-1);
            (h, h)
        }
        _ => sinh_cosh_x(x_of(a)),
    }
}

/// The principal square root of re + i im for finite parts; `im_neg` is im's sign, which
/// a zero carries. The result's imaginary part has im's sign.
fn csqrt_x(re: X, im: X, im_neg: bool) -> (X, X) {
    let (a, b) = (re.abs(), im.abs());
    if a.is_zero() && b.is_zero() {
        return (X::ZERO, X::ZERO);
    }
    let t = a.add(a.sqr().add(b.sqr()).sqrt()).scale(-1).sqrt();
    let other = b.div(t).scale(-1);
    let (r, i) = if re.is_neg() { (other, t) } else { (t, other) };
    (r, if im_neg { i.neg() } else { i })
}

// ----------------------------------------------------------------------------
// Parts, absolute value, argument, projection
// ----------------------------------------------------------------------------

fn cabs(z: Z, f: &Format) -> Bits {
    ld::hypot(z.rb, z.ib, f)
}

fn carg(z: Z, f: &Format) -> Bits {
    ld::atan2(z.ib, z.rb, f)
}

fn cproj(z: Z, f: &Format) -> (Bits, Bits) {
    if is_inf(z.re) || is_inf(z.im) {
        (inf(f), signed(zero(f), neg_of(z.im, z.ib, f), f))
    } else {
        (z.rb, z.ib)
    }
}

fn conj(z: Z, f: &Format) -> (Bits, Bits) {
    (z.rb, flip(z.ib, f))
}

// ----------------------------------------------------------------------------
// Exponential, logarithm, power, square root
// ----------------------------------------------------------------------------

/// e^x for a finite x, with |x| above 12000 left to the caller.
fn exp_val(x: Val) -> X {
    match x {
        Val::Fin(neg, s, e) => exp_exact(neg, s, e),
        _ => X::ONE,
    }
}

fn cexp(z: Z, f: &Format) -> (Bits, Bits) {
    let (x, y) = (z.re, z.im);
    match x {
        Val::Nan { .. } => {
            if is_zero(y) { (nan(&z, f), z.ib) } else { (nan(&z, f), nan(&z, f)) }
        }
        Val::Inf(false) => match y {
            Val::Zero(_) => (inf(f), z.ib),
            Val::Fin(..) => {
                let (s, c) = sc(y);
                (signed(inf(f), c.is_neg(), f), signed(inf(f), s.is_neg(), f))
            }
            Val::Inf(_) => (inf(f), invalid_nan(f)),
            Val::Nan { .. } => (inf(f), nan(&z, f)),
        },
        Val::Inf(true) => match y {
            Val::Zero(_) | Val::Fin(..) => {
                let (s, c) = sc(y);
                (signed(zero(f), c.is_neg(), f), signed(zero(f), s.is_neg(), f))
            }
            _ => (zero(f), zero(f)),
        },
        Val::Zero(_) | Val::Fin(..) => match y {
            Val::Inf(_) => (invalid_nan(f), invalid_nan(f)),
            Val::Nan { .. } => (nan(&z, f), nan(&z, f)),
            Val::Zero(_) => (ld::exp(z.rb, f), z.ib),
            _ => {
                let (s, c) = sc(y);
                let big = x_of(x).cmp_abs(X::from_f64(12000.0)) == Ordering::Greater;
                if big && !neg_of(x, z.rb, f) {
                    (overflow(c.is_neg(), f), overflow(s.is_neg(), f))
                } else if big {
                    (underflow(c.is_neg(), f), underflow(s.is_neg(), f))
                } else {
                    let e = exp_val(x);
                    (round_x(e.mul(c), c.is_neg(), f), round_x(e.mul(s), s.is_neg(), f))
                }
            }
        },
    }
}

/// ln |z| for finite z, not zero.
fn log_abs(x: Val, y: Val) -> X {
    let (a, b) = (x_of(x).abs(), x_of(y).abs());
    let (big, small) = if a.cmp_abs(b) == Ordering::Less { (b, a) } else { (a, b) };
    let n2 = big.sqr().add(small.sqr());
    if n2.cmp(X::from_f64(0.5)) != Ordering::Less && n2.cmp(X::from_f64(2.0)) != Ordering::Greater {
        // Near the unit circle: ln|z| = log1p(a^2 + b^2 - 1) / 2, the argument exact.
        return log1p_x(ld::norm_minus_one(x, y)).scale(-1);
    }
    let r = small.div(big);
    log_x(big).add(log1p_x(r.sqr()).scale(-1))
}

fn clog(z: Z, f: &Format) -> (Bits, Bits) {
    let (x, y) = (z.re, z.im);
    let yn = neg_of(y, z.ib, f);
    if is_nan(x) || is_nan(y) {
        return if is_inf(x) || is_inf(y) { (inf(f), nan(&z, f)) } else { (nan(&z, f), nan(&z, f)) };
    }
    let xn = neg_of(x, z.rb, f);
    let angle = |v: X| signed(val(v, f), yn, f);
    if is_inf(x) || is_inf(y) {
        let a = match (is_inf(x), is_inf(y)) {
            (true, true) => pi().mul_f(if xn { 0.75 } else { 0.25 }),
            (false, true) => pi_2(),
            _ => if xn { pi() } else { X::ZERO },
        };
        return (inf(f), if a.is_zero() { signed(zero(f), yn, f) } else { angle(a) });
    }
    if is_zero(x) && is_zero(y) {
        fenv::raise(fenv::FE_DIVBYZERO);
        let a = if xn { angle(pi()) } else { signed(zero(f), yn, f) };
        return (signed(inf(f), true, f), a);
    }
    (val(log_abs(x, y), f), ld::atan2(z.ib, z.rb, f))
}

fn csqrt(z: Z, f: &Format) -> (Bits, Bits) {
    let (x, y) = (z.re, z.im);
    let yn = neg_of(y, z.ib, f);
    if is_inf(y) {
        return (inf(f), signed(inf(f), yn, f));
    }
    match x {
        Val::Nan { .. } => (nan(&z, f), nan(&z, f)),
        Val::Inf(true) => {
            if is_nan(y) { (nan(&z, f), inf(f)) } else { (zero(f), signed(inf(f), yn, f)) }
        }
        Val::Inf(false) => {
            if is_nan(y) { (inf(f), nan(&z, f)) } else { (inf(f), signed(zero(f), yn, f)) }
        }
        _ if is_nan(y) => (nan(&z, f), nan(&z, f)),
        _ => {
            if is_zero(x) && is_zero(y) {
                return (zero(f), z.ib);
            }
            let (r, i) = csqrt_x(x_of(x), x_of(y), yn);
            (round_x(r, false, f), round_x(i, yn, f))
        }
    }
}

/// z^w as e^(w ln z), for finite parts.
fn cpow_finite(zv: Z, wv: Z, f: &Format) -> (Bits, Bits) {
    let lre = log_abs(zv.re, zv.im);
    let lim = atan2_x(x_of(zv.im), x_of(zv.re), neg_of(zv.im, zv.ib, f), neg_of(zv.re, zv.rb, f));
    let (wr, wi) = (x_of(wv.re), x_of(wv.im));
    let pr = wr.mul(lre).sub(wi.mul(lim));
    let pi_ = wr.mul(lim).add(wi.mul(lre));
    let (pn, ps, pe) = parts(pi_);
    let (s, c) = if pi_.is_zero() { (X::ZERO, X::ONE) } else { sincos(pn, ps, pe) };
    if pr.cmp(X::from_f64(12000.0)) == Ordering::Greater {
        return (overflow(c.is_neg(), f), overflow(s.is_neg(), f));
    }
    if pr.cmp(X::from_f64(-12000.0)) == Ordering::Less {
        return (underflow(c.is_neg(), f), underflow(s.is_neg(), f));
    }
    let e = exp_x(pr);
    (round_x(e.mul(c), c.is_neg(), f), round_x(e.mul(s), s.is_neg(), f))
}

fn cpow(zv: Z, wv: Z, f: &Format) -> (Bits, Bits) {
    if is_zero(wv.re) && is_zero(wv.im) {
        return (one(f), zero(f));
    }
    let finite = [zv.re, zv.im, wv.re, wv.im].iter().all(|&v| is_fin(v));
    if finite {
        if is_zero(zv.re) && is_zero(zv.im) {
            if !neg_of(wv.re, wv.rb, f) && !is_zero(wv.re) {
                return (zero(f), zero(f));
            }
        } else {
            return cpow_finite(zv, wv, f);
        }
    }
    // Infinities, NaNs and a zero base: e^(w ln z) with IEEE 754's special values, in double.
    let to_d = |b: Bits| f64::from_bits(ld::round_to(b, f, &F64) as u64);
    let lz = clog(z(to_d(zv.rb).to_bits() as u128, to_d(zv.ib).to_bits() as u128, &F64), &F64);
    let (pr, pi_) = mul_g(to_d(wv.rb), to_d(wv.ib), f64::from_bits(lz.0 as u64), f64::from_bits(lz.1 as u64));
    let r = cexp(z(pr.to_bits() as u128, pi_.to_bits() as u128, &F64), &F64);
    (ld::round_to(r.0, &F64, f), ld::round_to(r.1, &F64, f))
}

// ----------------------------------------------------------------------------
// Hyperbolic and trigonometric functions
// ----------------------------------------------------------------------------

/// csinh on the first quadrant: x and y non-negative.
fn csinh_q(z: &Z, a: Val, b: Val, f: &Format) -> (Bits, Bits) {
    match a {
        Val::Zero(_) => match b {
            Val::Zero(_) => (zero(f), zero(f)),
            Val::Inf(_) => (zero(f), invalid_nan(f)),
            Val::Nan { .. } => (zero(f), nan(z, f)),
            _ => {
                // sinh(+0) cos y + i cosh(0) sin y.
                let (s, c) = sc(b);
                (signed(zero(f), c.is_neg(), f), val(s, f))
            }
        },
        Val::Inf(_) => match b {
            Val::Zero(_) => (inf(f), zero(f)),
            Val::Fin(..) => {
                let (s, c) = sc(b);
                (signed(inf(f), c.is_neg(), f), signed(inf(f), s.is_neg(), f))
            }
            Val::Inf(_) => (inf(f), invalid_nan(f)),
            Val::Nan { .. } => (inf(f), nan(z, f)),
        },
        Val::Nan { .. } => if is_zero(b) { (nan(z, f), zero(f)) } else { (nan(z, f), nan(z, f)) },
        Val::Fin(..) => match b {
            Val::Inf(_) => (invalid_nan(f), invalid_nan(f)),
            Val::Nan { .. } => (nan(z, f), nan(z, f)),
            Val::Zero(_) => (ld::sinh(z.rb & !sign_bit(f), f), zero(f)),
            _ => {
                let (s, c) = sc(b);
                if x_of(a).cmp_abs(X::from_f64(12000.0)) == Ordering::Greater {
                    return (overflow(c.is_neg(), f), overflow(s.is_neg(), f));
                }
                let (sh, ch) = sh_ch(a);
                (round_x(sh.mul(c), c.is_neg(), f), round_x(ch.mul(s), false, f))
            }
        },
    }
}

fn csinh(z: Z, f: &Format) -> (Bits, Bits) {
    let (xn, yn) = (neg_of(z.re, z.rb, f), neg_of(z.im, z.ib, f));
    let (re, im) = csinh_q(&z, abs(z.re), abs(z.im), f);
    (signed(re, xn, f), signed(im, yn, f))
}

/// ccosh on the first quadrant.
fn ccosh_q(z: &Z, a: Val, b: Val, f: &Format) -> (Bits, Bits) {
    match a {
        Val::Zero(_) => match b {
            Val::Zero(_) => (one(f), zero(f)),
            Val::Inf(_) => (invalid_nan(f), zero(f)),
            Val::Nan { .. } => (nan(z, f), zero(f)),
            _ => {
                // cosh(0) cos y + i sinh(+0) sin y.
                let (s, c) = sc(b);
                (round_x(c, c.is_neg(), f), signed(zero(f), s.is_neg(), f))
            }
        },
        Val::Inf(_) => match b {
            Val::Zero(_) => (inf(f), zero(f)),
            Val::Fin(..) => {
                let (s, c) = sc(b);
                (signed(inf(f), c.is_neg(), f), signed(inf(f), s.is_neg(), f))
            }
            Val::Inf(_) => (inf(f), invalid_nan(f)),
            Val::Nan { .. } => (inf(f), nan(z, f)),
        },
        Val::Nan { .. } => if is_zero(b) { (nan(z, f), zero(f)) } else { (nan(z, f), nan(z, f)) },
        Val::Fin(..) => match b {
            Val::Inf(_) => (invalid_nan(f), invalid_nan(f)),
            Val::Nan { .. } => (nan(z, f), nan(z, f)),
            Val::Zero(_) => (ld::cosh(z.rb, f), zero(f)),
            _ => {
                let (s, c) = sc(b);
                if x_of(a).cmp_abs(X::from_f64(12000.0)) == Ordering::Greater {
                    return (overflow(c.is_neg(), f), overflow(s.is_neg(), f));
                }
                let (sh, ch) = sh_ch(a);
                (round_x(ch.mul(c), c.is_neg(), f), round_x(sh.mul(s), false, f))
            }
        },
    }
}

fn ccosh(z: Z, f: &Format) -> (Bits, Bits) {
    let (xn, yn) = (neg_of(z.re, z.rb, f), neg_of(z.im, z.ib, f));
    let (re, im) = ccosh_q(&z, abs(z.re), abs(z.im), f);
    (re, signed(im, xn != yn, f))
}

/// ctanh on the first quadrant.
fn ctanh_q(z: &Z, a: Val, b: Val, f: &Format) -> (Bits, Bits) {
    match a {
        Val::Zero(_) => match b {
            Val::Zero(_) => (zero(f), zero(f)),
            Val::Inf(_) => (zero(f), invalid_nan(f)),
            Val::Nan { .. } => (zero(f), nan(z, f)),
            _ => {
                let (s, c) = sc(b);
                (zero(f), val(s.div(c), f))
            }
        },
        Val::Inf(_) => match b {
            Val::Fin(..) => {
                // 1 + i0 sin(2y): the zero takes sin(2y)'s sign, sin y cos y's.
                let (s, c) = sc(b);
                (one(f), signed(zero(f), s.is_neg() != c.is_neg(), f))
            }
            _ => (one(f), zero(f)),
        },
        Val::Nan { .. } => if is_zero(b) { (nan(z, f), zero(f)) } else { (nan(z, f), nan(z, f)) },
        Val::Fin(..) => match b {
            Val::Inf(_) => (invalid_nan(f), invalid_nan(f)),
            Val::Nan { .. } => (nan(z, f), nan(z, f)),
            Val::Zero(_) => (ld::tanh(z.rb & !sign_bit(f), f), zero(f)),
            _ => {
                let (s, c) = sc(b);
                let ax = x_of(a);
                if ax.cmp_abs(X::from_f64(40.0)) == Ordering::Greater {
                    // tanh x is 1 to beyond any format's precision; the imaginary part is
                    // 4 sin y cos y e^(-2x).
                    let im = if ax.cmp_abs(X::from_f64(1.0e5)) == Ordering::Greater {
                        underflow(s.is_neg() != c.is_neg(), f)
                    } else {
                        round_x(s.mul(c).scale(2).mul(exp_x(ax.scale(1).neg())), false, f)
                    };
                    let p = f.p;
                    let re = super::ext::round_parts(false, (1u128 << (p + 1)) - 1, -(p as i64 + 1), true, f);
                    return (re, im);
                }
                // Kahan: t = tan y, beta = 1 + t^2, s = sinh x, rho = sqrt(1 + s^2):
                // tanh z = (beta rho s + i t) / (1 + beta s^2).
                let t = s.div(c);
                let beta = X::ONE.add(t.sqr());
                let (sh, ch) = sh_ch(a);
                let d = X::ONE.add(beta.mul(sh.sqr()));
                (val(beta.mul(ch).mul(sh).div(d), f), round_x(t.div(d), t.is_neg(), f))
            }
        },
    }
}

fn ctanh(z: Z, f: &Format) -> (Bits, Bits) {
    let (xn, yn) = (neg_of(z.re, z.rb, f), neg_of(z.im, z.ib, f));
    let (re, im) = ctanh_q(&z, abs(z.re), abs(z.im), f);
    (signed(re, xn, f), signed(im, yn, f))
}

/// i z.
fn times_i(z: Z, f: &Format) -> Z {
    self::z(flip(z.ib, f), z.rb, f)
}

/// -i w for w = (re, im): (im, -re).
fn times_minus_i(w: (Bits, Bits), f: &Format) -> (Bits, Bits) {
    (w.1, flip(w.0, f))
}

fn csin(z: Z, f: &Format) -> (Bits, Bits) {
    times_minus_i(csinh(times_i(z, f), f), f)
}

fn ccos(z: Z, f: &Format) -> (Bits, Bits) {
    ccosh(times_i(z, f), f)
}

fn ctan(z: Z, f: &Format) -> (Bits, Bits) {
    times_minus_i(ctanh(times_i(z, f), f), f)
}

// ----------------------------------------------------------------------------
// Inverse functions
// ----------------------------------------------------------------------------

/// The real part of a product of complex numbers (a, b) (c, d).
fn re_mul(a: (X, X), c: (X, X)) -> X {
    a.0.mul(c.0).sub(a.1.mul(c.1))
}

fn casinh(z: Z, f: &Format) -> (Bits, Bits) {
    let (xn, yn) = (neg_of(z.re, z.rb, f), neg_of(z.im, z.ib, f));
    let (a, b) = (abs(z.re), abs(z.im));
    let (re, im) = match (a, b) {
        (Val::Nan { .. }, Val::Zero(_)) => (nan(&z, f), zero(f)),
        (Val::Nan { .. }, Val::Inf(_)) => (inf(f), nan(&z, f)),
        (Val::Nan { .. }, _) => (nan(&z, f), nan(&z, f)),
        (Val::Inf(_), Val::Nan { .. }) => (inf(f), nan(&z, f)),
        (_, Val::Nan { .. }) => (nan(&z, f), nan(&z, f)),
        (Val::Inf(_), Val::Inf(_)) => (inf(f), val(pi().mul_f(0.25), f)),
        (Val::Inf(_), _) => (inf(f), zero(f)),
        (_, Val::Inf(_)) => (inf(f), val(pi_2(), f)),
        (Val::Zero(_), Val::Zero(_)) => (zero(f), zero(f)),
        _ => {
            // casinh z = -i casin(i z): with s1 = sqrt((1 + b) - i a), s2 = sqrt((1 - b) + i a),
            // casinh z = asinh(Re s1 Im s2 - Im s1 Re s2) + i atan2(b, Re(s1 s2)).
            let (xa, xb) = (x_of(a), x_of(b));
            let s1 = csqrt_x(X::ONE.add(xb), xa.neg(), true);
            let s2 = csqrt_x(X::ONE.sub(xb), xa, false);
            let r = asinh_x(s1.0.mul(s2.1).sub(s1.1.mul(s2.0)));
            let d = re_mul(s1, s2);
            let i = atan2_x(xb, d, false, d.is_neg());
            (val(r, f), val(i, f))
        }
    };
    (signed(re, xn, f), signed(im, yn, f))
}

fn casin(z: Z, f: &Format) -> (Bits, Bits) {
    times_minus_i(casinh(times_i(z, f), f), f)
}

fn cacos(z: Z, f: &Format) -> (Bits, Bits) {
    let yn = neg_of(z.im, z.ib, f);
    let x = z.re;
    let b = abs(z.im);
    let xn = neg_of(x, z.rb, f);
    let minus_inf = signed(inf(f), true, f);
    let (re, im) = match (x, b) {
        (Val::Nan { .. }, Val::Inf(_)) => (nan(&z, f), minus_inf),
        (Val::Nan { .. }, _) => (nan(&z, f), nan(&z, f)),
        (Val::Zero(_), Val::Nan { .. }) => (val(pi_2(), f), nan(&z, f)),
        (Val::Inf(_), Val::Nan { .. }) => (nan(&z, f), minus_inf),
        (_, Val::Nan { .. }) => (nan(&z, f), nan(&z, f)),
        (Val::Inf(_), Val::Inf(_)) => (val(pi().mul_f(if xn { 0.75 } else { 0.25 }), f), minus_inf),
        (_, Val::Inf(_)) => (val(pi_2(), f), minus_inf),
        (Val::Inf(_), _) => (if xn { val(pi(), f) } else { zero(f) }, minus_inf),
        (Val::Zero(_), Val::Zero(_)) => (val(pi_2(), f), signed(zero(f), true, f)),
        _ => {
            // s1 = sqrt(1 - z), s2 = sqrt(1 + z): cacos z = 2 atan2(Re s1, Re s2)
            // + i asinh(Re s2 Im s1 - Im s2 Re s1).
            let (xx, xb) = (x_of(x), x_of(b));
            let s1 = csqrt_x(X::ONE.sub(xx), xb.neg(), true);
            let s2 = csqrt_x(X::ONE.add(xx), xb, false);
            let r = atan2_x(s1.0, s2.0, false, false).scale(1);
            let i = asinh_x(s2.0.mul(s1.1).sub(s2.1.mul(s1.0)));
            (val(r, f), round_x(i, true, f))
        }
    };
    (re, signed(im, yn, f))
}

fn cacosh(z: Z, f: &Format) -> (Bits, Bits) {
    let yn = neg_of(z.im, z.ib, f);
    let x = z.re;
    let b = abs(z.im);
    let xn = neg_of(x, z.rb, f);
    let (re, im) = match (x, b) {
        (Val::Nan { .. }, Val::Inf(_)) => (inf(f), nan(&z, f)),
        (Val::Nan { .. }, _) => (nan(&z, f), nan(&z, f)),
        (Val::Inf(_), Val::Nan { .. }) => (inf(f), nan(&z, f)),
        (_, Val::Nan { .. }) => (nan(&z, f), nan(&z, f)),
        (Val::Inf(_), Val::Inf(_)) => (inf(f), val(pi().mul_f(if xn { 0.75 } else { 0.25 }), f)),
        (_, Val::Inf(_)) => (inf(f), val(pi_2(), f)),
        (Val::Inf(_), _) => (inf(f), if xn { val(pi(), f) } else { zero(f) }),
        (Val::Zero(_), Val::Zero(_)) => (zero(f), val(pi_2(), f)),
        _ => {
            // s1 = sqrt(z - 1), s2 = sqrt(z + 1): cacosh z = asinh(Re s1 Re s2 + Im s1 Im s2)
            // + 2i atan2(Im s1, Re s2).
            let (xx, xb) = (x_of(x), x_of(b));
            let s1 = csqrt_x(xx.add_f(-1.0), xb, false);
            let s2 = csqrt_x(xx.add_f(1.0), xb, false);
            let r = asinh_x(s1.0.mul(s2.0).add(s1.1.mul(s2.1)));
            let i = atan2_x(s1.1, s2.0, false, false).scale(1);
            (val(r, f), val(i, f))
        }
    };
    (re, signed(im, yn, f))
}

fn catanh(z: Z, f: &Format) -> (Bits, Bits) {
    let (xn, yn) = (neg_of(z.re, z.rb, f), neg_of(z.im, z.ib, f));
    let (a, b) = (abs(z.re), abs(z.im));
    let half_pi = || val(pi_2(), f);
    let (re, im) = match (a, b) {
        (Val::Nan { .. }, Val::Inf(_)) => (zero(f), half_pi()),
        (Val::Nan { .. }, _) => (nan(&z, f), nan(&z, f)),
        (Val::Zero(_) | Val::Inf(_), Val::Nan { .. }) => (zero(f), nan(&z, f)),
        (_, Val::Nan { .. }) => (nan(&z, f), nan(&z, f)),
        (Val::Inf(_), _) | (_, Val::Inf(_)) => (zero(f), half_pi()),
        (Val::Zero(_), Val::Zero(_)) => (zero(f), zero(f)),
        _ => {
            let (xa, xb) = (x_of(a), x_of(b));
            if is_zero(b) && xa.cmp(X::ONE) == Ordering::Equal {
                fenv::raise(fenv::FE_DIVBYZERO);
                (inf(f), zero(f))
            } else {
                // Re = log1p(4a / ((1 - a)^2 + b^2)) / 4, Im = atan2(2b, (1 - a)(1 + a) - b^2) / 2.
                let one_m = X::ONE.sub(xa);
                let d = one_m.sqr().add(xb.sqr());
                let r = log1p_x(xa.scale(2).div(d)).scale(-2);
                let den = one_m.mul(X::ONE.add(xa)).sub(xb.sqr());
                let i = atan2_x(xb.scale(1), den, false, den.is_neg()).scale(-1);
                (val(r, f), val(i, f))
            }
        }
    };
    (signed(re, xn, f), signed(im, yn, f))
}

fn catan(z: Z, f: &Format) -> (Bits, Bits) {
    times_minus_i(catanh(times_i(z, f), f), f)
}

// ----------------------------------------------------------------------------
// Multiplication and division (C17 G.5.1)
// ----------------------------------------------------------------------------

/// The arithmetic Annex G's algorithms need, over one floating type.
trait Real: Copy {
    fn add(self, o: Self) -> Self;
    fn sub(self, o: Self) -> Self;
    fn mul(self, o: Self) -> Self;
    fn div(self, o: Self) -> Self;
    fn is_nan(self) -> bool;
    fn is_inf(self) -> bool;
    fn is_finite(self) -> bool {
        !self.is_nan() && !self.is_inf()
    }
    fn is_zero(self) -> bool;
    fn sign_neg(self) -> bool;
    /// |self| with the sign of `s`.
    fn copysign(self, s: Self) -> Self;
    fn fabs(self) -> Self;
    fn fmax(self, o: Self) -> Self;
    fn logb(self) -> Self;
    fn scalbn(self, n: i32) -> Self;
    /// A small integer value: -1, 0, 1, or an infinity for 2 (the IEEE infinity).
    fn int(k: i32) -> Self;
    fn infinity() -> Self;
    fn to_i32(self) -> i32;
}

impl Real for f64 {
    fn add(self, o: f64) -> f64 { core::hint::black_box(self) + o }
    fn sub(self, o: f64) -> f64 { core::hint::black_box(self) - o }
    fn mul(self, o: f64) -> f64 { core::hint::black_box(self) * o }
    fn div(self, o: f64) -> f64 { core::hint::black_box(self) / o }
    fn is_nan(self) -> bool { self.is_nan() }
    fn is_inf(self) -> bool { self.is_infinite() }
    fn is_zero(self) -> bool { self == 0.0 }
    fn sign_neg(self) -> bool { self.is_sign_negative() }
    fn copysign(self, s: f64) -> f64 { super::basic::copysign(self, s) }
    fn fabs(self) -> f64 { super::basic::fabs(self) }
    fn fmax(self, o: f64) -> f64 { super::basic::fmax(self, o) }
    fn logb(self) -> f64 { super::basic::logb(self) }
    fn scalbn(self, n: i32) -> f64 { super::basic::scalbn(self, n) }
    fn int(k: i32) -> f64 { k as f64 }
    fn infinity() -> f64 { f64::INFINITY }
    fn to_i32(self) -> i32 { self as i32 }
}

impl Real for f32 {
    fn add(self, o: f32) -> f32 { core::hint::black_box(self) + o }
    fn sub(self, o: f32) -> f32 { core::hint::black_box(self) - o }
    fn mul(self, o: f32) -> f32 { core::hint::black_box(self) * o }
    fn div(self, o: f32) -> f32 { core::hint::black_box(self) / o }
    fn is_nan(self) -> bool { self.is_nan() }
    fn is_inf(self) -> bool { self.is_infinite() }
    fn is_zero(self) -> bool { self == 0.0 }
    fn sign_neg(self) -> bool { self.is_sign_negative() }
    fn copysign(self, s: f32) -> f32 { super::basic::copysignf(self, s) }
    fn fabs(self) -> f32 { super::basic::fabsf(self) }
    fn fmax(self, o: f32) -> f32 { super::basic::fmaxf(self, o) }
    fn logb(self) -> f32 { super::basic::logbf(self) }
    fn scalbn(self, n: i32) -> f32 { super::basic::scalbnf(self, n) }
    fn int(k: i32) -> f32 { k as f32 }
    fn infinity() -> f32 { f32::INFINITY }
    fn to_i32(self) -> i32 { self as i32 }
}

/// A long double, by its encoding in the native format.
#[derive(Clone, Copy)]
#[cfg_attr(test, allow(dead_code))]
struct L(Bits);

#[cfg(target_arch = "aarch64")]
#[cfg_attr(test, allow(dead_code))]
const LF: &Format = &crate::stdlib::floatconv::F128;
#[cfg(target_arch = "x86_64")]
#[cfg_attr(test, allow(dead_code))]
const LF: &Format = &crate::stdlib::floatconv::X87;

impl Real for L {
    fn add(self, o: L) -> L { L(ld::add(self.0, o.0, LF)) }
    fn sub(self, o: L) -> L { L(ld::add(self.0, flip(o.0, LF), LF)) }
    fn mul(self, o: L) -> L { L(ld::mul(self.0, o.0, LF)) }
    fn div(self, o: L) -> L { L(ld::div(self.0, o.0, LF)) }
    fn is_nan(self) -> bool { is_nan(unpack(self.0, LF)) }
    fn is_inf(self) -> bool { is_inf(unpack(self.0, LF)) }
    fn is_zero(self) -> bool { is_zero(unpack(self.0, LF)) }
    fn sign_neg(self) -> bool { self.0 & sign_bit(LF) != 0 }
    fn copysign(self, s: L) -> L { L(ld::copysign(self.0, s.0, LF)) }
    fn fabs(self) -> L { L(ld::fabs(self.0, LF)) }
    fn fmax(self, o: L) -> L { L(ld::fmax(self.0, o.0, LF)) }
    fn logb(self) -> L {
        let saved = crate::pthread::errno();
        let r = L(ld::logb(self.0, LF));
        crate::pthread::set_errno(saved);
        r
    }
    fn scalbn(self, n: i32) -> L { L(ld::scalbn(self.0, n as i64, LF)) }
    fn int(k: i32) -> L { L(ld::round_to((k as f64).to_bits() as u128, &F64, LF)) }
    fn infinity() -> L { L(LF.infinity(false)) }
    fn to_i32(self) -> i32 { ld::lround(self.0, LF) as i32 }
}

/// (a + ib)(c + id), recovering infinities (C17 G.5.1, example 1).
fn mul_g<T: Real>(mut a: T, mut b: T, mut c: T, mut d: T) -> (T, T) {
    let (ac, bd, ad, bc) = (a.mul(c), b.mul(d), a.mul(d), b.mul(c));
    let (mut x, mut y) = (ac.sub(bd), ad.add(bc));
    if x.is_nan() && y.is_nan() {
        let mut recalc = false;
        let unit = |v: T| T::int(v.is_inf() as i32).copysign(v);
        let zero_nan = |v: T| if v.is_nan() { T::int(0).copysign(v) } else { v };
        if a.is_inf() || b.is_inf() {
            a = unit(a);
            b = unit(b);
            c = zero_nan(c);
            d = zero_nan(d);
            recalc = true;
        }
        if c.is_inf() || d.is_inf() {
            c = unit(c);
            d = unit(d);
            a = zero_nan(a);
            b = zero_nan(b);
            recalc = true;
        }
        if !recalc && (ac.is_inf() || bd.is_inf() || ad.is_inf() || bc.is_inf()) {
            a = zero_nan(a);
            b = zero_nan(b);
            c = zero_nan(c);
            d = zero_nan(d);
            recalc = true;
        }
        if recalc {
            x = T::infinity().mul(a.mul(c).sub(b.mul(d)));
            y = T::infinity().mul(a.mul(d).add(b.mul(c)));
        }
    }
    (x, y)
}

/// (a + ib) / (c + id), scaling to avoid overflow and recovering infinities and zeros
/// (C17 G.5.1, example 2).
fn div_g<T: Real>(mut a: T, mut b: T, mut c: T, mut d: T) -> (T, T) {
    let logbw = c.fabs().fmax(d.fabs()).logb();
    let mut ilogbw = 0;
    if logbw.is_finite() {
        ilogbw = logbw.to_i32();
        c = c.scalbn(-ilogbw);
        d = d.scalbn(-ilogbw);
    }
    let denom = c.mul(c).add(d.mul(d));
    let mut x = a.mul(c).add(b.mul(d)).div(denom).scalbn(-ilogbw);
    let mut y = b.mul(c).sub(a.mul(d)).div(denom).scalbn(-ilogbw);
    if x.is_nan() && y.is_nan() {
        let unit = |v: T| T::int(v.is_inf() as i32).copysign(v);
        if denom.is_zero() && (!a.is_nan() || !b.is_nan()) {
            x = T::infinity().copysign(c).mul(a);
            y = T::infinity().copysign(c).mul(b);
        } else if (a.is_inf() || b.is_inf()) && c.is_finite() && d.is_finite() {
            a = unit(a);
            b = unit(b);
            x = T::infinity().mul(a.mul(c).add(b.mul(d)));
            y = T::infinity().mul(b.mul(c).sub(a.mul(d)));
        } else if logbw.is_inf() && !logbw.sign_neg() && a.is_finite() && b.is_finite() {
            c = unit(c);
            d = unit(d);
            x = T::int(0).mul(a.mul(c).add(b.mul(d)));
            y = T::int(0).mul(b.mul(c).sub(a.mul(d)));
        }
    }
    (x, y)
}

// ----------------------------------------------------------------------------
// C entry points
// ----------------------------------------------------------------------------

/// The exported functions, by operand shape, on (format, re, im) encodings.
type Unary = fn(Z, &Format) -> (Bits, Bits);

fn apply(func: Unary, re: Bits, im: Bits, f: &Format) -> (Bits, Bits) {
    func(z(re, im, f), f)
}

fn apply_real(func: fn(Z, &Format) -> Bits, re: Bits, im: Bits, f: &Format) -> Bits {
    func(z(re, im, f), f)
}

fn apply_pow(re: Bits, im: Bits, wre: Bits, wim: Bits, f: &Format) -> (Bits, Bits) {
    cpow(z(re, im, f), z(wre, wim, f), f)
}

fn creal_(z: Z, _f: &Format) -> Bits {
    z.rb
}

fn cimag_(z: Z, _f: &Format) -> Bits {
    z.ib
}

#[cfg(not(test))]
mod abi {
    use super::*;
    use crate::stdlib::floatconv::F32;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Cd {
        re: f64,
        im: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Cf {
        re: f32,
        im: f32,
    }

    fn cd(r: (Bits, Bits)) -> Cd {
        Cd { re: f64::from_bits(r.0 as u64), im: f64::from_bits(r.1 as u64) }
    }

    fn cf(r: (Bits, Bits)) -> Cf {
        Cf { re: f32::from_bits(r.0 as u32), im: f32::from_bits(r.1 as u32) }
    }

    macro_rules! complex_fns {
        ($($d:ident, $fl:ident => $func:ident;)*) => {$(
            #[no_mangle]
            pub extern "C" fn $d(z: Cd) -> Cd {
                cd(apply(super::$func, z.re.to_bits() as u128, z.im.to_bits() as u128, &F64))
            }
            #[no_mangle]
            pub extern "C" fn $fl(z: Cf) -> Cf {
                cf(apply(super::$func, z.re.to_bits() as u128, z.im.to_bits() as u128, &F32))
            }
        )*};
    }

    complex_fns!(
        cacos, cacosf => cacos; casin, casinf => casin; catan, catanf => catan;
        ccos, ccosf => ccos; csin, csinf => csin; ctan, ctanf => ctan;
        cacosh, cacoshf => cacosh; casinh, casinhf => casinh; catanh, catanhf => catanh;
        ccosh, ccoshf => ccosh; csinh, csinhf => csinh; ctanh, ctanhf => ctanh;
        cexp, cexpf => cexp; clog, clogf => clog; csqrt, csqrtf => csqrt;
        conj, conjf => conj; cproj, cprojf => cproj;
    );

    macro_rules! real_fns {
        ($($d:ident, $fl:ident => $func:ident;)*) => {$(
            #[no_mangle]
            pub extern "C" fn $d(z: Cd) -> f64 {
                f64::from_bits(apply_real(super::$func, z.re.to_bits() as u128, z.im.to_bits() as u128, &F64) as u64)
            }
            #[no_mangle]
            pub extern "C" fn $fl(z: Cf) -> f32 {
                f32::from_bits(apply_real(super::$func, z.re.to_bits() as u128, z.im.to_bits() as u128, &F32) as u32)
            }
        )*};
    }

    real_fns!(cabs, cabsf => cabs; carg, cargf => carg; creal, crealf => creal_; cimag, cimagf => cimag_;);

    #[no_mangle]
    pub extern "C" fn cpow(z: Cd, w: Cd) -> Cd {
        let b = |v: f64| v.to_bits() as u128;
        cd(super::apply_pow(b(z.re), b(z.im), b(w.re), b(w.im), &F64))
    }

    #[no_mangle]
    pub extern "C" fn cpowf(z: Cf, w: Cf) -> Cf {
        let b = |v: f32| v.to_bits() as u128;
        cf(super::apply_pow(b(z.re), b(z.im), b(w.re), b(w.im), &F32))
    }

    #[no_mangle]
    pub extern "C" fn __muldc3(a: f64, b: f64, c: f64, d: f64) -> Cd {
        let (re, im) = mul_g(a, b, c, d);
        Cd { re, im }
    }

    #[no_mangle]
    pub extern "C" fn __divdc3(a: f64, b: f64, c: f64, d: f64) -> Cd {
        let (re, im) = div_g(a, b, c, d);
        Cd { re, im }
    }

    #[no_mangle]
    pub extern "C" fn __mulsc3(a: f32, b: f32, c: f32, d: f32) -> Cf {
        let (re, im) = mul_g(a, b, c, d);
        Cf { re, im }
    }

    #[no_mangle]
    pub extern "C" fn __divsc3(a: f32, b: f32, c: f32, d: f32) -> Cf {
        let (re, im) = div_g(a, b, c, d);
        Cf { re, im }
    }

    #[cfg(target_arch = "aarch64")]
    mod long {
        //! binary128 complex values travel as two f128s in v0 and v1.
        use super::super::*;

        #[repr(C)]
        pub struct Cl {
            re: f128,
            im: f128,
        }

        fn cl(r: (Bits, Bits)) -> Cl {
            Cl { re: f128::from_bits(r.0), im: f128::from_bits(r.1) }
        }

        macro_rules! complex_fns {
            ($($name:ident => $func:ident;)*) => {$(
                #[no_mangle]
                pub extern "C" fn $name(z: Cl) -> Cl {
                    cl(apply($func, z.re.to_bits(), z.im.to_bits(), LF))
                }
            )*};
        }

        complex_fns!(
            cacosl => cacos; casinl => casin; catanl => catan; ccosl => ccos; csinl => csin;
            ctanl => ctan; cacoshl => cacosh; casinhl => casinh; catanhl => catanh;
            ccoshl => ccosh; csinhl => csinh; ctanhl => ctanh; cexpl => cexp; clogl => clog;
            csqrtl => csqrt; conjl => conj; cprojl => cproj;
        );

        macro_rules! real_fns {
            ($($name:ident => $func:ident;)*) => {$(
                #[no_mangle]
                pub extern "C" fn $name(z: Cl) -> f128 {
                    f128::from_bits(apply_real($func, z.re.to_bits(), z.im.to_bits(), LF))
                }
            )*};
        }

        real_fns!(cabsl => cabs; cargl => carg; creall => creal_; cimagl => cimag_;);

        #[no_mangle]
        pub extern "C" fn cpowl(z: Cl, w: Cl) -> Cl {
            cl(apply_pow(z.re.to_bits(), z.im.to_bits(), w.re.to_bits(), w.im.to_bits(), LF))
        }

        #[no_mangle]
        pub extern "C" fn __multc3(a: f128, b: f128, c: f128, d: f128) -> Cl {
            let (re, im) = mul_g(L(a.to_bits()), L(b.to_bits()), L(c.to_bits()), L(d.to_bits()));
            cl((re.0, im.0))
        }

        #[no_mangle]
        pub extern "C" fn __divtc3(a: f128, b: f128, c: f128, d: f128) -> Cl {
            let (re, im) = div_g(L(a.to_bits()), L(b.to_bits()), L(c.to_bits()), L(d.to_bits()));
            cl((re.0, im.0))
        }
    }

    #[cfg(target_arch = "x86_64")]
    mod long {
        //! x87 complex values travel on the stack, the parts in consecutive 16-byte slots,
        //! and come back in st(0) (real) and st(1) (imaginary). Each entry point passes the
        //! Rust function a pointer to its stack arguments and to a 32-byte result.
        use super::super::*;

        /// Slot `i` of the stack arguments: its meaningful 80 bits.
        unsafe fn slot(args: *const u8, i: usize) -> Bits {
            let p = args.add(16 * i);
            core::ptr::read_unaligned(p as *const u64) as u128
                | ((core::ptr::read_unaligned(p.add(8) as *const u16) as u128) << 64)
        }

        unsafe fn put(out: *mut u8, r: (Bits, Bits)) {
            core::ptr::write_unaligned(out as *mut u128, r.0);
            core::ptr::write_unaligned(out.add(16) as *mut u128, r.1);
        }

        macro_rules! complex_fns {
            ($($name:ident => $func:ident;)*) => {$(
                #[unsafe(naked)]
                #[no_mangle]
                pub unsafe extern "C" fn $name() {
                    core::arch::naked_asm!(
                        "lea rdi, [rsp + 8]",
                        "sub rsp, 40",
                        "mov rsi, rsp",
                        "call {f}",
                        "fld tbyte ptr [rsp + 16]",
                        "fld tbyte ptr [rsp]",
                        "add rsp, 40",
                        "ret",
                        f = sym $name::inner,
                    )
                }
                mod $name {
                    use super::*;
                    pub(super) unsafe extern "C" fn inner(args: *const u8, out: *mut u8) {
                        put(out, apply($func, slot(args, 0), slot(args, 1), LF));
                    }
                }
            )*};
        }

        complex_fns!(
            cacosl => cacos; casinl => casin; catanl => catan; ccosl => ccos; csinl => csin;
            ctanl => ctan; cacoshl => cacosh; casinhl => casinh; catanhl => catanh;
            ccoshl => ccosh; csinhl => csinh; ctanhl => ctanh; cexpl => cexp; clogl => clog;
            csqrtl => csqrt; conjl => conj; cprojl => cproj;
        );

        macro_rules! real_fns {
            ($($name:ident => $func:ident;)*) => {$(
                #[unsafe(naked)]
                #[no_mangle]
                pub unsafe extern "C" fn $name() {
                    core::arch::naked_asm!(
                        "lea rdi, [rsp + 8]",
                        "sub rsp, 40",
                        "mov rsi, rsp",
                        "call {f}",
                        "fld tbyte ptr [rsp]",
                        "add rsp, 40",
                        "ret",
                        f = sym $name::inner,
                    )
                }
                mod $name {
                    use super::*;
                    pub(super) unsafe extern "C" fn inner(args: *const u8, out: *mut u8) {
                        put(out, (apply_real($func, slot(args, 0), slot(args, 1), LF), 0));
                    }
                }
            )*};
        }

        real_fns!(cabsl => cabs; cargl => carg; creall => creal_; cimagl => cimag_;);

        /// A function of four long doubles returning a complex long double.
        macro_rules! four {
            ($($name:ident => |$a:ident, $b:ident, $c:ident, $d:ident| $body:expr;)*) => {$(
                #[unsafe(naked)]
                #[no_mangle]
                pub unsafe extern "C" fn $name() {
                    core::arch::naked_asm!(
                        "lea rdi, [rsp + 8]",
                        "sub rsp, 40",
                        "mov rsi, rsp",
                        "call {f}",
                        "fld tbyte ptr [rsp + 16]",
                        "fld tbyte ptr [rsp]",
                        "add rsp, 40",
                        "ret",
                        f = sym $name::inner,
                    )
                }
                mod $name {
                    use super::*;
                    pub(super) unsafe extern "C" fn inner(args: *const u8, out: *mut u8) {
                        let ($a, $b, $c, $d) = (slot(args, 0), slot(args, 1), slot(args, 2), slot(args, 3));
                        put(out, $body);
                    }
                }
            )*};
        }

        four!(
            cpowl => |a, b, c, d| apply_pow(a, b, c, d, LF);
            __mulxc3 => |a, b, c, d| {
                let (re, im) = mul_g(L(a), L(b), L(c), L(d));
                (re.0, im.0)
            };
            __divxc3 => |a, b, c, d| {
                let (re, im) = div_g(L(a), L(b), L(c), L(d));
                (re.0, im.0)
            };
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Python's cmath (double precision, within an ulp or two) for spot values.
    const CREFS: &[(&str, f64, f64, f64, f64)] = &[
        ("csin", 1.0, 1.0, 1.2984575814159773, 0.6349639147847361),
        ("ccos", 1.0, 1.0, 0.8337300251311491, -0.9888977057628651),
        ("ctan", 1.0, 1.0, 0.2717525853195118, 1.0839233273386943),
        ("csinh", 0.5, -2.0, -0.21685216292078974, -1.0253473885839877),
        ("ccosh", -0.5, 2.0, -0.4692579782290534, -0.473830620416407),
        ("ctanh", 0.3, 0.4, 0.3382547432654715, 0.38113207427244283),
        ("cexp", 1.5, -0.25, 4.342364210495382, -1.1087876201493594),
        ("clog", 3.0, -4.0, 1.6094379124341003, -0.9272952180016122),
        ("clog", 1e+300, 1e+300, 691.1221014884936, 0.7853981633974483),
        ("csqrt", -3.0, 4.0, 1.0, 2.0),
        ("casin", 0.5, 0.25, 0.5016088532755008, 0.28139605624529274),
        ("cacos", -0.75, 0.5, 2.2530002923783288, -0.6063349998873513),
        ("catan", 0.25, 2.0, 1.489877611175685, 0.5358800995910896),
        ("casinh", -2.0, 0.5, -1.4657153519472905, 0.22101863562288385),
        ("cacosh", 1.5, -0.5, 1.0693110431581105, -0.3996390673365241),
        ("catanh", 0.5, 0.5, 0.40235947810852507, 0.5535743588970452),
        ("casin", 3.0, 0.0, 1.5707963267948966, 1.762747174039086),
        ("cacosh", 0.5, 0.0, 0.0, 1.0471975511965979),
        ("catanh", 2.0, 0.0, 0.5493061443340549, 1.5707963267948966),
        ("catanh", 2.0, -0.0, 0.5493061443340549, -1.5707963267948966),
        ("cacos", 2.0, 0.0, 0.0, -1.3169578969248166),
        ("cacos", 2.0, -0.0, 0.0, 1.3169578969248166),
    ];

    fn d(v: f64) -> Bits {
        v.to_bits() as u128
    }

    fn run(func: Unary, x: f64, y: f64) -> (f64, f64) {
        let r = apply(func, d(x), d(y), &F64);
        (f64::from_bits(r.0 as u64), f64::from_bits(r.1 as u64))
    }

    fn by_name(name: &str) -> Unary {
        match name {
            "csin" => csin, "ccos" => ccos, "ctan" => ctan, "csinh" => csinh, "ccosh" => ccosh,
            "ctanh" => ctanh, "cexp" => cexp, "clog" => clog, "csqrt" => csqrt, "casin" => casin,
            "cacos" => cacos, "catan" => catan, "casinh" => casinh, "cacosh" => cacosh,
            "catanh" => catanh,
            _ => panic!("{name}"),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        if a == 0.0 || b == 0.0 {
            return a == b && a.is_sign_negative() == b.is_sign_negative() || (a - b).abs() < 1e-300;
        }
        a.is_sign_negative() == b.is_sign_negative() && (a.to_bits() as i64 - b.to_bits() as i64).abs() <= 2
    }

    #[test]
    fn against_cmath() {
        for &(name, x, y, re, im) in CREFS {
            let (r, i) = run(by_name(name), x, y);
            assert!(close(r, re) && close(i, im), "{name}({x}, {y}) = ({r:e}, {i:e}), want ({re:e}, {im:e})");
        }
    }

    #[test]
    fn annex_g_values() {
        let inf = f64::INFINITY;
        let nan = f64::NAN;
        let pi = core::f64::consts::PI;
        let same = |a: (f64, f64), b: (f64, f64)| {
            let eq = |p: f64, q: f64| (p.is_nan() && q.is_nan()) || (p == q && p.is_sign_negative() == q.is_sign_negative());
            eq(a.0, b.0) && eq(a.1, b.1)
        };
        let cases: &[(Unary, f64, f64, (f64, f64))] = &[
            (cexp, 0.0, -0.0, (1.0, -0.0)),
            (cexp, -inf, 1.0, (0.0, 0.0)),
            (cexp, inf, -0.0, (inf, -0.0)),
            (clog, -0.0, 0.0, (-inf, pi)),
            (clog, 0.0, -0.0, (-inf, -0.0)),
            (clog, -1.0, -0.0, (0.0, -pi)),
            (clog, -inf, inf, (inf, 0.75 * pi)),
            (clog, nan, inf, (inf, nan)),
            (csqrt, -4.0, 0.0, (0.0, 2.0)),
            (csqrt, -4.0, -0.0, (0.0, -2.0)),
            (csqrt, 3.0, 4.0, (2.0, 1.0)),
            (csqrt, nan, inf, (inf, inf)),
            (csqrt, -inf, 1.0, (0.0, inf)),
            (csqrt, 0.0, -0.0, (0.0, -0.0)),
            (csinh, -0.0, 0.0, (-0.0, 0.0)),
            (csinh, inf, 0.0, (inf, 0.0)),
            (ccosh, 0.0, 0.0, (1.0, 0.0)),
            (ccosh, -0.0, 0.0, (1.0, -0.0)),
            (ctanh, inf, 1.0, (1.0, 0.0)),
            (ctanh, -inf, -1.0, (-1.0, -0.0)),
            (ctanh, 1e6, 1.0, (1.0, 0.0)),
            (casinh, inf, 1.0, (inf, 0.0)),
            (casinh, -1.0, inf, (-inf, pi / 2.0)),
            (cacos, 0.0, 0.0, (pi / 2.0, -0.0)),
            (cacos, -0.0, -0.0, (pi / 2.0, 0.0)),
            (cacos, 1.0, 0.0, (0.0, -0.0)),
            (cacos, -inf, 1.0, (pi, -inf)),
            (cacosh, 0.0, 0.0, (0.0, pi / 2.0)),
            (cacosh, -inf, -1.0, (inf, -pi)),
            (catanh, 1.0, 0.0, (inf, 0.0)),
            (catanh, -1.0, -0.0, (-inf, -0.0)),
            (catanh, inf, nan, (0.0, nan)),
            (catanh, 0.5, inf, (0.0, pi / 2.0)),
            (cproj, -inf, -2.0, (inf, -0.0)),
            (conj, 1.0, 2.0, (1.0, -2.0)),
        ];
        for (i, &(func, x, y, want)) in cases.iter().enumerate() {
            let got = run(func, x, y);
            assert!(same(got, want), "case {i}: ({x}, {y}) -> {got:?}, want {want:?}");
        }
    }

    #[test]
    fn real_valued_and_power() {
        let real = |func: fn(Z, &Format) -> Bits, x: f64, y: f64| f64::from_bits(apply_real(func, d(x), d(y), &F64) as u64);
        assert_eq!(real(cabs, 3.0, -4.0), 5.0);
        assert_eq!(real(cabs, f64::NAN, -f64::INFINITY), f64::INFINITY);
        assert_eq!(real(carg, -1.0, 0.0), core::f64::consts::PI);
        assert_eq!(real(creal_, 1.5, 2.0), 1.5);
        assert_eq!(real(cimag_, 1.5, 2.0), 2.0);
        let pow = |a: f64, b: f64, c: f64, e: f64| {
            let r = apply_pow(d(a), d(b), d(c), d(e), &F64);
            (f64::from_bits(r.0 as u64), f64::from_bits(r.1 as u64))
        };
        let (r, i) = pow(1.0, 1.0, 2.0, 0.0);
        assert!(r.abs() < 1e-30 && i == 2.0, "(1+i)^2 = ({r}, {i})");
        assert_eq!(pow(2.0, 0.0, 10.0, 0.0), (1024.0, 0.0));
        assert_eq!(pow(5.0, 3.0, 0.0, 0.0), (1.0, 0.0));
        assert_eq!(pow(0.0, 0.0, 2.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn multiply_and_divide() {
        let inf = f64::INFINITY;
        assert_eq!(mul_g(1.0, 2.0, 3.0, 4.0), (-5.0, 10.0));
        let (x, y) = mul_g(inf, f64::NAN, 1.0, 0.0);
        assert!(x == inf && y.is_nan(), "{x} {y}");
        let (x, y) = mul_g(inf, 0.0, 0.0, 1.0);
        assert!(x.is_nan() || x == 0.0 || x.is_infinite(), "{x}");
        assert!(y.is_infinite(), "{y}");
        assert_eq!(div_g(-5.0, 10.0, 3.0, 4.0), (1.0, 2.0));
        let (x, y) = div_g(1.0, 0.0, 0.0, 0.0);
        assert!(x == inf && y.is_nan(), "{x} {y}");
        let (x, y) = div_g(1.0, 1.0, inf, 0.0);
        assert!(x == 0.0 && y == 0.0, "{x} {y}");
        let (x, y) = div_g(1e300, 1e300, 1e-300, 1e-300);
        assert!(x == inf || x > 1e307, "{x} {y}");
        // The divisor is scaled, not the dividend: G.5.1's example overflows a c + b d at
        // 1e308, so this stays below it.
        assert_eq!(div_g(1e300, 1e300, 1e300, 1e300), (1.0, 0.0));
    }
}
