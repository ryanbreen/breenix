use super::{charset, decode, encode, set_errno, Charset, Decoded, MbState};
use core::ptr::{null, null_mut};

// ISO C specifies separate internal states for these interfaces when ps is null.
static mut READ_STATE: MbState = MbState {
    value: 0,
    expected: 0,
    seen: 0,
    lead: 0,
    reserved: 0,
};
static mut LENGTH_STATE: MbState = MbState {
    value: 0,
    expected: 0,
    seen: 0,
    lead: 0,
    reserved: 0,
};
static mut WRITE_STATE: MbState = MbState {
    value: 0,
    expected: 0,
    seen: 0,
    lead: 0,
    reserved: 0,
};
static mut STRING_READ_STATE: MbState = MbState {
    value: 0,
    expected: 0,
    seen: 0,
    lead: 0,
    reserved: 0,
};
static mut STRING_WRITE_STATE: MbState = MbState {
    value: 0,
    expected: 0,
    seen: 0,
    lead: 0,
    reserved: 0,
};

#[no_mangle]
pub unsafe extern "C" fn mbsinit(st: *const MbState) -> i32 {
    i32::from(st.is_null() || (*st).expected == 0)
}

#[no_mangle]
pub unsafe extern "C" fn mbrtowc(wc: *mut u32, s: *const u8, n: usize, ps: *mut MbState) -> usize {
    let st = if ps.is_null() {
        &mut *core::ptr::addr_of_mut!(READ_STATE)
    } else {
        &mut *ps
    };
    let n = if s.is_null() { 1 } else { n };
    for i in 0..n {
        let byte = if s.is_null() { 0 } else { *s.add(i) };
        match decode(&[byte], st) {
            Decoded::Char(c, count) => {
                if !wc.is_null() && !s.is_null() {
                    *wc = c;
                }
                return if c == 0 { 0 } else { i + count };
            }
            Decoded::Incomplete(_) => continue,
            Decoded::Invalid => {
                set_errno(84);
                return usize::MAX;
            }
        }
    }
    usize::MAX - 1
}

#[no_mangle]
pub unsafe extern "C" fn mbrlen(s: *const u8, n: usize, ps: *mut MbState) -> usize {
    mbrtowc(
        null_mut(),
        s,
        n,
        if ps.is_null() {
            core::ptr::addr_of_mut!(LENGTH_STATE)
        } else {
            ps
        },
    )
}

#[no_mangle]
pub unsafe extern "C" fn wcrtomb(s: *mut u8, wc: u32, ps: *mut MbState) -> usize {
    let st = if ps.is_null() {
        core::ptr::addr_of_mut!(WRITE_STATE)
    } else {
        ps
    };
    let mut bytes = [0; 4];
    match encode(if s.is_null() { 0 } else { wc }, &mut bytes) {
        Some(n) => {
            *st = MbState::default();
            if !s.is_null() {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), s, n);
            }
            n
        }
        None => {
            set_errno(84);
            usize::MAX
        }
    }
}

/// mbsrtowcs with POSIX.1-2024 mbsnrtowcs's limit of nms source bytes. A character
/// cut by the limit stays in the state, and its bytes count as consumed.
unsafe fn to_wide(
    dst: *mut u32,
    src: *mut *const u8,
    nms: usize,
    len: usize,
    ps: *mut MbState,
) -> usize {
    let mut local = *ps;
    let st = if dst.is_null() { &mut local } else { &mut *ps };
    let mut s = *src;
    let mut used = 0;
    let mut count = 0;
    while dst.is_null() || count < len {
        // Never read past the terminator or the limit; decode a byte at a time.
        let mut consumed = 0;
        let c = loop {
            if used + consumed == nms {
                if !dst.is_null() {
                    *src = s.add(consumed);
                }
                return count;
            }
            let byte = *s.add(consumed);
            consumed += 1;
            match decode(&[byte], st) {
                Decoded::Char(c, _) => break c,
                Decoded::Incomplete(_) => continue,
                Decoded::Invalid => {
                    if !dst.is_null() {
                        *src = s;
                    }
                    set_errno(84);
                    return usize::MAX;
                }
            }
        };
        if !dst.is_null() {
            *dst.add(count) = c;
        }
        if c == 0 {
            if !dst.is_null() {
                *src = null();
            }
            return count;
        }
        s = s.add(consumed);
        used += consumed;
        count += 1;
    }
    *src = s;
    count
}
#[no_mangle]
pub unsafe extern "C" fn mbsrtowcs(
    dst: *mut u32,
    src: *mut *const u8,
    len: usize,
    ps: *mut MbState,
) -> usize {
    let ps = if ps.is_null() {
        core::ptr::addr_of_mut!(STRING_READ_STATE)
    } else {
        ps
    };
    to_wide(dst, src, usize::MAX, len, ps)
}
#[no_mangle]
pub unsafe extern "C" fn mbsnrtowcs(
    dst: *mut u32,
    src: *mut *const u8,
    nms: usize,
    len: usize,
    ps: *mut MbState,
) -> usize {
    let ps = if ps.is_null() {
        core::ptr::addr_of_mut!(STRING_READ_STATE)
    } else {
        ps
    };
    to_wide(dst, src, nms, len, ps)
}

