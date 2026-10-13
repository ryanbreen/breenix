//! Terminals & job control: the general terminal interface (termios), pseudo-terminals,
//! controlling terminals and sessions, and job control, as POSIX.1-2024 chapter 11 and the
//! termios.h, tcgetattr, tcsetattr, tcdrain, tcflush, tcflow, tcsendbreak, tcgetpgrp,
//! tcsetpgrp, tcgetsid, setsid, setpgid, getpgid, getsid, posix_openpt, grantpt, unlockpt and
//! ptsname pages specify them, on Linux's ABI: struct termios, the ioctl numbers, /dev/ptmx
//! and /dev/pts.
//!
//! Terminals are measured through the interface a portable C program uses. A function
//! Breenix's own C library (libs/libbreenix-libc) defines is called there: build.rs reads
//! libc.a's symbol index and sets `libc_has = "<name>"` for each one it has. A function the
//! library lacks that Linux implements as an ioctl or a system call is made by its Linux
//! number, as a C library makes it: tcgetattr through TCGETS, tcsetattr through TCSETS,
//! TCSETSW and TCSETSF, tcflush through TCFLSH, tcdrain and tcsendbreak through TCSBRK,
//! tcflow through TCXONC, tcgetpgrp, tcsetpgrp and tcgetsid through TIOCGPGRP, TIOCSPGRP and
//! TIOCGSID, posix_openpt by opening /dev/ptmx, unlockpt through TIOCSPTLCK, ptsname through
//! TIOCGPTN, and setsid, getsid, setpgid and getpgid directly, so a kernel that lacks one
//! fails the case with its error. A function a C library builds without the kernel
//! (cfgetispeed, cfgetospeed, cfsetispeed, cfsetospeed and grantpt, and tcsetattr's refusal
//! of an unknown optional_actions) fails each case that needs it with `the C library has no
//! <name>` while the library lacks it. The suite uses raw system calls otherwise only to
//! arrange a case: forking and reaping, signal masks and waits, non-blocking reads, clocks.
//!
//! Every case uses pseudo-terminals, so none needs a console. Each runs in its own forked
//! child under the runner's 10-second limit; the processes a case starts report through a
//! shared page and their exit status, and every wait on one is bounded and stops 1.5 seconds
//! before the case's deadline. A read the case expects to return, or to find nothing, is a
//! non-blocking read retried until its bound, so a terminal that never delivers fails the
//! case rather than hanging it. A process that takes a controlling terminal first makes its
//! own session, so no case touches the runner's session or process group.
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{self as signal, Sigaction, SIGHUP, SIGINT, SIGQUIT, SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU, SIGWINCH};
use libbreenix::suite::{case, case_ms_left, category, check, fail, suite, value, CaseError, CaseResult, Suite};
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU8, Ordering::Relaxed, Ordering::SeqCst};

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const MMAP: u64 = 9;
    pub const RT_SIGPROCMASK: u64 = 14;
    #[cfg(not(libc_has = "ioctl"))]
    pub const IOCTL: u64 = 16;
    pub const NANOSLEEP: u64 = 35;
    pub const GETPID: u64 = 39;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const FCNTL: u64 = 72;
    #[cfg(not(libc_has = "setpgid"))]
    pub const SETPGID: u64 = 109;
    pub const GETPPID: u64 = 110;
    #[cfg(not(libc_has = "setsid"))]
    pub const SETSID: u64 = 112;
    #[cfg(not(libc_has = "getpgid"))]
    pub const GETPGID: u64 = 121;
    #[cfg(not(libc_has = "getsid"))]
    pub const GETSID: u64 = 124;
    pub const RT_SIGTIMEDWAIT: u64 = 128;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const MMAP: u64 = 222;
    pub const RT_SIGPROCMASK: u64 = 135;
    #[cfg(not(libc_has = "ioctl"))]
    pub const IOCTL: u64 = 29;
    pub const NANOSLEEP: u64 = 101;
    pub const GETPID: u64 = 172;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const FCNTL: u64 = 25;
    #[cfg(not(libc_has = "setpgid"))]
    pub const SETPGID: u64 = 154;
    pub const GETPPID: u64 = 173;
    #[cfg(not(libc_has = "setsid"))]
    pub const SETSID: u64 = 157;
    #[cfg(not(libc_has = "getpgid"))]
    pub const GETPGID: u64 = 155;
    #[cfg(not(libc_has = "getsid"))]
    pub const GETSID: u64 = 156;
    pub const RT_SIGTIMEDWAIT: u64 = 137;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const EXIT_GROUP: u64 = 94;
}

const EPERM: i64 = 1;
const ESRCH: i64 = 3;
const EINTR: i64 = 4;
const EIO: i64 = 5;
const ENXIO: i64 = 6;
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const EINVAL: i64 = 22;
const ENOTTY: i64 = 25;

const O_RDWR: i32 = 2;
const O_NOCTTY: i32 = 0o400;
const O_NONBLOCK: u64 = 0o4000;
const F_GETFL: u64 = 3;
const F_SETFL: u64 = 4;

const POLLIN: i16 = 1;
const POLLHUP: i16 = 0x10;

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const MAP_SHARED_ANON: u64 = 0x21;
const SIG_SETMASK: u64 = 2;
const WNOHANG: u64 = 1;
const WUNTRACED: u64 = 2;

// The Linux ioctl requests, the same on x86-64 and ARM64.
#[cfg(not(libc_has = "tcgetattr"))]
const TCGETS: u64 = 0x5401;
#[cfg(not(libc_has = "tcsetattr"))]
const TCSETS: u64 = 0x5402;
#[cfg(not(all(libc_has = "tcdrain", libc_has = "tcsendbreak")))]
const TCSBRK: u64 = 0x5409;
#[cfg(not(libc_has = "tcflow"))]
const TCXONC: u64 = 0x540a;
#[cfg(not(libc_has = "tcflush"))]
const TCFLSH: u64 = 0x540b;
const TIOCSCTTY: u64 = 0x540e;
#[cfg(not(libc_has = "tcgetpgrp"))]
const TIOCGPGRP: u64 = 0x540f;
#[cfg(not(libc_has = "tcsetpgrp"))]
const TIOCSPGRP: u64 = 0x5410;
const TIOCGWINSZ: u64 = 0x5413;
const TIOCSWINSZ: u64 = 0x5414;
const TIOCNOTTY: u64 = 0x5422;
#[cfg(not(libc_has = "tcgetsid"))]
const TIOCGSID: u64 = 0x5429;
#[cfg(not(libc_has = "ptsname"))]
const TIOCGPTN: u64 = 0x8004_5430;
#[cfg(not(libc_has = "unlockpt"))]
const TIOCSPTLCK: u64 = 0x4004_5431;
#[cfg(not(libc_has = "posix_openpt"))]
const PTMX: &[u8] = b"/dev/ptmx\0";

// struct termios: c_iflag bits.
const ISTRIP: u32 = 0o40;
const INLCR: u32 = 0o100;
const IGNCR: u32 = 0o200;
const ICRNL: u32 = 0o400;
const IXON: u32 = 0o2000;
// c_oflag bits.
const OPOST: u32 = 0o1;
const ONLCR: u32 = 0o4;
// c_lflag bits.
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const ECHONL: u32 = 0o100;
const NOFLSH: u32 = 0o200;
const TOSTOP: u32 = 0o400;
const IEXTEN: u32 = 0o100000;
// c_cc indexes.
const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VTIME: usize = 5;
const VMIN: usize = 6;
const VSUSP: usize = 10;
const VEOL: usize = 11;
const NCCS: usize = 32;
// tcsetattr, tcflush and tcflow arguments.
const TCSANOW: i32 = 0;
const TCSADRAIN: i32 = 1;
const TCSAFLUSH: i32 = 2;
const TCIFLUSH: i32 = 0;
const TCOOFF: i32 = 0;
const TCOON: i32 = 1;
// Speeds.
const B9600: u32 = 0o15;
const B19200: u32 = 0o16;

/// The characters the job-control cases type: ^C, ^\ and ^Z.
const CTRL_C: u8 = 0x03;
const CTRL_BACKSLASH: u8 = 0x1c;
const CTRL_Z: u8 = 0x1a;

const NS: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;
const CLOCK_MONOTONIC: u64 = 1;
/// Time a case keeps to report and clean up after its last bounded wait.
const CLEANUP_MS: u64 = 1500;

// ---------------------------------------------------------------------------
// System calls made directly.

/// A system call made as a C library makes it: on x86-64 the SYSCALL instruction, and on
/// ARM64 svc.
fn sc(n: u64, args: &[u64]) -> i64 {
    let a = |i: usize| args.get(i).copied().unwrap_or(0);
    #[cfg(target_arch = "x86_64")]
    {
        let ret: i64;
        // SAFETY: every caller keeps the buffers its arguments point to alive through the
        // call; SYSCALL writes only rax, rcx and r11.
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n as i64 => ret,
                in("rdi") a(0), in("rsi") a(1), in("rdx") a(2), in("r10") a(3), in("r8") a(4), in("r9") a(5),
                lateout("rcx") _, lateout("r11") _,
                options(nostack),
            );
        }
        ret
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: every caller keeps the buffers its arguments point to alive through the call.
    unsafe { raw::syscall6(n, a(0), a(1), a(2), a(3), a(4), a(5)) as i64 }
}

// ---------------------------------------------------------------------------
// The C library's functions.

/// errno as the C library left it.
#[cfg(libc_has = "__errno_location")]
fn errno() -> i64 {
    extern "C" {
        fn __errno_location() -> *mut i32;
    }
    // SAFETY: the library returns the calling thread's errno, which lives as long as it.
    i64::from(unsafe { *__errno_location() })
}
#[cfg(not(libc_has = "__errno_location"))]
fn errno() -> i64 { 4095 }

/// The Linux ABI's struct termios as a C library declares it: the four mode words, the line
/// discipline, 32 control characters and the two speeds, 60 bytes. The kernel's TCGETS and
/// TCSETS use its first 36 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Termios { iflag: u32, oflag: u32, cflag: u32, lflag: u32, line: u8, cc: [u8; NCCS], ispeed: u32, ospeed: u32 }

/// The Linux ABI's struct winsize.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct Winsize { row: u16, col: u16, xpixel: u16, ypixel: u16 }

/// The Linux ABI's struct pollfd.
#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }

/// Declare C library functions the cases need: each is a Rust function of the same name
/// that calls the library's function when libc.a defines it, returning its result, or
/// `-errno` when it returns -1. When the library does not define it, the case fails with
/// "the C library has no <name>".
macro_rules! c_fn {
    ($( $sym:literal fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => { $(
        #[cfg(libc_has = $sym)]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> {
            extern "C" {
                #[link_name = $sym]
                fn f($($arg: $ty),*) -> $ret;
            }
            // SAFETY: every caller passes pointers to objects it keeps alive and large
            // enough for the C type.
            let r = unsafe { f($($arg),*) } as i64;
            Ok(if r == -1 { -errno() } else { r })
        }
        #[cfg(not(libc_has = $sym))]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> {
            let _ = ($($arg,)*);
            Err(CaseError::Fail(concat!("the C library has no ", $sym).into()))
        }
    )* };
}

/// Declare C library functions that Linux implements as an ioctl or a system call: each
/// calls the library's function when libc.a defines it, and otherwise runs the block, which
/// makes the call by its Linux number as a C library makes it. Either way the result is the
/// value, or `-errno`.
macro_rules! c_or {
    ($( $sym:literal fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $fallback:block )*) => { $(
        #[cfg(libc_has = $sym)]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> {
            extern "C" {
                #[link_name = $sym]
                fn f($($arg: $ty),*) -> $ret;
            }
            // SAFETY: every caller passes pointers to objects it keeps alive and large
            // enough for the C type.
            let r = unsafe { f($($arg),*) } as i64;
            Ok(if r == -1 { -errno() } else { r })
        }
        #[cfg(not(libc_has = $sym))]
        fn $name($($arg: $ty),*) -> Result<i64, CaseError> $fallback
    )* };
}

c_fn! {
    "open" fn open(path: *const u8, flags: i32, mode: u32) -> i32;
    "close" fn close(fd: i32) -> i32;
    "read" fn read(fd: i32, buf: *mut u8, len: usize) -> isize;
    "write" fn write(fd: i32, buf: *const u8, len: usize) -> isize;
    "poll" fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
    "isatty" fn isatty(fd: i32) -> i32;
    "grantpt" fn grantpt(fd: i32) -> i32;
    "cfgetispeed" fn cfgetispeed(t: *const Termios) -> u32;
    "cfgetospeed" fn cfgetospeed(t: *const Termios) -> u32;
    "cfsetispeed" fn cfsetispeed(t: *mut Termios, speed: u32) -> i32;
    "cfsetospeed" fn cfsetospeed(t: *mut Termios, speed: u32) -> i32;
}

c_or! {
    "ioctl" fn ioctl(fd: i32, request: u64, arg: u64) -> i32 = { Ok(sc(nr::IOCTL, &[fd as u64, request, arg])) }
    "posix_openpt" fn posix_openpt(flags: i32) -> i32 = { open(PTMX.as_ptr(), flags, 0) }
    "unlockpt" fn unlockpt(fd: i32) -> i32 = {
        let unlock = 0i32;
        ioctl(fd, TIOCSPTLCK, &unlock as *const i32 as u64)
    }
    "tcgetattr" fn tcgetattr(fd: i32, t: *mut Termios) -> i32 = { ioctl(fd, TCGETS, t as u64) }
    "tcsetattr" fn tcsetattr(fd: i32, act: i32, t: *const Termios) -> i32 = {
        if !(TCSANOW..=TCSAFLUSH).contains(&act) {
            return Err(CaseError::Fail("the C library has no tcsetattr to refuse an unknown optional_actions".into()));
        }
        ioctl(fd, TCSETS + act as u64, t as u64)
    }
    "tcflush" fn tcflush(fd: i32, queue: i32) -> i32 = { ioctl(fd, TCFLSH, queue as u64) }
    "tcdrain" fn tcdrain(fd: i32) -> i32 = { ioctl(fd, TCSBRK, 1) }
    "tcflow" fn tcflow(fd: i32, action: i32) -> i32 = { ioctl(fd, TCXONC, action as u64) }
    "tcsendbreak" fn tcsendbreak(fd: i32, duration: i32) -> i32 = {
        let _ = duration;
        ioctl(fd, TCSBRK, 0)
    }
    "tcgetpgrp" fn tcgetpgrp(fd: i32) -> i32 = {
        let mut pgrp = 0i32;
        let r = ioctl(fd, TIOCGPGRP, &mut pgrp as *mut i32 as u64)?;
        Ok(if r < 0 { r } else { i64::from(pgrp) })
    }
    "tcsetpgrp" fn tcsetpgrp(fd: i32, pgrp: i32) -> i32 = { ioctl(fd, TIOCSPGRP, &pgrp as *const i32 as u64) }
    "tcgetsid" fn tcgetsid(fd: i32) -> i32 = {
        let mut sid = 0i32;
        let r = ioctl(fd, TIOCGSID, &mut sid as *mut i32 as u64)?;
        Ok(if r < 0 { r } else { i64::from(sid) })
    }
    "setsid" fn setsid() -> i32 = { Ok(sc(nr::SETSID, &[])) }
    "getsid" fn getsid(p: i32) -> i32 = { Ok(sc(nr::GETSID, &[p as u64])) }
    "setpgid" fn setpgid(p: i32, pgid: i32) -> i32 = { Ok(sc(nr::SETPGID, &[p as u64, pgid as u64])) }
    "getpgid" fn getpgid(p: i32) -> i32 = { Ok(sc(nr::GETPGID, &[p as u64])) }
}

