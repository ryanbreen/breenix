//! Time & timers: clocks and their resolution, nanosleep and clock_nanosleep, POSIX
//! per-process timers, and alarm with the interval timers, as POSIX specifies them.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! Every wait inside a case is bounded and stops early enough to leave the case time to
//! clean up and report. The processes a case starts are killed when it ends, and the
//! runner, which a suite runs as PID 1, kills and reaps any that remain before the next
//! case, so no case depends on another. A case that sets CLOCK_REALTIME puts it back
//! before it ends, whatever its result.
//!
//! Cases call the kernel by its Linux numbers and assert on the raw return, so an
//! unimplemented call fails with ENOSYS. On x86-64 they use the SYSCALL instruction, as a
//! C library does. Library-level interfaces are made as a C library makes them: time()
//! through clock_gettime on ARM64, which has no time call, and alarm through setitimer
//! there.
//!
//! A sleep or timer may end late by at most two timer ticks and 20 ms (`LATE_MS`); the
//! tick is 5 ms on x86-64 (200 Hz) and 1 ms on ARM64. Nothing may end early. Each case
//! reports the quantities it asserts on as VALUE records, and each wait of 100 ms or more
//! on a sleep or timer as a WAIT record.
use libbreenix::io;
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::Sigaction;
use libbreenix::suite::{case, case_ms_left, category, check, fail, pulses, skip, suite, value, wait_for, CaseError, CaseResult, Suite};
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use libbreenix::types::Fd;
use std::ffi::CString;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const READ: u64 = 0;
    pub const IOCTL: u64 = 16;
    pub const RT_SIGACTION: u64 = 13;
    pub const RT_SIGPROCMASK: u64 = 14;
    pub const NANOSLEEP: u64 = 35;
    pub const GETITIMER: u64 = 36;
    pub const ALARM: u64 = 37;
    pub const SETITIMER: u64 = 38;
    pub const GETPID: u64 = 39;
    pub const EXECVE: u64 = 59;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const GETTIMEOFDAY: u64 = 96;
    pub const GETRUSAGE: u64 = 98;
    pub const SETUID: u64 = 105;
    pub const RT_SIGPENDING: u64 = 127;
    pub const RT_SIGTIMEDWAIT: u64 = 128;
    pub const TIME: u64 = 201;
    pub const SCHED_SETAFFINITY: u64 = 203;
    pub const TIMER_CREATE: u64 = 222;
    pub const TIMER_SETTIME: u64 = 223;
    pub const TIMER_GETTIME: u64 = 224;
    pub const TIMER_GETOVERRUN: u64 = 225;
    pub const TIMER_DELETE: u64 = 226;
    pub const CLOCK_SETTIME: u64 = 227;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const CLOCK_GETRES: u64 = 229;
    pub const CLOCK_NANOSLEEP: u64 = 230;
    pub const GETCPU: u64 = 309;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: u64 = 63;
    pub const IOCTL: u64 = 29;
    pub const RT_SIGACTION: u64 = 134;
    pub const RT_SIGPROCMASK: u64 = 135;
    pub const NANOSLEEP: u64 = 101;
    pub const GETITIMER: u64 = 102;
    pub const SETITIMER: u64 = 103;
    pub const GETPID: u64 = 172;
    pub const EXECVE: u64 = 221;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const GETTIMEOFDAY: u64 = 169;
    pub const GETRUSAGE: u64 = 165;
    pub const SCHED_SETAFFINITY: u64 = 122;
    pub const GETCPU: u64 = 168;
    pub const SETUID: u64 = 146;
    pub const RT_SIGPENDING: u64 = 136;
    pub const RT_SIGTIMEDWAIT: u64 = 137;
    pub const TIMER_CREATE: u64 = 107;
    pub const TIMER_GETTIME: u64 = 108;
    pub const TIMER_GETOVERRUN: u64 = 109;
    pub const TIMER_SETTIME: u64 = 110;
    pub const TIMER_DELETE: u64 = 111;
    pub const CLOCK_SETTIME: u64 = 112;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const CLOCK_GETRES: u64 = 114;
    pub const CLOCK_NANOSLEEP: u64 = 115;
}

const EPERM: i64 = 1;
const EINTR: i64 = 4;
const EAGAIN: i64 = 11;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;

const SIGKILL: i32 = 9;
const SIGUSR1: i32 = 10;
const SIGALRM: i32 = 14;
const SIGCONT: i32 = 18;
const SIGSTOP: i32 = 19;
const SIGVTALRM: i32 = 26;
const SIGPROF: i32 = 27;
/// The first realtime signal as the kernel numbers it, before a C library reserves any.
const SIGRTMIN: i32 = 32;

const SIG_IGN: u64 = 1;
const SIG_BLOCK: i32 = 0;
const SA_RESTORER: u64 = 0x0400_0000;
const SA_RESTART: u64 = 0x1000_0000;
/// si_code of a signal sent by a POSIX timer's expiry.
const SI_TIMER: i32 = -2;

const WNOHANG: i32 = 1;
const WUNTRACED: i32 = 2;
const WCONTINUED: i32 = 8;
const O_CLOEXEC: i32 = 0x80000;

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
const CLOCK_MONOTONIC_COARSE: i32 = 6;
/// A clock ID no system defines.
const CLOCK_UNKNOWN: i32 = 1000;
const TIMER_ABSTIME: u64 = 1;

const SIGEV_SIGNAL: i32 = 0;
const SIGEV_NONE: i32 = 1;

const ITIMER_REAL: i32 = 0;
const ITIMER_VIRTUAL: i32 = 1;
const ITIMER_PROF: i32 = 2;

/// Linux's RTC_RD_TIME: _IOR('p', 0x09, struct rtc_time), nine ints.
const RTC_RD_TIME: u64 = 0x8024_7009;

const NS: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

/// The kernel's timer tick in milliseconds: 200 Hz on x86-64, 1000 Hz on ARM64.
#[cfg(target_arch = "x86_64")]
const TICK_MS: i64 = 5;
#[cfg(target_arch = "aarch64")]
const TICK_MS: i64 = 1;
/// How late a sleep or timer may end: two ticks, and 20 ms for a busy host.
const LATE_MS: i64 = 2 * TICK_MS + 20;

/// A user of the suite's own, for the permission case.
const USER_A: u32 = 4242;

/// How long a case waits for another process to change state. A working kernel takes
/// milliseconds; the bound only keeps a failing case inside the runner's limit.
const WAIT_MS: u64 = 3000;
/// The same for an exec, which loads a program from disk.
const EXEC_MS: u64 = 6000;
/// What a case's waits leave of the runner's limit, for the case to clean up and report.
const CLEANUP_MS: u64 = 1500;
/// What cleanup's own waits leave, for the case to report.
const REPORT_MS: u64 = 300;
/// The helper the exec cases run (`time_exec.rs`).
const HELPER: &str = "/usr/local/test/bin/time-exec_test";

type Checked = Result<(), String>;
/// struct timespec: seconds and nanoseconds.
type Ts = [i64; 2];
/// struct itimerspec: the interval's seconds and nanoseconds, then the value's.
type Its = [i64; 4];
/// struct itimerval: the interval's seconds and microseconds, then the value's.
type Itv = [i64; 4];

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
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 9 => "EBADF", 10 => "ECHILD",
        11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES", 14 => "EFAULT", 19 => "ENODEV", 22 => "EINVAL",
        25 => "ENOTTY", 38 => "ENOSYS", 95 => "EOPNOTSUPP",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A raw return as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if ret < 0 { errname(-ret) } else { ret.to_string() } }

fn err<T>(msg: impl Into<String>) -> Result<T, CaseError> { Err(CaseError::Fail(msg.into())) }

/// The raw return of a call that returns a value, or a failure naming the call.
fn want(what: &str, ret: i64) -> Result<i64, CaseError> {
    if ret < 0 { err(format!("{what} failed with {}", errname(-ret))) } else { Ok(ret) }
}

/// A call that returns 0 when it succeeds: any other return fails, naming the call.
fn zero(what: &str, ret: i64) -> Checked {
    match ret {
        0 => Ok(()),
        r if r < 0 => Err(format!("{what} failed with {}", errname(-r))),
        r => Err(format!("{what} returned {r}, not 0")),
    }
}

fn want_eq(what: &str, got: i64, expected: i64) -> CaseResult {
    check(got == expected, &format!("{what}: expected {expected}, got {}", shown(got)))
}

fn want_err(what: &str, got: i64, errno: i64) -> CaseResult {
    check(got == -errno, &format!("{what}: expected {}, got {}", errname(errno), shown(got)))
}

fn clock_name(clock: i32) -> String {
    match clock {
        0 => "CLOCK_REALTIME".into(), 1 => "CLOCK_MONOTONIC".into(), 2 => "CLOCK_PROCESS_CPUTIME_ID".into(),
        3 => "CLOCK_THREAD_CPUTIME_ID".into(), 6 => "CLOCK_MONOTONIC_COARSE".into(),
        _ => format!("clock {clock}"),
    }
}

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }
fn kill(pid: i32, sig: i32) -> i64 { sc(nr::KILL, &[pid as i64 as u64, sig as i64 as u64]) }
fn setuid(uid: u32) -> i64 { sc(nr::SETUID, &[uid as u64]) }
fn wait4(pid: i32, status: *mut i32, options: i32) -> i64 {
    sc(nr::WAIT4, &[pid as i64 as u64, status as u64, options as u64, 0])
}

// ---------------------------------------------------------------------------
// Clocks and sleeps.

fn ts(ns: i64) -> Ts { [ns.div_euclid(NS), ns.rem_euclid(NS)] }
fn ns_of(t: &Ts) -> i64 { t[0] * NS + t[1] }

fn gettime_raw(clock: i32, t: &mut Ts) -> i64 {
    sc(nr::CLOCK_GETTIME, &[clock as i64 as u64, t.as_mut_ptr() as u64])
}

/// A clock's reading in nanoseconds.
fn clock_ns(clock: i32) -> Result<i64, String> {
    let mut t = [0i64; 2];
    zero(&format!("clock_gettime({})", clock_name(clock)), gettime_raw(clock, &mut t))?;
    Ok(ns_of(&t))
}

/// CLOCK_MONOTONIC in nanoseconds, for the suite's own timing; 0 if it cannot be read.
fn mono() -> i64 { clock_ns(CLOCK_MONOTONIC).unwrap_or(0) }
fn now_ms() -> u64 { (mono() / MS) as u64 }

fn getres_raw(clock: i32, t: Option<&mut Ts>) -> i64 {
    sc(nr::CLOCK_GETRES, &[clock as i64 as u64, t.map_or(0, |t| t.as_mut_ptr() as u64)])
}

fn settime_raw(clock: i32, t: &Ts) -> i64 {
    sc(nr::CLOCK_SETTIME, &[clock as i64 as u64, t.as_ptr() as u64])
}

fn nanosleep_raw(req: &Ts, rem: Option<&mut Ts>) -> i64 {
    sc(nr::NANOSLEEP, &[req.as_ptr() as u64, rem.map_or(0, |r| r.as_mut_ptr() as u64)])
}

fn clock_nanosleep_raw(clock: i32, flags: u64, req: &Ts, rem: Option<&mut Ts>) -> i64 {
    sc(nr::CLOCK_NANOSLEEP, &[clock as i64 as u64, flags, req.as_ptr() as u64, rem.map_or(0, |r| r.as_mut_ptr() as u64)])
}

/// `ms`, cut short so that `reserve` ms of the case's limit remain afterwards.
fn bounded(ms: u64, reserve: u64) -> u64 { ms.min(case_ms_left().saturating_sub(reserve)) }

/// A sleep for the suite's own pacing, resuming after any interruption.
fn pause_ms(ms: u64) {
    let end = mono() + ms as i64 * MS;
    loop {
        let left = end - mono();
        if left <= 0 { return; }
        let _ = nanosleep_raw(&ts(left.min(10 * MS)), None);
    }
}

fn nap() { let _ = nanosleep_raw(&ts(MS), None); }

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

/// Use the CPU for `ms` milliseconds of wall-clock time, mostly in user mode.
fn burn(ms: u64) { burn_until(ms, || false); }

/// Use the CPU until `cond` holds or `ms` milliseconds have passed; whether it held.
fn burn_until(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let start = now_ms();
    let mut x = 1u64;
    loop {
        if cond() { return true; }
        if now_ms().saturating_sub(start) >= ms { return false; }
        for i in 0..20_000u64 { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); }
        core::hint::black_box(x);
    }
}

/// How late `elapsed_ns` is against `want_ms`, in microseconds; reported and checked:
/// never early, and late by at most LATE_MS.
fn on_time(what: &str, elapsed_ns: i64, want_ms: i64) -> CaseResult {
    let late_us = (elapsed_ns - want_ms * MS) / 1000;
    value("late", late_us, "us", Some((0, LATE_MS * 1000)));
    check(elapsed_ns >= want_ms * MS, &format!("{what} ended after {} us, before the {want_ms} ms asked for", elapsed_ns / 1000))?;
    check(late_us <= LATE_MS * 1000, &format!("{what} ended {late_us} us late, more than {LATE_MS} ms"))
}

// ---------------------------------------------------------------------------
// Signals.

static COUNT: [AtomicU32; 65] = [const { AtomicU32::new(0) }; 65];
/// CLOCK_MONOTONIC when the first handled signal arrived.
static FIRST_NS: AtomicI64 = AtomicI64::new(0);
/// The most handled signals whose arrival is kept.
const STAMPS: usize = 32;
/// CLOCK_MONOTONIC when each of the first STAMPS handled signals arrived.
static STAMP_NS: [AtomicI64; STAMPS] = [const { AtomicI64::new(0) }; STAMPS];
static STAMPED: AtomicU32 = AtomicU32::new(0);

fn stamp() {
    let mut t = [0i64; 2];
    if gettime_raw(CLOCK_MONOTONIC, &mut t) == 0 {
        let now = ns_of(&t);
        let _ = FIRST_NS.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst);
        let i = STAMPED.fetch_add(1, Ordering::SeqCst) as usize;
        if i < STAMPS { STAMP_NS[i].store(now, Ordering::SeqCst); }
    }
}

/// When the kept handled signals arrived, in nanoseconds after `start`.
fn stamps(start: i64) -> Vec<i64> {
    let n = (STAMPED.load(Ordering::SeqCst) as usize).min(STAMPS);
    (0..n).map(|i| STAMP_NS[i].load(Ordering::SeqCst) - start).collect()
}

extern "C" fn on_sig(sig: i32) {
    stamp();
    if (1..=64).contains(&sig) { COUNT[sig as usize].fetch_add(1, Ordering::SeqCst); }
}

fn count(sig: i32) -> u32 { COUNT[sig as usize].load(Ordering::SeqCst) }

fn bit(sig: i32) -> u64 { 1u64 << (sig - 1) }

fn set_action(sig: i32, handler: u64, flags: u64) -> Checked {
    let act = Sigaction { handler, mask: 0, flags: flags | SA_RESTORER, restorer: restore_rt as usize as u64 };
    zero(&format!("sigaction({sig})"), sc(nr::RT_SIGACTION, &[sig as u64, &act as *const Sigaction as u64, 0, 8]))
}

/// Catch `sig` with a handler that counts it, without SA_RESTART.
fn catch(sig: i32) -> Checked { set_action(sig, on_sig as usize as u64, 0) }

fn procmask(how: i32, set: u64) -> Result<u64, String> {
    let set = [set];
    let mut old = [0u64];
    zero("sigprocmask", sc(nr::RT_SIGPROCMASK, &[how as u64, set.as_ptr() as u64, old.as_mut_ptr() as u64, 8]))?;
    Ok(old[0])
}
fn block(set: u64) -> Checked { procmask(SIG_BLOCK, set).map(|_| ()) }

fn pending() -> Result<u64, String> {
    let mut set = [0u64];
    zero("sigpending", sc(nr::RT_SIGPENDING, &[set.as_mut_ptr() as u64, 8]))?;
    Ok(set[0])
}

/// siginfo_t as the Linux ABI lays it out on both architectures: signo, errno and code,
/// then the union from byte 16. A timer's signal carries the timer's kernel ID and its
/// overrun count there, then si_value.
#[repr(C)]
#[derive(Clone, Copy)]
struct SigInfo { signo: i32, errno: i32, code: i32, pad: i32, fields: [u64; 14] }

impl SigInfo {
    const fn zero() -> SigInfo { SigInfo { signo: 0, errno: 0, code: 0, pad: 0, fields: [0; 14] } }
    fn overrun(&self) -> i32 { (self.fields[0] >> 32) as u32 as i32 }
    fn value(&self) -> u64 { self.fields[1] }
}

