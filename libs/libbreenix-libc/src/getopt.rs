//! Command-line option parsing: getopt (POSIX.1-2024 XSH getopt), getopt_long and
//! getopt_long_only (our long-option descriptor), and getsubopt
//! (XSH getsubopt).
//!
//! `getopt` follows POSIX exactly: it never reorders argv and stops at the first operand,
//! at a lone `-`, and after `--`. `getopt_long` and `getopt_long_only` are local extensions: they move operands after the options unless the option string
//! starts with `+` or POSIXLY_CORRECT is set, and a leading `-` returns each operand as
//! option 1. Diagnostics are formatted here and written to fd 2 with write(2), so option
//! parsing does not depend on stdio.
//!
//! Setting `optind` to 0, or to any value other than the one the last call left, restarts
//! the scan.

use core::ptr::{null, null_mut};

extern "C" {
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

#[no_mangle]
pub static mut optarg: *mut u8 = null_mut();
#[no_mangle]
pub static mut optind: i32 = 1;
#[no_mangle]
pub static mut opterr: i32 = 1;
#[no_mangle]
pub static mut optopt: i32 = b'?' as i32;

/// Our long-option descriptor; argument policies are consecutive enum values from 0.
#[repr(C)]
pub struct LongOption {
    name: *const u8,
    has_arg: i32,
    flag: *mut i32,
    val: i32,
}

const NO_ARGUMENT: i32 = 0;
const REQUIRED_ARGUMENT: i32 = 1;

/// Where the scan stands between calls.
struct Scan {
    /// Index of the next option character inside argv[optind]; 0 at an element boundary.
    next: usize,
    /// The optind value the last call left; any other value restarts the scan.
    left_optind: i32,
    /// Operands skipped while permuting: argv[first..last], ending where options resume.
    first: i32,
    last: i32,
}

static mut SCAN: Scan = Scan {
    next: 0,
    left_optind: 1,
    first: -1,
    last: -1,
};

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Short,
    Long,
    LongOnly,
}

unsafe fn c_len(s: *const u8) -> usize {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n
}

unsafe fn c_bytes<'a>(s: *const u8) -> &'a [u8] {
    core::slice::from_raw_parts(s, c_len(s))
}

/// A diagnostic line built in a fixed buffer and written to fd 2 in one write.
// One POSIX minimum PIPE_BUF-sized write; reserve three quarters for the message.
const DIAG_SIZE: usize = 512;
struct Diag {
    buf: [u8; DIAG_SIZE],
    len: usize,
}

impl Diag {
    fn new() -> Self {
        Diag {
            buf: [0; DIAG_SIZE],
            len: 0,
        }
    }
    fn put(&mut self, b: &[u8]) -> &mut Self {
        let room = self.buf.len() - 1 - self.len;
        let n = b.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
        self
    }
    unsafe fn send(&mut self) {
        self.buf[self.len] = b'\n';
        let count = self.len + 1;
        if write(2, self.buf.as_ptr(), count) != count as isize {
            // XSH getopt requires failed diagnostics to set stderr's error indicator.
            // The diagnostic itself remains a raw write, per the FILES contract.
            crate::stdio::file::stderr_write_failed();
        }
    }
}

unsafe fn program(argv: *const *mut u8) -> &'static [u8] {
    let name = *argv;
    if name.is_null() {
        b""
    } else {
        // Reserve space for the message and option even with a long argv[0].
        let bytes = c_bytes(name);
        &bytes[..bytes.len().min(DIAG_SIZE / 4)]
    }
}

unsafe fn arg(argv: *const *mut u8, i: i32) -> *mut u8 {
    *argv.add(i as usize)
}

/// Rotate argv[a..c] so that argv[b..c] comes first.
unsafe fn rotate(argv: *const *mut u8, a: i32, b: i32, c: i32) {
    let v = core::slice::from_raw_parts_mut(argv as *mut *mut u8, c as usize);
    v[a as usize..c as usize].rotate_left((b - a) as usize);
}

fn is_operand(a: &[u8]) -> bool {
    a.len() < 2 || a[0] != b'-'
}

unsafe fn finish() -> i32 {
    SCAN.next = 0;
    SCAN.first = -1;
    SCAN.last = -1;
    SCAN.left_optind = optind;
    -1
}

unsafe fn done(r: i32) -> i32 {
    SCAN.left_optind = optind;
    r
}