/// ptsname: the slave's path, or the errno of a null return.
#[cfg(libc_has = "ptsname")]
fn ptsname(fd: i32) -> Result<Result<String, i64>, CaseError> {
    extern "C" {
        #[link_name = "ptsname"]
        fn f(fd: i32) -> *const core::ffi::c_char;
    }
    // SAFETY: ptsname takes no pointers and returns null or a NUL-terminated string.
    let p = unsafe { f(fd) };
    if p.is_null() { return Ok(Err(errno())); }
    // SAFETY: p is a NUL-terminated string the library keeps until the next call.
    Ok(Ok(unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()))
}
#[cfg(not(libc_has = "ptsname"))]
fn ptsname(fd: i32) -> Result<Result<String, i64>, CaseError> {
    let mut n = 0u32;
    let r = ioctl(fd, TIOCGPTN, &mut n as *mut u32 as u64)?;
    Ok(if r < 0 { Err(-r) } else { Ok(format!("/dev/pts/{n}")) })
}

// ---------------------------------------------------------------------------
// Results, time and waiting.

fn errname(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 5 => "EIO", 6 => "ENXIO", 9 => "EBADF",
        10 => "ECHILD", 11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES", 14 => "EFAULT", 16 => "EBUSY",
        19 => "ENODEV", 22 => "EINVAL", 24 => "EMFILE", 25 => "ENOTTY", 38 => "ENOSYS", 95 => "EOPNOTSUPP",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A result as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if (-4095..0).contains(&ret) { errname(-ret) } else { ret.to_string() } }

fn err<T>(msg: impl Into<String>) -> Result<T, CaseError> { Err(CaseError::Fail(msg.into())) }

/// A call that must succeed: its value, or a failure naming the error.
fn ok(what: &str, ret: i64) -> Result<i64, CaseError> {
    if ret < 0 { err(format!("{what} failed with {}", shown(ret))) } else { Ok(ret) }
}

/// A call that must return exactly 0.
fn zero(what: &str, ret: i64) -> CaseResult {
    check(ret == 0, &format!("{what} returned {}", shown(ret)))
}

/// A call that must fail with `errno`.
fn want(what: &str, ret: i64, errno: i64) -> CaseResult {
    check(ret == -errno, &format!("{what} returned {}, expected {}", shown(ret), errname(errno)))
}

/// A call that must return `expected`.
fn equal(what: &str, ret: i64, expected: i64) -> CaseResult {
    check(ret == expected, &format!("{what} returned {}, expected {expected}", shown(ret)))
}

fn ts(ns: i64) -> [i64; 2] { [ns.div_euclid(NS), ns.rem_euclid(NS)] }
fn mono() -> i64 {
    let mut t = [0i64; 2];
    if sc(nr::CLOCK_GETTIME, &[CLOCK_MONOTONIC, t.as_mut_ptr() as u64]) != 0 { return 0; }
    t[0] * NS + t[1]
}
fn now_ms() -> u64 { (mono() / MS) as u64 }

/// `ms`, cut short so that `reserve` ms of the case's limit remain afterwards.
fn bounded(ms: u64, reserve: u64) -> u64 { ms.min(case_ms_left().saturating_sub(reserve)) }

fn nap() {
    let t = ts(MS);
    let _ = sc(nr::NANOSLEEP, &[t.as_ptr() as u64, 0]);
}

/// Sleep `ms`, resuming after any interruption.
fn sleep_ms(ms: u64) {
    let end = mono() + ms as i64 * MS;
    while mono() < end { nap(); }
}

/// Wait up to `ms` for `cond`, napping between looks.
fn until(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    loop {
        if cond() { return true; }
        if now_ms().saturating_sub(start) >= ms { return false; }
        nap();
    }
}

/// Stay up to `ms`, for a process that only has to exist while the case looks at it.
fn idle(ms: u64) { sleep_ms(bounded(ms, CLEANUP_MS)); }

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }
fn ppid() -> i32 { sc(nr::GETPPID, &[]) as i32 }
fn kill(p: i32, sig: i32) -> i64 { sc(nr::KILL, &[p as u64, sig as u64]) }

/// End the process at once, running no destructors.
fn exit_group(status: i32) -> ! {
    sc(nr::EXIT_GROUP, &[status as u64]);
    loop { core::hint::spin_loop(); }
}

/// A NUL-terminated copy of `s`.
fn c(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// Bytes as text, escaped and quoted, cut short when long.
fn show(bytes: &[u8]) -> String {
    if bytes.len() <= 48 { return format!("\"{}\"", bytes.escape_ascii()); }
    format!("\"{}\"... ({} bytes)", bytes[..48].escape_ascii(), bytes.len())
}

fn on(bits: u32) -> &'static str { if bits != 0 { "on" } else { "off" } }

// ---------------------------------------------------------------------------
// Signals.

fn sigbit(sig: i32) -> u64 { 1 << (sig - 1) }

/// Set the calling thread's signal mask to exactly `sigs`.
fn mask_only(sigs: &[i32]) -> CaseResult {
    let set: u64 = sigs.iter().map(|&s| sigbit(s)).fold(0, |a, b| a | b);
    zero("rt_sigprocmask(SIG_SETMASK)", sc(nr::RT_SIGPROCMASK, &[SIG_SETMASK, &set as *const u64 as u64, 0, 8]))
}

fn set_action(sig: i32, action: &Sigaction) -> CaseResult {
    signal::sigaction(sig, Some(action), None).map_err(|e| CaseError::Fail(format!("sigaction({}) failed: {e}", signal::signame(sig))))
}
/// Give `sig` its default action, whatever the runner left.
fn default(sig: i32) -> CaseResult { set_action(sig, &Sigaction::default_action()) }
fn ignore(sig: i32) -> CaseResult { set_action(sig, &Sigaction::ignore()) }

/// Wait up to `ms` for blocked signal `sig` with sigtimedwait: whether it came.
fn got_signal(sig: i32, ms: u64) -> Result<bool, CaseError> {
    let ms = bounded(ms, CLEANUP_MS);
    let set = sigbit(sig);
    let mut info = [0u8; 128];
    let t = ts(ms as i64 * MS);
    let r = sc(nr::RT_SIGTIMEDWAIT, &[&set as *const u64 as u64, info.as_mut_ptr() as u64, t.as_ptr() as u64, 8]);
    if r == -EAGAIN { return Ok(false); }
    if r == i64::from(sig) { return Ok(true); }
    err(format!("sigtimedwait for {} returned {}", signal::signame(sig), shown(r)))
}

/// Blocked signal `sig` must come within `ms`.
fn gets(sig: i32, ms: u64, who: &str) -> CaseResult {
    check(got_signal(sig, ms)?, &format!("{who} got no {} within {ms} ms", signal::signame(sig)))
}

/// Blocked signal `sig` must not come within `ms`.
fn gets_none(sig: i32, ms: u64, who: &str, when: &str) -> CaseResult {
    check(!got_signal(sig, ms)?, &format!("{who} got {} {when}", signal::signame(sig)))
}

// ---------------------------------------------------------------------------
// Descriptors and pseudo-terminals.

/// A descriptor the case opened, closed when dropped.
struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) { if self.0 >= 0 { let _ = close(self.0); } }
}

fn open_fd(path: &str, flags: i32) -> Result<Fd, CaseError> {
    let r = open(c(path).as_ptr(), flags, 0)?;
    Ok(Fd(ok(&format!("opening {path}"), r)? as i32))
}

/// /dev/null, a file that is not a terminal.
fn null() -> Result<Fd, CaseError> { open_fd("/dev/null", O_RDWR) }

/// A descriptor number that was just closed.
fn closed_fd() -> Result<i32, CaseError> {
    let fd = null()?;
    Ok(fd.0)
}

/// Opening `path` must fail with `errno`.
fn want_open(path: &str, flags: i32, errno: i64, what: &str) -> CaseResult {
    let r = open(c(path).as_ptr(), flags, 0)?;
    if r >= 0 { let _ = close(r as i32); }
    want(what, r, errno)
}

/// A pseudo-terminal pair: the master from posix_openpt, granted and unlocked, and its
/// slave opened with O_NOCTTY. Dropping it closes both.
struct Pty { master: i32, slave: i32, path: String }

impl Pty {
    fn close_master(&mut self) -> CaseResult {
        let r = close(self.master)?;
        self.master = -1;
        zero("closing the master", r)
    }
    fn close_slave(&mut self) -> CaseResult {
        let r = close(self.slave)?;
        self.slave = -1;
        zero("closing the slave", r)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.slave >= 0 { let _ = close(self.slave); }
        if self.master >= 0 { let _ = close(self.master); }
    }
}

/// A master from posix_openpt(O_RDWR | O_NOCTTY).
fn open_master() -> Result<Fd, CaseError> {
    let r = posix_openpt(O_RDWR | O_NOCTTY)?;
    Ok(Fd(ok("posix_openpt(O_RDWR | O_NOCTTY)", r)? as i32))
}

fn open_pty() -> Result<Pty, CaseError> {
    let r = posix_openpt(O_RDWR | O_NOCTTY)?;
    let master = ok("setting up: posix_openpt(O_RDWR | O_NOCTTY)", r)? as i32;
    let mut pty = Pty { master, slave: -1, path: String::new() };
    zero("setting up: grantpt", grantpt(master)?)?;
    zero("setting up: unlockpt", unlockpt(master)?)?;
    pty.path = ptsname(master)?.map_err(|e| CaseError::Fail(format!("setting up: ptsname failed with {}", errname(e))))?;
    let r = open(c(&pty.path).as_ptr(), O_RDWR | O_NOCTTY, 0)?;
    pty.slave = ok(&format!("setting up: opening {}", pty.path), r)? as i32;
    Ok(pty)
}