/// sigtimedwait as a C library makes it.
fn sigtimedwait(set: u64, info: &mut SigInfo, timeout_ms: u64) -> i64 {
    let set = [set];
    let t = ts(timeout_ms as i64 * MS);
    sc(nr::RT_SIGTIMEDWAIT, &[set.as_ptr() as u64, info as *mut SigInfo as u64, t.as_ptr() as u64, 8])
}

/// Wait up to `ms` for blocked `sig`; its siginfo, or a failure naming `what`.
fn await_signal(sig: i32, ms: u64, what: &str) -> Result<SigInfo, CaseError> {
    let mut info = SigInfo::zero();
    let ret = sigtimedwait(bit(sig), &mut info, bounded(ms, CLEANUP_MS));
    if ret == -EAGAIN { return err(format!("{what}: no signal {sig} within {ms} ms")); }
    want_eq(&format!("{what}: sigtimedwait"), ret, sig as i64)?;
    Ok(info)
}

/// Wait `ms` for blocked `sig` that must not come; fails if it does.
fn no_signal(sig: i32, ms: u64, what: &str) -> CaseResult {
    let mut info = SigInfo::zero();
    let ret = sigtimedwait(bit(sig), &mut info, bounded(ms, CLEANUP_MS));
    if ret == sig as i64 { return fail(format!("{what}: signal {sig} arrived (si_code {})", info.code)); }
    want_err(&format!("{what}: sigtimedwait"), ret, EAGAIN)
}

// ---------------------------------------------------------------------------
// POSIX timers.

/// struct sigevent as the Linux ABI lays it out: sigev_value, sigev_signo, sigev_notify,
/// then a union padding it to 64 bytes.
#[repr(C)]
struct SigEvent { value: u64, signo: i32, notify: i32, pad: [i32; 12] }

fn sigevent(notify: i32, signo: i32, value: u64) -> SigEvent { SigEvent { value, signo, notify, pad: [0; 12] } }

fn timer_create_raw(clock: i32, ev: Option<&SigEvent>, id: &mut i32) -> i64 {
    sc(nr::TIMER_CREATE, &[clock as i64 as u64, ev.map_or(0, |e| e as *const SigEvent as u64), id as *mut i32 as u64])
}

/// A timer the case created; dropping it deletes it.
struct Timer { id: i32 }

impl Timer {
    fn create(clock: i32, ev: Option<&SigEvent>) -> Result<Timer, CaseError> {
        let mut id = -1;
        zero(&format!("timer_create({})", clock_name(clock)), timer_create_raw(clock, ev, &mut id))?;
        Ok(Timer { id })
    }

    /// A CLOCK_MONOTONIC timer sending `sig` with `value`.
    fn signal(clock: i32, sig: i32, value: u64) -> Result<Timer, CaseError> {
        Timer::create(clock, Some(&sigevent(SIGEV_SIGNAL, sig, value)))
    }

    fn settime(&self, flags: u64, new: &Its, old: Option<&mut Its>) -> i64 {
        sc(nr::TIMER_SETTIME, &[self.id as i64 as u64, flags, new.as_ptr() as u64, old.map_or(0, |o| o.as_mut_ptr() as u64)])
    }

    fn arm(&self, interval_ns: i64, value_ns: i64) -> CaseResult {
        Ok(zero("timer_settime", self.settime(0, &its(interval_ns, value_ns), None))?)
    }

    fn get(&self) -> Result<Its, CaseError> {
        let mut cur = [0i64; 4];
        zero("timer_gettime", timer_gettime_raw(self.id, &mut cur))?;
        Ok(cur)
    }

    fn overrun(&self) -> i64 { sc(nr::TIMER_GETOVERRUN, &[self.id as i64 as u64]) }
}

impl Drop for Timer {
    fn drop(&mut self) { let _ = sc(nr::TIMER_DELETE, &[self.id as i64 as u64]); }
}

fn timer_gettime_raw(id: i32, cur: &mut Its) -> i64 {
    sc(nr::TIMER_GETTIME, &[id as i64 as u64, cur.as_mut_ptr() as u64])
}

fn its(interval_ns: i64, value_ns: i64) -> Its {
    let (i, v) = (ts(interval_ns), ts(value_ns));
    [i[0], i[1], v[0], v[1]]
}
fn its_interval(t: &Its) -> i64 { t[0] * NS + t[1] }
fn its_value(t: &Its) -> i64 { t[2] * NS + t[3] }

// ---------------------------------------------------------------------------
// alarm and the interval timers.

fn itv(interval_us: i64, value_us: i64) -> Itv {
    [interval_us / 1_000_000, interval_us % 1_000_000, value_us / 1_000_000, value_us % 1_000_000]
}
fn itv_interval(t: &Itv) -> i64 { t[0] * 1_000_000 + t[1] }
fn itv_value(t: &Itv) -> i64 { t[2] * 1_000_000 + t[3] }

fn setitimer(which: i32, new: &Itv, old: Option<&mut Itv>) -> i64 {
    sc(nr::SETITIMER, &[which as u64, new.as_ptr() as u64, old.map_or(0, |o| o.as_mut_ptr() as u64)])
}

fn getitimer(which: i32) -> Result<Itv, String> {
    let mut cur = [0i64; 4];
    zero("getitimer", sc(nr::GETITIMER, &[which as u64, cur.as_mut_ptr() as u64]))?;
    Ok(cur)
}

fn arm_itimer(which: i32, interval_us: i64, value_us: i64) -> Checked {
    zero("setitimer", setitimer(which, &itv(interval_us, value_us), None))
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
        let ret = setitimer(ITIMER_REAL, &new, Some(&mut old));
        if ret < 0 { return ret; }
        old[2] + (old[3] != 0) as i64
    }
}

/// Wait up to `ms` for `n` handled `sig`s; the CLOCK_MONOTONIC nanoseconds from `start`
/// to the first one.
fn handled(sig: i32, n: u32, start: i64, ms: u64) -> Option<i64> {
    until(ms, || count(sig) >= n).then(|| FIRST_NS.load(Ordering::SeqCst) - start)
}

// ---------------------------------------------------------------------------
// Processes.

// Wait statuses, decoded as a C library's macros decode them.
fn exited(s: i32) -> bool { s & 0x7f == 0 }
fn exit_code(s: i32) -> i32 { (s >> 8) & 0xff }
fn signaled(s: i32) -> bool { (((s & 0x7f) + 1) as i8 >> 1) > 0 }
fn term_sig(s: i32) -> i32 { s & 0x7f }
fn stopped(s: i32) -> bool { s & 0xff == 0x7f }
fn continued(s: i32) -> bool { s == 0xffff }
fn status_text(s: i32) -> String {
    if continued(s) { "continued".into() }
    else if stopped(s) { format!("stopped by signal {}", (s >> 8) & 0xff) }
    else if exited(s) { format!("exit {}", exit_code(s)) }
    else if signaled(s) { format!("death by signal {}", term_sig(s)) }
    else { format!("status {s:#x}") }
}

/// Poll waitpid(pid, options | WNOHANG) until it reports the child, for at most `ms`.
fn wait_within(pid: i32, options: i32, ms: u64) -> Result<i32, String> {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    loop {
        let mut status = 0;
        let r = wait4(pid, &mut status, options | WNOHANG);
        if r > 0 { return Ok(status); }
        if r < 0 { return Err(format!("waitpid({pid}) failed with {}", errname(-r))); }
        if now_ms().saturating_sub(start) >= ms {
            return Err(format!("waitpid({pid}) reported nothing within {ms} ms"));
        }
        nap();
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
    fn wait_for(&mut self, what: &str, ms: u64) -> Result<i32, CaseError> {
        let status = wait_within(self.pid, 0, ms).map_err(|e| CaseError::Fail(format!("{what} did not end: {e}")))?;
        self.live = false;
        Ok(status)
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.live {
            kill(self.pid, SIGKILL);
            let mut status = 0;
            let start = now_ms();
            while wait4(self.pid, &mut status, WNOHANG) == 0 && now_ms().saturating_sub(start) < bounded(1000, REPORT_MS) {
                nap();
            }
        }
    }
}

/// A child that sends `sig` to this process after `ms`; dropping it stops it.
fn send_after(ms: u64, sig: i32) -> Result<Child, CaseError> {
    send_at(mono() + ms as i64 * MS, sig)
}

/// A child that sends `sig` to this process once CLOCK_MONOTONIC reaches `at` ns.
fn send_at(at: i64, sig: i32) -> Result<Child, CaseError> {
    let target = pid();
    Child::start(move || {
        pause_ms(((at - mono()).max(0) / MS) as u64);
        while mono() < at { nap(); }
        kill(target, sig);
        0
    })
}

/// Run `f` in a child and report its result, for checks that change the process's
/// identity or may kill it.
fn in_child(f: impl FnOnce() -> Checked) -> CaseResult {
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(r);
        match f() {
            Ok(()) => 0,
            Err(m) => { let _ = io::write(w, m.as_bytes()); 1 }
        }
    });
    let _ = io::close(w);
    let mut child = match child {
        Ok(child) => child,
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    let status = child.wait_for("the child", WAIT_MS + 3000);
    let said = read_up_to(r, 512, 100).unwrap_or_default();
    let _ = io::close(r);
    let status = status?;
    if exited(status) && exit_code(status) == 0 { return Ok(()); }
    if exited(status) && exit_code(status) == 1 && !said.is_empty() {
        return fail(format!("in the child: {}", String::from_utf8_lossy(&said)));
    }
    fail(format!("the child ended with {}", status_text(status)))
}

/// Read until `want` bytes have arrived or end of file, for at most `ms`.
fn read_up_to(fd: Fd, want: usize, ms: u64) -> Result<Vec<u8>, String> {
    let ms = bounded(ms, REPORT_MS);
    let start = now_ms();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 512];
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

/// Whether every thread of `pid` is blocked, as /proc reports it.
fn is_parked(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .is_ok_and(|status| status.lines().any(|line| line == "State:\tBlocked"))
}

/// A child that runs `f` and sends back the two numbers it returns, or a message.
struct Probe { child: Child, out: Fd }

fn probe(f: impl FnOnce() -> Result<(i64, i64), String>) -> Result<Probe, CaseError> {
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(r);
        let said = match f() { Ok((a, b)) => format!("{a} {b}"), Err(m) => format!("!{m}") };
        let _ = io::write(w, said.as_bytes());
        0
    });
    let _ = io::close(w);
    match child {
        Ok(child) => Ok(Probe { child, out: r }),
        Err(e) => { let _ = io::close(r); Err(e) }
    }
}

impl Probe {
    fn result(mut self, ms: u64) -> Result<(i64, i64), CaseError> {
        let said = read_up_to(self.out, 512, ms)?;
        self.child.wait_for("the child", 1000)?;
        let said = String::from_utf8_lossy(&said).to_string();
        if let Some(m) = said.strip_prefix('!') { return err(format!("in the child: {m}")); }
        let mut words = said.split(' ').map(|w| w.parse::<i64>());
        match (words.next(), words.next()) {
            (Some(Ok(a)), Some(Ok(b))) => Ok((a, b)),
            _ => err(format!("the child's report was garbled: {said:?}")),
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) { let _ = io::close(self.out); }
}

/// The realtime clock as it was when the guard was made. `restore` sets it back,
/// advanced by the CLOCK_MONOTONIC time that has passed since, and checks that it took;
/// a guard dropped without `restore`, on an early return, sets it back unchecked. A
/// case killed before either is put right by the runner, which checks CLOCK_REALTIME
/// after every case.
struct RealtimeGuard { real: i64, mono: i64, restored: bool }

impl RealtimeGuard {
    fn new() -> Result<RealtimeGuard, CaseError> {
        Ok(RealtimeGuard { mono: clock_ns(CLOCK_MONOTONIC)?, real: clock_ns(CLOCK_REALTIME)?, restored: false })
    }

    fn now(&self) -> i64 { self.real + (mono() - self.mono) }

