//! Signals: dispositions and default actions, masks and pending signals, handlers and
//! their flags, sigsuspend/pause/sigtimedwait and interval timers, alternate signal
//! stacks, realtime signals and sigqueue, and job control, as POSIX specifies them.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! Every wait on another process or on a signal inside a case is bounded and stops early
//! enough to leave the case time to clean up and report. The processes a case starts are
//! killed when it ends, and the runner, which a suite runs as PID 1, kills and reaps any
//! that remain before the next case, so no case depends on another.
//!
//! Cases call the kernel by its Linux numbers and assert on the raw return, so an
//! unimplemented call fails with ENOSYS. On x86-64 they use the SYSCALL instruction, as a
//! C library does, and so does the handlers' restorer. Library-level interfaces are made as a C library
//! makes them: sigqueue through rt_sigqueueinfo, sigwaitinfo and sigtimedwait through
//! rt_sigtimedwait, pthread_kill through tgkill, raise as kill(getpid()), and, where the
//! architecture has no such call, pause through ppoll and alarm through setitimer.
//! Handlers are installed with the `struct sigaction` libbreenix passes to rt_sigaction;
//! `dispositions/sigaction-layout` checks that structure against the Linux ABI on its own.
use libbreenix::io;
use libbreenix::memory::{self, MAP_ANONYMOUS, MAP_PRIVATE, MAP_SHARED, PROT_READ, PROT_WRITE};
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{Sigaction, StackT};
use libbreenix::suite::{case, case_ms_left, category, check, fail, skip, suite, CaseError, CaseResult, Suite};
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use libbreenix::time;
use libbreenix::types::Fd;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const READ: u64 = 0;
    pub const RT_SIGACTION: u64 = 13;
    pub const RT_SIGPROCMASK: u64 = 14;
    pub const PAUSE: u64 = 34;
    pub const NANOSLEEP: u64 = 35;
    pub const GETITIMER: u64 = 36;
    pub const ALARM: u64 = 37;
    pub const SETITIMER: u64 = 38;
    pub const GETPID: u64 = 39;
    pub const EXECVE: u64 = 59;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const GETUID: u64 = 102;
    pub const SETUID: u64 = 105;
    pub const SETPGID: u64 = 109;
    pub const SETSID: u64 = 112;
    pub const RT_SIGPENDING: u64 = 127;
    pub const RT_SIGTIMEDWAIT: u64 = 128;
    pub const RT_SIGQUEUEINFO: u64 = 129;
    pub const RT_SIGSUSPEND: u64 = 130;
    pub const SIGALTSTACK: u64 = 131;
    pub const GETTID: u64 = 186;
    pub const TGKILL: u64 = 234;
    pub const WAITID: u64 = 247;
    pub const PRLIMIT64: u64 = 302;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: u64 = 63;
    pub const RT_SIGACTION: u64 = 134;
    pub const RT_SIGPROCMASK: u64 = 135;
    /// There is no pause here; a C library's pause calls ppoll with no descriptors.
    pub const PPOLL: u64 = 73;
    pub const NANOSLEEP: u64 = 101;
    pub const GETITIMER: u64 = 102;
    pub const SETITIMER: u64 = 103;
    pub const GETPID: u64 = 172;
    pub const EXECVE: u64 = 221;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const GETUID: u64 = 174;
    pub const SETUID: u64 = 146;
    pub const SETPGID: u64 = 154;
    pub const SETSID: u64 = 157;
    pub const RT_SIGPENDING: u64 = 136;
    pub const RT_SIGTIMEDWAIT: u64 = 137;
    pub const RT_SIGQUEUEINFO: u64 = 138;
    pub const RT_SIGSUSPEND: u64 = 133;
    pub const SIGALTSTACK: u64 = 132;
    pub const GETTID: u64 = 178;
    pub const TGKILL: u64 = 131;
    pub const WAITID: u64 = 95;
    pub const PRLIMIT64: u64 = 261;
}

const ESRCH: i64 = 3;
const EINTR: i64 = 4;
const ECHILD: i64 = 10;
const EAGAIN: i64 = 11;
const ENOMEM: i64 = 12;
const EPERM: i64 = 1;
const EINVAL: i64 = 22;

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGQUIT: i32 = 3;
const SIGILL: i32 = 4;
const SIGTRAP: i32 = 5;
const SIGABRT: i32 = 6;
const SIGBUS: i32 = 7;
const SIGFPE: i32 = 8;
const SIGKILL: i32 = 9;
const SIGUSR1: i32 = 10;
const SIGSEGV: i32 = 11;
const SIGUSR2: i32 = 12;
const SIGPIPE: i32 = 13;
const SIGALRM: i32 = 14;
const SIGTERM: i32 = 15;
const SIGCHLD: i32 = 17;
const SIGCONT: i32 = 18;
const SIGSTOP: i32 = 19;
const SIGTSTP: i32 = 20;
const SIGTTIN: i32 = 21;
const SIGTTOU: i32 = 22;
const SIGURG: i32 = 23;
const SIGXCPU: i32 = 24;
const SIGXFSZ: i32 = 25;
const SIGVTALRM: i32 = 26;
const SIGPROF: i32 = 27;
const SIGWINCH: i32 = 28;
const SIGPOLL: i32 = 29;
const SIGSYS: i32 = 31;
/// The kernel's realtime range (Linux's, before a C library reserves any for itself).
const SIGRTMIN: i32 = 32;
const SIGRTMAX: i32 = 64;

const SIG_DFL: u64 = 0;
const SIG_IGN: u64 = 1;
const SIG_BLOCK: i32 = 0;
const SIG_UNBLOCK: i32 = 1;
const SIG_SETMASK: i32 = 2;
const SA_NOCLDSTOP: u64 = 0x1;
const SA_NOCLDWAIT: u64 = 0x2;
const SA_SIGINFO: u64 = 0x4;
const SA_RESTORER: u64 = 0x0400_0000;
const SA_ONSTACK: u64 = 0x0800_0000;
const SA_RESTART: u64 = 0x1000_0000;
const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESETHAND: u64 = 0x8000_0000;
const SS_ONSTACK: i32 = 1;
const SS_DISABLE: i32 = 2;

const SI_USER: i32 = 0;
const SI_QUEUE: i32 = -1;
const SEGV_MAPERR: i32 = 1;
const CLD_EXITED: i32 = 1;
const CLD_KILLED: i32 = 2;
const CLD_STOPPED: i32 = 5;
const CLD_CONTINUED: i32 = 6;

const WNOHANG: i32 = 1;
const WUNTRACED: i32 = 2;
const WCONTINUED: i32 = 8;
const WEXITED: i32 = 4;
const P_PID: u64 = 1;

const ITIMER_REAL: i32 = 0;
const ITIMER_VIRTUAL: i32 = 1;
const ITIMER_PROF: i32 = 2;
const RLIMIT_STACK: u32 = 3;
const RLIMIT_SIGPENDING: u32 = 11;
const O_CLOEXEC: i32 = 0x80000;

/// Two users of the suite's own, for the permission cases.
const USER_A: u32 = 4242;
const USER_B: u32 = 4343;

/// How long a case waits for another process to change state. A working kernel
/// takes milliseconds; the bound only keeps a failing case inside the runner's limit.
const WAIT_MS: u64 = 3000;
/// The same for an exec, which loads a program from disk.
const EXEC_MS: u64 = 6000;
/// What a case's waits leave of the runner's limit, for the case to clean up and report.
const CLEANUP_MS: u64 = 1500;
/// What cleanup's own waits leave, for the case to report.
const REPORT_MS: u64 = 300;
/// How long a case watches for something that must not happen.
const QUIET_MS: u64 = 300;
/// The helper the exec cases run (`signals_exec.rs`).
const HELPER: &str = "/usr/local/test/bin/signals-exec_test";
/// In an exec case's argument list, replaced by the descriptor the program reports on.
const FD_ARG: &str = "{fd}";

type Checked = Result<(), String>;

/// A system call made as a C library makes it: on x86-64 the SYSCALL instruction, so
/// the cases run the kernel's SYSCALL entry and return path, and on ARM64 svc.
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

/// The signal restorer the suite's handlers return through: rt_sigreturn made the way
/// `sc` makes calls.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn restore_rt() -> ! {
    core::arch::naked_asm!("mov rax, 15", "syscall", "ud2")
}
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn restore_rt() -> ! {
    core::arch::naked_asm!("mov x8, 139", "svc #0", "brk #1")
}

fn errname(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 7 => "E2BIG", 8 => "ENOEXEC",
        9 => "EBADF", 10 => "ECHILD", 11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES",
        14 => "EFAULT", 22 => "EINVAL", 38 => "ENOSYS",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A raw return as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if ret < 0 { errname(-ret) } else { ret.to_string() } }

fn err<T>(msg: impl Into<String>) -> Result<T, CaseError> { Err(CaseError::Fail(msg.into())) }

/// The raw return, or a failure naming the call.
fn want(what: &str, ret: i64) -> Result<i64, CaseError> {
    if ret < 0 { err(format!("{what} failed with {}", errname(-ret))) } else { Ok(ret) }
}

fn want_eq(what: &str, got: i64, expected: i64) -> CaseResult {
    check(got == expected, &format!("{what}: expected {expected}, got {}", shown(got)))
}

fn want_err(what: &str, got: i64, errno: i64) -> CaseResult {
    check(got == -errno, &format!("{what}: expected {}, got {}", errname(errno), shown(got)))
}

/// `want` for code running in a forked child, which reports a message rather than a CaseError.
fn ok(what: &str, ret: i64) -> Result<i64, String> {
    if ret < 0 { Err(format!("{what} failed with {}", errname(-ret))) } else { Ok(ret) }
}

fn name(sig: i32) -> String {
    let known = match sig {
        1 => "SIGHUP", 2 => "SIGINT", 3 => "SIGQUIT", 4 => "SIGILL", 5 => "SIGTRAP", 6 => "SIGABRT",
        7 => "SIGBUS", 8 => "SIGFPE", 9 => "SIGKILL", 10 => "SIGUSR1", 11 => "SIGSEGV", 12 => "SIGUSR2",
        13 => "SIGPIPE", 14 => "SIGALRM", 15 => "SIGTERM", 17 => "SIGCHLD", 18 => "SIGCONT",
        19 => "SIGSTOP", 20 => "SIGTSTP", 21 => "SIGTTIN", 22 => "SIGTTOU", 23 => "SIGURG",
        24 => "SIGXCPU", 25 => "SIGXFSZ", 26 => "SIGVTALRM", 27 => "SIGPROF", 28 => "SIGWINCH",
        29 => "SIGPOLL", 31 => "SIGSYS",
        _ => return format!("signal {sig}"),
    };
    known.to_string()
}

fn bit(sig: i32) -> u64 { 1u64 << (sig - 1) }

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }
fn gettid() -> i32 { sc(nr::GETTID, &[]) as i32 }
fn getuid() -> u32 { sc(nr::GETUID, &[]) as u32 }
fn kill(pid: i32, sig: i32) -> i64 { sc(nr::KILL, &[pid as i64 as u64, sig as i64 as u64]) }
fn raise(sig: i32) -> i64 { kill(pid(), sig) }
fn setuid(uid: u32) -> i64 { sc(nr::SETUID, &[uid as u64]) }
fn setpgid(pid: i32, pgid: i32) -> i64 { sc(nr::SETPGID, &[pid as i64 as u64, pgid as i64 as u64]) }
fn setsid() -> i64 { sc(nr::SETSID, &[]) }
fn wait4(pid: i32, status: *mut i32, options: i32) -> i64 {
    sc(nr::WAIT4, &[pid as i64 as u64, status as u64, options as u64, 0])
}
fn tgkill(tgid: i32, tid: i32, sig: i32) -> i64 { sc(nr::TGKILL, &[tgid as u64, tid as u64, sig as u64]) }

/// The `struct sigaction` libbreenix passes to rt_sigaction, with the suite's restorer.
fn action(handler: u64, flags: u64, mask: u64) -> Sigaction {
    Sigaction { handler, mask, flags: flags | SA_RESTORER, restorer: restore_rt as usize as u64 }
}

fn sigaction(sig: i32, act: Option<&Sigaction>, old: Option<&mut Sigaction>) -> i64 {
    sc(nr::RT_SIGACTION, &[
        sig as i64 as u64,
        act.map_or(0, |a| a as *const Sigaction as u64),
        old.map_or(0, |o| o as *mut Sigaction as u64),
        8,
    ])
}

/// Install `act` for `sig`, as a C library's sigaction would.
fn set_action(sig: i32, act: &Sigaction) -> Checked {
    ok(&format!("sigaction({})", name(sig)), sigaction(sig, Some(act), None)).map(|_| ())
}

fn catch_with(sig: i32, handler: u64, flags: u64, mask: u64) -> Checked { set_action(sig, &action(handler, flags, mask)) }
/// Catch `sig` with `on_sig`, without SA_RESTART.
fn catch(sig: i32) -> Checked { catch_with(sig, on_sig as usize as u64, 0, 0) }
/// Catch `sig` with `on_info`, with SA_SIGINFO.
fn catch_info(sig: i32) -> Checked { catch_with(sig, on_info as usize as u64, SA_SIGINFO, 0) }
fn ignore(sig: i32) -> Checked { set_action(sig, &action(SIG_IGN, 0, 0)) }
fn default(sig: i32) -> Checked { set_action(sig, &action(SIG_DFL, 0, 0)) }

/// The disposition the kernel reports for `sig`.
fn disposition(sig: i32) -> Result<Sigaction, String> {
    let mut old = Sigaction::default();
    ok(&format!("querying sigaction({})", name(sig)), sigaction(sig, None, Some(&mut old)))?;
    Ok(old)
}

fn procmask(how: i32, set: Option<u64>) -> Result<u64, String> {
    let set = set.map(|s| [s]);
    let mut old = [0u64];
    ok("sigprocmask", sc(nr::RT_SIGPROCMASK, &[
        how as u64, set.as_ref().map_or(0, |s| s.as_ptr() as u64), old.as_mut_ptr() as u64, 8,
    ]))?;
    Ok(old[0])
}
fn mask_now() -> Result<u64, String> { procmask(SIG_BLOCK, None) }
fn block(set: u64) -> Result<u64, String> { procmask(SIG_BLOCK, Some(set)) }
fn unblock(set: u64) -> Result<u64, String> { procmask(SIG_UNBLOCK, Some(set)) }
fn setmask(set: u64) -> Result<u64, String> { procmask(SIG_SETMASK, Some(set)) }

/// The mask as the kernel holds it, for use inside a handler: no allocation.
fn mask_raw() -> u64 {
    let mut old = [0u64];
    sc(nr::RT_SIGPROCMASK, &[SIG_BLOCK as u64, 0, old.as_mut_ptr() as u64, 8]);
    old[0]
}

fn pending() -> Result<u64, String> {
    let mut set = [0u64];
    ok("sigpending", sc(nr::RT_SIGPENDING, &[set.as_mut_ptr() as u64, 8]))?;
    Ok(set[0])
}

fn sigsuspend(mask: u64) -> i64 {
    let set = [mask];
    sc(nr::RT_SIGSUSPEND, &[set.as_ptr() as u64, 8])
}

/// pause as a C library makes it: the pause call, or ppoll with no descriptors.
fn pause() -> i64 {
    #[cfg(target_arch = "x86_64")]
    { sc(nr::PAUSE, &[]) }
    #[cfg(target_arch = "aarch64")]
    { sc(nr::PPOLL, &[0, 0, 0, 0, 8]) }
}

/// alarm as a C library makes it: the alarm call, or setitimer(ITIMER_REAL) with the
/// seconds left of the old value rounded up.
fn alarm(seconds: u32) -> i64 {
    #[cfg(target_arch = "x86_64")]
    { sc(nr::ALARM, &[seconds as u64]) }
    #[cfg(target_arch = "aarch64")]
    {
        let new = [0i64, 0, seconds as i64, 0];
        let mut old = [0i64; 4];
        let ret = sc(nr::SETITIMER, &[ITIMER_REAL as u64, new.as_ptr() as u64, old.as_mut_ptr() as u64]);
        if ret < 0 { return ret; }
        old[2] + (old[3] != 0) as i64
    }
}

/// struct itimerval as four longs: interval seconds and microseconds, value seconds and microseconds.
type Itimer = [i64; 4];

fn us(t: (i64, i64)) -> i64 { t.0 * 1_000_000 + t.1 }
fn itimer(interval_us: i64, value_us: i64) -> Itimer {
    [interval_us / 1_000_000, interval_us % 1_000_000, value_us / 1_000_000, value_us % 1_000_000]
}

fn setitimer(which: i32, new: &Itimer, old: Option<&mut Itimer>) -> i64 {
    sc(nr::SETITIMER, &[which as u64, new.as_ptr() as u64, old.map_or(0, |o| o.as_mut_ptr() as u64)])
}

fn getitimer(which: i32) -> Result<Itimer, String> {
    let mut cur = [0i64; 4];
    ok("getitimer", sc(nr::GETITIMER, &[which as u64, cur.as_mut_ptr() as u64]))?;
    Ok(cur)
}

fn sigaltstack(new: Option<&StackT>, old: Option<&mut StackT>) -> i64 {
    sc(nr::SIGALTSTACK, &[new.map_or(0, |s| s as *const StackT as u64), old.map_or(0, |s| s as *mut StackT as u64)])
}

fn nanosleep(ms: u64, rem: &mut [i64; 2]) -> i64 {
    let req = [(ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64];
    sc(nr::NANOSLEEP, &[req.as_ptr() as u64, rem.as_mut_ptr() as u64])
}

fn read_raw(fd: Fd, buf: &mut [u8]) -> i64 {
    sc(nr::READ, &[fd.raw() as u64, buf.as_mut_ptr() as u64, buf.len() as u64])
}

/// siginfo_t as the Linux ABI lays it out on both architectures: signo, errno and code,
/// then the union from byte 16 (si_pid, si_uid, then si_value or si_status; or si_addr).
#[repr(C)]
#[derive(Clone, Copy)]
struct SigInfo { signo: i32, errno: i32, code: i32, pad: i32, fields: [u64; 14] }

impl SigInfo {
    const fn zero() -> SigInfo { SigInfo { signo: 0, errno: 0, code: 0, pad: 0, fields: [0; 14] } }
    fn pid(&self) -> i32 { self.fields[0] as u32 as i32 }
    fn uid(&self) -> u32 { (self.fields[0] >> 32) as u32 }
    fn value(&self) -> u64 { self.fields[1] }
    fn status(&self) -> i32 { self.fields[1] as u32 as i32 }
    fn addr(&self) -> u64 { self.fields[0] }
}

/// sigqueue as a C library makes it: rt_sigqueueinfo with SI_QUEUE, the caller's PID and
/// user ID, and the value.
fn sigqueue(pid: i32, sig: i32, value: u64) -> i64 {
    let mut info = SigInfo::zero();
    info.signo = sig;
    info.code = SI_QUEUE;
    info.fields[0] = (self::pid() as u32 as u64) | ((getuid() as u64) << 32);
    info.fields[1] = value;
    sc(nr::RT_SIGQUEUEINFO, &[pid as i64 as u64, sig as i64 as u64, &info as *const SigInfo as u64])
}

/// sigtimedwait as a C library makes it; `timeout` None waits without limit (sigwaitinfo).
fn sigtimedwait(set: u64, info: &mut SigInfo, timeout_ms: Option<u64>) -> i64 {
    let set = [set];
    let ts = timeout_ms.map(|ms| [(ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64]);
    sc(nr::RT_SIGTIMEDWAIT, &[
        set.as_ptr() as u64, info as *mut SigInfo as u64, ts.as_ref().map_or(0, |t| t.as_ptr() as u64), 8,
    ])
}

fn prlimit(resource: u32, soft: u64, hard: u64) -> i64 {
    let new = [soft, hard];
    sc(nr::PRLIMIT64, &[0, resource as u64, new.as_ptr() as u64, 0])
}

fn getrlimit(resource: u32) -> Result<(u64, u64), String> {
    let mut old = [0u64; 2];
    ok("getrlimit", sc(nr::PRLIMIT64, &[0, resource as u64, 0, old.as_mut_ptr() as u64]))?;
    Ok((old[0], old[1]))
}

// Wait statuses, decoded as a C library's macros decode them.
fn exited(s: i32) -> bool { s & 0x7f == 0 }
fn exit_code(s: i32) -> i32 { (s >> 8) & 0xff }
fn signaled(s: i32) -> bool { (((s & 0x7f) + 1) as i8 >> 1) > 0 }
fn term_sig(s: i32) -> i32 { s & 0x7f }
fn stopped(s: i32) -> bool { s & 0xff == 0x7f }
fn stop_sig(s: i32) -> i32 { (s >> 8) & 0xff }
fn continued(s: i32) -> bool { s == 0xffff }
fn status_text(s: i32) -> String {
    if continued(s) { "continued".into() }
    else if stopped(s) { format!("stopped by {}", name(stop_sig(s))) }
    else if exited(s) { format!("exit {}", exit_code(s)) }
    else if signaled(s) { format!("death by {}", name(term_sig(s))) }
    else { format!("status {s:#x}") }
}

/// `ms`, cut short so that `reserve` ms of the case's limit remain afterwards.
fn bounded(ms: u64, reserve: u64) -> u64 { ms.min(case_ms_left().saturating_sub(reserve)) }

fn now_ms() -> u64 { time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0) }
fn nap() { let _ = time::sleep_ms(2); }

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

/// Use the CPU for `ms` milliseconds of wall-clock time.
fn burn(ms: u64) {
    let start = now_ms();
    let mut x = 1u64;
    while now_ms().saturating_sub(start) < ms {
        for i in 0..20_000u64 { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); }
        core::hint::black_box(x);
    }
}

/// Processors online, from the `processor` lines of /proc/cpuinfo. A census that cannot
/// be read or counts none fails the case rather than passing for one processor.
fn processors() -> Result<usize, CaseError> {
    let info = std::fs::read_to_string("/proc/cpuinfo").map_err(|e| format!("reading /proc/cpuinfo failed: {e}"))?;
    let n = info.lines().filter(|line| line.starts_with("processor")).count();
    if n == 0 { return err("/proc/cpuinfo lists no processor"); }
    Ok(n)
}

/// Handoffs a pair must make within HANDOFF_MS to show they run at the same time.
const HANDOFFS: u64 = 1000;
const HANDOFF_MS: u64 = 1000;

/// Answer handoffs on `slot` for as long as the caller spins: an odd value there becomes
/// the next even one. No system call.
fn answer_handoff(slot: &AtomicU64) {
    let v = slot.load(Ordering::SeqCst);
    if v & 1 == 1 { slot.store(v + 1, Ordering::SeqCst); }
}

