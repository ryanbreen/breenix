use super::{format, objects, scan, streams};
use crate::stdio::file;
use core::ffi::c_void;
unsafe fn utf8() -> *mut c_void {
    let l = objects::newlocale(1 << 2, b"C.UTF-8\0".as_ptr(), core::ptr::null_mut());
    assert!(!l.is_null());
    objects::uselocale(l);
    l
}
unsafe fn done(l: *mut c_void) {
    objects::uselocale(usize::MAX as *mut c_void);
    objects::freelocale(l);
}
#[test]
fn wide_streams_capture_encoding_pushback_and_error_indicators() {
    unsafe {
        let l = utf8();
        let mut bytes = [0u8; 32];
        let f = file::fmemopen(bytes.as_mut_ptr(), bytes.len(), b"w+\0".as_ptr());
        assert!(!f.is_null());
        assert_eq!(streams::fwide(f, 0), 0);
        assert_eq!(streams::fwide(f, 1), 1);
        objects::uselocale(usize::MAX as *mut c_void);
        assert_eq!(streams::fputwc(0x1f600, f), 0x1f600);
        assert_eq!(file::fflush(f), 0);
        assert_eq!(&bytes[..4], b"\xf0\x9f\x98\x80");
        file::rewind(f);
        assert_eq!(streams::fgetwc(f), 0x1f600);
        assert_eq!(streams::ungetwc(0x20ac, f), 0x20ac);
        assert_eq!(streams::fgetwc(f), 0x20ac);
        assert_eq!(streams::fgetwc(f), u32::MAX);
        assert_ne!(file::feof(f), 0);
        assert_eq!(streams::ungetwc(65, f), 65);
        assert_eq!(file::feof(f), 0);
        assert_eq!(streams::fgetwc(f), 65);
        file::fclose(f);
        objects::uselocale(l);
        let mut invalid = [b'a', 0xff];
        let f = file::fmemopen(invalid.as_mut_ptr(), 2, b"r\0".as_ptr());
        assert_eq!(streams::fgetwc(f), 97);
        assert_eq!(streams::fgetwc(f), u32::MAX);
        assert_eq!(crate::pthread::errno(), 84);
        assert_ne!(file::ferror(f), 0);
        file::rewind(f);
        assert_eq!(streams::fgetwc(f), 97);
        file::fclose(f);
        done(l);
    }
}
#[test]
fn wide_printf_counts_characters_and_rejects_truncation() {
    unsafe {
        let l = utf8();
        let mut out = [0u32; 32];
        let fmt = [37, 100, 45, 37, 108, 115, 45, 37, 108, 99, 0];
        let word = [120, 0xe9, 0];
        assert_eq!(
            format::swprintf(
                out.as_mut_ptr(),
                32,
                fmt.as_ptr(),
                7i32,
                word.as_ptr(),
                0x20acu32
            ),
            6
        );
        assert_eq!(&out[..7], &[55, 45, 120, 0xe9, 45, 0x20ac, 0]);
        let fmt = [37, 115, 0];
        assert!(format::swprintf(out.as_mut_ptr(), 3, fmt.as_ptr(), b"abcdef\0".as_ptr()) < 0);
        assert_eq!(out[2], 0);
        assert!(format::swprintf(core::ptr::null_mut(), 0, fmt.as_ptr(), b"x\0".as_ptr()) < 0);
        done(l);
    }
}

