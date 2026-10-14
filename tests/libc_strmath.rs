// Run with rustc --test: this exercises only STRMATH's number scanners, conversion,
// sorting and math on the host (aarch64 or x86-64 with SSE2), with errno kept here.

mod pthread {
    use std::cell::Cell;
    thread_local!(static ERRNO: Cell<i32> = const { Cell::new(0) });
    pub fn set_errno(e: i32) {
        ERRNO.with(|c| c.set(e));
    }
    #[allow(dead_code)] // stdlib's errno re-export; these tests read errno through set_errno's effects
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