/// The number at the end of a slave's path, /dev/pts/<n>.
fn pts_number(path: &str) -> Option<&str> {
    path.strip_prefix("/dev/pts/").filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether /dev/pts lists `n`.
fn pts_listed(n: &str) -> Result<bool, CaseError> {
    let dir = std::fs::read_dir("/dev/pts").map_err(|e| format!("reading /dev/pts failed: {e}"))?;
    Ok(dir.filter_map(|entry| entry.ok()).any(|entry| entry.file_name().to_str() == Some(n)))
}

fn attrs(fd: i32, what: &str) -> Result<Termios, CaseError> {
    let mut t = Termios::default();
    let r = tcgetattr(fd, &mut t)?;
    if r != 0 { return err(format!("tcgetattr on {what} failed with {}", shown(r))); }
    Ok(t)
}

fn set_attrs(fd: i32, act: i32, t: &Termios, what: &str) -> CaseResult {
    zero(&format!("tcsetattr on {what}"), tcsetattr(fd, act, t)?)
}

/// Change the slave's attributes with tcgetattr and tcsetattr(TCSANOW).
fn modes(fd: i32, change: impl FnOnce(&mut Termios)) -> CaseResult {
    let mut t = attrs(fd, "the slave")?;
    change(&mut t);
    set_attrs(fd, TCSANOW, &t, "the slave")
}

/// Canonical input with the classic editing characters, no echo, no signals, no input
/// mapping and no output processing: what a case adds is all that changes the bytes.
fn line_mode(t: &mut Termios) {
    t.lflag |= ICANON;
    t.lflag &= !(ECHO | ECHONL | ISIG | IEXTEN | NOFLSH | TOSTOP);
    t.iflag &= !(ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    t.oflag &= !OPOST;
    t.cc[VERASE] = 0x7f;
    t.cc[VKILL] = 0x15;
    t.cc[VEOF] = 0x04;
    t.cc[VEOL] = 0;
}

/// Non-canonical input of one byte at a time, with nothing mapped either way.
fn raw_mode(t: &mut Termios) {
    t.lflag &= !(ICANON | ECHO | ECHONL | ISIG | IEXTEN);
    t.iflag &= !(ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    t.oflag &= !OPOST;
    t.cc[VMIN] = 1;
    t.cc[VTIME] = 0;
}

/// A pair whose slave is in line mode with `change` made.
fn line_pty(change: impl FnOnce(&mut Termios)) -> Result<Pty, CaseError> {
    let pty = open_pty()?;
    modes(pty.slave, |t| { line_mode(t); change(t) })?;
    Ok(pty)
}

/// A pair whose slave is in raw mode with `change` made.
fn raw_pty(change: impl FnOnce(&mut Termios)) -> Result<Pty, CaseError> {
    let pty = open_pty()?;
    modes(pty.slave, |t| { raw_mode(t); change(t) })?;
    Ok(pty)
}

/// Line mode with ISIG and the classic ^C, ^\ and ^Z, and `change` made.
fn signal_modes(fd: i32, change: impl FnOnce(&mut Termios)) -> CaseResult {
    modes(fd, |t| {
        line_mode(t);
        t.lflag |= ISIG;
        t.cc[VINTR] = CTRL_C;
        t.cc[VQUIT] = CTRL_BACKSLASH;
        t.cc[VSUSP] = CTRL_Z;
        change(t)
    })
}

fn get_size(fd: i32, what: &str) -> Result<Winsize, CaseError> {
    let mut ws = Winsize::default();
    zero(&format!("ioctl(TIOCGWINSZ) on {what}"), ioctl(fd, TIOCGWINSZ, &mut ws as *mut Winsize as u64)?)?;
    Ok(ws)
}

fn set_size(fd: i32, ws: Winsize, what: &str) -> CaseResult {
    zero(&format!("ioctl(TIOCSWINSZ) on {what}"), ioctl(fd, TIOCSWINSZ, &ws as *const Winsize as u64)?)
}

fn size_text(ws: Winsize) -> String { format!("{}x{} ({}x{} pixels)", ws.row, ws.col, ws.xpixel, ws.ypixel) }

/// Write `bytes` to `fd` in one call, which must take them all.
fn put(fd: i32, bytes: &[u8], what: &str) -> CaseResult {
    let r = write(fd, bytes.as_ptr(), bytes.len())?;
    check(r == bytes.len() as i64, &format!("writing {} to {what} returned {}", show(bytes), shown(r)))
}

/// Write all of `bytes` to `fd`, in as many calls as it takes.
fn put_all(fd: i32, bytes: &[u8], what: &str) -> CaseResult {
    let mut at = 0;
    while at < bytes.len() {
        let r = write(fd, bytes[at..].as_ptr(), bytes.len() - at)?;
        if r <= 0 { return fail(format!("writing to {what} returned {} after {at} of {} bytes", shown(r), bytes.len())); }
        at += r as usize;
    }
    Ok(())
}

/// What a read found.
enum Got { Data(Vec<u8>), Eof, Error(i64), Nothing }

impl Got {
    fn text(&self, ms: u64) -> String {
        match self {
            Got::Data(d) => format!("read {}", show(d)),
            Got::Eof => "read end of file".to_string(),
            Got::Error(e) => format!("failed with {}", errname(*e)),
            Got::Nothing => format!("had nothing to read within {ms} ms"),
        }
    }
}

/// One read of up to `cap` bytes from `fd`, made non-blocking and retried while it finds
/// nothing, for up to `ms`.
fn read_one(fd: i32, cap: usize, ms: u64) -> Result<Got, CaseError> {
    let flags = ok("fcntl(F_GETFL)", sc(nr::FCNTL, &[fd as u64, F_GETFL]))? as u64;
    zero("fcntl(F_SETFL, O_NONBLOCK)", sc(nr::FCNTL, &[fd as u64, F_SETFL, flags | O_NONBLOCK]))?;
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    let mut buf = vec![0u8; cap];
    let got = loop {
        let r = read(fd, buf.as_mut_ptr(), cap)?;
        if r > 0 { break Got::Data(buf[..r as usize].to_vec()); }
        if r == 0 { break Got::Eof; }
        if r != -EAGAIN && r != -EINTR { break Got::Error(-r); }
        if now_ms().saturating_sub(start) >= ms { break Got::Nothing; }
        nap();
    };
    let _ = sc(nr::FCNTL, &[fd as u64, F_SETFL, flags]);
    Ok(got)
}

fn same(got: &[u8], want: &[u8], what: &str) -> CaseResult {
    if got == want { return Ok(()); }
    if got.len() <= 48 && want.len() <= 48 { return fail(format!("{what} read {}, expected {}", show(got), show(want))); }
    let at = got.iter().zip(want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
    fail(format!("{what} read {} bytes that differ from the {} written, first at byte {at}", got.len(), want.len()))
}

/// One read of up to `cap` bytes from `fd` must return exactly `want` within `ms`.
fn expect_read(fd: i32, cap: usize, want: &[u8], ms: u64, what: &str) -> CaseResult {
    match read_one(fd, cap, ms)? {
        Got::Data(d) => same(&d, want, what),
        other => fail(format!("{what} {}, expected {}", other.text(ms), show(want))),
    }
}

/// Reads from `fd` must deliver exactly `want` within `ms`.
fn expect_bytes(fd: i32, want: &[u8], ms: u64, what: &str) -> CaseResult {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    let mut got = Vec::new();
    while got.len() < want.len() {
        let left = ms.saturating_sub(now_ms().saturating_sub(start));
        match read_one(fd, 4096, left)? {
            Got::Data(d) => got.extend_from_slice(&d),
            other => {
                let so_far = if got.is_empty() { String::new() } else { format!(" ({})", show(&got)) };
                return fail(format!("{what} {} after {} of {} bytes{so_far}", other.text(ms), got.len(), want.len()));
            }
        }
    }
    same(&got, want, what)
}

/// A read from `fd` must find nothing for `ms`.
fn expect_nothing(fd: i32, ms: u64, what: &str, when: &str) -> CaseResult {
    match read_one(fd, 256, ms)? {
        Got::Nothing => Ok(()),
        other => fail(format!("{what} {} {when}; expected nothing to read", other.text(ms))),
    }
}

/// A read from `fd` must find end of file within `ms`.
fn expect_eof(fd: i32, ms: u64, what: &str) -> CaseResult {
    match read_one(fd, 64, ms)? {
        Got::Eof => Ok(()),
        other => fail(format!("{what} {}, expected end of file", other.text(ms))),
    }
}

/// A read from `fd` must fail with `errno` within `ms`.
fn expect_error(fd: i32, errno: i64, ms: u64, what: &str) -> CaseResult {
    match read_one(fd, 64, ms)? {
        Got::Error(e) if e == errno => Ok(()),
        other => fail(format!("{what} {}, expected {}", other.text(ms), errname(errno))),
    }
}

/// poll on one descriptor: its result and revents.
fn poll1(fd: i32, events: i16, timeout_ms: i32) -> Result<(i64, i16), CaseError> {
    let mut p = PollFd { fd, events, revents: 0 };
    let r = poll(&mut p, 1, timeout_ms)?;
    Ok((r, p.revents))
}

/// `n` bytes of a pattern that runs through every byte value.
fn pattern(n: usize) -> Vec<u8> { (0..n).map(|i| (i * 7 + i / 256) as u8).collect() }

// ---------------------------------------------------------------------------
// Shared pages and the processes a case starts.

/// A page the case shares with the processes it starts: words for flags and results, and
/// a message or the bytes a read returned.
#[repr(C, align(64))]
struct Page {
    w: [AtomicI64; 16],
    len: AtomicU32,
    text: [AtomicU8; 480],
}

impl Page {
    fn get(&self, i: usize) -> i64 { self.w[i].load(SeqCst) }
    fn set(&self, i: usize, v: i64) { self.w[i].store(v, SeqCst) }
    fn say_bytes(&self, msg: &[u8]) {
        let n = msg.len().min(self.text.len());
        for (cell, &b) in self.text.iter().zip(&msg[..n]) { cell.store(b, Relaxed); }
        self.len.store(n as u32, SeqCst);
    }
    fn say(&self, msg: &str) { self.say_bytes(msg.as_bytes()) }
    fn bytes(&self) -> Vec<u8> {
        let n = (self.len.load(SeqCst) as usize).min(self.text.len());
        self.text[..n].iter().map(|cell| cell.load(Relaxed)).collect()
    }
    fn said(&self) -> String { String::from_utf8_lossy(&self.bytes()).into_owned() }
}

/// A fresh zeroed page shared with every process the caller forks afterwards.
fn page() -> Result<&'static Page, CaseError> {
    let ret = sc(nr::MMAP, &[0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED_ANON, u64::MAX, 0]);
    if (-4095..0).contains(&ret) { return err(format!("mmap of a shared page failed with {}", errname(-ret))); }
    // SAFETY: a fresh zeroed shared page, large enough for a Page and never unmapped.
    Ok(unsafe { &*(ret as *const Page) })
}

/// Wait up to `ms` for word `i` of `pg` to be set.
fn flag(pg: &Page, i: usize, ms: u64, who: &str) -> CaseResult {
    check(until(ms, || pg.get(i) != 0), &format!("{who} did not get ready within {ms} ms"))
}

/// Record a process's verdict in word `i`: 1 for a pass, 2 for a failure with its message.
fn report(pg: &Page, i: usize, result: CaseResult) {
    match result {
        Ok(()) => pg.set(i, 1),
        Err(CaseError::Fail(m)) | Err(CaseError::Skip(m)) => {
            pg.say(&m);
            pg.set(i, 2);
        }
    }
}

/// The verdict a process recorded in word `i`, within `ms`.
fn verdict(pg: &Page, i: usize, ms: u64, who: &str) -> CaseResult {
    if !until(ms, || pg.get(i) != 0) {
        return fail(format!("{who} reported nothing within {ms} ms: it is stopped, blocked or gone"));
    }
    if pg.get(i) == 1 { Ok(()) } else { fail(format!("{who}: {}", pg.said())) }
}

/// A wait status in words.
fn status_text(status: i32) -> String {
    if status & 0xff == 0x7f { return format!("was stopped by {}", signal::signame((status >> 8) & 0xff)); }
    let sig = status & 0x7f;
    if sig == 0 { format!("exited with status {}", (status >> 8) & 0xff) } else { format!("was killed by {}", signal::signame(sig)) }
}

/// A process the case started. Dropping it kills and reaps it.
struct Child { pid: i32, note: &'static Page, status: Option<i32>, owner: i32 }

/// Start `body` in a process of its own. It ends with status 0 when `body` passes, and
/// with status 1 after putting its message on its page when it fails.
fn spawn(body: impl FnOnce() -> CaseResult) -> Result<Child, CaseError> {
    let note = page()?;
    match process::fork() {
        Ok(ForkResult::Child) => {
            let code = match body() {
                Ok(()) => 0,
                Err(CaseError::Fail(m)) | Err(CaseError::Skip(m)) => { note.say(&m); 1 }
            };
            exit_group(code)
        }
        Ok(ForkResult::Parent(p)) => Ok(Child { pid: p.raw() as i32, note, status: None, owner: pid() }),
        Err(e) => err(format!("fork failed: {e}")),
    }
}

/// Start `body` in a process of its own, in a new process group of its own: a background
/// job when the caller's group is in the foreground. Both the job and the caller put it in
/// its group, as a shell does, so neither waits on the other.
fn job(body: impl FnOnce() -> CaseResult) -> Result<Child, CaseError> {
    let child = spawn(|| {
        zero("setting up: setpgid(0, 0)", setpgid(0, 0)?)?;
        body()
    })?;
    let _ = setpgid(child.pid, child.pid);
    Ok(child)
}

/// Make the calling process the leader of a new session whose controlling terminal is
/// `tty`, by setsid and TIOCSCTTY.
fn take_terminal(tty: i32) -> CaseResult {
    let sid = setsid()?;
    check(sid == i64::from(pid()), &format!("setting up: setsid returned {}", shown(sid)))?;
    zero("setting up: ioctl(TIOCSCTTY)", ioctl(tty, TIOCSCTTY, 0)?)
}

/// Make the calling process the leader of a new session with no controlling terminal.
fn new_session() -> CaseResult {
    let sid = setsid()?;
    check(sid == i64::from(pid()), &format!("setsid returned {}, expected the caller's PID {}", shown(sid), pid()))
}

/// Start `body` in the leader of a new session whose controlling terminal is the pair's
/// slave. The leader's process group is the terminal's foreground group.
fn leader(pty: &Pty, body: impl FnOnce() -> CaseResult) -> Result<Child, CaseError> {
    let tty = pty.slave;
    spawn(move || {
        take_terminal(tty)?;
        body()
    })
}

/// Start the leader of another session, with `tty` as its controlling terminal when one
/// is given, and wait until it has it. It stays until the case drops it.
fn holder(tty: Option<i32>) -> Result<Child, CaseError> {
    let pg = page()?;
    let child = spawn(move || {
        match tty { Some(fd) => take_terminal(fd)?, None => new_session()? }
        pg.set(0, 1);
        idle(9000);
        Ok(())
    })?;
    flag(pg, 0, 2000, "the other session's leader")?;
    Ok(child)
}

/// The PID of a process that has ended and been reaped.
fn dead_pid() -> Result<i32, CaseError> {
    let mut child = spawn(|| Ok(()))?;
    child.finish(2000, "a process that exits at once")?;
    Ok(child.pid)
}

impl Child {
    /// A change of the process's state: its wait status when it has ended or, with
    /// `stops`, when it has stopped. An ended process is reaped.
    fn change(&mut self, stops: bool) -> Option<i32> {
        if self.status.is_some() { return self.status; }
        let mut status = 0i32;
        let options = WNOHANG | if stops { WUNTRACED } else { 0 };
        let r = sc(nr::WAIT4, &[self.pid as u64, &mut status as *mut i32 as u64, options, 0]);
        if r != i64::from(self.pid) { return None; }
        if status & 0xff != 0x7f { self.status = Some(status); }
        Some(status)
    }

    /// The process's next change within `ms`, or a failure saying it made none.
    fn wait_change(&mut self, ms: u64, stops: bool, who: &str) -> Result<i32, CaseError> {
        let ms = bounded(ms, CLEANUP_MS);
        let start = now_ms();
        loop {
            if let Some(status) = self.change(stops) { return Ok(status); }
            if now_ms().saturating_sub(start) >= ms {
                let what = if stops { "neither stopped nor finished" } else { "did not finish" };
                return err(format!("{who} {what} within {ms} ms"));
            }
            nap();
        }
    }

    /// What an ended process's status says: its message when it failed, else its status.
    fn reason(&self, status: i32) -> String {
        let said = self.note.said();
        if status == 1 << 8 && !said.is_empty() { said } else { status_text(status) }
    }

    /// A pass for status 0, otherwise the process's message.
    fn outcome(&self, status: i32, who: &str) -> CaseResult {
        if status == 0 { return Ok(()); }
        fail(format!("{who}: {}", self.reason(status)))
    }

    /// Wait up to `ms` for the process to end, passing if it passed.
    fn finish(&mut self, ms: u64, who: &str) -> CaseResult {
        let status = self.wait_change(ms, false, who)?;
        self.outcome(status, who)
    }

    /// Wait up to `ms` for the process to be stopped by `sig`.
    fn stops(&mut self, sig: i32, ms: u64, who: &str) -> CaseResult {
        let name = signal::signame(sig);
        let status = self.wait_change(ms, true, who).map_err(|_| CaseError::Fail(format!("{who} was not stopped by {name} within {ms} ms")))?;
        if status & 0xff == 0x7f {
            let by = (status >> 8) & 0xff;
            return check(by == sig, &format!("{who} was stopped by {}, expected {name}", signal::signame(by)));
        }
        fail(format!("{who} was not stopped by {name}: {}", self.reason(status)))
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.status.is_some() || pid() != self.owner { return; }
        let _ = kill(self.pid, signal::SIGKILL);
        let start = now_ms();
        while self.change(false).is_none() && now_ms().saturating_sub(start) < 1000 { nap(); }
    }
}

/// The result a reader process recorded on `pg`: word 0 set when its read returned, word 1
/// the read's result, word 2 how long it took in ms, and the bytes it read.
fn blocking_read(pg: &Page, fd: i32) -> CaseResult {
    let mut buf = [0u8; 64];
    let start = mono();
    let r = read(fd, buf.as_mut_ptr(), buf.len())?;
    pg.set(2, (mono() - start) / MS);
    if r > 0 { pg.say_bytes(&buf[..r as usize]); }
    pg.set(1, r);
    pg.set(0, 1);
    Ok(())
}

fn read_result(pg: &Page) -> Got {
    match pg.get(1) {
        r if r < 0 => Got::Error(-r),
        0 => Got::Eof,
        _ => Got::Data(pg.bytes()),
    }
}

// ---------------------------------------------------------------------------
// termios

const FILL: u32 = 0xa5a5_a5a5;

fn t_tcgetattr() -> CaseResult {
    let pty = open_pty()?;
    let mut t = Termios { iflag: FILL, oflag: FILL, cflag: FILL, lflag: FILL, line: 0xa5, cc: [0xa5; NCCS], ispeed: FILL, ospeed: FILL };
    zero("tcgetattr on the slave", tcgetattr(pty.slave, &mut t)?)?;
    check(
        t.iflag != FILL && t.oflag != FILL && t.cflag != FILL && t.lflag != FILL,
        "tcgetattr returned 0 but left the mode words as they were",
    )
}

fn t_tcgetattr_enotty() -> CaseResult {
    let fd = null()?;
    let mut t = Termios::default();
    want("tcgetattr on /dev/null", tcgetattr(fd.0, &mut t)?, ENOTTY)
}

fn t_tcgetattr_ebadf() -> CaseResult {
    let fd = closed_fd()?;
    let mut t = Termios::default();
    want("tcgetattr on a closed descriptor", tcgetattr(fd, &mut t)?, EBADF)
}

/// Set ECHO the other way and VERASE to backspace with `act`, and check tcgetattr reports both.
fn set_and_read_back(pty: &Pty, act: i32, name: &str) -> CaseResult {
    let mut t = attrs(pty.slave, "the slave")?;
    t.lflag ^= ECHO;
    t.cc[VERASE] = 0x08;
    set_attrs(pty.slave, act, &t, "the slave")?;
    let u = attrs(pty.slave, "the slave")?;
    check(
        u.lflag & ECHO == t.lflag & ECHO && u.cc[VERASE] == 0x08,
        &format!(
            "after tcsetattr({name}) set ECHO {} and VERASE 0x08, tcgetattr reports ECHO {} and VERASE {:#04x}",
            on(t.lflag & ECHO), on(u.lflag & ECHO), u.cc[VERASE]
        ),
    )
}

fn t_tcsanow() -> CaseResult {
    let pty = open_pty()?;
    set_and_read_back(&pty, TCSANOW, "TCSANOW")
}

fn t_tcsadrain() -> CaseResult {
    let pty = open_pty()?;
    modes(pty.slave, |t| t.oflag &= !OPOST)?;
    put(pty.slave, b"abc", "the slave")?;
    set_and_read_back(&pty, TCSADRAIN, "TCSADRAIN")?;
    expect_bytes(pty.master, b"abc", 1000, "the master")
}

fn t_tcsaflush() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    put(pty.master, b"abc", "the master")?;
    sleep_ms(50);
    let mut t = attrs(pty.slave, "the slave")?;
    t.cc[VERASE] = 0x08;
    set_attrs(pty.slave, TCSAFLUSH, &t, "the slave")?;
    let u = attrs(pty.slave, "the slave")?;
    check(u.cc[VERASE] == 0x08, &format!("after tcsetattr(TCSAFLUSH) set VERASE 0x08, tcgetattr reports {:#04x}", u.cc[VERASE]))?;
    expect_nothing(pty.slave, 200, "the slave", "after tcsetattr(TCSAFLUSH), which discards unread input")?;
    put(pty.master, b"d", "the master")?;
    expect_read(pty.slave, 16, b"d", 1000, "the slave")
}

fn t_tcsetattr_einval() -> CaseResult {
    let pty = open_pty()?;
    let t = attrs(pty.slave, "the slave")?;
    want("tcsetattr with optional_actions 99", tcsetattr(pty.slave, 99, &t)?, EINVAL)
}

fn t_tcsetattr_enotty() -> CaseResult {
    let fd = null()?;
    let t = Termios::default();
    want("tcsetattr(TCSANOW) on /dev/null", tcsetattr(fd.0, TCSANOW, &t)?, ENOTTY)
}

fn t_canonical_line() -> CaseResult {
    let pty = line_pty(|_| {})?;
    put(pty.master, b"ab", "the master")?;
    expect_nothing(pty.slave, 200, "the slave", "before the line ended")?;
    put(pty.master, b"c\n", "the master")?;
    expect_read(pty.slave, 64, b"abc\n", 1000, "the slave")
}

fn t_canonical_one_line() -> CaseResult {
    let pty = line_pty(|_| {})?;
    put(pty.master, b"one\ntwo\n", "the master")?;
    expect_read(pty.slave, 64, b"one\n", 1000, "the first read on the slave")?;
    expect_read(pty.slave, 64, b"two\n", 1000, "the second read on the slave")
}

fn t_canonical_short_read() -> CaseResult {
    let pty = line_pty(|_| {})?;
    put(pty.master, b"abcdef\n", "the master")?;
    expect_read(pty.slave, 3, b"abc", 1000, "a 3-byte read on the slave")?;
    expect_read(pty.slave, 64, b"def\n", 1000, "the next read on the slave")
}

fn t_verase() -> CaseResult {
    let pty = line_pty(|t| t.cc[VERASE] = 0x7f)?;
    put(pty.master, b"abX\x7fc\n", "the master")?;
    expect_read(pty.slave, 64, b"abc\n", 1000, "the slave")
}

fn t_vkill() -> CaseResult {
    let pty = line_pty(|t| t.cc[VKILL] = 0x15)?;
    put(pty.master, b"junk\x15ok\n", "the master")?;
    expect_read(pty.slave, 64, b"ok\n", 1000, "the slave")
}

fn t_veof() -> CaseResult {
    let pty = line_pty(|t| t.cc[VEOF] = 0x04)?;
    put(pty.master, b"\x04", "the master")?;
    expect_eof(pty.slave, 1000, "the slave, given VEOF at the start of a line,")?;
    put(pty.master, b"x\n", "the master")?;
    expect_read(pty.slave, 64, b"x\n", 1000, "the next read on the slave")
}

fn t_veof_mid_line() -> CaseResult {
    let pty = line_pty(|t| t.cc[VEOF] = 0x04)?;
    put(pty.master, b"abc\x04", "the master")?;
    expect_read(pty.slave, 64, b"abc", 1000, "the slave")
}

fn t_veol() -> CaseResult {
    let pty = line_pty(|t| t.cc[VEOL] = b'!')?;
    put(pty.master, b"ab!", "the master")?;
    expect_read(pty.slave, 64, b"ab!", 1000, "the slave")
}

fn t_noncanonical() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    put(pty.master, b"x", "the master")?;
    expect_read(pty.slave, 64, b"x", 1000, "the slave")
}

fn t_vmin_blocks() -> CaseResult {
    let pty = raw_pty(|t| { t.cc[VMIN] = 3; t.cc[VTIME] = 0 })?;
    let pg = page()?;
    let slave = pty.slave;
    let mut reader = spawn(|| blocking_read(pg, slave))?;
    put(pty.master, b"ab", "the master")?;
    sleep_ms(300);
    if pg.get(0) != 0 {
        return fail(format!("with VMIN 3, the read {} once 2 bytes had arrived", read_result(pg).text(0)));
    }
    put(pty.master, b"c", "the master")?;
    reader.finish(2000, "the reader")?;
    match read_result(pg) {
        Got::Data(d) => same(&d, b"abc", "the reader"),
        other => fail(format!("the reader {}, expected \"abc\"", other.text(0))),
    }
}

/// A blocking read on a slave in raw mode with VMIN and VTIME set and `before` already
/// written: what it returned, and how long it took in ms.
fn timed_read(vmin: u8, vtime: u8, before: &[u8]) -> Result<(Got, i64), CaseError> {
    let pty = raw_pty(|t| { t.cc[VMIN] = vmin; t.cc[VTIME] = vtime })?;
    if !before.is_empty() { put(pty.master, before, "the master")?; }
    let pg = page()?;
    let slave = pty.slave;
    let mut reader = spawn(|| blocking_read(pg, slave))?;
    reader.finish(2000, &format!("the read with VMIN {vmin} and VTIME {vtime}"))?;
    Ok((read_result(pg), pg.get(2)))
}

fn t_vmin_zero() -> CaseResult {
    let (got, ms) = timed_read(0, 0, b"")?;
    value("returned-after", ms, "ms", Some((0, 100)));
    check(matches!(got, Got::Eof), &format!("with VMIN 0 and VTIME 0 and nothing written, the read {}, expected to return 0", got.text(0)))?;
    check(ms <= 100, &format!("with VMIN 0 and VTIME 0 and nothing written, the read took {ms} ms to return"))
}

fn t_vtime_timeout() -> CaseResult {
    let (got, ms) = timed_read(0, 2, b"")?;
    value("returned-after", ms, "ms", Some((180, 500)));
    check(matches!(got, Got::Eof), &format!("with VMIN 0 and VTIME 2 and nothing written, the read {}, expected to return 0", got.text(0)))?;
    check((180..=500).contains(&ms), &format!("with VMIN 0 and VTIME 2, the read returned after {ms} ms, expected about 200"))
}

fn t_vtime_interbyte() -> CaseResult {
    let (got, ms) = timed_read(5, 2, b"ab")?;
    value("returned-after", ms, "ms", Some((150, 1000)));
    match got {
        Got::Data(d) => same(&d, b"ab", "with VMIN 5 and VTIME 2, the read")?,
        other => return fail(format!("with VMIN 5 and VTIME 2 and 2 bytes waiting, the read {}, expected \"ab\"", other.text(0))),
    }
    check((150..=1000).contains(&ms), &format!("with VMIN 5 and VTIME 2, the read returned 2 bytes after {ms} ms, expected after the 200 ms inter-byte timer"))
}

fn t_echo() -> CaseResult {
    let pty = line_pty(|t| t.lflag |= ECHO)?;
    put(pty.master, b"hi\n", "the master")?;
    expect_bytes(pty.master, b"hi\n", 1000, "the master, reading the echo,")
}

fn t_echo_off() -> CaseResult {
    let pty = line_pty(|_| {})?;
    put(pty.master, b"hi\n", "the master")?;
    expect_read(pty.slave, 64, b"hi\n", 1000, "the slave")?;
    expect_nothing(pty.master, 200, "the master", "with ECHO clear")
}

fn t_echonl() -> CaseResult {
    let pty = line_pty(|t| t.lflag |= ECHONL)?;
    put(pty.master, b"ab\n", "the master")?;
    expect_bytes(pty.master, b"\n", 1000, "the master, reading the echo,")
}

fn t_isig_off() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    put(pty.master, &[CTRL_C, CTRL_Z, CTRL_BACKSLASH], "the master")?;
    expect_bytes(pty.slave, &[CTRL_C, CTRL_Z, CTRL_BACKSLASH], 1000, "the slave")
}

fn t_icrnl() -> CaseResult {
    let pty = line_pty(|t| t.iflag |= ICRNL)?;
    put(pty.master, b"ab\r", "the master")?;
    expect_read(pty.slave, 64, b"ab\n", 1000, "the slave")
}

fn t_igncr() -> CaseResult {
    let pty = line_pty(|t| t.iflag |= IGNCR)?;
    put(pty.master, b"a\rb\n", "the master")?;
    expect_read(pty.slave, 64, b"ab\n", 1000, "the slave")
}

fn t_istrip() -> CaseResult {
    let pty = raw_pty(|t| t.iflag |= ISTRIP)?;
    put(pty.master, &[0xc1], "the master")?;
    expect_read(pty.slave, 64, &[0x41], 1000, "the slave")
}

fn t_onlcr() -> CaseResult {
    let pty = open_pty()?;
    modes(pty.slave, |t| t.oflag |= OPOST | ONLCR)?;
    put(pty.slave, b"x\n", "the slave")?;
    expect_bytes(pty.master, b"x\r\n", 1000, "the master")
}

fn t_opost_off() -> CaseResult {
    let pty = open_pty()?;
    modes(pty.slave, |t| t.oflag &= !OPOST)?;
    put(pty.slave, b"x\n", "the slave")?;
    expect_bytes(pty.master, b"x\n", 1000, "the master")
}

fn t_cfospeed() -> CaseResult {
    let mut t = Termios::default();
    zero("cfsetospeed(B9600)", cfsetospeed(&mut t, B9600)?)?;
    equal("cfgetospeed after cfsetospeed(B9600)", cfgetospeed(&t)?, i64::from(B9600))
}

fn t_cfispeed() -> CaseResult {
    let mut t = Termios::default();
    zero("cfsetispeed(B19200)", cfsetispeed(&mut t, B19200)?)?;
    equal("cfgetispeed after cfsetispeed(B19200)", cfgetispeed(&t)?, i64::from(B19200))
}

fn t_cfspeed_einval() -> CaseResult {
    let mut t = Termios::default();
    want("cfsetospeed(12345)", cfsetospeed(&mut t, 12345)?, EINVAL)
}

fn t_speed_round_trip() -> CaseResult {
    let pty = open_pty()?;
    let mut t = attrs(pty.slave, "the slave")?;
    zero("cfsetospeed(B9600)", cfsetospeed(&mut t, B9600)?)?;
    zero("cfsetispeed(B9600)", cfsetispeed(&mut t, B9600)?)?;
    set_attrs(pty.slave, TCSANOW, &t, "the slave")?;
    let u = attrs(pty.slave, "the slave")?;
    equal("cfgetospeed of the attributes tcgetattr returned", cfgetospeed(&u)?, i64::from(B9600))
}

fn t_tcflush_input() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    put(pty.master, b"abc", "the master")?;
    sleep_ms(50);
    zero("tcflush(TCIFLUSH)", tcflush(pty.slave, TCIFLUSH)?)?;
    expect_nothing(pty.slave, 200, "the slave", "after tcflush(TCIFLUSH)")?;
    put(pty.master, b"d", "the master")?;
    expect_read(pty.slave, 16, b"d", 1000, "the slave")
}