/// Make HANDOFFS round trips on `slot` with a process or thread spinning in
/// `answer_handoff`. Every round trip needs the other side to run, and two that share a
/// processor take turns no faster than the timer tick, a millisecond or more apiece, so
/// HANDOFFS of them within HANDOFF_MS show the two running on different processors.
fn handoffs_show_parallel(slot: &AtomicU64) -> Checked {
    let start = now_ms();
    let base = slot.load(Ordering::SeqCst) & !1;
    for i in 0..HANDOFFS {
        let odd = base + 2 * i + 1;
        slot.store(odd, Ordering::SeqCst);
        let mut spins = 0u64;
        while slot.load(Ordering::SeqCst) != odd + 1 {
            core::hint::spin_loop();
            spins += 1;
            if spins % 4096 == 0 && now_ms().saturating_sub(start) >= HANDOFF_MS {
                return Err(format!("only {i} of {HANDOFFS} handoffs finished in {HANDOFF_MS} ms: the handoff threshold was not reached"));
            }
        }
    }
    Ok(())
}

/// Whether every thread of `pid` is blocked, as /proc reports it.
fn is_parked(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .is_ok_and(|status| status.lines().any(|line| line == "State:\tBlocked"))
}

/// Wait up to `ms` for every thread of `pid` to block.
fn parked(pid: i32, ms: u64) -> bool { until(ms, || is_parked(pid)) }

/// Poll waitpid(pid, options | WNOHANG) until it reports a child, for at most `ms`.
fn wait_within(pid: i32, options: i32, ms: u64) -> Result<(i32, i32), String> {
    poll_wait(pid, options, bounded(ms, CLEANUP_MS))
}

fn poll_wait(pid: i32, options: i32, ms: u64) -> Result<(i32, i32), String> {
    let start = now_ms();
    loop {
        let mut status = 0;
        let r = wait4(pid, &mut status, options | WNOHANG);
        if r > 0 { return Ok((r as i32, status)); }
        if r < 0 { return Err(format!("waitpid({pid}) failed with {}", errname(-r))); }
        if now_ms().saturating_sub(start) >= ms {
            return Err(format!("waitpid({pid}) reported nothing within {ms} ms"));
        }
        nap();
    }
}

/// Whether waitpid(pid, options) reports anything within `ms`; for things that must not happen.
fn reports_within(pid: i32, options: i32, ms: u64) -> Result<Option<i32>, String> {
    let start = now_ms();
    loop {
        let mut status = 0;
        let r = wait4(pid, &mut status, options | WNOHANG);
        if r > 0 { return Ok(Some(status)); }
        if r < 0 { return Err(format!("waitpid({pid}) failed with {}", errname(-r))); }
        if now_ms().saturating_sub(start) >= ms { return Ok(None); }
        nap();
    }
}

/// Read until `want` bytes have arrived or end of file, for at most `ms`.
fn read_up_to(fd: Fd, want: usize, ms: u64) -> Result<Vec<u8>, String> {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 4096];
    while out.len() < want {
        let left = ms.saturating_sub(now_ms().saturating_sub(start));
        if left == 0 { return Err(format!("no end of file within {ms} ms")); }
        let mut fds = [io::PollFd::new(fd, io::poll_events::POLLIN)];
        match io::poll(&mut fds, left.min(1000) as i32) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(e) => return Err(format!("poll failed: {e}")),
        }
        let room = (want - out.len()).min(buf.len());
        match io::read(fd, &mut buf[..room]) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) => return Err(format!("read failed: {e}")),
        }
    }
    Ok(out)
}

fn read_to_eof(fd: Fd, ms: u64) -> Result<Vec<u8>, String> { read_up_to(fd, usize::MAX, ms) }

/// Block on `fd` until end of file; false if a read failed first.
fn drain(fd: Fd) -> bool {
    let mut b = [0u8; 16];
    loop {
        match io::read(fd, &mut b) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

/// A child process. Dropping it while it may still run kills and reaps it.
struct Child { pid: i32, live: bool }

impl Child {
    /// Fork; the child runs `f` and exits with what it returns.
    fn start(f: impl FnOnce() -> i32) -> Result<Child, CaseError> {
        match process::fork()? {
            ForkResult::Child => process::exit(f()),
            ForkResult::Parent(pid) => Ok(Child { pid: pid.raw() as i32, live: true }),
        }
    }

    /// Wait for the child to end; its wait status.
    fn wait(&mut self) -> Result<i32, CaseError> {
        let (_, status) = wait_within(self.pid, 0, WAIT_MS)?;
        self.live = false;
        Ok(status)
    }

    /// `wait`, with a failure to end named after `what`.
    fn wait_for(&mut self, what: &str) -> Result<i32, CaseError> {
        self.wait().map_err(|e| CaseError::Fail(format!("{what} did not end: {}", msg(e))))
    }

    fn expect_exit(&mut self, code: i32, what: &str) -> CaseResult {
        let status = self.wait_for(what)?;
        check(exited(status) && exit_code(status) == code,
            &format!("{what} ended with {}, expected exit {code}", status_text(status)))
    }

    fn expect_death(&mut self, sig: i32, what: &str) -> CaseResult {
        let status = self.wait_for(what)?;
        check(signaled(status) && term_sig(status) == sig,
            &format!("{what} ended with {}, expected death by {}", status_text(status), name(sig)))
    }

    /// Wait for the child to report a stop; its stop signal.
    fn expect_stop(&mut self, sig: i32, what: &str) -> CaseResult {
        let (_, status) = wait_within(self.pid, WUNTRACED, WAIT_MS).map_err(|e| format!("{what}: {e}"))?;
        if !stopped(status) { self.live = false; }
        check(stopped(status) && stop_sig(status) == sig,
            &format!("{what}: waitpid(WUNTRACED) reported {}, expected stopped by {}", status_text(status), name(sig)))
    }

    fn expect_continued(&mut self, what: &str) -> CaseResult {
        let (_, status) = wait_within(self.pid, WCONTINUED, WAIT_MS).map_err(|e| format!("{what}: {e}"))?;
        if !continued(status) && !stopped(status) { self.live = false; }
        check(continued(status), &format!("{what}: waitpid(WCONTINUED) reported {}, expected continued", status_text(status)))
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.live {
            kill(self.pid, SIGKILL);
            let _ = poll_wait(self.pid, 0, bounded(1000, REPORT_MS));
        }
    }
}

/// A child running a check; `finish` fails with the message the child reported.
struct Task { child: Child, msg: Fd }

fn task(f: impl FnOnce() -> Checked) -> Result<Task, CaseError> {
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(r);
        match f() {
            Ok(()) => 0,
            Err(msg) => { let _ = io::write(w, msg.as_bytes()); 1 }
        }
    });
    let _ = io::close(w);
    match child {
        Ok(child) => Ok(Task { child, msg: r }),
        Err(e) => { let _ = io::close(r); Err(e) }
    }
}

impl Task {
    fn finish(mut self) -> CaseResult {
        let status = self.child.wait_for("the child")?;
        if exited(status) && exit_code(status) == 0 { return Ok(()); }
        let said = read_up_to(self.msg, 512, 100).unwrap_or_default();
        if exited(status) && exit_code(status) == 1 && !said.is_empty() {
            return fail(format!("in the child: {}", String::from_utf8_lossy(&said)));
        }
        fail(format!("the child ended with {}", status_text(status)))
    }
}

impl Drop for Task {
    fn drop(&mut self) { let _ = io::close(self.msg); }
}

/// Run `f` in a child and report its result: for checks that change the process's
/// identity or may kill it.
fn in_child(f: impl FnOnce() -> Checked) -> CaseResult { task(f)?.finish() }

/// A child that ran `setup`, said so, and now waits on a pipe to be released.
struct Held { child: Child, release: Option<Fd> }

fn held(setup: impl FnOnce() -> Checked) -> Result<Held, CaseError> {
    let (ready_r, ready_w) = io::pipe2(O_CLOEXEC)?;
    let (rel_r, rel_w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(ready_r);
        let _ = io::close(rel_w);
        let said = match setup() { Ok(()) => "r".to_string(), Err(msg) => format!("!{msg}") };
        let _ = io::write(ready_w, said.as_bytes());
        let _ = io::close(ready_w);
        if !drain(rel_r) { 2 } else if said == "r" { 0 } else { 1 }
    });
    let _ = io::close(ready_w);
    let _ = io::close(rel_r);
    let child = match child {
        Ok(child) => child,
        Err(e) => { let _ = io::close(ready_r); let _ = io::close(rel_w); return Err(e); }
    };
    let held = Held { child, release: Some(rel_w) };
    let said = read_to_eof(ready_r, WAIT_MS);
    let _ = io::close(ready_r);
    let said = said?;
    match said.first() {
        Some(b'r') => Ok(held),
        Some(b'!') => err(format!("in the child: {}", String::from_utf8_lossy(&said[1..]))),
        _ => err("the child never reported that it was ready"),
    }
}

impl Held {
    fn pid(&self) -> i32 { self.child.pid }

    /// Let the child go on; it exits 0 unless something ended it first.
    fn let_go(&mut self) {
        if let Some(fd) = self.release.take() { let _ = io::close(fd); }
    }

    fn release(mut self) -> CaseResult {
        self.let_go();
        self.child.expect_exit(0, "the held child")
    }

    /// Release the child and expect it to have died of `sig` instead of exiting.
    fn expect_death(mut self, sig: i32, what: &str) -> CaseResult {
        self.let_go();
        self.child.expect_death(sig, what)
    }
}

impl Drop for Held {
    fn drop(&mut self) { self.let_go(); }
}

/// A page shared with the case's children: slots that any of them can read and write.
struct Shared { page: *mut u8 }

// Keep independently written words on separate 64-byte cache lines.
const SHARED_SLOT_BYTES: usize = 64;

impl Shared {
    fn new() -> Result<Shared, CaseError> {
        let page = memory::mmap(core::ptr::null_mut(), 4096, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0)?;
        Ok(Shared { page })
    }

    fn slot(&self, i: usize) -> &AtomicU64 {
        assert!(i < 4096 / SHARED_SLOT_BYTES);
        // SAFETY: the page is mapped, aligned and zero-filled for as long as `self` lives.
        unsafe { &*(self.page.add(i * SHARED_SLOT_BYTES) as *const AtomicU64) }
    }

    fn get(&self, i: usize) -> u64 { self.slot(i).load(Ordering::SeqCst) }
    fn set(&self, i: usize, v: u64) { self.slot(i).store(v, Ordering::SeqCst) }
}

impl Drop for Shared {
    fn drop(&mut self) { let _ = memory::munmap(self.page, 4096); }
}

/// A child that sends `sig` to this process after `ms`, so that a wait that never ends
/// on its own still ends; dropping it stops it.
fn watchdog(ms: u64, sig: i32) -> Result<Child, CaseError> {
    let target = pid();
    Child::start(move || {
        let _ = time::sleep_ms(ms);
        kill(target, sig);
        0
    })
}

/// The PID of a child that has exited and been reaped, so names no process.
fn gone_pid() -> Result<i32, CaseError> {
    let mut child = Child::start(|| 0)?;
    child.expect_exit(0, "a short-lived child")?;
    Ok(child.pid)
}

// ---------------------------------------------------------------------------
// What handlers record. Each case is a fresh process, so these start at zero.

static COUNT: [AtomicU32; 65] = [const { AtomicU32::new(0) }; 65];
static ORDER: [AtomicU32; 64] = [const { AtomicU32::new(0) }; 64];
static ORDER_LEN: AtomicUsize = AtomicUsize::new(0);
/// The mask the last handler ran with.
static SEEN_MASK: AtomicU64 = AtomicU64::new(0);
/// The address of a local of the last handler, which says which stack it ran on.
static SEEN_SP: AtomicUsize = AtomicUsize::new(0);
/// The thread the last handler ran on.
static SEEN_TID: AtomicI32 = AtomicI32::new(0);
/// When the last handler ran, in monotonic ms.
static SEEN_AT: AtomicU64 = AtomicU64::new(0);
/// What the last SA_SIGINFO handler was given: whether its siginfo and context pointers
/// were null, and the siginfo's fields.
static INFO_NULL: AtomicI32 = AtomicI32::new(-1);
static CTX_NULL: AtomicI32 = AtomicI32::new(-1);
static INFO_SIGNO: AtomicI32 = AtomicI32::new(0);
static INFO_CODE: AtomicI32 = AtomicI32::new(0);
static INFO_PID: AtomicI32 = AtomicI32::new(0);
static INFO_UID: AtomicU32 = AtomicU32::new(0);
static INFO_STATUS: AtomicI32 = AtomicI32::new(0);
static INFO_VALUE: AtomicU64 = AtomicU64::new(0);
/// The si_value of each SA_SIGINFO delivery, in order.
static VALUES: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
static VALUES_LEN: AtomicUsize = AtomicUsize::new(0);
static DEPTH: AtomicI32 = AtomicI32::new(0);
static MAX_DEPTH: AtomicI32 = AtomicI32::new(0);

fn count(sig: i32) -> u32 { COUNT[sig as usize].load(Ordering::SeqCst) }

fn order() -> Vec<i32> {
    let n = ORDER_LEN.load(Ordering::SeqCst).min(64);
    (0..n).map(|i| ORDER[i].load(Ordering::SeqCst) as i32).collect()
}

fn record(sig: i32) {
    let local = 0u8;
    // Taking a volatile address keeps the local in this handler's stack frame.
    unsafe { core::ptr::read_volatile(&local) };
    SEEN_SP.store(core::ptr::addr_of!(local) as usize, Ordering::SeqCst);
    SEEN_MASK.store(mask_raw(), Ordering::SeqCst);
    SEEN_TID.store(gettid(), Ordering::SeqCst);
    SEEN_AT.store(now_ms(), Ordering::SeqCst);
    if (0..=64).contains(&sig) { COUNT[sig as usize].fetch_add(1, Ordering::SeqCst); }
    let at = ORDER_LEN.fetch_add(1, Ordering::SeqCst);
    if at < 64 { ORDER[at].store(sig as u32, Ordering::SeqCst); }
}

extern "C" fn on_sig(sig: i32) { record(sig); }

extern "C" fn on_info(sig: i32, info: *const SigInfo, ctx: *const u8) {
    INFO_NULL.store(info.is_null() as i32, Ordering::SeqCst);
    CTX_NULL.store(ctx.is_null() as i32, Ordering::SeqCst);
    if !info.is_null() {
        // SAFETY: the kernel passes a siginfo_t for the duration of the handler.
        let info = unsafe { core::ptr::read_volatile(info) };
        INFO_SIGNO.store(info.signo, Ordering::SeqCst);
        INFO_CODE.store(info.code, Ordering::SeqCst);
        INFO_PID.store(info.pid(), Ordering::SeqCst);
        INFO_UID.store(info.uid(), Ordering::SeqCst);
        INFO_STATUS.store(info.status(), Ordering::SeqCst);
        INFO_VALUE.store(info.value(), Ordering::SeqCst);
        let at = VALUES_LEN.fetch_add(1, Ordering::SeqCst);
        if at < 64 { VALUES[at].store(info.value(), Ordering::SeqCst); }
    }
    record(sig);
}

/// The siginfo the last SA_SIGINFO handler received, or why there is none.
fn seen_info() -> Result<SigInfo, String> {
    match INFO_NULL.load(Ordering::SeqCst) {
        -1 => return Err("the SA_SIGINFO handler never ran".into()),
        1 => return Err("the SA_SIGINFO handler was passed a null siginfo_t".into()),
        _ => {}
    }
    let mut info = SigInfo::zero();
    info.signo = INFO_SIGNO.load(Ordering::SeqCst);
    info.code = INFO_CODE.load(Ordering::SeqCst);
    info.fields[0] = (INFO_PID.load(Ordering::SeqCst) as u32 as u64) | ((INFO_UID.load(Ordering::SeqCst) as u64) << 32);
    info.fields[1] = INFO_VALUE.load(Ordering::SeqCst);
    Ok(info)
}

/// Check the last SA_SIGINFO delivery: its signal, si_code and sender.
fn expect_info(sig: i32, code: i32, sender: i32, uid: u32) -> Checked {
    let info = seen_info()?;
    if CTX_NULL.load(Ordering::SeqCst) != 0 {
        return Err("the SA_SIGINFO handler was passed a null ucontext".into());
    }
    if info.signo != sig { return Err(format!("si_signo is {}, expected {sig}", info.signo)); }
    if info.code != code { return Err(format!("si_code is {}, expected {code}", info.code)); }
    if info.pid() != sender { return Err(format!("si_pid is {}, expected the sender {sender}", info.pid())); }
    if info.uid() != uid { return Err(format!("si_uid is {}, expected the sender's real user ID {uid}", info.uid())); }
    Ok(())
}

fn msg(e: CaseError) -> String {
    match e { CaseError::Fail(m) | CaseError::Skip(m) => m }
}

fn waitid(pid: i32, options: i32, info: &mut SigInfo) -> i64 {
    sc(nr::WAITID, &[P_PID, pid as u64, info as *mut SigInfo as u64, options as u64, 0])
}

const WNOWAIT: i32 = 0x0100_0000;

/// Wait up to `ms` for `pid` to have exited without reaping it.
fn exited_unreaped(pid: i32, ms: u64) -> bool {
    until(ms, || {
        let mut info = SigInfo::zero();
        waitid(pid, WEXITED | WNOWAIT | WNOHANG, &mut info) == 0 && info.pid() == pid
    })
}

/// The meaning of each exit code a child that checks steps by exit code may end with.
fn coded(status: i32, sig: i32, what: &str, codes: &[(i32, &str)]) -> CaseResult {
    if signaled(status) && term_sig(status) == sig { return Ok(()); }
    if exited(status) {
        if let Some((_, meaning)) = codes.iter().find(|(code, _)| *code == exit_code(status)) {
            return fail(*meaning);
        }
    }
    fail(format!("{what} ended with {}, expected death by {}", status_text(status), name(sig)))
}

struct CArgs { _owned: Vec<CString>, ptrs: Vec<*const u8> }

fn cargs(items: &[String]) -> CArgs {
    let owned: Vec<CString> = items.iter().map(|s| CString::new(s.as_str()).expect("argument without NUL")).collect();
    let mut ptrs: Vec<*const u8> = owned.iter().map(|c| c.as_ptr() as *const u8).collect();
    ptrs.push(core::ptr::null());
    CArgs { _owned: owned, ptrs }
}

/// Fork a child that runs `setup` and then execs the helper with `args`; returns the child
/// and, when the exec failed, its errno. The child reports on a close-on-exec pipe, so end
/// of file with nothing written means the exec replaced it.
fn spawn_helper(args: &[String], setup: impl FnOnce() -> Checked) -> Result<(Child, Option<i64>), CaseError> {
    let path = CString::new(HELPER).expect("path without NUL");
    let argv = cargs(args);
    let envp = cargs(&[]);
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(r);
        if let Err(msg) = setup() {
            let _ = io::write(w, format!("S{msg}").as_bytes());
            return 126;
        }
        let ret = sc(nr::EXECVE, &[path.as_ptr() as u64, argv.ptrs.as_ptr() as u64, envp.ptrs.as_ptr() as u64]);
        let _ = io::write(w, format!("E{}", -ret).as_bytes());
        127
    });
    let _ = io::close(w);
    let child = match child {
        Ok(child) => child,
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    let answer = read_to_eof(r, EXEC_MS);
    let _ = io::close(r);
    let answer = answer.map_err(|e| format!("waiting for the exec: {e}"))?;
    match answer.first() {
        None => Ok((child, None)),
        Some(b'E') => Ok((child, std::str::from_utf8(&answer[1..]).ok().and_then(|s| s.parse().ok()))),
        Some(b'S') => err(format!("before the exec: {}", String::from_utf8_lossy(&answer[1..]))),
        _ => err("the exec's report was garbled"),
    }
}

