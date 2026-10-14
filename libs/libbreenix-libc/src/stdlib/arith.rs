//! Integer arithmetic: abs, labs, llabs, imaxabs, div, ldiv, lldiv and imaxdiv
//! (ISO C17 7.22.6, 7.8.2). Division truncates toward zero, so the remainder takes the
//! sign of the dividend.

#[repr(C)]
pub struct DivT {
    quot: i32,
    rem: i32,
}

#[repr(C)]
pub struct LDivT {
    quot: i64,
    rem: i64,
}

#[no_mangle]
pub extern "C" fn abs(x: i32) -> i32 {
    x.wrapping_abs()
}

#[no_mangle]
pub extern "C" fn labs(x: i64) -> i64 {
    x.wrapping_abs()
}

#[no_mangle]
pub extern "C" fn llabs(x: i64) -> i64 {
    x.wrapping_abs()
}

#[no_mangle]
pub extern "C" fn imaxabs(x: i64) -> i64 {
    x.wrapping_abs()
}

#[no_mangle]
pub extern "C" fn div(a: i32, b: i32) -> DivT {
    DivT { quot: a.wrapping_div(b), rem: a.wrapping_rem(b) }
}

#[no_mangle]
pub extern "C" fn ldiv(a: i64, b: i64) -> LDivT {
    LDivT { quot: a.wrapping_div(b), rem: a.wrapping_rem(b) }
}

#[no_mangle]
pub extern "C" fn lldiv(a: i64, b: i64) -> LDivT {
    ldiv(a, b)
}

#[no_mangle]
pub extern "C" fn imaxdiv(a: i64, b: i64) -> LDivT {
    ldiv(a, b)
}