fn t_tcflush_einval() -> CaseResult {
    let pty = open_pty()?;
    want("tcflush with queue_selector 7", tcflush(pty.slave, 7)?, EINVAL)
}

fn t_tcflush_enotty() -> CaseResult {
    let fd = null()?;
    want("tcflush(TCIFLUSH) on /dev/null", tcflush(fd.0, TCIFLUSH)?, ENOTTY)
}

fn t_tcdrain() -> CaseResult {
    let pty = open_pty()?;
    modes(pty.slave, |t| t.oflag &= !OPOST)?;
    put(pty.slave, b"abc", "the slave")?;
    zero("tcdrain on the slave", tcdrain(pty.slave)?)?;
    expect_bytes(pty.master, b"abc", 1000, "the master")
}

fn t_tcflow() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    zero("tcflow(TCOOFF)", tcflow(pty.slave, TCOOFF)?)?;
    let slave = pty.slave;
    let mut writer = spawn(|| put(slave, b"zz", "the slave"))?;
    expect_nothing(pty.master, 300, "the master", "while output was suspended by TCOOFF")?;
    zero("tcflow(TCOON)", tcflow(pty.slave, TCOON)?)?;
    expect_bytes(pty.master, b"zz", 1000, "the master, once TCOON restarted output,")?;
    writer.finish(2000, "the writer")
}

fn t_tcsendbreak() -> CaseResult {
    let pty = open_pty()?;
    zero("tcsendbreak(0) on the slave", tcsendbreak(pty.slave, 0)?)
}

// ---------------------------------------------------------------------------
// pty

fn p_openpt() -> CaseResult {
    open_master().map(|_| ())
}

fn p_openpt_distinct() -> CaseResult {
    let a = open_pty()?;
    let b = open_pty()?;
    check(a.path != b.path, &format!("two masters both have the slave {}", a.path))
}