/// The signal state the helper reports after `setup` and an exec: `mask`, `pending`,
/// `ign`, `caught` (as hexadecimal sets) and `altstack` (its ss_flags).
fn exec_state(setup: impl FnOnce() -> Checked) -> Result<HashMap<String, String>, CaseError> {
    let (r, w) = io::pipe()?;
    let args = ["signals-exec", "state", FD_ARG].iter()
        .map(|a| if *a == FD_ARG { w.raw().to_string() } else { a.to_string() })
        .collect::<Vec<_>>();
    let started = spawn_helper(&args, || { let _ = io::close(r); setup() });
    let _ = io::close(w);
    let (mut child, errno) = match started {
        Ok(started) => started,
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    if let Some(errno) = errno {
        let _ = io::close(r);
        return err(format!("exec of {HELPER} failed with {}", errname(errno)));
    }
    let out = read_to_eof(r, EXEC_MS);
    let _ = io::close(r);
    let out = out?;
    let status = child.wait_for("the exec'd helper")?;
    let state: HashMap<String, String> = String::from_utf8_lossy(&out).lines()
        .filter_map(|line| line.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    if let Some(error) = state.get("error") {
        return err(format!("in the exec'd helper: {error}"));
    }
    if !(exited(status) && exit_code(status) == 0) {
        return err(format!("the exec'd helper ended with {}", status_text(status)));
    }
    Ok(state)
}

fn state_set(state: &HashMap<String, String>, key: &str) -> Result<u64, CaseError> {
    let value = state.get(key).ok_or_else(|| format!("the exec'd helper did not report {key}"))?;
    u64::from_str_radix(value, 16).map_err(|_| CaseError::Fail(format!("the exec'd helper reported {key}={value}")))
}

// ---------------------------------------------------------------------------
// dispositions & default actions

/// Send each of `sigs` to a waiting child that has the default action; list those that
/// did not end it by that signal.
fn default_kills(sigs: &[i32]) -> CaseResult {
    let mut wrong = Vec::new();
    for &sig in sigs {
        let mut kid = held(|| if sig == SIGKILL { Ok(()) } else { default(sig) })?;
        want_eq(&format!("kill({})", name(sig)), kill(kid.pid(), sig), 0)?;
        kid.let_go();
        let status = kid.child.wait_for(&format!("a child sent {}", name(sig)))?;
        if !(signaled(status) && term_sig(status) == sig) {
            wrong.push(format!("{} left {}", name(sig), status_text(status)));
        }
    }
    check(wrong.is_empty(), &format!("with the default action, {}", wrong.join(", ")))
}

fn disp_terminate() -> CaseResult {
    default_kills(&[SIGHUP, SIGINT, SIGKILL, SIGPIPE, SIGALRM, SIGTERM, SIGUSR1, SIGUSR2, SIGPOLL, SIGPROF, SIGVTALRM])
}

fn disp_core() -> CaseResult {
    default_kills(&[SIGQUIT, SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGSEGV, SIGSYS, SIGXCPU, SIGXFSZ])
}

fn disp_ignore() -> CaseResult {
    let mut wrong = Vec::new();
    for sig in [SIGCHLD, SIGURG, SIGWINCH] {
        let mut kid = held(|| default(sig))?;
        want_eq(&format!("kill({})", name(sig)), kill(kid.pid(), sig), 0)?;
        kid.let_go();
        let status = kid.child.wait_for(&format!("a child sent {}", name(sig)))?;
        if !(exited(status) && exit_code(status) == 0) {
            wrong.push(format!("{} left {}", name(sig), status_text(status)));
        }
    }
    check(wrong.is_empty(), &format!("signals whose default action is to be ignored: {}", wrong.join(", ")))
}

fn disp_stop() -> CaseResult {
    for sig in [SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU] {
        // A group of its own, with its parent in another group of the session, so the
        // group is not orphaned and the terminal stop signals may stop it.
        let mut kid = held(|| {
            ok("setpgid", setpgid(0, 0))?;
            if sig == SIGSTOP { Ok(()) } else { default(sig) }
        })?;
        want_eq(&format!("kill({})", name(sig)), kill(kid.pid(), sig), 0)?;
        kid.child.expect_stop(sig, &format!("a child sent {}", name(sig)))?;
    }
    Ok(())
}

fn disp_continue() -> CaseResult {
    let kid = held(|| default(SIGCONT))?;
    want_eq("kill(SIGCONT)", kill(kid.pid(), SIGCONT), 0)?;
    kid.release()
}

fn disp_sig_ign() -> CaseResult {
    let sigs = [SIGHUP, SIGINT, SIGQUIT, SIGUSR1, SIGALRM, SIGTERM, SIGTSTP, SIGRTMIN + 2];
    let mut kid = held(|| {
        ok("setpgid", setpgid(0, 0))?;
        for sig in sigs { ignore(sig)?; }
        Ok(())
    })?;
    for sig in sigs { want_eq(&format!("kill({})", name(sig)), kill(kid.pid(), sig), 0)?; }
    if let Some(status) = reports_within(kid.pid(), WUNTRACED, QUIET_MS)? {
        kid.child.live = stopped(status);
        return fail(format!("a child ignoring these signals reported {} after being sent them", status_text(status)));
    }
    kid.release()
}

fn disp_ign_discards() -> CaseResult {
    catch(SIGUSR1)?;
    block(bit(SIGUSR1))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(pending()? & bit(SIGUSR1) != 0, "a blocked SIGUSR1 was not pending")?;
    ignore(SIGUSR1)?;
    check(pending()? & bit(SIGUSR1) == 0, "setting SIG_IGN left the blocked SIGUSR1 pending")?;
    catch(SIGUSR1)?;
    unblock(bit(SIGUSR1))?;
    check(count(SIGUSR1) == 0, "the discarded SIGUSR1 was delivered once it was caught and unblocked")
}

fn disp_dfl_restores() -> CaseResult {
    let mut kid = Child::start(|| {
        if catch(SIGUSR1).is_err() || raise(SIGUSR1) != 0 { return 10; }
        if count(SIGUSR1) != 1 { return 11; }
        if default(SIGUSR1).is_err() { return 12; }
        raise(SIGUSR1);
        0
    })?;
    let status = kid.wait_for("the child")?;
    coded(status, SIGUSR1, "a child that restored SIG_DFL and raised SIGUSR1", &[
        (10, "catching or raising SIGUSR1 failed"),
        (11, "the handler did not run on the first SIGUSR1"),
        (12, "sigaction(SIGUSR1, SIG_DFL) failed"),
        (0, "after SIG_DFL was restored, SIGUSR1 did not terminate the process"),
    ])
}

fn disp_sigaction_old() -> CaseResult {
    check(disposition(SIGUSR2)?.handler == SIG_DFL, "a signal never set does not report SIG_DFL")?;
    set_action(SIGUSR1, &action(on_sig as usize as u64, SA_RESTART, bit(SIGUSR2)))?;
    let mut old = Sigaction::default();
    want_eq("sigaction(SIGUSR1, SIG_IGN, &old)", sigaction(SIGUSR1, Some(&action(SIG_IGN, 0, 0)), Some(&mut old)), 0)?;
    check(old.handler == on_sig as usize as u64, &format!("the old handler is {:#x}, expected the installed one", old.handler))?;
    check(old.mask & bit(SIGUSR2) != 0, &format!("the old sa_mask is {:#x}, expected SIGUSR2 in it", old.mask))?;
    check(old.flags & SA_RESTART != 0, &format!("the old sa_flags are {:#x}, expected SA_RESTART", old.flags))?;
    check(disposition(SIGUSR1)?.handler == SIG_IGN, "a query with a null action did not report the SIG_IGN just installed")?;
    let mut again = Sigaction::default();
    want_eq("sigaction(SIGUSR1, NULL, &old)", sigaction(SIGUSR1, None, Some(&mut again)), 0)?;
    check(again.handler == SIG_IGN, "a query with a null action changed the disposition")
}

fn disp_einval() -> CaseResult {
    let act = action(on_sig as usize as u64, 0, 0);
    for sig in [0, -1, 65, 1000] {
        want_err(&format!("sigaction({sig}, handler)"), sigaction(sig, Some(&act), None), EINVAL)?;
        let mut old = Sigaction::default();
        want_err(&format!("sigaction({sig}, NULL, &old)"), sigaction(sig, None, Some(&mut old)), EINVAL)?;
    }
    Ok(())
}

fn disp_kill_stop() -> CaseResult {
    for sig in [SIGKILL, SIGSTOP] {
        want_err(&format!("catching {}", name(sig)), sigaction(sig, Some(&action(on_sig as usize as u64, 0, 0)), None), EINVAL)?;
        want_err(&format!("ignoring {}", name(sig)), sigaction(sig, Some(&action(SIG_IGN, 0, 0)), None), EINVAL)?;
        let old = disposition(sig)?;
        check(old.handler == SIG_DFL, &format!("{} reports handler {:#x}, expected SIG_DFL", name(sig), old.handler))?;
    }
    Ok(())
}

/// Linux's `struct sigaction` for rt_sigaction is handler, flags, restorer, mask. A C
/// library built for the Linux ABI fills it in that order.
fn disp_sigaction_layout() -> CaseResult {
    in_child(|| {
        let act: [u64; 4] = [on_sig as usize as u64, SA_RESTORER, restore_rt as usize as u64, bit(SIGUSR2)];
        ok("rt_sigaction with the Linux struct sigaction",
            sc(nr::RT_SIGACTION, &[SIGUSR1 as u64, act.as_ptr() as u64, 0, 8]))?;
        ok("raise(SIGUSR1)", raise(SIGUSR1))?;
        if count(SIGUSR1) != 1 {
            return Err("a handler installed with the Linux struct sigaction did not run".into());
        }
        let seen = SEEN_MASK.load(Ordering::SeqCst);
        if seen & bit(SIGUSR2) == 0 {
            return Err(format!("sa_mask, the fourth word, was not applied: the handler ran with mask {seen:#x}, without SIGUSR2"));
        }
        let mut back = [0u64; 4];
        ok("querying rt_sigaction", sc(nr::RT_SIGACTION, &[SIGUSR1 as u64, 0, back.as_mut_ptr() as u64, 8]))?;
        if back[0] != act[0] || back[3] != act[3] {
            return Err(format!("the old action came back as {back:#x?}, not in the Linux order"));
        }
        Ok(())
    })
}

fn disp_kill_zero() -> CaseResult {
    want_eq("kill(getpid(), 0)", kill(pid(), 0), 0)?;
    let kid = held(|| Ok(()))?;
    want_eq("kill(child, 0)", kill(kid.pid(), 0), 0)?;
    let mut zombie = Child::start(|| 0)?;
    check(exited_unreaped(zombie.pid, WAIT_MS), "a child that exits was never reported by waitid(WNOWAIT)")?;
    want_eq("kill(zombie, 0)", kill(zombie.pid, 0), 0)?;
    zombie.expect_exit(0, "the zombie")?;
    want_err("kill(reaped PID, 0)", kill(zombie.pid, 0), ESRCH)?;
    kid.release()
}

fn disp_kill_einval() -> CaseResult {
    for sig in [-1, 65, 1000] {
        want_err(&format!("kill(getpid(), {sig})"), kill(pid(), sig), EINVAL)?;
    }
    Ok(())
}

fn disp_kill_esrch() -> CaseResult {
    let gone = gone_pid()?;
    want_err("kill(reaped PID, SIGTERM)", kill(gone, SIGTERM), ESRCH)?;
    want_err("kill(reaped PID, 0)", kill(gone, 0), ESRCH)?;
    want_err("kill(-PGID of no group, SIGTERM)", kill(-gone, SIGTERM), ESRCH)
}

fn disp_kill_group() -> CaseResult {
    let leader = held(|| ok("setpgid", setpgid(0, 0)).map(|_| ()))?;
    let group = leader.pid();
    let member = held(move || ok("setpgid", setpgid(0, group)).map(|_| ()))?;
    let outsider = held(|| Ok(()))?;
    want_eq("kill(-pgid, SIGUSR1)", kill(-group, SIGUSR1), 0)?;
    leader.expect_death(SIGUSR1, "the group's leader")?;
    member.expect_death(SIGUSR1, "the group's other member")?;
    outsider.release().map_err(|e| CaseError::Fail(format!("a process outside the group: {}", msg(e))))
}

fn disp_kill_own_group() -> CaseResult {
    in_child(|| {
        ok("setpgid", setpgid(0, 0))?;
        let mut member = Child::start(|| loop { pause(); }).map_err(msg)?;
        catch(SIGUSR1)?;
        if !parked(member.pid, WAIT_MS) { return Err("the other member never blocked".into()); }
        ok("kill(0, SIGUSR1)", kill(0, SIGUSR1))?;
        let status = member.wait().map_err(msg)?;
        if !(signaled(status) && term_sig(status) == SIGUSR1) {
            return Err(format!("the other member of the caller's group ended with {}", status_text(status)));
        }
        if !until(1000, || count(SIGUSR1) == 1) {
            return Err("kill(0) did not signal the caller, which is in its own group".into());
        }
        Ok(())
    })
}

fn disp_kill_all() -> CaseResult {
    // The case runs as root; should kill(-1) reach it anyway, it survives to report.
    ignore(SIGUSR1)?;
    let other = held(|| { default(SIGUSR1)?; ok("setuid", setuid(USER_B)).map(|_| ()) })?;
    in_child(|| {
        ok("setuid", setuid(USER_A))?;
        let mut kids = Vec::new();
        for _ in 0..2 {
            kids.push(Child::start(|| { let _ = default(SIGUSR1); loop { pause(); } }).map_err(msg)?);
        }
        for kid in &kids {
            if !parked(kid.pid, WAIT_MS) { return Err("a target never blocked".into()); }
        }
        ok("kill(-1, SIGUSR1)", kill(-1, SIGUSR1))?;
        for kid in kids.iter_mut() {
            let status = kid.wait().map_err(msg)?;
            if !(signaled(status) && term_sig(status) == SIGUSR1) {
                return Err(format!("a process of the caller's user ended with {}", status_text(status)));
            }
        }
        Ok(())
    })?;
    other.release().map_err(|e| CaseError::Fail(format!("kill(-1) as non-root reached another user's process: {}", msg(e))))
}

fn disp_kill_eperm() -> CaseResult {
    let other = held(|| ok("setuid", setuid(USER_B)).map(|_| ()))?;
    let target = other.pid();
    in_child(move || {
        ok("setuid", setuid(USER_A))?;
        for sig in [SIGUSR1, 0] {
            let r = kill(target, sig);
            if r != -EPERM {
                return Err(format!("kill(another user's process, {sig}) as non-root: expected EPERM, got {}", shown(r)));
            }
        }
        let mut own = Child::start(|| loop { pause(); }).map_err(msg)?;
        ok("kill(own user's process, SIGTERM)", kill(own.pid, SIGTERM))?;
        let status = own.wait().map_err(msg)?;
        if !(signaled(status) && term_sig(status) == SIGTERM) {
            return Err(format!("a process of the same user sent SIGTERM ended with {}", status_text(status)));
        }
        Ok(())
    })?;
    other.release().map_err(|e| CaseError::Fail(format!("the other user's process: {}", msg(e))))
}

fn disp_sigcont_session() -> CaseResult {
    let other = held(|| ok("setuid", setuid(USER_B)).map(|_| ()))?;
    let target = other.pid();
    in_child(move || {
        ok("setuid", setuid(USER_A))?;
        let r = kill(target, SIGCONT);
        if r != 0 {
            return Err(format!("SIGCONT to another user's process in the same session: expected 0, got {}", shown(r)));
        }
        Ok(())
    })?;
    other.release()
}

fn disp_fork() -> CaseResult {
    catch(SIGUSR1)?;
    ignore(SIGUSR2)?;
    in_child(|| {
        let caught = disposition(SIGUSR1)?;
        if caught.handler != on_sig as usize as u64 {
            return Err(format!("the child's SIGUSR1 handler is {:#x}, not the parent's", caught.handler));
        }
        if disposition(SIGUSR2)?.handler != SIG_IGN { return Err("SIGUSR2 is not ignored in the child".into()); }
        ok("raise(SIGUSR1)", raise(SIGUSR1))?;
        if count(SIGUSR1) != 1 { return Err("the inherited handler did not run in the child".into()); }
        ok("raise(SIGUSR2)", raise(SIGUSR2))?;
        Ok(())
    })
}

fn disp_exec() -> CaseResult {
    let state = exec_state(|| { catch(SIGUSR1)?; ignore(SIGUSR2)?; ignore(SIGTERM) })?;
    let caught = state_set(&state, "caught")?;
    let ign = state_set(&state, "ign")?;
    check(caught == 0, &format!("after exec these signals still have handlers: {caught:#x}"))?;
    check(ign & (bit(SIGUSR2) | bit(SIGTERM)) == bit(SIGUSR2) | bit(SIGTERM),
        &format!("after exec the ignored set is {ign:#x}; SIGUSR2 and SIGTERM were ignored"))?;
    check(ign & bit(SIGUSR1) == 0, "after exec the caught SIGUSR1 is ignored instead of SIG_DFL")
}

// ---------------------------------------------------------------------------
// masks & pending signals

fn mask_block_pending() -> CaseResult {
    catch(SIGUSR1)?;
    block(bit(SIGUSR1))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 0, "a blocked SIGUSR1 was delivered")?;
    check(pending()? & bit(SIGUSR1) != 0, "a blocked SIGUSR1 is not in sigpending")?;
    unblock(bit(SIGUSR1))?;
    check(count(SIGUSR1) == 1, &format!("after unblocking, the handler ran {} times before sigprocmask returned, expected once", count(SIGUSR1)))?;
    check(pending()? & bit(SIGUSR1) == 0, "SIGUSR1 is still pending after its delivery")
}

fn mask_stays_pending() -> CaseResult {
    catch(SIGUSR1)?;
    block(bit(SIGUSR1))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    for round in 0..20 {
        let _ = process::yield_now();
        let _ = time::sleep_ms(2);
        let _ = pid();
        check(pending()? & bit(SIGUSR1) != 0, &format!("the blocked SIGUSR1 was no longer pending after {round} rounds of syscalls and sleeps"))?;
        check(count(SIGUSR1) == 0, "the blocked SIGUSR1 was delivered while blocked")?;
    }
    unblock(bit(SIGUSR1))?;
    check(count(SIGUSR1) == 1, "the pending SIGUSR1 was not delivered once unblocked")
}

/// POSIX leaves whether a repeated standard signal is delivered more than once to the
/// implementation; Linux delivers it once.
fn mask_not_queued() -> CaseResult {
    catch(SIGUSR1)?;
    block(bit(SIGUSR1))?;
    for _ in 0..5 { want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?; }
    unblock(bit(SIGUSR1))?;
    let _ = until(100, || false);
    check(count(SIGUSR1) == 1, &format!("SIGUSR1 raised five times while blocked was delivered {} times, expected once", count(SIGUSR1)))
}

fn mask_how() -> CaseResult {
    setmask(0)?;
    let old = procmask(SIG_BLOCK, Some(bit(SIGUSR1) | bit(SIGHUP)))?;
    check(old == 0, &format!("the old mask is {old:#x}, expected empty"))?;
    let old = procmask(SIG_BLOCK, Some(bit(SIGUSR2)))?;
    check(old == bit(SIGUSR1) | bit(SIGHUP), &format!("SIG_BLOCK: old mask {old:#x}"))?;
    let old = procmask(SIG_UNBLOCK, Some(bit(SIGHUP)))?;
    check(old == bit(SIGUSR1) | bit(SIGHUP) | bit(SIGUSR2), &format!("SIG_BLOCK did not add: mask was {old:#x}"))?;
    let old = procmask(SIG_SETMASK, Some(bit(SIGTERM)))?;
    check(old == bit(SIGUSR1) | bit(SIGUSR2), &format!("SIG_UNBLOCK did not remove only SIGHUP: mask was {old:#x}"))?;
    let now = mask_now()?;
    check(now == bit(SIGTERM), &format!("SIG_SETMASK did not replace the mask: it is {now:#x}"))
}

fn mask_einval() -> CaseResult {
    setmask(bit(SIGUSR1))?;
    for how in [3, 99, -1] {
        let set = [bit(SIGUSR2)];
        let mut old = [0u64];
        want_err(&format!("sigprocmask(how {how})"),
            sc(nr::RT_SIGPROCMASK, &[how as i64 as u64, set.as_ptr() as u64, old.as_mut_ptr() as u64, 8]), EINVAL)?;
    }
    let now = mask_now()?;
    check(now == bit(SIGUSR1), &format!("a failed sigprocmask changed the mask to {now:#x}"))
}

fn mask_null_set() -> CaseResult {
    setmask(bit(SIGUSR1))?;
    let mut old = [0u64];
    want_eq("sigprocmask(99, NULL, &old)", sc(nr::RT_SIGPROCMASK, &[99, 0, old.as_mut_ptr() as u64, 8]), 0)?;
    check(old[0] == bit(SIGUSR1), &format!("sigprocmask with a null set reported {:#x}, expected the mask", old[0]))?;
    check(mask_now()? == bit(SIGUSR1), "sigprocmask with a null set changed the mask")
}

fn mask_unblockable() -> CaseResult {
    setmask(!0)?;
    let now = mask_now()?;
    check(now & (bit(SIGKILL) | bit(SIGSTOP)) == 0, &format!("the mask {now:#x} includes SIGKILL or SIGSTOP"))?;
    check(now & bit(SIGUSR1) != 0, "blocking every signal did not block SIGUSR1")?;
    let mut stopper = held(|| setmask(!0).map(|_| ()))?;
    want_eq("kill(SIGSTOP)", kill(stopper.pid(), SIGSTOP), 0)?;
    stopper.child.expect_stop(SIGSTOP, "a child blocking every signal")?;
    let killed = held(|| setmask(!0).map(|_| ()))?;
    want_eq("kill(SIGKILL)", kill(killed.pid(), SIGKILL), 0)?;
    killed.expect_death(SIGKILL, "a child blocking every signal and sent SIGKILL")
}

fn mask_sigpending() -> CaseResult {
    catch(SIGUSR1)?;
    catch(SIGHUP)?;
    catch(SIGUSR2)?;
    block(bit(SIGUSR1) | bit(SIGUSR2) | bit(SIGHUP))?;
    check(pending()? == 0, "sigpending reports signals before any were raised")?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    want_eq("raise(SIGHUP)", raise(SIGHUP), 0)?;
    let set = pending()?;
    check(set == bit(SIGUSR1) | bit(SIGHUP), &format!("sigpending reports {set:#x}, expected exactly SIGHUP and SIGUSR1"))?;
    unblock(bit(SIGUSR1) | bit(SIGUSR2) | bit(SIGHUP))?;
    check(pending()? == 0, "sigpending still reports signals after they were delivered")
}

fn mask_unblock_all() -> CaseResult {
    catch(SIGUSR1)?;
    catch(SIGUSR2)?;
    block(bit(SIGUSR1) | bit(SIGUSR2))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    want_eq("raise(SIGUSR2)", raise(SIGUSR2), 0)?;
    unblock(bit(SIGUSR1) | bit(SIGUSR2))?;
    let _ = until(100, || false);
    check(count(SIGUSR1) == 1 && count(SIGUSR2) == 1,
        &format!("after unblocking both, SIGUSR1 ran {} and SIGUSR2 {} times, expected once each", count(SIGUSR1), count(SIGUSR2)))
}

static THREAD_TID: AtomicI32 = AtomicI32::new(0);

fn join<T>(handle: std::thread::JoinHandle<Result<T, String>>) -> Result<T, CaseError> {
    match handle.join() {
        Ok(result) => result.map_err(CaseError::Fail),
        Err(_) => err("the thread panicked"),
    }
}

fn mask_per_thread() -> CaseResult {
    setmask(0)?;
    let thread = std::thread::spawn(|| -> Result<u64, String> {
        block(bit(SIGUSR1))?;
        mask_now()
    });
    let theirs = join(thread)?;
    check(theirs & bit(SIGUSR1) != 0, &format!("a thread that blocked SIGUSR1 reports mask {theirs:#x}"))?;
    let mine = mask_now()?;
    check(mine & bit(SIGUSR1) == 0, "pthread_sigmask in one thread changed another thread's mask")
}

fn mask_thread_inherits() -> CaseResult {
    setmask(bit(SIGUSR2))?;
    let thread = std::thread::spawn(|| -> Result<u64, String> {
        let first = mask_now()?;
        unblock(bit(SIGUSR2))?;
        Ok(first)
    });
    let theirs = join(thread)?;
    check(theirs == bit(SIGUSR2), &format!("a new thread started with mask {theirs:#x}, expected its creator's {:#x}", bit(SIGUSR2)))?;
    check(mask_now()? == bit(SIGUSR2), "a thread unblocking SIGUSR2 changed its creator's mask")
}

/// Set by a case to let its waiting thread finish.
static THREAD_RELEASE: AtomicU32 = AtomicU32::new(0);

/// A thread with SIGUSR1 unblocked, waiting until its handler has run or the case
/// releases it, so it is alive whenever the case signals it.
fn waiting_thread() -> std::thread::JoinHandle<Result<(), String>> {
    std::thread::spawn(|| -> Result<(), String> {
        unblock(bit(SIGUSR1))?;
        THREAD_TID.store(gettid(), Ordering::SeqCst);
        while count(SIGUSR1) == 0 && THREAD_RELEASE.load(Ordering::SeqCst) == 0 { let _ = process::yield_now(); }
        Ok(())
    })
}

/// Wait for SIGUSR1's handler, then release the waiting thread and join it.
fn finish_waiting(thread: std::thread::JoinHandle<Result<(), String>>) -> CaseResult {
    let _ = until(WAIT_MS, || count(SIGUSR1) > 0);
    THREAD_RELEASE.store(1, Ordering::SeqCst);
    join(thread)
}

fn mask_process_directed() -> CaseResult {
    catch(SIGUSR1)?;
    block(bit(SIGUSR1))?;
    let thread = waiting_thread();
    if !until(WAIT_MS, || THREAD_TID.load(Ordering::SeqCst) != 0) {
        THREAD_RELEASE.store(1, Ordering::SeqCst);
        let _ = join(thread);
        return fail("the thread never started");
    }
    let r = raise(SIGUSR1);
    finish_waiting(thread)?;
    want_eq("kill(getpid(), SIGUSR1)", r, 0)?;
    check(count(SIGUSR1) == 1, "a signal sent to the process, blocked in one thread, did not reach the thread that has it unblocked")?;
    check(SEEN_TID.load(Ordering::SeqCst) == THREAD_TID.load(Ordering::SeqCst),
        "the handler ran on the thread that blocks the signal")
}

fn mask_thread_directed() -> CaseResult {
    catch(SIGUSR1)?;
    setmask(0)?;
    let thread = waiting_thread();
    if !until(WAIT_MS, || THREAD_TID.load(Ordering::SeqCst) != 0) {
        THREAD_RELEASE.store(1, Ordering::SeqCst);
        let _ = join(thread);
        return fail("the thread never started");
    }
    let r = tgkill(pid(), THREAD_TID.load(Ordering::SeqCst), SIGUSR1);
    finish_waiting(thread)?;
    want_eq("tgkill (pthread_kill)", r, 0)?;
    check(count(SIGUSR1) == 1, "pthread_kill's signal was never handled")?;
    check(SEEN_TID.load(Ordering::SeqCst) == THREAD_TID.load(Ordering::SeqCst),
        "pthread_kill's signal was handled on another thread than the one named")
}

fn mask_fork() -> CaseResult {
    setmask(bit(SIGUSR1) | bit(SIGHUP))?;
    in_child(|| {
        let mask = mask_now()?;
        if mask != bit(SIGUSR1) | bit(SIGHUP) { return Err(format!("the child's mask is {mask:#x}, not its parent's")); }
        Ok(())
    })
}

fn mask_fork_pending() -> CaseResult {
    catch(SIGUSR1)?;
    block(bit(SIGUSR1))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    in_child(|| {
        let set = pending()?;
        if set != 0 { return Err(format!("the child starts with pending set {set:#x}")); }
        unblock(bit(SIGUSR1))?;
        if count(SIGUSR1) != 0 { return Err("the parent's pending SIGUSR1 was delivered in the child".into()); }
        Ok(())
    })?;
    check(pending()? & bit(SIGUSR1) != 0, "the parent's pending SIGUSR1 was lost across fork")
}

fn mask_exec() -> CaseResult {
    let state = exec_state(|| {
        block(bit(SIGUSR1) | bit(SIGUSR2))?;
        ok("raise(SIGUSR2)", raise(SIGUSR2)).map(|_| ())
    })?;
    let mask = state_set(&state, "mask")?;
    let set = state_set(&state, "pending")?;
    check(mask & (bit(SIGUSR1) | bit(SIGUSR2)) == bit(SIGUSR1) | bit(SIGUSR2),
        &format!("after exec the mask is {mask:#x}; SIGUSR1 and SIGUSR2 were blocked"))?;
    check(set & bit(SIGUSR2) != 0, &format!("after exec the pending set is {set:#x}; SIGUSR2 was pending"))
}

fn mask_exec_delivered() -> CaseResult {
    // The helper says it runs before it unblocks: a SIGTERM that killed the child before
    // the exec also closes the exec pipe, and would otherwise look like a successful exec.
    let (r, w) = io::pipe()?;
    let args = ["signals-exec".to_string(), "unblock".to_string(), SIGTERM.to_string(), w.raw().to_string()];
    let started = spawn_helper(&args, || {
        let _ = io::close(r);
        default(SIGTERM)?;
        block(bit(SIGTERM))?;
        ok("raise(SIGTERM)", raise(SIGTERM))?;
        let set = pending()?;
        if set & bit(SIGTERM) == 0 { return Err(format!("the raised SIGTERM is not pending before the exec: pending set {set:#x}")); }
        Ok(())
    });
    let _ = io::close(w);
    let (mut child, errno) = match started {
        Ok(started) => started,
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    if let Some(errno) = errno {
        let _ = io::close(r);
        let _ = child.wait();
        return fail(format!("exec of {HELPER} failed with {}", errname(errno)));
    }
    let said = read_to_eof(r, EXEC_MS);
    let _ = io::close(r);
    let said = said?;
    let status = child.wait_for("the child")?;
    if said != b"exec'd\n" {
        return fail(format!("the exec'd helper never reported that it ran; the child ended with {}", status_text(status)));
    }
    check(signaled(status) && term_sig(status) == SIGTERM,
        &format!("a program that unblocked the SIGTERM pending across exec ended with {}, expected death by SIGTERM", status_text(status)))
}

// ---------------------------------------------------------------------------
// handlers, SA_RESTART & EINTR

/// Append `value` to the order handlers record into.
fn push(value: i32) {
    let at = ORDER_LEN.fetch_add(1, Ordering::SeqCst);
    if at < 64 { ORDER[at].store(value as u32, Ordering::SeqCst); }
}

fn h_runs() -> CaseResult {
    catch(SIGUSR1)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the handler had not run when kill(getpid()) returned")?;
    let seen = order();
    check(seen == [SIGUSR1], &format!("the handler was passed {seen:?}, expected the signal number {SIGUSR1}"))
}

fn h_siginfo_self() -> CaseResult {
    catch_info(SIGUSR1)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the SA_SIGINFO handler had not run when kill(getpid()) returned")?;
    expect_info(SIGUSR1, SI_USER, pid(), getuid())?;
    Ok(())
}

fn h_siginfo_sender() -> CaseResult {
    in_child(|| {
        ok("setuid", setuid(USER_A))?;
        catch_info(SIGUSR1)?;
        block(bit(SIGUSR1))?;
        let me = pid();
        let mut sender = Child::start(move || if kill(me, SIGUSR1) == 0 { 0 } else { 1 }).map_err(msg)?;
        sender.expect_exit(0, "the sending child").map_err(msg)?;
        unblock(bit(SIGUSR1))?;
        expect_info(SIGUSR1, SI_USER, sender.pid, USER_A)
    })
}

static FAULT_ADDR: AtomicU64 = AtomicU64::new(0);

extern "C" fn on_fault(_sig: i32, info: *const SigInfo, _ctx: *const u8) {
    if info.is_null() { process::exit(3); }
    // SAFETY: the kernel passes a siginfo_t for the duration of the handler.
    let info = unsafe { core::ptr::read_volatile(info) };
    if info.signo != SIGSEGV { process::exit(4); }
    if info.code != SEGV_MAPERR { process::exit(5); }
    if info.addr() != FAULT_ADDR.load(Ordering::SeqCst) { process::exit(6); }
    process::exit(0)
}

fn h_siginfo_fault() -> CaseResult {
    let page = memory::mmap(core::ptr::null_mut(), 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)?;
    memory::munmap(page, 4096)?;
    let addr = page as u64 + 24;
    FAULT_ADDR.store(addr, Ordering::SeqCst);
    let mut kid = Child::start(move || {
        if catch_with(SIGSEGV, on_fault as usize as u64, SA_SIGINFO, 0).is_err() { return 10; }
        // SAFETY: none; the page was unmapped, so this read faults.
        unsafe { core::ptr::read_volatile(addr as *const u64) };
        11
    })?;
    let status = kid.wait_for("the child")?;
    if exited(status) && exit_code(status) == 0 { return Ok(()); }
    if signaled(status) && term_sig(status) == SIGSEGV {
        return fail("a read of an unmapped page killed the process; its SA_SIGINFO SIGSEGV handler never ran");
    }
    fail(match exit_code(status) {
        3 => "the SIGSEGV handler was passed a null siginfo_t".to_string(),
        4 => "the SIGSEGV handler's si_signo is not SIGSEGV".to_string(),
        5 => "the SIGSEGV handler's si_code is not SEGV_MAPERR for an unmapped address".to_string(),
        6 => "the SIGSEGV handler's si_addr is not the address read".to_string(),
        10 => "installing the SIGSEGV handler failed".to_string(),
        11 => "a read of an unmapped page did not fault".to_string(),
        _ => format!("the faulting child ended with {}", status_text(status)),
    })
}

/// The handler sigaction reported from inside the SA_RESETHAND handler, or u64::MAX
/// when the query failed.
static RESET_SEEN: AtomicU64 = AtomicU64::new(0);

extern "C" fn on_reset(sig: i32) {
    let mut old = Sigaction::default();
    let ret = sigaction(sig, None, Some(&mut old));
    RESET_SEEN.store(if ret == 0 { old.handler } else { u64::MAX }, Ordering::SeqCst);
    record(sig);
}

fn h_resethand() -> CaseResult {
    let mut kid = Child::start(|| {
        if catch_with(SIGUSR1, on_reset as usize as u64, SA_RESETHAND, 0).is_err() { return 10; }
        if raise(SIGUSR1) != 0 || count(SIGUSR1) != 1 { return 11; }
        match RESET_SEEN.load(Ordering::SeqCst) { SIG_DFL => {} u64::MAX => return 13, _ => return 12 }
        raise(SIGUSR1);
        0
    })?;
    let status = kid.wait_for("the child")?;
    coded(status, SIGUSR1, "a child raising SIGUSR1 twice under SA_RESETHAND", &[
        (10, "installing the SA_RESETHAND handler failed"),
        (11, "the SA_RESETHAND handler did not run on the first SIGUSR1"),
        (12, "inside the SA_RESETHAND handler the disposition was not yet SIG_DFL: it is to be reset on entry"),
        (13, "querying sigaction inside the SA_RESETHAND handler failed"),
        (0, "a second SIGUSR1 after an SA_RESETHAND delivery did not terminate the process"),
    ])
}

fn h_self_blocked() -> CaseResult {
    setmask(0)?;
    catch(SIGUSR1)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    let seen = SEEN_MASK.load(Ordering::SeqCst);
    check(seen & bit(SIGUSR1) != 0, &format!("the handler ran with mask {seen:#x}, without its own signal"))?;
    let after = mask_now()?;
    check(after == 0, &format!("after the handler returned the mask is {after:#x}, expected empty"))
}

fn h_sa_mask() -> CaseResult {
    setmask(0)?;
    catch_with(SIGUSR1, on_sig as usize as u64, 0, bit(SIGUSR2) | bit(SIGHUP))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    let seen = SEEN_MASK.load(Ordering::SeqCst);
    check(seen & (bit(SIGUSR2) | bit(SIGHUP)) == bit(SIGUSR2) | bit(SIGHUP),
        &format!("the handler ran with mask {seen:#x}; sa_mask held SIGHUP and SIGUSR2"))?;
    let after = mask_now()?;
    check(after == 0, &format!("after the handler returned the mask is {after:#x}, expected empty"))
}

extern "C" fn on_reraise(sig: i32) {
    let depth = DEPTH.fetch_add(1, Ordering::SeqCst) + 1;
    MAX_DEPTH.fetch_max(depth, Ordering::SeqCst);
    record(sig);
    if count(sig) == 1 { raise(sig); }
    DEPTH.fetch_sub(1, Ordering::SeqCst);
}

fn h_nodefer() -> CaseResult {
    setmask(0)?;
    catch_with(SIGUSR1, on_reraise as usize as u64, SA_NODEFER, 0)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 2, &format!("the handler ran {} times, expected twice", count(SIGUSR1)))?;
    check(MAX_DEPTH.load(Ordering::SeqCst) == 2,
        "with SA_NODEFER, the signal raised in its own handler did not interrupt it")
}

fn h_deferred() -> CaseResult {
    setmask(0)?;
    catch_with(SIGUSR1, on_reraise as usize as u64, 0, 0)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    let _ = until(200, || count(SIGUSR1) >= 2);
    check(count(SIGUSR1) == 2, &format!("the handler ran {} times, expected twice", count(SIGUSR1)))?;
    check(MAX_DEPTH.load(Ordering::SeqCst) == 1,
        "without SA_NODEFER, the signal raised in its own handler interrupted it instead of waiting for it to return")
}

extern "C" fn on_outer(sig: i32) {
    record(sig);
    raise(SIGUSR2);
    push(100 + sig);
}

fn h_nested() -> CaseResult {
    setmask(0)?;
    catch_with(SIGUSR1, on_outer as usize as u64, 0, 0)?;
    catch(SIGUSR2)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    let seen = order();
    check(seen == [SIGUSR1, SIGUSR2, 100 + SIGUSR1],
        &format!("handler entries and exits ran in the order {seen:?}; expected SIGUSR2's handler inside SIGUSR1's ([10, 12, 110])"))
}

/// What on_blocks_hup's sigprocmask returned, and the mask it left the handler with.
static HUP_RET: AtomicI64 = AtomicI64::new(1);
static HUP_MASK: AtomicU64 = AtomicU64::new(0);

extern "C" fn on_blocks_hup(sig: i32) {
    record(sig);
    let set = [bit(SIGHUP)];
    HUP_RET.store(sc(nr::RT_SIGPROCMASK, &[SIG_BLOCK as u64, set.as_ptr() as u64, 0, 8]), Ordering::SeqCst);
    HUP_MASK.store(mask_raw(), Ordering::SeqCst);
}

fn h_mask_restored() -> CaseResult {
    setmask(0)?;
    catch_with(SIGUSR1, on_blocks_hup as usize as u64, 0, 0)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    want_eq("sigprocmask(SIG_BLOCK, SIGHUP) in the handler", HUP_RET.load(Ordering::SeqCst), 0)?;
    let inside = HUP_MASK.load(Ordering::SeqCst);
    check(inside & bit(SIGHUP) != 0, &format!("the handler blocked SIGHUP, but its mask was then {inside:#x}"))?;
    let after = mask_now()?;
    check(after == 0, &format!("a handler that blocked SIGHUP returned to a mask of {after:#x}, expected the empty mask it interrupted"))
}

/// A child that sends `sig` to this process once it blocks, then exits with `code`.
fn signal_when_parked(sig: i32) -> Result<Child, CaseError> {
    let me = pid();
    Child::start(move || {
        if !parked(me, WAIT_MS) { return 10; }
        if kill(me, sig) != 0 { return 11; }
        0
    })
}

fn h_restart_read() -> CaseResult {
    catch_with(SIGUSR1, on_sig as usize as u64, SA_RESTART, 0)?;
    let (r, w) = io::pipe()?;
    let me = pid();
    let s = Shared::new()?;
    let mut kid = Child::start(|| {
        let _ = io::close(r);
        if !parked(me, WAIT_MS) || kill(me, SIGUSR1) != 0 { return 10; }
        // Give the restarted read time to block again before the data arrives.
        let _ = time::sleep_ms(20);
        let _ = parked(me, WAIT_MS);
        s.set(0, now_ms().max(1));
        let _ = io::write(w, b"x");
        0
    })?;
    io::close(w)?;
    let mut buf = [0u8; 1];
    let got = read_raw(r, &mut buf);
    io::close(r)?;
    kid.expect_exit(0, "the signalling child")?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    check(SEEN_AT.load(Ordering::SeqCst) < s.get(0),
        "the handler ran only after the data arrived: the signal did not interrupt the blocked read")?;
    check(got == 1 && buf[0] == b'x',
        &format!("a read interrupted by an SA_RESTART handler returned {}, expected the byte written after the signal", shown(got)))
}

/// How long after its signal a child ends a wait that was wrongly restarted rather than
/// interrupted, so the case reports that instead of reaching the runner's limit.
const RESTART_END_MS: u64 = 1000;

fn h_eintr_read() -> CaseResult {
    catch(SIGUSR1)?;
    let (r, w) = io::pipe()?;
    let me = pid();
    let mut kid = Child::start(|| {
        let _ = io::close(r);
        if !parked(me, WAIT_MS) || kill(me, SIGUSR1) != 0 { return 10; }
        let _ = time::sleep_ms(RESTART_END_MS);
        let _ = io::write(w, b"x");
        0
    })?;
    io::close(w)?;
    let mut buf = [0u8; 1];
    let got = read_raw(r, &mut buf);
    // Kept open until the child has written, so its write does not raise SIGPIPE.
    let ended = kid.expect_exit(0, "the signalling child");
    let _ = io::close(r);
    ended?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    check(got != 1, &format!("a read interrupted by a handler without SA_RESTART was restarted: it returned the byte written {RESTART_END_MS} ms after the signal"))?;
    want_err("a read interrupted by a handler without SA_RESTART", got, EINTR)
}

fn h_nanosleep() -> CaseResult {
    catch_with(SIGUSR1, on_sig as usize as u64, SA_RESTART, 0)?;
    let mut kid = signal_when_parked(SIGUSR1)?;
    let mut rem = [0i64; 2];
    let start = now_ms();
    let got = nanosleep(3000, &mut rem);
    let slept = now_ms().saturating_sub(start) as i64;
    kid.expect_exit(0, "the signalling child")?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    want_err("a 3 s nanosleep interrupted by a handler, even with SA_RESTART", got, EINTR)?;
    check(slept < 1500, &format!("nanosleep returned only after {slept} ms of its 3000: the signal, sent once it blocked, did not end the sleep"))?;
    let left = rem[0] * 1000 + rem[1] / 1_000_000;
    check((left + slept - 3000).abs() <= 250,
        &format!("nanosleep slept {slept} ms of 3000 and reported {left} ms remaining"))
}

fn h_restart_wait() -> CaseResult {
    catch_with(SIGUSR2, on_sig as usize as u64, SA_RESTART, 0)?;
    let me = pid();
    let s = Shared::new()?;
    let mut kid = Child::start(|| {
        if !parked(me, WAIT_MS) || kill(me, SIGUSR2) != 0 { return 10; }
        let _ = time::sleep_ms(50);
        let _ = parked(me, WAIT_MS);
        s.set(0, now_ms().max(1));
        7
    })?;
    let mut status = 0;
    let got = wait4(kid.pid, &mut status, 0);
    if got == kid.pid as i64 { kid.live = false; }
    check(count(SIGUSR2) == 1, "the handler did not run")?;
    check(SEEN_AT.load(Ordering::SeqCst) < s.get(0),
        "the handler ran only after the child exited: the signal did not interrupt the blocked waitpid")?;
    want_eq("waitpid interrupted by an SA_RESTART handler", got, kid.pid as i64)?;
    check(exited(status) && exit_code(status) == 7, &format!("the restarted waitpid reported {}", status_text(status)))
}

fn h_eintr_wait() -> CaseResult {
    catch(SIGUSR2)?;
    let me = pid();
    let mut kid = Child::start(move || {
        if !parked(me, WAIT_MS) || kill(me, SIGUSR2) != 0 { return 10; }
        let _ = time::sleep_ms(RESTART_END_MS);
        7
    })?;
    let mut status = 0;
    let got = wait4(kid.pid, &mut status, 0);
    if got == kid.pid as i64 { kid.live = false; }
    check(count(SIGUSR2) == 1, "the handler did not run")?;
    check(got != kid.pid as i64, &format!("waitpid interrupted by a handler without SA_RESTART was restarted: it reported the child, which exited {RESTART_END_MS} ms after the signal, with {}", status_text(status)))?;
    want_err("waitpid interrupted by a handler without SA_RESTART", got, EINTR)
}

/// Integer work for the register cases' handler: it keeps many values live in registers,
/// overwriting what the interrupted code held there.
#[inline(never)]
fn checksum(rounds: u64) -> u64 {
    let (mut a, mut b, mut c, mut d) = (1u64, 2u64, 3u64, 5u64);
    let (mut e, mut f, mut g, mut h) = (7u64, 11u64, 13u64, 17u64);
    for i in 0..rounds {
        a = a.wrapping_mul(6364136223846793005).wrapping_add(b ^ i);
        b = b.rotate_left(7) ^ c;
        c = c.wrapping_add(d.wrapping_mul(3));
        d ^= e.rotate_right(11);
        e = e.wrapping_add(f ^ a);
        f = f.rotate_left(13).wrapping_add(g);
        g ^= h.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        h = h.wrapping_add(a ^ d);
    }
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h
}

/// Known values for the register cases, distinct in every 64-bit half.
const fn pattern(i: u64, salt: u64) -> u64 { (i + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ salt }

const fn int_pattern() -> [u64; 32] {
    let mut out = [0u64; 32];
    let mut i = 0;
    while i < 32 { out[i] = pattern(i as u64, 0x5a5a_0f0f_3c3c_9696); i += 1; }
    out
}

const fn fp_pattern() -> [[u64; 2]; 32] {
    let mut out = [[0u64; 2]; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = [pattern(i as u64, 0x0123_4567_89ab_cdef), pattern(i as u64, 0xfedc_ba98_7654_3210)];
        i += 1;
    }
    out
}

static INT_PATTERN: [u64; 32] = int_pattern();
static FP_PATTERN: [[u64; 2]; 32] = fp_pattern();

/// Load a known value into every general register the calling convention lets this
/// code hold, set IN_WORK, then check them all `rounds` times without a system call, so
/// a signal can only arrive through an interrupt while they are live; IN_WORK is cleared
/// before any of them is released. Returns 0, or 0x100 + the index of the first register
/// found wrong.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn int_hold(rounds: u64) -> u64 {
    let bad: u64;
    // SAFETY: reads INT_PATTERN, writes IN_WORK, and writes only registers declared below.
    unsafe {
        core::arch::asm!(
            "ldr x1, [x10, #0]",
            "ldr x2, [x10, #8]",
            "ldr x3, [x10, #16]",
            "ldr x4, [x10, #24]",
            "ldr x5, [x10, #32]",
            "ldr x6, [x10, #40]",
            "ldr x7, [x10, #48]",
            "ldr x8, [x10, #56]",
            "ldr x13, [x10, #64]",
            "ldr x14, [x10, #72]",
            "ldr x16, [x10, #80]",
            "ldr x17, [x10, #88]",
            "ldr x20, [x10, #96]",
            "ldr x21, [x10, #104]",
            "ldr x22, [x10, #112]",
            "ldr x23, [x10, #120]",
            "ldr x24, [x10, #128]",
            "ldr x25, [x10, #136]",
            "ldr x26, [x10, #144]",
            "ldr x27, [x10, #152]",
            "ldr x28, [x10, #160]",
            "mov w11, #1", "str w11, [x15]",
            "2:",
            "mov x12, #256", "ldr x11, [x10, #0]", "cmp x1, x11", "b.ne 3f",
            "mov x12, #257", "ldr x11, [x10, #8]", "cmp x2, x11", "b.ne 3f",
            "mov x12, #258", "ldr x11, [x10, #16]", "cmp x3, x11", "b.ne 3f",
            "mov x12, #259", "ldr x11, [x10, #24]", "cmp x4, x11", "b.ne 3f",
            "mov x12, #260", "ldr x11, [x10, #32]", "cmp x5, x11", "b.ne 3f",
            "mov x12, #261", "ldr x11, [x10, #40]", "cmp x6, x11", "b.ne 3f",
            "mov x12, #262", "ldr x11, [x10, #48]", "cmp x7, x11", "b.ne 3f",
            "mov x12, #263", "ldr x11, [x10, #56]", "cmp x8, x11", "b.ne 3f",
            "mov x12, #264", "ldr x11, [x10, #64]", "cmp x13, x11", "b.ne 3f",
            "mov x12, #265", "ldr x11, [x10, #72]", "cmp x14, x11", "b.ne 3f",
            "mov x12, #266", "ldr x11, [x10, #80]", "cmp x16, x11", "b.ne 3f",
            "mov x12, #267", "ldr x11, [x10, #88]", "cmp x17, x11", "b.ne 3f",
            "mov x12, #268", "ldr x11, [x10, #96]", "cmp x20, x11", "b.ne 3f",
            "mov x12, #269", "ldr x11, [x10, #104]", "cmp x21, x11", "b.ne 3f",
            "mov x12, #270", "ldr x11, [x10, #112]", "cmp x22, x11", "b.ne 3f",
            "mov x12, #271", "ldr x11, [x10, #120]", "cmp x23, x11", "b.ne 3f",
            "mov x12, #272", "ldr x11, [x10, #128]", "cmp x24, x11", "b.ne 3f",
            "mov x12, #273", "ldr x11, [x10, #136]", "cmp x25, x11", "b.ne 3f",
            "mov x12, #274", "ldr x11, [x10, #144]", "cmp x26, x11", "b.ne 3f",
            "mov x12, #275", "ldr x11, [x10, #152]", "cmp x27, x11", "b.ne 3f",
            "mov x12, #276", "ldr x11, [x10, #160]", "cmp x28, x11", "b.ne 3f",
            "subs x9, x9, #1",
            "b.ne 2b",
            "mov x12, #0",
            "3:",
            "str wzr, [x15]",
            inout("x9") rounds => _,
            in("x10") INT_PATTERN.as_ptr(),
            in("x15") IN_WORK.as_ptr(),
            out("x11") _, out("x12") bad,
            out("x1") _, out("x2") _, out("x3") _, out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x13") _, out("x14") _, out("x16") _, out("x17") _, out("x20") _, out("x21") _, out("x22") _, out("x23") _, out("x24") _, out("x25") _, out("x26") _, out("x27") _, out("x28") _,
            options(nostack),
        );
    }
    bad
}

/// As on ARM64, with the general registers x86-64 code may use in inline asm.
#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn int_hold(rounds: u64) -> u64 {
    let bad: u64;
    // SAFETY: reads INT_PATTERN, writes IN_WORK, and writes only registers declared below.
    unsafe {
        core::arch::asm!(
            "mov rax, [rsi + 0]",
            "mov rcx, [rsi + 8]",
            "mov rdx, [rsi + 16]",
            "mov r8, [rsi + 24]",
            "mov r12, [rsi + 32]",
            "mov r13, [rsi + 40]",
            "mov r14, [rsi + 48]",
            "mov r15, [rsi + 56]",
            "mov dword ptr [rdi], 1",
            "2:",
            "mov r10, 256", "cmp rax, [rsi + 0]", "jne 3f",
            "mov r10, 257", "cmp rcx, [rsi + 8]", "jne 3f",
            "mov r10, 258", "cmp rdx, [rsi + 16]", "jne 3f",
            "mov r10, 259", "cmp r8, [rsi + 24]", "jne 3f",
            "mov r10, 260", "cmp r12, [rsi + 32]", "jne 3f",
            "mov r10, 261", "cmp r13, [rsi + 40]", "jne 3f",
            "mov r10, 262", "cmp r14, [rsi + 48]", "jne 3f",
            "mov r10, 263", "cmp r15, [rsi + 56]", "jne 3f",
            "dec r9",
            "jnz 2b",
            "xor r10d, r10d",
            "3:",
            "mov dword ptr [rdi], 0",
            inout("r9") rounds => _,
            in("rsi") INT_PATTERN.as_ptr(),
            in("rdi") IN_WORK.as_ptr(),
            out("r10") bad, out("r11") _,
            out("rax") _, out("rcx") _, out("rdx") _, out("r8") _, out("r12") _, out("r13") _, out("r14") _, out("r15") _,
            options(nostack),
        );
    }
    bad
}

