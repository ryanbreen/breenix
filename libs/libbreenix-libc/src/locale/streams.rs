//! Wide streams retain the encoding selected when first oriented, ISO C 7.21.2.
use super::{charset, decode_charset, encode_charset, set_errno, Charset, Decoded, MbState};
use crate::stdio::file::{self, File};

pub(super) unsafe fn orient(f: *mut File) -> bool {
    if file::orient(f, 1) < 0 {
        return false;
    }
    let st = &mut *file::mb_state(f);
    if st.reserved == 0 {
        st.reserved = if charset() == Charset::Utf8 { 2 } else { 1 };
    }
    true
}
unsafe fn encoding(f: *mut File) -> Charset {
    if (*file::mb_state(f)).reserved == 2 {
        Charset::Utf8
    } else {
        Charset::Byte
    }
}
#[no_mangle]
pub unsafe extern "C" fn fwide(f: *mut File, mode: i32) -> i32 {
    file::lock(f);
    let r = file::orient(f, mode);
    if r > 0 {
        orient(f);
    }
    file::unlock(f);
    r
}
unsafe fn get(f: *mut File) -> u32 {
    if !orient(f) {
        return u32::MAX;
    }
    let mut partial = false;
    loop {
        let byte = file::getc_locked(f);
        if byte < 0 {
            if partial {
                file::set_error(f);
                set_errno(84);
            }
            return u32::MAX;
        }
        let enc = encoding(f);
        let st = &mut *file::mb_state(f);
        let saved = st.reserved;
        let decoded = decode_charset(&[byte as u8], st, enc);
        st.reserved = saved;
        match decoded {
            Decoded::Char(c, _) => return c,
            Decoded::Incomplete(_) => partial = true,
            Decoded::Invalid => {
                file::set_error(f);
                set_errno(84);
                return u32::MAX;
            }
        }
    }
}
pub(super) unsafe fn put(c: u32, f: *mut File) -> u32 {
    if !orient(f) {
        return u32::MAX;
    }
    let mut bytes = [0; 4];
    let Some(n) = encode_charset(c, &mut bytes, encoding(f)) else {
        file::set_error(f);
        set_errno(84);
        return u32::MAX;
    };
    if file::write_locked(f, &bytes[..n]) != n {
        u32::MAX
    } else {
        c
    }
}
#[no_mangle]
pub unsafe extern "C" fn fgetwc(f: *mut File) -> u32 {
    file::lock(f);
    let r = get(f);
    file::unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn getwc(f: *mut File) -> u32 {
    fgetwc(f)
}
#[no_mangle]
pub unsafe extern "C" fn getwchar() -> u32 {
    fgetwc(file::stdin)
}
#[no_mangle]
pub unsafe extern "C" fn fputwc(c: u32, f: *mut File) -> u32 {
    file::lock(f);
    let r = put(c, f);
    file::unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn putwc(c: u32, f: *mut File) -> u32 {
    fputwc(c, f)
}
#[no_mangle]
pub unsafe extern "C" fn putwchar(c: u32) -> u32 {
    fputwc(c, file::stdout)
}
#[no_mangle]
pub unsafe extern "C" fn fgetws(s: *mut u32, n: i32, f: *mut File) -> *mut u32 {
    if n <= 0 {
        return core::ptr::null_mut();
    }
    file::lock(f);
    let mut i = 0;
    while i + 1 < n {
        let c = get(f);
        if c == u32::MAX {
            if i == 0 || file::ferror_unlocked(f) != 0 {
                file::unlock(f);
                return core::ptr::null_mut();
            }
            break;
        }
        *s.add(i as usize) = c;
        i += 1;
        if c == 10 {
            break;
        }
    }
    *s.add(i as usize) = 0;
    file::unlock(f);
    s
}
#[no_mangle]
pub unsafe extern "C" fn fputws(s: *const u32, f: *mut File) -> i32 {
    file::lock(f);
    let mut i = 0;
    while *s.add(i) != 0 {
        if put(*s.add(i), f) == u32::MAX {
            file::unlock(f);
            return -1;
        }
        i += 1;
    }
    file::unlock(f);
    0
}
#[no_mangle]
pub unsafe extern "C" fn ungetwc(c: u32, f: *mut File) -> u32 {
    if c == u32::MAX {
        return u32::MAX;
    }
    file::lock(f);
    let mut bytes = [0; 4];
    let r = if !orient(f) {
        u32::MAX
    } else if let Some(n) = encode_charset(c, &mut bytes, encoding(f)) {
        let mut okay = true;
        for &b in bytes[..n].iter().rev() {
            if file::ungetc_locked(b as i32, f) < 0 {
                okay = false;
                break;
            }
        }
        if okay {
            let saved = (*file::mb_state(f)).reserved;
            *file::mb_state(f) = MbState::default();
            (*file::mb_state(f)).reserved = saved;
            c
        } else {
            u32::MAX
        }
    } else {
        set_errno(84);
        u32::MAX
    };
    file::unlock(f);
    r
}