unsafe fn parse(
    argc: i32,
    argv: *const *mut u8,
    optstring: *const u8,
    longopts: *const LongOption,
    longindex: *mut i32,
    mode: Mode,
) -> i32 {
    if optind == 0 || optind != SCAN.left_optind {
        if optind == 0 {
            optind = 1;
        }
        SCAN.next = 0;
        SCAN.first = -1;
        SCAN.last = -1;
    }
    let mut spec: &[u8] = if optstring.is_null() {
        b""
    } else {
        c_bytes(optstring)
    };
    let mut in_order = false;
    let mut posix = mode == Mode::Short;
    match spec.first() {
        Some(b'+') => {
            posix = true;
            spec = &spec[1..];
        }
        Some(b'-') => {
            in_order = mode != Mode::Short;
            spec = &spec[1..];
        }
        _ => {}
    }
    if !posix && crate::env::getenv_bytes(b"POSIXLY_CORRECT").is_some() {
        posix = true;
    }
    let colon = spec.first() == Some(&b':');
    if colon {
        spec = &spec[1..];
    }
    let report = opterr != 0 && !colon;
    let permute = !posix && !in_order;
    optarg = null_mut();

    if SCAN.next == 0 {
        if permute && SCAN.first >= 0 && SCAN.last < optind {
            // Options were taken after the skipped operands: move the operands after them.
            rotate(argv, SCAN.first, SCAN.last, optind);
            let len = SCAN.last - SCAN.first;
            SCAN.first = optind - len;
            SCAN.last = optind;
        }
        loop {
            if optind >= argc || arg(argv, optind).is_null() {
                if permute && SCAN.first >= 0 {
                    optind = SCAN.first;
                }
                return finish();
            }
            let a = c_bytes(arg(argv, optind));
            if !is_operand(a) {
                break;
            }
            if in_order {
                optarg = arg(argv, optind);
                optind += 1;
                return done(1);
            }
            if !permute {
                return finish();
            }
            if SCAN.first < 0 {
                SCAN.first = optind;
            }
            optind += 1;
            SCAN.last = optind;
        }
        let a = c_bytes(arg(argv, optind));
        if a == b"--" {
            optind += 1;
            if permute && SCAN.first >= 0 {
                rotate(argv, SCAN.first, SCAN.last, optind);
                optind -= SCAN.last - SCAN.first;
            }
            return finish();
        }
        if mode != Mode::Short && a[1] == b'-' {
            return done(long_option(
                argc,
                argv,
                &a[2..],
                b"--",
                longopts,
                longindex,
                colon,
                report,
            ));
        }
        if mode == Mode::LongOnly {
            // A long option first; failing that a short one; failing both, the long error.
            let name = &a[1..];
            let name_end = name.iter().position(|&b| b == b'=').unwrap_or(name.len());
            let long = matches!(lookup(longopts, &name[..name_end]), Some(Lookup::Found(_)));
            let short = name[0] != b':' && spec.contains(&name[0]);
            if long || !short {
                return done(long_option(
                    argc, argv, name, b"-", longopts, longindex, colon, report,
                ));
            }
        }
        SCAN.next = 1;
    }

    let element = arg(argv, optind);
    let c = *element.add(SCAN.next);
    SCAN.next += 1;
    let at_end = *element.add(SCAN.next) == 0;
    let found = if c == b':' {
        None
    } else {
        spec.iter().position(|&b| b == c)
    };
    let Some(at) = found else {
        optopt = c as i32;
        if at_end {
            optind += 1;
            SCAN.next = 0;
        }
        if report {
            Diag::new()
                .put(program(argv))
                .put(b": invalid option -- '")
                .put(&[c])
                .put(b"'")
                .send();
        }
        return done(b'?' as i32);
    };
    let takes = spec.get(at + 1) == Some(&b':');
    let optional = takes && spec.get(at + 2) == Some(&b':');
    if !takes {
        if at_end {
            optind += 1;
            SCAN.next = 0;
        }
        return done(c as i32);
    }
    if !at_end {
        optarg = element.add(SCAN.next);
        optind += 1;
        SCAN.next = 0;
        return done(c as i32);
    }
    optind += 1;
    SCAN.next = 0;
    if optional {
        return done(c as i32);
    }
    if optind < argc && !arg(argv, optind).is_null() {
        optarg = arg(argv, optind);
        optind += 1;
        return done(c as i32);
    }
    // XSH getopt advances past both the option and its missing argument.
    optind += 1;
    optopt = c as i32;
    if report {
        Diag::new()
            .put(program(argv))
            .put(b": option requires an argument -- '")
            .put(&[c])
            .put(b"'")
            .send();
    }
    done(if colon { b':' as i32 } else { b'?' as i32 })
}

enum Lookup {
    Found(usize),
    Ambiguous,
}

/// Find `name` in `longopts`: an exact match, else the only option it abbreviates. Several
/// abbreviated options that all do the same thing are one match.
unsafe fn lookup(longopts: *const LongOption, name: &[u8]) -> Option<Lookup> {
    if longopts.is_null() || name.is_empty() {
        return None;
    }
    let mut first: Option<usize> = None;
    let mut ambiguous = false;
    let mut i = 0;
    loop {
        let o = &*longopts.add(i);
        if o.name.is_null() {
            break;
        }
        let full = c_bytes(o.name);
        if full == name {
            return Some(Lookup::Found(i));
        }
        if full.starts_with(name) {
            match first {
                None => first = Some(i),
                Some(f) => {
                    let p = &*longopts.add(f);
                    if p.has_arg != o.has_arg || p.flag != o.flag || p.val != o.val {
                        ambiguous = true;
                    }
                }
            }
        }
        i += 1;
    }
    if ambiguous {
        return Some(Lookup::Ambiguous);
    }
    first.map(Lookup::Found)
}