    fn restore(mut self) -> CaseResult {
        self.restored = true;
        zero("clock_settime putting CLOCK_REALTIME back", settime_raw(CLOCK_REALTIME, &ts(self.now())))?;
        let off = clock_ns(CLOCK_REALTIME)? - self.now();
        check(off.abs() <= 50 * MS, &format!("CLOCK_REALTIME read {} ms off after it was put back", off / MS))
    }
}

impl Drop for RealtimeGuard {
    fn drop(&mut self) {
        if !self.restored { let _ = settime_raw(CLOCK_REALTIME, &ts(self.now())); }
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

struct CArgs { _owned: Vec<CString>, ptrs: Vec<*const u8> }

fn cargs(items: &[String]) -> CArgs {
    let owned: Vec<CString> = items.iter().map(|s| CString::new(s.as_str()).expect("argument without NUL")).collect();
    let mut ptrs: Vec<*const u8> = owned.iter().map(|c| c.as_ptr() as *const u8).collect();
    ptrs.push(core::ptr::null());
    CArgs { _owned: owned, ptrs }
}

/// Fork a child that runs `setup` and then execs the helper as `report FD` followed by
/// the arguments `setup` returns. Returns the child, the report descriptor and when the
/// child was started, after checking the exec happened.
fn exec_helper(setup: impl FnOnce() -> Result<Vec<String>, String>) -> Result<(Child, Fd, i64), CaseError> {
    let path = CString::new(HELPER).expect("path without NUL");
    let (out_r, out_w) = io::pipe()?;
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let start = mono();
    let child = Child::start(|| {
        let _ = io::close(r);
        let _ = io::close(out_r);
        let extra = match setup() {
            Ok(extra) => extra,
            Err(m) => {
                let _ = io::write(w, format!("S{m}").as_bytes());
                return 126;
            }
        };
        let mut args = vec!["time-exec".to_string(), "report".to_string(), out_w.raw().to_string()];
        args.extend(extra);
        let argv = cargs(&args);
        let envp = cargs(&[]);
        let ret = sc(nr::EXECVE, &[path.as_ptr() as u64, argv.ptrs.as_ptr() as u64, envp.ptrs.as_ptr() as u64]);
        let _ = io::write(w, format!("E{}", -ret).as_bytes());
        127
    });
    let _ = io::close(w);
    let _ = io::close(out_w);
    let child = match child {
        Ok(child) => child,
        Err(e) => { let _ = io::close(r); let _ = io::close(out_r); return Err(e); }
    };
    let answer = read_up_to(r, 512, EXEC_MS);
    let _ = io::close(r);
    let failure = match answer {
        Err(e) => Some(format!("waiting for the exec: {e}")),
        Ok(answer) => match answer.first() {
            None => None,
            Some(b'E') => Some(format!("exec of {HELPER} failed with {}", errname(String::from_utf8_lossy(&answer[1..]).parse().unwrap_or(0)))),
            Some(b'S') => Some(format!("before the exec: {}", String::from_utf8_lossy(&answer[1..]))),
            _ => Some("the exec's report was garbled".to_string()),
        },
    };
    if let Some(failure) = failure {
        let _ = io::close(out_r);
        return err(failure);
    }
    Ok((child, out_r, start))
}

/// The helper's first line: its `key=value` words.
fn report_words(line: &str) -> std::collections::HashMap<String, String> {
    line.split_whitespace().filter_map(|w| w.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// An interval timer the helper reported, as (value, interval) in microseconds.
fn reported_itimer(words: &std::collections::HashMap<String, String>, key: &str) -> Result<(i64, i64), CaseError> {
    let text = words.get(key).ok_or_else(|| format!("the exec'd helper did not report {key}"))?;
    let parsed = text.split_once('/').and_then(|(v, i)| Some((v.parse().ok()?, i.parse().ok()?)));
    parsed.ok_or_else(|| CaseError::Fail(format!("the exec'd helper reported {key}={text}")))
}

// ---------------------------------------------------------------------------
// clocks & resolution

fn clk_realtime() -> CaseResult {
    let mut t = [0i64; 2];
    zero("clock_gettime(CLOCK_REALTIME)", gettime_raw(CLOCK_REALTIME, &mut t))?;
    value("realtime", t[0], "s", Some((1_577_836_800, 4_102_444_800)));
    check((0..NS).contains(&t[1]), &format!("tv_nsec is {}, outside 0..999999999", t[1]))?;
    check((1_577_836_800..4_102_444_800).contains(&t[0]), &format!("CLOCK_REALTIME reads {} s, not a date between 2020 and 2100", t[0]))
}

fn clk_monotonic() -> CaseResult {
    let mut t = [0i64; 2];
    zero("clock_gettime(CLOCK_MONOTONIC)", gettime_raw(CLOCK_MONOTONIC, &mut t))?;
    value("monotonic", ns_of(&t) / MS, "ms", None);
    check((0..NS).contains(&t[1]), &format!("tv_nsec is {}, outside 0..999999999", t[1]))?;
    check(t[0] >= 0, &format!("CLOCK_MONOTONIC reads a negative {} s", t[0]))
}

/// Up to `reads` reads of CLOCK_MONOTONIC in a row, stopping when `ms` have passed: how
/// many were made, how many went backwards and the largest step back.
fn backwards(reads: u32, ms: u64) -> Result<(u32, i64, i64), String> {
    let mut last = clock_ns(CLOCK_MONOTONIC)?;
    let end = last + ms as i64 * MS;
    let (mut back, mut worst) = (0i64, 0i64);
    for done in 0..reads {
        let now = clock_ns(CLOCK_MONOTONIC)?;
        if now < last { back += 1; worst = worst.max(last - now); }
        last = now;
        if now >= end { return Ok((done + 1, back, worst)); }
    }
    Ok((reads, back, worst))
}

fn clk_monotonic_steady() -> CaseResult {
    let start = mono();
    let (reads, back, worst) = backwards(100_000, bounded(u64::MAX, CLEANUP_MS))?;
    let took = mono() - start;
    value("backward", back, "", Some((0, 0)));
    value("read-cost", took / reads as i64, "ns", None);
    check(back == 0, &format!("{back} of {reads} reads went backwards, by up to {worst} ns"))?;
    check(reads == 100_000, &format!("only {reads} of 100000 reads fit in {} ms: each clock_gettime took {} us", took / MS, took / reads as i64 / 1000))
}

/// The latest CLOCK_MONOTONIC reading either thread has published, and what they saw.
static CPU_LATEST: AtomicI64 = AtomicI64::new(0);
static CPU_BACK: AtomicI64 = AtomicI64::new(0);
static CPU_WORST: AtomicI64 = AtomicI64::new(0);
static CPU_READS: AtomicI64 = AtomicI64::new(0);
static CPU_STOP: AtomicU32 = AtomicU32::new(0);
static HANDOFF: AtomicU64 = AtomicU64::new(0);

/// Read CLOCK_MONOTONIC after seeing the other thread's latest reading: a read that
/// follows another thread's publication must not be earlier than it.
fn cross_cpu_reads(until_ms: Option<u64>) {
    while CPU_STOP.load(Ordering::SeqCst) == 2 {
        let seen = CPU_LATEST.load(Ordering::SeqCst);
        let now = mono();
        if now < seen {
            CPU_BACK.fetch_add(1, Ordering::SeqCst);
            CPU_WORST.fetch_max(seen - now, Ordering::SeqCst);
        }
        CPU_LATEST.fetch_max(now, Ordering::SeqCst);
        CPU_READS.fetch_add(1, Ordering::Relaxed);
        if until_ms.is_some_and(|end| (now / MS) as u64 >= end) { CPU_STOP.store(1, Ordering::SeqCst); }
    }
}

/// Handoffs a pair must make within HANDOFF_MS to show they run at the same time: two
/// threads taking turns on one processor hand off no faster than the timer tick.
const HANDOFFS: u64 = 1000;
const HANDOFF_MS: u64 = 1000;

fn clk_monotonic_cpus() -> CaseResult {
    let cpus = processors()?;
    if cpus < 2 {
        return skip(format!("{cpus} processor online; the case needs 2"));
    }
    let thread = std::thread::spawn(|| {
        // Answer handoffs: an odd value becomes the next even one.
        loop {
            let v = HANDOFF.load(Ordering::SeqCst);
            if v == u64::MAX { break; }
            if v & 1 == 1 { HANDOFF.store(v + 1, Ordering::SeqCst); }
            if CPU_STOP.load(Ordering::SeqCst) == 2 { cross_cpu_reads(None); break; }
        }
    });
    let start = now_ms();
    let mut parallel = true;
    for i in 0..HANDOFFS {
        let odd = 2 * i + 1;
        HANDOFF.store(odd, Ordering::SeqCst);
        let mut spins = 0u64;
        while HANDOFF.load(Ordering::SeqCst) != odd + 1 {
            core::hint::spin_loop();
            spins += 1;
            if spins % 4096 == 0 && now_ms().saturating_sub(start) >= HANDOFF_MS { parallel = false; break; }
        }
        if !parallel { break; }
    }
    if !parallel {
        HANDOFF.store(u64::MAX, Ordering::SeqCst);
        let _ = thread.join();
        return fail(format!("with {cpus} processors online, two threads did not make {HANDOFFS} handoffs in {HANDOFF_MS} ms, so they never ran at once"));
    }
    // Both threads read for a second; this one ends it.
    CPU_STOP.store(2, Ordering::SeqCst);
    cross_cpu_reads(Some(now_ms() + 1000));
    let _ = thread.join();
    let (back, worst, reads) = (CPU_BACK.load(Ordering::SeqCst), CPU_WORST.load(Ordering::SeqCst), CPU_READS.load(Ordering::SeqCst));
    value("backward", back, "", Some((0, 0)));
    value("reads", reads, "", None);
    check(reads > 1000, &format!("the two threads made only {reads} reads in a second"))?;
    check(back == 0, &format!("{back} of {reads} reads were earlier than one another thread had already made, by up to {worst} ns"))
}

fn clk_monotonic_fine() -> CaseResult {
    let mut last = clock_ns(CLOCK_MONOTONIC)?;
    let mut repeats = 0i64;
    for _ in 0..10_000 {
        let now = clock_ns(CLOCK_MONOTONIC)?;
        if now == last { repeats += 1; }
        last = now;
    }
    value("repeats", repeats, "", Some((0, 100)));
    check(repeats <= 100, &format!("{repeats} of 10000 consecutive reads repeated the one before: the clock moves in steps coarser than a system call"))
}

/// Read CLOCK_MONOTONIC and `clock` together: the other clock and the monotonic reading
/// halfway across it.
fn paired(clock: i32) -> Result<(i64, i64), String> {
    let before = clock_ns(CLOCK_MONOTONIC)?;
    let other = clock_ns(clock)?;
    let after = clock_ns(CLOCK_MONOTONIC)?;
    Ok((other, before + (after - before) / 2))
}

/// Parts per million by which `measured` differs from `reference`.
fn ppm(measured: i64, reference: i64) -> i64 {
    if reference == 0 { return i64::MAX; }
    ((measured - reference) as i128 * 1_000_000 / reference as i128) as i64
}

/// Resolve the 100 ppm comparison: two 100 us brackets contribute at most
/// 100 us of midpoint uncertainty over the two-second interval (50 ppm).
/// Preempted pairings are discarded, never accepted as clock-rate evidence.
fn realtime_pair() -> Result<(i64, i64, i64), String> {
    const MAX_BRACKET_NS: i64 = 100_000;
    let start = now_ms();
    loop {
        let before = clock_ns(CLOCK_MONOTONIC)?;
        let real = clock_ns(CLOCK_REALTIME)?;
        let after = clock_ns(CLOCK_MONOTONIC)?;
        let width = after - before;
        if (0..=MAX_BRACKET_NS).contains(&width) {
            return Ok((real, before + width / 2, width));
        }
        if now_ms().saturating_sub(start) >= 1000 {
            return Err(format!("no realtime pairing bracket at most {MAX_BRACKET_NS} ns in 1 s; last bracket {width} ns"));
        }
    }
}

fn clk_rate_realtime() -> CaseResult {
    let (r0, m0, w0) = realtime_pair()?;
    wait_for(2000, "drift");
    pause_ms(2000);
    let (r1, m1, w1) = realtime_pair()?;
    value("start-bracket", w0, "ns", Some((0, 100_000)));
    value("end-bracket", w1, "ns", Some((0, 100_000)));
    let drift = ppm(r1 - r0, m1 - m0);
    value("drift", drift, "ppm", Some((-100, 100)));
    check(drift.abs() <= 100, &format!("over {} ms of CLOCK_MONOTONIC, CLOCK_REALTIME advanced {} ms: {drift} ppm apart", (m1 - m0) / MS, (r1 - r0) / MS))
}

/// The processor's own counter, read in user mode: CNTVCT_EL0 on ARM64.
#[cfg(target_arch = "aarch64")]
fn counter_now() -> u64 {
    let count: u64;
    // SAFETY: a read of the virtual counter, which Linux lets user mode make.
    unsafe { core::arch::asm!("isb", "mrs {c}, cntvct_el0", c = out(reg) count, options(nostack)) };
    count
}

/// The processor's own counter, read in user mode: the TSC on x86-64.
#[cfg(target_arch = "x86_64")]
fn counter_now() -> u64 {
    // SAFETY: RDTSC is available in user mode on every x86-64 processor.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// The processor's own counter and its frequency in Hz, or why there is none.
#[cfg(target_arch = "aarch64")]
fn counter() -> Result<(u64, u64), String> {
    let count = counter_now();
    let freq: u64;
    // SAFETY: a read of the counter's frequency, which Linux lets user mode make.
    unsafe { core::arch::asm!("mrs {f}, cntfrq_el0", f = out(reg) freq, options(nostack)) };
    if freq == 0 { return Err("CNTFRQ_EL0 reads 0".into()); }
    Ok((count, freq))
}

#[cfg(target_arch = "x86_64")]
fn counter() -> Result<(u64, u64), String> {
    // SAFETY: CPUID is available in user mode on every x86-64 processor.
    let (max, leaf) = unsafe { (core::arch::x86_64::__cpuid(0).eax, core::arch::x86_64::__cpuid(0x15)) };
    let freq = if max >= 0x15 && leaf.eax != 0 && leaf.ebx != 0 && leaf.ecx != 0 {
        leaf.ecx as u64 * leaf.ebx as u64 / leaf.eax as u64
    } else {
        // KVM/QEMU and VMware publish the vCPU's TSC frequency in kHz in
        // this CPUID leaf. It comes from the hypervisor, independently of
        // the kernel's PIT calibration and monotonic-clock conversion.
        let hyper = unsafe { core::arch::x86_64::__cpuid(0x4000_0000) };
        let vendor = [hyper.ebx, hyper.ecx, hyper.edx];
        let kvm = vendor == [0x4b4d_564b, 0x564b_4d56, 0x0000_004d];
        let vmware = vendor == [0x6177_4d56, 0x4d56_6572, 0x6572_6177];
        if hyper.eax < 0x4000_0010 || !(kvm || vmware) {
            return Err("CPUID gives no independent TSC frequency".into());
        }
        let khz = unsafe { core::arch::x86_64::__cpuid(0x4000_0010).eax };
        if khz == 0 { return Err("CPUID hypervisor TSC frequency is zero".into()); }
        khz as u64 * 1000
    };
    Ok((counter_now(), freq))
}

/// The counter read halfway across a CLOCK_MONOTONIC pair: (count, monotonic ns, Hz).
fn counter_pair() -> Result<(u64, i64, u64), String> {
    let before = clock_ns(CLOCK_MONOTONIC)?;
    let (count, freq) = counter()?;
    let after = clock_ns(CLOCK_MONOTONIC)?;
    Ok((count, before + (after - before) / 2, freq))
}

fn clk_rate_counter() -> CaseResult {
    let (c0, m0, freq) = match counter_pair() {
        Ok(read) => read,
        Err(why) if cfg!(target_arch = "x86_64") && why.starts_with("CPUID") => return skip(why),
        Err(why) => return fail(why),
    };
    wait_for(2000, "drift");
    pause_ms(2000);
    let (c1, m1, _) = counter_pair()?;
    let counted = ((c1.wrapping_sub(c0)) as u128 * NS as u128 / freq as u128) as i64;
    let drift = ppm(m1 - m0, counted);
    value("frequency", freq as i64, "hz", None);
    value("drift", drift, "ppm", Some((-1000, 1000)));
    check(drift.abs() <= 1000, &format!("over {} ms of the counter at {freq} Hz, CLOCK_MONOTONIC advanced {} ms: {drift} ppm apart", counted / MS, (m1 - m0) / MS))
}

/// The most processors the counter case follows.
const MAX_CPUS: usize = 64;
/// Threads of the counter case that have finished, and those that read on their processor.
static CPU_DONE: AtomicU32 = AtomicU32::new(0);
static CPU_READ: AtomicU32 = AtomicU32::new(0);
/// Why the counter case's threads that failed did.
static CPU_FAILURES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The processor the calling thread runs on, from getcpu.
fn getcpu() -> Result<u32, String> {
    let mut cpu = [u32::MAX];
    zero("getcpu", sc(nr::GETCPU, &[cpu.as_mut_ptr() as u64, 0, 0]))?;
    Ok(cpu[0])
}

/// Pin the calling thread to processor `cpu` with sched_setaffinity, check with getcpu
/// that it runs there, read the counter twice and check again that it ran there.
fn read_counter_on(cpu: usize) -> Result<(), String> {
    let mask = [1u64 << cpu];
    zero(&format!("sched_setaffinity(processor {cpu})"), sc(nr::SCHED_SETAFFINITY, &[0, 8, mask.as_ptr() as u64]))?;
    let before = getcpu()?;
    let first = counter_now();
    let second = counter_now();
    let after = getcpu()?;
    if before != cpu as u32 || after != cpu as u32 {
        return Err(format!("the thread pinned to processor {cpu} ran on processor {before}, then {after}"));
    }
    if second < first { return Err(format!("on processor {cpu} the counter went back from {first} to {second}")); }
    Ok(())
}

/// One thread per online processor, each pinned to its processor with sched_setaffinity
/// and confirmed there by getcpu, reads the counter from user mode.
fn clk_counter_cpus() -> CaseResult {
    let cpus = processors()?;
    check(cpus <= MAX_CPUS, &format!("{cpus} processors are online, more than the {MAX_CPUS} the case follows"))?;
    for cpu in 0..cpus {
        std::thread::spawn(move || {
            match read_counter_on(cpu) {
                Ok(()) => { CPU_READ.fetch_add(1, Ordering::SeqCst); }
                Err(why) => CPU_FAILURES.lock().unwrap_or_else(|e| e.into_inner()).push(why),
            }
            CPU_DONE.fetch_add(1, Ordering::SeqCst);
        });
    }
    let finished = until(WAIT_MS, || CPU_DONE.load(Ordering::SeqCst) as usize == cpus);
    let read = CPU_READ.load(Ordering::SeqCst) as i64;
    value("processors", read, "", Some((cpus as i64, cpus as i64)));
    let failures = CPU_FAILURES.lock().unwrap_or_else(|e| e.into_inner()).join("; ");
    check(failures.is_empty(), &failures)?;
    check(finished, &format!("only {} of the {cpus} threads finished within {WAIT_MS} ms", CPU_DONE.load(Ordering::SeqCst)))
}

/// What user mode may and may not read of the ARM64 generic timer, as Linux grants it:
/// the virtual counter and its frequency, and neither the physical counter nor either
/// timer's registers.
#[cfg(target_arch = "aarch64")]
mod el0_timer {
    use super::*;

    const SIGILL: i32 = 4;
    const ILL_ILLOPC: i32 = 1;
    const SA_SIGINFO: u64 = 4;
    /// What a trapped read leaves in its destination register.
    const UNREAD: u64 = 0x5e5e;
    /// The ESR record in an ARM64 signal frame's __reserved area.
    const ESR_MAGIC: u32 = 0x4553_5201;
    /// Offsets in the Linux ARM64 ucontext_t: uc_mcontext.pc and uc_mcontext.__reserved.
    const UC_PC: usize = 440;
    const UC_RESERVED: usize = 464;
    const RESERVED_BYTES: usize = 4096;

    /// Read a system register from user mode, starting from UNREAD: the address of the
    /// mrs and the register it read into.
    macro_rules! el0_read {
        ($name:ident, $reg:literal) => {
            fn $name() -> (u64, u64) {
                let (at, got): (u64, u64);
                // SAFETY: a read of a system register into a general register; when the
                // kernel traps it, the case's SIGILL handler resumes after it.
                unsafe {
                    core::arch::asm!(
                        "adr {at}, 2f",
                        "mov {got}, #0x5e5e",
                        "2:",
                        concat!("mrs {got}, ", $reg),
                        at = out(reg) at, got = out(reg) got, options(nostack),
                    );
                }
                (at, got)
            }
        };
    }
    el0_read!(cntpct, "cntpct_el0");
    el0_read!(cntv_ctl, "cntv_ctl_el0");
    el0_read!(cntp_ctl, "cntp_ctl_el0");
    el0_read!(cntvct, "cntvct_el0");
    el0_read!(cntfrq, "cntfrq_el0");

    const DENIED: [(&str, fn() -> (u64, u64)); 3] = [("CNTPCT_EL0", cntpct), ("CNTV_CTL_EL0", cntv_ctl), ("CNTP_CTL_EL0", cntp_ctl)];
    const GRANTED: [(&str, fn() -> (u64, u64)); 2] = [("CNTVCT_EL0", cntvct), ("CNTFRQ_EL0", cntfrq)];

    /// SIGILLs handled, and the last one's si_code, si_addr and ESR exception class (-1
    /// when its frame carried no ESR record).
    static ILLS: AtomicU32 = AtomicU32::new(0);
    static ILL_CODE: AtomicI64 = AtomicI64::new(0);
    static ILL_ADDR: AtomicU64 = AtomicU64::new(0);
    static ILL_CLASS: AtomicI64 = AtomicI64::new(-1);

    /// Note the SIGILL and resume at the instruction after the one that trapped.
    extern "C" fn on_ill(_sig: i32, info: *const SigInfo, uc: *mut u8) {
        // SAFETY: the kernel passes the siginfo and the ucontext_t it wrote for this
        // delivery, laid out as Linux's: uc_mcontext.pc at UC_PC, then the records of
        // __reserved from UC_RESERVED, each a magic and a size, ending in a zero magic.
        unsafe {
            ILL_CODE.store((*info).code as i64, Ordering::SeqCst);
            ILL_ADDR.store((*info).fields[0], Ordering::SeqCst);
            let mut class = -1;
            let mut at = UC_RESERVED;
            while at + 8 <= UC_RESERVED + RESERVED_BYTES {
                let magic = (uc.add(at) as *const u32).read_unaligned();
                let size = (uc.add(at + 4) as *const u32).read_unaligned() as usize;
                if magic == 0 || size < 8 { break; }
                if magic == ESR_MAGIC && size >= 16 { class = ((uc.add(at + 8) as *const u64).read_unaligned() >> 26) as i64; }
                at += size;
            }
            ILL_CLASS.store(class, Ordering::SeqCst);
            let pc = uc.add(UC_PC) as *mut u64;
            pc.write_unaligned(pc.read_unaligned() + 4);
        }
        ILLS.fetch_add(1, Ordering::SeqCst);
    }

    pub fn trap_default() -> CaseResult {
        let mut killed = 0;
        for (name, read) in DENIED {
            let mut kid = Child::start(|| { let _ = read(); 0 })?;
            let status = kid.wait_for(&format!("the child reading {name}"), WAIT_MS)?;
            check(signaled(status) && term_sig(status) == SIGILL,
                &format!("a child that read {name} ended with {}, not death by SIGILL", status_text(status)))?;
            killed += 1;
        }
        value("killed", killed, "", Some((3, 3)));
        Ok(())
    }

    pub fn trap_caught() -> CaseResult {
        set_action(SIGILL, on_ill as usize as u64, SA_SIGINFO)?;
        for (name, read) in GRANTED {
            let _ = read();
            check(ILLS.load(Ordering::SeqCst) == 0, &format!("reading {name} raised SIGILL"))?;
        }
        let mut handled = 0;
        for (name, read) in DENIED {
            let before = ILLS.load(Ordering::SeqCst);
            let (at, got) = read();
            let n = ILLS.load(Ordering::SeqCst) - before;
            check(n == 1, &format!("reading {name} raised SIGILL {n} times, not once"))?;
            check(got == UNREAD, &format!("the trapped read of {name} put {got:#x} in its register"))?;
            let code = ILL_CODE.load(Ordering::SeqCst);
            check(code == ILL_ILLOPC as i64, &format!("the SIGILL for {name} has si_code {code}, not ILL_ILLOPC"))?;
            let addr = ILL_ADDR.load(Ordering::SeqCst);
            check(addr == at, &format!("the SIGILL for {name} has si_addr {addr:#x}, not the mrs at {at:#x}"))?;
            let class = ILL_CLASS.load(Ordering::SeqCst);
            check(class == -1 || class == 0x18, &format!("the SIGILL for {name} carries ESR class {class:#x}, not 0x18"))?;
            handled += 1;
        }
        value("handled", handled, "", Some((3, 3)));
        Ok(())
    }
}

/// The x86 counterpart: RDTSC is granted, but reading counter/timer MSRs
/// at CPL3 raises #GP. Linux delivers SIGSEGV/SI_KERNEL for that trap.
#[cfg(target_arch = "x86_64")]
mod user_timer_msr {
    use super::*;
    const SIGSEGV: i32 = 11;
    const SI_KERNEL: i32 = 128;
    const UNREAD: u32 = 0x5e5e;
    const UC_RIP: usize = 40 + 16 * 8;
    const MSRS: [(&str, u32); 3] = [("IA32_TSC", 0x10), ("IA32_TSC_DEADLINE", 0x6e0), ("X2APIC_INITIAL_COUNT", 0x838)];
    static TRAPS: AtomicU32 = AtomicU32::new(0);
    static CODE: AtomicI64 = AtomicI64::new(0);
    static ADDR: AtomicU64 = AtomicU64::new(0);
    static PC: AtomicU64 = AtomicU64::new(0);

    fn read(msr: u32) -> (u64, u32, u32) {
        let at: u64;
        let (mut lo, mut hi) = (UNREAD, UNREAD);
        // SAFETY: deliberate CPL3 privilege fault. The caught case advances
        // the saved RIP by exactly the two bytes of RDMSR.
        unsafe { core::arch::asm!("lea {at}, [rip + 2f]", "2:", "rdmsr",
            at = out(reg) at, in("ecx") msr, inout("eax") lo, inout("edx") hi, options(nostack)); }
        (at, lo, hi)
    }
    extern "C" fn caught(_sig: i32, info: *const SigInfo, uc: *mut u8) {
        unsafe {
            CODE.store((*info).code as i64, Ordering::SeqCst);
            ADDR.store((*info).fields[0], Ordering::SeqCst);
            let pc = uc.add(UC_RIP) as *mut u64;
            let fault = pc.read_unaligned();
            PC.store(fault, Ordering::SeqCst);
            pc.write_unaligned(fault + 2);
        }
        TRAPS.fetch_add(1, Ordering::SeqCst);
    }
    pub fn trap_default() -> CaseResult {
        let mut killed = 0;
        for (name, msr) in MSRS {
            let mut child = Child::start(|| { let _ = read(msr); 0 })?;
            let status = child.wait_for(name, WAIT_MS)?;
            check(signaled(status) && term_sig(status) == SIGSEGV,
                &format!("RDMSR({name}) ended with {}, not SIGSEGV", status_text(status)))?;
            killed += 1;
        }
        value("killed", killed, "", Some((3, 3)));
        Ok(())
    }
    pub fn trap_caught() -> CaseResult {
        set_action(SIGSEGV, caught as usize as u64, 4)?; // SA_SIGINFO
        let _ = counter_now();
        check(TRAPS.load(Ordering::SeqCst) == 0, "RDTSC raised SIGSEGV")?;
        for (name, msr) in MSRS {
            let before = TRAPS.load(Ordering::SeqCst);
            let (at, lo, hi) = read(msr);
            check(TRAPS.load(Ordering::SeqCst) == before + 1, &format!("RDMSR({name}) did not raise exactly one SIGSEGV"))?;
            check(lo == UNREAD && hi == UNREAD, &format!("RDMSR({name}) modified its destination registers"))?;
            check(CODE.load(Ordering::SeqCst) == SI_KERNEL as i64, &format!("RDMSR({name}) did not carry SI_KERNEL"))?;
            check(ADDR.load(Ordering::SeqCst) == 0, &format!("RDMSR({name}) did not carry a zero si_addr"))?;
            check(PC.load(Ordering::SeqCst) == at, &format!("RDMSR({name}) did not preserve its faulting instruction address"))?;
        }
        value("handled", TRAPS.load(Ordering::SeqCst) as i64, "", Some((3, 3)));
        Ok(())
    }
}

fn clk_counter_trap_default() -> CaseResult {
    #[cfg(target_arch = "aarch64")]
    { el0_timer::trap_default() }
    #[cfg(target_arch = "x86_64")]
    { user_timer_msr::trap_default() }
}

fn clk_counter_trap_caught() -> CaseResult {
    #[cfg(target_arch = "aarch64")]
    { el0_timer::trap_caught() }
    #[cfg(target_arch = "x86_64")]
    { user_timer_msr::trap_caught() }
}

/// The RTC's time as Linux's RTC_RD_TIME reports it: seconds of the day and the seconds field.
fn rtc_read(fd: Fd) -> Result<i32, String> {
    let mut tm = [0i32; 9];
    zero("ioctl(RTC_RD_TIME)", sc(nr::IOCTL, &[fd.raw(), RTC_RD_TIME, tm.as_mut_ptr() as u64]))?;
    Ok(tm[0])
}

/// Wait for the RTC's seconds field to change; CLOCK_REALTIME and CLOCK_MONOTONIC when it did.
fn rtc_edge(fd: Fd, ms: u64) -> Result<(i64, i64), String> {
    let first = rtc_read(fd)?;
    let start = now_ms();
    loop {
        let (real, mono_at) = paired(CLOCK_REALTIME)?;
        if rtc_read(fd)? != first { return Ok((real, mono_at)); }
        if now_ms().saturating_sub(start) >= ms { return Err(format!("the RTC's seconds did not change within {ms} ms")); }
        nap();
    }
}

fn clk_rate_rtc() -> CaseResult {
    let fd = libbreenix::fs::open("/dev/rtc0", libbreenix::fs::O_RDONLY).map_err(|e| format!("open(/dev/rtc0) failed: {e}"))?;
    let result = (|| -> CaseResult {
        let (r0, _) = rtc_edge(fd, 1500)?;
        wait_for(3000, "drift");
        let mut r1 = r0;
        for _ in 0..3 { r1 = rtc_edge(fd, 1500)?.0; }
        let drift = ppm(r1 - r0, 3 * NS);
        value("drift", drift, "ppm", Some((-2000, 2000)));
        check(drift.abs() <= 2000, &format!("over three RTC seconds CLOCK_REALTIME advanced {} us: {drift} ppm apart", (r1 - r0) / 1000))
    })();
    let _ = io::close(fd);
    result
}

fn clk_cputime_process() -> CaseResult {
    let c0 = clock_ns(CLOCK_PROCESS_CPUTIME_ID)?;
    wait_for(300, "asleep");
    pause_ms(300);
    let c1 = clock_ns(CLOCK_PROCESS_CPUTIME_ID)?;
    burn(300);
    let c2 = clock_ns(CLOCK_PROCESS_CPUTIME_ID)?;
    let (asleep, busy) = ((c1 - c0) / 1000, (c2 - c1) / 1000);
    value("asleep", asleep, "us", Some((0, 20_000)));
    value("busy", busy, "us", Some((100_000, (300 + LATE_MS) * 1000)));
    check((0..=20_000).contains(&asleep), &format!("the clock advanced {asleep} us while the process slept 300 ms"))?;
    check((100_000..=(300 + LATE_MS) * 1000).contains(&busy), &format!("the clock advanced {busy} us while the process computed for 300 ms"))
}

fn clk_cputime_thread() -> CaseResult {
    let t0 = clock_ns(CLOCK_THREAD_CPUTIME_ID)?;
    let p0 = clock_ns(CLOCK_PROCESS_CPUTIME_ID)?;
    let other = std::thread::spawn(|| burn(300));
    wait_for(300, "waiting");
    let _ = other.join();
    let t1 = clock_ns(CLOCK_THREAD_CPUTIME_ID)?;
    let p1 = clock_ns(CLOCK_PROCESS_CPUTIME_ID)?;
    burn(200);
    let t2 = clock_ns(CLOCK_THREAD_CPUTIME_ID)?;
    let (idle, process, own) = ((t1 - t0) / 1000, (p1 - p0) / 1000, (t2 - t1) / 1000);
    value("waiting", idle, "us", Some((0, 20_000)));
    value("own", own, "us", Some((60_000, (200 + LATE_MS) * 1000)));
    check((0..=20_000).contains(&idle), &format!("the thread's clock advanced {idle} us while another thread computed and it waited"))?;
    check(process >= 100_000, &format!("the process's clock advanced only {process} us while another of its threads computed for 300 ms"))?;
    check((60_000..=(200 + LATE_MS) * 1000).contains(&own), &format!("the thread's clock advanced {own} us while it computed for 200 ms"))
}

fn clk_invalid() -> CaseResult {
    for clock in [16, CLOCK_UNKNOWN] {
        let mut t = [0i64; 2];
        want_err(&format!("clock_gettime({clock})"), gettime_raw(clock, &mut t), EINVAL)?;
    }
    Ok(())
}

fn clk_efault() -> CaseResult {
    want_err("clock_gettime into address 16", sc(nr::CLOCK_GETTIME, &[CLOCK_MONOTONIC as u64, 16]), EFAULT)
}

fn clk_getres() -> CaseResult {
    for clock in [CLOCK_REALTIME, CLOCK_MONOTONIC, CLOCK_PROCESS_CPUTIME_ID, CLOCK_THREAD_CPUTIME_ID] {
        let mut t = [0i64; 2];
        zero(&format!("clock_getres({})", clock_name(clock)), getres_raw(clock, Some(&mut t)))?;
        let res = ns_of(&t);
        if clock == CLOCK_REALTIME { value("realtime", res, "ns", Some((1, 20 * MS))); }
        if clock == CLOCK_MONOTONIC { value("monotonic", res, "ns", Some((1, 20 * MS))); }
        check(res > 0 && res <= 20 * MS, &format!("clock_getres({}) reports {res} ns", clock_name(clock)))?;
    }
    Ok(())
}

fn clk_getres_null() -> CaseResult {
    want_eq("clock_getres(CLOCK_MONOTONIC, NULL)", getres_raw(CLOCK_MONOTONIC, None), 0)
}

fn clk_getres_einval() -> CaseResult {
    let mut t = [0i64; 2];
    want_err(&format!("clock_getres({CLOCK_UNKNOWN})"), getres_raw(CLOCK_UNKNOWN, Some(&mut t)), EINVAL)
}

fn clk_coarse_tick() -> CaseResult {
    let mut t = [0i64; 2];
    zero("clock_getres(CLOCK_MONOTONIC_COARSE)", getres_raw(CLOCK_MONOTONIC_COARSE, Some(&mut t)))?;
    let res = ns_of(&t);
    value("resolution", res / 1000, "us", Some((TICK_MS * 1000, TICK_MS * 1000)));
    let mut steps = Vec::new();
    let mut last = clock_ns(CLOCK_MONOTONIC_COARSE)?;
    let start = now_ms();
    while now_ms().saturating_sub(start) < 200 && steps.len() < 200 {
        let now = clock_ns(CLOCK_MONOTONIC_COARSE)?;
        if now != last { steps.push(now - last); last = now; }
    }
    steps.sort_unstable();
    let step = steps.get(steps.len() / 2).copied().unwrap_or(0);
    value("step", step / 1000, "us", Some((TICK_MS * 1000, TICK_MS * 1000)));
    check(res == TICK_MS * MS, &format!("clock_getres(CLOCK_MONOTONIC_COARSE) reports {res} ns, not the {TICK_MS} ms tick"))?;
    check(step == res, &format!("the coarse clock's median step is {step} ns, not its {res} ns resolution"))
}

fn clk_settime() -> CaseResult {
    let guard = RealtimeGuard::new()?;
    let result = (|| -> CaseResult {
        let target = (clock_ns(CLOCK_REALTIME)? / NS + 1000) * NS + 500 * MS;
        let m0 = clock_ns(CLOCK_MONOTONIC)?;
        want_eq("clock_settime(CLOCK_REALTIME)", settime_raw(CLOCK_REALTIME, &ts(target)), 0)?;
        let real = clock_ns(CLOCK_REALTIME)?;
        let m1 = clock_ns(CLOCK_MONOTONIC)?;
        let error = (real - target) / 1000;
        value("error", error, "us", Some((0, 50_000)));
        value("monotonic-step", (m1 - m0) / 1000, "us", Some((0, 50_000)));
        check((0..=50_000).contains(&error), &format!("after setting {} s {} ns, CLOCK_REALTIME read {} s {} ns", target / NS, target % NS, real / NS, real % NS))?;
        check((0..=50 * MS).contains(&(m1 - m0)), &format!("CLOCK_MONOTONIC moved {} us when CLOCK_REALTIME was set", (m1 - m0) / 1000))
    })();
    result.and(guard.restore())
}

fn clk_settime_eperm() -> CaseResult {
    let guard = RealtimeGuard::new()?;
    let result = in_child(|| {
        zero("setuid", setuid(USER_A))?;
        let before = clock_ns(CLOCK_REALTIME)?;
        let ret = settime_raw(CLOCK_REALTIME, &ts(before + 1000 * NS));
        let after = clock_ns(CLOCK_REALTIME)?;
        if ret != -EPERM { return Err(format!("clock_settime as user {USER_A}: expected EPERM, got {}", shown(ret))); }
        if !(0..=50 * MS).contains(&(after - before)) {
            return Err(format!("CLOCK_REALTIME moved {} us across the refused clock_settime", (after - before) / 1000));
        }
        Ok(())
    });
    result.and(guard.restore())
}

fn clk_settime_einval() -> CaseResult {
    let guard = RealtimeGuard::new()?;
    let result = (|| -> CaseResult {
        let now = clock_ns(CLOCK_REALTIME)?;
        let secs = now / NS;
        want_err("clock_settime with tv_nsec 1000000000", settime_raw(CLOCK_REALTIME, &[secs, NS]), EINVAL)?;
        want_err("clock_settime with tv_nsec -1", settime_raw(CLOCK_REALTIME, &[secs, -1]), EINVAL)?;
        want_err("clock_settime(CLOCK_MONOTONIC)", settime_raw(CLOCK_MONOTONIC, &ts(mono())), EINVAL)?;
        want_err(&format!("clock_settime({CLOCK_UNKNOWN})"), settime_raw(CLOCK_UNKNOWN, &ts(now)), EINVAL)
    })();
    result.and(guard.restore())
}

fn clk_gettimeofday() -> CaseResult {
    let before = clock_ns(CLOCK_REALTIME)? / 1000;
    let mut tv = [0i64; 2];
    zero("gettimeofday", sc(nr::GETTIMEOFDAY, &[tv.as_mut_ptr() as u64, 0]))?;
    let after = clock_ns(CLOCK_REALTIME)? / 1000;
    let got = tv[0] * 1_000_000 + tv[1];
    value("behind-realtime", after - got, "us", None);
    check((0..1_000_000).contains(&tv[1]), &format!("tv_usec is {}", tv[1]))?;
    check(before <= got && got <= after, &format!("gettimeofday read {got} us, outside the CLOCK_REALTIME reads {before}..{after} us around it"))
}

/// time() as a C library makes it: the time call, or CLOCK_REALTIME's seconds.
fn time_call(stored: &mut i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    { sc(nr::TIME, &[stored as *mut i64 as u64]) }
    #[cfg(target_arch = "aarch64")]
    {
        let mut t = [0i64; 2];
        let ret = gettime_raw(CLOCK_REALTIME, &mut t);
        if ret < 0 { return ret; }
        *stored = t[0];
        t[0]
    }
}

fn clk_time() -> CaseResult {
    let before = clock_ns(CLOCK_REALTIME)? / NS;
    let mut stored = -1i64;
    let got = want("time", time_call(&mut stored))?;
    let after = clock_ns(CLOCK_REALTIME)? / NS;
    check(stored == got, &format!("time returned {got} and stored {stored}"))?;
    check(before <= got && got <= after, &format!("time returned {got}, outside the CLOCK_REALTIME seconds {before}..{after} around it"))
}

// ---------------------------------------------------------------------------
// nanosleep & clock_nanosleep

/// Sleep with `sleep` and check it took `ms`, neither early nor too late.
fn timed_sleep(what: &str, ms: i64, sleep: impl FnOnce() -> i64) -> CaseResult {
    wait_for(ms as u64, "late");
    let start = mono();
    let ret = sleep();
    let elapsed = mono() - start;
    want_eq(what, ret, 0)?;
    on_time(what, elapsed, ms)
}

fn sl_nanosleep() -> CaseResult {
    timed_sleep("nanosleep(100 ms)", 100, || nanosleep_raw(&ts(100 * MS), None))
}

fn sl_nanosleep_seconds() -> CaseResult {
    timed_sleep("nanosleep(1 s 250000000 ns)", 1250, || nanosleep_raw(&[1, 250 * MS], None))
}

fn sl_nanosleep_short() -> CaseResult {
    let mut lates = Vec::new();
    wait_for(20 * (1 + TICK_MS as u64), "median-late");
    for _ in 0..20 {
        let start = mono();
        want_eq("nanosleep(1 ms)", nanosleep_raw(&ts(MS), None), 0)?;
        let elapsed = mono() - start;
        check(elapsed >= MS, &format!("a 1 ms nanosleep ended after {} us", elapsed / 1000))?;
        lates.push((elapsed - MS) / 1000);
    }
    lates.sort_unstable();
    let (median, worst) = (lates[10], lates[19]);
    value("median-late", median, "us", Some((0, (TICK_MS + 1) * 1000)));
    value("worst-late", worst, "us", None);
    check(median <= (TICK_MS + 1) * 1000, &format!("the median 1 ms nanosleep ended {median} us late, more than the {TICK_MS} ms tick and 1 ms"))
}

fn sl_nanosleep_zero() -> CaseResult {
    let start = mono();
    want_eq("nanosleep(0)", nanosleep_raw(&[0, 0], None), 0)?;
    let took = (mono() - start) / 1000;
    value("took", took, "us", Some((0, LATE_MS * 1000)));
    check(took <= LATE_MS * 1000, &format!("nanosleep(0) took {took} us"))
}

fn sl_nanosleep_einval() -> CaseResult {
    for (req, what) in [([0, NS], "tv_nsec 1000000000"), ([0, -1], "tv_nsec -1"), ([-1, 0], "tv_sec -1")] {
        let start = mono();
        want_err(&format!("nanosleep with {what}"), nanosleep_raw(&req, None), EINVAL)?;
        check(mono() - start < LATE_MS * MS, &format!("nanosleep with {what} slept before failing"))?;
    }
    Ok(())
}

fn sl_nanosleep_efault() -> CaseResult {
    want_err("nanosleep from address 16", sc(nr::NANOSLEEP, &[16, 0]), EFAULT)
}

/// A 2 s `sleep` interrupted by SIGUSR1, which a child sends 300 ms after the sleep
/// starts: the return, the time it took, the remaining time it wrote, and when the
/// signal was sent, in nanoseconds after the sleep began.
fn interrupted(flags: u64, sleep: impl FnOnce(&mut Ts) -> i64) -> Result<(i64, i64, Ts, i64), CaseError> {
    set_action(SIGUSR1, on_sig as usize as u64, flags)?;
    let at = mono() + 300 * MS;
    let _sender = send_at(at, SIGUSR1)?;
    let mut rem = [-7i64, -7];
    wait_for(2000, "interrupted-after");
    let start = mono();
    let ret = sleep(&mut rem);
    let elapsed = mono() - start;
    let due = at - start;
    value("interrupted-after", (elapsed - due) / 1000, "us", Some((0, LATE_MS * 1000)));
    Ok((ret, elapsed, rem, due))
}

/// The sleep ended by the signal: its handler ran, once, and the sleep ended no sooner
/// than the signal was sent, `due` ns after it began, and late by at most LATE_MS.
fn ended_by_signal(elapsed: i64, due: i64) -> CaseResult {
    let after = (elapsed - due) / 1000;
    check(count(SIGUSR1) == 1, &format!("the SIGUSR1 handler ran {} times during the sleep, not once", count(SIGUSR1)))?;
    check(elapsed >= due, &format!("the sleep ended {} us after it began, before SIGUSR1 was sent at {} us", elapsed / 1000, due / 1000))?;
    check(after <= LATE_MS * 1000, &format!("the sleep ended {after} us after SIGUSR1 was sent"))
}

fn check_rem(elapsed: i64, rem: &Ts) -> CaseResult {
    let left = ns_of(rem);
    let expected = 2 * NS - elapsed;
    value("rem", left / 1000, "us", Some(((expected - LATE_MS * MS) / 1000, (expected + LATE_MS * MS) / 1000)));
    check((0..NS).contains(&rem[1]), &format!("rem's tv_nsec is {}", rem[1]))?;
    check((left - expected).abs() <= LATE_MS * MS,
        &format!("after {} ms of a 2 s sleep, rem was {} ms", elapsed / MS, left / MS))
}

fn sl_nanosleep_eintr() -> CaseResult {
    let (ret, elapsed, rem, due) = interrupted(0, |rem| nanosleep_raw(&ts(2 * NS), Some(rem)))?;
    want_err("nanosleep interrupted by a caught signal", ret, EINTR)?;
    ended_by_signal(elapsed, due)?;
    check_rem(elapsed, &rem)
}

fn sl_nanosleep_resume() -> CaseResult {
    catch(SIGUSR1)?;
    let _sender = send_after(300, SIGUSR1)?;
    wait_for(1000, "late");
    let start = mono();
    let mut req = ts(NS);
    let mut interruptions = 0;
    loop {
        let mut rem = [0i64; 2];
        let ret = nanosleep_raw(&req, Some(&mut rem));
        if ret != -EINTR { want_eq("nanosleep", ret, 0)?; break; }
        interruptions += 1;
        if interruptions > 10 { return fail("nanosleep was interrupted more than ten times"); }
        req = rem;
    }
    let elapsed = mono() - start;
    check(interruptions >= 1, "the signal never interrupted the sleep")?;
    on_time("nanosleep resumed with rem", elapsed, 1000)
}

fn sl_nanosleep_sa_restart() -> CaseResult {
    let (ret, elapsed, _, due) = interrupted(SA_RESTART, |rem| nanosleep_raw(&ts(2 * NS), Some(rem)))?;
    want_err("nanosleep interrupted by a handler with SA_RESTART", ret, EINTR)?;
    ended_by_signal(elapsed, due)
}

/// A 800 ms nanosleep while a child sends SIGUSR1 after 200 ms, which `setup` has made
/// harmless.
fn undisturbed(setup: impl FnOnce() -> Checked) -> CaseResult {
    setup()?;
    let _sender = send_after(200, SIGUSR1)?;
    timed_sleep("nanosleep(800 ms) with SIGUSR1 sent at 200 ms", 800, || nanosleep_raw(&ts(800 * MS), None))
}

fn sl_nanosleep_ignored() -> CaseResult {
    undisturbed(|| set_action(SIGUSR1, SIG_IGN, 0))
}

fn sl_nanosleep_blocked() -> CaseResult {
    catch(SIGUSR1)?;
    undisturbed(|| block(bit(SIGUSR1)))?;
    check(pending()? & bit(SIGUSR1) != 0, "the blocked SIGUSR1 was not pending after the sleep")
}

fn sl_nanosleep_stop() -> CaseResult {
    let sleeper = probe(|| {
        let start = mono();
        let ret = nanosleep_raw(&ts(1500 * MS), None);
        Ok((ret, mono() - start))
    })?;
    let kid = sleeper.child.pid;
    check(until(1000, || is_parked(kid)), "the child never blocked in nanosleep")?;
    want_eq("kill(SIGSTOP)", kill(kid, SIGSTOP), 0)?;
    let status = wait_within(kid, WUNTRACED, WAIT_MS)?;
    check(stopped(status), &format!("waitpid(WUNTRACED) reported {}", status_text(status)))?;
    pause_ms(300);
    want_eq("kill(SIGCONT)", kill(kid, SIGCONT), 0)?;
    let status = wait_within(kid, WCONTINUED, WAIT_MS)?;
    check(continued(status), &format!("waitpid(WCONTINUED) reported {}", status_text(status)))?;
    wait_for(1500, "slept");
    let (ret, elapsed) = sleeper.result(3000)?;
    value("slept", elapsed / MS, "ms", Some((1500, 1500 + LATE_MS)));
    want_eq("nanosleep(1500 ms) stopped and continued", ret, 0)?;
    on_time("nanosleep(1500 ms) stopped and continued", elapsed, 1500)
}

fn sl_threads() -> CaseResult {
    wait_for(500, "worst-late");
    let threads: Vec<_> = [200i64, 300, 400, 500].into_iter().map(|ms| std::thread::spawn(move || {
        let start = mono();
        let ret = nanosleep_raw(&ts(ms * MS), None);
        (ms, ret, mono() - start)
    })).collect();
    let mut worst = 0;
    for thread in threads {
        let (ms, ret, elapsed) = thread.join().map_err(|_| CaseError::Fail("a sleeping thread panicked".into()))?;
        want_eq(&format!("a thread's nanosleep({ms} ms)"), ret, 0)?;
        let late = elapsed - ms * MS;
        check(late >= 0, &format!("a thread's {ms} ms sleep ended after {} us", elapsed / 1000))?;
        worst = worst.max(late);
    }
    value("worst-late", worst / 1000, "us", Some((0, LATE_MS * 1000)));
    check(worst <= LATE_MS * MS, &format!("a thread's sleep ended {} us late", worst / 1000))
}

fn sl_clock_monotonic() -> CaseResult {
    timed_sleep("clock_nanosleep(CLOCK_MONOTONIC, 100 ms)", 100, || clock_nanosleep_raw(CLOCK_MONOTONIC, 0, &ts(100 * MS), None))
}

fn sl_clock_realtime() -> CaseResult {
    timed_sleep("clock_nanosleep(CLOCK_REALTIME, 100 ms)", 100, || clock_nanosleep_raw(CLOCK_REALTIME, 0, &ts(100 * MS), None))
}

/// An absolute sleep on `clock` to 150 ms from now: it must not wake before the deadline.
fn absolute(clock: i32) -> CaseResult {
    let deadline = clock_ns(clock)? + 150 * MS;
    wait_for(150, "late");
    let ret = clock_nanosleep_raw(clock, TIMER_ABSTIME, &ts(deadline), None);
    let woke = clock_ns(clock)?;
    want_eq(&format!("clock_nanosleep({}, TIMER_ABSTIME)", clock_name(clock)), ret, 0)?;
    let late = (woke - deadline) / 1000;
    value("late", late, "us", Some((0, LATE_MS * 1000)));
    check(woke >= deadline, &format!("it woke {} us before its deadline", -late))?;
    check(late <= LATE_MS * 1000, &format!("it woke {late} us after its deadline"))
}

fn sl_abstime() -> CaseResult { absolute(CLOCK_MONOTONIC) }
fn sl_abstime_realtime() -> CaseResult { absolute(CLOCK_REALTIME) }

fn sl_abstime_past() -> CaseResult {
    let deadline = clock_ns(CLOCK_MONOTONIC)? - 100 * MS;
    let start = mono();
    want_eq("clock_nanosleep to a past deadline", clock_nanosleep_raw(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts(deadline.max(1)), None), 0)?;
    let took = (mono() - start) / 1000;
    value("took", took, "us", Some((0, LATE_MS * 1000)));
    check(took <= LATE_MS * 1000, &format!("a sleep to a past deadline took {took} us"))
}

fn sl_clock_eintr() -> CaseResult {
    let (ret, elapsed, rem, due) = interrupted(0, |rem| clock_nanosleep_raw(CLOCK_MONOTONIC, 0, &ts(2 * NS), Some(rem)))?;
    want_err("clock_nanosleep interrupted by a caught signal", ret, EINTR)?;
    ended_by_signal(elapsed, due)?;
    check_rem(elapsed, &rem)
}

fn sl_abstime_eintr() -> CaseResult {
    let deadline = clock_ns(CLOCK_MONOTONIC)? + 2 * NS;
    let (ret, elapsed, rem, due) = interrupted(0, |rem| clock_nanosleep_raw(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts(deadline), Some(rem)))?;
    want_err("an absolute clock_nanosleep interrupted by a caught signal", ret, EINTR)?;
    ended_by_signal(elapsed, due)?;
    check(rem == [-7, -7], &format!("the absolute sleep wrote rem as {} s {} ns", rem[0], rem[1]))
}

fn sl_clock_einval() -> CaseResult {
    want_err("clock_nanosleep with tv_nsec 1000000000", clock_nanosleep_raw(CLOCK_MONOTONIC, 0, &[0, NS], None), EINVAL)?;
    want_err("clock_nanosleep with tv_nsec -1", clock_nanosleep_raw(CLOCK_MONOTONIC, 0, &[0, -1], None), EINVAL)?;
    want_err(&format!("clock_nanosleep({CLOCK_UNKNOWN})"), clock_nanosleep_raw(CLOCK_UNKNOWN, 0, &ts(MS), None), EINVAL)?;
    want_err("clock_nanosleep(CLOCK_THREAD_CPUTIME_ID)", clock_nanosleep_raw(CLOCK_THREAD_CPUTIME_ID, 0, &ts(MS), None), EINVAL)
}

/// Run `sleep` in a child that reports (return, monotonic ns it took), and set
/// CLOCK_REALTIME forward by 10 s once the child blocks.
fn realtime_jump(sleep: impl FnOnce() -> i64 + 'static, ms: u64) -> Result<(i64, i64), CaseError> {
    let guard = RealtimeGuard::new()?;
    let sleeper = probe(move || {
        let start = mono();
        let ret = sleep();
        Ok((ret, mono() - start))
    })?;
    let kid = sleeper.child.pid;
    check(until(1000, || is_parked(kid)), "the child never blocked in its sleep")?;
    want_eq("clock_settime(CLOCK_REALTIME) forward 10 s", settime_raw(CLOCK_REALTIME, &ts(clock_ns(CLOCK_REALTIME)? + 10 * NS)), 0)?;
    wait_for(ms, "slept");
    let result = sleeper.result(ms + 1000);
    let restored = guard.restore();
    let result = result?;
    restored?;
    Ok(result)
}

fn sl_realtime_set_abstime() -> CaseResult {
    let deadline = clock_ns(CLOCK_REALTIME)? + 5 * NS;
    let (ret, elapsed) = realtime_jump(move || clock_nanosleep_raw(CLOCK_REALTIME, TIMER_ABSTIME, &ts(deadline), None), 1500)?;
    value("slept", elapsed / MS, "ms", Some((0, 1500)));
    want_eq("the absolute CLOCK_REALTIME sleep", ret, 0)?;
    check(elapsed < 1500 * MS, &format!("the sleep went on {} ms after the clock passed its deadline", elapsed / MS))
}

fn sl_realtime_set_nanosleep() -> CaseResult {
    let (ret, elapsed) = realtime_jump(|| nanosleep_raw(&ts(800 * MS), None), 800)?;
    value("slept", elapsed / MS, "ms", Some((800, 800 + LATE_MS)));
    want_eq("nanosleep(800 ms)", ret, 0)?;
    on_time("nanosleep(800 ms) across a clock change", elapsed, 800)
}

fn sl_realtime_set_relative() -> CaseResult {
    let (ret, elapsed) = realtime_jump(|| clock_nanosleep_raw(CLOCK_REALTIME, 0, &ts(800 * MS), None), 800)?;
    value("slept", elapsed / MS, "ms", Some((800, 800 + LATE_MS)));
    want_eq("clock_nanosleep(CLOCK_REALTIME, 800 ms)", ret, 0)?;
    on_time("a relative CLOCK_REALTIME sleep across a clock change", elapsed, 800)
}

// ---------------------------------------------------------------------------
// POSIX timers

/// Arm `timer` one-shot for `ms` and wait for blocked `sig`; its siginfo, checked on time.
fn fires_on_time(timer: &Timer, sig: i32, ms: i64) -> Result<SigInfo, CaseError> {
    wait_for(ms as u64, "late");
    let start = mono();
    timer.arm(0, ms * MS)?;
    let info = await_signal(sig, (ms + 1000) as u64, "the timer")?;
    on_time("the timer", mono() - start, ms)?;
    Ok(info)
}

fn tm_create_signal() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 0x5eed)?;
    let info = fires_on_time(&timer, SIGUSR1, 100)?;
    check(info.code == SI_TIMER, &format!("si_code is {}, not SI_TIMER", info.code))?;
    check(info.value() == 0x5eed, &format!("si_value is {:#x}, not the sigev_value 0x5eed", info.value()))
}

fn tm_create_default() -> CaseResult {
    block(bit(SIGALRM))?;
    let timer = Timer::create(CLOCK_MONOTONIC, None)?;
    let info = fires_on_time(&timer, SIGALRM, 50)?;
    check(info.code == SI_TIMER, &format!("si_code is {}, not SI_TIMER", info.code))?;
    check(info.value() as u32 as i32 == timer.id, &format!("si_value is {}, not the timer ID {}", info.value() as u32 as i32, timer.id))
}

fn tm_create_none() -> CaseResult {
    catch(SIGALRM)?;
    block(bit(SIGUSR1))?;
    let timer = Timer::create(CLOCK_MONOTONIC, Some(&sigevent(SIGEV_NONE, SIGUSR1, 0)))?;
    timer.arm(0, 200 * MS)?;
    wait_for(300, "after-expiry");
    pause_ms(50);
    let left = its_value(&timer.get()?);
    value("left", left / 1000, "us", Some((1, 150_000)));
    check(left > 0 && left <= 150 * MS, &format!("50 ms into a 200 ms timer, timer_gettime reports {} us left", left / 1000))?;
    pause_ms(250);
    let after = timer.get()?;
    value("after-expiry", its_value(&after) / 1000, "us", Some((0, 0)));
    check(after == [0; 4], &format!("after expiry timer_gettime reports {after:?}"))?;
    check(pending()? & bit(SIGUSR1) == 0 && count(SIGALRM) == 0, "a SIGEV_NONE timer sent a signal")
}

fn tm_realtime() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_REALTIME, SIGUSR1, 7)?;
    fires_on_time(&timer, SIGUSR1, 100).map(|_| ())
}

fn tm_create_einval() -> CaseResult {
    let cases = [
        (CLOCK_UNKNOWN, sigevent(SIGEV_SIGNAL, SIGUSR1, 0), "an unknown clock"),
        (CLOCK_MONOTONIC, sigevent(SIGEV_SIGNAL, 0, 0), "signal 0"),
        (CLOCK_MONOTONIC, sigevent(SIGEV_SIGNAL, 65, 0), "signal 65"),
        (CLOCK_MONOTONIC, sigevent(99, SIGUSR1, 0), "sigev_notify 99"),
    ];
    for (clock, ev, what) in cases {
        let mut id = -1;
        let ret = timer_create_raw(clock, Some(&ev), &mut id);
        if ret == 0 { let _ = sc(nr::TIMER_DELETE, &[id as i64 as u64]); }
        want_err(&format!("timer_create with {what}"), ret, EINVAL)?;
    }
    Ok(())
}

fn tm_oneshot() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    fires_on_time(&timer, SIGUSR1, 100)?;
    no_signal(SIGUSR1, 300, "300 ms after a one-shot timer fired")?;
    let after = timer.get()?;
    check(after == [0; 4], &format!("the expired one-shot timer reads {after:?}"))
}

/// Handle `sig` for `ms` while a periodic timer of `period_ms` runs. Each expiry is
/// checked against its own programmed time, k periods after arming: never early, and
/// late by at most LATE_MS. The count must be what that allows in the time the timer ran.
fn periodic(sig: i32, period_ms: i64, ms: i64, arm: impl FnOnce() -> CaseResult, disarm: impl FnOnce()) -> CaseResult {
    catch(sig)?;
    wait_for(ms as u64, "expiries");
    let start = mono();
    arm()?;
    pause_ms(ms as u64);
    let ran = mono() - start;
    disarm();
    let n = count(sig) as i64;
    let at = stamps(start);
    let period = period_ms * MS;
    let (low, high) = ((ran - LATE_MS * MS) / period, ran / period);
    value("expiries", n, "", Some((low, high)));
    let lates: Vec<i64> = at.iter().enumerate().map(|(k, at)| at - (k as i64 + 1) * period).collect();
    let earliest = lates.iter().copied().min().unwrap_or(0);
    let latest = lates.iter().copied().max().unwrap_or(0);
    value("worst-late", latest / 1000, "us", Some((0, LATE_MS * 1000)));
    let at_us: Vec<i64> = at.iter().map(|ns| ns / 1000).collect();
    pulses(period / 1000, LATE_MS * 1000, &at_us);
    check((low..=high).contains(&n), &format!("{n} expiries in {} ms of a {period_ms} ms periodic timer, not {low} to {high}", ran / MS))?;
    let first_early = lates.iter().position(|&late| late < 0).map_or(0, |k| k + 1);
    check(earliest >= 0, &format!("expiry {first_early} came {} us before its programmed time", -earliest / 1000))?;
    check(latest <= LATE_MS * MS, &format!("an expiry came {} us after its programmed time", latest / 1000))
}

fn tm_periodic() -> CaseResult {
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    periodic(SIGUSR1, 50, 1000, || timer.arm(50 * MS, 50 * MS), || { let _ = timer.arm(0, 0); })
}

fn tm_gettime() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    timer.arm(250 * MS, NS)?;
    let first = timer.get()?;
    pause_ms(200);
    let second = timer.get()?;
    let (a, b) = (its_value(&first), its_value(&second));
    value("left", b / 1000, "us", Some((700_000, 800_000)));
    check(its_interval(&first) == 250 * MS, &format!("the interval reads {} ns, not 250 ms", its_interval(&first)))?;
    check(a <= NS && a > 900 * MS, &format!("just after arming a 1 s timer, {} us were left", a / 1000))?;
    check(b < a && (700 * MS..=800 * MS).contains(&b), &format!("200 ms later, {} us were left", b / 1000))
}

