//! The wide scanf family (C17 7.29.2.2-7.29.2.12) on STDIO's scanf engine: this
//! module supplies the wide-character sources, and a wide format selects the
//! engine's wide rules.
use super::streams::{self, Input};
use crate::stdio::file::{self, File};
use crate::stdio::{scan::vscan, Format};
use crate::stdlib::numscan::Chars;
use core::ffi::VaList;

/// A wide string; input ends at its null character.
struct WideString {
    p: *const u32,
    n: usize,
}
impl Chars for WideString {
    fn peek(&mut self) -> Option<u32> {
        match unsafe { *self.p.add(self.n) } {
            0 => None,
            c => Some(c),
        }
    }
    fn bump(&mut self) {
        if self.peek().is_some() {
            self.n += 1;
        }
    }
    fn pos(&self) -> usize {
        self.n
    }
    // Like the narrow sources, scanf keeps what it consumed.
    fn rewind(&mut self, _pos: usize) -> bool {
        false
    }
}
/// A wide-oriented stream, read with fgetwc's decoding; an encoding error or a
/// read error is an input failure. The one character looked at but not taken
/// is pushed back when scanning ends (C17 7.29.2.2p9).
struct WideStream {
    f: *mut File,
    n: usize,
    look: Option<Option<u32>>,
}
impl Chars for WideStream {
    fn peek(&mut self) -> Option<u32> {
        if self.look.is_none() {
            self.look = Some(match unsafe { streams::get(self.f) } {
                Input::Char(c) => Some(c),
                Input::Eof | Input::Error => None,
            });
        }
        self.look.unwrap()
    }
    fn bump(&mut self) {
        if self.peek().is_some() {
            self.look = None;
            self.n += 1;
        }
    }
    fn pos(&self) -> usize {
        self.n
    }
    fn rewind(&mut self, _pos: usize) -> bool {
        false
    }
}
impl Drop for WideStream {
    fn drop(&mut self) {
        if let Some(Some(c)) = self.look {
            unsafe {
                streams::unget(c, self.f);
            }
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn vswscanf(s: *const u32, fmt: *const u32, ap: VaList<'_, '_>) -> i32 {
    vscan(
        &mut WideString { p: s, n: 0 },
        core::ptr::null_mut(),
        &Format::Wide(fmt),
        ap,
    )
}
#[no_mangle]
pub unsafe extern "C" fn swscanf(s: *const u32, fmt: *const u32, mut ap: ...) -> i32 {
    vswscanf(s, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vfwscanf(f: *mut File, fmt: *const u32, ap: VaList<'_, '_>) -> i32 {
    file::lock(f);
    let r = if streams::orient(f) {
        let mut source = WideStream {
            f,
            n: 0,
            look: None,
        };
        let r = vscan(&mut source, f, &Format::Wide(fmt), ap);
        drop(source);
        r
    } else {
        -1
    };
    file::unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fwscanf(f: *mut File, fmt: *const u32, mut ap: ...) -> i32 {
    vfwscanf(f, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vwscanf(fmt: *const u32, ap: VaList<'_, '_>) -> i32 {
    vfwscanf(file::stdin, fmt, ap)
}
#[no_mangle]
pub unsafe extern "C" fn wscanf(fmt: *const u32, mut ap: ...) -> i32 {
    vwscanf(fmt, ap.as_va_list())
}
