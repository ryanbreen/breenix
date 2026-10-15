// Run with rustc --test: this exercises only STRMATH's number scanners, conversion,
// sorting and math on the host (aarch64 or x86-64 with SSE2), with errno kept here.

mod pthread {
    use std::cell::Cell;
    thread_local!(static ERRNO: Cell<i32> = const { Cell::new(0) });
    pub fn set_errno(e: i32) {
        ERRNO.with(|c| c.set(e));
    }
    pub fn errno() -> i32 {
        ERRNO.with(|c| c.get())
    }
}

#[path = "../libs/libbreenix-libc/src/string"]
mod string {
    #[allow(dead_code)] // signal_text serves strsignal, which these tests do not include
    mod errtext;
    pub(crate) use errtext::error_text;
}

#[allow(dead_code)]
#[path = "../libs/libbreenix-libc/src/stdlib"]
mod stdlib {
    pub mod floatconv;
    pub mod numscan;
    pub mod sort;
    pub const EDOM: i32 = 33;
    pub const EINVAL: i32 = 22;
    pub const ERANGE: i32 = 34;
}

#[allow(dead_code)]
#[path = "../libs/libbreenix-libc/src/math/mod.rs"]
mod math;

use stdlib::numscan::{scan_f32, scan_f64, scan_int, CStrChars, Chars};

/// A source that cannot rewind, as a stream is.
struct Stream<'a>(&'a [u8], usize);

impl Chars for Stream<'_> {
    fn peek(&mut self) -> Option<u32> {
        self.0.get(self.1).map(|&c| c as u32)
    }
    fn bump(&mut self) {
        self.1 += 1;
    }
    fn pos(&self) -> usize {
        self.1
    }
    fn rewind(&mut self, _pos: usize) -> bool {
        false
    }
}

fn f64_of(s: &str) -> Option<(f64, usize)> {
    let c = std::ffi::CString::new(s).unwrap();
    let mut chars = unsafe { CStrChars::new(c.as_ptr() as *const u8) };
    scan_f64(&mut chars, usize::MAX).map(|r| (r.value, chars.pos()))
}

#[test]
fn decimal_and_hex_forms_round_exactly() {
    for s in ["0.1", "1e23", "2.2250738585072011e-308", "4.9e-324", "1.7976931348623157e308", "123456789012345678901234567890",
        "0.30000000000000004", "9007199254740993", "1.00000000000000011102230246251565404236316680908203125"]
    {
        assert_eq!(f64_of(s).unwrap().0.to_bits(), s.parse::<f64>().unwrap().to_bits(), "{s}");
    }
    assert_eq!(f64_of("0x1.8p1"), Some((3.0, 7)));
    assert_eq!(f64_of("0x.8"), Some((0.5, 4)));
    assert_eq!(f64_of("0X1P-1074").unwrap().0.to_bits(), 1);
    assert_eq!(f64_of("3.25e2xyz"), Some((325.0, 6)));
}

#[test]
fn partial_subjects_rewind_on_strings_and_fail_on_streams() {
    assert_eq!(f64_of("1e+x"), Some((1.0, 1)));
    assert_eq!(f64_of("0x"), Some((0.0, 1)));
    assert_eq!(f64_of("infinit").map(|r| r.1), Some(3));
    assert_eq!(f64_of("nan(12").map(|r| r.1), Some(3));
    assert_eq!(f64_of("nan(1_a)z").map(|r| r.1), Some(8));
    assert!(f64_of("-").is_none() && f64_of(".").is_none() && f64_of("e5").is_none());
    assert!(scan_f64(&mut Stream(b"1e+x", 0), usize::MAX).is_none());
    assert!(scan_f64(&mut Stream(b"0x", 0), usize::MAX).is_none());
    assert_eq!(scan_f64(&mut Stream(b"12.5e1 ", 0), usize::MAX).map(|r| r.value), Some(125.0));
    // A width of 3 sees only "1.5".
    assert_eq!(scan_f64(&mut Stream(b"1.5e3", 0), 3).map(|r| r.value), Some(1.5));
}