fn tm_settime_old() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    timer.arm(500 * MS, 2 * NS)?;
    pause_ms(100);
    let mut old = [-1i64; 4];
    zero("timer_settime", timer.settime(0, &its(0, NS), Some(&mut old)))?;
    check(its_interval(&old) == 500 * MS, &format!("old_value's interval is {} ns, not 500 ms", its_interval(&old)))?;
    let left = its_value(&old);
    check(left <= 2 * NS && left > 1500 * MS, &format!("old_value's value is {} us, not what was left of 2 s", left / 1000))
}

fn tm_disarm() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    timer.arm(0, 200 * MS)?;
    timer.arm(0, 0)?;
    no_signal(SIGUSR1, 400, "after the timer was disarmed")?;
    let after = timer.get()?;
    check(after == [0; 4], &format!("the disarmed timer reads {after:?}"))
}

fn tm_abstime() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    let deadline = clock_ns(CLOCK_MONOTONIC)? + 150 * MS;
    wait_for(150, "late");
    zero("timer_settime(TIMER_ABSTIME)", timer.settime(TIMER_ABSTIME, &its(0, deadline), None))?;
    await_signal(SIGUSR1, 1500, "the absolute timer")?;
    let late = (mono() - deadline) / 1000;
    value("late", late, "us", Some((0, LATE_MS * 1000)));
    check(late >= 0, &format!("the absolute timer fired {} us before its time", -late))?;
    check(late <= LATE_MS * 1000, &format!("the absolute timer fired {late} us after its time"))
}

