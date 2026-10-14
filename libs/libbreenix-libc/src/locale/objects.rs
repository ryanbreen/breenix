use super::{set_errno, Charset};
use core::ffi::c_void;
use core::ptr::{addr_of_mut, null_mut};
use core::sync::atomic::{AtomicU32, Ordering};

const ALL_MASK: u32 = 0x1fbf;
static GLOBAL: AtomicU32 = AtomicU32::new(0);
const NAMES: [&[u8]; 13] = [
    b"LC_CTYPE",
    b"LC_NUMERIC",
    b"LC_TIME",
    b"LC_COLLATE",
    b"LC_MONETARY",
    b"LC_MESSAGES",
    b"",
    b"LC_PAPER",
    b"LC_NAME",
    b"LC_ADDRESS",
    b"LC_TELEPHONE",
    b"LC_MEASUREMENT",
    b"LC_IDENTIFICATION",
];
const C: &[u8] = b"C\0";
const UTF8: &[u8] = b"C.UTF-8\0";
static mut QUERY: [u8; 512] = [0; 512];
#[repr(C)]
struct Locale {
    bits: u32,
}

unsafe fn bytes<'a>(s: *const u8) -> &'a [u8] {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(s, n)
}
fn parse(name: &[u8]) -> Option<bool> {
    match name {
        b"C" | b"POSIX" => Some(false),
        b"C.UTF-8" | b"C.utf8" => Some(true),
        _ => None,
    }
}
fn environment(cat: usize) -> Option<bool> {
    for name in [b"LC_ALL".as_slice(), NAMES[cat], b"LANG".as_slice()] {
        if let Some(value) = crate::env::getenv_bytes(name) {
            if !value.is_empty() {
                return parse(value);
            }
        }
    }
    Some(false)
}
fn selected() -> u32 {
    unsafe {
        let p = *crate::pthread::locale_slot();
        if p.is_null() {
            GLOBAL.load(Ordering::Acquire)
        } else {
            (*(p as *const Locale)).bits
        }
    }
}
pub(super) fn encoding() -> Charset {
    if selected() & 1 != 0 {
        Charset::Utf8
    } else {
        Charset::Byte
    }
}
pub(super) unsafe fn encoding_l(p: *mut c_void) -> Charset {
    let bits = if p.is_null() || p as usize == usize::MAX {
        GLOBAL.load(Ordering::Acquire)
    } else {
        (*(p as *const Locale)).bits
    };
    if bits & 1 != 0 {
        Charset::Utf8
    } else {
        Charset::Byte
    }
}
unsafe fn result(bits: u32, cat: i32) -> *mut u8 {
    if cat != 6 {
        return if bits & (1 << cat) == 0 {
            C.as_ptr() as *mut u8
        } else {
            UTF8.as_ptr() as *mut u8
        };
    }
    if bits == 0 {
        return C.as_ptr() as *mut u8;
    }
    if bits == ALL_MASK {
        return UTF8.as_ptr() as *mut u8;
    }
    let out = &mut *addr_of_mut!(QUERY);
    let mut n = 0;
    for (cat, name) in NAMES.iter().enumerate() {
        if cat == 6 {
            continue;
        }
        if n != 0 {
            out[n] = b';';
            n += 1;
        }
        for &c in *name {
            out[n] = c;
            n += 1;
        }
        out[n] = b'=';
        n += 1;
        let value = if bits & (1 << cat) == 0 { C } else { UTF8 };
        for &c in &value[..value.len() - 1] {
            out[n] = c;
            n += 1;
        }
    }
    out[n] = 0;
    out.as_mut_ptr()
}
fn apply(mut bits: u32, mask: u32, name: &[u8]) -> Option<u32> {
    // Accept our own aggregate LC_ALL query string as POSIX requires.
    if name.starts_with(b"LC_CTYPE=") {
        if mask != ALL_MASK {
            return None;
        }
        let mut found = 0;
        for assignment in name.split(|&c| c == b';') {
            let at = assignment.iter().position(|&c| c == b'=')?;
            let cat = NAMES.iter().position(|n| *n == &assignment[..at])?;
            if cat == 6 || found & (1 << cat) != 0 {
                return None;
            }
            found |= 1 << cat;
            if parse(&assignment[at + 1..])? {
                bits |= 1 << cat;
            } else {
                bits &= !(1 << cat);
            }
        }
        return if found == ALL_MASK { Some(bits) } else { None };
    }
    for cat in 0..13 {
        if mask & (1 << cat) == 0 {
            continue;
        }
        let utf8 = if name.is_empty() {
            environment(cat)?
        } else {
            parse(name)?
        };
        if utf8 {
            bits |= 1 << cat;
        } else {
            bits &= !(1 << cat);
        }
    }
    Some(bits)
}
#[no_mangle]
pub unsafe extern "C" fn setlocale(cat: i32, name: *const u8) -> *mut u8 {
    if !(0..13).contains(&cat) {
        return null_mut();
    }
    let old = GLOBAL.load(Ordering::Acquire);
    if name.is_null() {
        return result(old, cat);
    }
    let mask = if cat == 6 { ALL_MASK } else { 1 << cat };
    let Some(bits) = apply(old, mask, bytes(name)) else {
        return null_mut();
    };
    GLOBAL.store(bits, Ordering::Release);
    result(bits, cat)
}
#[no_mangle]
pub unsafe extern "C" fn newlocale(mask: i32, name: *const u8, base: *mut c_void) -> *mut c_void {
    if name.is_null() || mask < 0 || mask as u32 & !ALL_MASK != 0 || base as usize == usize::MAX {
        set_errno(22);
        return null_mut();
    }
    let old = if base.is_null() {
        0
    } else {
        (*(base as *const Locale)).bits
    };
    let Some(bits) = apply(old, mask as u32, bytes(name)) else {
        set_errno(2);
        return null_mut();
    };
    // Reuse the base only after all validation succeeds.
    let p = if base.is_null() {
        crate::malloc(core::mem::size_of::<Locale>()) as *mut c_void
    } else {
        base
    };
    if p.is_null() {
        set_errno(12);
        return p;
    }
    (*(p as *mut Locale)).bits = bits;
    p
}
#[no_mangle]
pub unsafe extern "C" fn duplocale(p: *mut c_void) -> *mut c_void {
    if p.is_null() {
        set_errno(22);
        return null_mut();
    }
    let bits = if p as usize == usize::MAX {
        GLOBAL.load(Ordering::Acquire)
    } else {
        (*(p as *const Locale)).bits
    };
    let out = crate::malloc(core::mem::size_of::<Locale>()) as *mut Locale;
    if out.is_null() {
        set_errno(12);
        return null_mut();
    }
    (*out).bits = bits;
    out as *mut c_void
}
#[no_mangle]
pub unsafe extern "C" fn freelocale(p: *mut c_void) {
    if !p.is_null() && p as usize != usize::MAX {
        crate::free(p as *mut u8);
    }
}
#[no_mangle]
pub unsafe extern "C" fn uselocale(p: *mut c_void) -> *mut c_void {
    let slot = crate::pthread::locale_slot();
    let old = *slot;
    if !p.is_null() {
        *slot = if p as usize == usize::MAX {
            null_mut()
        } else {
            p
        };
    }
    if old.is_null() {
        usize::MAX as *mut c_void
    } else {
        old
    }
}

