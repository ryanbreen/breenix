//! ISO C17 7.22/7.29; POSIX.1-2024 locale and wide-character interfaces.
//! All implementation is original; encoding follows Unicode's UTF-8 definition.

mod multibyte;
mod objects;
mod classify;
mod unicode_tables;
mod wide;
mod calendar;
#[cfg(not(test))]
mod streams;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct MbState {
    value: u32,
    expected: u8,
    seen: u8,
    lead: u8,
    reserved: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Charset { Byte, Utf8 }

pub(crate) fn charset() -> Charset {
    objects::encoding()
}

pub(crate) fn encode(wc: u32, out: &mut [u8; 4]) -> Option<usize> {
    encode_charset(wc, out, charset())
}

fn encode_charset(wc: u32, out: &mut [u8; 4], encoding: Charset) -> Option<usize> {
    match encoding {
        Charset::Byte => {
            if wc > 0x7f { return None; }
            out[0] = wc as u8;
            Some(1)
        }
        Charset::Utf8 => {
            let c = char::from_u32(wc)?;
            Some(c.encode_utf8(out).len())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Decoded { Char(u32, usize), Incomplete(usize), Invalid }

pub(crate) fn decode(b: &[u8], st: &mut MbState) -> Decoded {
    decode_charset(b, st, charset())
}

fn decode_charset(b: &[u8], st: &mut MbState, encoding: Charset) -> Decoded {
    if b.is_empty() { return Decoded::Incomplete(0); }
    if encoding == Charset::Byte {
        *st = MbState::default();
        return Decoded::Char(b[0] as u32, 1);
    }
    for (i, &byte) in b.iter().enumerate() {
        if st.expected == 0 {
            if byte < 0x80 { return Decoded::Char(byte as u32, i + 1); }
            st.expected = match byte { 0xc2..=0xdf => 2, 0xe0..=0xef => 3, 0xf0..=0xf4 => 4, _ => {
                *st = MbState::default(); return Decoded::Invalid;
            }};
            st.value = (byte & (0x7f >> st.expected)) as u32;
            st.lead = byte;
            st.seen = 1;
        } else {
            // Reject invalid prefixes immediately, including split sequences.
            let valid = (0x80..=0xbf).contains(&byte) && (st.seen != 1 || match st.lead {
                0xe0 => byte >= 0xa0, 0xed => byte < 0xa0,
                0xf0 => byte >= 0x90, 0xf4 => byte < 0x90, _ => true,
            });
            if !valid { *st = MbState::default(); return Decoded::Invalid; }
            st.value = (st.value << 6) | (byte & 0x3f) as u32;
            st.seen += 1;
            if st.seen == st.expected {
                let wc = st.value;
                *st = MbState::default();
                return Decoded::Char(wc, i + 1);
            }
        }
    }
    Decoded::Incomplete(b.len())
}

#[no_mangle]
pub extern "C" fn __breenix_mb_cur_max() -> usize {
    if charset() == Charset::Utf8 { 4 } else { 1 }
}

fn set_errno(e: i32) { crate::pthread::set_errno(e); }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_all_scalars_round_trip_and_resume() {
        assert_eq!(core::mem::size_of::<MbState>(), 8);
        for value in 0..=0x10ffff {
            let Some(c) = char::from_u32(value) else { continue; };
            let mut bytes = [0; 4];
            let b = c.encode_utf8(&mut bytes).as_bytes();
            assert_eq!(decode_charset(b, &mut MbState::default(), Charset::Utf8), Decoded::Char(value, b.len()));
            for split in 1..b.len() {
                let mut st = MbState::default();
                assert_eq!(decode_charset(&b[..split], &mut st, Charset::Utf8), Decoded::Incomplete(split));
                assert_eq!(decode_charset(&b[split..], &mut st, Charset::Utf8), Decoded::Char(value, b.len() - split));
                assert_eq!(st, MbState::default());
            }
        }
    }

    #[test]
    fn rejects_invalid_prefixes_and_preserves_byte_locale() {
        for b in [&[0xc0, 0x80][..], &[0xed, 0xa0], &[0xf4, 0x90], &[0xf5], &[0xe0, 0x9f], &[0xf0, 0x8f], &[0x80]] {
            assert_eq!(decode_charset(b, &mut MbState::default(), Charset::Utf8), Decoded::Invalid);
        }
        for b in 0..=255 {
            assert_eq!(decode_charset(&[b], &mut MbState::default(), Charset::Byte), Decoded::Char(b as u32, 1));
        }
        assert_eq!(decode_charset(&[], &mut MbState::default(), Charset::Utf8), Decoded::Incomplete(0));
    }

    #[test]
    fn locale_isolation_conversion_limits_and_unicode_classes() {
        unsafe {
            let l = objects::newlocale(1, b"C.UTF-8\0".as_ptr(), core::ptr::null_mut());
            assert!(!l.is_null());
            assert_eq!(objects::uselocale(l) as usize, usize::MAX);
            assert_eq!(charset(), Charset::Utf8);
            assert_eq!(encode(0x10ffff, &mut [0; 4]), Some(4));
            assert_eq!(encode(0xd800, &mut [0; 4]), None);
            let src = b"a\xc3\xa9\0".as_ptr();
            let mut source = src;
            let mut state = MbState::default();
            assert_eq!(multibyte::mbsrtowcs(core::ptr::null_mut(), &mut source, 0, &mut state), 2);
            assert_eq!(source, src);
            let mut output = [0; 4];
            assert_eq!(multibyte::mbsrtowcs(output.as_mut_ptr(), &mut source, 1, &mut state), 1);
            assert_eq!(source, src.add(1));
            assert_eq!(multibyte::mbsrtowcs(output.as_mut_ptr(), &mut source, 4, &mut state), 1);
            assert_eq!(output[..2], [0xe9, 0]);
            assert!(source.is_null());
            let wide = [0xe9, 0];
            let mut source = wide.as_ptr();
            let mut bytes = [0x55; 4];
            assert_eq!(multibyte::wcsrtombs(bytes.as_mut_ptr(), &mut source, 1, &mut state), 0);
            assert_eq!(source, wide.as_ptr());
            assert_eq!(bytes, [0x55; 4]);
            assert_eq!(multibyte::wcsrtombs(bytes.as_mut_ptr(), &mut source, 4, &mut state), 2);
            assert_eq!(&bytes[..3], b"\xc3\xa9\0");
            assert!(source.is_null());
            assert_eq!(classify::iswalpha(0xe9), 1);
            assert_eq!(classify::iswdigit(0x663), 0);
            assert_eq!(classify::towupper(0xe9), 0xc9);
            assert_eq!(classify::towlower(0x391), 0x3b1);
            assert_eq!(std::thread::spawn(|| (charset(), classify::iswalpha(0xe9))).join().unwrap(), (Charset::Byte, 0));
            objects::uselocale(usize::MAX as *mut core::ffi::c_void);
            objects::freelocale(l);
            assert_eq!(charset(), Charset::Byte);
        }
    }

    #[test]
    fn wide_strings_pad_move_search_and_tokenize() {
        unsafe {
            let mut w = [1, 2, 3, 4, 5, 0];
            wide::wmemmove(w.as_mut_ptr().add(1), w.as_ptr(), 4);
            assert_eq!(w, [1, 1, 2, 3, 4, 0]);
            wide::wmemmove(w.as_mut_ptr(), w.as_ptr().add(1), 4);
            assert_eq!(w[..4], [1, 2, 3, 4]);
            wide::wcsncpy(w.as_mut_ptr(), [65, 0].as_ptr(), 6);
            assert_eq!(w, [65, 0, 0, 0, 0, 0]);
            assert_eq!(wide::wcschr(w.as_ptr(), 0), w.as_mut_ptr().add(1));
            assert_eq!(wide::wcsstr(w.as_ptr(), [0].as_ptr()), w.as_mut_ptr());
            let mut words = [44, 97, 44, 44, 98, 0];
            let mut save = core::ptr::null_mut();
            let sep = [44, 0];
            assert_eq!(wide::wcstok(words.as_mut_ptr(), sep.as_ptr(), &mut save), words.as_mut_ptr().add(1));
            assert_eq!(wide::wcstok(core::ptr::null_mut(), sep.as_ptr(), &mut save), words.as_mut_ptr().add(4));
            assert!(wide::wcstok(core::ptr::null_mut(), sep.as_ptr(), &mut save).is_null());
        }
    }

    #[test]
    fn calendar_known_epochs_leap_centuries_and_normalization() {
        unsafe {
            let mut tm = calendar::Tm::default();
            for (epoch, date) in [(0, (1970,1,1,4)), (-1,(1969,12,31,3)),
                (951782400,(2000,2,29,2)), (-2203977600,(1900,2,28,3)),
                (4107542400,(2100,3,1,1))] {
                assert!(!calendar::gmtime_r(&epoch,&mut tm).is_null());
                assert_eq!((tm.year+1900,tm.mon+1,tm.mday,tm.wday),date);
                assert_eq!(calendar::timegm(&mut tm),epoch);
            }
            tm = calendar::Tm { year: 120, mon: 12, mday: 0, hour: 25, ..Default::default() };
            calendar::timegm(&mut tm);
            assert_eq!((tm.year,tm.mon,tm.mday,tm.hour),(121,0,1,1));
            let epoch = 1609459200;
            calendar::gmtime_r(&epoch,&mut tm);
            let mut b=[0u8;64];
            let n=calendar::strftime(b.as_mut_ptr(),64,b"%F %T %G-W%V-%u %z\0".as_ptr(),&tm);
            assert_eq!(&b[..n],b"2021-01-01 00:00:00 2020-W53-5 +0000");
            assert_eq!(calendar::strftime(b.as_mut_ptr(),4,b"%F\0".as_ptr(),&tm),0);
            assert!(!calendar::asctime_r(&tm,b.as_mut_ptr()).is_null());
            assert_eq!(&b[..26],b"Fri Jan  1 00:00:00 2021\n\0");
            let mut wide=[0u32;32];
            let fmt=[0x20ac,32,37,70,0];
            assert_eq!(calendar::wcsftime(wide.as_mut_ptr(),32,fmt.as_ptr(),&tm),12);
            assert_eq!(wide[0],0x20ac);
            assert_eq!(wide[2],b'2' as u32);
            assert_eq!(wide[12],0);
            assert!(calendar::gmtime_r(&i64::MAX,&mut tm).is_null());
        }
    }
}