fn tm_abstime_past() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    let past = (clock_ns(CLOCK_MONOTONIC)? - 100 * MS).max(1);
    let start = mono();
    zero("timer_settime(TIMER_ABSTIME) to a past time", timer.settime(TIMER_ABSTIME, &its(0, past), None))?;
    await_signal(SIGUSR1, 1000, "the timer armed for a past time")?;
    let took = (mono() - start) / 1000;
    value("took", took, "us", Some((0, LATE_MS * 1000)));
    check(took <= LATE_MS * 1000, &format!("a timer armed for a past time took {took} us to fire"))
}

fn tm_settime_einval() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    want_err("timer_settime with value tv_nsec 1000000000", timer.settime(0, &[0, 0, 0, NS], None), EINVAL)?;
    want_err("timer_settime with interval tv_nsec -1", timer.settime(0, &[0, -1, 1, 0], None), EINVAL)?;
    let other = Timer { id: timer.id.wrapping_add(1000) };
    let ret = other.settime(0, &its(0, NS), None);
    core::mem::forget(other);
    want_err("timer_settime on an ID no timer has", ret, EINVAL)
}

fn tm_overrun() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    let start = mono();
    timer.arm(20 * MS, 20 * MS)?;
    wait_for(300, "overruns");
    pause_ms(300);
    let info = await_signal(SIGUSR1, 500, "the periodic timer")?;
    let elapsed = mono() - start;
    let n = want("timer_getoverrun", timer.overrun())?;
    timer.arm(0, 0)?;
    let expected = elapsed / (20 * MS) - 1;
    value("overruns", n, "", Some((expected - 2, expected + 1)));
    check(info.overrun() as i64 == n, &format!("si_overrun is {}, timer_getoverrun {n}", info.overrun()))?;
    check((expected - 2..=expected + 1).contains(&n), &format!("{} ms of a blocked 20 ms timer gave {n} overruns, not about {expected}", elapsed / MS))
}