#[repr(C)]
pub struct Lconv {
    decimal_point: *const u8,
    thousands_sep: *const u8,
    grouping: *const u8,
    int_curr_symbol: *const u8,
    currency_symbol: *const u8,
    mon_decimal_point: *const u8,
    mon_thousands_sep: *const u8,
    mon_grouping: *const u8,
    positive_sign: *const u8,
    negative_sign: *const u8,
    int_frac_digits: u8,
    frac_digits: u8,
    p_cs_precedes: u8,
    p_sep_by_space: u8,
    n_cs_precedes: u8,
    n_sep_by_space: u8,
    p_sign_posn: u8,
    n_sign_posn: u8,
    int_p_cs_precedes: u8,
    int_p_sep_by_space: u8,
    int_n_cs_precedes: u8,
    int_n_sep_by_space: u8,
    int_p_sign_posn: u8,
    int_n_sign_posn: u8,
}
const UNSPEC: u8 = if cfg!(target_arch = "aarch64") {
    255
} else {
    127
};
static mut CONVENTIONS: Lconv = Lconv {
    decimal_point: b".\0".as_ptr(),
    thousands_sep: b"\0".as_ptr(),
    grouping: b"\0".as_ptr(),
    int_curr_symbol: b"\0".as_ptr(),
    currency_symbol: b"\0".as_ptr(),
    mon_decimal_point: b"\0".as_ptr(),
    mon_thousands_sep: b"\0".as_ptr(),
    mon_grouping: b"\0".as_ptr(),
    positive_sign: b"\0".as_ptr(),
    negative_sign: b"\0".as_ptr(),
    int_frac_digits: UNSPEC,
    frac_digits: UNSPEC,
    p_cs_precedes: UNSPEC,
    p_sep_by_space: UNSPEC,
    n_cs_precedes: UNSPEC,
    n_sep_by_space: UNSPEC,
    p_sign_posn: UNSPEC,
    n_sign_posn: UNSPEC,
    int_p_cs_precedes: UNSPEC,
    int_p_sep_by_space: UNSPEC,
    int_n_cs_precedes: UNSPEC,
    int_n_sep_by_space: UNSPEC,
    int_p_sign_posn: UNSPEC,
    int_n_sign_posn: UNSPEC,
};
#[no_mangle]
pub extern "C" fn localeconv() -> *mut Lconv {
    addr_of_mut!(CONVENTIONS)
}

fn info(item: i32, encoding: Charset) -> *mut u8 {
    let text: &[u8] = match item {
        0x20000..=0x20006 => return super::calendar::day_info((item - 0x20000) as usize, false),
        0x20007..=0x2000d => return super::calendar::day_info((item - 0x20007) as usize, true),
        0x2000e..=0x20019 => return super::calendar::month_info((item - 0x2000e) as usize, false),
        0x2001a..=0x20025 => return super::calendar::month_info((item - 0x2001a) as usize, true),
        0x4000f => b"-\0",
        14 => {
            if encoding == Charset::Utf8 {
                b"UTF-8\0"
            } else {
                b"ANSI_X3.4-1968\0"
            }
        }
        0x10000 => b".\0",
        0x10001 => b"\0",
        0x20026 => b"AM\0",
        0x20027 => b"PM\0",
        0x20028 => b"%a %b %e %H:%M:%S %Y\0",
        0x20029 => b"%m/%d/%y\0",
        0x2002a => b"%H:%M:%S\0",
        0x2002b => b"%I:%M:%S %p\0",
        0x50000 => b"^[yY]\0",
        0x50001 => b"^[nN]\0",
        _ => b"\0",
    };
    text.as_ptr() as *mut u8
}
#[no_mangle]
pub extern "C" fn nl_langinfo(item: i32) -> *mut u8 {
    info(item, encoding())
}
#[no_mangle]
pub unsafe extern "C" fn nl_langinfo_l(item: i32, p: *mut c_void) -> *mut u8 {
    info(item, encoding_l(p))
}