fn p_ptmx() -> CaseResult {
    let m = open_fd("/dev/ptmx", O_RDWR | O_NOCTTY)?;
    match ptsname(m.0)? {
        Ok(name) => check(pts_number(&name).is_some(), &format!("ptsname of the descriptor /dev/ptmx opened returned {name}")),
        Err(e) => fail(format!("ptsname of the descriptor /dev/ptmx opened failed with {}", errname(e))),
    }
}

fn p_grantpt() -> CaseResult {
    let m = open_master()?;
    zero("grantpt", grantpt(m.0)?)
}

fn p_grantpt_ebadf() -> CaseResult {
    let fd = closed_fd()?;
    want("grantpt on a closed descriptor", grantpt(fd)?, EBADF)
}

fn p_grantpt_einval() -> CaseResult {
    let fd = null()?;
    want("grantpt on /dev/null", grantpt(fd.0)?, EINVAL)
}

fn p_unlockpt() -> CaseResult {
    let m = open_master()?;
    zero("grantpt", grantpt(m.0)?)?;
    zero("unlockpt", unlockpt(m.0)?)?;
    let path = ptsname(m.0)?.map_err(|e| CaseError::Fail(format!("ptsname failed with {}", errname(e))))?;
    open_fd(&path, O_RDWR | O_NOCTTY).map(|_| ())
}

fn p_unlockpt_einval() -> CaseResult {
    let fd = null()?;
    want("unlockpt on /dev/null", unlockpt(fd.0)?, EINVAL)
}

fn p_locked() -> CaseResult {
    let m = open_master()?;
    zero("grantpt", grantpt(m.0)?)?;
    let path = ptsname(m.0)?.map_err(|e| CaseError::Fail(format!("ptsname failed with {}", errname(e))))?;
    want_open(&path, O_RDWR | O_NOCTTY, EIO, &format!("opening {path} before unlockpt"))
}

fn p_ptsname() -> CaseResult {
    let pty = open_pty()?;
    check(pts_number(&pty.path).is_some(), &format!("ptsname returned {}, expected /dev/pts/<n>", pty.path))?;
    put(pty.slave, b"ping", "the slave")?;
    expect_bytes(pty.master, b"ping", 1000, "the master")
}

fn p_ptsname_enotty() -> CaseResult {
    let fd = null()?;
    match ptsname(fd.0)? {
        Ok(name) => fail(format!("ptsname on /dev/null returned {name}, expected ENOTTY")),
        Err(e) => check(e == ENOTTY, &format!("ptsname on /dev/null failed with {}, expected ENOTTY", errname(e))),
    }
}

fn p_slave_isatty() -> CaseResult {
    let pty = open_pty()?;
    equal("isatty on the slave", isatty(pty.slave)?, 1)
}

fn p_master_to_slave() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    put(pty.master, b"hello", "the master")?;
    expect_bytes(pty.slave, b"hello", 1000, "the slave")
}

fn p_slave_to_master() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    put(pty.slave, b"world", "the slave")?;
    expect_bytes(pty.master, b"world", 1000, "the master")
}

/// 16 KiB written to `from` by another process arrive in order at `to`.
fn bulk(down: bool) -> CaseResult {
    let pty = raw_pty(|_| {})?;
    let data = pattern(16 * 1024);
    let (from, to, names) = if down { (pty.master, pty.slave, ("the master", "the slave")) } else { (pty.slave, pty.master, ("the slave", "the master")) };
    let mut writer = spawn(|| put_all(from, &data, names.0))?;
    expect_bytes(to, &data, 4000, names.1)?;
    writer.finish(2000, "the writer")
}

fn p_bulk_down() -> CaseResult { bulk(true) }
fn p_bulk_up() -> CaseResult { bulk(false) }

fn p_master_poll() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    let (r, ev) = poll1(pty.master, POLLIN, 0)?;
    check(r == 0, &format!("poll on the master returned {} (revents {ev:#x}) before the slave wrote", shown(r)))?;
    put(pty.slave, b"x", "the slave")?;
    let (r, ev) = poll1(pty.master, POLLIN, 1000)?;
    check(r == 1 && ev & POLLIN != 0, &format!("poll on the master returned {} with revents {ev:#x} once the slave wrote, expected POLLIN", shown(r)))
}

fn p_slave_poll() -> CaseResult {
    let pty = raw_pty(|_| {})?;
    let (r, ev) = poll1(pty.slave, POLLIN, 0)?;
    check(r == 0, &format!("poll on the slave returned {} (revents {ev:#x}) before the master wrote", shown(r)))?;
    put(pty.master, b"x", "the master")?;
    let (r, ev) = poll1(pty.slave, POLLIN, 1000)?;
    check(r == 1 && ev & POLLIN != 0, &format!("poll on the slave returned {} with revents {ev:#x} once the master wrote, expected POLLIN", shown(r)))
}

fn p_master_nonblock() -> CaseResult {
    let pty = open_pty()?;
    match read_one(pty.master, 16, 0)? {
        Got::Nothing => Ok(()),
        other => fail(format!("a non-blocking read on the master {}, expected EAGAIN", other.text(0))),
    }
}

fn p_slave_close_eio() -> CaseResult {
    let mut pty = open_pty()?;
    pty.close_slave()?;
    expect_error(pty.master, EIO, 1000, "a read on the master after the slave closed")
}

fn p_slave_close_data() -> CaseResult {
    let mut pty = raw_pty(|_| {})?;
    put(pty.slave, b"bye", "the slave")?;
    pty.close_slave()?;
    expect_bytes(pty.master, b"bye", 1000, "the master")?;
    expect_error(pty.master, EIO, 1000, "the next read on the master")
}

fn p_slave_close_pollhup() -> CaseResult {
    let mut pty = open_pty()?;
    pty.close_slave()?;
    let (r, ev) = poll1(pty.master, POLLIN, 1000)?;
    check(r == 1 && ev & POLLHUP != 0, &format!("poll on the master after the slave closed returned {} with revents {ev:#x}, expected POLLHUP", shown(r)))
}

fn p_master_close_eof() -> CaseResult {
    let mut pty = open_pty()?;
    pty.close_master()?;
    expect_eof(pty.slave, 1000, "a read on the slave after the master closed")
}

fn p_master_close_eio() -> CaseResult {
    let mut pty = open_pty()?;
    pty.close_master()?;
    let r = write(pty.slave, b"x".as_ptr(), 1)?;
    want("a write on the slave after the master closed", r, EIO)
}

fn p_master_close_sighup() -> CaseResult {
    let mut pty = open_pty()?;
    let pg = page()?;
    let master = pty.master;
    let mut l = leader(&pty, || {
        mask_only(&[SIGHUP])?;
        zero("closing the session leader's copy of the master", close(master)?)?;
        pg.set(0, 1);
        gets(SIGHUP, 3000, "the session leader, once the master closed,")
    })?;
    flag(pg, 0, 2000, "the session leader")?;
    pty.close_master()?;
    l.finish(4000, "the session leader")
}

fn p_winsize_get() -> CaseResult {
    let pty = open_pty()?;
    get_size(pty.slave, "the slave").map(|_| ())
}

/// A size set on one side of the pair is read back on the other.
fn size_across(ws: Winsize, on_master: bool) -> CaseResult {
    let pty = open_pty()?;
    let (to, from) = if on_master { ((pty.master, "the master"), (pty.slave, "the slave")) } else { ((pty.slave, "the slave"), (pty.master, "the master")) };
    set_size(to.0, ws, to.1)?;
    let got = get_size(from.0, from.1)?;
    check(got == ws, &format!("TIOCSWINSZ on {} set {}, and TIOCGWINSZ on {} reports {}", to.1, size_text(ws), from.1, size_text(got)))
}

fn p_winsize_set() -> CaseResult { size_across(Winsize { row: 24, col: 80, xpixel: 640, ypixel: 384 }, true) }
fn p_winsize_slave() -> CaseResult { size_across(Winsize { row: 50, col: 132, xpixel: 0, ypixel: 0 }, false) }

fn p_winsize_enotty() -> CaseResult {
    let fd = null()?;
    let mut ws = Winsize::default();
    want("ioctl(TIOCGWINSZ) on /dev/null", ioctl(fd.0, TIOCGWINSZ, &mut ws as *mut Winsize as u64)?, ENOTTY)
}

fn p_sigwinch() -> CaseResult {
    let pty = open_pty()?;
    let pg = page()?;
    let mut l = leader(&pty, || {
        mask_only(&[SIGWINCH])?;
        pg.set(0, 1);
        gets(SIGWINCH, 3000, "the foreground process, once the size changed,")
    })?;
    flag(pg, 0, 2000, "the session leader")?;
    set_size(pty.master, Winsize { row: 30, col: 100, xpixel: 0, ypixel: 0 }, "the master")?;
    l.finish(4000, "the session leader")
}

fn p_sigwinch_same() -> CaseResult {
    let pty = open_pty()?;
    let size = Winsize { row: 33, col: 99, xpixel: 0, ypixel: 0 };
    set_size(pty.master, size, "the master")?;
    let pg = page()?;
    let mut l = leader(&pty, || {
        mask_only(&[SIGWINCH])?;
        pg.set(0, 1);
        flag(pg, 1, 2000, "the case")?;
        gets_none(SIGWINCH, 300, "the foreground process", "when the size was set to the size it already was")?;
        pg.set(2, 1);
        gets(SIGWINCH, 3000, "the foreground process, once the size did change,")
    })?;
    flag(pg, 0, 2000, "the session leader")?;
    set_size(pty.master, size, "the master")?;
    pg.set(1, 1);
    flag(pg, 2, 2000, "the session leader")?;
    set_size(pty.master, Winsize { row: 34, col: 99, xpixel: 0, ypixel: 0 }, "the master")?;
    l.finish(4000, "the session leader")
}

fn p_master_tcsetattr() -> CaseResult {
    let pty = open_pty()?;
    let mut t = attrs(pty.master, "the master")?;
    t.lflag ^= ECHO;
    set_attrs(pty.master, TCSANOW, &t, "the master")?;
    let u = attrs(pty.slave, "the slave")?;
    check(u.lflag & ECHO == t.lflag & ECHO, &format!("tcsetattr on the master set ECHO {}, and tcgetattr on the slave reports ECHO {}", on(t.lflag & ECHO), on(u.lflag & ECHO)))
}

fn p_pts_listed() -> CaseResult {
    let pty = open_pty()?;
    let n = pts_number(&pty.path).ok_or_else(|| format!("ptsname returned {}, expected /dev/pts/<n>", pty.path))?;
    check(pts_listed(n)?, &format!("/dev/pts does not list {n} while {} is open", pty.path))
}

fn p_pts_removed() -> CaseResult {
    let mut pty = open_pty()?;
    let n = pts_number(&pty.path).ok_or_else(|| format!("ptsname returned {}, expected /dev/pts/<n>", pty.path))?.to_string();
    check(pts_listed(&n)?, &format!("/dev/pts does not list {n} while {} is open", pty.path))?;
    pty.close_slave()?;
    pty.close_master()?;
    let mut listed = Ok(true);
    let gone = until(1000, || { listed = pts_listed(&n); !matches!(listed, Ok(true)) });
    listed?;
    check(gone, &format!("/dev/pts still lists {n} a second after both ends closed"))
}

// ---------------------------------------------------------------------------
// controlling

fn c_setsid() -> CaseResult {
    spawn(new_session)?.finish(3000, "the new process")
}

fn c_setsid_ids() -> CaseResult {
    spawn(|| {
        new_session()?;
        let me = i64::from(pid());
        equal("getpgid(0) after setsid", getpgid(0)?, me)?;
        equal("getsid(0) after setsid", getsid(0)?, me)
    })?
    .finish(3000, "the new session's leader")
}

fn c_setsid_no_ctty() -> CaseResult {
    spawn(|| {
        new_session()?;
        want_open("/dev/tty", O_RDWR, ENXIO, "opening /dev/tty after setsid")
    })?
    .finish(3000, "the new session's leader")
}

fn c_setsid_eperm() -> CaseResult {
    spawn(|| {
        zero("setpgid(0, 0)", setpgid(0, 0)?)?;
        want("setsid in a process group leader", setsid()?, EPERM)
    })?
    .finish(3000, "the group leader")
}

fn c_setsid_leaves() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    leader(&pty, || {
        spawn(|| {
            new_session()?;
            want("tcgetsid on the old controlling terminal after setsid", tcgetsid(slave)?, ENOTTY)
        })?
        .finish(3000, "the child that made its own session")
    })?
    .finish(5000, "the session leader")
}

fn c_open_acquires() -> CaseResult {
    let pty = open_pty()?;
    let path = pty.path.clone();
    spawn(|| {
        new_session()?;
        let fd = open_fd(&path, O_RDWR)?;
        equal("tcgetsid on the terminal the session leader opened without O_NOCTTY", tcgetsid(fd.0)?, i64::from(pid()))
    })?
    .finish(3000, "the new session's leader")
}

fn c_open_noctty() -> CaseResult {
    let pty = open_pty()?;
    let path = pty.path.clone();
    spawn(|| {
        new_session()?;
        let fd = open_fd(&path, O_RDWR | O_NOCTTY)?;
        want("tcgetsid on the terminal the session leader opened with O_NOCTTY", tcgetsid(fd.0)?, ENOTTY)
    })?
    .finish(3000, "the new session's leader")
}

fn c_open_not_leader() -> CaseResult {
    let pty = open_pty()?;
    let path = pty.path.clone();
    spawn(|| {
        new_session()?;
        spawn(|| {
            let fd = open_fd(&path, O_RDWR)?;
            want("tcgetsid on the terminal a session member that is not its leader opened", tcgetsid(fd.0)?, ENOTTY)
        })?
        .finish(3000, "the session member")
    })?
    .finish(5000, "the new session's leader")
}

fn c_open_taken() -> CaseResult {
    let pty = open_pty()?;
    let _other = holder(Some(pty.slave))?;
    let path = pty.path.clone();
    spawn(|| {
        new_session()?;
        let fd = open_fd(&path, O_RDWR)?;
        want("tcgetsid on another session's controlling terminal, opened without O_NOCTTY", tcgetsid(fd.0)?, ENOTTY)
    })?
    .finish(3000, "the second session's leader")
}

fn c_tiocsctty() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    spawn(|| {
        new_session()?;
        zero("ioctl(TIOCSCTTY)", ioctl(slave, TIOCSCTTY, 0)?)?;
        equal("tcgetsid after TIOCSCTTY", tcgetsid(slave)?, i64::from(pid()))
    })?
    .finish(3000, "the new session's leader")
}

fn c_tiocsctty_not_leader() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    spawn(|| want("ioctl(TIOCSCTTY) in a process that is not a session leader", ioctl(slave, TIOCSCTTY, 0)?, EPERM))?
        .finish(3000, "the process")
}