#[test]
fn range_errors_and_floats() {
    let big = f64_of("1e999").unwrap();
    assert!(big.0.is_infinite());
    let c = std::ffi::CString::new("1e-999").unwrap();
    let r = scan_f64(&mut unsafe { CStrChars::new(c.as_ptr() as *const u8) }, usize::MAX).unwrap();
    assert!(r.value == 0.0 && r.range_error);
    let c = std::ffi::CString::new("3.4028235e38").unwrap();
    let (v, range) = scan_f32(&mut unsafe { CStrChars::new(c.as_ptr() as *const u8) }, usize::MAX).unwrap();
    assert!(v == f32::MAX && !range);
    let c = std::ffi::CString::new("0.1").unwrap();
    let (v, _) = scan_f32(&mut unsafe { CStrChars::new(c.as_ptr() as *const u8) }, usize::MAX).unwrap();
    assert_eq!(v, 0.1f32);
}

#[test]
fn integer_subjects() {
    let int = |s: &[u8], base, width| {
        let mut st = Stream(s, 0);
        scan_int(&mut st, base, width).map(|r| (r.magnitude, r.negative, r.overflow, r.digits, st.1))
    };
    assert_eq!(int(b"0x1A", 0, usize::MAX), Ok((26, false, false, true, 4)));
    assert_eq!(int(b"017", 0, usize::MAX), Ok((15, false, false, true, 3)));
    assert_eq!(int(b"-z", 36, usize::MAX), Ok((35, true, false, true, 2)));
    assert_eq!(int(b"99999999999999999999", 10, usize::MAX).map(|r| (r.0, r.2)), Ok((u64::MAX, true)));
    assert_eq!(int(b"12345", 10, 3), Ok((123, false, false, true, 3)));
    assert_eq!(int(b"1", 1, usize::MAX).err(), Some(22));
    // "0x" then a non-hex byte: a stream cannot give back the 'x'.
    assert_eq!(int(b"0xg", 16, usize::MAX).map(|r| r.3), Ok(false));
}

#[test]
fn rounding_directions_reach_the_decimal_converter() {
    let third = |mode| {
        math::fenv::fesetround(mode);
        let v = f64_of("0.333333333333333333333333").unwrap().0;
        math::fenv::fesetround(math::fenv::FE_TONEAREST);
        v
    };
    assert!(third(math::fenv::FE_UPWARD) > third(math::fenv::FE_DOWNWARD));
    assert_eq!(third(math::fenv::FE_TOWARDZERO), third(math::fenv::FE_DOWNWARD));
}

fn ulps(got: f64, want: f64) -> u64 {
    (got.to_bits() as i64 - want.to_bits() as i64).unsigned_abs()
}

#[test]
fn exact_math_results() {
    use math::{basic::*, explog::*, special::*};
    assert_eq!(exp(0.0), 1.0);
    assert_eq!(exp2(10.0), 1024.0);
    assert_eq!(pow(2.0, 10.0), 1024.0);
    assert_eq!(pow(4.0, 0.5), 2.0);
    assert_eq!(pow(-2.0, 3.0), -8.0);
    assert_eq!(log2(1024.0), 10.0);
    assert_eq!(log10(1000.0), 3.0);
    assert_eq!(cbrt(-27.0), -3.0);
    assert_eq!(hypot(3.0, 4.0), 5.0);
    assert_eq!(tgamma(5.0), 24.0);
    assert_eq!(fmod(5.5, 2.0), 1.5);
    // Exact remainders, computed with rational arithmetic.
    assert_eq!(fmod(1e300, 7.0), 1.0);
    assert_eq!(fmod(1e22, 0.1), 0.08768742176060307);
    assert_eq!(fmod(-1e308, 1.5e-300), -3.6117710462410844e-301);
    assert_eq!(remainder(7.0, 2.0), -1.0);
    assert_eq!(fma(1.0 + f64::EPSILON, 1.0 - f64::EPSILON, -1.0), -f64::EPSILON * f64::EPSILON);
    assert_eq!(scale(1.0, -1074).to_bits(), 1);
    assert_eq!(unsafe { lgamma(1.0) }, 0.0);
}

