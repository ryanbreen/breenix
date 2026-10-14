#!/usr/bin/env python3
"""Focused tests for the authorized LOCALE bounds/round-trip backport."""
from pathlib import Path
import os
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='libc-locale-bounds-') as tmp:
    directory = Path(tmp)
    for name in ('mod', 'multibyte'):
        text = (ROOT / 'libs/libbreenix-libc/src/locale' / (name + '.rs')).read_text()
        text = text.replace('#[no_mangle]', '').replace('mod multibyte;', 'pub(crate) mod multibyte;')
        if name == 'mod':
            text += '\npub fn test_charset_utf8(v: bool) { UTF8.store(v, Ordering::Relaxed); }\n'
        (directory / (name + '.rs')).write_text(text)
    src = directory / 'test.rs'
    src.write_text(r'''
#![deny(warnings)]
thread_local!(static ERRNO: std::cell::Cell<i32> = const { std::cell::Cell::new(0) });
#[no_mangle]
pub extern "C" fn __errno_location() -> *mut i32 { ERRNO.with(|e| e.as_ptr()) }
#[allow(dead_code)] // Complete C APIs outside this focused bounds test are intentionally retained.
#[path="__TMP__/mod.rs"] mod locale;
#[test]
fn enormous_n_stops_at_the_first_character() {
    unsafe {
        let mut wc = 99;
        let mut st = locale::MbState::default();
        assert_eq!(locale::multibyte::mbrtowc(&mut wc, b"\0".as_ptr(), usize::MAX, &mut st), 0);
        assert_eq!(wc, 0);
        assert_eq!(locale::multibyte::mbrlen(b"\0".as_ptr(), usize::MAX, &mut st), 0);
        assert_eq!(locale::multibyte::mbtowc(&mut wc, b"\0".as_ptr(), usize::MAX), 0);
        locale::test_charset_utf8(true);
        assert_eq!(locale::multibyte::mbrtowc(&mut wc, [0xe2u8].as_ptr(), 1, &mut st), usize::MAX-1);
        assert_eq!(locale::multibyte::mbrtowc(&mut wc, [0x82u8,0xac].as_ptr(), usize::MAX, &mut st), 2);
        assert_eq!(wc, 0x20ac);
        locale::test_charset_utf8(false);
    }
}
#[test]
fn every_c_byte_round_trips() {
    use locale::multibyte::*;
    for c in 0..=255 {
        assert_eq!(wctob(btowc(c)), c);
        let mut out = [0u8;4];
        let mut st = locale::MbState::default();
        assert_eq!(unsafe { wcrtomb(out.as_mut_ptr(), c as u32, &mut st) }, 1);
        assert_eq!(out[0], c as u8);
    }
    assert_eq!(wctob(256), -1);
}
'''.replace('__TMP__', str(directory)))
    exe = directory / 'test'
    subprocess.run(['rustc','--edition=2021','--test',str(src),'-o',str(exe)], check=True,
                   env=dict(os.environ, CARGO_BUILD_JOBS='6'))
    subprocess.run([str(exe),'--test-threads=1'], check=True)