fn c_tiocsctty_has() -> CaseResult {
    let first = open_pty()?;
    let second = open_pty()?;
    let (a, b) = (first.slave, second.slave);
    spawn(|| {
        take_terminal(a)?;
        want("ioctl(TIOCSCTTY) on a second terminal", ioctl(b, TIOCSCTTY, 0)?, EPERM)
    })?
    .finish(3000, "the session leader")
}

fn c_tiocsctty_taken() -> CaseResult {
    let pty = open_pty()?;
    let _other = holder(Some(pty.slave))?;
    let slave = pty.slave;
    spawn(|| {
        new_session()?;
        want("ioctl(TIOCSCTTY, 0) on another session's controlling terminal", ioctl(slave, TIOCSCTTY, 0)?, EPERM)
    })?
    .finish(3000, "the second session's leader")
}

fn c_tiocsctty_steal() -> CaseResult {
    let pty = open_pty()?;
    let _other = holder(Some(pty.slave))?;
    let slave = pty.slave;
    spawn(|| {
        new_session()?;
        zero("root's ioctl(TIOCSCTTY, 1) on another session's controlling terminal", ioctl(slave, TIOCSCTTY, 1)?)?;
        equal("tcgetsid after taking the terminal", tcgetsid(slave)?, i64::from(pid()))
    })?
    .finish(3000, "the second session's leader")
}

fn c_tiocnotty() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    leader(&pty, || {
        spawn(|| {
            zero("ioctl(TIOCNOTTY) in a session member", ioctl(slave, TIOCNOTTY, 0)?)?;
            want("tcgetsid after TIOCNOTTY", tcgetsid(slave)?, ENOTTY)
        })?
        .finish(3000, "the session member")?;
        equal("tcgetsid in the session leader after a member's TIOCNOTTY", tcgetsid(slave)?, i64::from(pid()))
    })?
    .finish(5000, "the session leader")
}

fn c_tiocnotty_enotty() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    spawn(|| {
        new_session()?;
        want("ioctl(TIOCNOTTY) on a terminal that is not the caller's controlling terminal", ioctl(slave, TIOCNOTTY, 0)?, ENOTTY)
    })?
    .finish(3000, "the new session's leader")
}

fn c_tiocnotty_leader() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    let pg = page()?;
    leader(&pty, || {
        mask_only(&[SIGHUP])?;
        let mut member = spawn(|| {
            pg.set(0, 1);
            gets(SIGHUP, 3000, "the foreground member, once the leader gave up the terminal,")
        })?;
        flag(pg, 0, 2000, "the foreground member")?;
        zero("ioctl(TIOCNOTTY) in the session leader", ioctl(slave, TIOCNOTTY, 0)?)?;
        member.finish(4000, "the foreground member")?;
        want("tcgetsid in the leader after its TIOCNOTTY", tcgetsid(slave)?, ENOTTY)
    })?
    .finish(6000, "the session leader")
}

fn c_tcgetsid() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    leader(&pty, || {
        let me = i64::from(pid());
        spawn(|| equal("tcgetsid in a session member", tcgetsid(slave)?, me))?.finish(3000, "the session member")
    })?
    .finish(5000, "the session leader")
}

fn c_tcgetsid_other() -> CaseResult {
    let pty = open_pty()?;
    let _other = holder(Some(pty.slave))?;
    want("tcgetsid on another session's controlling terminal", tcgetsid(pty.slave)?, ENOTTY)
}

fn c_tcgetsid_enotty() -> CaseResult {
    let fd = null()?;
    want("tcgetsid on /dev/null", tcgetsid(fd.0)?, ENOTTY)
}

fn c_getsid() -> CaseResult {
    spawn(|| {
        new_session()?;
        let me = i64::from(pid());
        spawn(|| equal("getsid(0) in a session member", getsid(0)?, me))?.finish(3000, "the session member")
    })?
    .finish(5000, "the session leader")
}

fn c_getsid_other() -> CaseResult {
    let other = holder(None)?;
    equal("getsid of another session's leader", getsid(other.pid)?, i64::from(other.pid))
}

fn c_getsid_esrch() -> CaseResult {
    let gone = dead_pid()?;
    want("getsid of a process that has ended", getsid(gone)?, ESRCH)
}

fn c_getpgid() -> CaseResult {
    let mine = ok("getpgid(0)", getpgid(0)?)?;
    spawn(|| {
        equal("getpgid(0) in a forked child", getpgid(0)?, mine)?;
        equal("getpgid(getpid()) in a forked child", getpgid(pid())?, mine)
    })?
    .finish(3000, "the child")
}

fn c_getpgid_other() -> CaseResult {
    let pg = page()?;
    let child = spawn(|| {
        zero("setpgid(0, 0)", setpgid(0, 0)?)?;
        pg.set(0, 1);
        idle(9000);
        Ok(())
    })?;
    flag(pg, 0, 2000, "the child")?;
    equal("getpgid of a child that made its own group", getpgid(child.pid)?, i64::from(child.pid))
}

fn c_getpgid_esrch() -> CaseResult {
    let gone = dead_pid()?;
    want("getpgid of a process that has ended", getpgid(gone)?, ESRCH)
}

fn c_setpgid() -> CaseResult {
    spawn(|| {
        zero("setpgid(0, 0)", setpgid(0, 0)?)?;
        equal("getpgid(0) after setpgid(0, 0)", getpgid(0)?, i64::from(pid()))
    })?
    .finish(3000, "the process")
}

fn c_setpgid_child() -> CaseResult {
    let child = spawn(|| { idle(9000); Ok(()) })?;
    zero("setpgid of a child into a group of its own", setpgid(child.pid, child.pid)?)?;
    equal("getpgid of the child", getpgid(child.pid)?, i64::from(child.pid))
}

fn c_setpgid_join() -> CaseResult {
    let pg = page()?;
    spawn(|| {
        new_session()?;
        let first = spawn(|| {
            zero("setpgid(0, 0)", setpgid(0, 0)?)?;
            pg.set(0, 1);
            idle(9000);
            Ok(())
        })?;
        flag(pg, 0, 2000, "the group's first process")?;
        let group = first.pid;
        spawn(|| {
            zero("setpgid into a group in the same session", setpgid(0, group)?)?;
            equal("getpgid(0) after joining the group", getpgid(0)?, i64::from(group))
        })?
        .finish(3000, "the process joining the group")
    })?
    .finish(6000, "the session leader")
}

fn c_setpgid_other_session() -> CaseResult {
    let other = holder(None)?;
    let group = other.pid;
    spawn(|| want("setpgid into a group in another session", setpgid(0, group)?, EPERM))?.finish(3000, "the process")
}

fn c_setpgid_leader() -> CaseResult {
    spawn(|| {
        new_session()?;
        want("setpgid(0, 0) in a session leader", setpgid(0, 0)?, EPERM)
    })?
    .finish(3000, "the session leader")
}

fn c_setpgid_child_session() -> CaseResult {
    let other = holder(None)?;
    want("setpgid of a child that made its own session", setpgid(other.pid, other.pid)?, EPERM)
}

fn c_setpgid_esrch() -> CaseResult {
    let gone = dead_pid()?;
    want("setpgid of a process that has ended", setpgid(gone, gone)?, ESRCH)
}

fn c_setpgid_einval() -> CaseResult {
    spawn(|| want("setpgid(0, -1)", setpgid(0, -1)?, EINVAL))?.finish(3000, "the process")
}

fn c_dev_tty() -> CaseResult {
    let pty = open_pty()?;
    modes(pty.slave, |t| t.oflag &= !OPOST)?;
    leader(&pty, || {
        let tty = open_fd("/dev/tty", O_RDWR)?;
        put(tty.0, b"via-tty", "/dev/tty")
    })?
    .finish(3000, "the session leader")?;
    expect_bytes(pty.master, b"via-tty", 1000, "the master")
}

fn c_ctty_inherited() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    leader(&pty, || {
        let me = i64::from(pid());
        spawn(|| {
            open_fd("/dev/tty", O_RDWR)?;
            equal("tcgetsid in a forked child of the session leader", tcgetsid(slave)?, me)
        })?
        .finish(3000, "the forked child")
    })?
    .finish(5000, "the session leader")
}

fn c_exit_sighup() -> CaseResult {
    let pty = open_pty()?;
    let pg = page()?;
    leader(&pty, || {
        mask_only(&[SIGHUP])?;
        let member = spawn(|| {
            pg.set(0, 1);
            report(pg, 1, gets(SIGHUP, 3000, "the foreground member, once the controlling process exited,"));
            Ok(())
        })?;
        flag(pg, 0, 2000, "the foreground member")?;
        std::mem::forget(member);
        Ok(())
    })?
    .finish(3000, "the session leader")?;
    verdict(pg, 1, 3500, "the foreground member")
}

fn c_exit_fg_only() -> CaseResult {
    let pty = open_pty()?;
    let pg = page()?;
    leader(&pty, || {
        mask_only(&[SIGHUP])?;
        let member = spawn(|| {
            pg.set(0, 1);
            report(pg, 1, gets(SIGHUP, 3000, "the foreground member, once the controlling process exited,"));
            Ok(())
        })?;
        let background = job(|| {
            pg.set(2, 1);
            let result = if until(4000, || pg.get(1) != 0) {
                gets_none(SIGHUP, 300, "the background member", "when the controlling process exited")
            } else {
                fail("the foreground member never reported")
            };
            report(pg, 3, result);
            Ok(())
        })?;
        flag(pg, 0, 2000, "the foreground member")?;
        flag(pg, 2, 2000, "the background member")?;
        std::mem::forget(member);
        std::mem::forget(background);
        Ok(())
    })?
    .finish(3000, "the session leader")?;
    verdict(pg, 1, 3500, "the foreground member")?;
    verdict(pg, 3, 1500, "the background member")
}

fn c_exit_frees() -> CaseResult {
    let pty = open_pty()?;
    leader(&pty, || Ok(()))?.finish(3000, "the first session leader")?;
    let slave = pty.slave;
    spawn(|| {
        new_session()?;
        zero("ioctl(TIOCSCTTY, 0) once the old controlling process had exited", ioctl(slave, TIOCSCTTY, 0)?)?;
        equal("tcgetsid after taking the terminal", tcgetsid(slave)?, i64::from(pid()))
    })?
    .finish(3000, "the second session leader")
}

// ---------------------------------------------------------------------------
// job-control

fn j_tcgetpgrp() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    leader(&pty, || equal("tcgetpgrp in the session leader", tcgetpgrp(slave)?, i64::from(pid())))?
        .finish(3000, "the session leader")
}

fn j_tcsetpgrp() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    leader(&pty, || {
        let other = job(|| { idle(9000); Ok(()) })?;
        zero("tcsetpgrp to another group in the session", tcsetpgrp(slave, other.pid)?)?;
        equal("tcgetpgrp after tcsetpgrp", tcgetpgrp(slave)?, i64::from(other.pid))
    })?
    .finish(4000, "the session leader")
}

fn j_tcsetpgrp_eperm() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    let theirs = ok("getpgid(0)", getpgid(0)?)? as i32;
    leader(&pty, || want("tcsetpgrp to a group in another session", tcsetpgrp(slave, theirs)?, EPERM))?
        .finish(3000, "the session leader")
}

fn j_tcsetpgrp_einval() -> CaseResult {
    let pty = open_pty()?;
    let slave = pty.slave;
    leader(&pty, || want("tcsetpgrp(-1)", tcsetpgrp(slave, -1)?, EINVAL))?.finish(3000, "the session leader")
}

fn j_tcsetpgrp_enotty() -> CaseResult {
    let pty = open_pty()?;
    let mine = ok("getpgid(0)", getpgid(0)?)? as i32;
    want("tcsetpgrp on a terminal that is not the caller's controlling terminal", tcsetpgrp(pty.slave, mine)?, ENOTTY)
}

fn j_tcgetpgrp_enotty() -> CaseResult {
    let pty = open_pty()?;
    want("tcgetpgrp on a terminal that is not the caller's controlling terminal", tcgetpgrp(pty.slave)?, ENOTTY)
}

/// In a new session on a fresh pair whose slave has had `change` made, run `body` in a
/// background job, and check it with `outcome`.
fn in_background(
    change: impl FnOnce(&mut Termios),
    body: impl FnOnce(i32) -> CaseResult,
    outcome: impl FnOnce(&mut Child) -> CaseResult,
) -> Result<Pty, CaseError> {
    let pty = open_pty()?;
    modes(pty.slave, change)?;
    let slave = pty.slave;
    leader(&pty, || {
        let mut background = job(|| body(slave))?;
        outcome(&mut background)
    })?
    .finish(6000, "the session leader")?;
    Ok(pty)
}

fn j_bg_tcsetpgrp() -> CaseResult {
    in_background(
        |_| {},
        |slave| {
            default(SIGTTOU)?;
            mask_only(&[])?;
            let r = tcsetpgrp(slave, pid())?;
            fail(format!("tcsetpgrp returned {}", shown(r)))
        },
        |b| b.stops(SIGTTOU, 2000, "the background process calling tcsetpgrp"),
    )
    .map(|_| ())
}

/// A background process with SIGTTOU blocked or ignored calls tcsetpgrp to its own group.
fn bg_tcsetpgrp_allowed(blocked: bool) -> CaseResult {
    in_background(
        |_| {},
        |slave| {
            if blocked { mask_only(&[SIGTTOU])? } else { ignore(SIGTTOU)?; mask_only(&[])? }
            zero("tcsetpgrp to its own group", tcsetpgrp(slave, pid())?)?;
            equal("tcgetpgrp after tcsetpgrp", tcgetpgrp(slave)?, i64::from(pid()))
        },
        |b| b.finish(3000, "the background process"),
    )
    .map(|_| ())
}

fn j_bg_tcsetpgrp_blocked() -> CaseResult { bg_tcsetpgrp_allowed(true) }
fn j_bg_tcsetpgrp_ignored() -> CaseResult { bg_tcsetpgrp_allowed(false) }

fn j_fg_read() -> CaseResult {
    let pty = line_pty(|_| {})?;
    let pg = page()?;
    let slave = pty.slave;
    let mut l = leader(&pty, || {
        pg.set(0, 1);
        expect_read(slave, 64, b"fg\n", 2000, "the foreground process")
    })?;
    flag(pg, 0, 2000, "the session leader")?;
    put(pty.master, b"fg\n", "the master")?;
    l.finish(3000, "the session leader")
}

fn j_bg_read() -> CaseResult {
    in_background(
        line_mode,
        |slave| {
            default(SIGTTIN)?;
            mask_only(&[])?;
            let mut buf = [0u8; 16];
            let r = read(slave, buf.as_mut_ptr(), buf.len())?;
            fail(format!("read returned {}", shown(r)))
        },
        |b| b.stops(SIGTTIN, 2000, "the background reader"),
    )
    .map(|_| ())
}