#[test]
fn transcendental_values_within_an_ulp() {
    use math::{explog::*, special::*, trig::*};
    // Values of the functions at these arguments, correctly rounded.
    let cases: [(f64, f64); 12] = [
        (exp(1.0), std::f64::consts::E),
        (log(10.0), std::f64::consts::LN_10),
        // The doubles just below pi/6 and pi/3.
        (sin(0.5235987755982988), 0.49999999999999994),
        (cos(1.0471975511965976), 0.5000000000000001),
        // The doubles nearest pi/6 and pi/3, which lie above them.
        (sin(std::f64::consts::FRAC_PI_6), 0.5),
        (cos(std::f64::consts::FRAC_PI_3), 0.4999999999999999),
        (atan(1.0), std::f64::consts::FRAC_PI_4),
        (sin(1e22), -0.8522008497671888),
        (tanh(1.0), 0.7615941559557649),
        (erf(1.0), 0.8427007929497149),
        (tgamma(0.5), 1.772453850905516),
        (asin(1.0), std::f64::consts::FRAC_PI_2),
    ];
    for (i, (got, want)) in cases.iter().enumerate() {
        assert!(ulps(*got, *want) <= 1, "case {i}: {got:e} vs {want:e}");
    }
}

#[test]
fn sort_matches_a_reference() {
    unsafe extern "C" fn by_u32(a: *const core::ffi::c_void, b: *const core::ffi::c_void) -> i32 {
        let (x, y) = (*(a as *const u32), *(b as *const u32));
        (x > y) as i32 - (x < y) as i32
    }
    let mut seed = 7u32;
    for n in [0, 1, 2, 17, 500, 4096] {
        let mut v: Vec<u32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                seed >> 20
            })
            .collect();
        let mut want = v.clone();
        want.sort();
        unsafe { stdlib::sort::qsort(v.as_mut_ptr() as *mut core::ffi::c_void, n, 4, by_u32) };
        assert_eq!(v, want);
    }
}