#[test]
fn wide_printf_preserves_c_locale_wide_values_and_utf8_precision() {
    unsafe {
        let mut out = [0u32; 32];
        let fmt = [37, 108, 115, 0];
        let word = [0x20ac, 0];
        assert_eq!(
            format::swprintf(out.as_mut_ptr(), 32, fmt.as_ptr(), word.as_ptr()),
            1
        );
        assert_eq!(&out[..2], &[0x20ac, 0]);
        let l = utf8();
        let fmt = [37, 46, 49, 115, 0];
        assert_eq!(
            format::swprintf(out.as_mut_ptr(), 32, fmt.as_ptr(), b"\xc3\xa9x\0".as_ptr()),
            1
        );
        assert_eq!(&out[..2], &[0xe9, 0]);
        let fmt = [37, 50, 36, 108, 115, 58, 37, 49, 36, 100, 0];
        assert_eq!(
            format::swprintf(out.as_mut_ptr(), 32, fmt.as_ptr(), 7i32, word.as_ptr()),
            3
        );
        assert_eq!(&out[..4], &[0x20ac, 58, 55, 0]);
        done(l);
    }
}

#[test]
fn empty_wide_io_orients_and_failed_pushback_preserves_input() {
    unsafe {
        let l = utf8();
        let mut bytes = [b'x', 0];
        let f = file::fmemopen(bytes.as_mut_ptr(), 2, b"r+\0".as_ptr());
        assert_eq!(streams::fputws([0].as_ptr(), f), 0);
        assert_eq!(streams::fwide(f, 0), 1);
        for _ in 0..6 {
            assert_eq!(streams::ungetwc(65, f), 65);
        }
        assert_eq!(streams::ungetwc(0x1f600, f), u32::MAX);
        // A failed push changes neither the input nor the indicators.
        assert_eq!((file::feof(f), file::ferror(f)), (0, 0));
        for _ in 0..6 {
            assert_eq!(streams::fgetwc(f), 65);
        }
        assert_eq!(streams::fgetwc(f), b'x' as u32);
        file::fclose(f);
        let f = file::fmemopen(bytes.as_mut_ptr(), 2, b"r\0".as_ptr());
        let mut w = [99u32; 1];
        assert_eq!(streams::fgetws(w.as_mut_ptr(), 1, f), w.as_mut_ptr());
        assert_eq!(w, [0]);
        assert_eq!(streams::fwide(f, 0), 1);
        file::fclose(f);
        done(l);
    }
}

#[test]
fn prior_error_does_not_fail_a_successful_wide_read() {
    unsafe {
        let mut bytes = *b"abc";
        let f = file::fmemopen(bytes.as_mut_ptr(), bytes.len(), b"r\0".as_ptr());
        assert_eq!(streams::fputwc(88, f), u32::MAX);
        assert_ne!(file::ferror(f), 0);
        let mut text = [0u32; 8];
        assert_eq!(streams::fgetws(text.as_mut_ptr(), 8, f), text.as_mut_ptr());
        assert_eq!(&text[..4], &[97, 98, 99, 0]);
        assert_ne!(file::ferror(f), 0);
        assert_ne!(file::feof(f), 0);
        file::fclose(f);
        let l = utf8();
        let mut bytes = *b"a\xe2\x82";
        let f = file::fmemopen(bytes.as_mut_ptr(), bytes.len(), b"r\0".as_ptr());
        assert!(streams::fgetws(text.as_mut_ptr(), 8, f).is_null());
        assert_eq!(crate::pthread::errno(), 84);
        assert_ne!(file::ferror(f), 0);
        file::rewind(f);
        assert_eq!(streams::fgetwc(f), 97);
        file::fclose(f);
        done(l);
    }
}