/// A background read with SIGTTIN blocked or ignored fails with EIO.
fn bg_read_eio(blocked: bool) -> CaseResult {
    in_background(
        line_mode,
        |slave| {
            if blocked { mask_only(&[SIGTTIN])? } else { ignore(SIGTTIN)?; mask_only(&[])? }
            let mut buf = [0u8; 16];
            let r = read(slave, buf.as_mut_ptr(), buf.len())?;
            want("read in a background process", r, EIO)
        },
        |b| b.finish(2000, "the background reader"),
    )
    .map(|_| ())
}

fn j_bg_read_ignored() -> CaseResult { bg_read_eio(false) }
fn j_bg_read_blocked() -> CaseResult { bg_read_eio(true) }

/// In a new session on a fresh pair whose slave has had `change` made, run `body` in a
/// background process group that is orphaned: its process's parent has exited, so it is
/// the only member and its parent is outside the session. The process reports `body`'s
/// verdict on the case's page.
fn in_orphaned_group(change: impl FnOnce(&mut Termios), body: impl FnOnce(i32) -> CaseResult) -> CaseResult {
    let pty = open_pty()?;
    modes(pty.slave, change)?;
    let slave = pty.slave;
    let pg = page()?;
    leader(&pty, || {
        spawn(|| {
            let parent = pid();
            let orphan = job(|| {
                let result = if until(2000, || ppid() != parent) {
                    body(slave)
                } else {
                    fail("the process was not given a new parent when its parent exited")
                };
                report(pg, 0, result);
                Ok(())
            })?;
            std::mem::forget(orphan);
            Ok(())
        })?
        .finish(2000, "the orphaned group's parent")?;
        verdict(pg, 0, 3000, "the process in the orphaned group")
    })?
    .finish(7000, "the session leader")
}

fn j_bg_read_orphan() -> CaseResult {
    in_orphaned_group(line_mode, |slave| {
        default(SIGTTIN)?;
        mask_only(&[])?;
        let mut buf = [0u8; 16];
        let r = read(slave, buf.as_mut_ptr(), buf.len())?;
        want("read in an orphaned background process group", r, EIO)
    })
}

fn j_fg_write() -> CaseResult {
    let pty = open_pty()?;
    modes(pty.slave, |t| { t.lflag |= TOSTOP; t.oflag &= !OPOST })?;
    let slave = pty.slave;
    leader(&pty, || put(slave, b"fg", "the slave"))?.finish(3000, "the foreground process")?;
    expect_bytes(pty.master, b"fg", 1000, "the master")
}

fn j_bg_write() -> CaseResult {
    let pty = in_background(
        |t| { t.lflag &= !TOSTOP; t.oflag &= !OPOST },
        |slave| put(slave, b"bg", "the slave"),
        |b| b.finish(3000, "the background writer"),
    )?;
    expect_bytes(pty.master, b"bg", 1000, "the master")
}

fn j_bg_write_tostop() -> CaseResult {
    in_background(
        |t| t.lflag |= TOSTOP,
        |slave| {
            default(SIGTTOU)?;
            mask_only(&[])?;
            let r = write(slave, b"bg".as_ptr(), 2)?;
            fail(format!("write returned {}", shown(r)))
        },
        |b| b.stops(SIGTTOU, 2000, "the background writer"),
    )
    .map(|_| ())
}

/// A background write with TOSTOP set and SIGTTOU blocked or ignored goes through.
fn bg_write_allowed(blocked: bool) -> CaseResult {
    let pty = in_background(
        |t| { t.lflag |= TOSTOP; t.oflag &= !OPOST },
        |slave| {
            if blocked { mask_only(&[SIGTTOU])? } else { ignore(SIGTTOU)?; mask_only(&[])? }
            put(slave, b"bg", "the slave")
        },
        |b| b.finish(3000, "the background writer"),
    )?;
    expect_bytes(pty.master, b"bg", 1000, "the master")
}

fn j_bg_write_tostop_ignored() -> CaseResult { bg_write_allowed(false) }
fn j_bg_write_tostop_blocked() -> CaseResult { bg_write_allowed(true) }

fn j_bg_write_orphan() -> CaseResult {
    in_orphaned_group(|t| t.lflag |= TOSTOP, |slave| {
        default(SIGTTOU)?;
        mask_only(&[])?;
        let r = write(slave, b"bg".as_ptr(), 2)?;
        want("write with TOSTOP in an orphaned background process group", r, EIO)
    })
}

fn j_bg_tcsetattr() -> CaseResult {
    in_background(
        |t| t.lflag &= !TOSTOP,
        |slave| {
            default(SIGTTOU)?;
            mask_only(&[])?;
            let t = attrs(slave, "the slave")?;
            let r = tcsetattr(slave, TCSANOW, &t)?;
            fail(format!("tcsetattr returned {}", shown(r)))
        },
        |b| b.stops(SIGTTOU, 2000, "the background process calling tcsetattr"),
    )
    .map(|_| ())
}

/// Typing `ch` on a slave in line mode with ISIG and `change` made sends `sig` to the
/// foreground process.
fn typed_signal(ch: u8, sig: i32, change: impl FnOnce(&mut Termios)) -> CaseResult {
    let pty = open_pty()?;
    signal_modes(pty.slave, change)?;
    let pg = page()?;
    let mut l = leader(&pty, || {
        mask_only(&[sig])?;
        pg.set(0, 1);
        gets(sig, 3000, &format!("the foreground process, once the master wrote {},", show(&[ch])))
    })?;
    flag(pg, 0, 2000, "the session leader")?;
    put(pty.master, &[ch], "the master")?;
    l.finish(4000, "the session leader")
}

fn j_vintr() -> CaseResult { typed_signal(CTRL_C, SIGINT, |_| {}) }
fn j_vquit() -> CaseResult { typed_signal(CTRL_BACKSLASH, SIGQUIT, |_| {}) }
fn j_vsusp() -> CaseResult { typed_signal(CTRL_Z, SIGTSTP, |_| {}) }
fn j_vintr_custom() -> CaseResult { typed_signal(0x01, SIGINT, |t| t.cc[VINTR] = 0x01) }

fn j_vsusp_stops() -> CaseResult {
    let pty = open_pty()?;
    signal_modes(pty.slave, |_| {})?;
    let pg = page()?;
    let slave = pty.slave;
    let mut l = leader(&pty, || {
        let mut foreground = job(|| {
            default(SIGTSTP)?;
            mask_only(&[])?;
            pg.set(0, 1);
            idle(9000);
            fail("it ran on")
        })?;
        flag(pg, 0, 2000, "the job")?;
        zero("tcsetpgrp to the job's group", tcsetpgrp(slave, foreground.pid)?)?;
        pg.set(1, 1);
        foreground.stops(SIGTSTP, 3000, "the foreground job")
    })?;
    flag(pg, 1, 3000, "the session leader")?;
    put(pty.master, &[CTRL_Z], "the master")?;
    l.finish(5000, "the session leader")
}

fn j_vintr_group() -> CaseResult {
    let pty = open_pty()?;
    signal_modes(pty.slave, |_| {})?;
    let pg = page()?;
    let mut l = leader(&pty, || {
        mask_only(&[SIGINT])?;
        let mut member = spawn(|| {
            pg.set(1, 1);
            gets(SIGINT, 3000, "the foreground group's second process, once ^C was typed,")
        })?;
        flag(pg, 1, 2000, "the foreground group's second process")?;
        pg.set(0, 1);
        gets(SIGINT, 3000, "the session leader, once ^C was typed,")?;
        member.finish(3000, "the foreground group's second process")
    })?;
    flag(pg, 0, 3000, "the session leader")?;
    put(pty.master, &[CTRL_C], "the master")?;
    l.finish(5000, "the session leader")
}

fn j_vintr_bg() -> CaseResult {
    let pty = open_pty()?;
    signal_modes(pty.slave, |_| {})?;
    let pg = page()?;
    let mut l = leader(&pty, || {
        mask_only(&[SIGINT])?;
        let mut background = job(|| {
            pg.set(1, 1);
            if !until(4000, || pg.get(2) != 0) { return fail("the foreground process never got its SIGINT"); }
            gets_none(SIGINT, 300, "the background process", "when ^C was typed")
        })?;
        flag(pg, 1, 2000, "the background process")?;
        pg.set(0, 1);
        let foreground = gets(SIGINT, 3000, "the foreground process, once ^C was typed,");
        pg.set(2, 1);
        foreground?;
        background.finish(3000, "the background process")
    })?;
    flag(pg, 0, 3000, "the session leader")?;
    put(pty.master, &[CTRL_C], "the master")?;
    l.finish(5000, "the session leader")
}

fn j_isig_off() -> CaseResult {
    let pty = line_pty(|_| {})?;
    let pg = page()?;
    let slave = pty.slave;
    let mut l = leader(&pty, || {
        mask_only(&[SIGINT])?;
        pg.set(0, 1);
        gets_none(SIGINT, 500, "the foreground process", "from ^C with ISIG clear")?;
        expect_read(slave, 64, b"\x03\n", 2000, "the foreground process")
    })?;
    flag(pg, 0, 2000, "the session leader")?;
    put(pty.master, b"\x03\n", "the master")?;
    l.finish(4000, "the session leader")
}

/// The master writes "abc", ^C and "d\n" in one write to a slave in line mode with ISIG
/// and `change` made: the foreground process gets SIGINT and then reads `want`.
fn interrupted_line(change: impl FnOnce(&mut Termios), want: &'static [u8]) -> CaseResult {
    let pty = open_pty()?;
    signal_modes(pty.slave, change)?;
    let pg = page()?;
    let slave = pty.slave;
    let mut l = leader(&pty, || {
        mask_only(&[SIGINT])?;
        pg.set(0, 1);
        gets(SIGINT, 3000, "the foreground process, once ^C was typed,")?;
        expect_read(slave, 64, want, 2000, "the foreground process")
    })?;
    flag(pg, 0, 2000, "the session leader")?;
    put(pty.master, b"abc\x03d\n", "the master")?;
    l.finish(5000, "the session leader")
}

fn j_intr_flush() -> CaseResult { interrupted_line(|_| {}, b"d\n") }
fn j_noflsh() -> CaseResult { interrupted_line(|t| t.lflag |= NOFLSH, b"abcd\n") }

fn j_orphan_stopped() -> CaseResult {
    let pg = page()?;
    spawn(|| {
        new_session()?;
        spawn(|| {
            let group = spawn(|| {
                let setup = (|| -> CaseResult {
                    zero("setpgid(0, 0)", setpgid(0, 0)?)?;
                    mask_only(&[SIGHUP])?;
                    let mut stopped = spawn(|| {
                        zero("kill(getpid(), SIGSTOP)", kill(pid(), SIGSTOP))?;
                        report(pg, 1, gets(SIGHUP, 3000, "the group's stopped member, once continued,"));
                        Ok(())
                    })?;
                    stopped.stops(SIGSTOP, 2000, "the group's member stopping itself")?;
                    std::mem::forget(stopped);
                    Ok(())
                })();
                if let Err(e) = setup {
                    report(pg, 2, Err(e));
                    pg.set(0, 1);
                    return Ok(());
                }
                pg.set(0, 1);
                report(pg, 2, gets(SIGHUP, 3000, "the group's running member, once the group was orphaned,"));
                Ok(())
            })?;
            let _ = setpgid(group.pid, group.pid);
            flag(pg, 0, 3000, "the group")?;
            std::mem::forget(group);
            Ok(())
        })?
        .finish(4000, "the group's parent")
    })?
    .finish(6000, "the session leader")?;
    verdict(pg, 2, 3500, "the group's running member")?;
    verdict(pg, 1, 3500, "the group's stopped member")
}

fn j_orphan_running() -> CaseResult {
    let pg = page()?;
    spawn(|| {
        new_session()?;
        spawn(|| {
            let parent = pid();
            let group = spawn(|| {
                let result = (|| -> CaseResult {
                    zero("setpgid(0, 0)", setpgid(0, 0)?)?;
                    mask_only(&[SIGHUP])?;
                    pg.set(0, 1);
                    check(until(2000, || ppid() != parent), "the process was not given a new parent when its parent exited")?;
                    gets_none(SIGHUP, 500, "the orphaned group with no stopped member", "when it was orphaned")
                })();
                report(pg, 1, result);
                pg.set(0, 1);
                Ok(())
            })?;
            let _ = setpgid(group.pid, group.pid);
            flag(pg, 0, 2000, "the group")?;
            std::mem::forget(group);
            Ok(())
        })?
        .finish(3000, "the group's parent")
    })?
    .finish(5000, "the session leader")?;
    verdict(pg, 1, 3500, "the group's member")
}

fn j_orphan_sigtstp() -> CaseResult {
    let mut orphan = spawn(|| {
        new_session()?;
        default(SIGTSTP)?;
        mask_only(&[])?;
        zero("kill(getpid(), SIGTSTP)", kill(pid(), SIGTSTP))?;
        sleep_ms(200);
        Ok(())
    })?;
    let status = orphan.wait_change(3000, true, "the process in an orphaned group")?;
    if status & 0xff == 0x7f {
        return fail(format!("the process in an orphaned group {}", status_text(status)));
    }
    orphan.outcome(status, "the process in an orphaned group")
}

// ---------------------------------------------------------------------------

