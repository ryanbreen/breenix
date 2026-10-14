#!/usr/bin/env python3
"""Focused tests for the authorized LOCALE bounds/round-trip backport.

Builds the whole locale module with the host services from tests/libc_locale.rs
(LOCALE's harness), exposing the multibyte and objects submodules to these cases.
"""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='libc-locale-bounds-') as tmp:
    directory = Path(tmp)
    # The module reads ../../include headers at compile time, so keep the crate layout.
    locale = directory / 'src/locale'
    shutil.copytree(ROOT / 'libs/libbreenix-libc/src/locale', locale)
    shutil.copytree(ROOT / 'libs/libbreenix-libc/include', directory / 'include')
    mod = locale / 'mod.rs'
    text = mod.read_text()
    for name in ('multibyte', 'objects'):
        text = text.replace(f'\nmod {name};', f'\npub(crate) mod {name};')
    mod.write_text(text)
    harness = (ROOT / 'tests/libc_locale.rs').read_text().replace(
        '#[path = "../libs/libbreenix-libc/src/locale/mod.rs"]', f'#[path = "{mod}"]')
    src = directory / 'test.rs'
    src.write_text(harness + r'''
// LC_CTYPE_MASK is 1 << LC_CTYPE (2) in include/locale.h.
fn c_utf8() -> *mut core::ffi::c_void {
    unsafe { locale::objects::newlocale(1 << 2, b"C.UTF-8\0".as_ptr(), core::ptr::null_mut()) }
}
#[test]
fn enormous_n_stops_at_the_first_character() {
    unsafe {
        let mut wc = 99;
        let mut st = locale::MbState::default();
        assert_eq!(locale::multibyte::mbrtowc(&mut wc, b"\0".as_ptr(), usize::MAX, &mut st), 0);
        assert_eq!(wc, 0);
        assert_eq!(locale::multibyte::mbrlen(b"\0".as_ptr(), usize::MAX, &mut st), 0);
        assert_eq!(locale::multibyte::mbtowc(&mut wc, b"\0".as_ptr(), usize::MAX), 0);
        let previous = locale::objects::uselocale(c_utf8());
        assert_eq!(locale::multibyte::mbrtowc(&mut wc, [0xe2u8].as_ptr(), 1, &mut st), usize::MAX-1);
        assert_eq!(locale::multibyte::mbrtowc(&mut wc, [0x82u8,0xac].as_ptr(), usize::MAX, &mut st), 2);
        assert_eq!(wc, 0x20ac);
        locale::objects::uselocale(previous);
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
''')
    exe = directory / 'test'
    subprocess.run(['rustc','--edition=2021','--test',str(src),'-o',str(exe)], check=True,
                   env=dict(os.environ, CARGO_BUILD_JOBS='6'))
    subprocess.run([str(exe),'--test-threads=1', 'enormous_n', 'every_c_byte'], check=True)
