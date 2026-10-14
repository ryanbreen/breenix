//! The float forms of the transcendental functions: each evaluates the double function on
//! the widened argument and rounds the result to float. The double result is within an
//! ulp of a double, far finer than a float's, so the float is correctly rounded except
//! when the exact value lies within that margin of a float rounding boundary. A float
//! overflow or underflow that the double result did not have is reported by `narrow`.

use super::narrow;

macro_rules! unary {
    ($($f:ident => $d:path),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $f(x: f32) -> f32 {
            narrow($d(x as f64))
        }
    )*};
}

macro_rules! binary {
    ($($f:ident => $d:path),* $(,)?) => {$(
        #[no_mangle]
        pub extern "C" fn $f(x: f32, y: f32) -> f32 {
            narrow($d(x as f64, y as f64))
        }
    )*};
}

unary! {
    expf => super::explog::exp,
    exp2f => super::explog::exp2,
    expm1f => super::explog::expm1,
    logf => super::explog::log,
    log2f => super::explog::log2,
    log10f => super::explog::log10,
    log1pf => super::explog::log1p,
    cbrtf => super::explog::cbrt,
    sinhf => super::explog::sinh,
    coshf => super::explog::cosh,
    tanhf => super::explog::tanh,
    asinhf => super::explog::asinh,
    acoshf => super::explog::acosh,
    atanhf => super::explog::atanh,
    sinf => super::trig::sin,
    cosf => super::trig::cos,
    tanf => super::trig::tan,
    asinf => super::trig::asin,
    acosf => super::trig::acos,
    atanf => super::trig::atan,
    erff => super::special::erf,
    erfcf => super::special::erfc,
    tgammaf => super::special::tgamma,
}

binary! {
    powf => super::explog::pow,
    hypotf => super::explog::hypot,
    atan2f => super::trig::atan2,
}

#[no_mangle]
pub unsafe extern "C" fn lgammaf(x: f32) -> f32 {
    narrow(super::special::lgamma(x as f64))
}

#[no_mangle]
pub unsafe extern "C" fn lgammaf_r(x: f32, sign: *mut i32) -> f32 {
    narrow(super::special::lgamma_r(x as f64, sign))
}
