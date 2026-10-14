//! ISO C17 7.22/7.29; POSIX.1-2024 locale and wide-character interfaces.
//! All implementation is original; encoding follows Unicode's UTF-8 definition.

mod calendar;
mod classify;
#[cfg(not(test))]
mod clocks;
#[cfg(not(test))]
mod format;
mod multibyte;
#[cfg(not(test))]
mod numbers;
mod objects;
mod parse_time;
#[cfg(not(test))]
mod streams;
mod unicode_tables;
mod wide;

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
pub(crate) enum Charset {
    Byte,
    Utf8,
}

pub(crate) fn charset() -> Charset {
    objects::encoding()
}

pub(crate) fn encode(wc: u32, out: &mut [u8; 4]) -> Option<usize> {
    encode_charset(wc, out, charset())
}

fn encode_charset(wc: u32, out: &mut [u8; 4], encoding: Charset) -> Option<usize> {
    match encoding {
        // The single-byte C locale maps each byte to the wide character of the same
        // value, so exactly 0x00-0xFF encode (C17 7.29.6.1.2 wctob(btowc(c)) == c).
        Charset::Byte => {
            if wc > 0xff {
                return None;
            }
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
pub(crate) enum Decoded {
    Char(u32, usize),
    Incomplete(usize),
    Invalid,
}

pub(crate) fn decode(b: &[u8], st: &mut MbState) -> Decoded {
    decode_charset(b, st, charset())
}

fn decode_charset(b: &[u8], st: &mut MbState, encoding: Charset) -> Decoded {
    if b.is_empty() {
        return Decoded::Incomplete(0);
    }
    if encoding == Charset::Byte {
        *st = MbState::default();
        return Decoded::Char(b[0] as u32, 1);
    }
    for (i, &byte) in b.iter().enumerate() {
        if st.expected == 0 {
            if byte < 0x80 {
                return Decoded::Char(byte as u32, i + 1);
            }
            st.expected = match byte {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => {
                    *st = MbState::default();
                    return Decoded::Invalid;
                }
            };
            st.value = (byte & (0x7f >> st.expected)) as u32;
            st.lead = byte;
            st.seen = 1;
        } else {
            // Reject invalid prefixes immediately, including split sequences.
            let valid = (0x80..=0xbf).contains(&byte)
                && (st.seen != 1
                    || match st.lead {
                        0xe0 => byte >= 0xa0,
                        0xed => byte < 0xa0,
                        0xf0 => byte >= 0x90,
                        0xf4 => byte < 0x90,
                        _ => true,
                    });
            if !valid {
                *st = MbState::default();
                return Decoded::Invalid;
            }
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
    if charset() == Charset::Utf8 {
        4
    } else {
        1
    }
}

fn set_errno(e: i32) {
    crate::pthread::set_errno(e);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_all_scalars_round_trip_and_resume() {
        assert_eq!(core::mem::size_of::<MbState>(), 8);
        for value in 0..=0x10ffff {
            let Some(c) = char::from_u32(value) else {
                continue;
            };
            let mut bytes = [0; 4];
            let b = c.encode_utf8(&mut bytes).as_bytes();
            assert_eq!(
                decode_charset(b, &mut MbState::default(), Charset::Utf8),
                Decoded::Char(value, b.len())
            );
            for split in 1..b.len() {
                let mut st = MbState::default();
                assert_eq!(
                    decode_charset(&b[..split], &mut st, Charset::Utf8),
                    Decoded::Incomplete(split)
                );
                assert_eq!(
                    decode_charset(&b[split..], &mut st, Charset::Utf8),
                    Decoded::Char(value, b.len() - split)
                );
                assert_eq!(st, MbState::default());
            }
        }
    }

    #[test]
    fn rejects_invalid_prefixes_and_preserves_byte_locale() {
        for b in [
            &[0xc0, 0x80][..],
            &[0xed, 0xa0],
            &[0xf4, 0x90],
            &[0xf5],
            &[0xe0, 0x9f],
            &[0xf0, 0x8f],
            &[0x80],
        ] {
            assert_eq!(
                decode_charset(b, &mut MbState::default(), Charset::Utf8),
                Decoded::Invalid
            );
        }
        for b in 0..=255 {
            assert_eq!(
                decode_charset(&[b], &mut MbState::default(), Charset::Byte),
                Decoded::Char(b as u32, 1)
            );
            let mut out = [0; 4];
            assert_eq!(encode_charset(b as u32, &mut out, Charset::Byte), Some(1));
            assert_eq!(out[0], b);
        }
        assert_eq!(encode_charset(0x100, &mut [0; 4], Charset::Byte), None);
        assert_eq!(encode_charset(0x20ac, &mut [0; 4], Charset::Byte), None);
        assert_eq!(
            decode_charset(&[], &mut MbState::default(), Charset::Utf8),
            Decoded::Incomplete(0)
        );
    }

    #[test]
    fn locale_isolation_conversion_limits_and_unicode_classes() {
        unsafe {
            let l = objects::newlocale(1 << 2, b"C.UTF-8\0".as_ptr(), core::ptr::null_mut());
            assert!(!l.is_null());
            assert_eq!(objects::uselocale(l) as usize, usize::MAX);
            assert_eq!(charset(), Charset::Utf8);
            assert_eq!(encode(0x10ffff, &mut [0; 4]), Some(4));
            assert_eq!(encode(0xd800, &mut [0; 4]), None);
            let src = b"a\xc3\xa9\0".as_ptr();
            let mut source = src;
            let mut state = MbState::default();
            assert_eq!(
                multibyte::mbsrtowcs(core::ptr::null_mut(), &mut source, 0, &mut state),
                2
            );
            assert_eq!(source, src);
            let mut output = [0; 4];
            assert_eq!(
                multibyte::mbsrtowcs(output.as_mut_ptr(), &mut source, 1, &mut state),
                1
            );
            assert_eq!(source, src.add(1));
            assert_eq!(
                multibyte::mbsrtowcs(output.as_mut_ptr(), &mut source, 4, &mut state),
                1
            );
            assert_eq!(output[..2], [0xe9, 0]);
            assert!(source.is_null());
            let wide = [0xe9, 0];
            let mut source = wide.as_ptr();
            let mut bytes = [0x55; 4];
            assert_eq!(
                multibyte::wcsrtombs(bytes.as_mut_ptr(), &mut source, 1, &mut state),
                0
            );
            assert_eq!(source, wide.as_ptr());
            assert_eq!(bytes, [0x55; 4]);
            assert_eq!(
                multibyte::wcsrtombs(bytes.as_mut_ptr(), &mut source, 4, &mut state),
                2
            );
            assert_eq!(&bytes[..3], b"\xc3\xa9\0");
            assert!(source.is_null());
            assert_eq!(classify::iswalpha(0xe9), 1);
            assert_eq!(classify::iswdigit(0x663), 0);
            assert_eq!(classify::towupper(0xe9), 0xc9);
            assert_eq!(classify::towlower(0x391), 0x3b1);
            assert_eq!(
                std::thread::spawn(|| (charset(), classify::iswalpha(0xe9)))
                    .join()
                    .unwrap(),
                (Charset::Byte, 0)
            );
            objects::uselocale(usize::MAX as *mut core::ffi::c_void);
            objects::freelocale(l);
            assert_eq!(charset(), Charset::Byte);
        }
    }

    #[test]
    fn c_locale_round_trip_case_pairs_and_widths() {
        for c in 0..=255u32 {
            assert_eq!(multibyte::wctob(multibyte::btowc(c as i32)), c as i32);
        }
        assert_eq!(multibyte::wctob(0x100), -1);
        // Titlecase letters are neither upper nor lower, so ISO C maps them to themselves.
        let l = unsafe { objects::newlocale(1 << 2, b"C.UTF-8\0".as_ptr(), core::ptr::null_mut()) };
        unsafe {
            objects::uselocale(l);
            for c in [0x1c5, 0x1c8, 0x1cb, 0x1f2, 0x1f88, 0x1fbc] {
                assert_eq!((classify::towupper(c), classify::towlower(c)), (c, c));
            }
            assert_eq!(classify::towupper(0x1c6), 0x1c4);
            assert_eq!(classify::towlower(0x1c4), 0x1c6);
            for c in 0..=0x10ffffu32 {
                let up = classify::towupper(c);
                if up != c {
                    assert!(classify::iswlower(c) != 0 && classify::iswupper(up) != 0, "{c:x}");
                }
                let down = classify::towlower(c);
                if down != c {
                    assert!(classify::iswupper(c) != 0 && classify::iswlower(down) != 0, "{c:x}");
                }
            }
            // Format characters such as U+200B are not printable here, so they are -1;
            // U+3164 is a printable default-ignorable character, so it is zero.
            for (c, w) in [(0, 0), (0x41, 1), (0xe9, 1), (0x301, 0), (0x3164, 0), (0x200b, -1), (0x4e00, 2), (0x1f600, 2), (7, -1), (0xd800, -1)] {
                assert_eq!(classify::wcwidth(c), w, "{c:x}");
            }
            let text = [0x41, 0x4e00, 0x301, 0];
            assert_eq!(classify::wcswidth(text.as_ptr(), 8), 3);
            assert_eq!(classify::wcswidth(text.as_ptr(), 1), 1);
            assert_eq!(classify::wcswidth([0x41, 7, 0].as_ptr(), 8), -1);
            // mbsnrtowcs keeps a character cut by its byte limit in the state.
            let src = b"a\xe2\x82\xacb\0";
            let mut p = src.as_ptr();
            let mut st = MbState::default();
            let mut out = [0u32; 8];
            assert_eq!(multibyte::mbsnrtowcs(out.as_mut_ptr(), &mut p, 3, 8, &mut st), 1);
            assert_eq!(p, src.as_ptr().add(3));
            assert_eq!(multibyte::mbsinit(&st), 0);
            assert_eq!(multibyte::mbsnrtowcs(out.as_mut_ptr().add(1), &mut p, 8, 8, &mut st), 2);
            assert_eq!(out[..4], [0x61, 0x20ac, 0x62, 0]);
            assert!(p.is_null());
            let wide = [0x61, 0x20ac, 0x62, 0];
            let mut q = wide.as_ptr();
            let mut bytes = [0u8; 8];
            assert_eq!(multibyte::wcsnrtombs(bytes.as_mut_ptr(), &mut q, 2, 8, &mut st), 4);
            assert_eq!(q, wide.as_ptr().add(2));
            assert_eq!(multibyte::wcsnrtombs(core::ptr::null_mut(), &mut q, 8, 0, &mut st), 1);
            objects::uselocale(usize::MAX as *mut core::ffi::c_void);
            assert_eq!(classify::wcwidth(0xe9), -1);
            objects::freelocale(l);
        }
    }

    #[test]
    fn posix_wide_copies_and_bounded_concatenation() {
        unsafe {
            let mut d = [9u32; 8];
            let end = wide::wcpcpy(d.as_mut_ptr(), [1, 2, 0].as_ptr());
            assert_eq!(end, d.as_mut_ptr().add(2));
            assert_eq!(d[..4], [1, 2, 0, 9]);
            let end = wide::wcpncpy(d.as_mut_ptr(), [3, 0].as_ptr(), 4);
            assert_eq!((end, d[..5].to_vec()), (d.as_mut_ptr().add(1), vec![3, 0, 0, 0, 9]));
            let end = wide::wcpncpy(d.as_mut_ptr(), [5, 6, 7, 0].as_ptr(), 2);
            assert_eq!((end, d[..3].to_vec()), (d.as_mut_ptr().add(2), vec![5, 6, 0]));
            assert_eq!(wide::wcslcpy(d.as_mut_ptr(), [1, 2, 3, 0].as_ptr(), 3), 3);
            assert_eq!(d[..3], [1, 2, 0]);
            assert_eq!(wide::wcslcat(d.as_mut_ptr(), [4, 5, 0].as_ptr(), 4), 4);
            assert_eq!(d[..4], [1, 2, 4, 0]);
            assert_eq!(wide::wcslcat(d.as_mut_ptr(), [4, 0].as_ptr(), 2), 3);
            assert_eq!(wide::wcslcpy(d.as_mut_ptr(), [1, 0].as_ptr(), 0), 1);
            assert_eq!(wide::wcscasecmp_l([0x41, 0].as_ptr(), [0x61, 0].as_ptr(), core::ptr::null_mut()), 0);
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
            assert_eq!(
                wide::wcstok(words.as_mut_ptr(), sep.as_ptr(), &mut save),
                words.as_mut_ptr().add(1)
            );
            assert_eq!(
                wide::wcstok(core::ptr::null_mut(), sep.as_ptr(), &mut save),
                words.as_mut_ptr().add(4)
            );
            assert!(wide::wcstok(core::ptr::null_mut(), sep.as_ptr(), &mut save).is_null());
        }
    }

    #[test]
    fn calendar_known_epochs_leap_centuries_and_normalization() {
        unsafe {
            let mut tm = calendar::Tm::default();
            for (epoch, date) in [
                (0, (1970, 1, 1, 4)),
                (-1, (1969, 12, 31, 3)),
                (951782400, (2000, 2, 29, 2)),
                (-2203977600, (1900, 2, 28, 3)),
                (4107542400, (2100, 3, 1, 1)),
            ] {
                assert!(!calendar::gmtime_r(&epoch, &mut tm).is_null());
                assert_eq!((tm.year + 1900, tm.mon + 1, tm.mday, tm.wday), date);
                assert_eq!(calendar::timegm(&mut tm), epoch);
            }
            tm = calendar::Tm {
                year: 120,
                mon: 12,
                mday: 0,
                hour: 25,
                ..Default::default()
            };
            calendar::timegm(&mut tm);
            assert_eq!((tm.year, tm.mon, tm.mday, tm.hour), (121, 0, 1, 1));
            let epoch = 1609459200;
            calendar::gmtime_r(&epoch, &mut tm);
            let mut b = [0u8; 64];
            let n = calendar::strftime(b.as_mut_ptr(), 64, b"%F %T %G-W%V-%u %z\0".as_ptr(), &tm);
            assert_eq!(&b[..n], b"2021-01-01 00:00:00 2020-W53-5 +0000");
            assert_eq!(
                calendar::strftime(b.as_mut_ptr(), 4, b"%F\0".as_ptr(), &tm),
                0
            );
            assert!(!calendar::asctime_r(&tm, b.as_mut_ptr()).is_null());
            assert_eq!(&b[..26], b"Fri Jan  1 00:00:00 2021\n\0");
            let mut wide = [0u32; 32];
            let fmt = [0x20ac, 32, 37, 70, 0];
            assert_eq!(
                calendar::wcsftime(wide.as_mut_ptr(), 32, fmt.as_ptr(), &tm),
                12
            );
            assert_eq!(wide[0], 0x20ac);
            assert_eq!(wide[2], b'2' as u32);
            assert_eq!(wide[12], 0);
            assert!(calendar::gmtime_r(&i64::MAX, &mut tm).is_null());
            // POSIX.1-2024 strftime year fields and the + flag.
            let mut b = [0u8; 64];
            for (year, fmt, text) in [
                (999, b"%Y %C %G\0".as_slice(), b"999 09 999".as_slice()),
                (1970, b"%+4Y %+5Y %+6Y %06Y\0", b"1970 +1970 +01970 001970"),
                (12345, b"%Y %+4Y %C %+C %+3C\0", b"12345 +12345 123 +123 +123"),
                (-5, b"%Y %C %05Y\0", b"-5 00 -0005"),
                (-150, b"%C %4C\0", b"-1 -001"),
                (1970, b"%F %10F %+12F %+F\0", b"1970-01-01 1970-01-01 +01970-01-01 1970-01-01"),
                (12345, b"%F\0", b"12345-01-01"),
            ] {
                tm = calendar::Tm { year: year - 1900, mday: 1, wday: 4, ..Default::default() };
                let n = calendar::strftime(b.as_mut_ptr(), 64, fmt.as_ptr(), &tm);
                assert_eq!(&b[..n], text, "{year}");
            }
            tm.isdst = -1;
            assert_eq!(calendar::strftime(b.as_mut_ptr(), 64, b"%z\0".as_ptr(), &tm), 0);
            assert_eq!(b[0], 0);
            let mut wide = [0u32; 16];
            let fmt = [37, 43, 54, 89, 0]; // %+6Y
            tm.year = 70;
            assert_eq!(calendar::wcsftime(wide.as_mut_ptr(), 16, fmt.as_ptr(), &tm), 6);
            assert_eq!(wide[..6], [43, 48, 49, 57, 55, 48]);
            // Week-based fields for any int year, without failing.
            for year in [i32::MIN, i32::MAX] {
                tm = calendar::Tm { year, yday: 0, wday: 1, ..Default::default() };
                assert_ne!(calendar::strftime(b.as_mut_ptr(), 64, b"%G %g %V\0".as_ptr(), &tm), 0);
            }
            tm = calendar::Tm { year: 99, yday: 1, wday: 6, ..Default::default() };
            let n = calendar::strftime(b.as_mut_ptr(), 64, b"%G %V\0".as_ptr(), &tm);
            assert_eq!(&b[..n], b"1998 53");
        }
    }

    #[test]
    fn fixed_offsets_round_trip_with_concurrent_calendar_calls() {
        unsafe {
            let old = std::env::var_os("TZ");
            std::env::set_var("TZ", "UTC+02:30");
            let mut tm = calendar::Tm::default();
            calendar::localtime_r(&0, &mut tm);
            assert_eq!(
                (tm.year, tm.mon, tm.mday, tm.hour, tm.min, tm.gmtoff),
                (69, 11, 31, 21, 30, -9000)
            );
            assert_eq!(calendar::mktime(&mut tm), 0);
            let jobs: Vec<_> = (0..4)
                .map(|_| {
                    std::thread::spawn(|| {
                        for epoch in [-86400, -1, 0, 86400, 951782400] {
                            let mut t = calendar::Tm::default();
                            calendar::localtime_r(&epoch, &mut t);
                            assert_eq!(t.gmtoff, -9000);
                            assert_eq!(calendar::mktime(&mut t), epoch);
                        }
                    })
                })
                .collect();
            for job in jobs {
                job.join().unwrap();
            }
            std::env::set_var("TZ", "GMT-05:45");
            calendar::localtime_r(&0, &mut tm);
            assert_eq!((tm.hour, tm.min, tm.gmtoff), (5, 45, 20700));
            assert_eq!(calendar::mktime(&mut tm), 0);
            std::env::set_var("TZ", "UTC+");
            calendar::localtime_r(&0, &mut tm);
            assert_eq!(tm.gmtoff, 0);
            // A dst part with no rule: Breenix's default M3.2.0,M11.1.0 at 02:00.
            std::env::set_var("TZ", "EST5EDT");
            let mut b = [0u8; 64];
            for (epoch, hour, isdst, gmtoff, zone) in [
                (1625140800, 8, 1, -14400, b"EDT %z=-0400".as_slice()),
                (1609502400, 7, 0, -18000, b"EST %z=-0500".as_slice()),
                // 2021-03-14 06:59:59 UTC is 01:59:59 EST; one second later is 03:00 EDT.
                (1615705199, 1, 0, -18000, b"EST %z=-0500".as_slice()),
                (1615705200, 3, 1, -14400, b"EDT %z=-0400".as_slice()),
                // 2021-11-07 05:59:59 UTC is 01:59:59 EDT; one second later is 01:00 EST.
                (1636264799, 1, 1, -14400, b"EDT %z=-0400".as_slice()),
                (1636264800, 1, 0, -18000, b"EST %z=-0500".as_slice()),
            ] {
                calendar::localtime_r(&epoch, &mut tm);
                assert_eq!((tm.hour, tm.isdst, tm.gmtoff), (hour, isdst, gmtoff), "{epoch}");
                let n = calendar::strftime(b.as_mut_ptr(), 64, b"%Z %%z=%z\0".as_ptr(), &tm);
                assert_eq!(&b[..n], zone);
                let mut copy = tm;
                assert_eq!(calendar::mktime(&mut copy), epoch);
                copy.isdst = -1;
                if epoch != 1636264799 {
                    assert_eq!(calendar::mktime(&mut copy), epoch);
                }
            }
            assert_eq!((calendar::timezone, calendar::daylight), (18000, 1));
            // An ambiguous wall time with tm_isdst < 0 reads as standard time, and a
            // skipped one reads as standard and normalizes into DST.
            tm = calendar::Tm { year: 121, mon: 10, mday: 7, hour: 1, min: 30, isdst: -1, ..Default::default() };
            assert_eq!(calendar::mktime(&mut tm), 1636266600);
            tm = calendar::Tm { year: 121, mon: 2, mday: 14, hour: 2, min: 30, isdst: -1, ..Default::default() };
            assert_eq!(calendar::mktime(&mut tm), 1615707000);
            assert_eq!((tm.hour, tm.isdst), (3, 1));
            // Southern hemisphere: DST spans the new year; quoted names; explicit times.
            std::env::set_var("TZ", "<+10>-10<+11>-11,M10.1.0,M4.1.0/3");
            calendar::localtime_r(&1609459200, &mut tm);
            assert_eq!((tm.hour, tm.isdst, tm.gmtoff), (11, 1, 39600));
            calendar::localtime_r(&1625140800, &mut tm);
            assert_eq!((tm.hour, tm.isdst, tm.gmtoff), (22, 0, 36000));
            let n = calendar::strftime(b.as_mut_ptr(), 64, b"%Z\0".as_ptr(), &tm);
            assert_eq!(&b[..n], b"+10");
            // POSIX's year-round example, and Jn/n dates with a negative time.
            std::env::set_var("TZ", "EST5EDT,0/0,J365/25");
            for epoch in [1609459200, 1625140800, 1640995199] {
                calendar::localtime_r(&epoch, &mut tm);
                assert_eq!(tm.isdst, 1, "{epoch}");
            }
            // DST ends on zero-based day 59 (29 Feb 2020) at 02:00 and starts again on
            // J60 (1 March) at -1:00, i.e. 23:00 on 29 February.
            std::env::set_var("TZ", "XXX3YYY,J60/-1,59");
            calendar::localtime_r(&1583020800, &mut tm); // 2020-03-01 00:00 UTC
            assert_eq!((tm.mon, tm.mday, tm.hour, tm.isdst), (1, 29, 21, 0));
            calendar::localtime_r(&1583029800, &mut tm); // 02:30 UTC, 23:30 standard
            assert_eq!((tm.mon, tm.mday, tm.hour, tm.isdst), (2, 1, 0, 1));
            calendar::localtime_r(&1582948800, &mut tm); // 2020-02-29 04:00 UTC
            assert_eq!((tm.mday, tm.hour, tm.isdst), (29, 1, 0));
            std::env::set_var("TZ", "EST5EDT,M3.2.0,M13.1.0");
            calendar::localtime_r(&0, &mut tm);
            assert_eq!((tm.gmtoff, tm.isdst), (0, 0));
            if let Some(old) = old {
                std::env::set_var("TZ", old);
            } else {
                std::env::remove_var("TZ");
            }
            calendar::tzset();
        }
    }

    #[test]
    fn strptime_dates_names_centuries_weeks_and_unconsumed_input() {
        unsafe {
            let mut t = calendar::Tm::default();
            let text = b"2024-02-29 23:59:60tail\0";
            let end = parse_time::strptime(text.as_ptr(), b"%F %T\0".as_ptr(), &mut t);
            assert_eq!(end, text.as_ptr().add(19) as *mut u8);
            assert_eq!(
                (t.year, t.mon, t.mday, t.hour, t.min, t.sec, t.yday, t.wday),
                (124, 1, 29, 23, 59, 60, 59, 4)
            );
            assert!(
                parse_time::strptime(b"2024-13-01\0".as_ptr(), b"%F\0".as_ptr(), &mut t).is_null()
            );
            assert!(!parse_time::strptime(
                b"tUe feb 29 2000\0".as_ptr(),
                b"%a %b %d %Y\0".as_ptr(),
                &mut t
            )
            .is_null());
            assert_eq!((t.year, t.mon, t.mday, t.wday), (100, 1, 29, 2));
            assert!(
                !parse_time::strptime(b"68 20\0".as_ptr(), b"%y %C\0".as_ptr(), &mut t).is_null()
            );
            assert_eq!(t.year, 168);
            assert!(!parse_time::strptime(b"69\0".as_ptr(), b"%y\0".as_ptr(), &mut t).is_null());
            assert_eq!(t.year, 69);
            t.sec = 42;
            assert!(
                !parse_time::strptime(b"PM 12:05\0".as_ptr(), b"%p %I:%M\0".as_ptr(), &mut t)
                    .is_null()
            );
            assert_eq!((t.hour, t.min, t.sec), (12, 5, 42));
            assert!(!parse_time::strptime(
                b"2020-W53-5\0".as_ptr(),
                b"%G-W%V-%u\0".as_ptr(),
                &mut t
            )
            .is_null());
            assert_eq!((t.year, t.mon, t.mday, t.wday), (121, 0, 1, 5));
            assert!(
                !parse_time::strptime(b"2020 060\0".as_ptr(), b"%Y %j\0".as_ptr(), &mut t)
                    .is_null()
            );
            assert_eq!((t.year, t.mon, t.mday, t.yday), (120, 1, 29, 59));
            assert!(
                !parse_time::strptime(b"+05:45\0".as_ptr(), b"%z\0".as_ptr(), &mut t).is_null()
            );
            assert_eq!(t.gmtoff, 20700);
            assert!(
                !parse_time::strptime(b"\x0b12\0".as_ptr(), b"\t%H\0".as_ptr(), &mut t).is_null()
            );
            assert_eq!(t.hour, 12);
        }
    }
}