fn tm_overrun_restarts() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    timer.arm(50 * MS, 50 * MS)?;
    pause_ms(270);
    await_signal(SIGUSR1, 500, "the periodic timer")?;
    let first = want("timer_getoverrun", timer.overrun())?;
    check(first >= 2, &format!("after 270 ms of a blocked 50 ms timer, timer_getoverrun was {first}"))?;
    await_signal(SIGUSR1, 500, "the timer's next expiry")?;
    let next = want("timer_getoverrun", timer.overrun())?;
    value("overruns", next, "", Some((0, 0)));
    check(next == 0, &format!("the expiry after the first signal was delivered reports {next} overruns"))
}

fn tm_delete() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    timer.arm(0, 200 * MS)?;
    let id = timer.id;
    core::mem::forget(timer);
    want_eq("timer_delete", sc(nr::TIMER_DELETE, &[id as i64 as u64]), 0)?;
    no_signal(SIGUSR1, 400, "after the armed timer was deleted")?;
    let mut cur = [0i64; 4];
    want_err("timer_gettime on the deleted timer", timer_gettime_raw(id, &mut cur), EINVAL)?;
    want_err("timer_delete on the deleted timer", sc(nr::TIMER_DELETE, &[id as i64 as u64]), EINVAL)
}

fn tm_many() -> CaseResult {
    let sig = SIGRTMIN + 2;
    block(bit(sig))?;
    let mut timers = Vec::new();
    for i in 0..8u64 {
        timers.push(Timer::signal(CLOCK_MONOTONIC, sig, i)?);
    }
    let mut ids: Vec<i32> = timers.iter().map(|t| t.id).collect();
    ids.sort_unstable();
    ids.dedup();
    check(ids.len() == 8, "timer_create gave two timers the same ID")?;
    for (i, timer) in timers.iter().enumerate() {
        timer.arm(0, (8 - i as i64) * 40 * MS)?;
    }
    wait_for(320, "in-order");
    let mut order = Vec::new();
    for _ in 0..8 {
        order.push(await_signal(sig, 1000, "eight timers")?.value());
    }
    let in_order = order.iter().enumerate().filter(|&(i, &v)| v == 7 - i as u64).count();
    value("in-order", in_order as i64, "", Some((8, 8)));
    check(order == [7, 6, 5, 4, 3, 2, 1, 0], &format!("the timers' values arrived as {order:?}, not in expiry order 7 to 0"))
}

/// Whether the thread-timer case's second thread should keep computing.
static BURNING: AtomicU32 = AtomicU32::new(0);

/// A CPU-time timer of 100 ms on `clock`: no signal during a 300 ms sleep, then one while
/// the caller computes, once `clock` has advanced by what timer_gettime said was left
/// when the computing began (the sleep's own system calls may have used some), and late
/// by at most LATE_MS of it. For the thread clock a second thread computes all along, so
/// the process's CPU time runs ahead of the caller's: a timer counting the process's
/// would fire during the sleep, or early.
fn cpu_timer(clock: i32) -> CaseResult {
    block(bit(SIGUSR1))?;
    let sibling = (clock == CLOCK_THREAD_CPUTIME_ID).then(|| {
        BURNING.store(1, Ordering::SeqCst);
        std::thread::spawn(|| { burn_until(bounded(u64::MAX, CLEANUP_MS), || BURNING.load(Ordering::SeqCst) == 0); })
    });
    let result = cpu_timer_fires(clock);
    BURNING.store(0, Ordering::SeqCst);
    if let Some(sibling) = sibling { let _ = sibling.join(); }
    result
}