#[test]
fn wide_number_end_pointers_limits_and_direct_float_rounding() {
    fn wide(s: &str) -> Vec<u32> {
        s.chars().map(|c| c as u32).chain([0]).collect()
    }
    unsafe {
        let s = wide("  -123x");
        let mut end = core::ptr::null_mut();
        assert_eq!(super::numbers::wcstol(s.as_ptr(), &mut end, 10), -123);
        assert_eq!(end, s.as_ptr().add(6) as *mut u32);
        let s = wide("9223372036854775808!");
        assert_eq!(super::numbers::wcstol(s.as_ptr(), &mut end, 10), i64::MAX);
        assert_eq!(crate::pthread::errno(), 34);
        assert_eq!(end, s.as_ptr().add(19) as *mut u32);
        let s = wide("-9223372036854775808");
        crate::pthread::set_errno(0);
        assert_eq!(super::numbers::wcstoll(s.as_ptr(), &mut end, 10), i64::MIN);
        assert_eq!(crate::pthread::errno(), 0);
        let s = wide("-1");
        assert_eq!(super::numbers::wcstoull(s.as_ptr(), &mut end, 10), u64::MAX);
        let s = wide("  +x");
        assert_eq!(super::numbers::wcstol(s.as_ptr(), &mut end, 10), 0);
        assert_eq!(end, s.as_ptr() as *mut u32);
        assert_eq!(super::numbers::wcstol(s.as_ptr(), &mut end, 37), 0);
        assert_eq!(crate::pthread::errno(), 22);
        let s = wide("2.5e1x");
        assert_eq!(super::numbers::wcstod(s.as_ptr(), &mut end), 25.0);
        assert_eq!(end, s.as_ptr().add(5) as *mut u32);
        let s = wide("1e+");
        assert_eq!(super::numbers::wcstod(s.as_ptr(), &mut end), 1.0);
        assert_eq!(end, s.as_ptr().add(1) as *mut u32);
        let s = wide("1.0000000596046447753906250000000000000001");
        assert_eq!(
            super::numbers::wcstof(s.as_ptr(), &mut end).to_bits(),
            0x3f800001
        );
        let s = wide("0x1p-1074");
        assert_eq!(super::numbers::wcstod(s.as_ptr(), &mut end).to_bits(), 1);
        let s = wide("1e9999");
        assert_eq!(super::numbers::wcstod(s.as_ptr(), &mut end), f64::INFINITY);
        assert_eq!(crate::pthread::errno(), 34);
    }
}

#[test]
fn nonblocking_partial_character_retains_state_and_io_errno() {
    extern "C" {
        fn pipe(fds: *mut i32) -> i32;
    }
    unsafe {
        let l = utf8();
        let mut fds = [0; 2];
        assert_eq!(pipe(fds.as_mut_ptr()), 0);
        let nonblock = if cfg!(target_os = "macos") {
            4i32
        } else {
            2048i32
        };
        assert_eq!(crate::host_fcntl(fds[0], 4, nonblock), 0);
        let f = file::fdopen(fds[0], b"r\0".as_ptr());
        assert!(!f.is_null());
        assert_eq!(crate::host_write(fds[1], b"\xe2\x82".as_ptr(), 2), 2);
        assert_eq!(streams::fgetwc(f), u32::MAX);
        assert_eq!(crate::pthread::errno(), *crate::host_error());
        assert_ne!(crate::pthread::errno(), 84);
        assert_eq!(crate::host_write(fds[1], b"\xac".as_ptr(), 1), 1);
        assert_eq!(streams::fgetwc(f), 0x20ac);
        file::fclose(f);
        crate::host_close(fds[1]);
        done(l);
    }
}

extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
    fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
}
/// `n` writable bytes that end exactly where an inaccessible region begins, so a
/// read past them faults.
unsafe fn before_guard(n: usize) -> (*mut u8, *mut c_void) {
    const HALF: usize = 65536;
    let anonymous = if cfg!(target_os = "macos") { 0x1000 } else { 0x20 };
    let base = mmap(core::ptr::null_mut(), 2 * HALF, 3, 2 | anonymous, -1, 0);
    assert_ne!(base as isize, -1);
    assert_eq!(mprotect((base as *mut u8).add(HALF) as *mut c_void, HALF, 0), 0);
    ((base as *mut u8).add(HALF - n), base)
}