/// wcsrtombs with POSIX.1-2024 wcsnrtombs's limit of nwc source characters.
unsafe fn to_bytes(
    dst: *mut u8,
    src: *mut *const u32,
    nwc: usize,
    len: usize,
    ps: *mut MbState,
) -> usize {
    let mut s = *src;
    let mut read = 0;
    let mut count = 0;
    while dst.is_null() || count < len {
        if read == nwc {
            break;
        }
        let mut bytes = [0; 4];
        let Some(n) = encode(*s, &mut bytes) else {
            if !dst.is_null() {
                *src = s;
            }
            set_errno(84);
            return usize::MAX;
        };
        if !dst.is_null() {
            if n > len - count {
                break;
            }
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst.add(count), n);
            *ps = MbState::default();
        }
        if *s == 0 {
            if !dst.is_null() {
                *src = null();
            }
            return count;
        }
        count += n;
        s = s.add(1);
        read += 1;
    }
    if !dst.is_null() {
        *src = s;
    }
    count
}
#[no_mangle]
pub unsafe extern "C" fn wcsrtombs(
    dst: *mut u8,
    src: *mut *const u32,
    len: usize,
    ps: *mut MbState,
) -> usize {
    let ps = if ps.is_null() {
        core::ptr::addr_of_mut!(STRING_WRITE_STATE)
    } else {
        ps
    };
    to_bytes(dst, src, usize::MAX, len, ps)
}
#[no_mangle]
pub unsafe extern "C" fn wcsnrtombs(
    dst: *mut u8,
    src: *mut *const u32,
    nwc: usize,
    len: usize,
    ps: *mut MbState,
) -> usize {
    let ps = if ps.is_null() {
        core::ptr::addr_of_mut!(STRING_WRITE_STATE)
    } else {
        ps
    };
    to_bytes(dst, src, nwc, len, ps)
}

#[no_mangle]
pub unsafe extern "C" fn mbstowcs(dst: *mut u32, s: *const u8, n: usize) -> usize {
    mbsrtowcs(dst, &mut { s }, n, &mut MbState::default())
}
#[no_mangle]
pub unsafe extern "C" fn wcstombs(dst: *mut u8, s: *const u32, n: usize) -> usize {
    wcsrtombs(dst, &mut { s }, n, &mut MbState::default())
}
#[no_mangle]
pub unsafe extern "C" fn mbtowc(dst: *mut u32, s: *const u8, n: usize) -> i32 {
    if s.is_null() {
        return 0;
    }
    let r = mbrtowc(dst, s, n, &mut MbState::default());
    if r >= usize::MAX - 1 {
        set_errno(84);
        -1
    } else {
        r as i32
    }
}
#[no_mangle]
pub unsafe extern "C" fn mblen(s: *const u8, n: usize) -> i32 {
    mbtowc(null_mut(), s, n)
}
#[no_mangle]
pub unsafe extern "C" fn wctomb(s: *mut u8, wc: u32) -> i32 {
    if s.is_null() {
        return 0;
    }
    wcrtomb(s, wc, &mut MbState::default()) as i32
}
#[no_mangle]
pub extern "C" fn btowc(c: i32) -> u32 {
    if (0..=127).contains(&c) || (charset() == Charset::Byte && (0..=255).contains(&c)) {
        c as u32
    } else {
        u32::MAX
    }
}
#[no_mangle]
pub extern "C" fn wctob(c: u32) -> i32 {
    // A single byte iff the current charset encodes c in one byte.
    let mut b = [0; 4];
    match encode(c, &mut b) {
        Some(1) => b[0] as i32,
        _ => -1,
    }
}