/// As `int_hold`, with all 128 bits of every floating-point/SIMD register, v0-v31.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn fp_hold(rounds: u64) -> u64 {
    let bad: u64;
    // SAFETY: reads FP_PATTERN, writes IN_WORK, and writes only registers declared below.
    unsafe {
        core::arch::asm!(
            "ldr q0, [x10, #0]",
            "ldr q1, [x10, #16]",
            "ldr q2, [x10, #32]",
            "ldr q3, [x10, #48]",
            "ldr q4, [x10, #64]",
            "ldr q5, [x10, #80]",
            "ldr q6, [x10, #96]",
            "ldr q7, [x10, #112]",
            "ldr q8, [x10, #128]",
            "ldr q9, [x10, #144]",
            "ldr q10, [x10, #160]",
            "ldr q11, [x10, #176]",
            "ldr q12, [x10, #192]",
            "ldr q13, [x10, #208]",
            "ldr q14, [x10, #224]",
            "ldr q15, [x10, #240]",
            "ldr q16, [x10, #256]",
            "ldr q17, [x10, #272]",
            "ldr q18, [x10, #288]",
            "ldr q19, [x10, #304]",
            "ldr q20, [x10, #320]",
            "ldr q21, [x10, #336]",
            "ldr q22, [x10, #352]",
            "ldr q23, [x10, #368]",
            "ldr q24, [x10, #384]",
            "ldr q25, [x10, #400]",
            "ldr q26, [x10, #416]",
            "ldr q27, [x10, #432]",
            "ldr q28, [x10, #448]",
            "ldr q29, [x10, #464]",
            "ldr q30, [x10, #480]",
            "ldr q31, [x10, #496]",
            "mov w11, #1", "str w11, [x15]",
            "2:",
            "mov x12, #256", "ldp x13, x14, [x10, #0]", "mov x11, v0.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v0.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #257", "ldp x13, x14, [x10, #16]", "mov x11, v1.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v1.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #258", "ldp x13, x14, [x10, #32]", "mov x11, v2.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v2.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #259", "ldp x13, x14, [x10, #48]", "mov x11, v3.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v3.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #260", "ldp x13, x14, [x10, #64]", "mov x11, v4.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v4.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #261", "ldp x13, x14, [x10, #80]", "mov x11, v5.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v5.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #262", "ldp x13, x14, [x10, #96]", "mov x11, v6.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v6.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #263", "ldp x13, x14, [x10, #112]", "mov x11, v7.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v7.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #264", "ldp x13, x14, [x10, #128]", "mov x11, v8.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v8.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #265", "ldp x13, x14, [x10, #144]", "mov x11, v9.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v9.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #266", "ldp x13, x14, [x10, #160]", "mov x11, v10.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v10.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #267", "ldp x13, x14, [x10, #176]", "mov x11, v11.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v11.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #268", "ldp x13, x14, [x10, #192]", "mov x11, v12.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v12.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #269", "ldp x13, x14, [x10, #208]", "mov x11, v13.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v13.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #270", "ldp x13, x14, [x10, #224]", "mov x11, v14.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v14.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #271", "ldp x13, x14, [x10, #240]", "mov x11, v15.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v15.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #272", "ldp x13, x14, [x10, #256]", "mov x11, v16.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v16.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #273", "ldp x13, x14, [x10, #272]", "mov x11, v17.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v17.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #274", "ldp x13, x14, [x10, #288]", "mov x11, v18.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v18.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #275", "ldp x13, x14, [x10, #304]", "mov x11, v19.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v19.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #276", "ldp x13, x14, [x10, #320]", "mov x11, v20.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v20.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #277", "ldp x13, x14, [x10, #336]", "mov x11, v21.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v21.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #278", "ldp x13, x14, [x10, #352]", "mov x11, v22.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v22.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #279", "ldp x13, x14, [x10, #368]", "mov x11, v23.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v23.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #280", "ldp x13, x14, [x10, #384]", "mov x11, v24.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v24.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #281", "ldp x13, x14, [x10, #400]", "mov x11, v25.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v25.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #282", "ldp x13, x14, [x10, #416]", "mov x11, v26.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v26.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #283", "ldp x13, x14, [x10, #432]", "mov x11, v27.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v27.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #284", "ldp x13, x14, [x10, #448]", "mov x11, v28.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v28.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #285", "ldp x13, x14, [x10, #464]", "mov x11, v29.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v29.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #286", "ldp x13, x14, [x10, #480]", "mov x11, v30.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v30.d[1]", "cmp x11, x14", "b.ne 3f",
            "mov x12, #287", "ldp x13, x14, [x10, #496]", "mov x11, v31.d[0]", "cmp x11, x13", "b.ne 3f", "mov x11, v31.d[1]", "cmp x11, x14", "b.ne 3f",
            "subs x9, x9, #1",
            "b.ne 2b",
            "mov x12, #0",
            "3:",
            "str wzr, [x15]",
            inout("x9") rounds => _,
            in("x10") FP_PATTERN.as_ptr(),
            in("x15") IN_WORK.as_ptr(),
            out("x11") _, out("x12") bad, out("x13") _, out("x14") _,
            out("v0") _, out("v1") _, out("v2") _, out("v3") _, out("v4") _, out("v5") _, out("v6") _, out("v7") _, out("v8") _, out("v9") _, out("v10") _, out("v11") _, out("v12") _, out("v13") _, out("v14") _, out("v15") _, out("v16") _, out("v17") _, out("v18") _, out("v19") _, out("v20") _, out("v21") _, out("v22") _, out("v23") _, out("v24") _, out("v25") _, out("v26") _, out("v27") _, out("v28") _, out("v29") _, out("v30") _, out("v31") _,
            options(nostack),
        );
    }
    bad
}

