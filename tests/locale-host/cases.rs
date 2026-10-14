use super::{format, objects, streams};
use crate::stdio::file;
use core::ffi::c_void;
unsafe fn utf8() -> *mut c_void {
    let l = objects::newlocale(1, b"C.UTF-8\0".as_ptr(), core::ptr::null_mut());
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
