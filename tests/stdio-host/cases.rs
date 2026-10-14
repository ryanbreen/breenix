use super::stdio::{file::*, format::*};
use std::{
    ffi::{CStr, CString},
    ptr,
};
unsafe fn printed(
    fmt: &CStr,
    args: impl FnOnce(*mut u8, usize, *const u8) -> i32,
) -> (String, i32) {
    let mut b = [0u8; 2048];
    let n = args(b.as_mut_ptr(), b.len(), fmt.as_ptr() as *const u8);
    (
        CStr::from_ptr(b.as_ptr() as *const i8)
            .to_str()
            .unwrap()
            .into(),
        n,
    )
}
#[test]
fn format_integer_extrema() {
    unsafe {
        let (s, n) = printed(c"%lld|%llu|%#o|%#.0o|%.0d|%hhd|%zu", |p, n, f| {
            snprintf(
                p,
                n,
                f,
                i64::MIN,
                u64::MAX,
                8u32,
                0u32,
                0i32,
                300i32,
                7usize,
            )
        });
        assert_eq!(s, "-9223372036854775808|18446744073709551615|010|0||44|7");
        assert_eq!(n, s.len() as i32);
    }
}
#[test]
fn format_fields_and_counts() {
    unsafe {
        let mut count = 0i32;
        let (s, n) = printed(c"%+08.3d|%-6s|%*.*f%n", |p, n, f| {
            snprintf(
                p,
                n,
                f,
                7i32,
                c"ab".as_ptr(),
                10i32,
                3i32,
                -1.5f64,
                &mut count,
            )
        });
        assert_eq!(s, "    +007|ab    |    -1.500");
        assert_eq!(n, count);
        assert_eq!(n, s.len() as i32);
        let mut b = [b'z'; 5];
        assert_eq!(
            snprintf(b.as_mut_ptr(), 5, c"abcdefgh".as_ptr() as *const u8),
            8
        );
        assert_eq!(&b, b"abcd\0");
        assert_eq!(
            snprintf(ptr::null_mut(), 0, c"%d".as_ptr() as *const u8, 12345i32),
            5
        );
    }
}
#[test]
fn format_exact_decimal_rounding() {
    unsafe {
        let (s, _) = printed(c"%.2f|%e|%g|%G|%.0f|%#.0f|%10.3f|%g", |p, n, f| {
            snprintf(
                p,
                n,
                f,
                3.14159f64,
                12345.678f64,
                0.0001f64,
                1e20f64,
                2.5f64,
                2.0f64,
                -1.5f64,
                100000f64,
            )
        });
        assert_eq!(s, "3.14|1.234568e+04|0.0001|1E+20|2|2.|    -1.500|100000");
        let (s, _) = printed(c"%.0f|%.0f|%.2f|%.2e|%.3g|%a", |p, n, f| {
            snprintf(
                p, n, f, 3.5f64, -2.5f64, 0.125f64, 9.999f64, 999.9f64, 1.5f64,
            )
        });
        assert_eq!(s, "4|-2|0.12|1.00e+01|1e+03|0x1.8p+0");
        let (s, _) = printed(c"%.17g|%.17g", |p, n, f| {
            snprintf(p, n, f, f64::from_bits(1), f64::MAX)
        });
        assert_eq!(s, "4.9406564584124654e-324|1.7976931348623157e+308");
    }
}
#[test]
fn format_binary64_round_trips() {
    unsafe {
        let mut bits = 1234567u64;
        for _ in 0..2000 {
            bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
            let v = f64::from_bits(bits);
            if !v.is_finite() {
                continue;
            }
            let (s, _) = printed(c"%.17g", |p, n, f| snprintf(p, n, f, v));
            let round = s.parse::<f64>().unwrap();
            assert_eq!(v.to_bits(), round.to_bits(), "{s}");
        }
    }
}
#[test]
fn stream_positions_pushback_and_update() {
    unsafe {
        let path = CString::new(format!("/tmp/breenix-stdio-host-{}", super::getpid())).unwrap();
        let f = fopen(path.as_ptr() as *const u8, c"w+".as_ptr() as *const u8);
        assert!(!f.is_null());
        assert_eq!(fwrite(b"abcdef".as_ptr(), 2, 3, f), 3);
        assert_eq!(ftell(f), 6);
        assert_eq!(fseek(f, 0, 0), 0);
        assert_eq!(fgetc(f), b'a' as i32);
        assert_eq!(ftell(f), 1);
        assert_eq!(ungetc(b'z' as i32, f), b'z' as i32);
        assert_eq!(ftell(f), 0);
        assert_eq!(fgetc(f), b'z' as i32);
        assert_eq!(fseek(f, 0, 1), 0);
        assert_eq!(fputc(b'X' as i32, f), b'X' as i32);
        rewind(f);
        let mut b = [0; 8];
        assert_eq!(fread(b.as_mut_ptr(), 1, 8, f), 6);
        assert_eq!(&b[..6], b"aXcdef");
        assert_ne!(feof(f), 0);
        assert_eq!(ferror(f), 0);
        assert_eq!(ungetc(b'q' as i32, f), b'q' as i32);
        assert_eq!(feof(f), 0);
        assert_eq!(fgetc(f), b'q' as i32);
        assert_eq!(fclose(f), 0);
        super::unlink(path.as_ptr() as *const u8);
    }
}
#[test]
fn memstream_seek_gaps_and_close_ownership() {
    unsafe {
        let mut p = ptr::null_mut();
        let mut n = 0;
        let f = open_memstream(&mut p, &mut n);
        assert!(!f.is_null());
        assert_eq!(fputs(c"abc".as_ptr() as *const u8, f), 0);
        assert_eq!(fseek(f, 7, 0), 0);
        assert_eq!(fputc(b'x' as i32, f), b'x' as i32);
        assert_eq!(fflush(f), 0);
        assert_eq!(n, 8);
        assert_eq!(std::slice::from_raw_parts(p, 9), b"abc\0\0\0\0x\0");
        assert_eq!(fseek(f, 2, 0), 0);
        assert_eq!(fflush(f), 0);
        assert_eq!(n, 2);
        assert_eq!(fclose(f), 0);
        assert_eq!(std::slice::from_raw_parts(p, 9), b"abc\0\0\0\0x\0");
        super::free(p);
    }
}
#[test]
fn fixed_memory_preserves_full_buffer() {
    unsafe {
        let mut b = *b"1234";
        let f = fmemopen(b.as_mut_ptr(), 4, c"w+".as_ptr() as *const u8);
        assert_eq!(fwrite(b"abcd".as_ptr(), 1, 4, f), 4);
        assert_eq!(fflush(f), 0);
        assert_eq!(b, *b"abcd");
        assert_eq!(fputc(88, f), -1);
        assert_ne!(ferror(f), 0);
        rewind(f);
        let mut out = [0; 4];
        assert_eq!(fread(out.as_mut_ptr(), 1, 4, f), 4);
        assert_eq!(out, *b"abcd");
        fclose(f);
    }
}
#[test]
fn positional_arguments_and_long_double() {
    unsafe {
        let (s, n) = printed(c"%3$*1$.*2$f|%4$lld|%4$lld", |p, n, f| {
            snprintf(p, n, f, 10i32, 3i32, -1.5f64, 123i64)
        });
        assert_eq!(s, "    -1.500|123|123");
        assert_eq!(n, s.len() as i32);
        let (s, _) = printed(c"%.18Lf|%La", |p, n, f| snprintf(p, n, f, 0.125f64, 1.5f64));
        assert_eq!(s, "0.125000000000000000|0x1.8p+0");
    }
}
struct WideOutput(Vec<u32>);
impl super::stdio::Sink for WideOutput {
    fn bytes(&mut self, b: &[u8]) -> bool {
        self.0.extend(b.iter().map(|&c| c as u32));
        true
    }
    fn wide(&mut self, c: u32) -> bool {
        self.0.push(c);
        true
    }
}
unsafe extern "C" fn wide_format(output: *mut WideOutput, fmt: *const u32, mut args: ...) -> i32 {
    match super::stdio::vformat(
        &mut *output,
        super::stdio::Format::Wide(fmt),
        args.as_va_list(),
    ) {
        Ok(()) => (*output).0.len() as i32,
        Err(e) => -e,
    }
}
#[test]
fn wide_printf_preserves_characters_in_c_locale() {
    unsafe {
        let mut out = WideOutput(Vec::new());
        let fmt = [37, 108, 115, 45, 37, 108, 99, 0];
        let text = [0xe9u32, 0];
        assert_eq!(
            wide_format(&mut out, fmt.as_ptr(), text.as_ptr(), 0x20acu32),
            3
        );
        assert_eq!(out.0, vec![0xe9, 45, 0x20ac]);
    }
}