/// As on ARM64, with all 128 bits of xmm0-xmm15. The userspace target is built without
/// SSE, so the compiler never uses these registers itself; the instructions run on the
/// processor's SSE unit, as they do in any program built for the standard x86-64 ABI.
/// Each register is copied to `seen` to be compared a half at a time.
#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn fp_hold(rounds: u64) -> u64 {
    let bad: u64;
    let mut seen = [0u64; 2];
    // SAFETY: reads FP_PATTERN, writes IN_WORK and `seen`, and writes only the xmm
    // registers, which compiled code here never uses, and the registers declared below.
    unsafe {
        core::arch::asm!(
            "movdqu xmm0, [rsi + 0]",
            "movdqu xmm1, [rsi + 16]",
            "movdqu xmm2, [rsi + 32]",
            "movdqu xmm3, [rsi + 48]",
            "movdqu xmm4, [rsi + 64]",
            "movdqu xmm5, [rsi + 80]",
            "movdqu xmm6, [rsi + 96]",
            "movdqu xmm7, [rsi + 112]",
            "movdqu xmm8, [rsi + 128]",
            "movdqu xmm9, [rsi + 144]",
            "movdqu xmm10, [rsi + 160]",
            "movdqu xmm11, [rsi + 176]",
            "movdqu xmm12, [rsi + 192]",
            "movdqu xmm13, [rsi + 208]",
            "movdqu xmm14, [rsi + 224]",
            "movdqu xmm15, [rsi + 240]",
            "mov dword ptr [rdi], 1",
            "2:",
            "mov r10, 256", "movdqu [rdx], xmm0", "mov r11, [rdx]", "cmp r11, [rsi + 0]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 8]", "jne 3f",
            "mov r10, 257", "movdqu [rdx], xmm1", "mov r11, [rdx]", "cmp r11, [rsi + 16]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 24]", "jne 3f",
            "mov r10, 258", "movdqu [rdx], xmm2", "mov r11, [rdx]", "cmp r11, [rsi + 32]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 40]", "jne 3f",
            "mov r10, 259", "movdqu [rdx], xmm3", "mov r11, [rdx]", "cmp r11, [rsi + 48]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 56]", "jne 3f",
            "mov r10, 260", "movdqu [rdx], xmm4", "mov r11, [rdx]", "cmp r11, [rsi + 64]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 72]", "jne 3f",
            "mov r10, 261", "movdqu [rdx], xmm5", "mov r11, [rdx]", "cmp r11, [rsi + 80]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 88]", "jne 3f",
            "mov r10, 262", "movdqu [rdx], xmm6", "mov r11, [rdx]", "cmp r11, [rsi + 96]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 104]", "jne 3f",
            "mov r10, 263", "movdqu [rdx], xmm7", "mov r11, [rdx]", "cmp r11, [rsi + 112]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 120]", "jne 3f",
            "mov r10, 264", "movdqu [rdx], xmm8", "mov r11, [rdx]", "cmp r11, [rsi + 128]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 136]", "jne 3f",
            "mov r10, 265", "movdqu [rdx], xmm9", "mov r11, [rdx]", "cmp r11, [rsi + 144]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 152]", "jne 3f",
            "mov r10, 266", "movdqu [rdx], xmm10", "mov r11, [rdx]", "cmp r11, [rsi + 160]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 168]", "jne 3f",
            "mov r10, 267", "movdqu [rdx], xmm11", "mov r11, [rdx]", "cmp r11, [rsi + 176]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 184]", "jne 3f",
            "mov r10, 268", "movdqu [rdx], xmm12", "mov r11, [rdx]", "cmp r11, [rsi + 192]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 200]", "jne 3f",
            "mov r10, 269", "movdqu [rdx], xmm13", "mov r11, [rdx]", "cmp r11, [rsi + 208]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 216]", "jne 3f",
            "mov r10, 270", "movdqu [rdx], xmm14", "mov r11, [rdx]", "cmp r11, [rsi + 224]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 232]", "jne 3f",
            "mov r10, 271", "movdqu [rdx], xmm15", "mov r11, [rdx]", "cmp r11, [rsi + 240]", "jne 3f", "mov r11, [rdx + 8]", "cmp r11, [rsi + 248]", "jne 3f",
            "dec r9",
            "jnz 2b",
            "xor r10d, r10d",
            "3:",
            "mov dword ptr [rdi], 0",
            inout("r9") rounds => _,
            in("rsi") FP_PATTERN.as_ptr(),
            in("rdi") IN_WORK.as_ptr(),
            in("rdx") seen.as_mut_ptr(),
            out("r10") bad, out("r11") _,
            options(nostack),
        );
    }
    bad
}

/// Overwrite every floating-point/SIMD register the calling convention lets a function
/// change, as a handler doing floating-point work would.
fn clobber_fp() {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: only the declared registers are written.
    unsafe {
        core::arch::asm!(
            "movi v0.16b, #0xa5",
            "movi v1.16b, #0xa5",
            "movi v2.16b, #0xa5",
            "movi v3.16b, #0xa5",
            "movi v4.16b, #0xa5",
            "movi v5.16b, #0xa5",
            "movi v6.16b, #0xa5",
            "movi v7.16b, #0xa5",
            "movi v8.16b, #0xa5",
            "movi v9.16b, #0xa5",
            "movi v10.16b, #0xa5",
            "movi v11.16b, #0xa5",
            "movi v12.16b, #0xa5",
            "movi v13.16b, #0xa5",
            "movi v14.16b, #0xa5",
            "movi v15.16b, #0xa5",
            "movi v16.16b, #0xa5",
            "movi v17.16b, #0xa5",
            "movi v18.16b, #0xa5",
            "movi v19.16b, #0xa5",
            "movi v20.16b, #0xa5",
            "movi v21.16b, #0xa5",
            "movi v22.16b, #0xa5",
            "movi v23.16b, #0xa5",
            "movi v24.16b, #0xa5",
            "movi v25.16b, #0xa5",
            "movi v26.16b, #0xa5",
            "movi v27.16b, #0xa5",
            "movi v28.16b, #0xa5",
            "movi v29.16b, #0xa5",
            "movi v30.16b, #0xa5",
            "movi v31.16b, #0xa5",
            out("v0") _, out("v1") _, out("v2") _, out("v3") _, out("v4") _, out("v5") _, out("v6") _, out("v7") _, out("v8") _, out("v9") _, out("v10") _, out("v11") _, out("v12") _, out("v13") _, out("v14") _, out("v15") _, out("v16") _, out("v17") _, out("v18") _, out("v19") _, out("v20") _, out("v21") _, out("v22") _, out("v23") _, out("v24") _, out("v25") _, out("v26") _, out("v27") _, out("v28") _, out("v29") _, out("v30") _, out("v31") _,
        );
    }
    #[cfg(target_arch = "x86_64")]
    // SAFETY: only xmm registers are written, which compiled code here never uses.
    unsafe {
        core::arch::asm!(
            "pcmpeqd xmm0, xmm0",
            "pcmpeqd xmm1, xmm1",
            "pcmpeqd xmm2, xmm2",
            "pcmpeqd xmm3, xmm3",
            "pcmpeqd xmm4, xmm4",
            "pcmpeqd xmm5, xmm5",
            "pcmpeqd xmm6, xmm6",
            "pcmpeqd xmm7, xmm7",
            "pcmpeqd xmm8, xmm8",
            "pcmpeqd xmm9, xmm9",
            "pcmpeqd xmm10, xmm10",
            "pcmpeqd xmm11, xmm11",
            "pcmpeqd xmm12, xmm12",
            "pcmpeqd xmm13, xmm13",
            "pcmpeqd xmm14, xmm14",
            "pcmpeqd xmm15, xmm15",
        );
    }
}

/// Set by `int_hold` and `fp_hold` while their registers hold the known values; a handler
/// that sees it counts itself in MID.
static IN_WORK: AtomicU32 = AtomicU32::new(0);
static MID: AtomicU32 = AtomicU32::new(0);
/// Handlers that must have interrupted a register check for its case to pass.
const MID_WANTED: u32 = 10;

extern "C" fn on_clobber(sig: i32) {
    if IN_WORK.load(Ordering::SeqCst) == 1 { MID.fetch_add(1, Ordering::SeqCst); }
    record(sig);
    core::hint::black_box(checksum(core::hint::black_box(64)));
    clobber_fp();
}

/// Run `hold` again and again while a child sends SIGUSR1 to this process every
/// millisecond, until MID_WANTED handlers have interrupted it while its registers were
/// live or `ms` have passed; returns the first nonzero result, and how many handlers
/// interrupted it.
fn under_signals(ms: u64, hold: impl Fn() -> u64) -> Result<(u64, u32), CaseError> {
    catch_with(SIGUSR1, on_clobber as usize as u64, 0, 0)?;
    let shared = Shared::new()?;
    let me = pid();
    let mut sender = Child::start(|| {
        while shared.get(0) == 0 {
            kill(me, SIGUSR1);
            let _ = time::sleep_ms(1);
        }
        0
    })?;
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    let mut wrong = 0;
    while MID.load(Ordering::SeqCst) < MID_WANTED && now_ms().saturating_sub(start) < ms {
        wrong = hold();
        if wrong != 0 { break; }
    }
    shared.set(0, 1);
    sender.expect_exit(0, "the signalling child")?;
    Ok((wrong, MID.load(Ordering::SeqCst)))
}

/// A round count for `hold` that takes at least 20 ms, longer than a timer tick on either
/// architecture, so a pending signal is taken by an interrupt in the middle of a run.
/// Each size is timed three times and the shortest counts, since time spent descheduled
/// only lengthens a run.
fn rounds_for(hold: impl Fn(u64) -> u64) -> u64 {
    let mut rounds = 10_000u64;
    while rounds < 1 << 30 {
        let fastest = (0..3).map(|_| {
            let start = now_ms();
            core::hint::black_box(hold(core::hint::black_box(rounds)));
            now_ms().saturating_sub(start)
        }).min().unwrap_or(0);
        if fastest >= 20 { break; }
        rounds *= 2;
    }
    rounds
}

fn h_registers() -> CaseResult {
    let rounds = rounds_for(int_hold);
    let (wrong, mid) = under_signals(5000, || int_hold(core::hint::black_box(rounds)))?;
    if wrong != 0 {
        return fail(format!("after handlers interrupted it, general register {} of the check no longer held its value", wrong - 0x100));
    }
    check(mid >= MID_WANTED, &format!("only {mid} handlers interrupted the register checks while the registers were live; the case needs {MID_WANTED}"))
}

fn h_fp_registers() -> CaseResult {
    let rounds = rounds_for(fp_hold);
    let (wrong, mid) = under_signals(5000, || fp_hold(core::hint::black_box(rounds)))?;
    if wrong != 0 {
        return fail(format!("after handlers that overwrite the floating-point registers, register {} no longer held all 128 bits of its value", wrong - 0x100));
    }
    check(mid >= MID_WANTED, &format!("only {mid} handlers interrupted the register checks while the registers were live; the case needs {MID_WANTED}"))
}

static SHARED_AT: AtomicUsize = AtomicUsize::new(0);

/// Slot 1 of the shared page set up with SHARED_AT: when a handler last ran (ms, at least 1).
extern "C" fn on_mark(sig: i32) {
    let page = SHARED_AT.load(Ordering::SeqCst);
    if page != 0 {
        // SAFETY: SHARED_AT holds the address of a mapped shared page.
        let slot = |i: usize| unsafe { &*((page + i * SHARED_SLOT_BYTES) as *const AtomicU64) };
        slot(1).store(now_ms().max(1), Ordering::SeqCst);
        slot(3).fetch_add(1, Ordering::SeqCst);
    }
    record(sig);
}

fn shared_for_handlers() -> Result<Shared, CaseError> {
    let shared = Shared::new()?;
    SHARED_AT.store(shared.page as usize, Ordering::SeqCst);
    Ok(shared)
}

fn h_spinning_target() -> CaseResult {
    let cpus = processors()?;
    if cpus < 2 {
        return skip(format!("{cpus} processor online; the case needs 2, so the target spins on one while the sender runs on another"));
    }
    let s = shared_for_handlers()?;
    let mut kid = Child::start(|| {
        if catch_with(SIGUSR1, on_mark as usize as u64, 0, 0).is_err() { return 10; }
        s.set(0, 1);
        // No system calls: only an interrupt can bring the signal in.
        while s.get(1) == 0 && s.get(2) == 0 {
            answer_handoff(s.slot(4));
            core::hint::spin_loop();
        }
        0
    })?;
    check(until(WAIT_MS, || s.get(0) == 1), "the target never started spinning")?;
    if let Err(why) = handoffs_show_parallel(s.slot(4)) {
        s.set(2, 1);
        let _ = kid.wait();
        return fail(format!("with {cpus} processors online, the target and this process: {why}"));
    }
    want_eq("kill(SIGUSR1)", kill(kid.pid, SIGUSR1), 0)?;
    let handled = until(WAIT_MS, || s.get(1) != 0);
    s.set(2, 1);
    kid.expect_exit(0, "the spinning target")?;
    check(handled, &format!("with {cpus} processors online, a process spinning in user mode on another processor ran no handler within {WAIT_MS} ms of being sent SIGUSR1"))
}

// ---------------------------------------------------------------------------
// sigsuspend, pause & sigtimedwait

/// The signal a watchdog uses to end a wait that would otherwise never end.
const DOG: i32 = SIGHUP;

/// Fail if the watchdog's signal was handled: then the wait ended only because it
/// arrived, even if the awaited signal's handler ran alongside it.
fn dog_quiet(wait: &str) -> CaseResult {
    check(count(DOG) == 0, &format!("{wait} ended only when the watchdog's later signal arrived"))
}

