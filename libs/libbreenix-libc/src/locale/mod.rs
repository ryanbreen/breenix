//! ISO C17 7.22/7.29; POSIX.1-2024 locale and wide-character interfaces.
//! All implementation is original; encoding follows Unicode's UTF-8 definition.

mod multibyte;

use core::sync::atomic::{AtomicBool, Ordering};

static UTF8: AtomicBool = AtomicBool::new(false);

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
    if UTF8.load(Ordering::Relaxed) { Charset::Utf8 } else { Charset::Byte }
}

pub(crate) fn encode(wc: u32, out: &mut [u8; 4]) -> Option<usize> {
    match charset() {
        // The C locale maps each byte to the same wide character and back
        // (C17 7.29.6.1.2), including 0x80-0xFF.
        Charset::Byte => {
            if wc > 0xff { return None; }
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

fn set_errno(e: i32) { unsafe { *crate::__errno_location() = e; } }

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
}
