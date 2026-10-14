use super::objects::encoding_l;
use super::unicode_tables::{CLASSES, LOWER, UPPER};
use super::{charset, Charset};
use core::ffi::c_void;

fn flags(c: u32, encoding: Charset) -> u16 {
    if c > 0x10ffff || (encoding == Charset::Byte && c >= 128) {
        return 0;
    }
    let i = CLASSES.partition_point(|&(lo, _, _)| lo <= c);
    if i != 0 && c <= CLASSES[i - 1].1 {
        CLASSES[i - 1].2
    } else {
        0
    }
}
fn has(c: u32, class: usize, encoding: Charset) -> i32 {
    i32::from((1..=12).contains(&class) && flags(c, encoding) & (1 << (class - 1)) != 0)
}
fn mapped(c: u32, table: &[(u32, u32)], encoding: Charset) -> u32 {
    if encoding == Charset::Byte && c >= 128 {
        return c;
    }
    match table.binary_search_by_key(&c, |&(cp, _)| cp) {
        Ok(i) => table[i].1,
        Err(_) => c,
    }
}
pub(super) fn lower_in(c: u32, encoding: Charset) -> u32 {
    mapped(c, LOWER, encoding)
}
#[no_mangle]
pub extern "C" fn towlower(c: u32) -> u32 {
    mapped(c, LOWER, charset())
}
#[no_mangle]
pub extern "C" fn towupper(c: u32) -> u32 {
    mapped(c, UPPER, charset())
}
#[no_mangle]
pub unsafe extern "C" fn towlower_l(c: u32, l: *mut c_void) -> u32 {
    mapped(c, LOWER, encoding_l(l))
}
#[no_mangle]
pub unsafe extern "C" fn towupper_l(c: u32, l: *mut c_void) -> u32 {
    mapped(c, UPPER, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn tolower(c: i32) -> i32 {
    if (65..=90).contains(&c) {
        c + 32
    } else {
        c
    }
}
#[no_mangle]
pub extern "C" fn toupper(c: i32) -> i32 {
    if (97..=122).contains(&c) {
        c - 32
    } else {
        c
    }
}
#[no_mangle]
pub extern "C" fn tolower_l(c: i32, _l: *mut c_void) -> i32 {
    tolower(c)
}
#[no_mangle]
pub extern "C" fn toupper_l(c: i32, _l: *mut c_void) -> i32 {
    toupper(c)
}
#[no_mangle]
pub extern "C" fn isascii(c: i32) -> i32 {
    i32::from((0..128).contains(&c))
}
#[no_mangle]
pub extern "C" fn toascii(c: i32) -> i32 {
    c & 127
}

unsafe fn name_is(p: *const u8, s: &[u8]) -> bool {
    if p.is_null() {
        return false;
    }
    for (i, &c) in s.iter().enumerate() {
        if *p.add(i) != c {
            return false;
        }
    }
    *p.add(s.len()) == 0
}
#[no_mangle]
pub unsafe extern "C" fn wctype(name: *const u8) -> usize {
    let names: [&[u8]; 12] = [
        b"alnum", b"alpha", b"blank", b"cntrl", b"digit", b"graph", b"lower", b"print", b"punct",
        b"space", b"upper", b"xdigit",
    ];
    names
        .iter()
        .position(|s| name_is(name, s))
        .map_or(0, |i| i + 1)
}
#[no_mangle]
pub unsafe extern "C" fn wctype_l(name: *const u8, _l: *mut c_void) -> usize {
    wctype(name)
}
#[no_mangle]
pub extern "C" fn iswctype(c: u32, class: usize) -> i32 {
    has(c, class, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswctype_l(c: u32, class: usize, l: *mut c_void) -> i32 {
    has(c, class, encoding_l(l))
}
static DOWN: i32 = 1;
static UP: i32 = 2;
#[no_mangle]
pub unsafe extern "C" fn wctrans(name: *const u8) -> *const i32 {
    if name_is(name, b"tolower") {
        &DOWN
    } else if name_is(name, b"toupper") {
        &UP
    } else {
        core::ptr::null()
    }
}
#[no_mangle]
pub unsafe extern "C" fn wctrans_l(name: *const u8, _l: *mut c_void) -> *const i32 {
    wctrans(name)
}
#[no_mangle]
pub extern "C" fn towctrans(c: u32, t: *const i32) -> u32 {
    if t == &DOWN {
        towlower(c)
    } else if t == &UP {
        towupper(c)
    } else {
        c
    }
}
#[no_mangle]
pub unsafe extern "C" fn towctrans_l(c: u32, t: *const i32, l: *mut c_void) -> u32 {
    if t == &DOWN {
        towlower_l(c, l)
    } else if t == &UP {
        towupper_l(c, l)
    } else {
        c
    }
}
#[no_mangle]
pub extern "C" fn iswalnum(c: u32) -> i32 {
    has(c, 1, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswalnum_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 1, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isalnum(c: i32) -> i32 {
    has(c as u32, 1, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isalnum_l(c: i32, _l: *mut c_void) -> i32 {
    isalnum(c)
}
#[no_mangle]
pub extern "C" fn iswalpha(c: u32) -> i32 {
    has(c, 2, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswalpha_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 2, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isalpha(c: i32) -> i32 {
    has(c as u32, 2, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isalpha_l(c: i32, _l: *mut c_void) -> i32 {
    isalpha(c)
}
#[no_mangle]
pub extern "C" fn iswblank(c: u32) -> i32 {
    has(c, 3, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswblank_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 3, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isblank(c: i32) -> i32 {
    has(c as u32, 3, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isblank_l(c: i32, _l: *mut c_void) -> i32 {
    isblank(c)
}
#[no_mangle]
pub extern "C" fn iswcntrl(c: u32) -> i32 {
    has(c, 4, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswcntrl_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 4, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn iscntrl(c: i32) -> i32 {
    has(c as u32, 4, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn iscntrl_l(c: i32, _l: *mut c_void) -> i32 {
    iscntrl(c)
}
#[no_mangle]
pub extern "C" fn iswdigit(c: u32) -> i32 {
    has(c, 5, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswdigit_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 5, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isdigit(c: i32) -> i32 {
    has(c as u32, 5, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isdigit_l(c: i32, _l: *mut c_void) -> i32 {
    isdigit(c)
}
#[no_mangle]
pub extern "C" fn iswgraph(c: u32) -> i32 {
    has(c, 6, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswgraph_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 6, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isgraph(c: i32) -> i32 {
    has(c as u32, 6, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isgraph_l(c: i32, _l: *mut c_void) -> i32 {
    isgraph(c)
}
#[no_mangle]
pub extern "C" fn iswlower(c: u32) -> i32 {
    has(c, 7, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswlower_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 7, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn islower(c: i32) -> i32 {
    has(c as u32, 7, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn islower_l(c: i32, _l: *mut c_void) -> i32 {
    islower(c)
}
#[no_mangle]
pub extern "C" fn iswprint(c: u32) -> i32 {
    has(c, 8, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswprint_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 8, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isprint(c: i32) -> i32 {
    has(c as u32, 8, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isprint_l(c: i32, _l: *mut c_void) -> i32 {
    isprint(c)
}
#[no_mangle]
pub extern "C" fn iswpunct(c: u32) -> i32 {
    has(c, 9, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswpunct_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 9, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn ispunct(c: i32) -> i32 {
    has(c as u32, 9, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn ispunct_l(c: i32, _l: *mut c_void) -> i32 {
    ispunct(c)
}
#[no_mangle]
pub extern "C" fn iswspace(c: u32) -> i32 {
    has(c, 10, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswspace_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 10, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isspace(c: i32) -> i32 {
    has(c as u32, 10, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isspace_l(c: i32, _l: *mut c_void) -> i32 {
    isspace(c)
}
#[no_mangle]
pub extern "C" fn iswupper(c: u32) -> i32 {
    has(c, 11, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswupper_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 11, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isupper(c: i32) -> i32 {
    has(c as u32, 11, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isupper_l(c: i32, _l: *mut c_void) -> i32 {
    isupper(c)
}
#[no_mangle]
pub extern "C" fn iswxdigit(c: u32) -> i32 {
    has(c, 12, charset())
}
#[no_mangle]
pub unsafe extern "C" fn iswxdigit_l(c: u32, l: *mut c_void) -> i32 {
    has(c, 12, encoding_l(l))
}
#[no_mangle]
pub extern "C" fn isxdigit(c: i32) -> i32 {
    has(c as u32, 12, Charset::Byte)
}
#[no_mangle]
pub extern "C" fn isxdigit_l(c: i32, _l: *mut c_void) -> i32 {
    isxdigit(c)
}
// Bits 12 and 13 of the generated classes mark zero- and two-column characters.
fn width(c: u32, encoding: Charset) -> i32 {
    if c == 0 {
        return 0;
    }
    let f = flags(c, encoding);
    if has(c, 8, encoding) == 0 {
        -1
    } else if f & (1 << 12) != 0 {
        0
    } else if f & (1 << 13) != 0 {
        2
    } else {
        1
    }
}
/// POSIX.1-2024 wcwidth: columns for a printable character, -1 otherwise.
#[no_mangle]
pub extern "C" fn wcwidth(c: u32) -> i32 {
    width(c, charset())
}
/// POSIX.1-2024 wcswidth: total columns of up to n characters, -1 if any is not printable.
#[no_mangle]
pub unsafe extern "C" fn wcswidth(s: *const u32, n: usize) -> i32 {
    let encoding = charset();
    let mut total: i32 = 0;
    let mut i = 0;
    while i < n && *s.add(i) != 0 {
        let w = width(*s.add(i), encoding);
        if w < 0 {
            return -1;
        }
        total = total.saturating_add(w);
        i += 1;
    }
    total
}