fn w_sigsuspend() -> CaseResult {
    catch(SIGUSR1)?;
    catch(DOG)?;
    setmask(bit(SIGUSR1))?;
    let mut kid = signal_when_parked(SIGUSR1)?;
    let _dog = watchdog(4000, DOG)?;
    let got = sigsuspend(0);
    dog_quiet("sigsuspend")?;
    check(count(SIGUSR1) == 1, "sigsuspend did not return when SIGUSR1 arrived; only the watchdog's later signal ended it")?;
    want_err("sigsuspend", got, EINTR)?;
    let after = mask_now()?;
    check(after == bit(SIGUSR1), &format!("after sigsuspend the mask is {after:#x}, expected the one it replaced ({:#x})", bit(SIGUSR1)))?;
    kid.expect_exit(0, "the signalling child")
}

fn w_sigsuspend_pending() -> CaseResult {
    catch(SIGUSR1)?;
    catch(DOG)?;
    setmask(bit(SIGUSR1))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    let _dog = watchdog(3000, DOG)?;
    let start = now_ms();
    let got = sigsuspend(0);
    let took = now_ms().saturating_sub(start);
    dog_quiet("sigsuspend")?;
    check(count(SIGUSR1) == 1, "with SIGUSR1 already pending, sigsuspend(empty mask) did not run its handler; only the watchdog's signal ended the wait")?;
    want_err("sigsuspend", got, EINTR)?;
    check(took < 1000, &format!("with SIGUSR1 already pending, sigsuspend took {took} ms to return"))
}

/// A child that, once this process blocks, sends `first`, waits QUIET_MS, then sets
/// slot 0 of `s` and sends SIGUSR1. A wait that has returned while slot 0 is still 0
/// was ended by `first`.
fn first_then_usr1(s: &Shared, first: i32) -> Result<Child, CaseError> {
    let me = pid();
    Child::start(move || {
        if !parked(me, WAIT_MS) || kill(me, first) != 0 { return 10; }
        let _ = time::sleep_ms(QUIET_MS);
        s.set(0, 1);
        if kill(me, SIGUSR1) != 0 { return 11; }
        0
    })
}

fn w_temp_mask() -> CaseResult {
    catch(SIGUSR1)?;
    catch(SIGUSR2)?;
    catch(DOG)?;
    setmask(bit(SIGUSR1))?;
    let s = Shared::new()?;
    let mut kid = first_then_usr1(&s, SIGUSR2)?;
    let _dog = watchdog(4000, DOG)?;
    let got = sigsuspend(bit(SIGUSR2));
    let usr1_sent = s.get(0) == 1;
    let _ = until(200, || count(SIGUSR2) > 0);
    kid.expect_exit(0, "the signalling child")?;
    dog_quiet("sigsuspend")?;
    check(usr1_sent, "SIGUSR2, blocked by sigsuspend's temporary mask, ended the wait before SIGUSR1 was sent")?;
    want_err("sigsuspend", got, EINTR)?;
    check(count(SIGUSR1) == 1, "sigsuspend did not return when SIGUSR1 arrived; only the watchdog's later signal ended it")?;
    let seen = order();
    check(seen.first() == Some(&SIGUSR1),
        &format!("handlers ran in the order {seen:?}: SIGUSR2 ran before SIGUSR1's handler, which should run under the temporary mask that blocks SIGUSR2"))?;
    check(count(SIGUSR2) == 1, "SIGUSR2, held pending by the temporary mask, was not delivered once sigsuspend restored the old mask")
}

fn w_terminates() -> CaseResult {
    let mut kid = Child::start(|| {
        if default(SIGTERM).is_err() { return 10; }
        sigsuspend(0);
        0
    })?;
    check(parked(kid.pid, WAIT_MS), "the child never blocked in sigsuspend")?;
    want_eq("kill(SIGTERM)", kill(kid.pid, SIGTERM), 0)?;
    kid.expect_death(SIGTERM, "a child in sigsuspend sent SIGTERM with the default action")
}

fn w_ignored() -> CaseResult {
    catch(SIGUSR1)?;
    ignore(SIGUSR2)?;
    catch(DOG)?;
    setmask(bit(SIGUSR1))?;
    let s = Shared::new()?;
    let mut kid = first_then_usr1(&s, SIGUSR2)?;
    let _dog = watchdog(4000, DOG)?;
    let got = sigsuspend(0);
    let usr1_sent = s.get(0) == 1;
    kid.expect_exit(0, "the signalling child")?;
    dog_quiet("sigsuspend")?;
    check(usr1_sent, "an ignored SIGUSR2 ended sigsuspend before SIGUSR1 was sent")?;
    check(count(SIGUSR1) == 1, "sigsuspend did not return when SIGUSR1 arrived")?;
    want_err("sigsuspend", got, EINTR)
}

fn w_pause() -> CaseResult {
    catch(SIGUSR1)?;
    catch(DOG)?;
    setmask(0)?;
    let mut kid = signal_when_parked(SIGUSR1)?;
    let _dog = watchdog(4000, DOG)?;
    let got = pause();
    dog_quiet("pause")?;
    check(count(SIGUSR1) == 1, "pause did not return when SIGUSR1 arrived; only the watchdog's later signal ended it")?;
    want_err("pause", got, EINTR)?;
    kid.expect_exit(0, "the signalling child")
}

fn w_signal_first() -> CaseResult {
    catch(SIGUSR1)?;
    catch(DOG)?;
    let old = setmask(bit(SIGUSR1))?;
    let me = pid();
    let mut kid = Child::start(move || if kill(me, SIGUSR1) == 0 { 0 } else { 1 })?;
    // The child has signalled before this process reaches its wait.
    kid.expect_exit(0, "the signalling child")?;
    let _dog = watchdog(3000, DOG)?;
    let start = now_ms();
    let got = sigsuspend(old & !bit(SIGUSR1));
    let took = now_ms().saturating_sub(start);
    dog_quiet("sigsuspend")?;
    check(count(SIGUSR1) == 1, "a SIGUSR1 sent by a child before the parent's sigsuspend was lost")?;
    want_err("sigsuspend", got, EINTR)?;
    check(took < 1000, &format!("sigsuspend took {took} ms to return for a signal already pending"))
}

fn w_race_loop() -> CaseResult {
    const ROUNDS: u64 = 100;
    const DOG_MS: u64 = 6000;
    catch(SIGUSR1)?;
    catch(DOG)?;
    setmask(bit(SIGUSR1))?;
    let me = pid();
    // Slot 0 holds the round this process has asked for; the sender, a child that never
    // makes a system call while it watches, sends SIGUSR1 as soon as the slot changes,
    // racing this process's way into sigsuspend. Slot 1 counts what it has sent.
    let s = Shared::new()?;
    let mut sender = Child::start(|| {
        let mut sent = 0;
        loop {
            let round = s.get(0);
            if round == u64::MAX { return 0; }
            if round > sent {
                if kill(me, SIGUSR1) != 0 { return 10; }
                sent = round;
                s.set(1, sent);
            }
            core::hint::spin_loop();
        }
    })?;
    let _dog = watchdog(DOG_MS, DOG)?;
    let mut lost = None;
    for round in 1..=ROUNDS {
        s.set(0, round);
        let got = sigsuspend(0);
        if count(DOG) != 0 || count(SIGUSR1) as u64 != round || got != -EINTR {
            lost = Some((round, got));
            break;
        }
    }
    s.set(0, u64::MAX);
    sender.expect_exit(0, "the signalling child")?;
    if let Some((round, got)) = lost {
        dog_quiet(&format!("round {round} of {ROUNDS}: sigsuspend"))?;
        check(count(SIGUSR1) as u64 == round,
            &format!("round {round} of {ROUNDS}: a child's SIGUSR1 racing the parent's mask-then-sigsuspend was lost"))?;
        return want_err(&format!("round {round} of {ROUNDS}: sigsuspend"), got, EINTR);
    }
    Ok(())
}

fn w_timedwait_pending() -> CaseResult {
    catch(SIGUSR1)?;
    setmask(bit(SIGUSR1))?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    let mut info = SigInfo::zero();
    let got = sigtimedwait(bit(SIGUSR1), &mut info, Some(1000));
    want_eq("sigtimedwait for a pending SIGUSR1", got, SIGUSR1 as i64)?;
    check(info.signo == SIGUSR1 && info.code == SI_USER && info.pid() == pid(),
        &format!("sigtimedwait's siginfo has si_signo {}, si_code {}, si_pid {}", info.signo, info.code, info.pid()))?;
    check(pending()? & bit(SIGUSR1) == 0, "SIGUSR1 is still pending after sigtimedwait accepted it")?;
    check(count(SIGUSR1) == 0, "the handler ran for a signal sigtimedwait accepted")
}

fn w_timedwait_timeout() -> CaseResult {
    setmask(bit(SIGUSR1))?;
    let mut info = SigInfo::zero();
    let start = now_ms();
    let got = sigtimedwait(bit(SIGUSR1), &mut info, Some(100));
    let took = now_ms().saturating_sub(start);
    want_err("sigtimedwait with nothing pending and a 100 ms timeout", got, EAGAIN)?;
    check((90..1000).contains(&took), &format!("a 100 ms sigtimedwait returned after {took} ms"))
}

fn w_timedwait_poll() -> CaseResult {
    setmask(bit(SIGUSR1))?;
    let mut info = SigInfo::zero();
    let start = now_ms();
    let got = sigtimedwait(bit(SIGUSR1), &mut info, Some(0));
    let took = now_ms().saturating_sub(start);
    want_err("sigtimedwait with a zero timeout and nothing pending", got, EAGAIN)?;
    check(took < 100, &format!("a zero-timeout sigtimedwait took {took} ms"))
}

fn w_timedwait_einval() -> CaseResult {
    setmask(bit(SIGUSR1))?;
    let set = [bit(SIGUSR1)];
    let mut info = SigInfo::zero();
    for ts in [[0i64, 1_000_000_000], [0, -1]] {
        want_err(&format!("sigtimedwait with tv_nsec {}", ts[1]), sc(nr::RT_SIGTIMEDWAIT, &[
            set.as_ptr() as u64, &mut info as *mut SigInfo as u64, ts.as_ptr() as u64, 8,
        ]), EINVAL)?;
    }
    Ok(())
}

fn w_sigwaitinfo() -> CaseResult {
    catch(DOG)?;
    setmask(bit(SIGUSR1))?;
    let mut kid = signal_when_parked(SIGUSR1)?;
    let _dog = watchdog(4000, DOG)?;
    let mut info = SigInfo::zero();
    let got = sigtimedwait(bit(SIGUSR1), &mut info, None);
    want_eq("sigwaitinfo for a SIGUSR1 sent by a child", got, SIGUSR1 as i64)?;
    check(info.pid() == kid.pid, &format!("sigwaitinfo's si_pid is {}, expected the sender {}", info.pid(), kid.pid))?;
    kid.expect_exit(0, "the signalling child")
}

fn w_timedwait_eintr() -> CaseResult {
    catch(SIGUSR2)?;
    setmask(bit(SIGUSR1))?;
    let mut kid = signal_when_parked(SIGUSR2)?;
    let mut info = SigInfo::zero();
    let got = sigtimedwait(bit(SIGUSR1), &mut info, Some(3000));
    want_err("sigtimedwait for SIGUSR1 interrupted by a caught SIGUSR2", got, EINTR)?;
    check(count(SIGUSR2) == 1, "the SIGUSR2 handler did not run")?;
    kid.expect_exit(0, "the signalling child")
}

/// Wait up to `ms` for `sig`'s handler to run; how long after `start` it ran.
fn handled_after(sig: i32, start: u64, ms: u64) -> Option<u64> {
    until(ms, || count(sig) > 0).then(|| SEEN_AT.load(Ordering::SeqCst).saturating_sub(start))
}

fn w_alarm() -> CaseResult {
    catch(SIGALRM)?;
    let start = now_ms();
    want_eq("alarm(1) with no alarm set", alarm(1), 0)?;
    let took = handled_after(SIGALRM, start, 3000).ok_or("alarm(1) delivered no SIGALRM within 3 s")?;
    check((900..=1600).contains(&took), &format!("alarm(1) delivered SIGALRM after {took} ms"))
}

fn w_alarm_default() -> CaseResult {
    let mut kid = Child::start(|| {
        if default(SIGALRM).is_err() || alarm(1) < 0 { return 10; }
        loop { pause(); }
    })?;
    kid.expect_death(SIGALRM, "a child waiting for alarm(1) with SIGALRM at its default action")
}

fn w_alarm_cancel() -> CaseResult {
    catch(SIGALRM)?;
    want_eq("alarm(5)", alarm(5), 0)?;
    let left = alarm(0);
    check(left == 4 || left == 5, &format!("alarm(0) after alarm(5) returned {}, expected the 5 seconds left", shown(left)))?;
    let cur = getitimer(ITIMER_REAL)?;
    check(cur == [0; 4], &format!("after alarm(0), getitimer(ITIMER_REAL) reports {cur:?}"))?;
    want_eq("alarm(1)", alarm(1), 0)?;
    want_eq("alarm(0)", alarm(0), 1)?;
    let _ = until(1300, || count(SIGALRM) > 0);
    check(count(SIGALRM) == 0, "a cancelled alarm still delivered SIGALRM")
}

fn w_setitimer() -> CaseResult {
    catch(SIGALRM)?;
    let start = now_ms();
    want_eq("setitimer(ITIMER_REAL, 200 ms)", setitimer(ITIMER_REAL, &itimer(0, 200_000), None), 0)?;
    let cur = getitimer(ITIMER_REAL)?;
    let left = us((cur[2], cur[3]));
    check(left > 0 && left <= 200_000 && us((cur[0], cur[1])) == 0,
        &format!("just after setting it, getitimer reports {left} us left, interval {} us", us((cur[0], cur[1]))))?;
    let took = handled_after(SIGALRM, start, 2000).ok_or("a 200 ms ITIMER_REAL delivered no SIGALRM within 2 s")?;
    check((190..=700).contains(&took), &format!("a 200 ms ITIMER_REAL delivered SIGALRM after {took} ms"))?;
    let after = getitimer(ITIMER_REAL)?;
    check(after == [0; 4], &format!("an expired one-shot ITIMER_REAL still reports {after:?}"))
}

/// A periodic timer reloads after each expiry, so it keeps delivering; expiries the
/// process misses while it is not running coalesce into one pending SIGALRM, so only an
/// upper bound on the count follows from the time elapsed.
fn w_interval() -> CaseResult {
    const WANT: u32 = 3;
    catch(SIGALRM)?;
    let start = now_ms();
    want_eq("setitimer(ITIMER_REAL, every 50 ms)", setitimer(ITIMER_REAL, &itimer(50_000, 50_000), None), 0)?;
    let reloaded = until(WAIT_MS, || count(SIGALRM) >= WANT);
    let n = count(SIGALRM);
    let cur = getitimer(ITIMER_REAL)?;
    let took = now_ms().saturating_sub(start);
    want_eq("disarming ITIMER_REAL", setitimer(ITIMER_REAL, &itimer(0, 0), None), 0)?;
    check(reloaded, &format!("a 50 ms periodic ITIMER_REAL delivered {n} SIGALRMs in {took} ms, expected it to keep firing"))?;
    check(n as u64 <= took / 50 + 1, &format!("a 50 ms periodic ITIMER_REAL delivered {n} SIGALRMs in {took} ms, more than it can expire"))?;
    check(us((cur[0], cur[1])) == 50_000, &format!("while it runs, getitimer reports an interval of {} us, expected 50000", us((cur[0], cur[1]))))
}

fn w_itimer_old() -> CaseResult {
    let mut old = [9i64; 4];
    want_eq("setitimer(ITIMER_REAL, 5 s)", setitimer(ITIMER_REAL, &itimer(1_000_000, 5_000_000), Some(&mut old)), 0)?;
    check(old == [0; 4], &format!("the first setitimer reported an old value of {old:?}"))?;
    want_eq("setitimer(ITIMER_REAL, 0)", setitimer(ITIMER_REAL, &itimer(0, 0), Some(&mut old)), 0)?;
    let left = us((old[2], old[3]));
    check(left > 4_000_000 && left <= 5_000_000 && us((old[0], old[1])) == 1_000_000,
        &format!("disarming returned an old value of {left} us left and interval {} us", us((old[0], old[1]))))?;
    let cur = getitimer(ITIMER_REAL)?;
    check(cur == [0; 4], &format!("a disarmed ITIMER_REAL reports {cur:?}"))
}

fn w_itimer_einval() -> CaseResult {
    let mut cur = [0i64; 4];
    want_err("getitimer(99)", sc(nr::GETITIMER, &[99, cur.as_mut_ptr() as u64]), EINVAL)?;
    want_err("setitimer(99)", setitimer(99, &itimer(0, 100_000), None), EINVAL)?;
    want_err("setitimer with tv_usec 1000000", setitimer(ITIMER_REAL, &[0, 0, 0, 1_000_000], None), EINVAL)?;
    want_err("setitimer with tv_usec -1", setitimer(ITIMER_REAL, &[0, 0, 0, -1], None), EINVAL)
}

/// An ITIMER_VIRTUAL or ITIMER_PROF timer of 100 ms counts only the process's CPU time:
/// it does not expire while the process sleeps for longer than that, and delivers `sig`
/// once the process has computed for its interval.
fn cpu_timer(which: i32, sig: i32, what: &str) -> CaseResult {
    const SLEEP_MS: u64 = 300;
    catch(sig)?;
    want_eq(&format!("setitimer({what}, 100 ms)"), setitimer(which, &itimer(0, 100_000), None), 0)?;
    let _ = time::sleep_ms(SLEEP_MS);
    check(count(sig) == 0, &format!("a 100 ms {what} delivered {} while the process slept for {SLEEP_MS} ms: it counts wall-clock time, not CPU time", name(sig)))?;
    let left = getitimer(which)?;
    let left_us = us((left[2], left[3]));
    check(left_us > 50_000, &format!("after the process slept for {SLEEP_MS} ms, a 100 ms {what} reports {left_us} us left"))?;
    let start = now_ms();
    let ms = bounded(3000, CLEANUP_MS);
    while count(sig) == 0 && now_ms().saturating_sub(start) < ms { burn(10); }
    let took = now_ms().saturating_sub(start);
    check(count(sig) == 1, &format!("a 100 ms {what} delivered {} within {took} ms of computation", name(sig)))?;
    check(took >= 45, &format!("a 100 ms {what}, with {left_us} us left, expired after {took} ms of computation"))
}

fn w_virtual() -> CaseResult { cpu_timer(ITIMER_VIRTUAL, SIGVTALRM, "ITIMER_VIRTUAL") }
fn w_prof() -> CaseResult { cpu_timer(ITIMER_PROF, SIGPROF, "ITIMER_PROF") }

// ---------------------------------------------------------------------------
// alternate signal stacks

const ALT_SIZE: usize = 65536;

/// The bounds of the alternate stack a case installed, for handlers to check against.
static ALT_LO: AtomicUsize = AtomicUsize::new(0);
static ALT_HI: AtomicUsize = AtomicUsize::new(0);
/// What a handler running on the alternate stack saw: sigaltstack's ss_flags, and the
/// returns of its attempts to change the stack.
static SEEN_SS_FLAGS: AtomicI32 = AtomicI32::new(-1);
static CHANGE_RET: AtomicI64 = AtomicI64::new(0);
static DISABLE_RET: AtomicI64 = AtomicI64::new(0);
static OUTER_SP: AtomicUsize = AtomicUsize::new(0);

fn on_alt(sp: usize) -> bool { (ALT_LO.load(Ordering::SeqCst)..ALT_HI.load(Ordering::SeqCst)).contains(&sp) }

/// Map and install an alternate stack of ALT_SIZE bytes.
fn install_alt() -> Result<StackT, CaseError> {
    let base = memory::mmap(core::ptr::null_mut(), ALT_SIZE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)?;
    ALT_LO.store(base as usize, Ordering::SeqCst);
    ALT_HI.store(base as usize + ALT_SIZE, Ordering::SeqCst);
    let stack = StackT { ss_sp: base as u64, ss_flags: 0, _pad: 0, ss_size: ALT_SIZE };
    want_eq("sigaltstack", sigaltstack(Some(&stack), None), 0)?;
    Ok(stack)
}

fn query_alt() -> Result<StackT, CaseError> {
    let mut old = StackT { ss_sp: 1, ss_flags: -1, _pad: 0, ss_size: 1 };
    want_eq("sigaltstack(NULL, &old)", sigaltstack(None, Some(&mut old)), 0)?;
    Ok(old)
}

extern "C" fn on_alt_query(sig: i32) {
    let mut old = StackT { ss_sp: 0, ss_flags: -1, _pad: 0, ss_size: 0 };
    sigaltstack(None, Some(&mut old));
    SEEN_SS_FLAGS.store(old.ss_flags, Ordering::SeqCst);
    record(sig);
}

fn a_initial() -> CaseResult {
    let old = query_alt()?;
    check(old.ss_flags == SS_DISABLE, &format!("a new process's alternate stack has ss_flags {}, expected SS_DISABLE", old.ss_flags))
}

fn a_set() -> CaseResult {
    let stack = install_alt()?;
    let old = query_alt()?;
    check(old.ss_sp == stack.ss_sp && old.ss_size == stack.ss_size && old.ss_flags == 0,
        &format!("sigaltstack reads back sp {:#x} size {} flags {}, expected sp {:#x} size {} flags 0",
            old.ss_sp, old.ss_size, old.ss_flags, stack.ss_sp, stack.ss_size))
}

fn a_enomem() -> CaseResult {
    let base = memory::mmap(core::ptr::null_mut(), 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)?;
    let small = StackT { ss_sp: base as u64, ss_flags: 0, _pad: 0, ss_size: 1024 };
    want_err("sigaltstack with a 1 KiB stack, below MINSIGSTKSZ", sigaltstack(Some(&small), None), ENOMEM)?;
    check(query_alt()?.ss_flags == SS_DISABLE, "a refused sigaltstack installed a stack anyway")
}

fn a_einval() -> CaseResult {
    let base = memory::mmap(core::ptr::null_mut(), ALT_SIZE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)?;
    let bad = StackT { ss_sp: base as u64, ss_flags: 0x10, _pad: 0, ss_size: ALT_SIZE };
    want_err("sigaltstack with an undefined ss_flags bit", sigaltstack(Some(&bad), None), EINVAL)
}

fn a_runs_on() -> CaseResult {
    install_alt()?;
    catch_with(SIGUSR1, on_alt_query as usize as u64, SA_ONSTACK, 0)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the SA_ONSTACK handler did not run")?;
    let sp = SEEN_SP.load(Ordering::SeqCst);
    check(on_alt(sp), &format!("an SA_ONSTACK handler ran with a local at {sp:#x}, outside the alternate stack"))?;
    let flags = SEEN_SS_FLAGS.load(Ordering::SeqCst);
    check(flags == SS_ONSTACK, &format!("inside the handler sigaltstack reports ss_flags {flags}, expected SS_ONSTACK"))?;
    let after = query_alt()?.ss_flags;
    check(after == 0, &format!("after the handler returned sigaltstack reports ss_flags {after}, expected 0"))
}

