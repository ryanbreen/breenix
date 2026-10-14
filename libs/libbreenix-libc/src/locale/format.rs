use super::{decode, set_errno, streams, Decoded, MbState};
use crate::stdio::file::{self, File};
use crate::stdio::{vformat, Format, Sink};
use core::ffi::VaList;

struct WideSink {
    dst: *mut u32,
    cap: usize,
    n: usize,
    state: MbState,
    error: i32,
}
impl Sink for WideSink {
    fn bytes(&mut self, b: &[u8]) -> bool {
        let mut i = 0;
        while i < b.len() {
            match decode(&b[i..], &mut self.state) {
                Decoded::Char(c, n) => {
                    i += n;
                    if !self.wide(c) {
                        return false;
                    }
                }
                Decoded::Incomplete(n) => {
                    i += n;
                }
                Decoded::Invalid => {
                    self.error = 84;
                    set_errno(84);
                    return false;
                }
            }
        }
        true
    }
    fn wide(&mut self, c: u32) -> bool {
        if self.n >= self.cap.saturating_sub(1) {
            self.error = 7;
            return false;
        }
        if self.n >= i32::MAX as usize {
            self.error = 75;
            set_errno(75);
            return false;
        }
        unsafe {
            *self.dst.add(self.n) = c;
        }
        self.n += 1;
        true
    }
}
#[no_mangle]
pub unsafe extern "C" fn vswprintf(
    dst: *mut u32,
    n: usize,
    fmt: *const u32,
    ap: VaList<'_, '_>,
) -> i32 {
    let mut out = WideSink {
        dst,
        cap: n,
        n: 0,
        state: MbState::default(),
        error: 0,
    };
    let result = vformat(&mut out, Format::Wide(fmt), ap);
    if n != 0 {
        *dst.add(out.n) = 0;
    }
    if result.is_err() || out.state.expected != 0 || n == 0 {
        let error = if out.error != 0 {
            out.error
        } else if out.state.expected != 0 {
            84
        } else {
            result.err().unwrap_or(7)
        };
        set_errno(error);
        -1
    } else {
        out.n as i32
    }
}
#[no_mangle]
pub unsafe extern "C" fn swprintf(dst: *mut u32, n: usize, fmt: *const u32, mut ap: ...) -> i32 {
    vswprintf(dst, n, fmt, ap.as_va_list())
}
struct StreamSink {
    file: *mut File,
    n: usize,
    state: MbState,
}
impl Sink for StreamSink {
    fn bytes(&mut self, b: &[u8]) -> bool {
        let mut i = 0;
        while i < b.len() {
            match decode(&b[i..], &mut self.state) {
                Decoded::Char(c, n) => {
                    i += n;
                    if !self.wide(c) {
                        return false;
                    }
                }
                Decoded::Incomplete(n) => {
                    i += n;
                }
                Decoded::Invalid => {
                    unsafe {
                        file::set_error(self.file);
                    }
                    set_errno(84);
                    return false;
                }
            }
        }
        true
    }
    fn wide(&mut self, c: u32) -> bool {
        if self.n >= i32::MAX as usize {
            set_errno(75);
            return false;
        }
        if unsafe { streams::put(c, self.file) } == u32::MAX {
            return false;
        }
        self.n += 1;
        true
    }
}
#[no_mangle]
pub unsafe extern "C" fn vfwprintf(f: *mut File, fmt: *const u32, ap: VaList<'_, '_>) -> i32 {
    file::lock(f);
    let mut sink = StreamSink {
        file: f,
        n: 0,
        state: MbState::default(),
    };
    let result = if streams::orient(f) {
        vformat(&mut sink, Format::Wide(fmt), ap)
    } else {
        Err(22)
    };
    let r = if result.is_err() || sink.state.expected != 0 {
        let error = if sink.state.expected != 0 {
            84
        } else {
            result.err().unwrap()
        };
        set_errno(error);
        file::set_error(f);
        -1
    } else {
        sink.n as i32
    };
    file::unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fwprintf(f: *mut File, fmt: *const u32, mut ap: ...) -> i32 {
    vfwprintf(f, fmt, ap.as_va_list())
}
#[no_mangle]
pub unsafe extern "C" fn vwprintf(fmt: *const u32, ap: VaList<'_, '_>) -> i32 {
    vfwprintf(file::stdout, fmt, ap)
}
#[no_mangle]
pub unsafe extern "C" fn wprintf(fmt: *const u32, mut ap: ...) -> i32 {
    vwprintf(fmt, ap.as_va_list())
}
