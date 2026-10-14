//! <math.h> and <fenv.h> (ISO C17 7.12, 7.6 and Annex F; POSIX.1-2024).
//!
//! math_errhandling is MATH_ERRNO | MATH_ERREXCEPT: every domain, pole and range error
//! sets errno and raises the matching floating-point exception. The exceptions come from
//! real operations on values hidden from the optimizer (core::hint::black_box), so the
//! hardware raises them and the result honours the rounding direction. Each result also
//! passes through black_box, so the operation runs even where the caller discards it.

pub(crate) mod basic;
mod consts;
pub(crate) mod dd;
pub(crate) mod explog;
pub(crate) mod fenv;
mod float32;
pub(crate) mod special;
pub(crate) mod trig;

use crate::pthread::set_errno;
use crate::stdlib::floatconv::Converted;
use crate::stdlib::{EDOM, ERANGE};
use core::hint::black_box;

/// A domain error: EDOM, FE_INVALID, and a NaN.
pub(crate) fn domain() -> f64 {
    set_errno(EDOM);
    let z = black_box(0.0f64);
    black_box(z / z)
}

/// A pole error: ERANGE, FE_DIVBYZERO, and an infinity of the given sign.
pub(crate) fn pole(neg: bool) -> f64 {
    set_errno(ERANGE);
    let one = if neg { -1.0 } else { 1.0 };
    black_box(one / black_box(0.0f64))
}

/// An overflowing result: ERANGE, FE_OVERFLOW and FE_INEXACT, and the infinity (or the
/// largest finite value, as the rounding direction decides) of the given sign.
pub(crate) fn overflow(neg: bool) -> f64 {
    set_errno(ERANGE);
    let big = black_box(if neg { -f64::MAX } else { f64::MAX });
    black_box(big * 2.0)
}

/// An underflowing result: ERANGE, FE_UNDERFLOW and FE_INEXACT, and a zero (or the
/// smallest subnormal, as the rounding direction decides) of the given sign.
pub(crate) fn underflow(neg: bool) -> f64 {
    set_errno(ERANGE);
    let tiny = black_box(if neg { -f64::MIN_POSITIVE } else { f64::MIN_POSITIVE });
    black_box(tiny * f64::MIN_POSITIVE)
}

/// Reports a floatconv rounding: errno for overflow and underflow, and the flags.
pub(crate) fn report(c: &Converted) {
    let mut flags = 0;
    if c.inexact {
        flags |= fenv::FE_INEXACT;
    }
    if c.overflow {
        flags |= fenv::FE_OVERFLOW;
    }
    if c.underflow {
        flags |= fenv::FE_UNDERFLOW;
    }
    if c.overflow || c.underflow {
        set_errno(ERANGE);
    }
    fenv::raise(flags);
}

/// x itself, with a signaling NaN quieted.
pub(crate) fn quiet(x: f64) -> f64 {
    if x.is_nan() { x + x } else { x }
}

/// Rounds a double result to float, reporting a range error when the float overflows or
/// underflows where the double did not.
pub(crate) fn narrow(v: f64) -> f32 {
    let before = fenv::flags();
    fenv::set_flags(fenv::FE_OVERFLOW, 0);
    let r = black_box(v) as f32;
    let raised = fenv::flags();
    fenv::set_flags(fenv::FE_OVERFLOW, (before | raised) & fenv::FE_OVERFLOW);
    if v.is_finite() && raised & fenv::FE_OVERFLOW != 0 {
        set_errno(ERANGE);
    } else if v != 0.0 && basic::fabs(r as f64) < f32::MIN_POSITIVE as f64 && r as f64 != v {
        set_errno(ERANGE);
    }
    r
}