fn a_without_onstack() -> CaseResult {
    install_alt()?;
    catch(SIGUSR1)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    check(!on_alt(SEEN_SP.load(Ordering::SeqCst)), "a handler installed without SA_ONSTACK ran on the alternate stack")
}

extern "C" fn on_alt_change(sig: i32) {
    let base = ALT_LO.load(Ordering::SeqCst) as u64;
    let other = StackT { ss_sp: base, ss_flags: 0, _pad: 0, ss_size: ALT_SIZE / 2 };
    CHANGE_RET.store(sigaltstack(Some(&other), None), Ordering::SeqCst);
    let off = StackT { ss_sp: 0, ss_flags: SS_DISABLE, _pad: 0, ss_size: 0 };
    DISABLE_RET.store(sigaltstack(Some(&off), None), Ordering::SeqCst);
    record(sig);
}

fn a_eperm() -> CaseResult {
    install_alt()?;
    catch_with(SIGUSR1, on_alt_change as usize as u64, SA_ONSTACK, 0)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the SA_ONSTACK handler did not run")?;
    check(on_alt(SEEN_SP.load(Ordering::SeqCst)), "the SA_ONSTACK handler did not run on the alternate stack")?;
    want_err("changing the alternate stack while running on it", CHANGE_RET.load(Ordering::SeqCst), EPERM)?;
    want_err("disabling the alternate stack while running on it", DISABLE_RET.load(Ordering::SeqCst), EPERM)
}

fn a_disable() -> CaseResult {
    install_alt()?;
    let off = StackT { ss_sp: 0, ss_flags: SS_DISABLE, _pad: 0, ss_size: 0 };
    want_eq("sigaltstack(SS_DISABLE)", sigaltstack(Some(&off), None), 0)?;
    check(query_alt()?.ss_flags == SS_DISABLE, "after SS_DISABLE sigaltstack does not report SS_DISABLE")?;
    catch_with(SIGUSR1, on_sig as usize as u64, SA_ONSTACK, 0)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR1) == 1, "the handler did not run")?;
    check(!on_alt(SEEN_SP.load(Ordering::SeqCst)), "an SA_ONSTACK handler ran on an alternate stack that was disabled")
}

extern "C" fn on_overflow(_sig: i32) {
    let local = 0u8;
    // SAFETY: reads a local of this frame.
    unsafe { core::ptr::read_volatile(&local) };
    process::exit(if on_alt(core::ptr::addr_of!(local) as usize) { 0 } else { 7 })
}

/// Use about `kib` KiB of stack, in 16 KiB frames.
#[inline(never)]
fn deep(kib: usize) -> u8 {
    let frame = core::hint::black_box([kib as u8; 16384]);
    if kib <= 16 { frame[0] } else { deep(kib - 16).wrapping_add(frame[kib % 16384]) }
}

fn a_overflow() -> CaseResult {
    const LIMIT: u64 = 256 << 10;
    const DEPTH_KIB: usize = 1024;
    let (_, hard) = getrlimit(RLIMIT_STACK)?;
    let mut kid = Child::start(|| {
        if install_alt().is_err() { return 10; }
        if catch_with(SIGSEGV, on_overflow as usize as u64, SA_ONSTACK, 0).is_err() { return 11; }
        if prlimit(RLIMIT_STACK, LIMIT, hard) != 0 { return 12; }
        core::hint::black_box(deep(DEPTH_KIB));
        8
    })?;
    let status = kid.wait_for("the child")?;
    if exited(status) && exit_code(status) == 0 { return Ok(()); }
    if signaled(status) && term_sig(status) == SIGSEGV {
        return fail("the stack overflow killed the process: its SIGSEGV handler did not run on the alternate stack");
    }
    fail(match exit_code(status) {
        7 => "after the stack overflow, the SA_ONSTACK SIGSEGV handler ran outside the alternate stack".to_string(),
        8 => format!("using {DEPTH_KIB} KiB of stack under a {} KiB RLIMIT_STACK did not overflow", LIMIT >> 10),
        10 => "installing the alternate stack failed".to_string(),
        11 => "installing the SIGSEGV handler failed".to_string(),
        12 => "lowering RLIMIT_STACK failed".to_string(),
        _ => format!("the overflowing child ended with {}", status_text(status)),
    })
}

extern "C" fn on_alt_outer(sig: i32) {
    let local = 0u8;
    // SAFETY: reads a local of this frame.
    unsafe { core::ptr::read_volatile(&local) };
    OUTER_SP.store(core::ptr::addr_of!(local) as usize, Ordering::SeqCst);
    raise(SIGUSR2);
    push(sig);
}

fn a_nested() -> CaseResult {
    install_alt()?;
    catch_with(SIGUSR1, on_alt_outer as usize as u64, SA_ONSTACK, 0)?;
    catch_with(SIGUSR2, on_sig as usize as u64, SA_ONSTACK, 0)?;
    want_eq("raise(SIGUSR1)", raise(SIGUSR1), 0)?;
    check(count(SIGUSR2) == 1, "the nested SIGUSR2 handler did not run")?;
    let seen = order();
    check(seen == [SIGUSR2, SIGUSR1],
        &format!("handlers finished in the order {seen:?}; expected SIGUSR2's inside SIGUSR1's, which finishes after it ([12, 10])"))?;
    let (outer, inner) = (OUTER_SP.load(Ordering::SeqCst), SEEN_SP.load(Ordering::SeqCst));
    check(on_alt(outer) && on_alt(inner),
        &format!("the outer and nested handlers ran at {outer:#x} and {inner:#x}; both belong on the alternate stack"))?;
    check(inner < outer, &format!("the nested handler's frame ({inner:#x}) is not below the interrupted handler's ({outer:#x}) on the alternate stack"))
}

fn a_thread() -> CaseResult {
    install_alt()?;
    let thread = std::thread::spawn(|| -> Result<i32, String> {
        let mut old = StackT { ss_sp: 1, ss_flags: -1, _pad: 0, ss_size: 1 };
        ok("sigaltstack in the thread", sigaltstack(None, Some(&mut old)))?;
        Ok(old.ss_flags)
    });
    let flags = join(thread)?;
    check(flags == SS_DISABLE, &format!("a new thread reports alternate stack ss_flags {flags}, expected SS_DISABLE (not inherited)"))
}

fn a_exec() -> CaseResult {
    let state = exec_state(|| install_alt().map(|_| ()).map_err(msg))?;
    let flags = state.get("altstack").cloned().unwrap_or_default();
    check(flags == SS_DISABLE.to_string(), &format!("after exec the alternate stack's ss_flags are {flags}, expected SS_DISABLE ({SS_DISABLE})"))
}

// ---------------------------------------------------------------------------
// realtime signals & sigqueue

fn rt_bits() -> u64 { (SIGRTMIN..=SIGRTMAX).fold(0, |set, sig| set | bit(sig)) }

fn r_range() -> CaseResult {
    for sig in SIGRTMIN..=SIGRTMAX { catch(sig)?; }
    block(rt_bits())?;
    for sig in SIGRTMIN..=SIGRTMAX { want_eq(&format!("raise({sig})"), raise(sig), 0)?; }
    let set = pending()?;
    check(set & rt_bits() == rt_bits(), &format!("with every realtime signal raised and blocked, sigpending reports {set:#x}"))?;
    unblock(rt_bits())?;
    let _ = until(200, || (SIGRTMIN..=SIGRTMAX).all(|sig| count(sig) > 0));
    let missing: Vec<i32> = (SIGRTMIN..=SIGRTMAX).filter(|&sig| count(sig) != 1).collect();
    check(missing.is_empty(), &format!("these realtime signals were not delivered exactly once: {missing:?}"))
}

/// POSIX requires queuing for a signal sent by sigqueue to a process with SA_SIGINFO set
/// for it.
fn r_queued() -> CaseResult {
    let sig = SIGRTMIN + 1;
    catch_info(sig)?;
    block(bit(sig))?;
    for value in 0..5 { want_eq("sigqueue(getpid(), SIGRTMIN+1)", sigqueue(pid(), sig, value), 0)?; }
    unblock(bit(sig))?;
    let _ = until(200, || count(sig) >= 5);
    check(count(sig) == 5, &format!("SIGRTMIN+1 sent by sigqueue five times while blocked, to an SA_SIGINFO handler, was delivered {} times", count(sig)))
}

/// POSIX leaves queuing of a realtime signal sent by kill, to a handler without
/// SA_SIGINFO, to the implementation; Linux queues it.
fn r_kill_queued() -> CaseResult {
    let sig = SIGRTMIN + 1;
    catch(sig)?;
    block(bit(sig))?;
    for _ in 0..5 { want_eq("raise(SIGRTMIN+1)", raise(sig), 0)?; }
    unblock(bit(sig))?;
    let _ = until(200, || count(sig) >= 5);
    check(count(sig) == 5, &format!("SIGRTMIN+1 sent by kill five times while blocked was delivered {} times", count(sig)))
}

fn r_lowest_first() -> CaseResult {
    let sigs = [SIGRTMIN + 8, SIGRTMIN + 3, SIGRTMIN + 13];
    let set = sigs.iter().fold(0, |set, &sig| set | bit(sig));
    // Each handler blocks the others, so the next is delivered only once it returns:
    // otherwise the later deliveries nest and their handlers run first.
    for sig in sigs { catch_with(sig, on_sig as usize as u64, 0, set)?; }
    block(set)?;
    for sig in sigs { want_eq(&format!("raise({sig})"), raise(sig), 0)?; }
    unblock(set)?;
    let _ = until(200, || order().len() >= 3);
    let seen = order();
    check(seen == [SIGRTMIN + 3, SIGRTMIN + 8, SIGRTMIN + 13],
        &format!("pending realtime signals were delivered in the order {seen:?}, expected lowest-numbered first"))
}

fn r_default() -> CaseResult {
    let sig = SIGRTMIN + 3;
    let kid = held(|| default(sig))?;
    want_eq("kill(SIGRTMIN+3)", kill(kid.pid(), sig), 0)?;
    kid.expect_death(sig, "a child sent SIGRTMIN+3 with the default action")
}

fn r_kill_info() -> CaseResult {
    catch_info(SIGRTMIN)?;
    want_eq("raise(SIGRTMIN)", raise(SIGRTMIN), 0)?;
    expect_info(SIGRTMIN, SI_USER, pid(), getuid())?;
    Ok(())
}

fn r_sigqueue() -> CaseResult {
    let sig = SIGRTMIN + 1;
    catch_info(sig)?;
    want_eq("sigqueue(getpid(), SIGRTMIN+1, 0x12345678)", sigqueue(pid(), sig, 0x1234_5678), 0)?;
    check(count(sig) == 1, "the handler had not run when sigqueue to the caller returned")?;
    expect_info(sig, SI_QUEUE, pid(), getuid())?;
    let value = INFO_VALUE.load(Ordering::SeqCst);
    check(value == 0x1234_5678, &format!("si_value is {value:#x}, expected 0x12345678"))
}

fn r_fifo() -> CaseResult {
    let sig = SIGRTMIN + 2;
    catch_info(sig)?;
    block(bit(sig))?;
    for value in 1..=8u64 { want_eq(&format!("sigqueue value {value}"), sigqueue(pid(), sig, value), 0)?; }
    unblock(bit(sig))?;
    let _ = until(200, || count(sig) >= 8);
    let n = VALUES_LEN.load(Ordering::SeqCst).min(64);
    let values: Vec<u64> = (0..n).map(|i| VALUES[i].load(Ordering::SeqCst)).collect();
    check(values == (1..=8).collect::<Vec<u64>>(),
        &format!("eight queued SIGRTMIN+2 arrived with values {values:?}, expected 1 to 8 in order"))
}

fn r_sigqueue_standard() -> CaseResult {
    catch_info(SIGUSR1)?;
    want_eq("sigqueue(getpid(), SIGUSR1, 77)", sigqueue(pid(), SIGUSR1, 77), 0)?;
    expect_info(SIGUSR1, SI_QUEUE, pid(), getuid())?;
    let value = INFO_VALUE.load(Ordering::SeqCst);
    check(value == 77, &format!("si_value is {value}, expected 77"))
}

fn r_sigqueue_errors() -> CaseResult {
    let gone = gone_pid()?;
    want_err("sigqueue to a reaped PID", sigqueue(gone, SIGRTMIN, 0), ESRCH)?;
    want_err("sigqueue of signal 65", sigqueue(pid(), 65, 0), EINVAL)?;
    want_eq("sigqueue of signal 0 to the caller", sigqueue(pid(), 0, 0), 0)
}

/// Linux sets the queue limit, POSIX's SIGQUEUE_MAX, with RLIMIT_SIGPENDING. The case
/// lowers it, so sigqueue must refuse within that many signals.
fn r_eagain() -> CaseResult {
    const POSIX_SIGQUEUE_MAX: u64 = 32;
    let sig = SIGRTMIN + 4;
    catch_info(sig)?;
    block(bit(sig))?;
    let (_, hard) = getrlimit(RLIMIT_SIGPENDING)?;
    let limit = hard.min(64);
    want_eq(&format!("prlimit(RLIMIT_SIGPENDING, {limit})"), prlimit(RLIMIT_SIGPENDING, limit, hard), 0)?;
    let mut queued = 0u64;
    let refused = loop {
        let r = sigqueue(pid(), sig, queued);
        if r == -EAGAIN { break true; }
        want("sigqueue", r)?;
        queued += 1;
        if queued > limit { break false; }
    };
    check(refused, &format!("with RLIMIT_SIGPENDING at {limit}, sigqueue queued {queued} signals without failing with EAGAIN"))?;
    check(queued >= POSIX_SIGQUEUE_MAX, &format!("sigqueue failed with EAGAIN after {queued} signals, fewer than _POSIX_SIGQUEUE_MAX (32)"))?;
    unblock(bit(sig))?;
    let _ = until(500, || count(sig) as u64 >= queued);
    check(count(sig) as u64 == queued, &format!("{queued} signals were queued, {} delivered", count(sig)))
}

fn r_sigwaitinfo() -> CaseResult {
    let sig = SIGRTMIN + 5;
    // SA_SIGINFO set for the signal is what makes POSIX require sigqueue to queue it.
    catch_info(sig)?;
    setmask(bit(sig))?;
    for value in [11u64, 22, 33] { want_eq("sigqueue", sigqueue(pid(), sig, value), 0)?; }
    for value in [11u64, 22, 33] {
        let mut info = SigInfo::zero();
        want_eq("sigtimedwait for a queued SIGRTMIN+5", sigtimedwait(bit(sig), &mut info, Some(1000)), sig as i64)?;
        check(info.code == SI_QUEUE && info.value() == value,
            &format!("sigtimedwait returned si_code {} and value {}, expected SI_QUEUE and {value}", info.code, info.value()))?;
    }
    let mut info = SigInfo::zero();
    want_err("a fourth sigtimedwait with a zero timeout", sigtimedwait(bit(sig), &mut info, Some(0)), EAGAIN)
}

// ---------------------------------------------------------------------------
// SIGSTOP, SIGCONT & SIGCHLD

/// A child that counts in slot 0 of `s` for as long as it runs.
fn counter(s: &Shared) -> Result<Child, CaseError> {
    let kid = Child::start(|| loop {
        s.slot(0).fetch_add(1, Ordering::SeqCst);
        for _ in 0..1000 { core::hint::spin_loop(); }
    })?;
    check(until(WAIT_MS, || s.get(0) > 0), "the counting child never ran")?;
    Ok(kid)
}

/// Whether slot `i` of `s` stays unchanged over QUIET_MS.
fn frozen(s: &Shared, i: usize) -> bool {
    let before = s.get(i);
    let _ = time::sleep_ms(QUIET_MS);
    s.get(i) == before
}

fn j_stop_cont() -> CaseResult {
    let s = Shared::new()?;
    let mut kid = counter(&s)?;
    want_eq("kill(SIGSTOP)", kill(kid.pid, SIGSTOP), 0)?;
    kid.expect_stop(SIGSTOP, "a running child sent SIGSTOP")?;
    check(frozen(&s, 0), "a child reported stopped went on running")?;
    let stopped_at = s.get(0);
    want_eq("kill(SIGCONT)", kill(kid.pid, SIGCONT), 0)?;
    kid.expect_continued("the stopped child sent SIGCONT")?;
    check(until(1000, || s.get(0) > stopped_at), "a child reported continued did not run again")
}

fn j_pending_while_stopped() -> CaseResult {
    let s = shared_for_handlers()?;
    let mut kid = Child::start(|| {
        if catch_with(SIGUSR1, on_mark as usize as u64, SA_RESTART, 0).is_err() { return 10; }
        s.set(0, 1);
        loop { pause(); }
    })?;
    check(until(WAIT_MS, || s.get(0) == 1) && parked(kid.pid, WAIT_MS), "the child never got ready")?;
    want_eq("kill(SIGSTOP)", kill(kid.pid, SIGSTOP), 0)?;
    kid.expect_stop(SIGSTOP, "the child sent SIGSTOP")?;
    want_eq("kill(SIGUSR1) to the stopped child", kill(kid.pid, SIGUSR1), 0)?;
    let _ = time::sleep_ms(QUIET_MS);
    check(s.get(1) == 0, "a stopped process ran a handler for a signal sent while it was stopped")?;
    want_eq("kill(SIGCONT)", kill(kid.pid, SIGCONT), 0)?;
    check(until(1000, || s.get(1) != 0), "SIGUSR1, sent while the child was stopped, was not delivered after SIGCONT")
}

fn j_kill_stopped() -> CaseResult {
    let mut kid = held(|| Ok(()))?;
    want_eq("kill(SIGSTOP)", kill(kid.pid(), SIGSTOP), 0)?;
    kid.child.expect_stop(SIGSTOP, "the child sent SIGSTOP")?;
    want_eq("kill(SIGKILL) to the stopped child", kill(kid.pid(), SIGKILL), 0)?;
    kid.child.expect_death(SIGKILL, "a stopped child sent SIGKILL")
}

fn j_tstp_caught() -> CaseResult {
    let s = shared_for_handlers()?;
    let mut kid = held(|| { ok("setpgid", setpgid(0, 0))?; catch_with(SIGTSTP, on_mark as usize as u64, SA_RESTART, 0) })?;
    want_eq("kill(SIGTSTP)", kill(kid.pid(), SIGTSTP), 0)?;
    if let Some(status) = reports_within(kid.pid(), WUNTRACED, QUIET_MS)? {
        kid.child.live = stopped(status);
        return fail(format!("a child catching SIGTSTP reported {}", status_text(status)));
    }
    check(s.get(1) != 0, "the SIGTSTP handler did not run")?;
    kid.release()
}

fn j_orphaned() -> CaseResult {
    for sig in [SIGTSTP, SIGTTIN, SIGTTOU] {
        // A session of its own: its group has no parent in another group of its session.
        let mut kid = held(|| { ok("setsid", setsid())?; default(sig) })?;
        want_eq(&format!("kill({})", name(sig)), kill(kid.pid(), sig), 0)?;
        if let Some(status) = reports_within(kid.pid(), WUNTRACED, QUIET_MS)? {
            kid.child.live = stopped(status);
            return fail(format!("a process in an orphaned process group sent {} reported {}", name(sig), status_text(status)));
        }
        kid.release().map_err(|e| CaseError::Fail(format!("after {}: {}", name(sig), msg(e))))?;
    }
    Ok(())
}

fn j_cont_blocked() -> CaseResult {
    for (what, setup) in [("blocked", 0), ("ignored", 1)] {
        let mut kid = held(move || if setup == 0 { block(bit(SIGCONT)).map(|_| ()) } else { ignore(SIGCONT) })?;
        want_eq("kill(SIGSTOP)", kill(kid.pid(), SIGSTOP), 0)?;
        kid.child.expect_stop(SIGSTOP, &format!("a child with SIGCONT {what}"))?;
        want_eq("kill(SIGCONT)", kill(kid.pid(), SIGCONT), 0)?;
        kid.child.expect_continued(&format!("a stopped child with SIGCONT {what}"))?;
        kid.release()?;
    }
    Ok(())
}

fn j_cont_discards() -> CaseResult {
    const VALID: u64 = 1 << 63;
    let s = Shared::new()?;
    let mut kid = Child::start(|| {
        if setpgid(0, 0) != 0 || block(bit(SIGTSTP)).is_err() { return 10; }
        s.set(0, 1);
        while s.get(2) == 0 {
            match pending() { Ok(set) => s.set(1, set | VALID), Err(_) => return 11 }
            nap();
        }
        if unblock(bit(SIGTSTP)).is_err() { return 12; }
        0
    })?;
    check(until(WAIT_MS, || s.get(0) == 1), "the child never got ready")?;
    want_eq("kill(SIGTSTP)", kill(kid.pid, SIGTSTP), 0)?;
    check(until(1000, || s.get(1) & bit(SIGTSTP) != 0), "a blocked SIGTSTP never showed as pending in the child")?;
    want_eq("kill(SIGCONT)", kill(kid.pid, SIGCONT), 0)?;
    let discarded = until(1000, || s.get(1) & VALID != 0 && s.get(1) & bit(SIGTSTP) == 0);
    s.set(2, 1);
    check(discarded, "SIGCONT did not discard the pending SIGTSTP")?;
    let (_, status) = wait_within(kid.pid, WUNTRACED, WAIT_MS)?;
    kid.live = stopped(status);
    check(exited(status) && exit_code(status) == 0, &format!("after unblocking SIGTSTP the child reported {}", status_text(status)))
}

/// Wait up to a second for the `n`th SIGCHLD to be handled, then check its siginfo.
fn sigchld_info(n: u32, code: i32, child: i32, status: i32) -> CaseResult {
    check(until(1000, || count(SIGCHLD) >= n), &format!("SIGCHLD number {n} never arrived"))?;
    let info = seen_info()?;
    let got_status = INFO_STATUS.load(Ordering::SeqCst);
    check(info.signo == SIGCHLD && info.code == code && info.pid() == child && got_status == status,
        &format!("SIGCHLD had si_signo {}, si_code {}, si_pid {}, si_status {got_status}; expected {SIGCHLD}, {code}, {child}, {status}",
            info.signo, info.code, info.pid()))
}

fn j_sigchld_exit() -> CaseResult {
    catch_info(SIGCHLD)?;
    let mut kid = Child::start(|| 42)?;
    let result = sigchld_info(1, CLD_EXITED, kid.pid, 42);
    kid.expect_exit(42, "the child")?;
    result
}

fn j_sigchld_killed() -> CaseResult {
    catch_info(SIGCHLD)?;
    let kid = held(|| Ok(()))?;
    let child = kid.pid();
    want_eq("kill(SIGTERM)", kill(child, SIGTERM), 0)?;
    let result = sigchld_info(1, CLD_KILLED, child, SIGTERM);
    kid.expect_death(SIGTERM, "the child sent SIGTERM")?;
    result
}