static SUITE: Suite = suite("terminals", "Terminals & job control", &[
    category("termios", "termios modes & attributes", &[
        case("tcgetattr", "tcgetattr on a pseudo-terminal slave returns 0 and fills in its attributes", t_tcgetattr),
        case("tcgetattr-enotty", "tcgetattr on /dev/null fails with ENOTTY", t_tcgetattr_enotty),
        case("tcgetattr-ebadf", "tcgetattr on a closed descriptor fails with EBADF", t_tcgetattr_ebadf),
        case("tcsanow", "tcsetattr(TCSANOW) changes the attributes tcgetattr reports", t_tcsanow),
        case("tcsadrain", "tcsetattr(TCSADRAIN) applies the change and keeps the output written before it", t_tcsadrain),
        case("tcsaflush", "tcsetattr(TCSAFLUSH) applies the change and discards input received but not read", t_tcsaflush),
        case("tcsetattr-einval", "tcsetattr with an unknown optional_actions fails with EINVAL", t_tcsetattr_einval),
        case("tcsetattr-enotty", "tcsetattr on /dev/null fails with ENOTTY", t_tcsetattr_enotty),
        case("canonical-line", "In canonical mode a read returns nothing until the line ends, then the whole line", t_canonical_line),
        case("canonical-one-line", "A canonical read returns at most one line", t_canonical_one_line),
        case("canonical-short-read", "A canonical read asking for less than the line returns that much, and the rest comes next", t_canonical_short_read),
        case("verase", "The ERASE character deletes the character before it", t_verase),
        case("vkill", "The KILL character deletes the line before it", t_vkill),
        case("veof", "The EOF character at the start of a line makes a read return 0", t_veof),
        case("veof-mid-line", "The EOF character after text passes the text on without a newline", t_veof_mid_line),
        case("veol", "The EOL character ends a line, and the read returns it", t_veol),
        case("noncanonical", "With ICANON clear and VMIN 1, a read returns a byte without waiting for a newline", t_noncanonical),
        case("vmin-blocks", "With VMIN 3 and VTIME 0, a read waits until 3 bytes have arrived", t_vmin_blocks),
        case("vmin-zero", "With VMIN 0 and VTIME 0 and nothing written, a read returns 0 at once", t_vmin_zero),
        case("vtime-timeout", "With VMIN 0 and VTIME 2 and nothing written, a read returns 0 after 0.2 seconds", t_vtime_timeout),
        case("vtime-interbyte", "With VMIN 5 and VTIME 2, a read returns the 2 bytes waiting once the inter-byte timer expires", t_vtime_interbyte),
        case("echo", "With ECHO set, input is echoed to the master", t_echo),
        case("echo-off", "With ECHO clear, input is read but not echoed", t_echo_off),
        case("echonl", "With ECHONL set and ECHO clear, only the newline is echoed", t_echonl),
        case("isig-off", "With ISIG clear, the INTR, SUSP and QUIT characters are read as data", t_isig_off),
        case("icrnl", "ICRNL maps a carriage return in input to a newline", t_icrnl),
        case("igncr", "IGNCR discards carriage returns in input", t_igncr),
        case("istrip", "ISTRIP clears the eighth bit of input bytes", t_istrip),
        case("onlcr", "OPOST with ONLCR writes a newline as carriage return and newline", t_onlcr),
        case("opost-off", "With OPOST clear, output is written unchanged", t_opost_off),
        case("cfospeed", "cfgetospeed returns the speed cfsetospeed set", t_cfospeed),
        case("cfispeed", "cfgetispeed returns the speed cfsetispeed set", t_cfispeed),
        case("cfspeed-einval", "cfsetospeed with a speed that is not a B constant fails with EINVAL", t_cfspeed_einval),
        case("speed-round-trip", "An output speed set by tcsetattr is reported by tcgetattr", t_speed_round_trip),
        case("tcflush-input", "tcflush(TCIFLUSH) discards input received but not read", t_tcflush_input),
        case("tcflush-einval", "tcflush with an unknown queue_selector fails with EINVAL", t_tcflush_einval),
        case("tcflush-enotty", "tcflush on /dev/null fails with ENOTTY", t_tcflush_enotty),
        case("tcdrain", "tcdrain returns 0 once the output written has been sent", t_tcdrain),
        case("tcflow", "tcflow(TCOOFF) suspends output and tcflow(TCOON) restarts it", t_tcflow),
        case("tcsendbreak", "tcsendbreak on a pseudo-terminal returns 0", t_tcsendbreak),
    ]),
    category("pty", "pseudo-terminals", &[
        case("openpt", "posix_openpt(O_RDWR | O_NOCTTY) opens a master", p_openpt),
        case("openpt-distinct", "Two masters have different slaves", p_openpt_distinct),
        case("ptmx", "Linux ABI: opening /dev/ptmx gives a master with a /dev/pts slave", p_ptmx),
        case("grantpt", "grantpt on a master returns 0", p_grantpt),
        case("grantpt-ebadf", "grantpt on a closed descriptor fails with EBADF", p_grantpt_ebadf),
        case("grantpt-einval", "grantpt on a descriptor that is not a master fails with EINVAL", p_grantpt_einval),
        case("unlockpt", "After grantpt and unlockpt the slave opens", p_unlockpt),
        case("unlockpt-einval", "unlockpt on a descriptor that is not a master fails with EINVAL", p_unlockpt_einval),
        case("locked", "Linux ABI: opening the slave before unlockpt fails with EIO", p_locked),
        case("ptsname", "ptsname names /dev/pts/<n>, and that path opens the master's slave", p_ptsname),
        case("ptsname-enotty", "ptsname on a descriptor that is not a master fails with ENOTTY", p_ptsname_enotty),
        case("slave-isatty", "isatty is true for the slave", p_slave_isatty),
        case("master-to-slave", "Bytes written to the master are read from the slave", p_master_to_slave),
        case("slave-to-master", "Bytes written to the slave are read from the master", p_slave_to_master),
        case("bulk-down", "16 KiB written to the master by another process are read from the slave in order", p_bulk_down),
        case("bulk-up", "16 KiB written to the slave by another process are read from the master in order", p_bulk_up),
        case("master-poll", "poll on the master reports POLLIN once the slave writes, and not before", p_master_poll),
        case("slave-poll", "poll on the slave reports POLLIN once the master writes, and not before", p_slave_poll),
        case("master-nonblock", "A non-blocking read on the master with nothing written fails with EAGAIN", p_master_nonblock),
        case("slave-close-eio", "Linux ABI: once the slave closes, a read on the master fails with EIO", p_slave_close_eio),
        case("slave-close-data", "Linux ABI: the master reads what the slave wrote before it closed, then EIO", p_slave_close_data),
        case("slave-close-pollhup", "Linux ABI: once the slave closes, poll on the master reports POLLHUP", p_slave_close_pollhup),
        case("master-close-eof", "Once the master closes, a read on the slave returns end of file", p_master_close_eof),
        case("master-close-eio", "Once the master closes, a write on the slave fails with EIO", p_master_close_eio),
        case("master-close-sighup", "Closing the master sends SIGHUP to the controlling process of the slave", p_master_close_sighup),
        case("winsize-get", "TIOCGWINSZ on the slave returns 0", p_winsize_get),
        case("winsize-set", "A size set with TIOCSWINSZ on the master is read back with TIOCGWINSZ on the slave", p_winsize_set),
        case("winsize-slave", "A size set with TIOCSWINSZ on the slave is read back with TIOCGWINSZ on the master", p_winsize_slave),
        case("winsize-enotty", "TIOCGWINSZ on /dev/null fails with ENOTTY", p_winsize_enotty),
        case("sigwinch", "Changing the size sends SIGWINCH to the foreground process group", p_sigwinch),
        case("sigwinch-same", "Setting the size the terminal already has sends no SIGWINCH", p_sigwinch_same),
        case("master-tcsetattr", "Linux ABI: tcsetattr on the master sets the slave's attributes", p_master_tcsetattr),
        case("pts-listed", "Linux ABI: /dev/pts lists the slave's number while the pair is open", p_pts_listed),
        case("pts-removed", "Linux ABI: /dev/pts stops listing the slave's number once both ends close", p_pts_removed),
    ]),
    category("controlling", "controlling terminal & sessions", &[
        case("setsid", "setsid in a process that is not a group leader returns its PID", c_setsid),
        case("setsid-ids", "After setsid the process's group and session IDs are its PID", c_setsid_ids),
        case("setsid-no-ctty", "Linux ABI: after setsid there is no controlling terminal, and opening /dev/tty fails with ENXIO", c_setsid_no_ctty),
        case("setsid-eperm", "setsid in a process group leader fails with EPERM", c_setsid_eperm),
        case("setsid-leaves", "A process that calls setsid no longer has its old controlling terminal", c_setsid_leaves),
        case("open-acquires", "A session leader without a controlling terminal acquires a terminal it opens without O_NOCTTY", c_open_acquires),
        case("open-noctty", "A session leader that opens a terminal with O_NOCTTY does not acquire it", c_open_noctty),
        case("open-not-leader", "A process that is not a session leader does not acquire a terminal it opens", c_open_not_leader),
        case("open-taken", "A session leader does not acquire a terminal that is another session's controlling terminal", c_open_taken),
        case("tiocsctty", "TIOCSCTTY makes a terminal the controlling terminal of a session leader without one", c_tiocsctty),
        case("tiocsctty-not-leader", "TIOCSCTTY in a process that is not a session leader fails with EPERM", c_tiocsctty_not_leader),
        case("tiocsctty-has", "TIOCSCTTY in a session leader that has a controlling terminal fails with EPERM", c_tiocsctty_has),
        case("tiocsctty-taken", "TIOCSCTTY with argument 0 on another session's controlling terminal fails with EPERM", c_tiocsctty_taken),
        case("tiocsctty-steal", "Linux ABI: root's TIOCSCTTY with argument 1 takes a terminal from another session", c_tiocsctty_steal),
        case("tiocnotty", "TIOCNOTTY in a session member gives up its controlling terminal and leaves the leader's", c_tiocnotty),
        case("tiocnotty-enotty", "TIOCNOTTY on a terminal that is not the caller's controlling terminal fails with ENOTTY", c_tiocnotty_enotty),
        case("tiocnotty-leader", "Linux ABI: TIOCNOTTY in the session leader sends SIGHUP to the foreground group and frees the terminal", c_tiocnotty_leader),
        case("tcgetsid", "tcgetsid on the controlling terminal returns the session leader's PID", c_tcgetsid),
        case("tcgetsid-other", "tcgetsid on another session's controlling terminal fails with ENOTTY", c_tcgetsid_other),
        case("tcgetsid-enotty", "tcgetsid on /dev/null fails with ENOTTY", c_tcgetsid_enotty),
        case("getsid", "getsid(0) in a session member returns the session leader's PID", c_getsid),
        case("getsid-other", "getsid of another process returns its session ID", c_getsid_other),
        case("getsid-esrch", "getsid of a process that has ended fails with ESRCH", c_getsid_esrch),
        case("getpgid", "getpgid(0) and getpgid(getpid()) return the group a forked child inherited", c_getpgid),
        case("getpgid-other", "getpgid of another process returns its process group", c_getpgid_other),
        case("getpgid-esrch", "getpgid of a process that has ended fails with ESRCH", c_getpgid_esrch),
        case("setpgid", "setpgid(0, 0) makes the caller the leader of a new process group", c_setpgid),
        case("setpgid-child", "setpgid moves a child into a process group of its own", c_setpgid_child),
        case("setpgid-join", "setpgid moves a process into an existing group in its session", c_setpgid_join),
        case("setpgid-other-session", "setpgid into a group in another session fails with EPERM", c_setpgid_other_session),
        case("setpgid-leader", "setpgid on a session leader fails with EPERM", c_setpgid_leader),
        case("setpgid-child-session", "setpgid on a child in another session fails with EPERM", c_setpgid_child_session),
        case("setpgid-esrch", "setpgid of a process that has ended fails with ESRCH", c_setpgid_esrch),
        case("setpgid-einval", "setpgid with a negative process group fails with EINVAL", c_setpgid_einval),
        case("dev-tty", "/dev/tty opened by a process with a controlling terminal writes to that terminal", c_dev_tty),
        case("ctty-inherited", "A forked child shares its parent's controlling terminal", c_ctty_inherited),
        case("exit-sighup", "When the controlling process exits, its foreground process group gets SIGHUP", c_exit_sighup),
        case("exit-fg-only", "When the controlling process exits, a background process group gets no SIGHUP", c_exit_fg_only),
        case("exit-frees", "Once the controlling process exits, another session leader can acquire the terminal", c_exit_frees),
    ]),
    category("job-control", "foreground, background & SIGTTOU", &[
        case("tcgetpgrp", "tcgetpgrp returns the session leader's group once it acquires the terminal", j_tcgetpgrp),
        case("tcsetpgrp", "tcsetpgrp makes another group in the session the foreground group", j_tcsetpgrp),
        case("tcsetpgrp-eperm", "tcsetpgrp to a group in another session fails with EPERM", j_tcsetpgrp_eperm),
        case("tcsetpgrp-einval", "tcsetpgrp with a negative group fails with EINVAL", j_tcsetpgrp_einval),
        case("tcsetpgrp-enotty", "tcsetpgrp on a terminal that is not the caller's controlling terminal fails with ENOTTY", j_tcsetpgrp_enotty),
        case("tcgetpgrp-enotty", "tcgetpgrp on a terminal that is not the caller's controlling terminal fails with ENOTTY", j_tcgetpgrp_enotty),
        case("bg-tcsetpgrp", "tcsetpgrp in a background process stops it with SIGTTOU", j_bg_tcsetpgrp),
        case("bg-tcsetpgrp-blocked", "tcsetpgrp in a background process with SIGTTOU blocked succeeds", j_bg_tcsetpgrp_blocked),
        case("bg-tcsetpgrp-ignored", "tcsetpgrp in a background process ignoring SIGTTOU succeeds", j_bg_tcsetpgrp_ignored),
        case("fg-read", "The foreground process group reads the terminal", j_fg_read),
        case("bg-read", "A read from the terminal in a background process stops it with SIGTTIN", j_bg_read),
        case("bg-read-ignored", "A background read with SIGTTIN ignored fails with EIO", j_bg_read_ignored),
        case("bg-read-blocked", "A background read with SIGTTIN blocked fails with EIO", j_bg_read_blocked),
        case("bg-read-orphan", "A read in an orphaned background process group fails with EIO", j_bg_read_orphan),
        case("fg-write", "The foreground process group writes the terminal with TOSTOP set", j_fg_write),
        case("bg-write", "A background process writes the terminal when TOSTOP is clear", j_bg_write),
        case("bg-write-tostop", "With TOSTOP set, a write in a background process stops it with SIGTTOU", j_bg_write_tostop),
        case("bg-write-tostop-ignored", "With TOSTOP set, a background process ignoring SIGTTOU writes", j_bg_write_tostop_ignored),
        case("bg-write-tostop-blocked", "With TOSTOP set, a background process blocking SIGTTOU writes", j_bg_write_tostop_blocked),
        case("bg-write-orphan", "With TOSTOP set, a write in an orphaned background process group fails with EIO", j_bg_write_orphan),
        case("bg-tcsetattr", "tcsetattr in a background process stops it with SIGTTOU", j_bg_tcsetattr),
        case("vintr", "Typing ^C sends SIGINT to the foreground process group", j_vintr),
        case("vquit", "Typing ^\\ sends SIGQUIT to the foreground process group", j_vquit),
        case("vsusp", "Typing ^Z sends SIGTSTP to the foreground process group", j_vsusp),
        case("vsusp-stops", "Typing ^Z stops a foreground job, as its parent sees with WUNTRACED", j_vsusp_stops),
        case("vintr-group", "^C reaches every process in the foreground group", j_vintr_group),
        case("vintr-bg", "^C does not reach a background process group", j_vintr_bg),
        case("vintr-custom", "A changed VINTR character sends SIGINT", j_vintr_custom),
        case("isig-off", "With ISIG clear, ^C sends no SIGINT and is read as data", j_isig_off),
        case("intr-flush", "^C discards the unread input before it", j_intr_flush),
        case("noflsh", "With NOFLSH set, ^C keeps the unread input", j_noflsh),
        case("orphan-stopped", "A group orphaned with a stopped member gets SIGHUP and SIGCONT", j_orphan_stopped),
        case("orphan-running", "A group orphaned with no stopped member gets no SIGHUP", j_orphan_running),
        case("orphan-sigtstp", "SIGTSTP does not stop a process in an orphaned process group", j_orphan_sigtstp),
    ]),
]);

fn main() { SUITE.run() }
