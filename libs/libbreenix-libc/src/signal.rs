//! Signal set operations, using Breenix's 64-signal Linux kernel ABI.
use crate::pthread::set_errno;

pub(crate) const CANCEL_SIGNAL: i32 = 32;
pub(crate) const CANCEL_MASK: u64 = 1 << (CANCEL_SIGNAL - 1);

fn invalid() -> i32 { set_errno(22); -1 }
fn bit(sig: i32) -> Option<u64> {
    if (1..=64).contains(&sig) && sig != CANCEL_SIGNAL {
        Some(1u64 << (sig - 1))
    } else { None }
}
#[no_mangle]
pub unsafe extern "C" fn sigemptyset(set: *mut u64) -> i32 {
    if set.is_null() { return invalid(); }
    *set = 0;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigfillset(set: *mut u64) -> i32 {
    if set.is_null() { return invalid(); }
    *set = !CANCEL_MASK;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigaddset(set: *mut u64, sig: i32) -> i32 {
    let Some(b) = bit(sig) else { return invalid(); };
    if set.is_null() { return invalid(); }
    *set |= b;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigdelset(set: *mut u64, sig: i32) -> i32 {
    let Some(b) = bit(sig) else { return invalid(); };
    if set.is_null() { return invalid(); }
    *set &= !b;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigismember(set: *const u64, sig: i32) -> i32 {
    let Some(b) = bit(sig) else { return invalid(); };
    if set.is_null() { return invalid(); }
    ((*set & b) != 0) as i32
}

#[no_mangle]
pub unsafe extern "C" fn sigpending(set: *mut u64) -> i32 {
    if set.is_null() { return invalid(); }
    let r = libbreenix::raw::syscall2(libbreenix::syscall::nr::SIGPENDING, set as u64, 8) as i64;
    if r >= 0 { *set &= !CANCEL_MASK; }
    crate::syscall_result_to_c_int(r)
}
#[no_mangle]
pub unsafe extern "C" fn sigtimedwait(set: *const u64, info: *mut u8, timeout: *const i64) -> i32 {
    if set.is_null() { return invalid(); }
    let mask = *set & !CANCEL_MASK;
    crate::syscall_result_to_c_int(crate::pthread::cancel_syscall(libbreenix::syscall::nr::SIGTIMEDWAIT,
        [&mask as *const u64 as u64, info as u64, timeout as u64, 8, 0, 0]))
}
#[no_mangle]
pub unsafe extern "C" fn sigwaitinfo(set: *const u64, info: *mut u8) -> i32 {
    sigtimedwait(set, info, core::ptr::null())
}

/// rt_sigqueueinfo, as the kernel dispatches it
/// (kernel/src/syscall/mod.rs RT_SIGQUEUEINFO_SYSCALL_NUMBER).
#[cfg(target_arch = "aarch64")]
const RT_SIGQUEUEINFO: u64 = 138;
#[cfg(target_arch = "x86_64")]
const RT_SIGQUEUEINFO: u64 = 129;
/// si_code for sigqueue (kernel/src/signal/constants.rs SI_QUEUE).
const SI_QUEUE: i32 = -1;
const SIGRTMIN: i32 = CANCEL_SIGNAL + 1;
const SIGRTMAX: i32 = 64;

/// POSIX.1-2024 sigqueue: the signal with `value`, si_code SI_QUEUE and the
/// sender's PID and real user ID, in the kernel's Linux siginfo layout.
#[no_mangle]
pub unsafe extern "C" fn sigqueue(pid: i32, sig: i32, value: u64) -> i32 {
    if sig == CANCEL_SIGNAL { return invalid(); }
    let mut info = [0u64; 16];
    info[0] = sig as u32 as u64;
    info[1] = SI_QUEUE as u32 as u64;
    let me = libbreenix::raw::syscall0(libbreenix::syscall::nr::GETPID);
    info[2] = (me as u32 as u64) | ((crate::getuid() as u64) << 32);
    info[3] = value;
    crate::syscall_result_to_c_int(libbreenix::raw::syscall3(RT_SIGQUEUEINFO,
        pid as i64 as u64, sig as i64 as u64, info.as_ptr() as u64) as i64)
}

/// Names without the SIG prefix, by number; signal 32 is reserved.
const NAMES: [&[u8]; 32] = [
    b"", b"HUP", b"INT", b"QUIT", b"ILL", b"TRAP", b"ABRT", b"BUS", b"FPE", b"KILL",
    b"USR1", b"SEGV", b"USR2", b"PIPE", b"ALRM", b"TERM", b"STKFLT", b"CHLD", b"CONT",
    b"STOP", b"TSTP", b"TTIN", b"TTOU", b"URG", b"XCPU", b"XFSZ", b"VTALRM", b"PROF",
    b"WINCH", b"IO", b"PWR", b"SYS",
];
/// Other names signal.h gives the same numbers.
const ALIASES: [(&[u8], i32); 3] = [(b"IOT", 6), (b"CLD", 17), (b"POLL", 29)];
/// Descriptions, from the POSIX.1-2024 <signal.h> table where it lists the
/// signal, by number.
const DESCRIPTIONS: [&[u8]; 32] = [
    b"", b"Hangup", b"Terminal interrupt signal", b"Terminal quit signal",
    b"Illegal instruction", b"Trace/breakpoint trap", b"Process abort signal",
    b"Access to an undefined portion of a memory object", b"Erroneous arithmetic operation",
    b"Kill", b"User-defined signal 1", b"Invalid memory reference", b"User-defined signal 2",
    b"Write on a pipe with no one to read it", b"Alarm clock", b"Termination signal",
    b"Stack fault", b"Child process terminated, stopped, or continued",
    b"Continue executing, if stopped", b"Stop executing", b"Terminal stop signal",
    b"Background process attempting read", b"Background process attempting write",
    b"High bandwidth data is available at a socket", b"CPU time limit exceeded",
    b"File size limit exceeded", b"Virtual timer expired", b"Profiling timer expired",
    b"Terminal window size changed", b"Pollable event", b"Power failure",
    b"Bad system call",
];
fn valid(sig: i32) -> bool {
    (1..=SIGRTMAX).contains(&sig) && sig != CANCEL_SIGNAL
}
fn append(out: &mut [u8], len: &mut usize, s: &[u8]) {
    let n = s.len().min(out.len() - 1 - *len);
    out[*len..*len + n].copy_from_slice(&s[..n]);
    *len += n;
}
/// Append `n` (0..=99) in decimal.
fn push_number(out: &mut [u8], len: &mut usize, n: i32) {
    if n >= 10 { append(out, len, &[b'0' + (n / 10) as u8]); }
    append(out, len, &[b'0' + (n % 10) as u8]);
}
/// The name of a valid signal, without the SIG prefix and unterminated:
/// realtime signals are RTMIN, RTMIN+n for n up to half the range, RTMAX-n
/// above that, and RTMAX (POSIX.1-2024 sig2str).
fn name(sig: i32, out: &mut [u8; 16]) -> usize {
    let mut len = 0;
    if sig < SIGRTMIN {
        append(out, &mut len, NAMES[sig as usize]);
    } else if sig == SIGRTMAX {
        append(out, &mut len, b"RTMAX");
    } else if sig - SIGRTMIN <= (SIGRTMAX - SIGRTMIN) / 2 {
        append(out, &mut len, b"RTMIN");
        if sig > SIGRTMIN {
            append(out, &mut len, b"+");
            push_number(out, &mut len, sig - SIGRTMIN);
        }
    } else {
        append(out, &mut len, b"RTMAX-");
        push_number(out, &mut len, SIGRTMAX - sig);
    }
    len
}
/// POSIX.1-2024 sig2str: `str` holds at least SIG2STR_MAX bytes.
#[no_mangle]
pub unsafe extern "C" fn sig2str(sig: i32, str: *mut u8) -> i32 {
    if !valid(sig) { return -1; }
    let mut buf = [0u8; 16];
    let len = name(sig, &mut buf);
    core::ptr::copy_nonoverlapping(buf.as_ptr(), str, len);
    *str.add(len) = 0;
    0
}
fn decimal(s: &[u8]) -> Option<i32> {
    if s.is_empty() || s.len() > 2 || !s.iter().all(u8::is_ascii_digit) { return None; }
    Some(s.iter().fold(0, |n, d| n * 10 + (d - b'0') as i32))
}
/// The signal a name without the SIG prefix, or a decimal number, denotes.
fn parse(s: &[u8]) -> Option<i32> {
    let realtime = |sig: i32| (SIGRTMIN..=SIGRTMAX).contains(&sig).then_some(sig);
    let sig = if let Some(n) = decimal(s) {
        n
    } else if let Some(i) = NAMES.iter().position(|n| !n.is_empty() && *n == s) {
        i as i32
    } else if let Some(&(_, n)) = ALIASES.iter().find(|(a, _)| *a == s) {
        n
    } else if s == b"RTMIN" {
        SIGRTMIN
    } else if s == b"RTMAX" {
        SIGRTMAX
    } else if let Some(n) = s.strip_prefix(b"RTMIN+").and_then(decimal) {
        realtime(SIGRTMIN + n)?
    } else if let Some(n) = s.strip_prefix(b"RTMAX-").and_then(decimal) {
        realtime(SIGRTMAX - n)?
    } else {
        return None;
    };
    valid(sig).then_some(sig)
}
/// POSIX.1-2024 str2sig.
#[no_mangle]
pub unsafe extern "C" fn str2sig(str: *const u8, num: *mut i32) -> i32 {
    if str.is_null() || num.is_null() { return -1; }
    let mut len = 0;
    while *str.add(len) != 0 {
        len += 1;
        if len > 16 { return -1; }
    }
    match parse(core::slice::from_raw_parts(str, len)) {
        Some(sig) => { *num = sig; 0 }
        None => -1,
    }
}

/// "<prefix>: <description>\n" on stderr, the prefix and its separator only
/// when `prefix` is neither null nor empty.
unsafe fn report(sig: i32, prefix: *const u8) {
    let mut line = [0u8; 80];
    let mut len = 0;
    if valid(sig) && sig < SIGRTMIN {
        append(&mut line, &mut len, DESCRIPTIONS[sig as usize]);
    } else if valid(sig) {
        let mut buf = [0u8; 16];
        let n = name(sig, &mut buf);
        append(&mut line, &mut len, b"Real-time signal ");
        append(&mut line, &mut len, &buf[..n]);
    } else {
        append(&mut line, &mut len, b"Unknown signal ");
        if (0..100).contains(&sig) {
            push_number(&mut line, &mut len, sig);
        } else {
            append(&mut line, &mut len, b"?");
        }
    }
    append(&mut line, &mut len, b"\n");
    let err = crate::stdio::file::stderr;
    crate::stdio::file::lock(err);
    if !prefix.is_null() && *prefix != 0 {
        crate::stdio::file::fputs(prefix, err);
        crate::stdio::file::fputs(b": \0".as_ptr(), err);
    }
    crate::stdio::file::fputs(line.as_ptr(), err);
    crate::stdio::file::unlock(err);
}
/// POSIX.1-2024 psignal.
#[no_mangle]
pub unsafe extern "C" fn psignal(sig: i32, prefix: *const u8) {
    report(sig, prefix);
}
/// POSIX.1-2024 psiginfo: as psignal for the siginfo's signal number.
#[no_mangle]
pub unsafe extern "C" fn psiginfo(info: *const i32, prefix: *const u8) {
    report(if info.is_null() { 0 } else { *info }, prefix);
}
