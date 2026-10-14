use super::{set_errno, Charset};
use core::ffi::c_void;
use core::ptr::{addr_of_mut, null_mut};
use core::sync::atomic::{AtomicU32, Ordering};

// Breenix category numbers, mirrored in include/locale.h: 0-6 in the order
// POSIX.1-2024 <locale.h> lists the categories. A locale is one bit per category,
// at 1 << category, set when that category is C.UTF-8 rather than C.
const LC_ALL: i32 = 0;
const LC_CTYPE: usize = 2;
const CATEGORIES: usize = 7;
const ALL_MASK: u32 = 0x7e;
static GLOBAL: AtomicU32 = AtomicU32::new(0);
const NAMES: [&[u8]; CATEGORIES] = [
    b"LC_ALL",
    b"LC_COLLATE",
    b"LC_CTYPE",
    b"LC_MESSAGES",
    b"LC_MONETARY",
    b"LC_NUMERIC",
    b"LC_TIME",
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
    for name in [NAMES[LC_ALL as usize], NAMES[cat], b"LANG".as_slice()] {
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
fn ctype(bits: u32) -> Charset {
    if bits & (1 << LC_CTYPE) != 0 {
        Charset::Utf8
    } else {
        Charset::Byte
    }
}
pub(super) fn encoding() -> Charset {
    ctype(selected())
}
pub(super) unsafe fn encoding_l(p: *mut c_void) -> Charset {
    ctype(bits_l(p))
}
unsafe fn bits_l(p: *mut c_void) -> u32 {
    if p.is_null() || p as usize == usize::MAX {
        GLOBAL.load(Ordering::Acquire)
    } else {
        (*(p as *const Locale)).bits
    }
}
unsafe fn result(bits: u32, cat: i32) -> *mut u8 {
    if cat != LC_ALL {
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
    for (cat, name) in NAMES.iter().enumerate().skip(1) {
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
    if name.starts_with(b"LC_") {
        if mask != ALL_MASK {
            return None;
        }
        let mut found = 0;
        for assignment in name.split(|&c| c == b';') {
            let at = assignment.iter().position(|&c| c == b'=')?;
            let cat = NAMES.iter().position(|n| *n == &assignment[..at])?;
            if cat == 0 || found & (1 << cat) != 0 {
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
    for cat in 1..CATEGORIES {
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
    if !(0..CATEGORIES as i32).contains(&cat) {
        return null_mut();
    }
    let old = GLOBAL.load(Ordering::Acquire);
    if name.is_null() {
        return result(old, cat);
    }
    let mask = if cat == LC_ALL { ALL_MASK } else { 1 << cat };
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
    let bits = bits_l(p);
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

// Item numbers, mirrored in include/langinfo.h: consecutive from 1 in the order
// of the POSIX.1-2024 <langinfo.h> table.
const CODESET: i32 = 1;
const D_T_FMT: i32 = 2;
const D_FMT: i32 = 3;
const T_FMT: i32 = 4;
const T_FMT_AMPM: i32 = 5;
const AM_STR: i32 = 6;
const PM_STR: i32 = 7;
const DAY_1: i32 = 8;
const ABDAY_1: i32 = 15;
const MON_1: i32 = 22;
const ALTMON_1: i32 = 34;
const ABMON_1: i32 = 46;
const ABALTMON_1: i32 = 58;
const ERA: i32 = 70;
const ALT_DIGITS: i32 = 74;
const RADIXCHAR: i32 = 75;
const THOUSEP: i32 = 76;
const YESEXPR: i32 = 77;
const NOEXPR: i32 = 78;
const CRNCYSTR: i32 = 79;

fn info(item: i32, encoding: Charset) -> *mut u8 {
    use super::calendar::{day_info, month_info};
    // The values are the POSIX locale's (POSIX.1-2024 XBD 7.3), which C.UTF-8 shares
    // apart from its codeset.
    let text: &[u8] = match item {
        DAY_1..=14 => return day_info((item - DAY_1) as usize, true),
        ABDAY_1..=21 => return day_info((item - ABDAY_1) as usize, false),
        // The POSIX locale has no alternative month names, so they are the plain ones.
        MON_1..=33 => return month_info((item - MON_1) as usize, true),
        ALTMON_1..=45 => return month_info((item - ALTMON_1) as usize, true),
        ABMON_1..=57 => return month_info((item - ABMON_1) as usize, false),
        ABALTMON_1..=69 => return month_info((item - ABALTMON_1) as usize, false),
        CODESET => {
            if encoding == Charset::Utf8 {
                b"UTF-8\0"
            } else {
                // The C locale maps each byte 0x00-0xFF to U+0000-U+00FF and back; this
                // is the IANA character-set registry's name for that mapping.
                b"ISO-8859-1\0"
            }
        }
        D_T_FMT => b"%a %b %e %H:%M:%S %Y\0",
        D_FMT => b"%m/%d/%y\0",
        T_FMT => b"%H:%M:%S\0",
        T_FMT_AMPM => b"%I:%M:%S %p\0",
        AM_STR => b"AM\0",
        PM_STR => b"PM\0",
        RADIXCHAR => b".\0",
        YESEXPR => b"^[yY]\0",
        NOEXPR => b"^[nN]\0",
        // No eras or alternative digits, and THOUSEP is empty. CRNCYSTR is empty
        // because the currency symbol is (POSIX.1-2024 <langinfo.h> allows "" then).
        ERA..=ALT_DIGITS | THOUSEP | CRNCYSTR => b"\0",
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
/// POSIX.1-2024 getlocalename_l: the name of one category of a locale object.
#[no_mangle]
pub unsafe extern "C" fn getlocalename_l(cat: i32, p: *mut c_void) -> *const u8 {
    if !(1..CATEGORIES as i32).contains(&cat) || p.is_null() {
        return core::ptr::null();
    }
    result(bits_l(p), cat)
}

#[cfg(test)]
mod tests {
    // The C headers and these constants must agree; parse the headers' #defines.
    fn defines(header: &str) -> std::collections::HashMap<&str, i32> {
        header
            .lines()
            .filter_map(|l| {
                let mut w = l.strip_prefix("#define ")?.split_whitespace();
                Some((w.next()?, w.next()?.parse().ok()?))
            })
            .collect()
    }
    #[test]
    fn headers_match_breenix_numbering() {
        let lc = defines(include_str!("../../include/locale.h"));
        assert_eq!(lc["LC_ALL"], super::LC_ALL);
        assert_eq!(lc["LC_CTYPE"] as usize, super::LC_CTYPE);
        assert_eq!(lc.len(), super::CATEGORIES);
        for (i, name) in super::NAMES.iter().enumerate() {
            assert_eq!(lc[core::str::from_utf8(name).unwrap()], i as i32);
        }
        let li = defines(include_str!("../../include/langinfo.h"));
        for (name, value) in [
            ("CODESET", super::CODESET),
            ("D_T_FMT", super::D_T_FMT),
            ("D_FMT", super::D_FMT),
            ("T_FMT", super::T_FMT),
            ("T_FMT_AMPM", super::T_FMT_AMPM),
            ("AM_STR", super::AM_STR),
            ("PM_STR", super::PM_STR),
            ("DAY_1", super::DAY_1),
            ("DAY_7", 14),
            ("ABDAY_1", super::ABDAY_1),
            ("ABDAY_7", 21),
            ("MON_1", super::MON_1),
            ("MON_12", 33),
            ("ALTMON_1", super::ALTMON_1),
            ("ALTMON_12", 45),
            ("ABMON_1", super::ABMON_1),
            ("ABMON_12", 57),
            ("ABALTMON_1", super::ABALTMON_1),
            ("ABALTMON_12", 69),
            ("ERA", super::ERA),
            ("ALT_DIGITS", super::ALT_DIGITS),
            ("RADIXCHAR", super::RADIXCHAR),
            ("THOUSEP", super::THOUSEP),
            ("YESEXPR", super::YESEXPR),
            ("NOEXPR", super::NOEXPR),
            ("CRNCYSTR", super::CRNCYSTR),
        ] {
            assert_eq!(li[name], value, "{name}");
        }
        assert_eq!(li.len(), 79);
        let mut values: Vec<_> = li.values().collect();
        values.sort();
        assert!(values.iter().enumerate().all(|(i, &&v)| v == i as i32 + 1));
    }
}
