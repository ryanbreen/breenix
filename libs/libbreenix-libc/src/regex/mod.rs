//! Regular expressions (POSIX.1-2024 XSH regcomp, regexec, regerror, regfree; syntax per
//! XBD 9). Flags and errors are assigned by this implementation: consecutive flag bits
//! and consecutive nonzero error codes in declaration order; REG_STARTEND is our extension.
//!
//! Matching consumes bytes in C/POSIX and decoded characters in C.UTF-8; match offsets
//! remain byte offsets. Charset and classification are supplied by LOCALE.
//! `regoff_t` is `long`, as POSIX.1-2024 requires (it must hold any ssize_t value).

mod buf;
mod compile;
mod exec;

use compile::Prog;

extern "C" {
    fn malloc(size: usize) -> *mut u8;
    fn free(ptr: *mut u8);
}

pub(crate) const REG_EXTENDED: i32 = 1;
pub(crate) const REG_ICASE: i32 = 2;
pub(crate) const REG_NEWLINE: i32 = 4;
const REG_NOSUB: i32 = 8;
// Implementation-assigned next independent compile flag bit (XBD <regex.h>).
pub(crate) const REG_MINIMAL: i32 = 16;

const REG_NOTBOL: i32 = 1;
const REG_NOTEOL: i32 = 2;
const REG_STARTEND: i32 = 4;

const REG_NOMATCH: i32 = 1;
pub(crate) const REG_BADPAT: i32 = 2;
pub(crate) const REG_ECOLLATE: i32 = 3;
pub(crate) const REG_ECTYPE: i32 = 4;
pub(crate) const REG_EESCAPE: i32 = 5;
pub(crate) const REG_ESUBREG: i32 = 6;
pub(crate) const REG_EBRACK: i32 = 7;
pub(crate) const REG_EPAREN: i32 = 8;
pub(crate) const REG_EBRACE: i32 = 9;
pub(crate) const REG_BADBR: i32 = 10;
pub(crate) const REG_ERANGE: i32 = 11;
pub(crate) const REG_ESPACE: i32 = 12;
pub(crate) const REG_BADRPT: i32 = 13;

/// The `regex_t` of <regex.h>: re_nsub, then this library's own fields.
#[repr(C)]
pub struct Regex {
    re_nsub: usize,
    prog: *mut Prog,
    cflags: i32,
}

#[repr(C)]
pub struct RegMatch {
    rm_so: isize,
    rm_eo: isize,
}

/// regcomp - compile `pattern` into `preg`.
#[no_mangle]
pub unsafe extern "C" fn regcomp(preg: *mut Regex, pattern: *const u8, cflags: i32) -> i32 {
    let p = core::ffi::CStr::from_ptr(pattern as *const core::ffi::c_char).to_bytes();
    let prog = match compile::compile(p, cflags) {
        Ok(prog) => prog,
        Err(code) => return code,
    };
    let boxed = malloc(core::mem::size_of::<Prog>()) as *mut Prog;
    if boxed.is_null() {
        return REG_ESPACE;
    }
    boxed.write(prog);
    (*preg).re_nsub = (*boxed).nsub;
    (*preg).prog = boxed;
    (*preg).cflags = cflags;
    0
}

/// regexec - match `string` against `preg`, filling up to `nmatch` entries of `pmatch`.
#[no_mangle]
pub unsafe extern "C" fn regexec(
    preg: *const Regex,
    string: *const u8,
    nmatch: usize,
    pmatch: *mut RegMatch,
    eflags: i32,
) -> i32 {
    let re = &*preg;
    if re.prog.is_null() {
        return REG_BADPAT;
    }
    let prog = &*re.prog;
    let (from, end) = if eflags & REG_STARTEND != 0 && !pmatch.is_null() {
        (
            (*pmatch).rm_so.max(0) as usize,
            (*pmatch).rm_eo.max(0) as usize,
        )
    } else {
        (
            0,
            core::ffi::CStr::from_ptr(string as *const core::ffi::c_char)
                .to_bytes()
                .len(),
        )
    };
    if from > end {
        return REG_NOMATCH;
    }
    let text = exec::Text {
        s: core::slice::from_raw_parts(string, end),
        from,
        notbol: eflags & REG_NOTBOL != 0,
        noteol: eflags & REG_NOTEOL != 0,
    };
    let mut caps: buf::Buf<isize> = buf::Buf::new();
    if !caps.resize(2 * (prog.nsub + 1 + prog.repeats as usize), -1) {
        return REG_ESPACE;
    }
    match exec::search(prog, &text, caps.as_mut_slice()) {
        Err(code) => return code,
        Ok(false) => return REG_NOMATCH,
        Ok(true) => {}
    }
    if re.cflags & REG_NOSUB == 0 && !pmatch.is_null() {
        let caps = caps.as_slice();
        for i in 0..nmatch {
            let m = &mut *pmatch.add(i);
            if i <= prog.nsub {
                m.rm_so = caps[2 * i];
                m.rm_eo = caps[2 * i + 1];
            } else {
                m.rm_so = -1;
                m.rm_eo = -1;
            }
        }
    }
    0
}

/// regfree - release what regcomp allocated.
#[no_mangle]
pub unsafe extern "C" fn regfree(preg: *mut Regex) {
    let re = &mut *preg;
    if !re.prog.is_null() {
        core::ptr::drop_in_place(re.prog);
        free(re.prog as *mut u8);
        re.prog = core::ptr::null_mut();
    }
}

fn message(code: i32) -> &'static [u8] {
    match code {
        0 => b"Success",
        REG_NOMATCH => b"No match",
        REG_BADPAT => b"Invalid regular expression",
        REG_ECOLLATE => b"Invalid collating element",
        REG_ECTYPE => b"Invalid character class name",
        REG_EESCAPE => b"Trailing backslash",
        REG_ESUBREG => b"Back-reference to a group that does not exist",
        REG_EBRACK => b"Bracket expression is not closed",
        REG_EPAREN => b"Parentheses are not balanced",
        REG_EBRACE => b"Braces are not balanced",
        REG_BADBR => b"Invalid repetition count",
        REG_ERANGE => b"Invalid range in a bracket expression",
        REG_ESPACE => b"Out of memory",
        REG_BADRPT => b"Repetition operator has nothing to repeat",
        _ => b"Unknown regular expression error",
    }
}

/// regerror - the text for `errcode`, truncated to fit `errbuf_size`; returns the size the
/// whole text needs, terminator included.
#[no_mangle]
pub unsafe extern "C" fn regerror(
    errcode: i32,
    _preg: *const Regex,
    errbuf: *mut u8,
    errbuf_size: usize,
) -> usize {
    let m = message(errcode);
    if errbuf_size > 0 && !errbuf.is_null() {
        let n = m.len().min(errbuf_size - 1);
        core::ptr::copy_nonoverlapping(m.as_ptr(), errbuf, n);
        *errbuf.add(n) = 0;
    }
    m.len() + 1
}