/// Handle the long option `text` (argv[optind] after its `prefix` dashes).
unsafe fn long_option(
    argc: i32,
    argv: *const *mut u8,
    text: &[u8],
    prefix: &[u8],
    longopts: *const LongOption,
    longindex: *mut i32,
    colon: bool,
    report: bool,
) -> i32 {
    let eq = text.iter().position(|&b| b == b'=');
    let name = &text[..eq.unwrap_or(text.len())];
    let element = arg(argv, optind);
    optind += 1;
    let index = match lookup(longopts, name) {
        Some(Lookup::Found(i)) => i,
        Some(Lookup::Ambiguous) => {
            optopt = 0;
            if report {
                let mut d = Diag::new();
                d.put(program(argv))
                    .put(b": option '")
                    .put(prefix)
                    .put(name)
                    .put(b"' is ambiguous; possibilities:");
                let mut i = 0;
                loop {
                    let o = &*longopts.add(i);
                    if o.name.is_null() {
                        break;
                    }
                    if c_bytes(o.name).starts_with(name) {
                        d.put(b" '").put(prefix).put(c_bytes(o.name)).put(b"'");
                    }
                    i += 1;
                }
                d.send();
            }
            return b'?' as i32;
        }
        None => {
            optopt = 0;
            if report {
                Diag::new()
                    .put(program(argv))
                    .put(b": unrecognized option '")
                    .put(c_bytes(element))
                    .put(b"'")
                    .send();
            }
            return b'?' as i32;
        }
    };
    let o = &*longopts.add(index);
    let full = c_bytes(o.name);
    match eq {
        Some(at) => {
            if o.has_arg == NO_ARGUMENT {
                optopt = o.val;
                if report {
                    Diag::new()
                        .put(program(argv))
                        .put(b": option '")
                        .put(prefix)
                        .put(full)
                        .put(b"' doesn't allow an argument")
                        .send();
                }
                return b'?' as i32;
            }
            optarg = (text.as_ptr() as *mut u8).add(at + 1);
        }
        None => {
            if o.has_arg == REQUIRED_ARGUMENT {
                if optind < argc && !arg(argv, optind).is_null() {
                    optarg = arg(argv, optind);
                    optind += 1;
                } else {
                    optopt = o.val;
                    if report {
                        Diag::new()
                            .put(program(argv))
                            .put(b": option '")
                            .put(prefix)
                            .put(full)
                            .put(b"' requires an argument")
                            .send();
                    }
                    return if colon { b':' as i32 } else { b'?' as i32 };
                }
            }
        }
    }
    if !longindex.is_null() {
        *longindex = index as i32;
    }
    if !o.flag.is_null() {
        *o.flag = o.val;
        return 0;
    }
    o.val
}

/// getopt - the next option character of argv, POSIX rules.
#[no_mangle]
pub unsafe extern "C" fn getopt(argc: i32, argv: *const *mut u8, optstring: *const u8) -> i32 {
    parse(argc, argv, optstring, null(), null_mut(), Mode::Short)
}

/// getopt_long - getopt plus `--name[=value]` options from `longopts`.
#[no_mangle]
pub unsafe extern "C" fn getopt_long(
    argc: i32,
    argv: *const *mut u8,
    optstring: *const u8,
    longopts: *const LongOption,
    longindex: *mut i32,
) -> i32 {
    parse(argc, argv, optstring, longopts, longindex, Mode::Long)
}

/// getopt_long_only - getopt_long that also accepts long options after a single `-`.
#[no_mangle]
pub unsafe extern "C" fn getopt_long_only(
    argc: i32,
    argv: *const *mut u8,
    optstring: *const u8,
    longopts: *const LongOption,
    longindex: *mut i32,
) -> i32 {
    parse(argc, argv, optstring, longopts, longindex, Mode::LongOnly)
}

/// getsubopt - split the next comma-separated suboption off `*optionp` and match its name
/// against `tokens`. Returns the token's index, with `*valuep` at the text after `=` (or
/// null), including when the name is unknown and the return value is -1.
#[no_mangle]
pub unsafe extern "C" fn getsubopt(
    optionp: *mut *mut u8,
    tokens: *const *const u8,
    valuep: *mut *mut u8,
) -> i32 {
    let start = *optionp;
    *valuep = null_mut();
    if start.is_null() || *start == 0 {
        return -1;
    }
    let mut end = start;
    while *end != 0 && *end != b',' {
        end = end.add(1);
    }
    let rest = if *end == b',' {
        *end = 0;
        end.add(1)
    } else {
        end
    };
    *optionp = rest;
    let text = core::slice::from_raw_parts(start, end.offset_from(start) as usize);
    let name = match text.iter().position(|&b| b == b'=') {
        Some(eq) => {
            *valuep = start.add(eq + 1);
            &text[..eq]
        }
        None => text,
    };
    let mut i = 0;
    loop {
        let t = *tokens.add(i);
        if t.is_null() {
            break;
        }
        if c_bytes(t) == name {
            return i as i32;
        }
        i += 1;
    }
    -1
}