fn cpu_timer_fires(clock: i32) -> CaseResult {
    let name = clock_name(clock);
    let timer = Timer::signal(clock, SIGUSR1, 1)?;
    timer.arm(0, 100 * MS)?;
    no_signal(SIGUSR1, 300, &format!("a 100 ms {name} timer while the caller slept 300 ms"))?;
    // Start CPU accounting before reading the remaining duration: the gap
    // between calls must not make an on-time expiry appear early (#1276).
    let c0 = clock_ns(clock)?;
    let left = its_value(&timer.get()?);
    let fired = burn_until(2000, || pending().is_ok_and(|p| p & bit(SIGUSR1) != 0));
    let used = clock_ns(clock)? - c0;
    value("left", left / MS, "ms", Some((1, 100)));
    value("cpu-at-expiry", used / MS, "ms", Some((left / MS, left / MS + LATE_MS)));
    check(left > 0 && left <= 100 * MS, &format!("after the sleep, timer_gettime reports {} us left of a 100 ms {name} timer that has not fired", left / 1000))?;
    check(fired, &format!("a 100 ms {name} timer did not fire in 2 s of computing"))?;
    check(used >= left, &format!("a {name} timer with {} us left fired after {} us of {name}", left / 1000, used / 1000))?;
    check(used <= left + LATE_MS * MS, &format!("a {name} timer with {} us left fired only after {} us of {name}", left / 1000, used / 1000))
}

fn tm_cputime_process() -> CaseResult { cpu_timer(CLOCK_PROCESS_CPUTIME_ID) }
fn tm_cputime_thread() -> CaseResult { cpu_timer(CLOCK_THREAD_CPUTIME_ID) }

fn tm_fork() -> CaseResult {
    block(bit(SIGUSR1))?;
    let timer = Timer::signal(CLOCK_MONOTONIC, SIGUSR1, 1)?;
    timer.arm(0, 300 * MS)?;
    let id = timer.id;
    let child = probe(move || {
        let mut cur = [0i64; 4];
        let ret = timer_gettime_raw(id, &mut cur);
        pause_ms(600);
        Ok((ret, (pending()? & bit(SIGUSR1) != 0) as i64))
    })?;
    wait_for(600, "child-pending");
    await_signal(SIGUSR1, 1000, "the parent's timer")?;
    let (ret, got) = child.result(1500)?;
    value("child-pending", got, "", Some((0, 0)));
    want_err("timer_gettime of the parent's timer in the child", ret, EINVAL)?;
    check(got == 0, "the parent's timer sent its signal to the child")
}

fn tm_exec() -> CaseResult {
    // The timer sends SIGUSR1, whose default action ends the process, after 500 ms; the
    // new program sleeps 1200 ms and says `alive` only if the whole sleep completed.
    let (mut child, out, start) = exec_helper(|| {
        let mut id = -1;
        zero("timer_create", timer_create_raw(CLOCK_MONOTONIC, Some(&sigevent(SIGEV_SIGNAL, SIGUSR1, 0)), &mut id))?;
        let t = its(0, 500 * MS);
        zero("timer_settime", sc(nr::TIMER_SETTIME, &[id as i64 as u64, 0, t.as_ptr() as u64, 0]))?;
        Ok(vec![id.to_string(), "1200".to_string()])
    })?;
    wait_for(1200, "ended-after");
    let said = read_up_to(out, 4096, 3000).map(|s| String::from_utf8_lossy(&s).to_string());
    let _ = io::close(out);
    let said = said?;
    let status = child.wait_for("the exec'd helper", WAIT_MS)?;
    let ended = (mono() - start) / MS;
    value("ended-after", ended, "ms", Some((1200, 1200 + EXEC_MS as i64)));
    let words = report_words(said.lines().next().unwrap_or(""));
    let ret = words.get("timer").and_then(|t| t.parse::<i64>().ok()).ok_or_else(|| format!("the helper reported {said:?}"))?;
    want_err("timer_gettime of the pre-exec timer in the new program", ret, EINVAL)?;
    check(exited(status) && exit_code(status) == 0 && said.lines().any(|line| line == "alive"),
        &format!("the new program ended with {} and did not finish its 1200 ms sleep", status_text(status)))?;
    check((1200..=1200 + EXEC_MS as i64).contains(&ended), &format!("the new program ended {ended} ms after the fork, not after its 1200 ms sleep"))
}

// ---------------------------------------------------------------------------
// alarm & interval timers

fn it_alarm() -> CaseResult {
    catch(SIGALRM)?;
    wait_for(1000, "late");
    let start = mono();
    want_eq("alarm(1)", alarm(1), 0)?;
    let took = handled(SIGALRM, 1, start, 2500).ok_or("no SIGALRM within 2.5 s of alarm(1)")?;
    on_time("alarm(1)", took, 1000)
}

/// CLOCK_MONOTONIC as a child read it just before arming its alarm, which it writes
/// down a pipe whose read end is `r`.
fn armed_at(r: Fd) -> Result<i64, CaseError> {
    let said = read_up_to(r, 8, WAIT_MS)?;
    let bytes: [u8; 8] = said.as_slice().try_into().map_err(|_| CaseError::Fail("the child never reported arming its alarm".into()))?;
    Ok(i64::from_le_bytes(bytes))
}