fn j_sigchld_stop() -> CaseResult {
    catch_info(SIGCHLD)?;
    let mut kid = held(|| Ok(()))?;
    let child = kid.pid();
    want_eq("kill(SIGSTOP)", kill(child, SIGSTOP), 0)?;
    sigchld_info(1, CLD_STOPPED, child, SIGSTOP)?;
    kid.child.expect_stop(SIGSTOP, "the child sent SIGSTOP")?;
    want_eq("kill(SIGCONT)", kill(child, SIGCONT), 0)?;
    sigchld_info(2, CLD_CONTINUED, child, SIGCONT)?;
    kid.child.expect_continued("the child sent SIGCONT")?;
    kid.release()
}

fn j_nocldstop() -> CaseResult {
    catch_with(SIGCHLD, on_sig as usize as u64, SA_NOCLDSTOP, 0)?;
    let mut kid = held(|| Ok(()))?;
    want_eq("kill(SIGSTOP)", kill(kid.pid(), SIGSTOP), 0)?;
    kid.child.expect_stop(SIGSTOP, "the child sent SIGSTOP")?;
    let _ = time::sleep_ms(QUIET_MS);
    check(count(SIGCHLD) == 0, "with SA_NOCLDSTOP, a child's stop raised SIGCHLD")?;
    want_eq("kill(SIGCONT)", kill(kid.pid(), SIGCONT), 0)?;
    kid.child.expect_continued("the child sent SIGCONT")?;
    let _ = time::sleep_ms(QUIET_MS);
    check(count(SIGCHLD) == 0, "with SA_NOCLDSTOP, a child's continuing raised SIGCHLD")?;
    kid.release()?;
    check(until(1000, || count(SIGCHLD) == 1), "with SA_NOCLDSTOP, a child's exit did not raise SIGCHLD")
}

fn j_nocldwait() -> CaseResult {
    set_action(SIGCHLD, &action(SIG_DFL, SA_NOCLDWAIT, 0))?;
    let mut kid = Child::start(|| 0)?;
    let gone = until(WAIT_MS, || kill(kid.pid, 0) == -ESRCH);
    check(gone, "with SA_NOCLDWAIT, an exited child stayed as a zombie")?;
    kid.live = false;
    let mut status = 0;
    want_err("waitpid(-1, WNOHANG) with SA_NOCLDWAIT and no live children", wait4(-1, &mut status, WNOHANG), ECHILD)
}

fn j_waitid() -> CaseResult {
    let mut kid = held(|| Ok(()))?;
    let child = kid.pid();
    let report = |options: i32| -> Result<SigInfo, CaseError> {
        let mut info = SigInfo::zero();
        let seen = until(WAIT_MS, || {
            info = SigInfo::zero();
            waitid(child, options | WNOHANG, &mut info) == 0 && info.pid() == child
        });
        if !seen { return err(format!("waitid(P_PID, options {options:#x}) reported nothing")); }
        Ok(info)
    };
    want_eq("kill(SIGSTOP)", kill(child, SIGSTOP), 0)?;
    let info = report(WUNTRACED)?;
    check(info.code == CLD_STOPPED && info.status() == SIGSTOP,
        &format!("waitid(WSTOPPED) reported si_code {} si_status {}, expected CLD_STOPPED and SIGSTOP", info.code, info.status()))?;
    want_eq("kill(SIGCONT)", kill(child, SIGCONT), 0)?;
    let info = report(WCONTINUED)?;
    check(info.code == CLD_CONTINUED && info.status() == SIGCONT,
        &format!("waitid(WCONTINUED) reported si_code {} si_status {}, expected CLD_CONTINUED and SIGCONT", info.code, info.status()))?;
    kid.let_go();
    let info = report(WEXITED)?;
    kid.child.live = false;
    check(info.code == CLD_EXITED && info.status() == 0,
        &format!("waitid(WEXITED) reported si_code {} si_status {}, expected CLD_EXITED and 0", info.code, info.status()))
}

/// Expect a member of a group continued by kill(-pgid, SIGCONT), and then released, to
/// exit 0. If it does not, a second SIGCONT tells the two ways it can be stuck apart: a
/// process still stopped runs on and exits, one that missed its wakeup stays blocked.
fn released_member_exits(member: &mut Held, what: &str) -> CaseResult {
    let Err(e) = member.child.expect_exit(0, what) else { return Ok(()) };
    if !member.child.live { return Err(e); }
    let pid = member.pid();
    want_eq("a second kill(SIGCONT)", kill(pid, SIGCONT), 0)?;
    let stuck = match reports_within(pid, 0, bounded(1000, REPORT_MS))? {
        Some(status) => {
            member.child.live = false;
            format!("it was still stopped: a second SIGCONT sent to it alone let it run on, and it ended with {}", status_text(status))
        }
        None => "it was not stopped: a second SIGCONT sent to it alone did not end it either, so it stayed blocked reading a pipe at end of file".to_string(),
    };
    fail(format!("{}; {stuck}", msg(e)))
}

fn j_group() -> CaseResult {
    let mut leader = held(|| ok("setpgid", setpgid(0, 0)).map(|_| ()))?;
    let group = leader.pid();
    let mut member = held(move || ok("setpgid", setpgid(0, group)).map(|_| ()))?;
    want_eq("kill(-pgid, SIGSTOP)", kill(-group, SIGSTOP), 0)?;
    leader.child.expect_stop(SIGSTOP, "the group's leader")?;
    member.child.expect_stop(SIGSTOP, "the group's other member")?;
    want_eq("kill(-pgid, SIGCONT)", kill(-group, SIGCONT), 0)?;
    leader.child.expect_continued("the group's leader")?;
    member.child.expect_continued("the group's other member")?;
    leader.let_go();
    member.let_go();
    released_member_exits(&mut leader, "the group's leader, released after SIGCONT,")?;
    released_member_exits(&mut member, "the group's other member, released after SIGCONT,")
}

fn j_stop_threads() -> CaseResult {
    let cpus = processors()?;
    if cpus < 2 {
        return skip(format!("{cpus} processor online; the case needs 2, so both threads compute at once"));
    }
    let s = Shared::new()?;
    let page = s.page as usize;
    // Slots 0 and 1 count each thread's turns; slot 2 carries handoffs from the first
    // thread to the second.
    let mut kid = Child::start(move || {
        // SAFETY: the shared page stays mapped in the child for its whole life.
        let slot = move |i: usize| unsafe { &*((page + SHARED_SLOT_BYTES * i) as *const AtomicU64) };
        // Each counter has one writer. Keep its count in a register and
        // publish it with a store, avoiding LL/SC retries for a value that
        // does not need a read-modify-write operation.
        let _thread = std::thread::spawn(move || {
            let mut turns = 0u64;
            loop {
                turns = turns.wrapping_add(1);
                slot(1).store(turns, Ordering::SeqCst);
                answer_handoff(slot(2));
            }
        });
        let mut next = 1u64;
        let mut turns = 0u64;
        loop {
            turns = turns.wrapping_add(1);
            slot(0).store(turns, Ordering::SeqCst);
            // Hand off once the second thread has answered the last one.
            if slot(2).load(Ordering::SeqCst) == next - 1 {
                slot(2).store(next, Ordering::SeqCst);
                next += 2;
            }
        }
    })?;
    check(until(WAIT_MS, || s.get(0) > 0 && s.get(1) > 0), "the child's two threads never both ran")?;
    // Taking turns on one processor, the threads hand off at most once per timer tick.
    let (first_before, second_before) = (s.get(0), s.get(1));
    let (before, start) = (s.get(2), now_ms());
    let _ = time::sleep_ms(200);
    let (made, took) = ((s.get(2) - before) / 2, now_ms().saturating_sub(start));
    let first_delta = s.get(0).wrapping_sub(first_before);
    let second_delta = s.get(1).wrapping_sub(second_before);
    check(made >= HANDOFFS, &format!("with {cpus} processors online, the child's two threads made {made} handoffs in {took} ms, fewer than {HANDOFFS}; first counter advanced {first_delta}, second counter advanced {second_delta}"))?;
    want_eq("kill(SIGSTOP)", kill(kid.pid, SIGSTOP), 0)?;
    kid.expect_stop(SIGSTOP, "a two-threaded child sent SIGSTOP")?;
    let (a, b) = (s.get(0), s.get(1));
    let _ = time::sleep_ms(QUIET_MS);
    check(s.get(0) == a, "the stopped process's first thread went on running")?;
    check(s.get(1) == b, &format!("with {cpus} processors online, the stopped process's second thread went on running"))?;
    want_eq("kill(SIGCONT)", kill(kid.pid, SIGCONT), 0)?;
    kid.expect_continued("the two-threaded child sent SIGCONT")?;
    check(until(1000, || s.get(0) > a && s.get(1) > b), "after SIGCONT, both threads did not run again")
}

static SUITE: Suite = suite(
    "signals", "Signals", &[
        category("dispositions", "dispositions & default actions", &[
            case("terminate", "Signals whose default action is to terminate end the process with that signal in its wait status", disp_terminate),
            case("core", "Signals whose default action is to terminate with a core end the process with that signal in its wait status", disp_core),
            case("ignore", "SIGCHLD, SIGURG and SIGWINCH are ignored by default", disp_ignore),
            case("stop", "SIGSTOP, SIGTSTP, SIGTTIN and SIGTTOU stop the process by default, reported by WUNTRACED with the signal", disp_stop),
            case("continue", "SIGCONT sent to a running process with the default action leaves it running", disp_continue),
            case("sig-ign", "Signals set to SIG_IGN, including terminal stop and realtime signals, have no effect", disp_sig_ign),
            case("ign-discards-pending", "Setting SIG_IGN discards a pending signal, even a blocked one", disp_ign_discards),
            case("dfl-restores", "Setting SIG_DFL after a handler restores the default action", disp_dfl_restores),
            case("sigaction-old", "sigaction returns the previous handler, mask and flags, and a null action only queries", disp_sigaction_old),
            case("sigaction-einval", "sigaction of signal 0, a negative signal or one above 64 fails with EINVAL", disp_einval),
            case("kill-stop-fixed", "SIGKILL and SIGSTOP cannot be caught or ignored (EINVAL) and report SIG_DFL", disp_kill_stop),
            case("sigaction-layout", "Linux ABI: rt_sigaction takes struct sigaction as handler, flags, restorer, mask", disp_sigaction_layout),
            case("kill-zero", "kill with signal 0 succeeds for a live process and a zombie, and fails with ESRCH once it is reaped", disp_kill_zero),
            case("kill-einval", "kill with an invalid signal number fails with EINVAL", disp_kill_einval),
            case("kill-esrch", "kill of a missing process or process group fails with ESRCH", disp_kill_esrch),
            case("kill-group", "kill(-pgid) signals every member of that process group and no other process", disp_kill_group),
            case("kill-own-group", "kill(0) signals every member of the caller's process group, the caller included", disp_kill_own_group),
            case("kill-all", "kill(-1) as non-root signals every process of the caller's user and none of another user", disp_kill_all),
            case("kill-eperm", "kill as non-root fails with EPERM for another user's process and works for its own", disp_kill_eperm),
            case("sigcont-session", "SIGCONT may be sent to another user's process in the caller's session", disp_sigcont_session),
            case("fork", "A child inherits its parent's handlers and ignored signals, and the handlers run in it", disp_fork),
            case("exec", "exec resets caught signals to SIG_DFL and keeps ignored ones ignored", disp_exec),
        ]),
        category("masks", "masks & pending signals", &[
            case("block-pending", "A blocked signal stays pending and is delivered before sigprocmask returns once unblocked", mask_block_pending),
            case("stays-pending", "A blocked signal stays pending across system calls, sleeps and yields", mask_stays_pending),
            case("not-queued", "Linux policy: a standard signal sent by kill five times while blocked is delivered once", mask_not_queued),
            case("how", "SIG_BLOCK adds to the mask, SIG_UNBLOCK removes from it, SIG_SETMASK replaces it, and each returns the old mask", mask_how),
            case("einval", "sigprocmask with an invalid how fails with EINVAL and leaves the mask unchanged", mask_einval),
            case("null-set", "sigprocmask with a null set ignores how and reports the mask", mask_null_set),
            case("unblockable", "SIGKILL and SIGSTOP cannot be blocked and still kill and stop", mask_unblockable),
            case("sigpending", "sigpending reports exactly the blocked signals that are pending", mask_sigpending),
            case("unblock-all", "Unblocking two pending signals delivers both", mask_unblock_all),
            case("per-thread", "pthread_sigmask in one thread leaves another thread's mask alone", mask_per_thread),
            case("thread-inherits", "A new thread starts with its creator's signal mask", mask_thread_inherits),
            case("process-directed", "A signal sent to the process is delivered to a thread that does not block it", mask_process_directed),
            case("thread-directed", "pthread_kill delivers a signal to the thread it names", mask_thread_directed),
            case("fork-mask", "A child inherits its parent's signal mask", mask_fork),
            case("fork-pending", "A child starts with no pending signals and the parent's stay pending", mask_fork_pending),
            case("exec-mask", "exec keeps the signal mask and the pending signals", mask_exec),
            case("exec-delivered", "A signal pending across exec is delivered with its default action once the new program unblocks it", mask_exec_delivered),
        ]),
        category("handlers", "handlers, SA_RESTART & EINTR", &[
            case("runs", "A handler runs with the signal number before kill to the caller returns", h_runs),
            case("siginfo-self", "An SA_SIGINFO handler gets si_signo, si_code SI_USER, si_pid, si_uid and a context", h_siginfo_self),
            case("siginfo-sender", "SA_SIGINFO reports the sending process's PID and real user ID", h_siginfo_sender),
            case("siginfo-fault", "A fault on an unmapped address gives SIGSEGV with SEGV_MAPERR and the address in si_addr", h_siginfo_fault),
            case("resethand", "SA_RESETHAND resets the action to SIG_DFL on entry to the handler, so the handler runs once", h_resethand),
            case("self-blocked", "A signal is blocked while its own handler runs and unblocked when it returns", h_self_blocked),
            case("sa-mask", "sa_mask is blocked while the handler runs and unblocked when it returns", h_sa_mask),
            case("nodefer", "With SA_NODEFER the signal raised in its own handler interrupts it", h_nodefer),
            case("deferred", "Without SA_NODEFER the signal raised in its own handler runs after it returns", h_deferred),
            case("nested", "A different signal raised in a handler runs its own handler inside it", h_nested),
            case("mask-restored", "Returning from a handler restores the mask it interrupted, undoing the handler's sigprocmask", h_mask_restored),
            case("restart-read", "A blocking read interrupted by an SA_RESTART handler is restarted", h_restart_read),
            case("eintr-read", "A blocking read interrupted by a handler without SA_RESTART fails with EINTR", h_eintr_read),
            case("nanosleep", "A caught signal interrupts nanosleep with EINTR and the time left, even with SA_RESTART", h_nanosleep),
            case("restart-wait", "waitpid interrupted by an SA_RESTART handler is restarted", h_restart_wait),
            case("eintr-wait", "waitpid interrupted by a handler without SA_RESTART fails with EINTR", h_eintr_wait),
            case("registers", "General registers survive handlers that interrupt code holding them", h_registers),
            case("fp-registers", "All 128 bits of every floating-point and SIMD register survive handlers that overwrite them", h_fp_registers),
            case("spinning-target", "A process spinning in user mode, with no system call, on another processor runs its handler", h_spinning_target),
        ]),
        category("waits", "sigsuspend, pause & sigtimedwait", &[
            case("sigsuspend", "sigsuspend returns EINTR after the handler and restores the mask", w_sigsuspend),
            case("sigsuspend-pending", "A signal already pending when sigsuspend unblocks it ends the wait at once with its handler run", w_sigsuspend_pending),
            case("temp-mask", "A signal blocked by sigsuspend's mask does not end the wait and is delivered after it", w_temp_mask),
            case("terminates", "A default-action terminating signal ends a process in sigsuspend", w_terminates),
            case("ignored", "An ignored signal does not end sigsuspend", w_ignored),
            case("pause", "pause returns EINTR after a handler runs", w_pause),
            case("signal-first", "A child's signal sent before the parent waits is caught by mask-then-sigsuspend", w_signal_first),
            case("race-loop", "100 rounds of a child signalling while the parent enters sigsuspend lose no signal", w_race_loop),
            case("sigtimedwait-pending", "sigtimedwait accepts a pending signal with its siginfo, without running the handler", w_timedwait_pending),
            case("sigtimedwait-timeout", "sigtimedwait with nothing pending fails with EAGAIN after its timeout", w_timedwait_timeout),
            case("sigtimedwait-poll", "sigtimedwait with a zero timeout and nothing pending fails with EAGAIN at once", w_timedwait_poll),
            case("sigtimedwait-einval", "sigtimedwait with an invalid timeout fails with EINVAL", w_timedwait_einval),
            case("sigwaitinfo", "sigwaitinfo waits for a signal sent by another process and reports its sender", w_sigwaitinfo),
            case("sigtimedwait-eintr", "A caught signal outside the set interrupts sigtimedwait with EINTR", w_timedwait_eintr),
            case("alarm", "alarm(1) delivers SIGALRM about a second later", w_alarm),
            case("alarm-default", "SIGALRM from alarm with the default action terminates the process", w_alarm_default),
            case("alarm-cancel", "alarm(0) returns the seconds left and cancels the alarm", w_alarm_cancel),
            case("setitimer", "A one-shot ITIMER_REAL delivers SIGALRM on time and getitimer reports the time left", w_setitimer),
            case("interval", "A periodic ITIMER_REAL keeps delivering SIGALRM, no more often than its interval", w_interval),
            case("itimer-old", "setitimer returns the previous value and disarming leaves getitimer at zero", w_itimer_old),
            case("itimer-einval", "getitimer and setitimer fail with EINVAL for an unknown timer or an invalid time", w_itimer_einval),
            case("itimer-virtual", "ITIMER_VIRTUAL counts CPU time, not sleep, and delivers SIGVTALRM after the process computes for its interval", w_virtual),
            case("itimer-prof", "ITIMER_PROF counts CPU time, not sleep, and delivers SIGPROF after the process computes for its interval", w_prof),
        ]),
        category("altstack", "alternate signal stacks", &[
            case("initial", "A process starts with its alternate stack disabled", a_initial),
            case("set", "sigaltstack installs a stack and reads it back", a_set),
            case("enomem", "A stack smaller than MINSIGSTKSZ fails with ENOMEM", a_enomem),
            case("einval", "An undefined ss_flags value fails with EINVAL", a_einval),
            case("runs-on", "An SA_ONSTACK handler runs on the alternate stack, which reports SS_ONSTACK only while it does", a_runs_on),
            case("without-onstack", "A handler without SA_ONSTACK runs on the normal stack", a_without_onstack),
            case("eperm", "Changing or disabling the alternate stack while running on it fails with EPERM", a_eperm),
            case("disable", "SS_DISABLE disables the stack and SA_ONSTACK handlers run on the normal stack", a_disable),
            case("overflow", "After a stack overflow, the SIGSEGV handler runs on the alternate stack", a_overflow),
            case("nested", "A signal nested in an alternate-stack handler runs further down the same alternate stack", a_nested),
            case("thread", "A new thread does not inherit its creator's alternate stack", a_thread),
            case("exec", "exec leaves the new program with no alternate stack", a_exec),
        ]),
        category("realtime", "realtime signals & sigqueue", &[
            case("range", "Every realtime signal from 32 to 64 can be caught, blocked, held pending and delivered", r_range),
            case("queued", "A realtime signal sent by sigqueue five times while blocked, to an SA_SIGINFO handler, is delivered five times", r_queued),
            case("kill-queued", "Linux policy: a realtime signal sent by kill five times while blocked is delivered five times", r_kill_queued),
            case("lowest-first", "Pending realtime signals are delivered lowest-numbered first", r_lowest_first),
            case("default", "A realtime signal with the default action terminates the process", r_default),
            case("kill-info", "A realtime signal sent by kill reports SI_USER and the sender", r_kill_info),
            case("sigqueue", "sigqueue delivers its value with SI_QUEUE and the sender", r_sigqueue),
            case("fifo", "Queued instances of one realtime signal arrive in the order sent, each with its value", r_fifo),
            case("standard", "sigqueue of a standard signal delivers its value with SI_QUEUE", r_sigqueue_standard),
            case("sigqueue-errors", "sigqueue fails with ESRCH and EINVAL, and signal 0 checks the target", r_sigqueue_errors),
            case("eagain", "Linux ABI: with RLIMIT_SIGPENDING lowered to 64, sigqueue fails with EAGAIN at that limit, after at least 32, and every queued signal is delivered", r_eagain),
            case("sigwaitinfo", "sigtimedwait takes realtime signals queued by sigqueue one at a time, in order, with their values", r_sigwaitinfo),
        ]),
        category("job-control", "SIGSTOP, SIGCONT & SIGCHLD", &[
            case("stop-cont", "A stopped process does not run until SIGCONT, and waitpid reports the stop and the continue", j_stop_cont),
            case("pending-while-stopped", "A signal sent to a stopped process is delivered after SIGCONT, not before", j_pending_while_stopped),
            case("kill-stopped", "SIGKILL ends a stopped process", j_kill_stopped),
            case("tstp-caught", "A caught SIGTSTP runs its handler and does not stop the process", j_tstp_caught),
            case("orphaned", "SIGTSTP, SIGTTIN and SIGTTOU do not stop a process in an orphaned process group", j_orphaned),
            case("cont-blocked", "SIGCONT continues a stopped process even when blocked or ignored", j_cont_blocked),
            case("cont-discards", "SIGCONT discards a pending stop signal", j_cont_discards),
            case("sigchld-exit", "A child's exit raises SIGCHLD with CLD_EXITED, its PID and exit status", j_sigchld_exit),
            case("sigchld-killed", "A child killed by a signal raises SIGCHLD with CLD_KILLED and the signal", j_sigchld_killed),
            case("sigchld-stop", "A child's stop and continue raise SIGCHLD with CLD_STOPPED and CLD_CONTINUED", j_sigchld_stop),
            case("nocldstop", "With SA_NOCLDSTOP a child's stop and continue raise no SIGCHLD, but its exit does", j_nocldstop),
            case("nocldwait", "With SA_NOCLDWAIT an exited child leaves no zombie and waitpid fails with ECHILD", j_nocldwait),
            case("waitid", "waitid reports CLD_STOPPED, CLD_CONTINUED and CLD_EXITED with their signals and status", j_waitid),
            case("group", "kill(-pgid) with SIGSTOP and SIGCONT stops and continues every member", j_group),
            case("stop-threads", "SIGSTOP stops every thread of a process computing on two processors, and SIGCONT resumes them", j_stop_threads),
        ]),
    ],
);

fn main() { SUITE.run() }