#[test]
fn review_math_extremes_and_flags() {
    use math::{basic::*, explog::*, special::*, trig::*, fenv::*};
    let nearest = FE_TONEAREST;
    fesetround(nearest);
    assert!(ulps(log(1e308), 709.1962086421661) <= 1);
    pthread::set_errno(0);
    assert!(atan(1e308).is_finite());
    assert_eq!(pthread::errno(), 0);
    assert!(atan2(1.0, 1e308) > 0.0);
    assert_eq!(pow(2.0, 1e308), f64::INFINITY);
    for f in [atan, asin, log1p, asinh] { assert_eq!(f(f64::from_bits(1)).to_bits(), 1); }
    feclearexcept(FE_ALL_EXCEPT);
    pthread::set_errno(0);
    assert_eq!(fma(f64::MAX, 2.0, f64::NEG_INFINITY), f64::NEG_INFINITY);
    assert_eq!(fetestexcept(FE_INVALID), 0);
    assert_eq!(pthread::errno(), 0);
    assert_eq!(fmaf(f32::MAX, 2.0, f32::NEG_INFINITY), f32::NEG_INFINITY);
    feclearexcept(FE_ALL_EXCEPT);
    assert_eq!(round(f64::INFINITY), f64::INFINITY);
    assert_eq!(roundf(f32::NEG_INFINITY), f32::NEG_INFINITY);
    assert_eq!(fetestexcept(FE_INVALID), 0);
    pthread::set_errno(0);
    assert_eq!(fdim(f64::INFINITY, 0.0), f64::INFINITY);
    assert_eq!(pthread::errno(), 0);
    for f in [lrint, llrint, lround, llround] {
        pthread::set_errno(0);
        feclearexcept(FE_ALL_EXCEPT);
        f(f64::INFINITY);
        assert_eq!(pthread::errno(), 33);
        assert_ne!(fetestexcept(FE_INVALID), 0);
    }
    for mode in [FE_UPWARD, FE_DOWNWARD, FE_TOWARDZERO, nearest] {
        fesetround(mode);
        assert_eq!(tanh(f64::INFINITY), 1.0);
        assert_eq!(erf(f64::NEG_INFINITY), -1.0);
        assert_eq!(expm1(f64::NEG_INFINITY), -1.0);
        assert_eq!(pow(-2.0, -1075.0).to_bits(), if mode == FE_DOWNWARD { (1u64<<63)|1 } else { 1u64<<63 });
    }
    fesetround(FE_DOWNWARD);
    pthread::set_errno(0);
    assert_eq!(fdim(f64::MAX, -f64::MAX), f64::MAX);
    assert_eq!(pthread::errno(), 34);
    pthread::set_errno(0);
    assert_eq!(math::narrow(exp(90.0)), f32::MAX);
    assert_eq!(pthread::errno(), 34);
    fesetround(FE_UPWARD);
    pthread::set_errno(0);
    assert_eq!(hypot(f64::MAX, 1.0), f64::INFINITY);
    assert_eq!(pthread::errno(), 34);
    fesetround(nearest);
}

#[test]
fn conversions_raise_flags() {
    use math::fenv::*;
    feclearexcept(FE_ALL_EXCEPT);
    f64_of("0.1").unwrap();
    assert_ne!(fetestexcept(FE_INEXACT), 0);
    feclearexcept(FE_ALL_EXCEPT);
    f64_of("1e999").unwrap();
    assert_eq!(fetestexcept(FE_OVERFLOW | FE_INEXACT), FE_OVERFLOW | FE_INEXACT);
    feclearexcept(FE_ALL_EXCEPT);
    f64_of("1e-999").unwrap();
    assert_eq!(fetestexcept(FE_UNDERFLOW | FE_INEXACT), FE_UNDERFLOW | FE_INEXACT);
}

#[test]
fn sort_transfers_uninitialized_padding() {
    #[repr(C)]
    struct Padded { key: u8, value: u64 }
    unsafe extern "C" fn cmp(a: *const core::ffi::c_void, b: *const core::ffi::c_void) -> i32 {
        let a = &*(a as *const Padded);
        let b = &*(b as *const Padded);
        (a.key > b.key) as i32 - (a.key < b.key) as i32
    }
    let mut objects = [Padded { key: 2, value: 20 }, Padded { key: 1, value: 10 }];
    unsafe { stdlib::sort::qsort(objects.as_mut_ptr().cast(), 2, std::mem::size_of::<Padded>(), cmp); }
    assert_eq!((objects[0].key, objects[0].value), (1, 10));
    assert_eq!((objects[1].key, objects[1].value), (2, 20));
}

#[test]
fn long_double_subnormal_midpoint_uses_all_deciding_digits() {
    #[cfg(target_arch = "aarch64")]
    let (fmt, k) = (&stdlib::floatconv::F128, 16495usize);
    #[cfg(target_arch = "x86_64")]
    let (fmt, k) = (&stdlib::floatconv::X87, 16446usize);
    // 2^-k = 5^k * 10^-k, exactly the midpoint between zero and the
    // smallest subnormal. Generate its decimal digits independently.
    let mut digits = vec![1u8];
    for _ in 0..k {
        let mut carry = 0;
        for d in &mut digits {
            let v = *d * 5 + carry;
            *d = v % 10;
            carry = v / 10;
        }
        if carry != 0 { digits.push(carry); }
    }
    let scan = |d: &[u8]| {
        let mut text: Vec<u8> = d.iter().rev().map(|x| b'0' + x).collect();
        text.extend_from_slice(format!("e-{k}").as_bytes());
        text.push(0);
        let mut chars = unsafe { CStrChars::new(text.as_ptr()) };
        stdlib::numscan::scan_float(&mut chars, usize::MAX, fmt).unwrap().bits
    };
    math::fenv::fesetround(math::fenv::FE_TONEAREST);
    assert_eq!(scan(&digits), 0);
    digits[0] += 1;
    assert_eq!(scan(&digits), 1);
    digits[0] -= 2;
    assert_eq!(scan(&digits), 0);
}

