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
use super::stdio::scan::*;
#[test]
fn scanf_integer_widths_and_scansets() {
    unsafe {
        let mut decimal = 0i32;
        let mut octal = 0u32;
        let mut hex = 0u32;
        let mut neg = 0i64;
        let mut tiny = 0u8;
        assert_eq!(
            sscanf(
                c"42 017 0x1f -9223372036854775808 257".as_ptr() as *const u8,
                c"%d %o %x %lld %hhu".as_ptr() as *const u8,
                &mut decimal,
                &mut octal,
                &mut hex,
                &mut neg,
                &mut tiny
            ),
            5
        );
        assert_eq!((decimal, octal, hex, neg, tiny), (42, 15, 31, i64::MIN, 1));
        let mut a = [0u8; 8];
        let mut b = [0u8; 8];
        let mut d = 0i32;
        assert_eq!(
            sscanf(
                c"abcdef123".as_ptr() as *const u8,
                c"%3s%7[a-z]%2d".as_ptr() as *const u8,
                a.as_mut_ptr(),
                b.as_mut_ptr(),
                &mut d
            ),
            3
        );
        assert_eq!(&a[..4], b"abc\0");
        assert_eq!(&b[..4], b"def\0");
        assert_eq!(d, 12);
        assert_eq!(
            sscanf(
                c"]-rest".as_ptr() as *const u8,
                c"%7[]-]".as_ptr() as *const u8,
                a.as_mut_ptr()
            ),
            1
        );
        assert_eq!(&a[..3], b"]-\0");
    }
}
#[test]
fn scanf_assignment_counts_and_failures() {
    unsafe {
        let mut v = 0i32;
        let mut n = 0i32;
        assert_eq!(
            sscanf(
                c"".as_ptr() as *const u8,
                c"%d".as_ptr() as *const u8,
                &mut v
            ),
            -1
        );
        assert_eq!(
            sscanf(
                c"x".as_ptr() as *const u8,
                c"%d".as_ptr() as *const u8,
                &mut v
            ),
            0
        );
        assert_eq!(
            sscanf(
                c"12 34".as_ptr() as *const u8,
                c"%*d %d%n".as_ptr() as *const u8,
                &mut v,
                &mut n
            ),
            1
        );
        assert_eq!((v, n), (34, 5));
        let mut other = 0i32;
        assert_eq!(
            sscanf(
                c"12 34".as_ptr() as *const u8,
                c"%2$d %1$d".as_ptr() as *const u8,
                &mut other,
                &mut v
            ),
            2
        );
        assert_eq!((other, v), (34, 12));
        assert_eq!(
            sscanf(
                c"".as_ptr() as *const u8,
                c"%n%d".as_ptr() as *const u8,
                &mut n,
                &mut v
            ),
            0
        );
        assert_eq!(n, 0);
    }
}
#[test]
fn scanf_float_rounds_once_and_rejects_partial_forms() {
    unsafe {
        let mut x = 0f64;
        let mut y = 0f32;
        let mut z = 0f64;
        assert_eq!(
            sscanf(
                c"3.5e2 -0.25 inf".as_ptr() as *const u8,
                c"%lf %f %lf".as_ptr() as *const u8,
                &mut x,
                &mut y,
                &mut z
            ),
            3
        );
        assert_eq!(x, 350.0);
        assert_eq!(y, -0.25);
        assert_eq!(z, f64::INFINITY);
        assert_eq!(
            sscanf(
                c"1.00000005960464477539062500001".as_ptr() as *const u8,
                c"%f".as_ptr() as *const u8,
                &mut y
            ),
            1
        );
        assert_eq!(y.to_bits(), 1f32.to_bits() + 1);
        assert_eq!(
            sscanf(
                c"1e+".as_ptr() as *const u8,
                c"%lf".as_ptr() as *const u8,
                &mut x
            ),
            0
        );
        assert_eq!(
            sscanf(
                c"0x".as_ptr() as *const u8,
                c"%x".as_ptr() as *const u8,
                &mut v_for_hex()
            ),
            0
        );
    }
}
fn v_for_hex() -> u32 {
    0
}
#[test]
fn fscanf_leaves_first_unmatched_character_unread() {
    unsafe {
        let mut bytes = *b"123x";
        let f = fmemopen(bytes.as_mut_ptr(), 4, c"r".as_ptr() as *const u8);
        let mut v = 0i32;
        assert_eq!(fscanf(f, c"%d".as_ptr() as *const u8, &mut v), 1);
        assert_eq!(v, 123);
        assert_eq!(fgetc(f), b'x' as i32);
        assert_eq!(fclose(f), 0);
    }
}
#[test]
fn scanf_assignment_allocation() {
    unsafe {
        let mut p = ptr::null_mut::<u8>();
        assert_eq!(
            sscanf(
                c"abc,rest".as_ptr() as *const u8,
                c"%m[^,]".as_ptr() as *const u8,
                &mut p
            ),
            1
        );
        assert_eq!(std::slice::from_raw_parts(p, 4), b"abc\0");
        super::free(p);
    }
}
#[test]
fn popen_reads_writes_and_waits_for_status() {
    unsafe {
        use super::stdio::pipe::*;
        super::env::environ = *super::host_environ();
        let f = popen(
            c"printf 'pipe text'; exit 7".as_ptr() as *const u8,
            c"r".as_ptr() as *const u8,
        );
        assert!(!f.is_null());
        let mut b = [0u8; 32];
        assert_eq!(fgets(b.as_mut_ptr(), 32, f), b.as_mut_ptr());
        assert_eq!(CStr::from_ptr(b.as_ptr() as *const i8), c"pipe text");
        assert_eq!(pclose(f), 7 << 8);
        let path = format!("/tmp/breenix-popen-{}", super::getpid());
        let command = CString::new(format!("cat > {path}")).unwrap();
        let f = popen(command.as_ptr() as *const u8, c"w".as_ptr() as *const u8);
        assert!(!f.is_null());
        assert_eq!(fputs_unlocked(c"written".as_ptr() as *const u8, f), 0);
        assert!(fileno_unlocked(f) >= 0);
        assert_eq!(pclose(f), 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"written");
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn failed_character_scan_does_not_assign() {
    unsafe {
        let mut b = *b"keep";
        assert_eq!(
            sscanf(
                c"x".as_ptr() as *const u8,
                c"%2c".as_ptr() as *const u8,
                b.as_mut_ptr()
            ),
            -1
        );
        assert_eq!(&b, b"keep");
    }
}
#[test]
fn zero_length_io_leaves_stream_state_unchanged() {
    unsafe {
        let mut b = [0u8; 8];
        let f = fmemopen(b.as_mut_ptr(), b.len(), c"w+".as_ptr() as *const u8);
        assert!(!f.is_null());
        assert_eq!(fread(ptr::null_mut(), 1, 0, f), 0);
        assert_eq!(fwrite(ptr::null(), 1, 0, f), 0);
        assert_eq!(super::stdio::file::orient(f, 0), 0);
        assert_eq!(ftell(f), 0);
        assert_eq!(feof(f), 0);
        assert_eq!(ferror(f), 0);
        fclose(f);
    }
}
mod review {
    //! One case per review finding fixed on PR #1535.
    use super::super::stdio::{file::*, format::*, misc::*};
    use super::{printed, wide_format, WideOutput};
    use std::{ffi::CStr, ptr, sync::mpsc, thread, time::Duration};
    extern "C" {
        fn pipe(fds: *mut i32) -> i32;
        fn socketpair(domain: i32, kind: i32, protocol: i32, fds: *mut i32) -> i32;
        fn recv(fd: i32, b: *mut u8, n: usize, flags: i32) -> isize;
        fn mmap(a: *mut u8, n: usize, prot: i32, flags: i32, fd: i32, o: i64) -> *mut u8;
        fn mprotect(a: *mut u8, n: usize, prot: i32) -> i32;
        fn munmap(a: *mut u8, n: usize) -> i32;
        fn getpagesize() -> i32;
        fn dup(fd: i32) -> i32;
    }
    const F_SETFL: i32 = 4; // macOS
    const O_NONBLOCK: i32 = 4; // macOS
    const MSG_DONTWAIT: i32 = 0x80; // macOS
    const FULL: i32 = 0;
    const LINE: i32 = 1;
    const NONE: i32 = 2;
    fn c(s: &CStr) -> *const u8 {
        s.as_ptr() as *const u8
    }
    unsafe fn nonblocking_pipe() -> [i32; 2] {
        let mut fds = [0; 2];
        assert_eq!(pipe(fds.as_mut_ptr()), 0);
        assert_eq!(super::super::host_fcntl(fds[0], F_SETFL, O_NONBLOCK), 0);
        assert_eq!(super::super::host_fcntl(fds[1], F_SETFL, O_NONBLOCK), 0);
        fds
    }
    unsafe fn fill(fd: i32) {
        let junk = [b'j'; 512];
        while super::super::host_write(fd, junk.as_ptr(), junk.len()) > 0 {}
    }
    unsafe fn drain(fd: i32) -> Vec<u8> {
        let mut all = Vec::new();
        let mut b = [0u8; 4096];
        loop {
            let n = super::super::host_read(fd, b.as_mut_ptr(), b.len());
            if n <= 0 {
                return all;
            }
            all.extend_from_slice(&b[..n as usize]);
        }
    }
    unsafe fn temp_path(tag: &str, contents: &[u8]) -> (String, std::ffi::CString) {
        let path = format!("/tmp/breenix-stdio-{tag}-{}", super::super::getpid());
        std::fs::write(&path, contents).unwrap();
        let c = std::ffi::CString::new(path.clone()).unwrap();
        (path, c)
    }
    /// Finding 1: a full buffer whose flush fails refuses bytes instead of storing
    /// past its end, and a newline whose line-buffered flush fails is not kept.
    #[test]
    fn failed_flush_neither_overflows_nor_duplicates() {
        unsafe {
            let fds = nonblocking_pipe();
            fill(fds[1]);
            let f = fdopen(fds[1], c(c"w"));
            assert!(!f.is_null());
            let mut area = [0xaau8; 48];
            assert_eq!(setvbuf(f, area.as_mut_ptr().add(16), FULL, 16), 0);
            let mut accepted = 0;
            for i in 0..100u8 {
                if fputc((b'a' + i % 26) as i32, f) == EOF {
                    break;
                }
                accepted += 1;
            }
            assert_eq!(accepted, 16);
            assert_ne!(ferror(f), 0);
            assert!(area[..16].iter().chain(&area[32..]).all(|&b| b == 0xaa));
            drain(fds[0]);
            clearerr(f);
            assert_eq!(fflush(f), 0);
            assert_eq!(drain(fds[0]), b"abcdefghijklmnop");

            assert_eq!(setvbuf(f, ptr::null_mut(), LINE, 0), 0);
            assert_eq!(fputs(c(c"xy"), f), 0);
            fill(fds[1]);
            assert_eq!(fputc(b'\n' as i32, f), EOF);
            drain(fds[0]);
            clearerr(f);
            assert_eq!(fputc(b'\n' as i32, f), b'\n' as i32);
            assert_eq!(drain(fds[0]), b"xy\n");
            assert_eq!(fclose(f), 0);
            super::super::close(fds[0]);
        }
    }
    /// Finding 3: fflush(NULL) and fopen return while another thread is blocked
    /// reading a stream.
    #[test]
    fn flush_all_passes_a_blocked_reader() {
        unsafe {
            let mut fds = [0; 2];
            assert_eq!(pipe(fds.as_mut_ptr()), 0);
            let r = fdopen(fds[0], c(c"r")) as usize;
            assert_ne!(r, 0);
            let reader = thread::spawn(move || fgetc(r as *mut File));
            thread::sleep(Duration::from_millis(100));
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                assert_eq!(fflush(ptr::null_mut()), 0);
                let g = fopen(c(c"/dev/null"), c(c"r"));
                assert_eq!(fclose(g), 0);
                tx.send(()).unwrap();
            });
            assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
            assert_eq!(super::super::write(fds[1], b"z".as_ptr(), 1), 1);
            assert_eq!(reader.join().unwrap(), b'z' as i32);
            assert_eq!(fclose(r as *mut File), 0);
            super::super::close(fds[1]);
        }
    }
    /// Finding 4: fflush(NULL) waits for a busy stream without holding the registry,
    /// so the stream's holder can still open a stream.
    #[test]
    fn flush_all_waits_outside_the_registry() {
        unsafe {
            let f = tmpfile() as usize;
            assert_ne!(f, 0);
            let (ready_tx, ready_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let done = done_tx.clone();
            thread::spawn(move || {
                let f = f as *mut File;
                flockfile(f);
                assert_eq!(fputs(c(c"pending"), f), 0);
                ready_tx.send(()).unwrap();
                thread::sleep(Duration::from_millis(200));
                let g = fopen(c(c"/dev/null"), c(c"r"));
                assert!(!g.is_null());
                assert_eq!(fclose(g), 0);
                funlockfile(f);
                done.send(()).unwrap();
            });
            ready_rx.recv().unwrap();
            thread::spawn(move || {
                assert_eq!(fflush(ptr::null_mut()), 0);
                done_tx.send(()).unwrap();
            });
            for _ in 0..2 {
                assert!(done_rx.recv_timeout(Duration::from_secs(5)).is_ok());
            }
            assert_eq!(ftell(f as *mut File), 7);
            assert_eq!(fclose(f as *mut File), 0);
        }
    }
    /// Finding 5: reading an unbuffered stream first flushes line-buffered output.
    #[test]
    fn line_buffered_output_is_flushed_before_input() {
        unsafe {
            let out = nonblocking_pipe();
            let mut input = [0; 2];
            assert_eq!(pipe(input.as_mut_ptr()), 0);
            let g = fdopen(out[1], c(c"w"));
            assert_eq!(setvbuf(g, ptr::null_mut(), LINE, 0), 0);
            assert_eq!(fputs(c(c"prompt> "), g), 0);
            assert_eq!(drain(out[0]), b"");
            let f = fdopen(input[0], c(c"r"));
            assert_eq!(setvbuf(f, ptr::null_mut(), NONE, 0), 0);
            assert_eq!(super::super::write(input[1], b"a".as_ptr(), 1), 1);
            assert_eq!(fgetc(f), b'a' as i32);
            assert_eq!(drain(out[0]), b"prompt> ");
            assert_eq!(fclose(f), 0);
            assert_eq!(fclose(g), 0);
            super::super::close(out[0]);
            super::super::close(input[1]);
        }
    }
    /// Finding 6: a precision bounds how much of an unterminated array is read.
    #[test]
    fn precision_bounds_unterminated_arrays() {
        unsafe {
            let page = getpagesize() as usize;
            // Two readable pages, each followed by an inaccessible one.
            let m = mmap(ptr::null_mut(), 4 * page, 3, 0x1002, -1, 0);
            assert!(!m.is_null() && m as isize != -1);
            assert_eq!(mprotect(m.add(page), page, 0), 0);
            assert_eq!(mprotect(m.add(3 * page), page, 0), 0);
            let s = m.add(page - 3);
            ptr::copy_nonoverlapping(b"abc".as_ptr(), s, 3);
            let w = m.add(3 * page - 12) as *mut u32;
            for (i, ch) in "xyz".chars().enumerate() {
                *w.add(i) = ch as u32;
            }
            let (text, _) = printed(c"%.3s|%.2s|%.3ls|%.2ls", |p, n, f| {
                snprintf(p, n, f, s, s, w, w)
            });
            assert_eq!(text, "abc|ab|xyz|xy");
            let mut out = WideOutput(Vec::new());
            let fmt: Vec<u32> = "%.3s|%.3ls\0".chars().map(|c| c as u32).collect();
            assert_eq!(wide_format(&mut out, fmt.as_ptr(), s, w), 7);
            let expected: Vec<u32> = "abc|xyz".chars().map(|c| c as u32).collect();
            assert_eq!(out.0, expected);
            munmap(m, 4 * page);
        }
    }
    /// Finding 7: one formatted call on an unbuffered stream, and one dprintf, is
    /// one write (each write on a datagram socket is one datagram).
    #[test]
    fn unbuffered_calls_write_once() {
        unsafe {
            let mut sv = [0; 2];
            assert_eq!(socketpair(1, 2, 0, sv.as_mut_ptr()), 0);
            let f = fdopen(sv[0], c(c"w"));
            assert_eq!(setvbuf(f, ptr::null_mut(), NONE, 0), 0);
            let datagram = || {
                let mut b = [0u8; 256];
                let n = recv(sv[1], b.as_mut_ptr(), b.len(), MSG_DONTWAIT);
                (n > 0).then(|| b[..n as usize].to_vec())
            };
            assert_eq!(
                fprintf(f, c(c"x=%d y=%s %5.1f\n"), 42i32, c(c"zz"), 2.5f64),
                16
            );
            assert_eq!(datagram().unwrap(), b"x=42 y=zz   2.5\n");
            assert_eq!(fputs(c(c"whole line\n"), f), 0);
            assert_eq!(datagram().unwrap(), b"whole line\n");
            assert_eq!(dprintf(sv[0], c(c"%d-%d|%s"), 1i32, 2i32, c(c"end")), 7);
            assert_eq!(datagram().unwrap(), b"1-2|end");
            assert_eq!(datagram(), None);
            assert_eq!(fclose(f), 0);
            super::super::close(sv[1]);
        }
    }
    /// Finding 10: buffered input goes back to the file on fflush and fclose
    /// (POSIX XSH 2.5.1), seen through a duplicate descriptor.
    #[test]
    fn input_offsets_return_on_flush_and_close() {
        unsafe {
            let (path, cpath) = temp_path("offset", b"abcdef");
            let fd = super::super::open(cpath.as_ptr() as *const u8, 0, 0);
            let other = dup(fd);
            let f = fdopen(fd, c(c"r"));
            assert_eq!(fgetc(f), b'a' as i32);
            assert_eq!(super::super::lseek(other, 0, 1), 6);
            assert_eq!(fflush(ptr::null_mut()), 0);
            assert_eq!(super::super::lseek(other, 0, 1), 1);
            assert_eq!(fgetc(f), b'b' as i32);
            assert_eq!(fclose(f), 0);
            assert_eq!(super::super::lseek(other, 0, 1), 2);
            super::super::close(other);
            std::fs::remove_file(path).unwrap();
        }
    }
    /// Finding 12: an error indicator from an earlier call does not turn a last line
    /// that ends at end-of-file into a failure.
    #[test]
    fn stale_error_keeps_a_final_unterminated_line() {
        unsafe {
            let (path, cpath) = temp_path("stale", b"last");
            let f = fopen(cpath.as_ptr() as *const u8, c(c"r"));
            assert_eq!(fputc(b'x' as i32, f), EOF);
            let mut b = [0u8; 16];
            assert_eq!(fgets(b.as_mut_ptr(), 16, f), b.as_mut_ptr());
            assert_eq!(&b[..5], b"last\0");
            assert_eq!(fseek(f, 0, 0), 0);
            assert_eq!(fputc(b'x' as i32, f), EOF);
            let mut line = ptr::null_mut();
            let mut cap = 0;
            assert_eq!(getline(&mut line, &mut cap, f), 4);
            super::super::free(line);
            assert_eq!(fclose(f), 0);
            std::fs::remove_file(path).unwrap();
        }
    }
    /// Finding 13: setvbuf with a caller array of size 0 does not write into it.
    #[test]
    fn setvbuf_size_zero_ignores_the_callers_array() {
        unsafe {
            let mut guard = [0x55u8; 8];
            let f = tmpfile();
            assert_eq!(setvbuf(f, guard.as_mut_ptr(), FULL, 0), 0);
            let data = [b'q'; 3000];
            assert_eq!(fwrite(data.as_ptr(), 1, data.len(), f), data.len());
            assert_eq!(guard, [0x55; 8]);
            rewind(f);
            let mut back = [0u8; 3000];
            assert_eq!(fread(back.as_mut_ptr(), 1, back.len(), f), back.len());
            assert_eq!(back, data);
            assert_eq!(fclose(f), 0);
        }
    }
    /// Finding 14: the ' flag and XSI %C and %S.
    #[test]
    fn grouping_flag_and_xsi_wide_conversions() {
        unsafe {
            let wide: Vec<u32> = "wide\0".chars().map(|c| c as u32).collect();
            let (s, n) = printed(c"%'d|%C|%S|%'.2f", |p, n, f| {
                snprintf(p, n, f, 1234567i32, 'A' as u32, wide.as_ptr(), 1.5f64)
            });
            assert_eq!(s, "1234567|A|wide|1.50");
            assert_eq!(n, s.len() as i32);
        }
    }
    /// Finding 15: fmemopen append starts at the first null byte, or at size.
    #[test]
    fn fmemopen_append_position() {
        unsafe {
            let mut full = *b"abc";
            let f = fmemopen(full.as_mut_ptr(), 3, c(c"a"));
            assert_eq!(ftell(f), 3);
            assert_eq!(fclose(f), 0);
            let mut part = *b"ab\0zz";
            let f = fmemopen(part.as_mut_ptr(), 5, c(c"a+"));
            assert_eq!(ftell(f), 2);
            assert_eq!(fclose(f), 0);
        }
    }
    /// Finding 16: ftell on an append stream counts output not yet written.
    #[test]
    fn append_ftell_counts_pending_output() {
        unsafe {
            let (path, cpath) = temp_path("append", b"hello");
            let f = fopen(cpath.as_ptr() as *const u8, c(c"a"));
            assert_eq!(fputs(c(c"xy"), f), 0);
            assert_eq!(ftell(f), 7);
            assert_eq!(fclose(f), 0);
            assert_eq!(std::fs::read(&path).unwrap(), b"helloxy");
            std::fs::remove_file(path).unwrap();
        }
    }
    /// Finding 17: fflush on unseekable input succeeds and keeps the buffered bytes;
    /// setvbuf refuses only when it would lose them.
    #[test]
    fn unseekable_input_keeps_buffered_bytes() {
        unsafe {
            let mut fds = [0; 2];
            assert_eq!(pipe(fds.as_mut_ptr()), 0);
            let f = fdopen(fds[0], c(c"r"));
            assert_eq!(setvbuf(f, ptr::null_mut(), FULL, 64), 0);
            assert_eq!(super::super::write(fds[1], b"abc".as_ptr(), 3), 3);
            assert_eq!(fgetc(f), b'a' as i32);
            assert_eq!(fflush(f), 0);
            assert_eq!(ferror(f), 0);
            assert_eq!(fgetc(f), b'b' as i32);
            assert_ne!(setvbuf(f, ptr::null_mut(), FULL, 128), 0);
            assert_eq!(fgetc(f), b'c' as i32);
            assert_eq!(fclose(f), 0);
            super::super::close(fds[1]);
        }
    }
    /// Finding 18: freopen(NULL, ...) needs a descriptor whose access mode allows
    /// the new mode, and does not apply to memory streams.
    #[test]
    fn freopen_null_path_checks_the_access_mode() {
        unsafe {
            let mut fds = [0; 2];
            assert_eq!(pipe(fds.as_mut_ptr()), 0);
            let f = fdopen(fds[1], c(c"w"));
            assert_eq!(freopen(ptr::null(), c(c"a"), f), f);
            assert!(freopen(ptr::null(), c(c"r"), f).is_null());
            assert_eq!(super::super::pthread::errno(), 9);
            // The failed freopen closed the stream's file; fclose releases the FILE.
            assert_eq!(fclose(f), EOF);
            super::super::close(fds[0]);
            let mut p = ptr::null_mut();
            let mut n = 0;
            let m = open_memstream(&mut p, &mut n);
            assert_eq!(fputs(c(c"kept"), m), 0);
            assert!(freopen(ptr::null(), c(c"w"), m).is_null());
            assert_eq!(super::super::pthread::errno(), 9);
            assert_eq!(
                (n, std::slice::from_raw_parts(p, 5)),
                (4, b"kept\0".as_slice())
            );
            assert_eq!(fclose(m), EOF);
            super::super::free(p);
        }
    }
    /// Finding 19: perror leaves stderr's orientation alone.
    #[test]
    fn perror_does_not_orient_stderr() {
        unsafe {
            super::super::pthread::set_errno(2);
            perror(c(c"stdio-host perror check"));
            assert_eq!(orient(stderr, 0), 0);
            assert_eq!(super::super::pthread::errno(), 2);
        }
    }
    /// Finding 21: tmpnam names are distinct, unused and of length L_tmpnam - 1.
    #[test]
    fn tmpnam_names_are_distinct_and_unused() {
        unsafe {
            let mut names = Vec::new();
            for _ in 0..3 {
                let mut b = [0u8; 35];
                assert_eq!(tmpnam(b.as_mut_ptr()), b.as_mut_ptr());
                let name = CStr::from_ptr(b.as_ptr() as *const i8).to_bytes().to_vec();
                assert_eq!(name.len(), 34);
                assert!(name.starts_with(b"/tmp/breenix-name-"));
                names.push(name);
            }
            names.dedup();
            assert_eq!(names.len(), 3);
        }
    }
    /// Finding 22: %% skips leading white space.
    #[test]
    fn scanf_percent_skips_white_space() {
        unsafe {
            let mut v = 0i32;
            assert_eq!(
                super::super::stdio::scan::sscanf(c(c"  %5"), c(c"%%%d"), &mut v),
                1
            );
            assert_eq!(v, 5);
        }
    }
    /// Finding 23: formatting allocates nothing, so a failing allocator cannot fail it.
    #[test]
    fn formatting_does_not_allocate() {
        unsafe {
            super::super::FAIL_REALLOC.with(|s| s.set(true));
            let (s, _) = printed(c"%s|%-6d|%08.3f|%.30e|%a", |p, n, f| {
                snprintf(p, n, f, c(c"text"), 42i32, -2.5f64, 0.1f64, 1.0f64)
            });
            assert!(super::super::FAIL_REALLOC.with(|s| s.replace(false)));
            assert_eq!(
                s,
                "text|42    |-002.500|1.000000000000000055511151231258e-01|0x1p+0"
            );
        }
    }
    /// Finding 24: %n$ accepts n up to NL_ARGMAX (9) and no further.
    #[test]
    fn positional_limit_is_nl_argmax() {
        unsafe {
            let (s, _) = printed(c"%9$d%8$d%7$d%6$d%5$d%4$d%3$d%2$d%1$d", |p, n, f| {
                snprintf(
                    p, n, f, 1i32, 2i32, 3i32, 4i32, 5i32, 6i32, 7i32, 8i32, 9i32,
                )
            });
            assert_eq!(s, "987654321");
            let mut b = [0u8; 8];
            assert_eq!(snprintf(b.as_mut_ptr(), 8, c(c"%10$d"), 1i32), -1);
            assert_eq!(super::super::pthread::errno(), 22);
        }
    }
    /// Finding 25: ctermid and renameat.
    #[test]
    fn ctermid_and_renameat() {
        unsafe {
            assert_eq!(
                CStr::from_ptr(ctermid(ptr::null_mut()) as *const i8),
                c"/dev/tty"
            );
            let mut b = [0u8; 9];
            assert_eq!(ctermid(b.as_mut_ptr()), b.as_mut_ptr());
            assert_eq!(&b, b"/dev/tty\0");
            let (old, cold) = temp_path("renameat", b"moved");
            let new = format!("{old}-new");
            let cnew = std::ffi::CString::new(new.clone()).unwrap();
            assert_eq!(
                renameat(
                    -100,
                    cold.as_ptr() as *const u8,
                    -100,
                    cnew.as_ptr() as *const u8
                ),
                0
            );
            assert_eq!(std::fs::read(&new).unwrap(), b"moved");
            std::fs::remove_file(new).unwrap();
        }
    }
}