fn it_alarm_default() -> CaseResult {
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let kid = Child::start(|| {
        let _ = io::close(r);
        let armed = mono();
        if alarm(1) != 0 { return 2; }
        let _ = io::write(w, &armed.to_le_bytes());
        pause_ms(3000);
        0
    });
    let _ = io::close(w);
    let mut kid = match kid {
        Ok(kid) => kid,
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    wait_for(1000, "late");
    let armed = armed_at(r);
    let _ = io::close(r);
    let armed = armed?;
    let status = kid.wait_for("the child with an alarm", 4000)?;
    let took = mono() - armed;
    check(signaled(status) && term_sig(status) == SIGALRM, &format!("the child ended with {}, not death by SIGALRM", status_text(status)))?;
    on_time("the child's death by alarm(1)", took, 1000)
}

fn it_alarm_remaining() -> CaseResult {
    catch(SIGALRM)?;
    want_eq("the first alarm(5)", alarm(5), 0)?;
    pause_ms(1200);
    let left = alarm(3);
    let _ = alarm(0);
    value("returned", left, "s", Some((3, 4)));
    check(left == 3 || left == 4, &format!("alarm(3) 1.2 s after alarm(5) returned {}, not the 3.8 s left", shown(left)))
}

fn it_alarm_zero() -> CaseResult {
    catch(SIGALRM)?;
    want_eq("alarm(1)", alarm(1), 0)?;
    pause_ms(300);
    want_eq("alarm(0) 300 ms after alarm(1)", alarm(0), 1)?;
    wait_for(1200, "alarms");
    pause_ms(1200);
    value("alarms", count(SIGALRM) as i64, "", Some((0, 0)));
    check(count(SIGALRM) == 0, "SIGALRM arrived after alarm(0) cancelled the alarm")
}

fn it_alarm_replace() -> CaseResult {
    catch(SIGALRM)?;
    want_eq("alarm(5)", alarm(5), 0)?;
    let start = mono();
    let left = alarm(1);
    check(left == 5 || left == 4, &format!("alarm(1) after alarm(5) returned {}", shown(left)))?;
    // Watch past the time the replaced alarm(5) would have fired.
    let watch = 5000 + LATE_MS as u64 + 100;
    wait_for(watch, "alarms");
    pause_ms(bounded(watch, CLEANUP_MS));
    let watched = mono() - start;
    let n = count(SIGALRM);
    let first = FIRST_NS.load(Ordering::SeqCst) - start;
    value("alarms", n as i64, "", Some((1, 1)));
    want_eq("alarm(0) after the replacement fired", alarm(0), 0)?;
    check(watched >= (5000 + LATE_MS) * MS, &format!("the case could watch only {} ms, not past the replaced alarm's 5 s", watched / MS))?;
    check(n == 1, &format!("{n} SIGALRMs in {} ms after alarm(5) was replaced by alarm(1)", watched / MS))?;
    on_time("the replacing alarm(1)", first, 1000)
}

fn it_alarm_read() -> CaseResult {
    catch(SIGALRM)?;
    let (r, w) = io::pipe()?;
    wait_for(1000, "interrupted-after");
    let start = mono();
    want_eq("alarm(1)", alarm(1), 0)?;
    let mut buf = [0u8; 8];
    let ret = sc(nr::READ, &[r.raw(), buf.as_mut_ptr() as u64, buf.len() as u64]);
    let took = mono() - start;
    let _ = (io::close(r), io::close(w));
    value("interrupted-after", took / MS, "ms", Some((1000, 1000 + LATE_MS)));
    want_err("read of an empty pipe when the alarm fired", ret, EINTR)?;
    on_time("the read's interruption", took, 1000)
}

fn it_real() -> CaseResult {
    catch(SIGALRM)?;
    wait_for(600, "alarms");
    let start = mono();
    arm_itimer(ITIMER_REAL, 0, 200_000)?;
    pause_ms(600);
    let n = count(SIGALRM);
    value("alarms", n as i64, "", Some((1, 1)));
    check(n == 1, &format!("{n} SIGALRMs in 600 ms of a 200 ms one-shot ITIMER_REAL"))?;
    on_time("ITIMER_REAL", FIRST_NS.load(Ordering::SeqCst) - start, 200)
}

fn it_real_interval() -> CaseResult {
    periodic(SIGALRM, 50, 1000, || Ok(arm_itimer(ITIMER_REAL, 50_000, 50_000)?), || { let _ = arm_itimer(ITIMER_REAL, 0, 0); })
}

fn it_getitimer() -> CaseResult {
    catch(SIGALRM)?;
    arm_itimer(ITIMER_REAL, 250_000, 1_000_000)?;
    let first = getitimer(ITIMER_REAL)?;
    pause_ms(200);
    let second = getitimer(ITIMER_REAL)?;
    arm_itimer(ITIMER_REAL, 0, 0)?;
    let (a, b) = (itv_value(&first), itv_value(&second));
    value("left", b, "us", Some((700_000, 800_000)));
    check(itv_interval(&first) == 250_000, &format!("the interval reads {} us, not 250 ms", itv_interval(&first)))?;
    check(a <= 1_000_000 && a > 900_000, &format!("just after arming a 1 s timer, {a} us were left"))?;
    check(b < a && (700_000..=800_000).contains(&b), &format!("200 ms later, {b} us were left"))
}

fn it_old_value() -> CaseResult {
    catch(SIGALRM)?;
    arm_itimer(ITIMER_REAL, 500_000, 2_000_000)?;
    pause_ms(100);
    let mut old = [-1i64; 4];
    zero("setitimer", setitimer(ITIMER_REAL, &itv(0, 1_000_000), Some(&mut old)))?;
    arm_itimer(ITIMER_REAL, 0, 0)?;
    check(itv_interval(&old) == 500_000, &format!("old_value's interval is {} us, not 500 ms", itv_interval(&old)))?;
    let left = itv_value(&old);
    check(left <= 2_000_000 && left > 1_500_000, &format!("old_value's value is {left} us, not what was left of 2 s"))
}

fn it_disarm() -> CaseResult {
    catch(SIGALRM)?;
    arm_itimer(ITIMER_REAL, 0, 200_000)?;
    arm_itimer(ITIMER_REAL, 0, 0)?;
    pause_ms(400);
    check(count(SIGALRM) == 0, "SIGALRM arrived after ITIMER_REAL was disarmed")?;
    let after = getitimer(ITIMER_REAL)?;
    check(after == [0; 4], &format!("the disarmed timer reads {after:?}"))
}

fn it_small() -> CaseResult {
    catch(SIGALRM)?;
    arm_itimer(ITIMER_REAL, 0, 1)?;
    check(until(200, || count(SIGALRM) == 1), "a 1 us ITIMER_REAL did not fire within 200 ms")
}

fn it_einval() -> CaseResult {
    want_err("setitimer(3)", setitimer(3, &itv(0, 1_000_000), None), EINVAL)?;
    let mut cur = [0i64; 4];
    want_err("getitimer(3)", sc(nr::GETITIMER, &[3, cur.as_mut_ptr() as u64]), EINVAL)?;
    want_err("setitimer with tv_usec 1000000", setitimer(ITIMER_REAL, &[0, 0, 0, 1_000_000], None), EINVAL)?;
    want_err("setitimer with interval tv_usec -1", setitimer(ITIMER_REAL, &[0, -1, 1, 0], None), EINVAL)
}

fn it_alarm_shares_real() -> CaseResult {
    catch(SIGALRM)?;
    want_eq("alarm(5)", alarm(5), 0)?;
    let real = itv_value(&getitimer(ITIMER_REAL)?);
    check(real > 4_000_000 && real <= 5_000_000, &format!("after alarm(5), ITIMER_REAL reads {real} us"))?;
    arm_itimer(ITIMER_REAL, 0, 2_000_000)?;
    let left = alarm(0);
    check(left == 2, &format!("alarm(0) after setitimer(ITIMER_REAL, 2 s) returned {}", shown(left)))
}

/// The process's CPU time as getrusage(RUSAGE_SELF) reports it, in microseconds: user
/// time, and user and system time together.
fn rusage_us() -> Result<(i64, i64), String> {
    let mut usage = [0i64; 18];
    zero("getrusage(RUSAGE_SELF)", sc(nr::GETRUSAGE, &[0, usage.as_mut_ptr() as u64]))?;
    let user = usage[0] * 1_000_000 + usage[1];
    Ok((user, user + usage[2] * 1_000_000 + usage[3]))
}

/// The CPU time interval timer `which` counts, from getrusage, in microseconds:
/// ITIMER_VIRTUAL user time, ITIMER_PROF user and system time.
fn itimer_cpu_us(which: i32) -> Result<i64, String> {
    let (user, all) = rusage_us()?;
    Ok(if which == ITIMER_VIRTUAL { user } else { all })
}

/// A 100 ms CPU-time interval timer `which` sending `sig`: none while the process sleeps
/// 300 ms, at least `min_left` ms of it left after the sleep, then one while it computes,
/// once the CPU time the timer counts, measured by getrusage, has grown by what getitimer
/// said was left when the computing began, and late by at most LATE_MS of it.
/// ITIMER_PROF counts the sleep's own system calls; ITIMER_VIRTUAL counts only the few
/// instructions the sleep runs in user mode.
fn cpu_itimer(which: i32, sig: i32, min_left: i64) -> CaseResult {
    catch(sig)?;
    arm_itimer(which, 0, 100_000)?;
    pause_ms(300);
    check(count(sig) == 0, &format!("signal {sig} arrived while the process slept"))?;
    let left = itv_value(&getitimer(which)?);
    let c0 = itimer_cpu_us(which)?;
    let fired = burn_until(2000, || count(sig) > 0);
    let used = itimer_cpu_us(which)? - c0;
    value("left", left / 1000, "ms", Some((min_left, 100)));
    value("cpu-at-expiry", used / 1000, "ms", Some((left / 1000, left / 1000 + LATE_MS)));
    check(left >= min_left * 1000 && left <= 100_000, &format!("after a 300 ms sleep, getitimer reports {left} us left of the 100 ms timer, not {min_left} ms or more"))?;
    check(fired, &format!("a 100 ms timer did not send signal {sig} in 2 s of computing"))?;
    check(used >= left, &format!("a timer with {left} us of CPU time left fired after {used} us of it"))?;
    check(used <= left + LATE_MS * 1000, &format!("a timer with {left} us of CPU time left fired only after {used} us of it"))
}

/// What a sleep may take of ITIMER_VIRTUAL's 100 ms: two ticks charged to user mode, and 2 ms.
fn it_virtual() -> CaseResult { cpu_itimer(ITIMER_VIRTUAL, SIGVTALRM, 100 - 2 * TICK_MS - 2) }
fn it_prof() -> CaseResult { cpu_itimer(ITIMER_PROF, SIGPROF, 1) }

/// A 20 ms ITIMER_VIRTUAL reloading while the process computes for 500 ms fires once per
/// 20 ms of the user CPU time getrusage counts across it: never sooner, and with the
/// last expiry late by at most LATE_MS of it.
fn it_virtual_interval() -> CaseResult {
    catch(SIGVTALRM)?;
    let u0 = itimer_cpu_us(ITIMER_VIRTUAL)?;
    arm_itimer(ITIMER_VIRTUAL, 20_000, 20_000)?;
    burn(500);
    arm_itimer(ITIMER_VIRTUAL, 0, 0)?;
    let user = itimer_cpu_us(ITIMER_VIRTUAL)? - u0;
    let n = count(SIGVTALRM) as i64;
    let (low, high) = ((user - LATE_MS * 1000).max(0) / 20_000, user / 20_000);
    value("user-cpu", user / 1000, "ms", None);
    value("expiries", n, "", Some((low, high)));
    check(user >= 100_000, &format!("getrusage counted {} ms of user CPU time in 500 ms of computing", user / 1000))?;
    check((low..=high).contains(&n), &format!("a 20 ms ITIMER_VIRTUAL fired {n} times in {} ms of user CPU time, not {low} to {high}", user / 1000))
}

fn it_fork() -> CaseResult {
    catch(SIGALRM)?;
    arm_itimer(ITIMER_REAL, 0, 5_000_000)?;
    arm_itimer(ITIMER_VIRTUAL, 0, 5_000_000)?;
    arm_itimer(ITIMER_PROF, 0, 5_000_000)?;
    let result = in_child(|| {
        for (which, name) in [(ITIMER_REAL, "ITIMER_REAL"), (ITIMER_VIRTUAL, "ITIMER_VIRTUAL"), (ITIMER_PROF, "ITIMER_PROF")] {
            let cur = getitimer(which)?;
            if cur != [0; 4] { return Err(format!("the child's {name} reads {cur:?}")); }
        }
        let left = alarm(0);
        if left != 0 { return Err(format!("the child's alarm(0) returned {}", shown(left))); }
        Ok(())
    });
    let timers = [(ITIMER_REAL, "ITIMER_REAL"), (ITIMER_VIRTUAL, "ITIMER_VIRTUAL"), (ITIMER_PROF, "ITIMER_PROF")];
    let parent: Result<Vec<i64>, String> = timers.iter().map(|&(which, _)| getitimer(which).map(|t| itv_value(&t))).collect();
    for (which, _) in timers { let _ = arm_itimer(which, 0, 0); }
    result?;
    for ((_, name), left) in timers.iter().zip(parent?) {
        check(left > 0 && left <= 5_000_000, &format!("after the fork, the parent's {name} reads {left} us of 5 s"))?;
    }
    Ok(())
}

fn it_exec_alarm() -> CaseResult {
    let (ar, aw) = io::pipe2(O_CLOEXEC)?;
    let helper = exec_helper(|| {
        let _ = io::close(ar);
        let armed = mono();
        zero("alarm(2)", alarm(2))?;
        let _ = io::write(aw, &armed.to_le_bytes());
        Ok(vec!["-1".to_string(), "4000".to_string()])
    });
    let _ = io::close(aw);
    let (mut child, out, _) = match helper {
        Ok(helper) => helper,
        Err(e) => { let _ = io::close(ar); return Err(e); }
    };
    let armed = armed_at(ar);
    let _ = io::close(ar);
    let armed = match armed {
        Ok(armed) => armed,
        Err(e) => { let _ = io::close(out); return Err(e); }
    };
    wait_for(2000, "late");
    let said = read_up_to(out, 4096, 4000).map(|s| String::from_utf8_lossy(&s).to_string());
    let _ = io::close(out);
    let said = said?;
    let status = child.wait_for("the exec'd helper", WAIT_MS)?;
    let ended = mono() - armed;
    let words = report_words(said.lines().next().unwrap_or(""));
    let (left, _) = reported_itimer(&words, "real")?;
    value("left-after-exec", left / 1000, "ms", Some((1, 2000)));
    check(left > 0 && left <= 2_000_000, &format!("after exec, ITIMER_REAL reads {left} us of the 2 s alarm"))?;
    check(signaled(status) && term_sig(status) == SIGALRM,
        &format!("the new program ended with {}, not death by the alarm's SIGALRM", status_text(status)))?;
    on_time("the new program's death by alarm(2)", ended, 2000)
}

fn it_exec_cpu() -> CaseResult {
    let (mut child, out, _) = exec_helper(|| {
        arm_itimer(ITIMER_VIRTUAL, 0, 5_000_000)?;
        arm_itimer(ITIMER_PROF, 0, 5_000_000)?;
        Ok(Vec::new())
    })?;
    let said = read_up_to(out, 4096, 2000).map(|s| String::from_utf8_lossy(&s).to_string());
    let _ = io::close(out);
    let words = report_words(said?.lines().next().unwrap_or(""));
    let status = child.wait_for("the exec'd helper", WAIT_MS)?;
    check(exited(status) && exit_code(status) == 0, &format!("the helper ended with {}", status_text(status)))?;
    for key in ["virtual", "prof"] {
        let (left, _) = reported_itimer(&words, key)?;
        check(left > 0 && left <= 5_000_000, &format!("after exec, ITIMER_{} reads {left} us of 5 s", key.to_uppercase()))?;
    }
    Ok(())
}

static SUITE: Suite = suite(
    "time", "Time & timers", &[
        category("clocks", "clocks & resolution", &[
            case("realtime", "clock_gettime(CLOCK_REALTIME) succeeds with tv_nsec below one second and a date between 2020 and 2100", clk_realtime),
            case("monotonic", "clock_gettime(CLOCK_MONOTONIC) succeeds with a nonnegative time and tv_nsec below one second", clk_monotonic),
            case("monotonic-steady", "CLOCK_MONOTONIC never goes backwards over 100000 reads in one thread", clk_monotonic_steady),
            case("monotonic-cpus", "CLOCK_MONOTONIC never goes backwards between threads running on two processors", clk_monotonic_cpus),
            case("monotonic-fine", "Linux policy: CLOCK_MONOTONIC advances between consecutive reads rather than once per tick", clk_monotonic_fine),
            case("rate-realtime", "CLOCK_REALTIME and CLOCK_MONOTONIC advance at the same rate over 2 seconds", clk_rate_realtime),
            case("rate-counter", "CLOCK_MONOTONIC advances at the rate of the processor's counter over 2 seconds (ARM64 CNTVCT_EL0, x86-64 the TSC at its CPUID frequency)", clk_rate_counter),
            case("counter-cpus", "Linux ABI: user mode reads the processor's counter (ARM64 CNTVCT_EL0, x86-64 the TSC) on every online processor, from a thread pinned there by sched_setaffinity and found there by getcpu", clk_counter_cpus),
            case("counter-trap-default", "Linux ABI: denied counter and timer register reads kill user mode with SIGILL on ARM64 (CNTPCT_EL0, CNTV_CTL_EL0, CNTP_CTL_EL0) and SIGSEGV on x86-64 (TSC, TSC-deadline, x2APIC initial-count MSRs)", clk_counter_trap_default),
            case("counter-trap-caught", "Linux ABI: caught denied counter and timer register reads carry ILL_ILLOPC and si_addr on ARM64, SI_KERNEL and faulting RIP on x86-64, and resume past the instruction; granted counter reads do not trap", clk_counter_trap_caught),
            case("rate-rtc", "Linux ABI: CLOCK_REALTIME advances at the rate of the RTC, read through /dev/rtc0 RTC_RD_TIME, over 3 seconds", clk_rate_rtc),
            case("cputime-process", "CLOCK_PROCESS_CPUTIME_ID advances while the process computes and not while it sleeps", clk_cputime_process),
            case("cputime-thread", "CLOCK_THREAD_CPUTIME_ID counts only the calling thread's CPU time", clk_cputime_thread),
            case("invalid", "clock_gettime of an unknown clock fails with EINVAL", clk_invalid),
            case("efault", "Linux policy: clock_gettime into an unmapped buffer fails with EFAULT", clk_efault),
            case("getres", "clock_getres reports a resolution above zero and no coarser than 20 ms for the realtime, monotonic and CPU-time clocks", clk_getres),
            case("getres-null", "clock_getres with a null resolution pointer succeeds", clk_getres_null),
            case("getres-einval", "clock_getres of an unknown clock fails with EINVAL", clk_getres_einval),
            case("coarse-tick", "Linux policy: CLOCK_MONOTONIC_COARSE reports the kernel tick as its resolution and advances in steps of it (5 ms on x86-64, 1 ms on ARM64)", clk_coarse_tick),
            case("settime", "clock_settime(CLOCK_REALTIME) sets the clock to the nanosecond value given and CLOCK_MONOTONIC does not jump", clk_settime),
            case("settime-eperm", "clock_settime by an unprivileged process fails with EPERM and leaves CLOCK_REALTIME unchanged", clk_settime_eperm),
            case("settime-einval", "clock_settime with tv_nsec out of range, on CLOCK_MONOTONIC or on an unknown clock fails with EINVAL", clk_settime_einval),
            case("gettimeofday", "gettimeofday succeeds and agrees with CLOCK_REALTIME to the microsecond", clk_gettimeofday),
            case("time", "time() returns CLOCK_REALTIME's seconds and stores them through its argument", clk_time),
        ]),
        category("sleep", "nanosleep & clock_nanosleep", &[
            case("nanosleep", "nanosleep of 100 ms returns 0 no earlier than 100 ms later, late by at most two ticks and 20 ms", sl_nanosleep),
            case("nanosleep-seconds", "nanosleep of 1 s and 250000000 ns sleeps the whole of both", sl_nanosleep_seconds),
            case("nanosleep-short", "Twenty 1 ms nanosleeps each end no earlier than 1 ms, with a median late by at most one tick and 1 ms", sl_nanosleep_short),
            case("nanosleep-zero", "nanosleep of zero returns 0 at once", sl_nanosleep_zero),
            case("nanosleep-einval", "nanosleep with tv_nsec below 0 or at least 1000000000, or a negative tv_sec, fails with EINVAL at once", sl_nanosleep_einval),
            case("nanosleep-efault", "Linux policy: nanosleep with an unmapped request fails with EFAULT", sl_nanosleep_efault),
            case("nanosleep-eintr", "A caught signal ends nanosleep early with EINTR and writes the time left to rem", sl_nanosleep_eintr),
            case("nanosleep-resume", "Calling nanosleep again with rem after EINTR completes the sleep first asked for", sl_nanosleep_resume),
            case("nanosleep-sa-restart", "Linux policy: nanosleep fails with EINTR after a handler even with SA_RESTART", sl_nanosleep_sa_restart),
            case("nanosleep-ignored", "A signal that is ignored does not end nanosleep", sl_nanosleep_ignored),
            case("nanosleep-blocked", "A blocked signal does not end nanosleep and stays pending", sl_nanosleep_blocked),
            case("nanosleep-stop", "A sleeper stopped and continued by SIGSTOP and SIGCONT finishes its nanosleep, neither early nor with EINTR", sl_nanosleep_stop),
            case("threads", "Four threads sleeping 200 to 500 ms at once each wake on time", sl_threads),
            case("clock-monotonic", "clock_nanosleep on CLOCK_MONOTONIC for 100 ms sleeps no less, late by at most two ticks and 20 ms", sl_clock_monotonic),
            case("clock-realtime", "clock_nanosleep on CLOCK_REALTIME for 100 ms sleeps no less, late by at most two ticks and 20 ms", sl_clock_realtime),
            case("abstime", "clock_nanosleep with TIMER_ABSTIME on CLOCK_MONOTONIC wakes no earlier than the deadline and late by at most two ticks and 20 ms", sl_abstime),
            case("abstime-realtime", "clock_nanosleep with TIMER_ABSTIME on CLOCK_REALTIME wakes no earlier than the deadline and late by at most two ticks and 20 ms", sl_abstime_realtime),
            case("abstime-past", "clock_nanosleep with TIMER_ABSTIME and a deadline already past returns 0 at once", sl_abstime_past),
            case("clock-eintr", "A caught signal ends a relative clock_nanosleep with EINTR and the time left in rem", sl_clock_eintr),
            case("abstime-eintr", "A caught signal ends an absolute clock_nanosleep with EINTR and leaves rem untouched", sl_abstime_eintr),
            case("clock-einval", "clock_nanosleep with tv_nsec out of range, an unknown clock or the caller's thread CPU-time clock fails with EINVAL", sl_clock_einval),
            case("realtime-set-abstime", "Setting CLOCK_REALTIME past an absolute CLOCK_REALTIME sleep's deadline wakes it", sl_realtime_set_abstime),
            case("realtime-set-nanosleep", "Setting CLOCK_REALTIME forward does not shorten a nanosleep", sl_realtime_set_nanosleep),
            case("realtime-set-relative", "Setting CLOCK_REALTIME forward does not shorten a relative clock_nanosleep on CLOCK_REALTIME", sl_realtime_set_relative),
        ]),
        category("timers", "POSIX timers", &[
            case("create-signal", "A CLOCK_MONOTONIC timer with SIGEV_SIGNAL delivers its signal on time with si_code SI_TIMER and its sigev_value", tm_create_signal),
            case("create-default", "A timer created with no sigevent delivers SIGALRM with the timer ID as its value", tm_create_default),
            case("create-none", "A SIGEV_NONE timer sends no signal, and timer_gettime shows it count down and expire", tm_create_none),
            case("realtime", "A CLOCK_REALTIME timer fires on time", tm_realtime),
            case("create-einval", "timer_create with an unknown clock, signal 0 or 65, or an unknown sigev_notify fails with EINVAL", tm_create_einval),
            case("oneshot", "A one-shot timer fires once, on time, and then reads as disarmed", tm_oneshot),
            case("periodic", "A periodic 50 ms timer fires twenty times a second, 50 ms apart", tm_periodic),
            case("gettime", "timer_gettime reports the time left, decreasing and never above the value set, and the interval", tm_gettime),
            case("settime-old", "timer_settime returns the previous setting in old_value", tm_settime_old),
            case("disarm", "timer_settime with a zero it_value disarms the timer", tm_disarm),
            case("abstime", "A timer armed with TIMER_ABSTIME fires no earlier than that time and late by at most two ticks and 20 ms", tm_abstime),
            case("abstime-past", "A timer armed with TIMER_ABSTIME for a time already past fires at once", tm_abstime_past),
            case("settime-einval", "timer_settime with tv_nsec out of range or on an unknown timer fails with EINVAL", tm_settime_einval),
            case("overrun", "timer_getoverrun and si_overrun count the expiries a pending timer signal stood for", tm_overrun),
            case("overrun-restarts", "The overrun count starts again from zero once the timer's signal is delivered", tm_overrun_restarts),
            case("delete", "timer_delete disarms the timer, and its ID then fails with EINVAL", tm_delete),
            case("many", "Eight timers get distinct IDs and each delivers its own value, in expiry order", tm_many),
            case("cputime-process", "A CLOCK_PROCESS_CPUTIME_ID timer counts the process's CPU time, not time asleep", tm_cputime_process),
            case("cputime-thread", "A CLOCK_THREAD_CPUTIME_ID timer counts the calling thread's CPU time, not time asleep", tm_cputime_thread),
            case("fork", "A child created by fork inherits none of its parent's timers", tm_fork),
            case("exec", "exec deletes the caller's timers: their IDs are invalid in the new program and they never fire", tm_exec),
        ]),
        category("itimers", "alarm & interval timers", &[
            case("alarm", "alarm(1) delivers SIGALRM no earlier than one second later, late by at most two ticks and 20 ms", it_alarm),
            case("alarm-default", "SIGALRM from alarm terminates a process that has not caught it", it_alarm_default),
            case("alarm-remaining", "alarm returns the seconds left of the previous alarm", it_alarm_remaining),
            case("alarm-zero", "alarm(0) cancels a pending alarm and returns the seconds it had left", it_alarm_zero),
            case("alarm-replace", "A new alarm replaces the old one: only the new one fires", it_alarm_replace),
            case("alarm-read", "An alarm interrupts a blocking read of an empty pipe with EINTR", it_alarm_read),
            case("real", "setitimer ITIMER_REAL with no interval fires SIGALRM once, on time", it_real),
            case("real-interval", "ITIMER_REAL reloads its interval: a 50 ms timer fires twenty times a second, 50 ms apart", it_real_interval),
            case("getitimer", "getitimer reports the time left, decreasing and never above the value set, and the interval", it_getitimer),
            case("old-value", "setitimer returns the previous setting in old_value", it_old_value),
            case("disarm", "setitimer with a zero it_value disarms the timer", it_disarm),
            case("small", "An ITIMER_REAL value of one microsecond still fires", it_small),
            case("einval", "setitimer and getitimer of an unknown timer, and setitimer with tv_usec out of range, fail with EINVAL", it_einval),
            case("alarm-shares-real", "Linux policy: alarm and setitimer ITIMER_REAL are one timer", it_alarm_shares_real),
            case("virtual", "ITIMER_VIRTUAL counts user CPU time: a 300 ms sleep uses almost none of it and sends no SIGVTALRM; computing sends one", it_virtual),
            case("virtual-interval", "ITIMER_VIRTUAL reloads its interval while the process computes", it_virtual_interval),
            case("prof", "ITIMER_PROF counts CPU time: no SIGPROF while the process sleeps, one while it computes", it_prof),
            case("fork", "fork clears the child's alarm and interval timers and leaves the parent's armed", it_fork),
            case("exec-alarm", "exec keeps the time left until an alarm, and the alarm fires in the new program", it_exec_alarm),
            case("exec-cpu", "Linux policy: exec keeps ITIMER_VIRTUAL and ITIMER_PROF", it_exec_cpu),
        ]),
    ],
);

fn main() { SUITE.run() }