#[test]
fn split_products_keep_the_exact_residual() {
    let mut bits = 0x12345678abcdef01u64;
    for _ in 0..10000 {
        bits = bits.wrapping_mul((1u64<<32)+1).wrapping_add(1);
        let a = f64::from_bits((bits & ((1u64<<52)-1)) | (1023u64<<52));
        bits = bits.rotate_left(27).wrapping_add(7);
        let b = f64::from_bits((bits & ((1u64<<52)-1)) | (1023u64<<52));
        let got = math::dd::two_prod(a, b);
        let residual = math::basic::fma(a, b, -got.hi);
        assert_eq!(got.lo, residual, "a={a:?} b={b:?}");
    }
}

#[test]
fn ieee_maximum_minimum_and_roundeven() {
    use math::{basic::*, fenv::*};
    let nan = f64::NAN;
    assert!(fmaximum(1.0, nan).is_nan() && fmaximum(nan, 1.0).is_nan());
    assert!(fminimum(1.0, nan).is_nan() && fminimum(nan, -1.0).is_nan());
    assert!(fmaximumf(f32::NAN, 2.0).is_nan() && fminimumf(2.0, f32::NAN).is_nan());
    assert_eq!(fmaximum(-0.0, 0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(fmaximum(0.0, -0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(fminimum(0.0, -0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(fminimum(-0.0, 0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(fmaximumf(-0.0, 0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(fminimumf(0.0, -0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(fmaximum(f64::NEG_INFINITY, -f64::MAX), -f64::MAX);
    assert_eq!(fminimum(3.0, f64::NEG_INFINITY), f64::NEG_INFINITY);
    assert_eq!(fmaximumf(1.25, 0.5), 1.25);
    assert_eq!(fminimumf(2.5, -3.5), -3.5);

    let mut values = vec![0.0, -0.0, 0.25, 0.5, 0.75, 1.5, 2.5, 3.5, -2.5, 4503599627370495.5,
                          4503599627370497.0, 1e300, f64::MIN_POSITIVE, f64::from_bits(1),
                          f64::INFINITY, f64::NEG_INFINITY];
    for i in -40..=40 {
        values.push(i as f64 * 0.25);
        values.push(i as f64 * 0.5 + 0.5);
    }
    for x in values {
        let xf = x as f32;
        feclearexcept(FE_ALL_EXCEPT);
        let (r, rf) = (roundeven(x), roundevenf(xf));
        assert_eq!(fetestexcept(FE_ALL_EXCEPT), 0, "roundeven({x}) raised an exception");
        assert_eq!(r.to_bits(), x.round_ties_even().to_bits(), "roundeven({x})");
        assert_eq!(rf.to_bits(), xf.round_ties_even().to_bits(), "roundevenf({xf})");
    }
    assert!(roundeven(nan).is_nan());
}

#[test]
fn interchange_formats_round_even_and_order_ieee() {
    use math::long_double::{fmaximum, fminimum, fmod, roundeven, sqrt};
    use stdlib::floatconv::{F128, F16};
    // binary16 values through f32, which holds each of them, and their results, exactly.
    let h = |x: f32| -> u128 {
        let b = x.to_bits();
        let (sign, exp, frac) = ((b >> 16) & 0x8000, ((b >> 23) & 0xff) as i32, b & 0x7f_ffff);
        let bits = match exp {
            0xff => sign | 0x7c00 | (frac >> 13),
            0 => sign,
            _ if exp - 127 < -14 => sign | ((frac | 0x80_0000) >> (126 - exp - 14 + 13 + 1)),
            _ => {
                // Round the 24-bit significand to 11 bits, halfway cases to even.
                let m = frac | 0x80_0000;
                let (r, rem) = (m >> 13, m & 0x1fff);
                let r = r + (rem > 0x1000 || (rem == 0x1000 && r & 1 == 1)) as u32;
                sign | ((((exp - 127 + 15) as u32) << 10) + (r - 0x400))
            }
        };
        bits as u128
    };
    let f = |b: u128| -> f32 {
        let b = b as u32;
        let (sign, exp, frac) = ((b & 0x8000) << 16, (b >> 10) & 0x1f, b & 0x3ff);
        let mag = match exp {
            0x1f => f32::from_bits(0x7f80_0000 | (frac << 13)),
            0 => frac as f32 * 2f32.powi(-24),
            _ => f32::from_bits(((exp + 127 - 15) << 23) | (frac << 13)),
        };
        f32::from_bits(mag.to_bits() | sign)
    };
    for x in [0.5f32, 1.5, 2.5, 3.5, -2.5, -0.5, 1023.5, 2047.0, 0.25, 6.0e-8, 65504.0] {
        assert_eq!(f(roundeven(h(x), &F16)).to_bits(), x.round_ties_even().to_bits(), "roundevenf16({x})");
    }
    for x in [2.0f32, 0.25, 1024.0, 3.0, 65504.0] {
        assert_eq!(f(sqrt(h(x), &F16)), f(h(x.sqrt())), "sqrtf16({x})");
    }
    assert_eq!(f(fmod(h(7.5), h(2.0), &F16)), 1.5);
    assert_eq!(fmaximum(h(-0.0), h(0.0), &F16), h(0.0));
    assert_eq!(fminimum(h(0.0), h(-0.0), &F16), h(-0.0));
    assert!(f(fmaximum(h(f32::NAN), h(1.0), &F16)).is_nan());
    assert!(f(fminimum(h(1.0), h(f32::NAN), &F16)).is_nan());
    assert_eq!(f(fmaximum(h(1.5), h(-3.0), &F16)), 1.5);
    assert_eq!(f(fminimum(h(1.5), h(-3.0), &F16)), -3.0);

    // binary128: exponent bias 16383, 112 fraction bits.
    let q = |neg: bool, e: i64, frac: u128| ((neg as u128) << 127) | (((e + 16383) as u128) << 112) | frac;
    let half = 1u128 << 111;
    let (one, two, three) = (q(false, 0, 0), q(false, 1, 0), q(false, 1, half));
    assert_eq!(roundeven(q(false, 1, 1u128 << 110), &F128), two); // 2.5 -> 2
    assert_eq!(roundeven(q(false, 1, (1u128 << 111) | (1u128 << 110)), &F128), q(false, 2, 0)); // 3.5 -> 4
    assert_eq!(roundeven(q(true, -1, 0), &F128), 1u128 << 127); // -0.5 -> -0
    assert_eq!(roundeven(q(false, 0, half), &F128), two); // 1.5 -> 2
    assert_eq!(fmaximum(one, three, &F128), three);
    assert_eq!(fminimum(one, three, &F128), one);
    assert_eq!(fmaximum(1u128 << 127, 0, &F128), 0);
    assert_eq!(fminimum(0, 1u128 << 127, &F128), 1u128 << 127);
    let nan = q(false, 16384, half);
    assert_eq!(fmaximum(one, nan, &F128) >> 111, nan >> 111);
    assert_eq!(fminimum(nan, one, &F128) >> 111, nan >> 111);
}