#[test]
fn c_locale_wide_streams_round_trip_high_bytes_and_unterminated_precision() {
    unsafe {
        // The C locale maps bytes 0x80-0xFF to the same wide values and back.
        let mut bytes = [0u8; 4];
        let f = file::fmemopen(bytes.as_mut_ptr(), 4, b"w+\0".as_ptr());
        assert!(!f.is_null());
        assert_eq!(streams::fputwc(0xe9, f), 0xe9);
        file::fflush(f);
        assert_eq!(bytes[0], 0xe9);
        assert_eq!(streams::fputwc(0x100, f), u32::MAX);
        assert_eq!(crate::pthread::errno(), 84);
        file::clearerr(f);
        file::rewind(f);
        assert_eq!(streams::fgetwc(f), 0xe9);
        assert_eq!(streams::ungetwc(0xe9, f), 0xe9);
        assert_eq!(streams::fgetwc(f), 0xe9);
        file::fclose(f);
        // With a precision the argument need not be terminated: nothing past the
        // precision is read.
        let mut out = [0u32; 8];
        let (p, region) = before_guard(8);
        let wide = p as *mut u32;
        *wide = 0x61;
        *wide.add(1) = 0x62;
        let fmt = [37, 46, 50, 108, 115, 0]; // %.2ls
        assert_eq!(format::swprintf(out.as_mut_ptr(), 8, fmt.as_ptr(), wide), 2);
        assert_eq!(&out[..3], &[0x61, 0x62, 0]);
        let (p, region2) = before_guard(2);
        *p = b'a';
        *p.add(1) = b'b';
        let fmt = [37, 46, 50, 115, 0]; // %.2s
        assert_eq!(format::swprintf(out.as_mut_ptr(), 8, fmt.as_ptr(), p), 2);
        assert_eq!(&out[..3], &[0x61, 0x62, 0]);
        munmap(region, 131072);
        munmap(region2, 131072);
    }
}

#[test]
fn wide_scanf_reads_wide_strings_and_streams() {
    unsafe {
        let l = utf8();
        // From a wide string: numbers, %ls, %lc, and a wide scanset.
        let text = [52, 50, 32, 104, 0xe9, 108, 32, 0x20ac, 0xe9, 0xe9, 120, 0];
        let fmt = [37, 100, 32, 37, 108, 115, 32, 37, 108, 99, 37, 108, 91, 0xe9, 93, 0];
        let (mut i, mut word, mut wc, mut set) = (0i32, [0u32; 8], 0u32, [0u32; 8]);
        assert_eq!(
            scan::swscanf(text.as_ptr(), fmt.as_ptr(), &mut i, word.as_mut_ptr(), &mut wc, set.as_mut_ptr()),
            4
        );
        assert_eq!((i, &word[..4], wc, &set[..3]), (42, &[104, 0xe9, 108, 0][..], 0x20ac, &[0xe9, 0xe9, 0][..]));
        // %s without l stores the multibyte encoding.
        let mut bytes = [0u8; 8];
        assert_eq!(scan::swscanf([104, 0xe9, 0].as_ptr(), [37, 115, 0].as_ptr(), bytes.as_mut_ptr()), 1);
        assert_eq!(&bytes[..4], b"h\xc3\xa9\0");
        // From a stream: the character after a conversion is pushed back.
        let mut input = *b"7 \xe2\x82\xacx\0";
        let f = file::fmemopen(input.as_mut_ptr(), 6, b"r\0".as_ptr());
        assert_eq!(scan::fwscanf(f, [37, 100, 32, 37, 108, 99, 0].as_ptr(), &mut i, &mut wc), 2);
        assert_eq!((i, wc), (7, 0x20ac));
        assert_eq!(streams::fgetwc(f), b'x' as u32);
        assert_eq!(streams::fwide(f, 0), 1);
        file::fclose(f);
        // An encoding error before the first conversion is an input failure.
        let mut input = *b"\xff1\0";
        let f = file::fmemopen(input.as_mut_ptr(), 2, b"r\0".as_ptr());
        assert_eq!(scan::fwscanf(f, [37, 100, 0].as_ptr(), &mut i), -1);
        assert_eq!(crate::pthread::errno(), 84);
        assert_ne!(file::ferror(f), 0);
        file::fclose(f);
        done(l);
    }
}
