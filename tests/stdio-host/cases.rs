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
#[test]
fn failed_memstream_growth_preserves_buffer() {
    unsafe {
        let mut p = ptr::null_mut();
        let mut n = 0;
        let f = open_memstream(&mut p, &mut n);
        assert!(!f.is_null());
        assert_eq!(fputs(c"kept".as_ptr() as *const u8, f), 0);
        assert_eq!(fflush(f), 0);
        let original = p;
        assert_eq!(fseek(f, 10000, 0), 0);
        super::FAIL_REALLOC.with(|s| s.set(true));
        assert_eq!(fputc(b'x' as i32, f), -1);
        assert_eq!(super::pthread::errno(), 12);
        assert_ne!(ferror(f), 0);
        assert_eq!(fflush(f), 0);
        assert_eq!(p, original);
        assert_eq!(std::slice::from_raw_parts(p, 5), b"kept\0");
        assert_eq!(fclose(f), 0);
        super::free(p);
    }
}
#[test]
fn flush_and_close_registry_concurrency() {
    let threads: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| unsafe {
                for _ in 0..20 {
                    let mut p = ptr::null_mut();
                    let mut n = 0;
                    let f = open_memstream(&mut p, &mut n);
                    assert!(!f.is_null());
                    flockfile(f);
                    assert_eq!(fputs(c"one".as_ptr() as *const u8, f), 0);
                    assert_eq!(fputs(c"two".as_ptr() as *const u8, f), 0);
                    funlockfile(f);
                    assert_eq!(fflush(ptr::null_mut()), 0);
                    assert_eq!(fclose(f), 0);
                    assert_eq!(n, 6);
                    assert_eq!(std::slice::from_raw_parts(p, 7), b"onetwo\0");
                    super::free(p);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}
#[test]
fn full_line_and_unbuffered_visibility() {
    unsafe {
        extern "C" {
            fn pipe(fds: *mut i32) -> i32;
        }
        for mode in 0..3 {
            let mut fds = [0; 2];
            assert_eq!(pipe(fds.as_mut_ptr()), 0);
            let f = fdopen(fds[1], c"w".as_ptr() as *const u8);
            assert!(!f.is_null());
            assert_eq!(super::host_fcntl(fds[0], 4, 4i32), 0); // macOS O_NONBLOCK.
            assert_eq!(setvbuf(f, ptr::null_mut(), mode, 64), 0);
            assert_eq!(fputs(c"ab".as_ptr() as *const u8, f), 0);
            let mut b = [0; 3];
            if mode == 2 {
                assert_eq!(super::read(fds[0], b.as_mut_ptr(), 3), 2);
                assert_eq!(&b[..2], b"ab");
            } else {
                assert_eq!(super::read(fds[0], b.as_mut_ptr(), 3), -1);
            }
            assert_eq!(fputc(10, f), 10);
            if mode == 0 {
                assert_eq!(super::read(fds[0], b.as_mut_ptr(), 3), -1);
                assert_eq!(fflush(f), 0);
            }
            let expected = if mode == 2 {
                b"\n".as_slice()
            } else {
                b"ab\n".as_slice()
            };
            assert_eq!(
                super::read(fds[0], b.as_mut_ptr(), 3),
                expected.len() as isize
            );
            assert_eq!(&b[..expected.len()], expected);
            assert_eq!(fclose(f), 0);
            super::close(fds[0]);
        }
    }
}
#[test]
fn failed_memstream_construction_is_unpublished() {
    unsafe {
        let mut p = ptr::null_mut();
        let mut n = 99;
        super::FAIL_REALLOC.with(|s| s.set(true));
        assert!(open_memstream(&mut p, &mut n).is_null());
        assert_eq!(super::pthread::errno(), 12);
        assert_eq!(fflush(ptr::null_mut()), 0);
        assert_eq!(n, 99);
        assert!(p.is_null());
    }
}
#[test]
fn prior_error_does_not_discard_a_successful_fgets() {
    unsafe {
        let mut bytes = *b"abc\n";
        let f = fmemopen(bytes.as_mut_ptr(), 4, c"r".as_ptr() as *const u8);
        assert_eq!(fputc(1, f), -1);
        assert_ne!(ferror(f), 0);
        let mut b = [0; 5];
        assert_eq!(fgets(b.as_mut_ptr(), 5, f), b.as_mut_ptr());
        assert_eq!(&b, b"abc\n\0");
        assert_ne!(ferror(f), 0);
        assert_eq!(fclose(f), 0);
    }
}
#[test]
fn getline_ignores_capacity_when_pointer_is_null() {
    unsafe {
        let mut bytes = *b"long line\nx";
        let f = fmemopen(bytes.as_mut_ptr(), bytes.len(), c"r".as_ptr() as *const u8);
        let mut p = ptr::null_mut();
        let mut capacity = usize::MAX;
        assert_eq!(getline(&mut p, &mut capacity, f), 10);
        assert!(capacity > 10);
        assert_eq!(std::slice::from_raw_parts(p, 11), b"long line\n\0");
        assert_eq!(getline(&mut p, &mut capacity, f), 1);
        assert_eq!(std::slice::from_raw_parts(p, 2), b"x\0");
        assert_eq!(getline(&mut p, &mut capacity, f), -1);
        assert_ne!(feof(f), 0);
        fclose(f);
        super::free(p);
    }
}
#[test]
fn large_general_precision_and_overflowing_width() {
    unsafe {
        let (s, n) = printed(c"%.2147483647g", |p, n, f| snprintf(p, n, f, 1.25f64));
        assert_eq!(s, "1.25");
        assert_eq!(n, 4);
        let mut b = [0u8; 16];
        assert_eq!(
            snprintf(
                b.as_mut_ptr(),
                b.len(),
                c"%*d".as_ptr() as *const u8,
                i32::MIN,
                1i32
            ),
            -1
        );
        assert_eq!(super::pthread::errno(), 75);
    }
}
#[test]
fn printf_honors_all_rounding_directions() {
    unsafe {
        struct RoundGuard(u64);
        impl Drop for RoundGuard {
            fn drop(&mut self) {
                unsafe {
                    core::arch::asm!("msr fpcr, {}",in(reg)self.0,options(nomem,nostack));
                }
            }
        }
        let saved: u64;
        core::arch::asm!("mrs {}, fpcr",out(reg)saved,options(nomem,nostack));
        let guard = RoundGuard(saved);
        for (mode, want) in [
            (0, "1.2|-1.2"),
            (1, "1.3|-1.2"),
            (2, "1.2|-1.3"),
            (3, "1.2|-1.2"),
        ] {
            let setting = (saved & !(3u64 << 22)) | (mode << 22);
            core::arch::asm!("msr fpcr, {}",in(reg)setting,options(nomem,nostack));
            let (s, _) = printed(c"%.1f|%.1f", |p, n, f| snprintf(p, n, f, 1.25f64, -1.25f64));
            assert_eq!(s, want);
        }
        drop(guard);
    }
}
