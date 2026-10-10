//! Threads: creating, joining and detaching threads, mutexes, condition variables,
//! rwlocks, barriers and once, thread-specific data and compiler TLS, per-thread
//! signals and scheduling parameters, as POSIX specifies them.
//!
//! Threads are measured through the interface a portable C program uses: the cases call
//! Breenix's own C library (libs/libbreenix-libc) for every pthread, sched and signal
//! function they test. A function the library does not define fails the case that needs
//! it, saying so: build.rs reads libc.a's symbol index and sets `libc_has = "<name>"`
//! for each function it has. Cases titled `Linux ABI:` call the kernel underneath
//! directly (futex, tgkill, rt_sigprocmask, sched_setscheduler) by its Linux numbers.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit,
//! and runs its threads in a process of its own below that (a trial), which reports
//! through shared memory. A thread that never returns from a lock or a wait therefore
//! fails the case with the step it was stuck in, rather than running the case out of
//! time, and the trial's process is killed when the case ends. Every wait is bounded.
//!
//! Cases that need several processors read the count from /proc/cpuinfo, skip below
//! two, and start one thread per processor, at most four. Each pins itself to its own
//! processor and the threads show they run at once before they measure: a thousand
//! rendezvous within 750 ms, which threads taking turns on one processor cannot make.
#![feature(thread_local)]

use libbreenix::process::{self, ForkResult};
use libbreenix::signal::Sigaction;
use libbreenix::suite::{case, case_ms_left, category, check, fail, suite, value, wait_for, CaseError, CaseResult, Suite};
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use std::cell::{Cell, UnsafeCell};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering::Relaxed, Ordering::SeqCst};
use std::sync::Arc;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const MMAP: u64 = 9;
    pub const MUNMAP: u64 = 11;
    pub const RT_SIGACTION: u64 = 13;
    pub const RT_SIGPROCMASK: u64 = 14;
    pub const SIGALTSTACK: u64 = 131;
    pub const NANOSLEEP: u64 = 35;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const SETUID: u64 = 105;
    pub const PRLIMIT64: u64 = 302;
    pub const RT_SIGPENDING: u64 = 127;
    pub const SCHED_SETPARAM: u64 = 142;
    pub const SCHED_GETPARAM: u64 = 143;
    pub const SCHED_SETSCHEDULER: u64 = 144;
    pub const SCHED_GETSCHEDULER: u64 = 145;
    pub const SCHED_GET_PRIORITY_MIN: u64 = 147;
    pub const ARCH_PRCTL: u64 = 158;
    pub const GETTID: u64 = 186;
    pub const FUTEX: u64 = 202;
    pub const SCHED_SETAFFINITY: u64 = 203;
    pub const CLOCK_SETTIME: u64 = 227;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231;
    pub const TGKILL: u64 = 234;
    pub const GETCPU: u64 = 309;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const MMAP: u64 = 222;
    pub const MUNMAP: u64 = 215;
    pub const RT_SIGACTION: u64 = 134;
    pub const RT_SIGPROCMASK: u64 = 135;
    pub const SIGALTSTACK: u64 = 132;
    pub const NANOSLEEP: u64 = 101;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const SETUID: u64 = 146;
    pub const PRLIMIT64: u64 = 261;
    pub const RT_SIGPENDING: u64 = 136;
    pub const SCHED_SETPARAM: u64 = 118;
    pub const SCHED_SETSCHEDULER: u64 = 119;
    pub const SCHED_GETSCHEDULER: u64 = 120;
    pub const SCHED_GETPARAM: u64 = 121;
    pub const SCHED_GET_PRIORITY_MIN: u64 = 126;
    pub const GETTID: u64 = 178;
    pub const FUTEX: u64 = 98;
    pub const SCHED_SETAFFINITY: u64 = 122;
    pub const CLOCK_SETTIME: u64 = 112;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const EXIT_GROUP: u64 = 94;
    pub const TGKILL: u64 = 131;
    pub const GETCPU: u64 = 168;
}

const EPERM: i32 = 1;
const EINTR: i32 = 4;
const EBADF: i32 = 9;
const EAGAIN: i32 = 11;
const EBUSY: i32 = 16;
const EINVAL: i32 = 22;
const EDEADLK: i32 = 35;
const ETIMEDOUT: i32 = 110;
const EOWNERDEAD: i32 = 130;
const ENOTRECOVERABLE: i32 = 131;

const SIGKILL: i32 = 9;
const SIGUSR1: i32 = 10;
const SIGSEGV: i32 = 11;
const SIGUSR2: i32 = 12;
const SIGTERM: i32 = 15;
const SA_SIGINFO: u64 = 4;
const SA_RESTORER: u64 = 0x0400_0000;
const SA_ONSTACK: u64 = 0x0800_0000;
const SIG_BLOCK: i32 = 0;
const SIG_UNBLOCK: i32 = 1;

const PTHREAD_CREATE_JOINABLE: i32 = 0;
const PTHREAD_CREATE_DETACHED: i32 = 1;
const PTHREAD_MUTEX_NORMAL: i32 = 0;
const PTHREAD_MUTEX_RECURSIVE: i32 = 1;
const PTHREAD_MUTEX_ERRORCHECK: i32 = 2;
const PTHREAD_MUTEX_DEFAULT: i32 = 0;
const PTHREAD_MUTEX_STALLED: i32 = 0;
const PTHREAD_MUTEX_ROBUST: i32 = 1;
const PTHREAD_PRIO_INHERIT: i32 = 1;
const PTHREAD_INHERIT_SCHED: i32 = 0;
const PTHREAD_EXPLICIT_SCHED: i32 = 1;
const PTHREAD_BARRIER_SERIAL_THREAD: i32 = -1;

const SCHED_OTHER: i32 = 0;
const SCHED_FIFO: i32 = 1;
const SCHED_RR: i32 = 2;

const SC_THREAD_DESTRUCTOR_ITERATIONS: i32 = 73;
const SC_THREAD_KEYS_MAX: i32 = 74;
const SC_THREAD_PRIO_INHERIT: i32 = 80;
/// The POSIX minimums of PTHREAD_DESTRUCTOR_ITERATIONS and PTHREAD_KEYS_MAX.
const POSIX_DESTRUCTOR_ITERATIONS: i64 = 4;
const POSIX_KEYS_MAX: i64 = 128;

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;

const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_CMP_REQUEUE: u64 = 4;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_PRIVATE: u64 = 128;
const FUTEX_CLOCK_REALTIME: u64 = 256;

const PROT_RW: u64 = 3;
const MAP_SHARED_ANON: u64 = 0x21;
const MAP_PRIVATE_ANON: u64 = 0x22;
const WNOHANG: i32 = 1;

const NS: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;
const KIB: usize = 1024;
const MIB: usize = 1 << 20;

/// The timer tick: 200 Hz on x86-64 and 1000 Hz on ARM64. A timed wait may end late by
/// at most two ticks and 20 ms, as the time suite holds its sleeps to.
#[cfg(target_arch = "x86_64")]
const TICK_MS: i64 = 5;
#[cfg(target_arch = "aarch64")]
const TICK_MS: i64 = 1;
const LATE_MS: i64 = 2 * TICK_MS + 20;
/// How soon a thread blocked on a lock or condition must be woken once it is released.
/// This is a liveness bound, not a performance target.
const WAKE_MS: i64 = 100;
/// Time a case keeps to report and clean up after its last bounded wait.
const CLEANUP_MS: u64 = 1500;
/// A user ID with no privileges.
const USER_A: u32 = 4242;

// ---------------------------------------------------------------------------
// The C library's functions.

/// Declare C library functions: each is a Rust function of the same name that calls the
/// library's function and returns `Ok(result)` when libc.a defines it, and fails the case
/// with "the C library has no <name>" when it does not.
macro_rules! libc {
    ($( $sym:literal fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => { $(
        #[cfg(libc_has = $sym)]
        fn $name($($arg: $ty),*) -> Result<$ret, CaseError> {
            extern "C" {
                #[link_name = $sym]
                fn f($($arg: $ty),*) -> $ret;
            }
            // SAFETY: every caller passes pointers to objects it keeps alive and large
            // enough for the C type, and function pointers of the C signature.
            Ok(unsafe { f($($arg),*) })
        }
        #[cfg(not(libc_has = $sym))]
        fn $name($($arg: $ty),*) -> Result<$ret, CaseError> {
            let _ = ($($arg,)*);
            Err(CaseError::Fail(concat!("the C library has no ", $sym).into()))
        }
    )* };
}

type Start = extern "C" fn(*mut u8) -> *mut u8;
type Destructor = Option<unsafe extern "C" fn(*mut u8)>;

libc! {
    "pthread_create" fn pthread_create(t: *mut usize, attr: *const u8, start: Start, arg: *mut u8) -> i32;
    "pthread_join" fn pthread_join(t: usize, value: *mut *mut u8) -> i32;
    "pthread_detach" fn pthread_detach(t: usize) -> i32;
    "pthread_self" fn pthread_self() -> usize;
    "pthread_equal" fn pthread_equal(a: usize, b: usize) -> i32;
    "pthread_attr_init" fn pthread_attr_init(attr: *mut u8) -> i32;
    "pthread_attr_setdetachstate" fn pthread_attr_setdetachstate(attr: *mut u8, state: i32) -> i32;
    "pthread_attr_getdetachstate" fn pthread_attr_getdetachstate(attr: *const u8, state: *mut i32) -> i32;
    "pthread_attr_setstacksize" fn pthread_attr_setstacksize(attr: *mut u8, size: usize) -> i32;
    "pthread_attr_getstacksize" fn pthread_attr_getstacksize(attr: *const u8, size: *mut usize) -> i32;
    "pthread_attr_setstack" fn pthread_attr_setstack(attr: *mut u8, addr: *mut u8, size: usize) -> i32;
    "pthread_attr_getstack" fn pthread_attr_getstack(attr: *const u8, addr: *mut *mut u8, size: *mut usize) -> i32;
    "pthread_attr_setguardsize" fn pthread_attr_setguardsize(attr: *mut u8, size: usize) -> i32;
    "pthread_attr_getguardsize" fn pthread_attr_getguardsize(attr: *const u8, size: *mut usize) -> i32;
    "pthread_attr_setinheritsched" fn pthread_attr_setinheritsched(attr: *mut u8, inherit: i32) -> i32;
    "pthread_attr_getinheritsched" fn pthread_attr_getinheritsched(attr: *const u8, inherit: *mut i32) -> i32;
    "pthread_attr_setschedpolicy" fn pthread_attr_setschedpolicy(attr: *mut u8, policy: i32) -> i32;
    "pthread_attr_getschedpolicy" fn pthread_attr_getschedpolicy(attr: *const u8, policy: *mut i32) -> i32;
    "pthread_attr_setschedparam" fn pthread_attr_setschedparam(attr: *mut u8, param: *const i32) -> i32;
    "pthread_attr_getschedparam" fn pthread_attr_getschedparam(attr: *const u8, param: *mut i32) -> i32;

    "pthread_mutex_init" fn pthread_mutex_init(m: *mut u8, attr: *const u8) -> i32;
    "pthread_mutex_destroy" fn pthread_mutex_destroy(m: *mut u8) -> i32;
    "pthread_mutex_lock" fn pthread_mutex_lock(m: *mut u8) -> i32;
    "pthread_mutex_trylock" fn pthread_mutex_trylock(m: *mut u8) -> i32;
    "pthread_mutex_timedlock" fn pthread_mutex_timedlock(m: *mut u8, abstime: *const i64) -> i32;
    "pthread_mutex_unlock" fn pthread_mutex_unlock(m: *mut u8) -> i32;
    "pthread_mutex_consistent" fn pthread_mutex_consistent(m: *mut u8) -> i32;
    "pthread_mutexattr_init" fn pthread_mutexattr_init(attr: *mut u8) -> i32;
    "pthread_mutexattr_settype" fn pthread_mutexattr_settype(attr: *mut u8, kind: i32) -> i32;
    "pthread_mutexattr_gettype" fn pthread_mutexattr_gettype(attr: *const u8, kind: *mut i32) -> i32;
    "pthread_mutexattr_setrobust" fn pthread_mutexattr_setrobust(attr: *mut u8, robust: i32) -> i32;
    "pthread_mutexattr_getrobust" fn pthread_mutexattr_getrobust(attr: *const u8, robust: *mut i32) -> i32;
    "pthread_mutexattr_setprotocol" fn pthread_mutexattr_setprotocol(attr: *mut u8, protocol: i32) -> i32;
    "pthread_mutexattr_getprotocol" fn pthread_mutexattr_getprotocol(attr: *const u8, protocol: *mut i32) -> i32;

    "pthread_cond_init" fn pthread_cond_init(c: *mut u8, attr: *const u8) -> i32;
    "pthread_cond_signal" fn pthread_cond_signal(c: *mut u8) -> i32;
    "pthread_cond_broadcast" fn pthread_cond_broadcast(c: *mut u8) -> i32;
    "pthread_cond_wait" fn pthread_cond_wait(c: *mut u8, m: *mut u8) -> i32;
    "pthread_cond_timedwait" fn pthread_cond_timedwait(c: *mut u8, m: *mut u8, abstime: *const i64) -> i32;
    "pthread_condattr_init" fn pthread_condattr_init(attr: *mut u8) -> i32;
    "pthread_condattr_setclock" fn pthread_condattr_setclock(attr: *mut u8, clock: i32) -> i32;
    "pthread_condattr_getclock" fn pthread_condattr_getclock(attr: *const u8, clock: *mut i32) -> i32;

    "pthread_rwlock_init" fn pthread_rwlock_init(rw: *mut u8, attr: *const u8) -> i32;
    "pthread_rwlock_rdlock" fn pthread_rwlock_rdlock(rw: *mut u8) -> i32;
    "pthread_rwlock_tryrdlock" fn pthread_rwlock_tryrdlock(rw: *mut u8) -> i32;
    "pthread_rwlock_timedrdlock" fn pthread_rwlock_timedrdlock(rw: *mut u8, abstime: *const i64) -> i32;
    "pthread_rwlock_wrlock" fn pthread_rwlock_wrlock(rw: *mut u8) -> i32;
    "pthread_rwlock_trywrlock" fn pthread_rwlock_trywrlock(rw: *mut u8) -> i32;
    "pthread_rwlock_timedwrlock" fn pthread_rwlock_timedwrlock(rw: *mut u8, abstime: *const i64) -> i32;
    "pthread_rwlock_unlock" fn pthread_rwlock_unlock(rw: *mut u8) -> i32;

    "pthread_barrier_init" fn pthread_barrier_init(b: *mut u8, attr: *const u8, count: u32) -> i32;
    "pthread_barrier_wait" fn pthread_barrier_wait(b: *mut u8) -> i32;
    "pthread_once" fn pthread_once(once: *mut i32, init: extern "C" fn()) -> i32;

    "pthread_key_create" fn pthread_key_create(key: *mut u32, destructor: Destructor) -> i32;
    "pthread_key_delete" fn pthread_key_delete(key: u32) -> i32;
    "pthread_getspecific" fn pthread_getspecific(key: u32) -> *mut u8;
    "pthread_setspecific" fn pthread_setspecific(key: u32, value: *const u8) -> i32;

    "pthread_kill" fn pthread_kill(t: usize, sig: i32) -> i32;
    "pthread_sigmask" fn pthread_sigmask(how: i32, set: *const u64, old: *mut u64) -> i32;
    "sigwait" fn sigwait(set: *const u64, sig: *mut i32) -> i32;

    "pthread_setschedparam" fn pthread_setschedparam(t: usize, policy: i32, param: *const i32) -> i32;
    "pthread_getschedparam" fn pthread_getschedparam(t: usize, policy: *mut i32, param: *mut i32) -> i32;
    "pthread_setschedprio" fn pthread_setschedprio(t: usize, prio: i32) -> i32;
    "sched_get_priority_max" fn sched_get_priority_max(policy: i32) -> i32;
    "sched_get_priority_min" fn sched_get_priority_min(policy: i32) -> i32;
    "sched_rr_get_interval" fn sched_rr_get_interval(pid: i32, interval: *mut i64) -> i32;
    "sched_yield" fn sched_yield() -> i32;

    "sysconf" fn sysconf(name: i32) -> i64;
    "__errno_location" fn errno_location() -> *mut i32;
    "raise" fn raise(sig: i32) -> i32;
    "kill" fn c_kill(pid: i32, sig: i32) -> i32;
    "getpid" fn c_getpid() -> i32;
    "close" fn c_close(fd: i32) -> i32;
    "pipe" fn c_pipe(fds: *mut i32) -> i32;
    "read" fn c_read(fd: i32, buf: *mut u8, len: usize) -> isize;
    "write" fn c_write(fd: i32, buf: *const u8, len: usize) -> isize;
}

/// pthread_exit, which does not return when the library has it.
#[cfg(libc_has = "pthread_exit")]
fn pthread_exit(value: *mut u8) -> CaseError {
    extern "C" {
        #[link_name = "pthread_exit"]
        fn f(value: *mut u8) -> !;
    }
    // SAFETY: ends the calling thread; nothing after the call runs.
    unsafe { f(value) }
}
#[cfg(not(libc_has = "pthread_exit"))]
fn pthread_exit(_value: *mut u8) -> CaseError { CaseError::Fail("the C library has no pthread_exit".into()) }
fn have_pthread_exit() -> CaseResult {
    if cfg!(libc_has = "pthread_exit") { Ok(()) } else { fail("the C library has no pthread_exit") }
}

/// The C library's exit, which does not return when the library has it.
#[cfg(libc_has = "exit")]
fn c_exit(status: i32) -> CaseError {
    extern "C" {
        #[link_name = "exit"]
        fn f(status: i32) -> !;
    }
    // SAFETY: ends the process; nothing after the call runs.
    unsafe { f(status) }
}
#[cfg(not(libc_has = "exit"))]
fn c_exit(_status: i32) -> CaseError { CaseError::Fail("the C library has no exit".into()) }

// ---------------------------------------------------------------------------
// System calls, results and time.

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

/// The signal restorer the suite's handlers return through.
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
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 9 => "EBADF", 11 => "EAGAIN",
        12 => "ENOMEM", 14 => "EFAULT", 16 => "EBUSY", 22 => "EINVAL", 35 => "EDEADLK", 38 => "ENOSYS",
        95 => "ENOTSUP", 110 => "ETIMEDOUT", 130 => "EOWNERDEAD", 131 => "ENOTRECOVERABLE",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A pthread-style return (0 or an error number) as text.
fn rc_text(v: i32) -> String {
    match v {
        0 => "0".into(),
        v if v > 0 => errname(v as i64),
        v => v.to_string(),
    }
}

/// A raw system-call return as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if (-4095..0).contains(&ret) { errname(-ret) } else { ret.to_string() } }

fn err<T>(msg: impl Into<String>) -> Result<T, CaseError> { Err(CaseError::Fail(msg.into())) }

/// A case error's message, for a worker that reports failures as text.
fn text(e: CaseError) -> String {
    match e {
        CaseError::Fail(m) | CaseError::Skip(m) => m,
    }
}

/// A function that returns 0 or an error number: anything but 0 fails, naming it.
fn rc(what: &str, got: i32) -> CaseResult {
    if got == 0 { Ok(()) } else { fail(format!("{what} returned {}", rc_text(got))) }
}

/// A function that must return the error number `errno`.
fn want(what: &str, got: i32, errno: i32) -> CaseResult {
    check(got == errno, &format!("{what} returned {}, expected {}", rc_text(got), errname(errno as i64)))
}

/// A raw system call that must return 0.
fn sys0(what: &str, ret: i64) -> CaseResult {
    check(ret == 0, &format!("{what} returned {}", shown(ret)))
}

fn gettid() -> i64 { sc(nr::GETTID, &[]) }
fn tgkill(tgid: i64, tid: i64, sig: i32) -> i64 { sc(nr::TGKILL, &[tgid as u64, tid as u64, sig as u64]) }

type Ts = [i64; 2];
fn ts(ns: i64) -> Ts { [ns.div_euclid(NS), ns.rem_euclid(NS)] }

fn clock_ns(clock: i32) -> i64 {
    let mut t: Ts = [0; 2];
    if sc(nr::CLOCK_GETTIME, &[clock as u64, t.as_mut_ptr() as u64]) != 0 { return 0; }
    t[0] * NS + t[1]
}
fn mono() -> i64 { clock_ns(CLOCK_MONOTONIC) }
fn rt() -> i64 { clock_ns(CLOCK_REALTIME) }
fn now_ms() -> u64 { (mono() / MS) as u64 }

/// The absolute time `ms` from now on `clock`, as a timespec.
fn deadline(clock: i32, ms: i64) -> Ts { ts(clock_ns(clock) + ms * MS) }
fn ts_ns(t: &Ts) -> i64 { t[0] * NS + t[1] }

/// `ms`, cut short so that `reserve` ms of the case's limit remain afterwards.
fn bounded(ms: u64, reserve: u64) -> u64 { ms.min(case_ms_left().saturating_sub(reserve)) }

/// Sleep `ms`, resuming after any interruption.
fn sleep_ms(ms: u64) {
    let end = mono() + ms as i64 * MS;
    loop {
        let left = end - mono();
        if left <= 0 { return; }
        let t = ts(left.min(10 * MS));
        let _ = sc(nr::NANOSLEEP, &[t.as_ptr() as u64, 0]);
    }
}

fn nap() {
    let t = ts(MS);
    let _ = sc(nr::NANOSLEEP, &[t.as_ptr() as u64, 0]);
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

/// Spin up to `ms` for `cond`, without sleeping.
fn spin_until(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let start = now_ms();
    let mut spins = 0u64;
    loop {
        if cond() { return true; }
        spins += 1;
        if spins % 1024 == 0 && now_ms().saturating_sub(start) >= ms { return false; }
        core::hint::spin_loop();
    }
}

/// How late a timed wait that had to run to `deadline_ns` ended, `end_ns`, on the same
/// clock: reported, and checked never early and late by at most LATE_MS.
fn on_time(what: &str, end_ns: i64, deadline_ns: i64) -> CaseResult {
    let late_us = (end_ns - deadline_ns) / 1000;
    value("late", late_us, "us", Some((0, LATE_MS * 1000)));
    check(end_ns >= deadline_ns, &format!("{what} returned {} us before its deadline", -late_us))?;
    check(late_us <= LATE_MS * 1000, &format!("{what} returned {late_us} us after its deadline, more than {LATE_MS} ms"))
}

/// How soon a blocked thread came back once it was released, `woke_ns` against
/// `released_ns`: reported, and checked not before the release and within WAKE_MS.
fn woken(what: &str, woke_ns: i64, released_ns: i64) -> CaseResult {
    let us = (woke_ns - released_ns) / 1000;
    value("wake", us, "us", Some((0, WAKE_MS * 1000)));
    check(woke_ns >= released_ns, &format!("{what} returned {} us before it was released", -us))?;
    check(us <= WAKE_MS * 1000, &format!("{what} returned {us} us after it was released, more than {WAKE_MS} ms"))
}

fn mmap_anon(len: usize, flags: u64) -> Result<*mut u8, CaseError> {
    let ret = sc(nr::MMAP, &[0, len as u64, PROT_RW, flags, u64::MAX, 0]);
    if (-4095..0).contains(&ret) { return err(format!("mmap of {len} bytes failed with {}", errname(-ret))); }
    Ok(ret as *mut u8)
}

fn leak<T>(v: T) -> &'static T { Box::leak(Box::new(v)) }

/// A pointer to an object threads share: a pthread object or attribute.
#[derive(Clone, Copy)]
struct P(*mut u8);
// SAFETY: the objects are made for sharing between threads; the C library synchronizes
// access to them.
unsafe impl Send for P {}
unsafe impl Sync for P {}
impl P {
    /// The pointer, through the whole value, so a closure captures the Send wrapper.
    fn p(self) -> *mut u8 { self.0 }
}

/// Zeroed storage for one pthread object or attribute, larger and more aligned than any of
/// them on the Linux ABI, and never freed.
#[repr(C, align(16))]
struct Obj([u8; 256]);
fn obj() -> P { P(Box::leak(Box::new(Obj([0; 256]))).0.as_mut_ptr()) }

/// A signal set as the Linux ABI's 128-byte sigset_t lays it out.
type SigSet = [u64; 16];
fn sigset(sigs: &[i32]) -> SigSet {
    let mut set = [0u64; 16];
    for &s in sigs { set[0] |= 1 << (s - 1); }
    set
}
fn has(set: &SigSet, sig: i32) -> bool { set[0] & (1 << (sig - 1)) != 0 }

// ---------------------------------------------------------------------------
// Trials: a case's threads run in a process of its own, which reports through shared
// memory, so one stuck in a call fails the case with the step it was in.

/// The page a trial and the case share: the result, the step the trial is at, and a few
/// words for the case's own use.
#[repr(C)]
struct Board {
    state: AtomicU32,
    msg_len: AtomicU32,
    step_len: AtomicU32,
    words: [AtomicI64; 16],
    msg: UnsafeCell<[u8; 256]>,
    step: [AtomicU8; 160],
}
const RUNNING: u32 = 0;
const PASSED: u32 = 1;
const FAILED: u32 = 2;
const SKIPPED: u32 = 3;

/// In a trial's process, its board.
static BOARD: AtomicUsize = AtomicUsize::new(0);

fn board() -> Option<&'static Board> {
    let p = BOARD.load(SeqCst);
    // SAFETY: set only to a shared mapping of a Board that outlives the process.
    if p == 0 { None } else { Some(unsafe { &*(p as *const Board) }) }
}

fn put_text(buf: &UnsafeCell<[u8; 256]>, len: &AtomicU32, text: &str) {
    let n = text.len().min(255);
    // SAFETY: the board's bytes are written here and read by the case after the trial
    // ends or sets its state.
    unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), (*buf.get()).as_mut_ptr(), n) };
    len.store(n as u32, SeqCst);
}

/// Say what the trial is doing now, for the message if it never finishes. Any of a trial's
/// threads may call it; the bytes are atomic, so two at once only garble the message.
fn step(text: &str) {
    let Some(b) = board() else { return };
    let n = text.len().min(b.step.len());
    for (cell, &byte) in b.step.iter().zip(&text.as_bytes()[..n]) { cell.store(byte, Relaxed); }
    b.step_len.store(n as u32, SeqCst);
}

fn text_of(bytes: &[u8], len: u32) -> String {
    String::from_utf8_lossy(&bytes[..(len as usize).min(bytes.len())]).into_owned()
}

impl Board {
    fn msg(&self) -> String {
        // SAFETY: read after the trial set its state or ended.
        text_of(unsafe { &*self.msg.get() }, self.msg_len.load(SeqCst))
    }
    fn last_step(&self) -> String {
        let bytes: Vec<u8> = self.step.iter().map(|cell| cell.load(Relaxed)).collect();
        let s = text_of(&bytes, self.step_len.load(SeqCst));
        if s.is_empty() { String::new() } else { format!(" (last step: {s})") }
    }
    fn word(&self, i: usize) -> i64 { self.words[i].load(SeqCst) }
}

fn report(b: &Board, result: CaseResult) {
    let (state, msg) = match result {
        Ok(()) => (PASSED, String::new()),
        Err(CaseError::Fail(m)) => (FAILED, m),
        Err(CaseError::Skip(m)) => (SKIPPED, m),
    };
    put_text(&b.msg, &b.msg_len, &msg);
    b.state.store(state, SeqCst);
}

/// End the whole process, as exit_group does, so a trial's threads stop with it.
fn exit_group(status: i32) -> ! {
    sc(nr::EXIT_GROUP, &[status as u64]);
    loop { core::hint::spin_loop(); }
}

/// A process a case started, with the board it shares. Dropping it kills and reaps it.
struct Proc { pid: i32, board: &'static Board, reaped: bool }

/// Start `body` in a process of its own. If `body` returns, its result goes on the board
/// and the process ends with status 0 for a pass and 1 otherwise.
fn spawn_proc(body: impl FnOnce() -> CaseResult) -> Result<Proc, CaseError> {
    let page = mmap_anon(4096, MAP_SHARED_ANON)?;
    // SAFETY: a fresh zeroed shared page, large enough for a Board and never unmapped.
    let board: &'static Board = unsafe { &*(page as *const Board) };
    match process::fork() {
        Ok(ForkResult::Child) => {
            BOARD.store(page as usize, SeqCst);
            let result = body();
            let ok = result.is_ok();
            report(board, result);
            exit_group(if ok { 0 } else { 1 })
        }
        Ok(ForkResult::Parent(pid)) => Ok(Proc { pid: pid.raw() as i32, board, reaped: false }),
        Err(e) => err(format!("fork failed: {e}")),
    }
}

/// A wait status in words.
fn status_text(status: i32) -> String {
    let sig = status & 0x7f;
    if sig == 0 { format!("exited with status {}", (status >> 8) & 0xff) } else { format!("was killed by signal {sig}") }
}

impl Proc {
    /// The process's wait status once it ends, within `ms`; None if it is still running.
    fn wait(&mut self, ms: u64) -> Option<i32> {
        let mut status = 0;
        let ms = bounded(ms, CLEANUP_MS);
        let start = now_ms();
        loop {
            let r = sc(nr::WAIT4, &[self.pid as u64, &mut status as *mut i32 as u64, WNOHANG as u64, 0]);
            if r == self.pid as i64 { self.reaped = true; return Some(status); }
            if r != 0 || now_ms().saturating_sub(start) >= ms { return None; }
            nap();
        }
    }

    /// Kill the process and reap it, waiting up to a second. Fails if the kill is refused,
    /// wait4 fails, or the process is not reaped within the second.
    fn stop(&mut self) -> CaseResult {
        if self.reaped { return Ok(()); }
        let k = sc(nr::KILL, &[self.pid as u64, SIGKILL as u64]);
        if k != 0 { return fail(format!("kill(SIGKILL) of the case's process {} returned {}", self.pid, shown(k))); }
        let mut status = 0;
        let start = now_ms();
        loop {
            let r = sc(nr::WAIT4, &[self.pid as u64, &mut status as *mut i32 as u64, WNOHANG as u64, 0]);
            if r == self.pid as i64 { self.reaped = true; return Ok(()); }
            if r != 0 && r != -(EINTR as i64) { return fail(format!("wait4 for the case's process {} returned {}", self.pid, shown(r))); }
            if now_ms().saturating_sub(start) >= 1000 {
                return fail(format!("the case's process {} was not reaped within a second of SIGKILL", self.pid));
            }
            nap();
        }
    }

    /// Wait up to `ms` for the process to put a result on its board or end. A result is
    /// returned as the case's; a process that ended without one, or ran too long, fails.
    /// The process is killed and reaped before its result is read.
    fn result(&mut self, ms: u64) -> CaseResult {
        let ms = bounded(ms, CLEANUP_MS);
        let start = now_ms();
        let mut status = 0;
        loop {
            let state = self.board.state.load(SeqCst);
            if state != RUNNING {
                self.stop()?;
                return match state {
                    PASSED => Ok(()),
                    SKIPPED => Err(CaseError::Skip(self.board.msg())),
                    _ => fail(self.board.msg()),
                };
            }
            if !self.reaped {
                let r = sc(nr::WAIT4, &[self.pid as u64, &mut status as *mut i32 as u64, WNOHANG as u64, 0]);
                if r == self.pid as i64 {
                    self.reaped = true;
                    if self.board.state.load(SeqCst) != RUNNING { continue; }
                    return fail(format!("the case's process {} without a result{}", status_text(status), self.board.last_step()));
                }
            }
            if now_ms().saturating_sub(start) >= ms {
                let stopped = self.stop().err().map(|e| format!("; {}", text(e))).unwrap_or_default();
                return fail(format!("the case did not finish within {ms} ms{}{stopped}", self.board.last_step()));
            }
            nap();
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) { let _ = self.stop(); }
}

/// Run `body` as a trial with all the time the case has left.
fn trial(body: impl FnOnce() -> CaseResult) -> CaseResult { trial_ms(u64::MAX, body) }

/// Run `body` as a trial that must finish within `ms`.
fn trial_ms(ms: u64, body: impl FnOnce() -> CaseResult) -> CaseResult { spawn_proc(body)?.result(ms) }

// ---------------------------------------------------------------------------
// Threads the cases start to do their work, through the C library's pthread_create.

struct Slot<T> { done: AtomicU32, value: UnsafeCell<Option<T>> }
// SAFETY: value is written once by the thread before done is set and read only after.
unsafe impl<T: Send> Sync for Slot<T> {}

struct Th<T> { handle: usize, slot: Arc<Slot<T>>, name: String }

extern "C" fn trampoline<F: FnOnce() -> T, T>(arg: *mut u8) -> *mut u8 {
    // SAFETY: spawn passed a Box<(F, Arc<Slot<T>>)> leaked for this thread alone.
    let (f, slot) = *unsafe { Box::from_raw(arg as *mut (F, Arc<Slot<T>>)) };
    let v = f();
    // SAFETY: only this thread writes the slot, before done is set.
    unsafe { *slot.value.get() = Some(v) };
    slot.done.store(1, SeqCst);
    null_mut()
}

/// Start a thread running `f` with pthread_create and default attributes.
fn spawn<T: Send + 'static, F: FnOnce() -> T + Send + 'static>(name: &str, f: F) -> Result<Th<T>, CaseError> {
    let slot = Arc::new(Slot { done: AtomicU32::new(0), value: UnsafeCell::new(None) });
    let arg = Box::into_raw(Box::new((f, slot.clone()))) as *mut u8;
    let mut handle = 0usize;
    let r = pthread_create(&mut handle, null(), trampoline::<F, T>, arg)?;
    if r != 0 {
        // SAFETY: the thread was not started, so the box is still ours.
        drop(unsafe { Box::from_raw(arg as *mut (F, Arc<Slot<T>>)) });
        return err(format!("pthread_create for {name} returned {}", rc_text(r)));
    }
    Ok(Th { handle, slot, name: name.to_string() })
}

impl<T> Th<T> {
    fn finished(&self) -> bool { self.slot.done.load(SeqCst) != 0 }

    /// The thread's result, once it has finished, within `ms`.
    fn wait(self, ms: u64) -> Result<T, CaseError> {
        if !until(ms, || self.finished()) {
            let at = board().map(|b| b.last_step()).unwrap_or_default();
            return err(format!("{} did not finish within {ms} ms{at}", self.name));
        }
        // SAFETY: done is set, so the thread wrote the value and no longer touches it.
        unsafe { (*self.slot.value.get()).take() }.ok_or_else(|| CaseError::Fail(format!("{} left no result", self.name)))
    }
}

/// A thread that takes a lock, holds it until released or until `ms` pass, and records
/// when it let go.
struct Holder { release: Arc<AtomicU32>, unlocked: Arc<AtomicI64>, th: Th<CaseResult> }

fn holder(
    what: &'static str,
    ms: u64,
    lock: impl FnOnce() -> Result<i32, CaseError> + Send + 'static,
    unlock: impl FnOnce() -> Result<i32, CaseError> + Send + 'static,
) -> Result<Holder, CaseError> {
    let held = Arc::new(AtomicU32::new(0));
    let release = Arc::new(AtomicU32::new(0));
    let unlocked = Arc::new(AtomicI64::new(0));
    let (h, r, u) = (held.clone(), release.clone(), unlocked.clone());
    let th = spawn("the holding thread", move || -> CaseResult {
        rc(&format!("{what} in the holding thread"), lock()?)?;
        h.store(1, SeqCst);
        let start = now_ms();
        while r.load(SeqCst) == 0 && now_ms().saturating_sub(start) < ms { nap(); }
        u.store(mono(), SeqCst);
        rc(&format!("unlocking in the holding thread after {what}"), unlock()?)
    })?;
    if !until(2000, || held.load(SeqCst) != 0 || th.finished()) {
        return err(format!("the holding thread's {what} did not return within 2 s"));
    }
    if held.load(SeqCst) == 0 { th.wait(100)??; return err(format!("the holding thread did not take the lock with {what}")); }
    Ok(Holder { release, unlocked, th })
}

impl Holder {
    fn release(&self) { self.release.store(1, SeqCst); }
    fn unlocked_at(&self) -> i64 { self.unlocked.load(SeqCst) }
    fn finish(self) -> CaseResult {
        self.release();
        self.th.wait(3000)?
    }
}

// ---------------------------------------------------------------------------
// Pthread objects.

fn new_mutex(kind: Option<i32>, robust: bool) -> Result<P, CaseError> {
    let m = obj();
    if kind.is_none() && !robust {
        rc("pthread_mutex_init", pthread_mutex_init(m.p(), null())?)?;
        return Ok(m);
    }
    let a = obj();
    rc("pthread_mutexattr_init", pthread_mutexattr_init(a.p())?)?;
    if let Some(k) = kind {
        rc(&format!("pthread_mutexattr_settype({})", type_name(k)), pthread_mutexattr_settype(a.p(), k)?)?;
    }
    if robust {
        rc("pthread_mutexattr_setrobust(PTHREAD_MUTEX_ROBUST)", pthread_mutexattr_setrobust(a.p(), PTHREAD_MUTEX_ROBUST)?)?;
    }
    rc("pthread_mutex_init", pthread_mutex_init(m.p(), a.p())?)?;
    Ok(m)
}

fn type_name(k: i32) -> &'static str {
    match k {
        PTHREAD_MUTEX_NORMAL => "PTHREAD_MUTEX_NORMAL",
        PTHREAD_MUTEX_RECURSIVE => "PTHREAD_MUTEX_RECURSIVE",
        PTHREAD_MUTEX_ERRORCHECK => "PTHREAD_MUTEX_ERRORCHECK",
        _ => "an invalid type",
    }
}

fn new_cond(clock: Option<i32>) -> Result<P, CaseError> {
    let c = obj();
    match clock {
        None => rc("pthread_cond_init", pthread_cond_init(c.p(), null())?)?,
        Some(clock) => {
            let a = obj();
            rc("pthread_condattr_init", pthread_condattr_init(a.p())?)?;
            rc("pthread_condattr_setclock", pthread_condattr_setclock(a.p(), clock)?)?;
            rc("pthread_cond_init", pthread_cond_init(c.p(), a.p())?)?;
        }
    }
    Ok(c)
}

fn new_rwlock() -> Result<P, CaseError> {
    let rw = obj();
    rc("pthread_rwlock_init", pthread_rwlock_init(rw.p(), null())?)?;
    Ok(rw)
}

fn lock(m: P) -> CaseResult { rc("pthread_mutex_lock", pthread_mutex_lock(m.p())?) }
fn unlock(m: P) -> CaseResult { rc("pthread_mutex_unlock", pthread_mutex_unlock(m.p())?) }
fn cond_wait(c: P, m: P) -> CaseResult { rc("pthread_cond_wait", pthread_cond_wait(c.p(), m.p())?) }
fn signal(c: P) -> CaseResult { rc("pthread_cond_signal", pthread_cond_signal(c.p())?) }
fn broadcast(c: P) -> CaseResult { rc("pthread_cond_broadcast", pthread_cond_broadcast(c.p())?) }

/// trylock from a thread of its own (unlocking again if it got the mutex): what another
/// thread sees.
fn trylock_elsewhere(m: P) -> Result<i32, CaseError> {
    spawn("the trylock thread", move || -> Result<i32, CaseError> {
        let r = pthread_mutex_trylock(m.p())?;
        if r == 0 { unlock(m)?; }
        Ok(r)
    })?
    .wait(2000)?
}

/// trywrlock from a thread of its own (unlocking again if it got the lock).
fn trywrlock_elsewhere(rw: P) -> Result<i32, CaseError> {
    spawn("the trywrlock thread", move || -> Result<i32, CaseError> {
        let r = pthread_rwlock_trywrlock(rw.p())?;
        if r == 0 { rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)?; }
        Ok(r)
    })?
    .wait(2000)?
}

/// The calling thread's pthread_t.
fn me() -> Result<usize, CaseError> { pthread_self() }

/// Set the calling thread's policy and priority.
fn set_sched(policy: i32, prio: i32) -> CaseResult {
    let param = prio;
    rc(&format!("pthread_setschedparam({}, {prio})", policy_name(policy)), pthread_setschedparam(me()?, policy, &param)?)
}

/// The calling thread's policy and priority.
fn get_sched(t: usize) -> Result<(i32, i32), CaseError> {
    let (mut policy, mut param) = (-1, -1);
    rc("pthread_getschedparam", pthread_getschedparam(t, &mut policy, &mut param)?)?;
    Ok((policy, param))
}

fn policy_name(p: i32) -> String {
    match p {
        SCHED_OTHER => "SCHED_OTHER".into(),
        SCHED_FIFO => "SCHED_FIFO".into(),
        SCHED_RR => "SCHED_RR".into(),
        p => format!("policy {p}"),
    }
}

/// The lowest SCHED_FIFO priority, from sched_get_priority_min.
fn fifo_min() -> Result<i32, CaseError> {
    let lo = sched_get_priority_min(SCHED_FIFO)?;
    if lo < 0 { return err(format!("sched_get_priority_min(SCHED_FIFO) returned {lo}")); }
    Ok(lo)
}

// ---------------------------------------------------------------------------
// Threads on several processors.

/// Rendezvous all of a team's threads make within HANDOFF_MS: threads sharing a processor
/// meet only when one is preempted, at most once per timer tick, so they need at least
/// HANDOFFS ms.
const HANDOFFS: u64 = 1000;
const HANDOFF_MS: u64 = 750;
/// The most threads a team starts; it starts one per processor online.
const MAX_TEAM: usize = 4;

/// Processors online, from the `processor` lines of /proc/cpuinfo. A census that cannot
/// be read or counts none fails the case rather than passing for one processor.
fn processors() -> Result<usize, CaseError> {
    let info = std::fs::read_to_string("/proc/cpuinfo").map_err(|e| format!("reading /proc/cpuinfo failed: {e}"))?;
    let n = info.lines().filter(|line| line.starts_with("processor")).count();
    if n == 0 { return err("/proc/cpuinfo lists no processor"); }
    Ok(n)
}

/// The threads a team case starts: one per processor, at most MAX_TEAM. Skips below two.
fn team_size() -> Result<usize, CaseError> {
    let cpus = processors()?;
    if cpus < 2 { return Err(CaseError::Skip(format!("{cpus} processor online; the case needs 2"))); }
    Ok(cpus.min(MAX_TEAM))
}

/// The processor the calling thread runs on, from getcpu.
fn this_cpu() -> Option<u32> {
    let mut cpu = u32::MAX;
    if sc(nr::GETCPU, &[&mut cpu as *mut u32 as u64, 0, 0]) != 0 { return None; }
    Some(cpu)
}

/// Pin the calling thread to processor `cpu` and wait up to a second for it to run there.
fn pin_to(cpu: u32) -> Result<(), String> {
    let mask = 1u64 << cpu;
    let r = sc(nr::SCHED_SETAFFINITY, &[0, 8, &mask as *const u64 as u64]);
    if r != 0 { return Err(format!("sched_setaffinity to processor {cpu} returned {}", shown(r))); }
    if until(1000, || this_cpu() == Some(cpu)) { Ok(()) } else {
        Err(format!("pinned to processor {cpu}, a thread still ran on processor {:?} after a second", this_cpu()))
    }
}

struct TeamState { pinned: AtomicU64, arrived: AtomicU64, failed: AtomicU32 }

/// Start `n` threads, thread i pinned to processor i; show that they run at once with
/// HANDOFFS rendezvous within HANDOFF_MS; then run `body(i)` in each and return their
/// results in order.
fn team<T: Send + 'static>(
    n: usize,
    body: impl Fn(usize) -> Result<T, String> + Send + Sync + 'static,
) -> Result<Vec<T>, CaseError> {
    let state = Arc::new(TeamState { pinned: AtomicU64::new(0), arrived: AtomicU64::new(0), failed: AtomicU32::new(0) });
    let body = Arc::new(body);
    let mut threads = Vec::new();
    for i in 0..n {
        let (s, b) = (state.clone(), body.clone());
        threads.push(spawn(&format!("worker {i}"), move || -> Result<T, String> {
            let stop = |msg: String| -> Result<T, String> { s.failed.store(1, SeqCst); Err(msg) };
            if let Err(e) = pin_to(i as u32) { return stop(e); }
            s.pinned.fetch_add(1, SeqCst);
            if !spin_until(2000, || s.pinned.load(SeqCst) == n as u64 || s.failed.load(SeqCst) != 0) {
                return stop(format!("worker {i} waited 2 s for the others to pin themselves"));
            }
            let start = now_ms();
            for r in 0..HANDOFFS {
                s.arrived.fetch_add(1, SeqCst);
                let target = n as u64 * (r + 1);
                let left = HANDOFF_MS.saturating_sub(now_ms().saturating_sub(start));
                if !spin_until(left, || s.arrived.load(SeqCst) >= target || s.failed.load(SeqCst) != 0) || s.failed.load(SeqCst) != 0 {
                    return stop(format!(
                        "the {n} threads made only {r} of {HANDOFFS} rendezvous in {HANDOFF_MS} ms, so they did not run at the same time"
                    ));
                }
            }
            if i == 0 { value("rendezvous", now_ms().saturating_sub(start) as i64, "ms", Some((0, HANDOFF_MS as i64))); }
            if this_cpu() != Some(i as u32) {
                return stop(format!("pinned to processor {i}, worker {i} ran on processor {:?}", this_cpu()));
            }
            b(i)
        })?);
    }
    let mut out = Vec::new();
    for th in threads {
        out.push(th.wait(bounded(8000, CLEANUP_MS))?.map_err(CaseError::Fail)?);
    }
    Ok(out)
}

/// A u64 several threads update under a lock the case is testing with a separate load and
/// store, so a lock that does not exclude loses updates. The accesses are relaxed atomics:
/// a broken lock lets them interleave, which is the measurement, without a data race.
struct Racy(AtomicU64);
impl Racy {
    fn new() -> Racy { Racy(AtomicU64::new(0)) }
    fn get(&self) -> u64 { self.0.load(Relaxed) }
    fn set(&self, v: u64) { self.0.store(v, Relaxed) }
}

// ---------------------------------------------------------------------------
// lifecycle

extern "C" fn rt_flag_after_sleep(arg: *mut u8) -> *mut u8 {
    sleep_ms(200);
    // SAFETY: the case passed a leaked AtomicU32.
    unsafe { (*(arg as *const AtomicU32)).store(1, SeqCst) };
    arg
}

extern "C" fn rt_flag(arg: *mut u8) -> *mut u8 {
    // SAFETY: the case passed a leaked AtomicU32.
    unsafe { (*(arg as *const AtomicU32)).store(1, SeqCst) };
    null_mut()
}

extern "C" fn rt_return_value(_: *mut u8) -> *mut u8 { 0x5a5a_1234usize as *mut u8 }

fn create(start: Start, arg: *mut u8) -> Result<usize, CaseError> {
    let mut t = 0usize;
    rc("pthread_create", pthread_create(&mut t, null(), start, arg)?)?;
    Ok(t)
}

fn create_attr(attr: P, start: Start, arg: *mut u8) -> Result<usize, CaseError> {
    let mut t = 0usize;
    rc("pthread_create with the attributes", pthread_create(&mut t, attr.p(), start, arg)?)?;
    Ok(t)
}

fn new_attr() -> Result<P, CaseError> {
    let a = obj();
    rc("pthread_attr_init", pthread_attr_init(a.p())?)?;
    Ok(a)
}

fn lc_create_join() -> CaseResult {
    trial(|| {
        let flag = leak(AtomicU32::new(0));
        let t = create(rt_flag_after_sleep, flag as *const AtomicU32 as *mut u8)?;
        let t0 = mono();
        step("in pthread_join for a thread that returns after 200 ms");
        rc("pthread_join", pthread_join(t, null_mut())?)?;
        let waited = (mono() - t0) / MS;
        value("join-wait", waited, "ms", None);
        check(flag.load(SeqCst) == 1, &format!("pthread_join returned after {waited} ms, before the start routine had returned"))
    })
}

fn lc_join_value() -> CaseResult {
    trial(|| {
        let t = create(rt_return_value, null_mut())?;
        let mut v = 0xdeadusize as *mut u8;
        step("in pthread_join");
        rc("pthread_join", pthread_join(t, &mut v)?)?;
        check(v as usize == 0x5a5a_1234, &format!("pthread_join stored {:#x}, not the 0x5a5a1234 the start routine returned", v as usize))
    })
}

static AFTER_EXIT: AtomicU32 = AtomicU32::new(0);

#[inline(never)]
fn exit_from(depth: u32) {
    if depth == 0 {
        let _ = pthread_exit(0x77abusize as *mut u8);
    } else {
        exit_from(core::hint::black_box(depth - 1));
    }
}

extern "C" fn rt_exit_nested(_: *mut u8) -> *mut u8 {
    exit_from(3);
    AFTER_EXIT.store(1, SeqCst);
    null_mut()
}

fn lc_exit_value() -> CaseResult {
    trial(|| {
        have_pthread_exit()?;
        let t = create(rt_exit_nested, null_mut())?;
        let mut v = null_mut();
        step("in pthread_join for a thread that calls pthread_exit");
        rc("pthread_join", pthread_join(t, &mut v)?)?;
        check(AFTER_EXIT.load(SeqCst) == 0, "the thread ran on after pthread_exit")?;
        check(v as usize == 0x77ab, &format!("pthread_join stored {:#x}, not the 0x77ab passed to pthread_exit", v as usize))
    })
}

extern "C" fn rt_flag_return(arg: *mut u8) -> *mut u8 {
    // SAFETY: the case passed a leaked AtomicU32.
    unsafe { (*(arg as *const AtomicU32)).store(1, SeqCst) };
    0x99usize as *mut u8
}

fn lc_join_ended() -> CaseResult {
    trial(|| {
        let flag = leak(AtomicU32::new(0));
        let t = create(rt_flag_return, flag as *const AtomicU32 as *mut u8)?;
        check(until(1000, || flag.load(SeqCst) == 1), "the thread did not run within a second")?;
        sleep_ms(100);
        let mut v = null_mut();
        let t0 = mono();
        step("in pthread_join for a thread that ended 100 ms ago");
        rc("pthread_join", pthread_join(t, &mut v)?)?;
        let us = (mono() - t0) / 1000;
        value("join", us, "us", Some((0, WAKE_MS * 1000)));
        check(us <= WAKE_MS * 1000, &format!("pthread_join of a thread that had ended took {us} us"))?;
        check(v as usize == 0x99, &format!("pthread_join stored {:#x}, not the 0x99 the thread returned", v as usize))
    })
}

static SEEN_SELF: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

extern "C" fn rt_record_self(arg: *mut u8) -> *mut u8 {
    SEEN_SELF[arg as usize].store(pthread_self().unwrap_or(0), SeqCst);
    sleep_ms(300);
    null_mut()
}

fn lc_self_equal() -> CaseResult {
    trial(|| {
        let t = create(rt_record_self, null_mut())?;
        check(until(1000, || SEEN_SELF[0].load(SeqCst) != 0), "the thread did not record pthread_self within a second")?;
        let seen = SEEN_SELF[0].load(SeqCst);
        check(pthread_equal(seen, t)? != 0, &format!("pthread_self in the thread ({seen:#x}) is not pthread_equal to the ID pthread_create gave ({t:#x})"))?;
        let main = me()?;
        check(pthread_equal(main, me()?)? != 0, "pthread_self is not pthread_equal to itself")?;
        Ok(())
    })
}

fn lc_self_distinct() -> CaseResult {
    trial(|| {
        let a = create(rt_record_self, null_mut())?;
        let b = create(rt_record_self, 1usize as *mut u8)?;
        check(until(1000, || SEEN_SELF.iter().all(|s| s.load(SeqCst) != 0)), "the threads did not record pthread_self within a second")?;
        let ids = [me()?, SEEN_SELF[0].load(SeqCst), SEEN_SELF[1].load(SeqCst)];
        for i in 0..3 {
            for j in 0..3 {
                let eq = pthread_equal(ids[i], ids[j])? != 0;
                check(eq == (i == j), &format!("pthread_equal({:#x}, {:#x}) returned {}", ids[i], ids[j], eq))?;
            }
        }
        let _ = (a, b);
        Ok(())
    })
}

fn lc_join_self() -> CaseResult {
    trial_ms(3000, || {
        step("in pthread_join(pthread_self())");
        want("pthread_join(pthread_self())", pthread_join(me()?, null_mut())?, EDEADLK)
    })
}

extern "C" fn rt_sleep_1s(_: *mut u8) -> *mut u8 { sleep_ms(1000); null_mut() }

fn lc_join_detached() -> CaseResult {
    trial_ms(3000, || {
        let t = create(rt_sleep_1s, null_mut())?;
        rc("pthread_detach", pthread_detach(t)?)?;
        let t0 = mono();
        step("in pthread_join of a detached thread");
        let r = pthread_join(t, null_mut())?;
        let ms = (mono() - t0) / MS;
        check(r == EINVAL, &format!("pthread_join of a detached, running thread returned {} after {ms} ms, expected EINVAL", rc_text(r)))
    })
}

fn lc_detach_running() -> CaseResult {
    trial(|| {
        let flag = leak(AtomicU32::new(0));
        let t = create(rt_flag_after_sleep, flag as *const AtomicU32 as *mut u8)?;
        rc("pthread_detach of a running thread", pthread_detach(t)?)?;
        check(until(2000, || flag.load(SeqCst) == 1), "a detached thread did not run to the end of its start routine within 2 s")
    })
}

fn lc_detach_ended() -> CaseResult {
    trial(|| {
        let flag = leak(AtomicU32::new(0));
        let t = create(rt_flag, flag as *const AtomicU32 as *mut u8)?;
        check(until(1000, || flag.load(SeqCst) == 1), "the thread did not run within a second")?;
        sleep_ms(50);
        step("in pthread_detach of a thread that has ended");
        rc("pthread_detach of a thread that has ended", pthread_detach(t)?)
    })
}

/// A field of /proc/<pid>/status in kB.
fn status_kb(field: &str) -> Result<i64, CaseError> {
    let pid = c_getpid()?;
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| format!("reading /proc/{pid}/status failed: {e}"))?;
    let line = text.lines().find(|l| l.starts_with(field)).ok_or_else(|| format!("/proc/{pid}/status has no {field}"))?;
    line[field.len()..].trim().trim_end_matches("kB").trim().parse().map_err(|_| CaseError::Fail(format!("cannot read {line:?}")))
}

static COUNT: AtomicU32 = AtomicU32::new(0);
extern "C" fn rt_count(_: *mut u8) -> *mut u8 { COUNT.fetch_add(1, SeqCst); null_mut() }

fn lc_detach_many() -> CaseResult {
    const N: u32 = 256;
    trial(|| {
        let before = status_kb("VmRSS:")?;
        for i in 0..N {
            let t = create(rt_count, null_mut())?;
            rc("pthread_detach", pthread_detach(t)?)?;
            if !until(1000, || COUNT.load(SeqCst) > i) { return err(format!("detached thread {i} did not run within a second")); }
        }
        sleep_ms(100);
        let grown = status_kb("VmRSS:")? - before;
        value("threads", N as i64, "", Some((N as i64, N as i64)));
        value("rss-growth", grown, "kb", None);
        check(grown <= 1024, &format!("{N} detached threads that ended left the process {grown} kB larger"))
    })
}

fn lc_attr_default() -> CaseResult {
    trial_ms(3000, || {
        let a = new_attr()?;
        let mut state = -1;
        rc("pthread_attr_getdetachstate", pthread_attr_getdetachstate(a.p(), &mut state)?)?;
        check(state == PTHREAD_CREATE_JOINABLE, &format!("a new attribute object's detach state is {state}, not PTHREAD_CREATE_JOINABLE"))
    })
}

fn lc_attr_detached() -> CaseResult {
    trial(|| {
        let a = new_attr()?;
        rc("pthread_attr_setdetachstate(PTHREAD_CREATE_DETACHED)", pthread_attr_setdetachstate(a.p(), PTHREAD_CREATE_DETACHED)?)?;
        let mut state = -1;
        rc("pthread_attr_getdetachstate", pthread_attr_getdetachstate(a.p(), &mut state)?)?;
        check(state == PTHREAD_CREATE_DETACHED, &format!("pthread_attr_getdetachstate gave {state} after PTHREAD_CREATE_DETACHED was set"))?;
        let flag = leak(AtomicU32::new(0));
        create_attr(a, rt_flag, flag as *const AtomicU32 as *mut u8)?;
        check(until(1000, || flag.load(SeqCst) == 1), "a thread created detached did not run within a second")
    })
}

/// Use `depth` frames of a little over 4 KiB each of the stack, touching every page.
#[inline(never)]
fn burn_stack(depth: usize) -> u64 {
    let mut page = [0u8; 4000];
    page[0] = depth as u8;
    page[3999] = 1;
    let page = core::hint::black_box(&mut page);
    if depth == 0 { return page[0] as u64; }
    burn_stack(depth - 1) + core::hint::black_box(page[3999] as u64)
}

extern "C" fn rt_burn_6mib(arg: *mut u8) -> *mut u8 {
    step("a thread using 6 MiB of the 8 MiB stack it was created with");
    let _ = core::hint::black_box(burn_stack(6 * MIB / 4096));
    // SAFETY: the case passed a leaked AtomicU32.
    unsafe { (*(arg as *const AtomicU32)).store(1, SeqCst) };
    null_mut()
}

fn lc_attr_stacksize() -> CaseResult {
    trial(|| {
        let a = new_attr()?;
        rc("pthread_attr_setstacksize(8 MiB)", pthread_attr_setstacksize(a.p(), 8 * MIB)?)?;
        let mut size = 0usize;
        rc("pthread_attr_getstacksize", pthread_attr_getstacksize(a.p(), &mut size)?)?;
        check(size == 8 * MIB, &format!("pthread_attr_getstacksize gave {size} after 8 MiB was set"))?;
        let flag = leak(AtomicU32::new(0));
        create_attr(a, rt_burn_6mib, flag as *const AtomicU32 as *mut u8)?;
        check(until(3000, || flag.load(SeqCst) == 1), "the thread did not finish using 6 MiB of its stack within 3 s")
    })
}

static LOCAL_AT: AtomicUsize = AtomicUsize::new(0);
extern "C" fn rt_local_address(_: *mut u8) -> *mut u8 {
    let x = core::hint::black_box(7u64);
    LOCAL_AT.store(&x as *const u64 as usize, SeqCst);
    null_mut()
}

fn lc_attr_stack() -> CaseResult {
    trial(|| {
        let len = MIB;
        let buf = mmap_anon(len, MAP_PRIVATE_ANON)?;
        let a = new_attr()?;
        rc("pthread_attr_setstack", pthread_attr_setstack(a.p(), buf, len)?)?;
        let (mut addr, mut size) = (null_mut(), 0usize);
        rc("pthread_attr_getstack", pthread_attr_getstack(a.p(), &mut addr, &mut size)?)?;
        check(addr == buf && size == len, &format!("pthread_attr_getstack gave {:#x}+{size} after {:#x}+{len} was set", addr as usize, buf as usize))?;
        create_attr(a, rt_local_address, null_mut())?;
        check(until(1000, || LOCAL_AT.load(SeqCst) != 0), "the thread did not run within a second")?;
        let at = LOCAL_AT.load(SeqCst);
        check((buf as usize..buf as usize + len).contains(&at), &format!("the thread's local variable is at {at:#x}, outside the stack it was given at {:#x}", buf as usize))
    })
}

fn lc_attr_stacksize_min() -> CaseResult {
    trial_ms(3000, || {
        let a = new_attr()?;
        want("pthread_attr_setstacksize(1)", pthread_attr_setstacksize(a.p(), 1)?, EINVAL)
    })
}

fn lc_attr_guardsize() -> CaseResult {
    trial_ms(3000, || {
        let a = new_attr()?;
        rc("pthread_attr_setguardsize(3 pages)", pthread_attr_setguardsize(a.p(), 3 * 4096)?)?;
        let mut size = 0usize;
        rc("pthread_attr_getguardsize", pthread_attr_getguardsize(a.p(), &mut size)?)?;
        check(size == 3 * 4096, &format!("pthread_attr_getguardsize gave {size} after 3 pages (12288) were set"))
    })
}

/// Recurse without end, recording on the board how far below `top` the stack has reached.
#[inline(never)]
fn dive(top: usize) -> u64 {
    let mut page = [0u8; 1000];
    page[0] = 1;
    let here = core::hint::black_box(&mut page).as_ptr() as usize;
    if let Some(b) = board() { b.words[0].store(top.saturating_sub(here) as i64, SeqCst); }
    if core::hint::black_box(page[0]) == 0 { return 0; }
    dive(top) + core::hint::black_box(page[0] as u64)
}

/// Board words of `guard-overflow`: how deep the thread got (0), that it started or the
/// error its sigaltstack returned (1), the address and thread its SIGSEGV handler saw (2, 5),
/// the diving thread's ID (6), that the handler ran (7) and the thread's stack top (8).
const DIVE_DEPTH: usize = 0;
const DIVE_STARTED: usize = 1;
const FAULT_ADDR: usize = 2;
const FAULT_TID: usize = 5;
const DIVER_TID: usize = 6;
const FAULT_SEEN: usize = 7;
const DIVE_TOP: usize = 8;

/// The SIGSEGV handler of `guard-overflow`, run on the faulting thread's alternate stack: it
/// records the faulting address and thread, restores the default action and returns, so the
/// access faults again and the default action ends the process.
extern "C" fn on_overflow(_sig: i32, info: *const u8, _ctx: *mut u8) {
    if let Some(b) = board() {
        if !info.is_null() {
            // SAFETY: the kernel passes a siginfo_t, whose si_addr is at offset 16 on both ABIs.
            let addr = unsafe { core::ptr::read_unaligned(info.add(16) as *const u64) };
            b.words[FAULT_ADDR].store(addr as i64, SeqCst);
        }
        b.words[FAULT_TID].store(gettid(), SeqCst);
        b.words[FAULT_SEEN].store(1, SeqCst);
    }
    let dfl = Sigaction { handler: 0, flags: 0, restorer: 0, mask: 0 };
    sc(nr::RT_SIGACTION, &[SIGSEGV as u64, &dfl as *const Sigaction as u64, 0, 8]);
}

extern "C" fn rt_dive(_: *mut u8) -> *mut u8 {
    let x = 0u64;
    let Some(b) = board() else { return null_mut() };
    const ALT: usize = 64 * KIB;
    let alt = match mmap_anon(ALT, MAP_PRIVATE_ANON) {
        Ok(p) => p,
        Err(_) => { b.words[DIVE_STARTED].store(-12, SeqCst); return null_mut(); }
    };
    let stack: [u64; 3] = [alt as u64, 0, ALT as u64];
    let r = sc(nr::SIGALTSTACK, &[stack.as_ptr() as u64, 0]);
    if r != 0 { b.words[DIVE_STARTED].store(r.min(-1), SeqCst); return null_mut(); }
    let top = core::hint::black_box(&x) as *const u64 as usize;
    b.words[DIVE_TOP].store(top as i64, SeqCst);
    b.words[DIVER_TID].store(gettid(), SeqCst);
    b.words[DIVE_STARTED].store(1, SeqCst);
    let _ = dive(top);
    null_mut()
}

fn lc_guard_overflow() -> CaseResult {
    const STACK: usize = 256 * KIB;
    const GUARD: usize = 64 * KIB;
    let mut p = spawn_proc(|| {
        let act = Sigaction {
            handler: on_overflow as usize as u64,
            flags: SA_SIGINFO | SA_ONSTACK | SA_RESTORER,
            restorer: restore_rt as usize as u64,
            mask: 0,
        };
        sys0("rt_sigaction(SIGSEGV)", sc(nr::RT_SIGACTION, &[SIGSEGV as u64, &act as *const Sigaction as u64, 0, 8]))?;
        let a = new_attr()?;
        rc("pthread_attr_setstacksize(256 KiB)", pthread_attr_setstacksize(a.p(), STACK)?)?;
        rc("pthread_attr_setguardsize(64 KiB)", pthread_attr_setguardsize(a.p(), GUARD)?)?;
        create_attr(a, rt_dive, null_mut())?;
        sleep_ms(3000);
        let b = board().ok_or("no board")?;
        let started = b.word(DIVE_STARTED);
        if started < 0 { return err(format!("giving the overflowing thread an alternate signal stack failed with {}", shown(started))); }
        err("the thread recursed for 3 s without a fault")
    })?;
    let status = p.wait(5000);
    if p.board.state.load(SeqCst) == FAILED { return fail(p.board.msg()); }
    let Some(status) = status else { return fail("the process overflowing a thread's stack was still running after 5 s") };
    let b = p.board;
    let deep = b.word(DIVE_DEPTH) as usize / KIB;
    value("depth", deep as i64, "kb", Some((128, ((STACK + GUARD) / KIB) as i64)));
    check(b.word(DIVE_STARTED) == 1, "the thread never started")?;
    check(status & 0x7f == SIGSEGV, &format!("the process overflowing a thread's 256 KiB stack {}, not killed by SIGSEGV", status_text(status)))?;
    check(b.word(FAULT_SEEN) == 1, "the process was killed by SIGSEGV but its handler, on the thread's alternate stack, never ran, so the fault is not placed")?;
    let (faulted, diver) = (b.word(FAULT_TID), b.word(DIVER_TID));
    check(faulted == diver, &format!("the SIGSEGV was taken in thread {faulted}, not in thread {diver}, which was overflowing its stack"))?;
    let (top, addr) = (b.word(DIVE_TOP) as u64, b.word(FAULT_ADDR) as u64);
    let below = top.checked_sub(addr).map(|d| d as usize / KIB);
    check(
        below.is_some_and(|kb| (128..=(STACK + GUARD) / KIB).contains(&kb)),
        &format!("the fault was at {addr:#x}, which is not between 128 KiB and the 256 KiB stack plus its 64 KiB guard below the thread's stack top {top:#x}"),
    )?;
    check(deep <= (STACK + GUARD) / KIB, &format!("the thread ran {deep} KiB deep in a 256 KiB stack with a 64 KiB guard before the fault"))?;
    check(deep >= 128, &format!("the thread faulted only {deep} KiB into a 256 KiB stack"))
}

fn lc_many_threads() -> CaseResult {
    // _POSIX_THREAD_THREADS_MAX: the fewest threads per process POSIX allows a system.
    const N: usize = 64;
    trial(|| {
        // Each thread counts itself in `alive` and stays until the case has seen all N in at
        // once; one that gives up waiting counts itself out before it returns, so `alive`
        // reaching N means N threads were live together.
        let alive = Arc::new(AtomicU32::new(0));
        let seen = Arc::new(AtomicU32::new(0));
        let mut threads = Vec::new();
        for i in 0..N {
            let (a, s) = (alive.clone(), seen.clone());
            threads.push(spawn(&format!("thread {i}"), move || -> Option<usize> {
                a.fetch_add(1, SeqCst);
                let t0 = now_ms();
                while s.load(SeqCst) == 0 && now_ms() - t0 < 4000 { nap(); }
                if s.load(SeqCst) == 0 { a.fetch_sub(1, SeqCst); return None; }
                Some(i * 7 + 1)
            })?);
        }
        let all = until(4000, || alive.load(SeqCst) == N as u32);
        if all { seen.store(1, SeqCst); }
        let most = alive.load(SeqCst);
        value("alive-at-once", if all { N as i64 } else { most as i64 }, "", Some((N as i64, N as i64)));
        check(all, &format!("only {most} of {N} threads were running at once"))?;
        for (i, th) in threads.into_iter().enumerate() {
            let v = th.wait(3000)?;
            check(v == Some(i * 7 + 1), &format!("thread {i} returned {v:?}, not its own {}", i * 7 + 1))?;
        }
        Ok(())
    })
}

extern "C" fn rt_survivor(_: *mut u8) -> *mut u8 {
    sleep_ms(300);
    if let Some(b) = board() { b.words[0].store(1, SeqCst); }
    null_mut()
}

fn lc_main_exit() -> CaseResult {
    let mut p = spawn_proc(|| {
        have_pthread_exit()?;
        create(rt_survivor, null_mut())?;
        Err(pthread_exit(null_mut()))
    })?;
    sleep_ms(100);
    let early = p.wait(0);
    if let Some(status) = early {
        if p.board.state.load(SeqCst) == FAILED { return fail(p.board.msg()); }
        return fail(format!("the process {} when its main thread called pthread_exit, while another thread ran", status_text(status)));
    }
    let Some(status) = p.wait(3000) else { return fail("the process was still running 3 s after its last thread ended") };
    check(p.board.word(0) == 1, "the other thread did not run to its end")?;
    check(status == 0, &format!("after its last thread ended the process {}, not exited with status 0", status_text(status)))
}

fn lc_exit_from_thread() -> CaseResult {
    let mut p = spawn_proc(|| {
        let forever = spawn("the sleeping thread", || -> () { loop { sleep_ms(1000) } })?;
        spawn("the exiting thread", || {
            sleep_ms(100);
            let e = c_exit(5);
            if let (Some(b), CaseError::Fail(m)) = (board(), e) { report(b, fail(m)); }
        })?;
        step("in pthread_join of a thread that never ends");
        let _ = pthread_join(forever.handle, null_mut());
        err("pthread_join returned for a thread that never ends")
    })?;
    let status = p.wait(2000);
    if p.board.state.load(SeqCst) == FAILED { return fail(p.board.msg()); }
    let Some(status) = status else { return fail("2 s after a thread called exit(5), the process was still running") };
    check(status == 5 << 8, &format!("after a thread called exit(5) the process {}", status_text(status)))
}

/// Board words for a process's spinning threads: the count of those that started, and one
/// heartbeat word per thread from HEARTBEAT on.
const STARTED: usize = 3;
const HEARTBEAT: usize = 4;

/// Start `n` threads that count themselves in on the board and then, for up to 10 s, keep
/// bumping a heartbeat word of their own. Waits up to 2 s for all of them to be beating.
fn heartbeat_threads(n: usize) -> CaseResult {
    let b = board().ok_or("no board")?;
    for i in 0..n {
        spawn(&format!("spinner {i}"), move || {
            let Some(b) = board() else { return };
            b.words[STARTED].fetch_add(1, SeqCst);
            let t0 = now_ms();
            while now_ms() - t0 < 10_000 { b.words[HEARTBEAT + i].fetch_add(1, SeqCst); }
        })?;
    }
    let beating = || b.word(STARTED) == n as i64 && (0..n).all(|i| b.word(HEARTBEAT + i) > 0);
    check(until(2000, beating), &format!("{} of {n} threads started within 2 s", b.word(STARTED)))
}

/// After a process with `n` heartbeat threads has been reaped: every thread had started, and
/// none still runs, so no heartbeat word moves over 200 ms. The board page is shared with
/// this process, so a thread that outlived its process would still be seen beating.
fn heartbeats_stopped(b: &Board, n: usize) -> CaseResult {
    check(b.word(STARTED) == n as i64, &format!("only {} of the process's {n} threads had started", b.word(STARTED)))?;
    let before: Vec<i64> = (0..n).map(|i| b.word(HEARTBEAT + i)).collect();
    sleep_ms(200);
    let running = (0..n).filter(|&i| b.word(HEARTBEAT + i) != before[i]).count();
    value("threads-still-running", running as i64, "", Some((0, 0)));
    check(running == 0, &format!("{running} of the process's {n} threads were still running 200 ms after it was reaped"))
}

fn lc_exit_from_main() -> CaseResult {
    let mut p = spawn_proc(|| {
        heartbeat_threads(3)?;
        Err(c_exit(3))
    })?;
    let status = p.wait(3000);
    if p.board.state.load(SeqCst) == FAILED { return fail(p.board.msg()); }
    let Some(status) = status else { return fail("the process was still running 3 s after its main thread started three spinning threads and called exit(3)") };
    check(status == 3 << 8, &format!("after the main thread called exit(3) the process {}", status_text(status)))?;
    heartbeats_stopped(p.board, 3)
}

/// The most rounds `exit-tid-word` makes (it stops at the first change it finds), the pages
/// it maps after each, and the pattern it fills them with.
const TID_ROUNDS: usize = 12;
const TID_PAGES: usize = 32;
const PATTERN: u8 = 0xa5;

/// Count the thread in on the board, then sleep a second.
extern "C" fn rt_count_and_sleep(_: *mut u8) -> *mut u8 {
    if let Some(b) = board() { b.words[3].fetch_add(1, SeqCst); }
    sleep_ms(1000);
    null_mut()
}

fn lc_exit_tid_word() -> CaseResult {
    // Each round starts a process that makes two threads (each with an exit-cleared
    // thread-ID word, CLONE_CHILD_CLEARTID, in a page of its own), waits until both run
    // and sleep, and then ends its main thread with exit_group. The case kills and reaps it, maps fresh pages
    // at once, fills them with a pattern, waits and checks every byte: the dead process's
    // threads, ending, may write only to their own memory. The case waits 50 ms after the
    // process reports before killing it, so the kill does not race its main thread's exit.
    let mut hits = 0;
    let mut first = None;
    for round in 0..TID_ROUNDS {
        let mut p = spawn_proc(|| {
            create(rt_count_and_sleep, null_mut())?;
            create(rt_count_and_sleep, null_mut())?;
            let b = board().ok_or("no board")?;
            check(until(1000, || b.word(3) == 2), "the two threads did not both start within a second")
        })?;
        if !until(2000, || p.board.state.load(SeqCst) != RUNNING) {
            return fail(format!("round {round}: the process did not start its two threads within 2 s{}", p.board.last_step()));
        }
        if p.board.state.load(SeqCst) == FAILED { return fail(p.board.msg()); }
        sleep_ms(50);
        p.stop()?;
        let len = TID_PAGES * 4096;
        let mem = mmap_anon(len, MAP_PRIVATE_ANON)?;
        // SAFETY: a fresh private mapping of len bytes, unmapped below.
        unsafe { core::ptr::write_bytes(mem, PATTERN, len) };
        sleep_ms(50);
        // SAFETY: as above.
        let bytes = unsafe { core::slice::from_raw_parts(mem, len) };
        if let Some(at) = bytes.iter().position(|&b| b != PATTERN) {
            hits += 1;
            let changed = bytes.iter().filter(|&&b| b != PATTERN).count();
            first.get_or_insert((round, at % 4096, changed));
        }
        let _ = sc(nr::MUNMAP, &[mem as u64, len as u64]);
        if first.is_some() { break; }
    }
    value("rounds-changed", hits, "", Some((0, 0)));
    match first {
        None => Ok(()),
        Some((round, offset, changed)) => fail(format!(
            "in round {} of up to {TID_ROUNDS}, {changed} bytes of pages mapped just after killing and reaping a process with sleeping threads changed, at page offset {offset}",
            round + 1
        )),
    }
}

fn lc_getpid_shared() -> CaseResult {
    trial(|| {
        let pid = c_getpid()?;
        let main_tid = gettid();
        let mut threads = Vec::new();
        for i in 0..3 {
            threads.push(spawn(&format!("thread {i}"), || -> Result<(i32, i64), CaseError> { Ok((c_getpid()?, gettid())) })?);
        }
        let mut tids = vec![main_tid];
        for th in threads {
            let (p, tid) = th.wait(2000)??;
            check(p == pid, &format!("getpid in a thread returned {p}, not the process's {pid}"))?;
            check(!tids.contains(&tid), &format!("two threads share thread ID {tid}"))?;
            tids.push(tid);
        }
        Ok(())
    })
}

fn lc_shared_fds() -> CaseResult {
    trial(|| {
        let th = spawn("the thread making a pipe", || -> Result<[i32; 2], CaseError> {
            let mut fds = [-1i32; 2];
            check(c_pipe(fds.as_mut_ptr())? == 0, "pipe in the thread failed")?;
            check(c_write(fds[1], b"t".as_ptr(), 1)? == 1, "write in the thread failed")?;
            Ok(fds)
        })?;
        let fds = th.wait(2000)??;
        let mut byte = [0u8; 1];
        let n = c_read(fds[0], byte.as_mut_ptr(), 1)?;
        // SAFETY: __errno_location points to the calling thread's errno.
        let errno = unsafe { *errno_location()? };
        if n < 0 { return fail(format!("reading the pipe another thread made failed with {} in the main thread", rc_text(errno))); }
        check(n == 1 && byte[0] == b't', &format!("reading the pipe another thread made returned {n} bytes in the main thread"))?;
        rc("close", c_close(fds[0])?)?;
        rc("close", c_close(fds[1])?)
    })
}

// ---------------------------------------------------------------------------
// mutex

fn mx_lock_unlock() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(None, false)?;
        lock(m)?;
        unlock(m)?;
        rc("pthread_mutex_trylock of a free mutex", pthread_mutex_trylock(m.p())?)?;
        unlock(m)?;
        rc("pthread_mutex_destroy", pthread_mutex_destroy(m.p())?)
    })
}

fn mx_static_init() -> CaseResult {
    trial_ms(3000, || {
        let m = obj();
        lock(m)?;
        want("trylock from another thread of a zero-filled mutex the main thread locked", trylock_elsewhere(m)?, EBUSY)?;
        unlock(m)?;
        rc("trylock from another thread once it was unlocked", trylock_elsewhere(m)?)
    })
}

fn mx_exclusion() -> CaseResult {
    trial(|| {
        let m = new_mutex(None, false)?;
        let h = holder("pthread_mutex_lock", 200, move || pthread_mutex_lock(m.p()), move || pthread_mutex_unlock(m.p()))?;
        step("in pthread_mutex_lock while another thread holds the mutex for 200 ms");
        lock(m)?;
        let got = mono();
        unlock(m)?;
        let released = h.unlocked_at();
        check(released != 0, "pthread_mutex_lock returned while the other thread still held the mutex")?;
        woken("pthread_mutex_lock", got, released)?;
        h.finish()
    })
}

fn mx_trylock_ebusy() -> CaseResult {
    trial(|| {
        let m = new_mutex(None, false)?;
        let h = holder("pthread_mutex_lock", 3000, move || pthread_mutex_lock(m.p()), move || pthread_mutex_unlock(m.p()))?;
        want("pthread_mutex_trylock while another thread holds it", pthread_mutex_trylock(m.p())?, EBUSY)?;
        h.finish()?;
        rc("pthread_mutex_trylock once it was unlocked", pthread_mutex_trylock(m.p())?)?;
        unlock(m)
    })
}

fn mx_trylock_owner() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(Some(PTHREAD_MUTEX_NORMAL), false)?;
        lock(m)?;
        want("pthread_mutex_trylock by the owner of a normal mutex", pthread_mutex_trylock(m.p())?, EBUSY)?;
        unlock(m)
    })
}

fn mx_contention() -> CaseResult {
    const ITERS: u64 = 10_000;
    let n = team_size()?;
    trial(move || {
        let m = new_mutex(None, false)?;
        let counter = Arc::new(Racy::new());
        let c = counter.clone();
        let contended = team(n, move |i| -> Result<u64, String> {
            let mut contended = 0;
            for _ in 0..ITERS {
                match pthread_mutex_trylock(m.p()).map_err(text)? {
                    0 => {}
                    EBUSY => {
                        contended += 1;
                        let r = pthread_mutex_lock(m.p()).map_err(text)?;
                        if r != 0 { return Err(format!("pthread_mutex_lock in worker {i} returned {}", rc_text(r))); }
                    }
                    r => return Err(format!("pthread_mutex_trylock in worker {i} returned {}", rc_text(r))),
                }
                c.set(c.get() + 1);
                let r = pthread_mutex_unlock(m.p()).map_err(text)?;
                if r != 0 { return Err(format!("pthread_mutex_unlock in worker {i} returned {}", rc_text(r))); }
            }
            Ok(contended)
        })?;
        let total = counter.get();
        let contended: u64 = contended.iter().sum();
        value("increments", total as i64, "", Some(((n as u64 * ITERS) as i64, (n as u64 * ITERS) as i64)));
        value("contended", contended as i64, "", None);
        check(total == n as u64 * ITERS, &format!("{n} threads each made {ITERS} increments under the mutex, and the counter reads {total}"))?;
        check(contended > 0, "no thread ever found the mutex held, so the threads never contended")
    })
}

fn mx_errorcheck_relock() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(Some(PTHREAD_MUTEX_ERRORCHECK), false)?;
        lock(m)?;
        step("relocking an error-checking mutex the thread holds");
        want("pthread_mutex_lock by the owner of an error-checking mutex", pthread_mutex_lock(m.p())?, EDEADLK)
    })
}

fn mx_errorcheck_unlock_other() -> CaseResult {
    trial(|| {
        let m = new_mutex(Some(PTHREAD_MUTEX_ERRORCHECK), false)?;
        let h = holder("pthread_mutex_lock", 3000, move || pthread_mutex_lock(m.p()), move || pthread_mutex_unlock(m.p()))?;
        want("pthread_mutex_unlock of an error-checking mutex another thread holds", pthread_mutex_unlock(m.p())?, EPERM)?;
        want("pthread_mutex_trylock after the refused unlock", pthread_mutex_trylock(m.p())?, EBUSY)?;
        h.finish()
    })
}

fn mx_errorcheck_unlock_unlocked() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(Some(PTHREAD_MUTEX_ERRORCHECK), false)?;
        want("pthread_mutex_unlock of an unlocked error-checking mutex", pthread_mutex_unlock(m.p())?, EPERM)
    })
}

fn mx_recursive() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(Some(PTHREAD_MUTEX_RECURSIVE), false)?;
        step("locking a recursive mutex three times");
        for k in 1..=3 { rc(&format!("pthread_mutex_lock number {k} of a recursive mutex"), pthread_mutex_lock(m.p())?)?; }
        for k in 1..=3 {
            unlock(m)?;
            let r = trylock_elsewhere(m)?;
            if k < 3 {
                want(&format!("trylock from another thread after {k} of 3 unlocks"), r, EBUSY)?;
            } else {
                rc("trylock from another thread after the third unlock", r)?;
            }
        }
        Ok(())
    })
}

fn mx_recursive_trylock() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(Some(PTHREAD_MUTEX_RECURSIVE), false)?;
        lock(m)?;
        rc("pthread_mutex_trylock by the owner of a recursive mutex", pthread_mutex_trylock(m.p())?)?;
        unlock(m)?;
        want("trylock from another thread with one lock still held", trylock_elsewhere(m)?, EBUSY)?;
        unlock(m)?;
        rc("trylock from another thread once both were released", trylock_elsewhere(m)?)
    })
}

fn mx_recursive_unlock_other() -> CaseResult {
    trial(|| {
        let m = new_mutex(Some(PTHREAD_MUTEX_RECURSIVE), false)?;
        let h = holder("pthread_mutex_lock", 3000, move || pthread_mutex_lock(m.p()), move || pthread_mutex_unlock(m.p()))?;
        want("pthread_mutex_unlock of a recursive mutex another thread holds", pthread_mutex_unlock(m.p())?, EPERM)?;
        h.finish()
    })
}

fn mx_attr_type() -> CaseResult {
    trial_ms(3000, || {
        let a = obj();
        rc("pthread_mutexattr_init", pthread_mutexattr_init(a.p())?)?;
        let mut kind = -1;
        rc("pthread_mutexattr_gettype", pthread_mutexattr_gettype(a.p(), &mut kind)?)?;
        check(kind == PTHREAD_MUTEX_DEFAULT, &format!("a new mutex attribute's type is {kind}, not PTHREAD_MUTEX_DEFAULT"))?;
        for k in [PTHREAD_MUTEX_NORMAL, PTHREAD_MUTEX_ERRORCHECK, PTHREAD_MUTEX_RECURSIVE, PTHREAD_MUTEX_DEFAULT] {
            rc(&format!("pthread_mutexattr_settype({})", type_name(k)), pthread_mutexattr_settype(a.p(), k)?)?;
            rc("pthread_mutexattr_gettype", pthread_mutexattr_gettype(a.p(), &mut kind)?)?;
            check(kind == k, &format!("pthread_mutexattr_gettype gave {kind} after {} was set", type_name(k)))?;
        }
        want("pthread_mutexattr_settype(99)", pthread_mutexattr_settype(a.p(), 99)?, EINVAL)
    })
}

fn mx_timedlock_free() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(None, false)?;
        let when = deadline(CLOCK_REALTIME, 1000);
        let t0 = mono();
        rc("pthread_mutex_timedlock of a free mutex", pthread_mutex_timedlock(m.p(), when.as_ptr())?)?;
        let us = (mono() - t0) / 1000;
        check(us <= WAKE_MS * 1000, &format!("pthread_mutex_timedlock of a free mutex took {us} us"))?;
        unlock(m)
    })
}

fn mx_timedlock_timeout() -> CaseResult {
    trial_ms(3000, || {
        let m = new_mutex(None, false)?;
        let h = holder("pthread_mutex_lock", 3000, move || pthread_mutex_lock(m.p()), move || pthread_mutex_unlock(m.p()))?;
        let when = deadline(CLOCK_REALTIME, 200);
        wait_for(200, "late");
        step("in pthread_mutex_timedlock with a deadline 200 ms ahead, the mutex held elsewhere");
        let r = pthread_mutex_timedlock(m.p(), when.as_ptr())?;
        let end = rt();
        want("pthread_mutex_timedlock of a held mutex", r, ETIMEDOUT)?;
        on_time("pthread_mutex_timedlock", end, ts_ns(&when))?;
        h.finish()
    })
}

fn mx_timedlock_acquire() -> CaseResult {
    trial(|| {
        let m = new_mutex(None, false)?;
        let h = holder("pthread_mutex_lock", 100, move || pthread_mutex_lock(m.p()), move || pthread_mutex_unlock(m.p()))?;
        let when = deadline(CLOCK_REALTIME, 2000);
        step("in pthread_mutex_timedlock with a deadline 2 s ahead, the mutex released after 100 ms");
        let r = pthread_mutex_timedlock(m.p(), when.as_ptr())?;
        let got = mono();
        rc("pthread_mutex_timedlock of a mutex released before the deadline", r)?;
        check(rt() < ts_ns(&when), "pthread_mutex_timedlock returned only at its deadline")?;
        woken("pthread_mutex_timedlock", got, h.unlocked_at())?;
        unlock(m)?;
        h.finish()
    })
}

fn mx_timedlock_einval() -> CaseResult {
    trial(|| {
        let m = new_mutex(None, false)?;
        let h = holder("pthread_mutex_lock", 3000, move || pthread_mutex_lock(m.p()), move || pthread_mutex_unlock(m.p()))?;
        let bad: Ts = [rt() / NS + 1, NS];
        step("in pthread_mutex_timedlock with tv_nsec of 1000000000");
        want("pthread_mutex_timedlock with tv_nsec 1000000000 on a held mutex", pthread_mutex_timedlock(m.p(), bad.as_ptr())?, EINVAL)?;
        h.finish()
    })
}

/// Lock `m` in a thread that then ends without unlocking it.
fn die_holding(m: P) -> CaseResult {
    spawn("the thread that ends holding the mutex", move || -> CaseResult { lock(m) })?.wait(2000)??;
    sleep_ms(50);
    Ok(())
}

fn mx_robust_ownerdead() -> CaseResult {
    trial_ms(4000, || {
        let m = new_mutex(None, true)?;
        die_holding(m)?;
        step("locking a robust mutex whose owner ended holding it");
        want("pthread_mutex_lock of a robust mutex whose owner ended holding it", pthread_mutex_lock(m.p())?, EOWNERDEAD)?;
        rc("pthread_mutex_consistent", pthread_mutex_consistent(m.p())?)?;
        unlock(m)?;
        rc("pthread_mutex_lock once it was made consistent", pthread_mutex_lock(m.p())?)?;
        unlock(m)
    })
}

fn mx_robust_notrecoverable() -> CaseResult {
    trial_ms(4000, || {
        let m = new_mutex(None, true)?;
        die_holding(m)?;
        step("locking a robust mutex whose owner ended holding it");
        want("pthread_mutex_lock of a robust mutex whose owner ended holding it", pthread_mutex_lock(m.p())?, EOWNERDEAD)?;
        unlock(m)?;
        step("locking a robust mutex unlocked without pthread_mutex_consistent");
        want("pthread_mutex_lock after it was unlocked without pthread_mutex_consistent", pthread_mutex_lock(m.p())?, ENOTRECOVERABLE)
    })
}

fn mx_robust_attr() -> CaseResult {
    trial_ms(3000, || {
        let a = obj();
        rc("pthread_mutexattr_init", pthread_mutexattr_init(a.p())?)?;
        let mut robust = -1;
        rc("pthread_mutexattr_getrobust", pthread_mutexattr_getrobust(a.p(), &mut robust)?)?;
        check(robust == PTHREAD_MUTEX_STALLED, &format!("a new mutex attribute's robustness is {robust}, not PTHREAD_MUTEX_STALLED"))?;
        rc("pthread_mutexattr_setrobust(PTHREAD_MUTEX_ROBUST)", pthread_mutexattr_setrobust(a.p(), PTHREAD_MUTEX_ROBUST)?)?;
        rc("pthread_mutexattr_getrobust", pthread_mutexattr_getrobust(a.p(), &mut robust)?)?;
        check(robust == PTHREAD_MUTEX_ROBUST, &format!("pthread_mutexattr_getrobust gave {robust} after PTHREAD_MUTEX_ROBUST was set"))?;
        want("pthread_mutexattr_setrobust(99)", pthread_mutexattr_setrobust(a.p(), 99)?, EINVAL)
    })
}

fn mx_prio_inherit() -> CaseResult {
    let offered = trial_value(|| Ok(sysconf(SC_THREAD_PRIO_INHERIT)?))?;
    if offered <= 0 {
        return Err(CaseError::Skip(format!("sysconf(_SC_THREAD_PRIO_INHERIT) returns {offered}: the option is not offered")));
    }
    trial(|| {
        let a = obj();
        rc("pthread_mutexattr_init", pthread_mutexattr_init(a.p())?)?;
        rc("pthread_mutexattr_setprotocol(PTHREAD_PRIO_INHERIT)", pthread_mutexattr_setprotocol(a.p(), PTHREAD_PRIO_INHERIT)?)?;
        let mut proto = -1;
        rc("pthread_mutexattr_getprotocol", pthread_mutexattr_getprotocol(a.p(), &mut proto)?)?;
        check(proto == PTHREAD_PRIO_INHERIT, &format!("pthread_mutexattr_getprotocol gave {proto} after PTHREAD_PRIO_INHERIT was set"))?;
        let m = obj();
        rc("pthread_mutex_init", pthread_mutex_init(m.p(), a.p())?)?;
        let lo = fifo_min()?;
        // Three threads meet on processor 0: a low-priority owner, a high-priority waiter and
        // a middle-priority spinner. The case's own thread runs on processor 1, and each
        // thread starts there and takes its priority before it moves to processor 0. The
        // owner takes the mutex and sleeps holding it; the waiter blocks on it; the spinner
        // computes for 500 ms; then the case tells the owner to finish, which takes it 20 ms
        // of computing. Only inheritance lets the owner run past the spinner to do that.
        let cpus = processors()?;
        if cpus < 2 { return Err(CaseError::Skip(format!("{cpus} processor online; the case needs 2"))); }
        pin_to(1).map_err(CaseError::Fail)?;
        let owned = Arc::new(AtomicU32::new(0));
        let waiting = Arc::new(AtomicU32::new(0));
        let spinning = Arc::new(AtomicU32::new(0));
        let go_at = Arc::new(AtomicI64::new(0));
        let (o, g) = (owned.clone(), go_at.clone());
        let low = spawn("the low-priority owner", move || -> CaseResult {
            set_sched(SCHED_FIFO, lo + 1)?;
            pin_to(0).map_err(CaseError::Fail)?;
            lock(m)?;
            o.store(1, SeqCst);
            let t0 = now_ms();
            while g.load(SeqCst) == 0 && now_ms() - t0 < 3000 { nap(); }
            let told = g.load(SeqCst) != 0;
            let t = mono();
            while mono() - t < 20 * MS { core::hint::spin_loop(); }
            unlock(m)?;
            check(told, "the owner held the mutex for 3 s without being told to finish")
        })?;
        check(until(1000, || owned.load(SeqCst) == 1), "the low-priority thread did not take the mutex")?;
        let w = waiting.clone();
        let high = spawn("the high-priority waiter", move || -> Result<i64, CaseError> {
            set_sched(SCHED_FIFO, lo + 3)?;
            pin_to(0).map_err(CaseError::Fail)?;
            let r = pthread_mutex_trylock(m.p())?;
            if r == 0 {
                unlock(m)?;
                return err("the mutex was free when the high-priority thread came to take it, so it had nothing to wait for");
            }
            want("pthread_mutex_trylock while the low-priority thread holds the mutex", r, EBUSY)?;
            w.store(1, SeqCst);
            lock(m)?;
            let got = mono();
            unlock(m)?;
            Ok(got)
        })?;
        check(until(1000, || waiting.load(SeqCst) == 1 || high.finished()), "the high-priority thread did not come to the mutex within a second")?;
        sleep_ms(20);
        let sp = spinning.clone();
        let mid = spawn("the middle-priority spinner", move || -> CaseResult {
            set_sched(SCHED_FIFO, lo + 2)?;
            pin_to(0).map_err(CaseError::Fail)?;
            sp.store(1, SeqCst);
            let t0 = now_ms();
            while now_ms() - t0 < 500 { core::hint::spin_loop(); }
            Ok(())
        })?;
        check(until(1000, || spinning.load(SeqCst) == 1 || mid.finished()), "the middle-priority thread did not start computing on processor 0 within a second")?;
        go_at.store(mono(), SeqCst);
        let got = high.wait(4000)??;
        mid.wait(3000)??;
        low.wait(3000)??;
        let waited = got - go_at.load(SeqCst);
        value("waited", waited / 1000, "us", Some((0, 200_000)));
        check(waited <= 200 * MS, &format!("the high-priority thread got the mutex {} ms after its owner was told to finish, behind a 500 ms middle-priority spinner", waited / MS))
    })
}

/// A value computed in a trial (for a case that decides what to measure from it).
fn trial_value(f: impl FnOnce() -> Result<i64, CaseError>) -> Result<i64, CaseError> {
    let mut p = spawn_proc(|| {
        let v = f()?;
        if let Some(b) = board() { b.words[0].store(v, SeqCst); }
        Ok(())
    })?;
    p.result(3000)?;
    Ok(p.board.word(0))
}

fn futex(word: &AtomicU32, op: u64, val: u64, timeout: u64, word2: u64, val3: u64) -> i64 {
    sc(nr::FUTEX, &[word as *const AtomicU32 as u64, op, val, timeout, word2, val3])
}

fn mx_futex_eagain() -> CaseResult {
    trial_ms(3000, || {
        let word = AtomicU32::new(5);
        step("in FUTEX_WAIT for 4 on a word holding 5");
        let r = futex(&word, FUTEX_WAIT, 4, 0, 0, 0);
        check(r == -(EAGAIN as i64), &format!("FUTEX_WAIT on a word that does not hold the value returned {}, expected EAGAIN", shown(r)))
    })
}

/// Threads waiting on a futex word: how many have been woken, and the first unexpected
/// return any of them got from FUTEX_WAIT (0 if none).
struct FutexWaiters { back: Arc<AtomicU32>, bad: Arc<AtomicI64> }

impl FutexWaiters {
    /// How many waiters FUTEX_WAIT has returned 0 to; fails if any got an error other than
    /// EINTR or EAGAIN, which is not a wakeup.
    fn woken(&self) -> Result<u32, CaseError> {
        let bad = self.bad.load(SeqCst);
        if bad != 0 { return err(format!("a waiter's FUTEX_WAIT returned {}", shown(bad))); }
        Ok(self.back.load(SeqCst))
    }
    /// Wait up to `ms` for `n` waiters to have been woken.
    fn reach(&self, ms: u64, n: u32) -> Result<bool, CaseError> {
        let reached = until(ms, || self.back.load(SeqCst) >= n || self.bad.load(SeqCst) != 0);
        self.woken()?;
        Ok(reached)
    }
}

/// Start `n` threads that each FUTEX_WAIT on `word` while it holds 0, and wait until all
/// have said they are about to. A waiter counts as woken only when FUTEX_WAIT returned 0.
fn futex_waiters(word: &'static AtomicU32, n: usize, op: u64) -> Result<FutexWaiters, CaseError> {
    let ready = Arc::new(AtomicU32::new(0));
    let back = Arc::new(AtomicU32::new(0));
    let bad = Arc::new(AtomicI64::new(0));
    for i in 0..n {
        let (r, b, e) = (ready.clone(), back.clone(), bad.clone());
        spawn(&format!("futex waiter {i}"), move || {
            r.fetch_add(1, SeqCst);
            while word.load(SeqCst) == 0 {
                let ret = futex(word, op, 0, 0, 0, 0);
                if ret == 0 { b.fetch_add(1, SeqCst); return; }
                if ret != -(EINTR as i64) && ret != -(EAGAIN as i64) {
                    let _ = e.compare_exchange(0, ret, SeqCst, SeqCst);
                    return;
                }
            }
        })?;
    }
    check(until(1000, || ready.load(SeqCst) == n as u32), "the waiters did not start within a second")?;
    sleep_ms(100);
    Ok(FutexWaiters { back, bad })
}

fn mx_futex_wake_count() -> CaseResult {
    trial(|| {
        let word = leak(AtomicU32::new(0));
        let back = futex_waiters(word, 3, FUTEX_WAIT)?;
        check(back.woken()? == 0, "a FUTEX_WAIT returned before any wake")?;
        let r = futex(word, FUTEX_WAKE, 1, 0, 0, 0);
        check(r == 1, &format!("FUTEX_WAKE of 1 with three waiters returned {}", shown(r)))?;
        sleep_ms(100);
        let woke = back.woken()?;
        value("woken-by-one", woke as i64, "", Some((1, 1)));
        check(woke == 1, &format!("FUTEX_WAKE of 1 let {woke} of three waiters return"))?;
        let r = futex(word, FUTEX_WAKE, 10, 0, 0, 0);
        check(r == 2, &format!("FUTEX_WAKE of 10 with two waiters left returned {}", shown(r)))?;
        check(back.reach(1000, 3)?, "the last two waiters did not return within a second")
    })
}

fn mx_futex_timeout() -> CaseResult {
    trial_ms(3000, || {
        let word = AtomicU32::new(0);
        let rel = ts(100 * MS);
        wait_for(100, "late");
        let t0 = mono();
        step("in FUTEX_WAIT with a 100 ms timeout");
        let r = futex(&word, FUTEX_WAIT, 0, rel.as_ptr() as u64, 0, 0);
        let end = mono();
        check(r == -(ETIMEDOUT as i64), &format!("FUTEX_WAIT with a 100 ms timeout returned {}, expected ETIMEDOUT", shown(r)))?;
        on_time("FUTEX_WAIT", end, t0 + 100 * MS)
    })
}

fn mx_futex_private() -> CaseResult {
    trial(|| {
        let word = leak(AtomicU32::new(0));
        let back = futex_waiters(word, 1, FUTEX_WAIT | FUTEX_PRIVATE)?;
        let r = futex(word, FUTEX_WAKE | FUTEX_PRIVATE, 1, 0, 0, 0);
        check(r == 1, &format!("FUTEX_WAKE_PRIVATE with one FUTEX_WAIT_PRIVATE waiter returned {}", shown(r)))?;
        check(back.reach(1000, 1)?, "the FUTEX_WAIT_PRIVATE waiter did not return within a second")
    })
}

// ---------------------------------------------------------------------------
// cond

struct Shared { m: P, c: P, flag: AtomicU32, waiting: AtomicU32, woke_at: AtomicI64 }

fn shared(kind: Option<i32>, clock: Option<i32>) -> Result<Arc<Shared>, CaseError> {
    Ok(Arc::new(Shared {
        m: new_mutex(kind, false)?,
        c: new_cond(clock)?,
        flag: AtomicU32::new(0),
        waiting: AtomicU32::new(0),
        woke_at: AtomicI64::new(0),
    }))
}

/// A thread that waits on `s.c` until `s.flag` is set, records when it came back, and
/// returns what unlocking the mutex then returned (0 if it held it).
fn waiter(s: &Arc<Shared>) -> Result<Th<Result<i32, CaseError>>, CaseError> {
    let w = s.clone();
    let th = spawn("the waiting thread", move || -> Result<i32, CaseError> {
        lock(w.m)?;
        w.waiting.store(1, SeqCst);
        while w.flag.load(SeqCst) == 0 { cond_wait(w.c, w.m)?; }
        w.woke_at.store(mono(), SeqCst);
        pthread_mutex_unlock(w.m.p())
    })?;
    check(until(1000, || s.waiting.load(SeqCst) == 1), "the waiting thread did not start within a second")?;
    Ok(th)
}

fn cv_signal_wakes() -> CaseResult {
    trial(|| {
        let s = shared(Some(PTHREAD_MUTEX_ERRORCHECK), None)?;
        let w = waiter(&s)?;
        lock(s.m)?;
        s.flag.store(1, SeqCst);
        let sent = mono();
        signal(s.c)?;
        unlock(s.m)?;
        let r = w.wait(1000)??;
        rc("pthread_mutex_unlock by the woken thread", r)?;
        woken("pthread_cond_wait", s.woke_at.load(SeqCst), sent)
    })
}

fn cv_signal_one() -> CaseResult {
    trial(|| {
        let s = shared(None, None)?;
        let tokens = Arc::new(AtomicU32::new(0));
        let taken = Arc::new(AtomicU32::new(0));
        let ready = Arc::new(AtomicU32::new(0));
        for i in 0..3 {
            let (s, tokens, taken, ready) = (s.clone(), tokens.clone(), taken.clone(), ready.clone());
            spawn(&format!("waiter {i}"), move || -> CaseResult {
                lock(s.m)?;
                ready.fetch_add(1, SeqCst);
                while tokens.load(SeqCst) == 0 { cond_wait(s.c, s.m)?; }
                tokens.fetch_sub(1, SeqCst);
                taken.fetch_add(1, SeqCst);
                unlock(s.m)
            })?;
        }
        check(until(1000, || ready.load(SeqCst) == 3), "the three waiters did not start within a second")?;
        lock(s.m)?;
        tokens.store(1, SeqCst);
        signal(s.c)?;
        unlock(s.m)?;
        check(until(1000, || taken.load(SeqCst) == 1), "pthread_cond_signal with three waiters woke none within a second")?;
        lock(s.m)?;
        tokens.store(2, SeqCst);
        broadcast(s.c)?;
        unlock(s.m)?;
        let all = until(1000, || taken.load(SeqCst) == 3);
        value("woken", taken.load(SeqCst) as i64, "", Some((3, 3)));
        check(all, &format!("after a signal and a broadcast only {} of three waiters returned", taken.load(SeqCst)))
    })
}

fn cv_broadcast_all() -> CaseResult {
    trial(|| {
        let s = shared(None, None)?;
        let back = Arc::new(AtomicU32::new(0));
        let ready = Arc::new(AtomicU32::new(0));
        for i in 0..4 {
            let (s, back, ready) = (s.clone(), back.clone(), ready.clone());
            spawn(&format!("waiter {i}"), move || -> CaseResult {
                lock(s.m)?;
                ready.fetch_add(1, SeqCst);
                while s.flag.load(SeqCst) == 0 { cond_wait(s.c, s.m)?; }
                back.fetch_add(1, SeqCst);
                unlock(s.m)
            })?;
        }
        check(until(1000, || ready.load(SeqCst) == 4), "the four waiters did not start within a second")?;
        lock(s.m)?;
        s.flag.store(1, SeqCst);
        broadcast(s.c)?;
        unlock(s.m)?;
        let all = until(1000, || back.load(SeqCst) == 4);
        value("woken", back.load(SeqCst) as i64, "", Some((4, 4)));
        check(all, &format!("pthread_cond_broadcast woke {} of four waiters within a second", back.load(SeqCst)))
    })
}

fn cv_wait_releases() -> CaseResult {
    trial(|| {
        let s = shared(None, None)?;
        let w = waiter(&s)?;
        // The waiter says it is waiting just before pthread_cond_wait releases the mutex, so
        // trylock may find the mutex held for a moment; it must get it within a second.
        let mut last = EBUSY;
        let mut failed = None;
        until(1000, || match pthread_mutex_trylock(s.m.p()) {
            Ok(0) => { last = 0; true }
            Ok(EBUSY) => false,
            Ok(r) => { last = r; true }
            Err(e) => { failed = Some(e); true }
        });
        if let Some(e) = failed { return Err(e); }
        if last == EBUSY { return fail("pthread_mutex_trylock still returned EBUSY a second after the other thread began waiting on the condition, so pthread_cond_wait did not release the mutex"); }
        rc("pthread_mutex_trylock while the other thread waits on the condition", last)?;
        s.flag.store(1, SeqCst);
        signal(s.c)?;
        unlock(s.m)?;
        rc("pthread_mutex_unlock by the woken thread", w.wait(1000)??)
    })
}

fn cv_wait_reacquires() -> CaseResult {
    trial(|| {
        let s = shared(Some(PTHREAD_MUTEX_ERRORCHECK), None)?;
        let w = waiter(&s)?;
        lock(s.m)?;
        s.flag.store(1, SeqCst);
        signal(s.c)?;
        sleep_ms(100);
        let released = mono();
        unlock(s.m)?;
        rc("pthread_mutex_unlock by the woken thread", w.wait(1000)??)?;
        let woke = s.woke_at.load(SeqCst);
        check(woke >= released, &format!("pthread_cond_wait returned {} us before the signalling thread unlocked the mutex", (released - woke) / 1000))
    })
}

fn cv_wakes_waiting() -> CaseResult {
    trial(|| {
        let s = shared(None, None)?;
        let a = waiter(&s)?;
        let late = Arc::new(AtomicU32::new(0));
        lock(s.m)?;
        s.flag.store(1, SeqCst);
        signal(s.c)?;
        unlock(s.m)?;
        let (s2, l) = (s.clone(), late.clone());
        let b = spawn("the thread that starts waiting after the signal", move || -> CaseResult {
            lock(s2.m)?;
            while l.load(SeqCst) == 0 { cond_wait(s2.c, s2.m)?; }
            unlock(s2.m)
        })?;
        let r = a.wait(1000);
        lock(s.m)?;
        late.store(1, SeqCst);
        broadcast(s.c)?;
        unlock(s.m)?;
        rc("pthread_mutex_unlock by the thread that was waiting", r.map_err(|_| CaseError::Fail("pthread_cond_signal did not wake the thread waiting when it was sent".into()))??)?;
        b.wait(1000)?
    })
}

fn cv_timedwait(s: &Shared, when: &Ts) -> Result<i32, CaseError> {
    loop {
        let r = pthread_cond_timedwait(s.c.p(), s.m.p(), when.as_ptr())?;
        if r != 0 || s.flag.load(SeqCst) != 0 { return Ok(r); }
    }
}

fn cv_timedwait_realtime() -> CaseResult {
    trial_ms(3000, || {
        let s = shared(Some(PTHREAD_MUTEX_ERRORCHECK), None)?;
        lock(s.m)?;
        let when = deadline(CLOCK_REALTIME, 200);
        wait_for(200, "late");
        step("in pthread_cond_timedwait with a CLOCK_REALTIME deadline 200 ms ahead");
        let r = cv_timedwait(&s, &when)?;
        let end = rt();
        want("pthread_cond_timedwait with nothing signalled", r, ETIMEDOUT)?;
        on_time("pthread_cond_timedwait", end, ts_ns(&when))?;
        rc("pthread_mutex_unlock after the timed-out wait", pthread_mutex_unlock(s.m.p())?)
    })
}

fn cv_timedwait_monotonic() -> CaseResult {
    trial_ms(3000, || {
        let a = obj();
        rc("pthread_condattr_init", pthread_condattr_init(a.p())?)?;
        let mut clock = -1;
        rc("pthread_condattr_getclock", pthread_condattr_getclock(a.p(), &mut clock)?)?;
        check(clock == CLOCK_REALTIME, &format!("a new condition attribute's clock is {clock}, not CLOCK_REALTIME"))?;
        rc("pthread_condattr_setclock(CLOCK_MONOTONIC)", pthread_condattr_setclock(a.p(), CLOCK_MONOTONIC)?)?;
        rc("pthread_condattr_getclock", pthread_condattr_getclock(a.p(), &mut clock)?)?;
        check(clock == CLOCK_MONOTONIC, &format!("pthread_condattr_getclock gave {clock} after CLOCK_MONOTONIC was set"))?;
        let s = shared(Some(PTHREAD_MUTEX_ERRORCHECK), Some(CLOCK_MONOTONIC))?;
        lock(s.m)?;
        let when = deadline(CLOCK_MONOTONIC, 200);
        wait_for(200, "late");
        step("in pthread_cond_timedwait with a CLOCK_MONOTONIC deadline 200 ms ahead");
        let r = cv_timedwait(&s, &when)?;
        let end = mono();
        want("pthread_cond_timedwait on a CLOCK_MONOTONIC condition with nothing signalled", r, ETIMEDOUT)?;
        on_time("pthread_cond_timedwait", end, ts_ns(&when))?;
        rc("pthread_mutex_unlock after the timed-out wait", pthread_mutex_unlock(s.m.p())?)
    })
}

/// CLOCK_REALTIME as it was when the guard was made; set back, advanced by the
/// CLOCK_MONOTONIC time that has passed, when restored or dropped.
struct RealtimeGuard { real: i64, mono: i64, restored: bool }
impl RealtimeGuard {
    fn new() -> RealtimeGuard { RealtimeGuard { real: rt(), mono: mono(), restored: false } }
    fn now(&self) -> i64 { self.real + (mono() - self.mono) }
    fn restore(mut self) -> CaseResult {
        self.restored = true;
        let t = ts(self.now());
        sys0("clock_settime putting CLOCK_REALTIME back", sc(nr::CLOCK_SETTIME, &[CLOCK_REALTIME as u64, t.as_ptr() as u64]))
    }
}
impl Drop for RealtimeGuard {
    fn drop(&mut self) {
        if !self.restored {
            let t = ts(self.now());
            let _ = sc(nr::CLOCK_SETTIME, &[CLOCK_REALTIME as u64, t.as_ptr() as u64]);
        }
    }
}

/// A thread that, `after_ms` from now, sets CLOCK_REALTIME 10 s ahead; returns the
/// CLOCK_MONOTONIC time it did so.
fn set_clock_ahead(after_ms: u64) -> Result<Th<Result<i64, CaseError>>, CaseError> {
    spawn("the thread setting the clock", move || -> Result<i64, CaseError> {
        sleep_ms(after_ms);
        let t = ts(rt() + 10 * NS);
        let at = mono();
        sys0("clock_settime(CLOCK_REALTIME) 10 s ahead", sc(nr::CLOCK_SETTIME, &[CLOCK_REALTIME as u64, t.as_ptr() as u64]))?;
        Ok(at)
    })
}

fn cv_timedwait_realtime_set() -> CaseResult {
    let guard = RealtimeGuard::new();
    let result = trial_ms(4000, || {
        let s = shared(None, None)?;
        lock(s.m)?;
        let when = deadline(CLOCK_REALTIME, 2000);
        let setter = set_clock_ahead(100)?;
        step("in pthread_cond_timedwait with a CLOCK_REALTIME deadline 2 s ahead, the clock set 10 s ahead after 100 ms");
        let r = cv_timedwait(&s, &when)?;
        let end = mono();
        let set_at = setter.wait(1000)??;
        want("pthread_cond_timedwait once CLOCK_REALTIME was set past its deadline", r, ETIMEDOUT)?;
        on_time("pthread_cond_timedwait after the clock was set past its deadline", end, set_at)?;
        unlock(s.m)
    });
    let restored = guard.restore();
    result.and(restored)
}

fn cv_timedwait_monotonic_set() -> CaseResult {
    let guard = RealtimeGuard::new();
    let result = trial_ms(3000, || {
        let s = shared(None, Some(CLOCK_MONOTONIC))?;
        lock(s.m)?;
        let when = deadline(CLOCK_MONOTONIC, 300);
        let setter = set_clock_ahead(50)?;
        wait_for(300, "late");
        step("in pthread_cond_timedwait with a CLOCK_MONOTONIC deadline 300 ms ahead, CLOCK_REALTIME set 10 s ahead after 50 ms");
        let r = cv_timedwait(&s, &when)?;
        let end = mono();
        setter.wait(1000)??;
        want("pthread_cond_timedwait on a CLOCK_MONOTONIC condition", r, ETIMEDOUT)?;
        on_time("pthread_cond_timedwait on a CLOCK_MONOTONIC condition", end, ts_ns(&when))?;
        unlock(s.m)
    });
    let restored = guard.restore();
    result.and(restored)
}

fn cv_timedwait_signaled() -> CaseResult {
    trial_ms(4000, || {
        let s = shared(None, None)?;
        lock(s.m)?;
        let s2 = s.clone();
        let sent = Arc::new(AtomicI64::new(0));
        let sent2 = sent.clone();
        let signaller = spawn("the signalling thread", move || -> CaseResult {
            sleep_ms(100);
            lock(s2.m)?;
            s2.flag.store(1, SeqCst);
            sent2.store(mono(), SeqCst);
            signal(s2.c)?;
            unlock(s2.m)
        })?;
        let when = deadline(CLOCK_REALTIME, 3000);
        step("in pthread_cond_timedwait with a deadline 3 s ahead, signalled after 100 ms");
        let r = cv_timedwait(&s, &when)?;
        let got = mono();
        rc("pthread_cond_timedwait signalled before its deadline", r)?;
        check(s.flag.load(SeqCst) == 1, "pthread_cond_timedwait returned 0 before it was signalled")?;
        woken("pthread_cond_timedwait", got, sent.load(SeqCst))?;
        unlock(s.m)?;
        signaller.wait(1000)?
    })
}

fn cv_timedwait_past() -> CaseResult {
    trial_ms(3000, || {
        let s = shared(Some(PTHREAD_MUTEX_ERRORCHECK), None)?;
        lock(s.m)?;
        let when = deadline(CLOCK_REALTIME, -1000);
        let t0 = mono();
        step("in pthread_cond_timedwait with a deadline a second past");
        let r = cv_timedwait(&s, &when)?;
        let us = (mono() - t0) / 1000;
        want("pthread_cond_timedwait with a deadline already past", r, ETIMEDOUT)?;
        value("returned", us, "us", Some((0, LATE_MS * 1000)));
        check(us <= LATE_MS * 1000, &format!("pthread_cond_timedwait with a deadline already past took {us} us"))?;
        rc("pthread_mutex_unlock after the timed-out wait", pthread_mutex_unlock(s.m.p())?)
    })
}

fn cv_timedwait_einval() -> CaseResult {
    trial_ms(3000, || {
        let s = shared(None, None)?;
        lock(s.m)?;
        let bad: Ts = [rt() / NS + 1, NS];
        step("in pthread_cond_timedwait with tv_nsec of 1000000000");
        want("pthread_cond_timedwait with tv_nsec 1000000000", pthread_cond_timedwait(s.c.p(), s.m.p(), bad.as_ptr())?, EINVAL)
    })
}

fn cv_wait_eperm() -> CaseResult {
    trial_ms(3000, || {
        let s = shared(Some(PTHREAD_MUTEX_ERRORCHECK), None)?;
        step("in pthread_cond_wait with an error-checking mutex the thread does not hold");
        want("pthread_cond_wait with an error-checking mutex the caller does not hold", pthread_cond_wait(s.c.p(), s.m.p())?, EPERM)
    })
}

fn cv_pingpong() -> CaseResult {
    const PASSES: u64 = 2000;
    let n = team_size()?;
    trial(move || {
        let m = new_mutex(None, false)?;
        let conds: Vec<P> = (0..n).map(|_| new_cond(None)).collect::<Result<_, _>>()?;
        let turn = Arc::new(AtomicU64::new(0));
        let t0 = Arc::new(AtomicI64::new(0));
        let (tn, t0w) = (turn.clone(), t0.clone());
        let conds = Arc::new(conds);
        let ends = team(n, move |i| -> Result<i64, String> {
            let e = text;
            if i == 0 { t0w.store(mono(), SeqCst); }
            let mut k = i as u64;
            while k < PASSES {
                lock(m).map_err(e)?;
                let start = now_ms();
                while tn.load(SeqCst) != k {
                    if now_ms() - start > 4000 { let _ = unlock(m); return Err(format!("the ring stalled at pass {} of {PASSES}: a wakeup was lost", tn.load(SeqCst))); }
                    cond_wait(conds[i], m).map_err(e)?;
                }
                tn.store(k + 1, SeqCst);
                signal(conds[(i + 1) % n]).map_err(e)?;
                unlock(m).map_err(e)?;
                k += n as u64;
            }
            Ok(mono())
        })?;
        let total = turn.load(SeqCst);
        let end = ends.into_iter().max().unwrap_or(0);
        value("passes", total as i64, "", Some((PASSES as i64, PASSES as i64)));
        value("pass-time", (end - t0.load(SeqCst)) / 1000 / PASSES as i64, "us", None);
        check(total == PASSES, &format!("the token went round {total} of {PASSES} times"))
    })
}

fn cv_queue() -> CaseResult {
    const ITEMS: u64 = 4000;
    const CAP: u64 = 8;
    let n = team_size()?;
    trial(move || {
        let m = new_mutex(None, false)?;
        let not_full = new_cond(None)?;
        let not_empty = new_cond(None)?;
        let queue = Arc::new((Racy::new(), Racy::new(), [const { AtomicU64::new(0) }; 8], AtomicU32::new(0)));
        // How many times each item was taken, indexed by item.
        let taken: Arc<Vec<AtomicU32>> = Arc::new((0..=ITEMS).map(|_| AtomicU32::new(0)).collect());
        let q = queue.clone();
        let tk = taken.clone();
        let got = team(n, move |i| -> Result<(u64, u64), String> {
            let e = text;
            let (head, tail, slots, done) = (&q.0, &q.1, &q.2, &q.3);
            let start = now_ms();
            if i == 0 {
                for item in 1..=ITEMS {
                    lock(m).map_err(e)?;
                    while tail.get() - head.get() == CAP {
                        if now_ms() - start > 6000 { let _ = unlock(m); return Err(format!("the producer stalled at item {item}: a wakeup was lost")); }
                        cond_wait(not_full, m).map_err(e)?;
                    }
                    slots[(tail.get() % CAP) as usize].store(item, SeqCst);
                    tail.set(tail.get() + 1);
                    signal(not_empty).map_err(e)?;
                    unlock(m).map_err(e)?;
                }
                lock(m).map_err(e)?;
                done.store(1, SeqCst);
                broadcast(not_empty).map_err(e)?;
                unlock(m).map_err(e)?;
                return Ok((0, 0));
            }
            let (mut count, mut sum) = (0u64, 0u64);
            loop {
                lock(m).map_err(e)?;
                while tail.get() == head.get() && done.load(SeqCst) == 0 {
                    if now_ms() - start > 6000 { let _ = unlock(m); return Err(format!("consumer {i} stalled after {count} items: a wakeup was lost")); }
                    cond_wait(not_empty, m).map_err(e)?;
                }
                if tail.get() == head.get() { unlock(m).map_err(e)?; break; }
                let item = slots[(head.get() % CAP) as usize].load(SeqCst);
                head.set(head.get() + 1);
                signal(not_full).map_err(e)?;
                unlock(m).map_err(e)?;
                match tk.get(item as usize) {
                    Some(t) if item != 0 => { t.fetch_add(1, SeqCst); }
                    _ => return Err(format!("consumer {i} took item {item}, which the producer never queued")),
                }
                count += 1;
                sum += item;
            }
            Ok((count, sum))
        })?;
        let count: u64 = got.iter().map(|g| g.0).sum();
        let sum: u64 = got.iter().map(|g| g.1).sum();
        value("items", count as i64, "", Some((ITEMS as i64, ITEMS as i64)));
        check(count == ITEMS, &format!("the consumers took {count} of {ITEMS} items"))?;
        let missed = (1..=ITEMS).filter(|&k| taken[k as usize].load(SeqCst) == 0).count();
        let twice = (1..=ITEMS).filter(|&k| taken[k as usize].load(SeqCst) > 1).count();
        check(missed == 0 && twice == 0, &format!("the consumers missed {missed} items and took {twice} more than once"))?;
        check(sum == ITEMS * (ITEMS + 1) / 2, &format!("the items the consumers took add up to {sum}, not {}", ITEMS * (ITEMS + 1) / 2))
    })
}

fn cv_broadcast_cycles() -> CaseResult {
    const CYCLES: i64 = 200;
    const WAITERS: i64 = 4;
    trial(|| {
        let m = new_mutex(None, false)?;
        let go = new_cond(None)?;
        let ack = new_cond(None)?;
        let gen = Arc::new(AtomicI64::new(0));
        let acks = Arc::new(AtomicI64::new(0));
        let mut threads = Vec::new();
        for i in 0..WAITERS {
            let (gen, acks) = (gen.clone(), acks.clone());
            threads.push(spawn(&format!("waiter {i}"), move || -> Result<i64, CaseError> {
                let mut seen = 0;
                let mut missed = 0;
                while seen < CYCLES {
                    lock(m)?;
                    while gen.load(SeqCst) == seen { cond_wait(go, m)?; }
                    let g = gen.load(SeqCst);
                    if g != seen + 1 { missed += g - seen - 1; }
                    seen = g;
                    acks.fetch_add(1, SeqCst);
                    signal(ack)?;
                    unlock(m)?;
                }
                Ok(missed)
            })?);
        }
        for g in 1..=CYCLES {
            step(&format!("waiting for all four waiters to see generation {}", g - 1));
            lock(m)?;
            while acks.load(SeqCst) < WAITERS * (g - 1) { cond_wait(ack, m)?; }
            gen.store(g, SeqCst);
            broadcast(go)?;
            unlock(m)?;
        }
        let mut missed = 0;
        for th in threads { missed += th.wait(3000)??; }
        value("cycles", gen.load(SeqCst), "", Some((CYCLES, CYCLES)));
        value("missed", missed, "", Some((0, 0)));
        check(missed == 0, &format!("the waiters missed {missed} of {} broadcasts", CYCLES * WAITERS))
    })
}

fn cv_static_init() -> CaseResult {
    trial(|| {
        let s = Arc::new(Shared { m: obj(), c: obj(), flag: AtomicU32::new(0), waiting: AtomicU32::new(0), woke_at: AtomicI64::new(0) });
        let w = waiter(&s)?;
        lock(s.m)?;
        s.flag.store(1, SeqCst);
        signal(s.c)?;
        unlock(s.m)?;
        rc("pthread_mutex_unlock by the woken thread", w.wait(1000)??)
    })
}

fn cv_futex_bitset() -> CaseResult {
    trial_ms(3000, || {
        let word = AtomicU32::new(0);
        let when = deadline(CLOCK_REALTIME, 150);
        wait_for(150, "late");
        step("in FUTEX_WAIT_BITSET with a CLOCK_REALTIME deadline 150 ms ahead");
        let r = futex(&word, FUTEX_WAIT_BITSET | FUTEX_PRIVATE | FUTEX_CLOCK_REALTIME, 0, when.as_ptr() as u64, 0, u32::MAX as u64);
        let end = rt();
        check(r == -(ETIMEDOUT as i64), &format!("FUTEX_WAIT_BITSET with FUTEX_CLOCK_REALTIME returned {}, expected ETIMEDOUT", shown(r)))?;
        on_time("FUTEX_WAIT_BITSET", end, ts_ns(&when))
    })
}

fn cv_futex_requeue() -> CaseResult {
    trial(|| {
        let word = leak(AtomicU32::new(0));
        let target = leak(AtomicU32::new(0));
        let back = futex_waiters(word, 3, FUTEX_WAIT | FUTEX_PRIVATE)?;
        // FUTEX_CMP_REQUEUE: wake 1, move up to i32::MAX (passed in the timeout slot) to
        // the second word, if the first still holds 0.
        let r = futex(word, FUTEX_CMP_REQUEUE | FUTEX_PRIVATE, 1, i32::MAX as u64, target as *const AtomicU32 as u64, 0);
        check(r == 3, &format!("FUTEX_CMP_REQUEUE of three waiters (wake 1, move the rest) returned {}, expected 3", shown(r)))?;
        sleep_ms(100);
        let woke = back.woken()?;
        check(woke == 1, &format!("FUTEX_CMP_REQUEUE woke {woke} waiters, expected 1"))?;
        word.store(1, SeqCst);
        let r = futex(target, FUTEX_WAKE | FUTEX_PRIVATE, 10, 0, 0, 0);
        check(r == 2, &format!("FUTEX_WAKE on the second word returned {}, expected the 2 moved waiters", shown(r)))?;
        check(back.reach(1000, 3)?, "the moved waiters did not return within a second")
    })
}

// ---------------------------------------------------------------------------
// rwlock-barrier

fn rw_readers_share() -> CaseResult {
    trial(|| {
        let rw = new_rwlock()?;
        let holding = Arc::new(AtomicU32::new(0));
        let mut threads = Vec::new();
        for i in 0..3 {
            let h = holding.clone();
            threads.push(spawn(&format!("reader {i}"), move || -> Result<bool, CaseError> {
                rc("pthread_rwlock_rdlock", pthread_rwlock_rdlock(rw.p())?)?;
                h.fetch_add(1, SeqCst);
                let all = until(1000, || h.load(SeqCst) == 3);
                rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)?;
                Ok(all)
            })?);
        }
        let mut all = true;
        for th in threads { all &= th.wait(3000)??; }
        value("readers", holding.load(SeqCst) as i64, "", Some((3, 3)));
        check(all, "three readers could not all hold the read lock at once")
    })
}

fn rw_lock_holder(rw: P, write: bool, ms: u64) -> Result<Holder, CaseError> {
    if write {
        holder("pthread_rwlock_wrlock", ms, move || pthread_rwlock_wrlock(rw.p()), move || pthread_rwlock_unlock(rw.p()))
    } else {
        holder("pthread_rwlock_rdlock", ms, move || pthread_rwlock_rdlock(rw.p()), move || pthread_rwlock_unlock(rw.p()))
    }
}

/// While another thread holds `rw` (for writing or not) for 200 ms, the try form of the
/// other lock fails with EBUSY and the blocking form returns only after it is released.
fn rw_excludes(held_write: bool, want_write: bool) -> CaseResult {
    trial(|| {
        let rw = new_rwlock()?;
        let h = rw_lock_holder(rw, held_write, 200)?;
        let held = if held_write { "a write lock" } else { "a read lock" };
        let (try_name, r) = if want_write {
            ("pthread_rwlock_trywrlock", pthread_rwlock_trywrlock(rw.p())?)
        } else {
            ("pthread_rwlock_tryrdlock", pthread_rwlock_tryrdlock(rw.p())?)
        };
        want(&format!("{try_name} while another thread holds {held}"), r, EBUSY)?;
        step(&format!("blocking for the lock while another thread holds {held} for 200 ms"));
        let name = if want_write { "pthread_rwlock_wrlock" } else { "pthread_rwlock_rdlock" };
        let r = if want_write { pthread_rwlock_wrlock(rw.p())? } else { pthread_rwlock_rdlock(rw.p())? };
        let got = mono();
        rc(name, r)?;
        let released = h.unlocked_at();
        check(released != 0, &format!("{name} returned while another thread still held {held}"))?;
        woken(name, got, released)?;
        rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)?;
        h.finish()
    })
}

fn rw_writer_excludes_readers() -> CaseResult { rw_excludes(true, false) }
fn rw_writer_excludes_writers() -> CaseResult { rw_excludes(true, true) }
fn rw_reader_blocks_writer() -> CaseResult { rw_excludes(false, true) }

fn rw_read_recursive() -> CaseResult {
    trial(|| {
        let rw = new_rwlock()?;
        rc("pthread_rwlock_rdlock", pthread_rwlock_rdlock(rw.p())?)?;
        rc("a second pthread_rwlock_rdlock by the same thread", pthread_rwlock_rdlock(rw.p())?)?;
        want("trywrlock from another thread while two read locks are held", trywrlock_elsewhere(rw)?, EBUSY)?;
        rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)?;
        want("trywrlock from another thread while one read lock is still held", trywrlock_elsewhere(rw)?, EBUSY)?;
        rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)?;
        rc("trywrlock from another thread once both read locks were released", trywrlock_elsewhere(rw)?)
    })
}

fn rw_try_free() -> CaseResult {
    trial_ms(3000, || {
        let rw = new_rwlock()?;
        rc("pthread_rwlock_tryrdlock of a free lock", pthread_rwlock_tryrdlock(rw.p())?)?;
        rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)?;
        rc("pthread_rwlock_trywrlock of a free lock", pthread_rwlock_trywrlock(rw.p())?)?;
        rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)
    })
}

fn rw_writer_preference() -> CaseResult {
    trial(|| {
        let rw = new_rwlock()?;
        let prio = fifo_min()? + 1;
        let release = Arc::new(AtomicU32::new(0));
        let r1 = release.clone();
        let held = Arc::new(AtomicU32::new(0));
        let h1 = held.clone();
        let reader = spawn("the reader holding the lock", move || -> CaseResult {
            set_sched(SCHED_FIFO, prio)?;
            rc("pthread_rwlock_rdlock", pthread_rwlock_rdlock(rw.p())?)?;
            h1.store(1, SeqCst);
            check(until(3000, || r1.load(SeqCst) != 0), "never released")?;
            rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)
        })?;
        check(until(1000, || held.load(SeqCst) == 1 || reader.finished()), "the first reader did not take the lock within a second")?;
        if reader.finished() { return reader.wait(10)?; }
        let waiting = Arc::new(AtomicU32::new(0));
        let w1 = waiting.clone();
        let writer = spawn("the waiting writer", move || -> CaseResult {
            set_sched(SCHED_FIFO, prio)?;
            w1.store(1, SeqCst);
            rc("pthread_rwlock_wrlock", pthread_rwlock_wrlock(rw.p())?)?;
            rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)
        })?;
        check(until(1000, || waiting.load(SeqCst) == 1 || writer.finished()), "the writer did not start within a second")?;
        sleep_ms(100);
        if writer.finished() { writer.wait(10)??; return fail("the writer took the write lock while a reader held it"); }
        let r = spawn("the second reader", move || -> Result<i32, CaseError> {
            set_sched(SCHED_FIFO, prio)?;
            let r = pthread_rwlock_tryrdlock(rw.p())?;
            if r == 0 { rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p())?)?; }
            Ok(r)
        })?
        .wait(2000)??;
        release.store(1, SeqCst);
        reader.wait(3000)??;
        writer.wait(3000)??;
        want("pthread_rwlock_tryrdlock by a SCHED_FIFO reader while a SCHED_FIFO writer of equal priority waits", r, EBUSY)
    })
}

fn rw_timed(held_write: bool) -> CaseResult {
    trial_ms(3000, || {
        let rw = new_rwlock()?;
        let h = rw_lock_holder(rw, held_write, 3000)?;
        let when = deadline(CLOCK_REALTIME, 200);
        let (name, held) = if held_write { ("pthread_rwlock_timedrdlock", "a write lock") } else { ("pthread_rwlock_timedwrlock", "a read lock") };
        wait_for(200, "late");
        step(&format!("in {name} with a deadline 200 ms ahead while another thread holds {held}"));
        let r = if held_write { pthread_rwlock_timedrdlock(rw.p(), when.as_ptr())? } else { pthread_rwlock_timedwrlock(rw.p(), when.as_ptr())? };
        let end = rt();
        want(&format!("{name} while another thread holds {held}"), r, ETIMEDOUT)?;
        on_time(name, end, ts_ns(&when))?;
        h.finish()
    })
}

fn rw_timedwrlock() -> CaseResult { rw_timed(false) }
fn rw_timedrdlock() -> CaseResult { rw_timed(true) }

fn rw_contention() -> CaseResult {
    const OPS: u64 = 4000;
    let n = team_size()?;
    trial(move || {
        let rw = new_rwlock()?;
        let pair = Arc::new((Racy::new(), Racy::new()));
        let p = pair.clone();
        let got = team(n, move |i| -> Result<(u64, u64), String> {
            let e = text;
            let (a, b) = (&p.0, &p.1);
            let (mut writes, mut torn) = (0u64, 0u64);
            for k in 0..OPS {
                if (k + i as u64) % 8 == 0 {
                    rc("pthread_rwlock_wrlock", pthread_rwlock_wrlock(rw.p()).map_err(e)?).map_err(e)?;
                    a.set(a.get() + 1);
                    for _ in 0..32 { core::hint::spin_loop(); }
                    b.set(b.get() + 1);
                    writes += 1;
                } else {
                    rc("pthread_rwlock_rdlock", pthread_rwlock_rdlock(rw.p()).map_err(e)?).map_err(e)?;
                    if a.get() != b.get() { torn += 1; }
                }
                rc("pthread_rwlock_unlock", pthread_rwlock_unlock(rw.p()).map_err(e)?).map_err(e)?;
            }
            Ok((writes, torn))
        })?;
        let writes: u64 = got.iter().map(|g| g.0).sum();
        let torn: u64 = got.iter().map(|g| g.1).sum();
        let (a, b) = (pair.0.get(), pair.1.get());
        value("writes", writes as i64, "", None);
        value("torn-reads", torn as i64, "", Some((0, 0)));
        check(torn == 0, &format!("readers saw a writer's update half done {torn} times"))?;
        check(a == writes && b == writes, &format!("{writes} writes under the write lock left the counters at {a} and {b}"))
    })
}

fn new_barrier(count: u32) -> Result<P, CaseError> {
    let b = obj();
    rc(&format!("pthread_barrier_init({count})"), pthread_barrier_init(b.p(), null(), count)?)?;
    Ok(b)
}

fn br_serial_once() -> CaseResult {
    const CYCLES: usize = 100;
    trial(|| {
        let b = new_barrier(4)?;
        let serial: Arc<Vec<AtomicU32>> = Arc::new((0..CYCLES).map(|_| AtomicU32::new(0)).collect());
        let mut threads = Vec::new();
        for i in 0..4 {
            let s = serial.clone();
            threads.push(spawn(&format!("thread {i}"), move || -> CaseResult {
                for k in 0..CYCLES {
                    match pthread_barrier_wait(b.p())? {
                        PTHREAD_BARRIER_SERIAL_THREAD => { s[k].fetch_add(1, SeqCst); }
                        0 => {}
                        r => return fail(format!("pthread_barrier_wait returned {}", rc_text(r))),
                    }
                }
                Ok(())
            })?);
        }
        for th in threads { th.wait(5000)??; }
        let wrong: Vec<usize> = (0..CYCLES).filter(|&k| serial[k].load(SeqCst) != 1).collect();
        value("cycles", (CYCLES - wrong.len()) as i64, "", Some((CYCLES as i64, CYCLES as i64)));
        match wrong.first() {
            None => Ok(()),
            Some(&k) => fail(format!("{} of {CYCLES} cycles did not have exactly one PTHREAD_BARRIER_SERIAL_THREAD; cycle {k} had {}", wrong.len(), serial[k].load(SeqCst))),
        }
    })
}

fn br_holds() -> CaseResult {
    trial(|| {
        let b = new_barrier(4)?;
        let back = Arc::new(AtomicU32::new(0));
        let mut threads = Vec::new();
        for i in 0..3 {
            let back = back.clone();
            threads.push(spawn(&format!("thread {i}"), move || -> Result<i32, CaseError> {
                let r = pthread_barrier_wait(b.p())?;
                back.fetch_add(1, SeqCst);
                Ok(r)
            })?);
        }
        sleep_ms(200);
        check(back.load(SeqCst) == 0, &format!("{} of three threads passed a barrier of four before the fourth arrived", back.load(SeqCst)))?;
        step("in pthread_barrier_wait as the fourth thread");
        let mut returns = vec![pthread_barrier_wait(b.p())?];
        check(until(1000, || back.load(SeqCst) == 3), "the three waiting threads did not pass within a second of the fourth arriving")?;
        for th in threads { returns.push(th.wait(1000)??); }
        if let Some(&r) = returns.iter().find(|&&r| r != 0 && r != PTHREAD_BARRIER_SERIAL_THREAD) {
            return fail(format!("pthread_barrier_wait returned {}, neither 0 nor PTHREAD_BARRIER_SERIAL_THREAD", rc_text(r)));
        }
        let serials = returns.iter().filter(|&&r| r == PTHREAD_BARRIER_SERIAL_THREAD).count();
        check(serials == 1, &format!("{serials} of four threads got PTHREAD_BARRIER_SERIAL_THREAD"))
    })
}

fn br_cpus() -> CaseResult {
    const CYCLES: u64 = 2000;
    let n = team_size()?;
    trial(move || {
        let b = new_barrier(n as u32)?;
        let passed: Arc<Vec<AtomicU64>> = Arc::new((0..n).map(|_| AtomicU64::new(0)).collect());
        let p = passed.clone();
        let laps = team(n, move |i| -> Result<u64, String> {
            let mut laps = 0;
            for k in 0..CYCLES {
                let r = pthread_barrier_wait(b.p()).map_err(text)?;
                if r != 0 && r != PTHREAD_BARRIER_SERIAL_THREAD { return Err(format!("pthread_barrier_wait returned {}", rc_text(r))); }
                p[i].store(k + 1, SeqCst);
                for j in 0..n {
                    let v = p[j].load(SeqCst);
                    if v < k || v > k + 1 { laps += 1; }
                }
            }
            Ok(laps)
        })?;
        let laps: u64 = laps.iter().sum();
        value("cycles", CYCLES as i64, "", None);
        value("laps", laps as i64, "", Some((0, 0)));
        check(laps == 0, &format!("threads were seen a cycle ahead or behind the barrier {laps} times"))
    })
}

fn br_init_einval() -> CaseResult {
    trial_ms(3000, || {
        let b = obj();
        want("pthread_barrier_init with count 0", pthread_barrier_init(b.p(), null(), 0)?, EINVAL)
    })
}

fn br_one() -> CaseResult {
    trial_ms(3000, || {
        let b = new_barrier(1)?;
        for k in 1..=3 {
            want(&format!("pthread_barrier_wait number {k} on a barrier of one"), pthread_barrier_wait(b.p())?, PTHREAD_BARRIER_SERIAL_THREAD)?;
        }
        Ok(())
    })
}

static ONCE_RUNS: AtomicU32 = AtomicU32::new(0);
static ONCE_DONE: AtomicU32 = AtomicU32::new(0);
static ONCE_OTHER: AtomicU32 = AtomicU32::new(0);
extern "C" fn once_init() {
    ONCE_RUNS.fetch_add(1, SeqCst);
    sleep_ms(100);
    ONCE_DONE.store(1, SeqCst);
}
extern "C" fn once_other() { ONCE_OTHER.fetch_add(1, SeqCst); }

fn on_once_race() -> CaseResult {
    const N: usize = 8;
    trial(|| {
        let control: &'static AtomicI32 = leak(AtomicI32::new(0));
        let go = Arc::new(AtomicU32::new(0));
        let mut threads = Vec::new();
        for i in 0..N {
            let go = go.clone();
            threads.push(spawn(&format!("thread {i}"), move || -> Result<bool, CaseError> {
                while go.load(SeqCst) == 0 { core::hint::spin_loop(); }
                rc("pthread_once", pthread_once(control as *const AtomicI32 as *mut i32, once_init)?)?;
                Ok(ONCE_DONE.load(SeqCst) == 1)
            })?);
        }
        sleep_ms(20);
        go.store(1, SeqCst);
        let mut early = 0;
        for th in threads { if !th.wait(3000)?? { early += 1; } }
        let runs = ONCE_RUNS.load(SeqCst);
        value("runs", runs as i64, "", Some((1, 1)));
        check(runs == 1, &format!("{N} threads calling pthread_once at once ran the routine {runs} times"))?;
        check(early == 0, &format!("{early} of {N} pthread_once calls returned before the routine had finished"))
    })
}

fn on_once_again() -> CaseResult {
    trial_ms(3000, || {
        let mut control = 0i32;
        rc("pthread_once", pthread_once(&mut control, once_other)?)?;
        rc("pthread_once again on the same control", pthread_once(&mut control, once_other)?)?;
        rc("pthread_once a third time, with another routine", pthread_once(&mut control, once_init)?)?;
        check(ONCE_OTHER.load(SeqCst) == 1 && ONCE_RUNS.load(SeqCst) == 0, &format!(
            "three pthread_once calls on one control ran the first routine {} times and the second {} times",
            ONCE_OTHER.load(SeqCst), ONCE_RUNS.load(SeqCst)
        ))
    })
}

// ---------------------------------------------------------------------------
// tls

fn new_key(destructor: Destructor) -> Result<u32, CaseError> {
    let mut key = u32::MAX;
    rc("pthread_key_create", pthread_key_create(&mut key, destructor)?)?;
    Ok(key)
}

fn get(key: u32) -> Result<usize, CaseError> { Ok(pthread_getspecific(key)? as usize) }
fn set(key: u32, v: usize) -> CaseResult { rc("pthread_setspecific", pthread_setspecific(key, v as *const u8)?) }

fn tls_key_distinct() -> CaseResult {
    trial_ms(3000, || {
        let a = new_key(None)?;
        let b = new_key(None)?;
        check(a != b, &format!("two pthread_key_create calls gave the same key {a}"))?;
        check(get(a)? == 0 && get(b)? == 0, "a new key's value is not NULL")?;
        set(a, 0xa)?;
        let (va, vb) = (get(a)?, get(b)?);
        check(va == 0xa && vb == 0, &format!("after the first key was set to 0xa the keys read {va:#x} and {vb:#x}"))
    })
}

fn tls_set_get() -> CaseResult {
    trial_ms(3000, || {
        let k = new_key(None)?;
        set(k, 0x1234)?;
        let v = get(k)?;
        check(v == 0x1234, &format!("pthread_getspecific returned {v:#x} after 0x1234 was set"))
    })
}

fn tls_per_thread() -> CaseResult {
    trial(|| {
        let k = new_key(None)?;
        set(k, 0x1111)?;
        let first = spawn("the first thread", move || -> Result<(usize, usize), CaseError> {
            let before = get(k)?;
            set(k, 0x2222)?;
            Ok((before, get(k)?))
        })?
        .wait(2000)??;
        check(first.0 == 0, &format!("a new thread's value for a key the main thread set is {:#x}, not NULL", first.0))?;
        check(first.1 == 0x2222, &format!("pthread_getspecific in a thread returned {:#x} after it set 0x2222", first.1))?;
        let second = spawn("the second thread", move || get(k))?.wait(2000)??;
        check(second == 0, &format!("a second thread sees {second:#x} for a key another thread set"))?;
        let main = get(k)?;
        check(main == 0x1111, &format!("the main thread's value changed to {main:#x} after a thread set its own"))
    })
}

static DTOR_CALLS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
static DTOR_VALUE: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
static DTOR_TID: AtomicI64 = AtomicI64::new(0);
static DTOR_SEES: AtomicUsize = AtomicUsize::new(1);
static KEYS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
/// How many calls `dtor0_again` makes before it stops setting its value again.
static DTOR_AGAIN_LIMIT: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn dtor0(v: *mut u8) {
    DTOR_VALUE[0].store(v as usize, SeqCst);
    DTOR_TID.store(gettid(), SeqCst);
    DTOR_SEES.store(pthread_getspecific(KEYS[0].load(SeqCst)).map(|p| p as usize).unwrap_or(1), SeqCst);
    DTOR_CALLS[0].fetch_add(1, SeqCst);
}
unsafe extern "C" fn dtor1(v: *mut u8) {
    DTOR_VALUE[1].store(v as usize, SeqCst);
    DTOR_CALLS[1].fetch_add(1, SeqCst);
}
/// The destructor of all three keys of `destructor-rounds`, told apart by the value: the
/// middle key's value 0xa1 makes it give the lowest and highest keys values 0xb0 and 0xb2,
/// and those values count calls for the lowest (0) and highest (2) keys.
unsafe extern "C" fn dtor_rounds(v: *mut u8) {
    match v as usize {
        0xa1 => {
            let _ = pthread_setspecific(KEYS[0].load(SeqCst), 0xb0usize as *const u8);
            let _ = pthread_setspecific(KEYS[2].load(SeqCst), 0xb2usize as *const u8);
            DTOR_CALLS[1].fetch_add(1, SeqCst);
        }
        0xb0 => { DTOR_CALLS[0].fetch_add(1, SeqCst); }
        0xb2 => { DTOR_CALLS[2].fetch_add(1, SeqCst); }
        _ => {}
    }
}
/// Gives its own key a value again until it has been called DTOR_AGAIN_LIMIT times.
unsafe extern "C" fn dtor0_again(_v: *mut u8) {
    let n = DTOR_CALLS[0].fetch_add(1, SeqCst) + 1;
    if n < DTOR_AGAIN_LIMIT.load(SeqCst) { let _ = pthread_setspecific(KEYS[0].load(SeqCst), (n as usize + 1) as *const u8); }
}

/// Wait for destructor `i` to have been called `n` times, then a further 100 ms in case it
/// is called again.
fn dtor_settled(i: usize, n: u32) -> Result<u32, CaseError> {
    let reached = until(1000, || DTOR_CALLS[i].load(SeqCst) >= n);
    sleep_ms(100);
    let calls = DTOR_CALLS[i].load(SeqCst);
    if !reached { return err(format!("the key's destructor was called {calls} times within a second of the thread ending, expected {n}")); }
    Ok(calls)
}

fn tls_destructor_runs() -> CaseResult {
    trial(|| {
        let k = new_key(Some(dtor0))?;
        KEYS[0].store(k, SeqCst);
        let tid = spawn("the thread", move || -> Result<i64, CaseError> { set(k, 0xbeef)?; Ok(gettid()) })?.wait(2000)??;
        let calls = dtor_settled(0, 1)?;
        check(calls == 1, &format!("the destructor was called {calls} times for one value"))?;
        let v = DTOR_VALUE[0].load(SeqCst);
        check(v == 0xbeef, &format!("the destructor was passed {v:#x}, not the thread's 0xbeef"))?;
        let ran = DTOR_TID.load(SeqCst);
        check(ran == tid, &format!("the destructor ran in thread {ran}, not the exiting thread {tid}"))
    })
}

fn tls_destructor_null() -> CaseResult {
    trial(|| {
        let a = new_key(Some(dtor0))?;
        let b = new_key(Some(dtor1))?;
        KEYS[0].store(a, SeqCst);
        spawn("the thread", move || -> CaseResult { set(a, 0x1)?; set(a, 0)?; set(b, 0x2) })?.wait(2000)??;
        dtor_settled(1, 1)?;
        let calls = DTOR_CALLS[0].load(SeqCst);
        check(calls == 0, &format!("a destructor was called {calls} times for a key whose value was NULL"))
    })
}

fn tls_destructor_cleared() -> CaseResult {
    trial(|| {
        let k = new_key(Some(dtor0))?;
        KEYS[0].store(k, SeqCst);
        spawn("the thread", move || set(k, 0x77))?.wait(2000)??;
        dtor_settled(0, 1)?;
        let seen = DTOR_SEES.load(SeqCst);
        check(seen == 0, &format!("pthread_getspecific inside the destructor returned {seen:#x}, not NULL"))
    })
}

fn tls_destructor_rounds() -> CaseResult {
    // Three keys; the middle one's destructor gives the lowest and the highest values. A
    // round visiting keys upwards or downwards reaches one of those before the middle key,
    // so its destructor can only run in a later round.
    trial(|| {
        let mut keys = [new_key(Some(dtor_rounds))?, new_key(Some(dtor_rounds))?, new_key(Some(dtor_rounds))?];
        keys.sort_unstable();
        for (slot, &k) in KEYS.iter().zip(&keys) { slot.store(k, SeqCst); }
        let mid = keys[1];
        spawn("the thread", move || set(mid, 0xa1))?.wait(2000)??;
        dtor_settled(1, 1)?;
        let reached = until(1000, || DTOR_CALLS[0].load(SeqCst) >= 1 && DTOR_CALLS[2].load(SeqCst) >= 1);
        sleep_ms(100);
        let (low, high) = (DTOR_CALLS[0].load(SeqCst), DTOR_CALLS[2].load(SeqCst));
        check(reached && low == 1 && high == 1, &format!(
            "values the middle key's destructor set for the lowest and highest keys were destroyed {low} and {high} times, not once each"
        ))
    })
}

fn tls_destructor_iterations() -> CaseResult {
    trial(|| {
        let reported = sysconf(SC_THREAD_DESTRUCTOR_ITERATIONS)?;
        let rounds = reported.max(POSIX_DESTRUCTOR_ITERATIONS);
        let Ok(rounds) = u32::try_from(rounds) else { return fail(format!("sysconf(_SC_THREAD_DESTRUCTOR_ITERATIONS) returned {reported}")) };
        DTOR_AGAIN_LIMIT.store(rounds, SeqCst);
        let k = new_key(Some(dtor0_again))?;
        KEYS[0].store(k, SeqCst);
        spawn("the thread", move || set(k, 1))?.wait(2000)??;
        let calls = dtor_settled(0, rounds)?;
        value("calls", calls as i64, "", Some((rounds as i64, rounds as i64)));
        check(calls >= rounds, &format!("a destructor that kept setting its value was called {calls} times, fewer than PTHREAD_DESTRUCTOR_ITERATIONS ({rounds})"))
    })
}

fn tls_destructor_exit() -> CaseResult {
    trial(|| {
        have_pthread_exit()?;
        let k = new_key(Some(dtor0))?;
        KEYS[0].store(k, SeqCst);
        let th = spawn("the thread ending with pthread_exit", move || -> CaseResult {
            set(k, 0xe1)?;
            Err(pthread_exit(null_mut()))
        })?;
        dtor_settled(0, 1)?;
        if th.finished() { th.wait(10)??; }
        let v = DTOR_VALUE[0].load(SeqCst);
        check(v == 0xe1, &format!("the destructor was passed {v:#x}, not the 0xe1 the thread set before pthread_exit"))
    })
}

fn tls_key_delete() -> CaseResult {
    trial(|| {
        let k = new_key(Some(dtor0))?;
        let kept = new_key(Some(dtor1))?;
        KEYS[0].store(k, SeqCst);
        let (ready, go) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let (r, g) = (ready.clone(), go.clone());
        let th = spawn("the thread", move || -> CaseResult {
            set(k, 0x5)?;
            set(kept, 0x6)?;
            r.store(1, SeqCst);
            check(until(2000, || g.load(SeqCst) == 1), "never told to end")
        })?;
        check(until(1000, || ready.load(SeqCst) == 1 || th.finished()), "the thread did not set its value within a second")?;
        rc("pthread_key_delete", pthread_key_delete(k)?)?;
        go.store(1, SeqCst);
        th.wait(3000)??;
        dtor_settled(1, 1)?;
        let calls = DTOR_CALLS[0].load(SeqCst);
        check(calls == 0, &format!("the destructor of a deleted key was called {calls} times when a thread holding a value ended"))
    })
}

fn tls_keys_max() -> CaseResult {
    trial(|| {
        let reported = sysconf(SC_THREAD_KEYS_MAX)?;
        let n = reported.max(POSIX_KEYS_MAX);
        let Ok(n) = usize::try_from(n) else { return fail(format!("sysconf(_SC_THREAD_KEYS_MAX) returned {reported}")) };
        let mut keys = Vec::with_capacity(n);
        let mut given = std::collections::HashSet::with_capacity(n);
        for i in 0..n {
            let mut key = u32::MAX;
            let r = pthread_key_create(&mut key, None)?;
            if r != 0 { return fail(format!("pthread_key_create number {} of {n} returned {}", i + 1, rc_text(r))); }
            if !given.insert(key) { return fail(format!("pthread_key_create gave key {key} twice")); }
            keys.push(key);
        }
        for (i, &k) in keys.iter().enumerate() { set(k, i + 1)?; }
        let wrong = keys.iter().enumerate().filter(|&(i, &k)| get(k).map(|v| v != i + 1).unwrap_or(true)).count();
        value("keys", (n - wrong) as i64, "", Some((n as i64, n as i64)));
        check(wrong == 0, &format!("{wrong} of {n} keys did not keep their own value"))
    })
}

fn tls_errno() -> CaseResult {
    trial(|| {
        let main_errno = errno_location()?;
        // SAFETY: __errno_location points to the calling thread's errno.
        unsafe { *main_errno = 0 };
        let (addr, set_to) = spawn("the thread", || -> Result<(usize, i32), CaseError> {
            let e = errno_location()?;
            let r = c_close(-1)?;
            check(r == -1, "close(-1) did not fail")?;
            // SAFETY: as above.
            Ok((e as usize, unsafe { *e }))
        })?
        .wait(2000)??;
        check(set_to == EBADF, &format!("close(-1) in a thread left errno {set_to}, not EBADF"))?;
        check(addr != main_errno as usize, &format!("__errno_location returns {addr:#x} in both the main thread and another"))?;
        // SAFETY: as above.
        let now = unsafe { *main_errno };
        check(now == 0, &format!("a failing call in another thread changed the main thread's errno to {now}"))
    })
}

/// The calling thread's thread pointer: TPIDR_EL0 on ARM64, the FS base on x86-64.
fn thread_pointer() -> Result<u64, CaseError> {
    #[cfg(target_arch = "aarch64")]
    {
        let v: u64;
        // SAFETY: reading TPIDR_EL0 from user mode is always allowed.
        unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) v, options(nomem, nostack)) };
        Ok(v)
    }
    #[cfg(target_arch = "x86_64")]
    {
        const ARCH_GET_FS: u64 = 0x1003;
        let mut v = 0u64;
        let r = sc(nr::ARCH_PRCTL, &[ARCH_GET_FS, &mut v as *mut u64 as u64]);
        if r != 0 { return err(format!("arch_prctl(ARCH_GET_FS) returned {}", shown(r))); }
        Ok(v)
    }
}

fn tls_thread_pointer() -> CaseResult {
    // Three threads read their thread pointers and stay until all three have, so the four
    // pointers compared belong to threads that are all live at once.
    const N: u32 = 3;
    trial(|| {
        let main = thread_pointer()?;
        check(main != 0, "the main thread's thread pointer is 0")?;
        let read = Arc::new(AtomicU32::new(0));
        let mut threads = Vec::new();
        for i in 0..N {
            let r = read.clone();
            threads.push(spawn(&format!("thread {i}"), move || -> Result<u64, CaseError> {
                let tp = thread_pointer();
                r.fetch_add(1, SeqCst);
                check(until(2000, || r.load(SeqCst) == N), "the threads did not all read their thread pointers within 2 s")?;
                tp
            })?);
        }
        let mut seen = vec![main];
        for (i, th) in threads.into_iter().enumerate() {
            let tp = th.wait(3000)??;
            check(tp != 0, &format!("new thread {i}'s thread pointer is 0"))?;
            check(!seen.contains(&tp), &format!("new thread {i}'s thread pointer {tp:#x} is that of another thread live at the same time"))?;
            seen.push(tp);
        }
        Ok(())
    })
}

#[thread_local]
static TLS_INIT: Cell<u64> = Cell::new(0x5eed);
#[thread_local]
static TLS_ZERO: Cell<u64> = Cell::new(0);

/// Before touching compiler TLS in a new thread, check it has a thread pointer of its own,
/// so a missing one fails the case rather than faulting.
fn own_thread_pointer(main: u64) -> CaseResult {
    let tp = thread_pointer()?;
    check(tp != 0, "a new thread's thread pointer is 0, so it has no thread-local storage")?;
    check(tp != main, "a new thread's thread pointer is the main thread's")
}

fn tls_compiler_initial() -> CaseResult {
    trial(|| {
        let main_tp = thread_pointer()?;
        check(main_tp != 0, "the main thread's thread pointer is 0, so it has no thread-local storage")?;
        let (i, z) = (TLS_INIT.get(), TLS_ZERO.get());
        check(i == 0x5eed && z == 0, &format!("in the main thread a thread-local initialized to 0x5eed reads {i:#x} and one initialized to 0 reads {z:#x}"))?;
        let (i, z) = spawn("the thread", move || -> Result<(u64, u64), CaseError> {
            own_thread_pointer(main_tp)?;
            step("reading compiler TLS in a new thread");
            Ok((TLS_INIT.get(), TLS_ZERO.get()))
        })?
        .wait(2000)??;
        check(i == 0x5eed && z == 0, &format!("in a new thread a thread-local initialized to 0x5eed reads {i:#x} and one initialized to 0 reads {z:#x}"))
    })
}

fn tls_compiler_isolated() -> CaseResult {
    const N: u64 = 4;
    trial(|| {
        let main_tp = thread_pointer()?;
        check(main_tp != 0, "the main thread's thread pointer is 0, so it has no thread-local storage")?;
        TLS_INIT.set(7);
        let written = Arc::new(AtomicU64::new(0));
        let mut threads = Vec::new();
        for i in 0..N {
            let w = written.clone();
            threads.push(spawn(&format!("thread {i}"), move || -> Result<(u64, usize), CaseError> {
                own_thread_pointer(main_tp)?;
                step("writing compiler TLS in a new thread");
                TLS_INIT.set(1000 + i);
                w.fetch_add(1, SeqCst);
                check(until(2000, || w.load(SeqCst) == N), "the threads did not all write within 2 s")?;
                Ok((TLS_INIT.get(), &TLS_INIT as *const Cell<u64> as usize))
            })?);
        }
        let mut addrs = Vec::new();
        for (i, th) in threads.into_iter().enumerate() {
            let (v, at) = th.wait(3000)??;
            check(v == 1000 + i as u64, &format!("thread {i} wrote {} to its thread-local and read back {v}", 1000 + i))?;
            check(!addrs.contains(&at), &format!("two threads' thread-locals are at the same address {at:#x}"))?;
            addrs.push(at);
        }
        let main = TLS_INIT.get();
        check(main == 7, &format!("the main thread's thread-local reads {main} after other threads wrote theirs, not 7"))
    })
}

// ---------------------------------------------------------------------------
// signals

static HITS: AtomicU32 = AtomicU32::new(0);
static HIT_TID: AtomicI64 = AtomicI64::new(0);
static STOP: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_signal(_sig: i32) {
    HIT_TID.store(gettid(), SeqCst);
    HITS.fetch_add(1, SeqCst);
}

fn install(sig: i32) -> CaseResult {
    let act = Sigaction { handler: on_signal as usize as u64, flags: SA_RESTORER, restorer: restore_rt as usize as u64, mask: 0 };
    sys0(&format!("rt_sigaction({sig})"), sc(nr::RT_SIGACTION, &[sig as u64, &act as *const Sigaction as u64, 0, 8]))
}

/// A thread that records its thread ID, says it is ready, runs `setup` and then naps until
/// STOP is set.
fn idler(name: &str, setup: impl FnOnce() -> CaseResult + Send + 'static) -> Result<(Th<CaseResult>, i64), CaseError> {
    let tid = Arc::new(AtomicI64::new(0));
    let t = tid.clone();
    let th = spawn(name, move || -> CaseResult {
        setup()?;
        t.store(gettid(), SeqCst);
        let t0 = now_ms();
        while STOP.load(SeqCst) == 0 && now_ms() - t0 < 8000 { nap(); }
        Ok(())
    })?;
    if !until(1000, || tid.load(SeqCst) != 0 || th.finished()) { return err(format!("{name} did not start within a second")); }
    if th.finished() { th.wait(10)??; return err(format!("{name} ended early")); }
    let id = tid.load(SeqCst);
    Ok((th, id))
}

fn sigmask(how: i32, set: Option<&SigSet>) -> Result<SigSet, CaseError> {
    let mut old = [0u64; 16];
    let what = match (how, set) {
        (_, None) => "pthread_sigmask reading the mask",
        (SIG_BLOCK, _) => "pthread_sigmask(SIG_BLOCK)",
        _ => "pthread_sigmask(SIG_UNBLOCK)",
    };
    rc(what, pthread_sigmask(how, set.map_or(null(), |s| s.as_ptr()), old.as_mut_ptr())?)?;
    Ok(old)
}

fn current_mask() -> Result<SigSet, CaseError> { sigmask(SIG_BLOCK, None) }

fn raw_pending() -> Result<SigSet, CaseError> {
    let mut set = [0u64; 16];
    sys0("rt_sigpending", sc(nr::RT_SIGPENDING, &[set.as_mut_ptr() as u64, 8]))?;
    Ok(set)
}

fn finish_idlers(threads: Vec<Th<CaseResult>>) -> CaseResult {
    STOP.store(1, SeqCst);
    for th in threads { th.wait(2000)??; }
    Ok(())
}

fn sg_pthread_kill() -> CaseResult {
    trial(|| {
        install(SIGUSR1)?;
        let mut threads = Vec::new();
        let mut tids = Vec::new();
        for i in 0..3 {
            let (th, tid) = idler(&format!("thread {i}"), || Ok(()))?;
            threads.push(th);
            tids.push(tid);
        }
        rc("pthread_kill(thread 1, SIGUSR1)", pthread_kill(threads[1].handle, SIGUSR1)?)?;
        check(until(1000, || HITS.load(SeqCst) >= 1), "the handler did not run within a second of pthread_kill")?;
        sleep_ms(50);
        let hits = HITS.load(SeqCst);
        let ran = HIT_TID.load(SeqCst);
        finish_idlers(threads)?;
        check(hits == 1, &format!("one pthread_kill ran the handler {hits} times"))?;
        check(ran == tids[1], &format!("the handler ran in thread {ran}, not the target thread {}", tids[1]))
    })
}

fn sg_pthread_kill_zero() -> CaseResult {
    trial(|| {
        let (th, _) = idler("the thread", || Ok(()))?;
        rc("pthread_kill(thread, 0) of a live thread", pthread_kill(th.handle, 0)?)?;
        finish_idlers(vec![th])
    })
}

fn sg_pthread_kill_einval() -> CaseResult {
    trial(|| {
        let (th, _) = idler("the thread", || Ok(()))?;
        want("pthread_kill(thread, 99)", pthread_kill(th.handle, 99)?, EINVAL)?;
        finish_idlers(vec![th])
    })
}

fn sg_sigmask_per_thread() -> CaseResult {
    trial(|| {
        let seen = Arc::new(AtomicU64::new(0));
        let s = seen.clone();
        let (th, _) = idler("the blocking thread", move || -> CaseResult {
            sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR1])))?;
            s.store(current_mask()?[0] | 1 << 63, SeqCst);
            Ok(())
        })?;
        let theirs = seen.load(SeqCst);
        let mine = current_mask()?;
        finish_idlers(vec![th])?;
        check(theirs & (1 << (SIGUSR1 - 1)) != 0, "the thread's own mask did not include the SIGUSR1 it blocked")?;
        check(!has(&mine, SIGUSR1), "another thread's pthread_sigmask(SIG_BLOCK) blocked SIGUSR1 in the main thread")
    })
}

fn sg_sigmask_inherited() -> CaseResult {
    trial(|| {
        sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR2])))?;
        let mask = spawn("the thread", current_mask)?.wait(2000)??;
        check(has(&mask, SIGUSR2), "a new thread's mask did not include the SIGUSR2 its creator had blocked")
    })
}

fn sg_sigmask_einval() -> CaseResult {
    trial_ms(3000, || {
        let set = sigset(&[SIGUSR1]);
        want("pthread_sigmask(99, set)", pthread_sigmask(99, set.as_ptr(), null_mut())?, EINVAL)
    })
}

fn sg_pending_per_thread() -> CaseResult {
    trial(|| {
        install(SIGUSR1)?;
        let (theirs, unblock) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU32::new(0)));
        let (t, u) = (theirs.clone(), unblock.clone());
        let tid = Arc::new(AtomicI64::new(0));
        let tid2 = tid.clone();
        let ready = Arc::new(AtomicU32::new(0));
        let r = ready.clone();
        let th = spawn("the blocking thread", move || -> CaseResult {
            sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR1])))?;
            tid2.store(gettid(), SeqCst);
            r.store(1, SeqCst);
            check(until(2000, || u.load(SeqCst) == 1), "never told to look")?;
            t.store(raw_pending()?[0] | 1 << 63, SeqCst);
            sigmask(SIG_UNBLOCK, Some(&sigset(&[SIGUSR1])))?;
            Ok(())
        })?;
        check(until(1000, || ready.load(SeqCst) == 1 || th.finished()), "the thread did not block SIGUSR1 within a second")?;
        rc("pthread_kill(thread, SIGUSR1)", pthread_kill(th.handle, SIGUSR1)?)?;
        sleep_ms(50);
        let mine = raw_pending()?;
        unblock.store(1, SeqCst);
        th.wait(2000)??;
        check(until(1000, || HITS.load(SeqCst) >= 1), "the signal was not delivered once the thread unblocked it")?;
        check(theirs.load(SeqCst) & (1 << (SIGUSR1 - 1)) != 0, "the target thread's pending set did not include the SIGUSR1 sent to it")?;
        check(!has(&mine, SIGUSR1), "a signal sent to one thread is pending in another")?;
        let ran = HIT_TID.load(SeqCst);
        check(ran == tid.load(SeqCst), &format!("the handler ran in thread {ran}, not the target thread {}", tid.load(SeqCst)))
    })
}

fn sg_process_to_unblocked() -> CaseResult {
    trial(|| {
        install(SIGUSR1)?;
        sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR1])))?;
        let (blocked, _) = idler("the blocking thread", || Ok(()))?;
        let (open, open_tid) = idler("the thread that unblocks", || sigmask(SIG_UNBLOCK, Some(&sigset(&[SIGUSR1]))).map(|_| ()))?;
        rc("kill(getpid(), SIGUSR1)", c_kill(c_getpid()?, SIGUSR1)?)?;
        let ran = until(1000, || HITS.load(SeqCst) >= 1);
        let tid = HIT_TID.load(SeqCst);
        finish_idlers(vec![blocked, open])?;
        check(ran, "a signal sent to the process did not run the handler within a second")?;
        check(tid == open_tid, &format!("a signal sent to the process ran the handler in thread {tid}, not the only thread that did not block it ({open_tid})"))
    })
}

fn sg_process_pending() -> CaseResult {
    trial(|| {
        install(SIGUSR1)?;
        sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR1])))?;
        let go = Arc::new(AtomicU32::new(0));
        let g = go.clone();
        let tid = Arc::new(AtomicI64::new(0));
        let t = tid.clone();
        let th = spawn("the thread that unblocks later", move || -> CaseResult {
            t.store(gettid(), SeqCst);
            check(until(2000, || g.load(SeqCst) == 1), "never told to unblock")?;
            sigmask(SIG_UNBLOCK, Some(&sigset(&[SIGUSR1])))?;
            sleep_ms(200);
            Ok(())
        })?;
        check(until(1000, || tid.load(SeqCst) != 0), "the thread did not start within a second")?;
        rc("kill(getpid(), SIGUSR1)", c_kill(c_getpid()?, SIGUSR1)?)?;
        sleep_ms(100);
        let early = HITS.load(SeqCst);
        go.store(1, SeqCst);
        let ran = until(1000, || HITS.load(SeqCst) >= 1);
        th.wait(2000)??;
        check(early == 0, "the handler ran while every thread blocked the signal")?;
        check(ran, "a pending process signal was not delivered when a thread unblocked it")?;
        let hit = HIT_TID.load(SeqCst);
        check(hit == tid.load(SeqCst), &format!("the handler ran in thread {hit}, not the thread that unblocked the signal"))
    })
}

/// A thread that calls sigwait for `sig` and returns what it got.
fn sigwaiter(sig: i32) -> Result<(Th<Result<i32, CaseError>>, Arc<AtomicU32>), CaseError> {
    let ready = Arc::new(AtomicU32::new(0));
    let r = ready.clone();
    let th = spawn("the sigwait thread", move || -> Result<i32, CaseError> {
        let set = sigset(&[sig]);
        let mut got = 0;
        r.store(1, SeqCst);
        rc("sigwait", sigwait(set.as_ptr(), &mut got)?)?;
        Ok(got)
    })?;
    check(until(1000, || ready.load(SeqCst) == 1), "the sigwait thread did not start within a second")?;
    sleep_ms(50);
    Ok((th, ready))
}

fn sg_sigwait_process() -> CaseResult {
    trial(|| {
        install(SIGUSR1)?;
        sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR1])))?;
        let (th, _) = sigwaiter(SIGUSR1)?;
        rc("kill(getpid(), SIGUSR1)", c_kill(c_getpid()?, SIGUSR1)?)?;
        let got = th.wait(1000).map_err(|_| CaseError::Fail("sigwait did not return within a second of a signal sent to the process".into()))??;
        check(got == SIGUSR1, &format!("sigwait returned signal {got}, not SIGUSR1"))?;
        check(HITS.load(SeqCst) == 0, "the handler ran for a signal a thread was waiting for with sigwait")
    })
}

fn sg_sigwait_thread() -> CaseResult {
    trial(|| {
        sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR2])))?;
        let (th, _) = sigwaiter(SIGUSR2)?;
        rc("pthread_kill(sigwait thread, SIGUSR2)", pthread_kill(th.handle, SIGUSR2)?)?;
        let got = th.wait(1000).map_err(|_| CaseError::Fail("sigwait did not return within a second of pthread_kill to its thread".into()))??;
        check(got == SIGUSR2, &format!("sigwait returned signal {got}, not SIGUSR2"))
    })
}

fn sg_sigwait_pending() -> CaseResult {
    trial_ms(3000, || {
        sigmask(SIG_BLOCK, Some(&sigset(&[SIGUSR1])))?;
        rc("kill(getpid(), SIGUSR1)", c_kill(c_getpid()?, SIGUSR1)?)?;
        check(has(&raw_pending()?, SIGUSR1), "the blocked SIGUSR1 is not pending")?;
        let set = sigset(&[SIGUSR1]);
        let mut got = 0;
        step("in sigwait with SIGUSR1 already pending");
        rc("sigwait", sigwait(set.as_ptr(), &mut got)?)?;
        check(got == SIGUSR1, &format!("sigwait returned signal {got}, not SIGUSR1"))?;
        check(!has(&raw_pending()?, SIGUSR1), "SIGUSR1 is still pending after sigwait took it")
    })
}

fn sg_raise_thread() -> CaseResult {
    trial(|| {
        install(SIGUSR1)?;
        let th = spawn("the raising thread", || -> Result<i64, CaseError> {
            rc("raise(SIGUSR1)", raise(SIGUSR1)?)?;
            Ok(gettid())
        })?;
        let tid = th.wait(2000)??;
        check(until(1000, || HITS.load(SeqCst) >= 1), "the handler did not run within a second of raise")?;
        let ran = HIT_TID.load(SeqCst);
        check(ran == tid, &format!("raise in thread {tid} ran the handler in thread {ran}"))
    })
}

fn sg_default_whole_process() -> CaseResult {
    let mut p = spawn_proc(|| {
        let (th, _) = idler("the target thread", || Ok(()))?;
        rc("pthread_kill(thread, SIGTERM)", pthread_kill(th.handle, SIGTERM)?)?;
        sleep_ms(2000);
        err("the process was still running 2 s after pthread_kill sent SIGTERM with its default action")
    })?;
    let status = p.wait(3000);
    if p.board.state.load(SeqCst) == FAILED { return fail(p.board.msg()); }
    let Some(status) = status else { return fail("the process was still running 3 s after pthread_kill sent SIGTERM") };
    check(status & 0x7f == SIGTERM, &format!("after pthread_kill sent one thread SIGTERM the process {}, not killed by SIGTERM", status_text(status)))
}

fn sg_sigkill_threads() -> CaseResult {
    let mut p = spawn_proc(|| {
        heartbeat_threads(4)?;
        if let Some(b) = board() { b.words[0].store(1, SeqCst); }
        sleep_ms(8000);
        Ok(())
    })?;
    if !until(3000, || p.board.word(0) == 1 || p.board.state.load(SeqCst) != RUNNING) {
        return fail(format!("the process did not start its four threads within 3 s{}", p.board.last_step()));
    }
    if p.board.state.load(SeqCst) == FAILED { return fail(p.board.msg()); }
    sys0("kill(SIGKILL)", sc(nr::KILL, &[p.pid as u64, SIGKILL as u64]))?;
    let t0 = mono();
    let Some(status) = p.wait(1000) else { return fail("a process with four spinning threads was still running a second after SIGKILL") };
    value("ended", (mono() - t0) / 1000, "us", Some((0, 1_000_000)));
    check(status & 0x7f == SIGKILL, &format!("after SIGKILL the process {}", status_text(status)))?;
    heartbeats_stopped(p.board, 4)
}

fn sg_tgkill() -> CaseResult {
    trial(|| {
        install(SIGUSR1)?;
        let mut threads = Vec::new();
        let mut tids = Vec::new();
        for i in 0..3 {
            let (th, tid) = idler(&format!("thread {i}"), || Ok(()))?;
            threads.push(th);
            tids.push(tid);
        }
        let pid = c_getpid()?;
        sys0("tgkill(pid, thread 2, SIGUSR1)", tgkill(pid as i64, tids[2], SIGUSR1))?;
        let ran = until(1000, || HITS.load(SeqCst) >= 1);
        let tid = HIT_TID.load(SeqCst);
        finish_idlers(threads)?;
        check(ran, "the handler did not run within a second of tgkill")?;
        check(tid == tids[2], &format!("tgkill to thread {} ran the handler in thread {tid}", tids[2]))
    })
}

fn raw_mask(how: i32, set: Option<&SigSet>) -> Result<u64, CaseError> {
    let mut old = [0u64; 16];
    sys0("rt_sigprocmask", sc(nr::RT_SIGPROCMASK, &[how as u64, set.map_or(0, |s| s.as_ptr() as u64), old.as_mut_ptr() as u64, 8]))?;
    Ok(old[0])
}

fn sg_rt_sigprocmask() -> CaseResult {
    trial(|| {
        let before = raw_mask(SIG_BLOCK, None)?;
        let theirs = spawn("the blocking thread", || -> Result<u64, CaseError> {
            raw_mask(SIG_BLOCK, Some(&sigset(&[SIGUSR2])))?;
            raw_mask(SIG_BLOCK, None)
        })?
        .wait(2000)??;
        let mine = raw_mask(SIG_BLOCK, None)?;
        let bit = 1u64 << (SIGUSR2 - 1);
        check(theirs & bit != 0, "rt_sigprocmask(SIG_BLOCK, SIGUSR2) in a thread did not change that thread's mask")?;
        check(mine == before, &format!("rt_sigprocmask in another thread changed the main thread's mask from {before:#x} to {mine:#x}"))
    })
}

// ---------------------------------------------------------------------------
// scheduling

fn sc_getschedparam_self() -> CaseResult {
    trial_ms(3000, || {
        let (policy, prio) = get_sched(me()?)?;
        check(policy == SCHED_OTHER && prio == 0, &format!("a thread started normally reports {} with priority {prio}", policy_name(policy)))
    })
}

fn sc_priority_range() -> CaseResult {
    trial_ms(3000, || {
        for policy in [SCHED_FIFO, SCHED_RR] {
            let (lo, hi) = (sched_get_priority_min(policy)?, sched_get_priority_max(policy)?);
            check(lo >= 0 && hi >= 0, &format!("sched_get_priority_min and _max for {} returned {lo} and {hi}", policy_name(policy)))?;
            if policy == SCHED_FIFO { value("fifo-priorities", (hi - lo + 1) as i64, "", None); }
            check(hi - lo >= 31, &format!("{} has priorities {lo} to {hi}, fewer than 32", policy_name(policy)))?;
        }
        Ok(())
    })
}

fn sc_priority_einval() -> CaseResult {
    trial_ms(3000, || {
        let e = errno_location()?;
        // SAFETY: __errno_location points to the calling thread's errno.
        unsafe { *e = 0 };
        let r = sched_get_priority_max(99)?;
        // SAFETY: as above.
        let errno = unsafe { *e };
        check(r == -1 && errno == EINVAL, &format!("sched_get_priority_max(99) returned {r} with errno {}", rc_text(errno)))
    })
}

fn sc_set_policy(policy: i32) -> CaseResult {
    trial_ms(4000, move || {
        let prio = fifo_min()? + 1;
        set_sched(policy, prio)?;
        let (p, q) = get_sched(me()?)?;
        check(p == policy && q == prio, &format!("after {} with priority {prio}, pthread_getschedparam reports {} with priority {q}", policy_name(policy), policy_name(p)))?;
        if policy == SCHED_RR {
            let mut iv: Ts = [0; 2];
            rc("sched_rr_get_interval", sched_rr_get_interval(0, iv.as_mut_ptr())?)?;
            let us = ts_ns(&iv) / 1000;
            value("interval", us, "us", None);
            check(us > 0, "sched_rr_get_interval reported a quantum of 0")?;
        }
        set_sched(SCHED_OTHER, 0)
    })
}

fn sc_setschedparam_fifo() -> CaseResult { sc_set_policy(SCHED_FIFO) }
fn sc_setschedparam_rr() -> CaseResult { sc_set_policy(SCHED_RR) }

fn sc_setschedparam_thread() -> CaseResult {
    trial(|| {
        let prio = fifo_min()? + 2;
        let th = spawn("the thread", move || -> Result<(i32, i32), CaseError> {
            let mut seen = (SCHED_OTHER, 0);
            let t0 = now_ms();
            while now_ms() - t0 < 2000 {
                seen = get_sched(me()?)?;
                if seen.0 != SCHED_OTHER { break; }
                nap();
            }
            Ok(seen)
        })?;
        let param = prio;
        rc(&format!("pthread_setschedparam(thread, SCHED_FIFO, {prio})"), pthread_setschedparam(th.handle, SCHED_FIFO, &param)?)?;
        let (p, q) = th.wait(3000)??;
        check(p == SCHED_FIFO && q == prio, &format!("the thread reports {} with priority {q} after another set SCHED_FIFO with {prio}", policy_name(p)))
    })
}

fn sc_setschedparam_eperm() -> CaseResult {
    // An unprivileged thread may still take SCHED_FIFO up to its RLIMIT_RTPRIO, so the case
    // sets that limit to 0 before giving up its user ID.
    const RLIMIT_RTPRIO: u64 = 14;
    trial_ms(3000, || {
        let prio = fifo_min()? + 1;
        let zero: [u64; 2] = [0, 0];
        sys0("prlimit64(RLIMIT_RTPRIO, 0)", sc(nr::PRLIMIT64, &[0, RLIMIT_RTPRIO, zero.as_ptr() as u64, 0]))?;
        sys0(&format!("setuid({USER_A})"), sc(nr::SETUID, &[USER_A as u64]))?;
        let param = prio;
        want("pthread_setschedparam(SCHED_FIFO) by an unprivileged thread", pthread_setschedparam(me()?, SCHED_FIFO, &param)?, EPERM)?;
        let (p, _) = get_sched(me()?)?;
        check(p == SCHED_OTHER, &format!("after the refused change the thread reports {}", policy_name(p)))
    })
}

fn sc_setschedprio() -> CaseResult {
    trial_ms(3000, || {
        let lo = fifo_min()?;
        set_sched(SCHED_FIFO, lo + 1)?;
        rc(&format!("pthread_setschedprio({})", lo + 3), pthread_setschedprio(me()?, lo + 3)?)?;
        let (p, q) = get_sched(me()?)?;
        check(p == SCHED_FIFO && q == lo + 3, &format!("after pthread_setschedprio({}) the thread reports {} with priority {q}", lo + 3, policy_name(p)))?;
        set_sched(SCHED_OTHER, 0)
    })
}

fn sc_inherit_sched() -> CaseResult {
    trial(|| {
        let prio = fifo_min()? + 4;
        set_sched(SCHED_FIFO, prio)?;
        let a = new_attr()?;
        rc("pthread_attr_setinheritsched(PTHREAD_INHERIT_SCHED)", pthread_attr_setinheritsched(a.p(), PTHREAD_INHERIT_SCHED)?)?;
        let mut inherit = -1;
        rc("pthread_attr_getinheritsched", pthread_attr_getinheritsched(a.p(), &mut inherit)?)?;
        check(inherit == PTHREAD_INHERIT_SCHED, &format!("pthread_attr_getinheritsched gave {inherit} after PTHREAD_INHERIT_SCHED was set"))?;
        let (p, q) = attr_thread_sched(a)?;
        set_sched(SCHED_OTHER, 0)?;
        check(p == SCHED_FIFO && q == prio, &format!("a thread created to inherit from a SCHED_FIFO {prio} creator reports {} with priority {q}", policy_name(p)))
    })
}

static SCHED_SEEN: [AtomicI32; 2] = [AtomicI32::new(-1), AtomicI32::new(-1)];
extern "C" fn rt_report_sched(_: *mut u8) -> *mut u8 {
    if let Ok(Ok((p, q))) = me().map(get_sched) {
        SCHED_SEEN[1].store(q, SeqCst);
        SCHED_SEEN[0].store(p, SeqCst);
    } else {
        SCHED_SEEN[0].store(-2, SeqCst);
    }
    null_mut()
}

/// The policy and priority a thread created with `attr` reports for itself.
fn attr_thread_sched(attr: P) -> Result<(i32, i32), CaseError> {
    create_attr(attr, rt_report_sched, null_mut())?;
    check(until(1000, || SCHED_SEEN[0].load(SeqCst) != -1), "the new thread did not report within a second")?;
    let p = SCHED_SEEN[0].load(SeqCst);
    if p == -2 { return err("the new thread could not read its own scheduling parameters"); }
    Ok((p, SCHED_SEEN[1].load(SeqCst)))
}

fn sc_explicit_sched() -> CaseResult {
    trial(|| {
        let prio = fifo_min()? + 5;
        let a = new_attr()?;
        rc("pthread_attr_setinheritsched(PTHREAD_EXPLICIT_SCHED)", pthread_attr_setinheritsched(a.p(), PTHREAD_EXPLICIT_SCHED)?)?;
        rc("pthread_attr_setschedpolicy(SCHED_RR)", pthread_attr_setschedpolicy(a.p(), SCHED_RR)?)?;
        let param = prio;
        rc(&format!("pthread_attr_setschedparam({prio})"), pthread_attr_setschedparam(a.p(), &param)?)?;
        let (mut policy, mut q) = (-1, -1);
        rc("pthread_attr_getschedpolicy", pthread_attr_getschedpolicy(a.p(), &mut policy)?)?;
        rc("pthread_attr_getschedparam", pthread_attr_getschedparam(a.p(), &mut q)?)?;
        check(policy == SCHED_RR && q == prio, &format!("the attribute reports {} with priority {q} after SCHED_RR with {prio} was set", policy_name(policy)))?;
        let (p, q) = attr_thread_sched(a)?;
        check(p == SCHED_RR && q == prio, &format!("a thread created with explicit SCHED_RR {prio} reports {} with priority {q}", policy_name(p)))
    })
}

fn sc_inheritsched_default() -> CaseResult {
    trial_ms(3000, || {
        let a = new_attr()?;
        let mut inherit = -1;
        rc("pthread_attr_getinheritsched", pthread_attr_getinheritsched(a.p(), &mut inherit)?)?;
        check(inherit == PTHREAD_INHERIT_SCHED, &format!("a new attribute object's inherit-scheduler is {inherit}, not PTHREAD_INHERIT_SCHED"))
    })
}

/// Two threads on processor 0 hand a turn back and forth `HANDOFFS` times, each waiting
/// for its turn by calling sched_yield. With `fifo`, both run SCHED_FIFO at one priority.
fn yield_pair(fifo: bool) -> CaseResult {
    trial(move || {
        let prio = if fifo { fifo_min()? + 1 } else { 0 };
        let turn = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicU32::new(0));
        let mut threads = Vec::new();
        for me_i in 0..2u64 {
            let (turn, ready) = (turn.clone(), ready.clone());
            threads.push(spawn(&format!("thread {me_i}"), move || -> Result<i64, CaseError> {
                pin_to(0).map_err(CaseError::Fail)?;
                if fifo { set_sched(SCHED_FIFO, prio)?; }
                ready.fetch_add(1, SeqCst);
                // Sleep, not yield, until both are ready: a SCHED_FIFO thread yielding keeps
                // processor 0 from the other until it has taken SCHED_FIFO too.
                check(until(2000, || ready.load(SeqCst) >= 2), "the other thread did not get ready on processor 0 within 2 s")?;
                let t0 = mono();
                loop {
                    let t = turn.load(SeqCst);
                    if t >= 2 * HANDOFFS { break; }
                    if t % 2 == me_i {
                        turn.store(t + 1, SeqCst);
                    } else {
                        rc("sched_yield", sched_yield()?)?;
                        if mono() - t0 > 2 * NS { return err(format!("the threads made only {} of {} handoffs in 2 s", t, 2 * HANDOFFS)); }
                    }
                }
                Ok((mono() - t0) / MS)
            })?);
        }
        let mut ms = 0;
        for th in threads { ms = ms.max(th.wait(4000)??); }
        value("handoffs", ms, "ms", Some((0, 500)));
        check(ms <= 500, &format!("two threads on one processor took {ms} ms to hand the processor back and forth {} times with sched_yield", 2 * HANDOFFS))
    })
}

fn sc_yield_other() -> CaseResult { yield_pair(false) }
fn sc_yield_fifo() -> CaseResult { yield_pair(true) }

/// What `fifo_race` saw, each on CLOCK_MONOTONIC: when B went to sleep, its deadline and
/// when it ran again, and when A started and stopped computing.
struct Race { b_slept: i64, b_deadline: i64, b_woke: i64, a_start: i64, a_end: i64 }

/// On processor 0, thread B (SCHED_FIFO at `prio + prio_b_offset`) goes to sleep until a
/// deadline 150 ms ahead; thread A (SCHED_FIFO at `prio`) starts computing 50 ms before that
/// deadline and computes for 300 ms, so B's deadline passes while A computes.
fn fifo_race(prio_b_offset: i32) -> Result<Race, CaseError> {
    let prio = fifo_min()? + 1;
    let deadline = Arc::new(AtomicI64::new(0));
    let db = deadline.clone();
    let b = spawn("thread B", move || -> Result<(i64, i64, i64), CaseError> {
        pin_to(0).map_err(CaseError::Fail)?;
        set_sched(SCHED_FIFO, prio + prio_b_offset)?;
        let slept = mono();
        let wake = slept + 150 * MS;
        db.store(wake, SeqCst);
        let t = ts(wake - mono());
        let _ = sc(nr::NANOSLEEP, &[t.as_ptr() as u64, 0]);
        Ok((slept, wake, mono()))
    })?;
    let da = deadline.clone();
    let a = spawn("thread A", move || -> Result<(i64, i64), CaseError> {
        pin_to(0).map_err(CaseError::Fail)?;
        set_sched(SCHED_FIFO, prio)?;
        check(until(2000, || da.load(SeqCst) != 0), "thread B never went to sleep")?;
        let wake = da.load(SeqCst);
        while mono() < wake - 50 * MS { nap(); }
        let start = mono();
        while mono() - start < 300 * MS { core::hint::spin_loop(); }
        Ok((start, mono()))
    })?;
    let (a_start, a_end) = a.wait(4000)??;
    let (b_slept, b_deadline, b_woke) = b.wait(4000)??;
    let race = Race { b_slept, b_deadline, b_woke, a_start, a_end };
    check(
        race.b_slept < race.a_start && race.a_start < race.b_deadline && race.b_deadline < race.a_end,
        &format!(
            "thread B slept from {} ms to a deadline at {} ms, and thread A computed from {} ms to {} ms, so the deadline did not fall while A computed",
            race.b_slept / MS, race.b_deadline / MS, race.a_start / MS, race.a_end / MS
        ),
    )?;
    Ok(race)
}

fn sc_fifo_no_preempt() -> CaseResult {
    trial(|| {
        let r = fifo_race(0)?;
        value("waited", (r.b_woke - r.b_deadline) / 1000, "us", None);
        check(r.b_woke >= r.a_end, &format!(
            "a SCHED_FIFO thread whose sleep ended {} ms into an equal-priority thread's computing on its processor ran {} ms in, before that thread stopped at {} ms",
            (r.b_deadline - r.a_start) / MS, (r.b_woke - r.a_start) / MS, (r.a_end - r.a_start) / MS
        ))
    })
}

fn sc_fifo_preempt() -> CaseResult {
    trial(|| {
        let r = fifo_race(1)?;
        on_time("the higher-priority SCHED_FIFO thread's nanosleep, while a lower-priority thread computed on its processor,", r.b_woke, r.b_deadline)
    })
}

fn sc_kernel_getscheduler() -> CaseResult {
    trial_ms(3000, || {
        let r = sc(nr::SCHED_GETSCHEDULER, &[0]);
        check(r == SCHED_OTHER as i64, &format!("sched_getscheduler(0) returned {}, expected SCHED_OTHER (0)", shown(r)))
    })
}

fn sc_kernel_setscheduler() -> CaseResult {
    trial_ms(3000, || {
        let tid = gettid();
        let lo = sc(nr::SCHED_GET_PRIORITY_MIN, &[SCHED_FIFO as u64]);
        check(lo >= 0, &format!("sched_get_priority_min(SCHED_FIFO) returned {}", shown(lo)))?;
        let param = lo as i32 + 1;
        sys0("sched_setscheduler(tid, SCHED_FIFO)", sc(nr::SCHED_SETSCHEDULER, &[tid as u64, SCHED_FIFO as u64, &param as *const i32 as u64]))?;
        let p = sc(nr::SCHED_GETSCHEDULER, &[tid as u64]);
        check(p == SCHED_FIFO as i64, &format!("sched_getscheduler(tid) returned {} after SCHED_FIFO was set", shown(p)))?;
        let mut got = -1i32;
        sys0("sched_getparam(tid)", sc(nr::SCHED_GETPARAM, &[tid as u64, &mut got as *mut i32 as u64]))?;
        check(got == param, &format!("sched_getparam(tid) gave priority {got} after {param} was set"))?;
        let zero = 0i32;
        sys0("sched_setscheduler(tid, SCHED_OTHER)", sc(nr::SCHED_SETSCHEDULER, &[tid as u64, SCHED_OTHER as u64, &zero as *const i32 as u64]))?;
        sys0("sched_setparam(tid, 0)", sc(nr::SCHED_SETPARAM, &[tid as u64, &zero as *const i32 as u64]))
    })
}

// ---------------------------------------------------------------------------

static SUITE: Suite = suite("threads", "Threads", &[
    category("lifecycle", "pthread create, join & detach", &[
        case("create-join", "pthread_create runs the start routine and pthread_join returns once it has returned", lc_create_join),
        case("join-value", "pthread_join stores the value the start routine returned", lc_join_value),
        case("exit-value", "pthread_exit from a nested call ends the thread with its value for pthread_join", lc_exit_value),
        case("join-ended", "pthread_join of a thread that has already ended returns at once with its value", lc_join_ended),
        case("self-equal", "pthread_self in a thread is pthread_equal to the ID pthread_create gave", lc_self_equal),
        case("self-distinct", "pthread_equal tells two threads and the main thread apart", lc_self_distinct),
        case("join-self", "Linux policy: pthread_join of the calling thread returns EDEADLK", lc_join_self),
        case("join-detached", "Linux policy: pthread_join of a detached, running thread returns EINVAL", lc_join_detached),
        case("detach-running", "pthread_detach of a running thread returns 0 and the thread runs to the end", lc_detach_running),
        case("detach-ended", "pthread_detach of a thread that has ended returns 0", lc_detach_ended),
        case("detach-many", "Linux policy: 256 detached threads that ended leave the process's resident memory within 1 MiB", lc_detach_many),
        case("attr-default", "A new thread attribute object is PTHREAD_CREATE_JOINABLE", lc_attr_default),
        case("attr-detached", "PTHREAD_CREATE_DETACHED reads back and a thread created with it runs", lc_attr_detached),
        case("attr-stacksize", "An 8 MiB pthread_attr_setstacksize reads back and its thread can use 6 MiB of stack", lc_attr_stacksize),
        case("attr-stack", "A thread created with pthread_attr_setstack runs on the memory it was given", lc_attr_stack),
        case("attr-stacksize-min", "pthread_attr_setstacksize below PTHREAD_STACK_MIN returns EINVAL", lc_attr_stacksize_min),
        case("attr-guardsize", "pthread_attr_setguardsize's value is what pthread_attr_getguardsize returns", lc_attr_guardsize),
        case("guard-overflow", "Linux policy: a thread overflowing the 256 KiB stack it asked for faults with SIGSEGV within its 64 KiB guard", lc_guard_overflow),
        case("many-threads", "64 threads (_POSIX_THREAD_THREADS_MAX) run at once, each returning its own result", lc_many_threads),
        case("main-exit", "pthread_exit in the main thread leaves the others running and the process exits 0 after the last", lc_main_exit),
        case("exit-from-thread", "exit in one thread ends the whole process, a thread blocked in pthread_join included", lc_exit_from_thread),
        case("exit-from-main", "exit in the main thread ends the process while other threads run", lc_exit_from_main),
        case("exit-tid-word", "Linux ABI: the threads of a process killed and reaped write nothing into pages its parent maps afterwards", lc_exit_tid_word),
        case("getpid-shared", "Every thread sees the process's getpid, and gettid gives each its own thread ID", lc_getpid_shared),
        case("shared-fds", "A descriptor one thread opens is usable in another", lc_shared_fds),
    ]),
    category("mutex", "mutexes", &[
        case("lock-unlock", "A default mutex locks, unlocks, trylocks and is destroyed", mx_lock_unlock),
        case("static-init", "Linux ABI: a zero-filled mutex (PTHREAD_MUTEX_INITIALIZER) excludes another thread", mx_static_init),
        case("exclusion", "pthread_mutex_lock waits while another thread holds the mutex and returns once it is unlocked", mx_exclusion),
        case("trylock-ebusy", "pthread_mutex_trylock returns EBUSY while another thread holds the mutex", mx_trylock_ebusy),
        case("trylock-owner", "pthread_mutex_trylock by the owner of a normal mutex returns EBUSY", mx_trylock_owner),
        case("contention-cpus", "Threads on every processor increment a counter under one mutex without losing an update", mx_contention),
        case("errorcheck-relock", "Relocking an error-checking mutex the thread holds returns EDEADLK", mx_errorcheck_relock),
        case("errorcheck-unlock-other", "Unlocking an error-checking mutex another thread holds returns EPERM", mx_errorcheck_unlock_other),
        case("errorcheck-unlock-unlocked", "Unlocking an unlocked error-checking mutex returns EPERM", mx_errorcheck_unlock_unlocked),
        case("recursive", "A recursive mutex locked three times is free to others only after three unlocks", mx_recursive),
        case("recursive-trylock", "pthread_mutex_trylock by the owner of a recursive mutex takes it again", mx_recursive_trylock),
        case("recursive-unlock-other", "Unlocking a recursive mutex another thread holds returns EPERM", mx_recursive_unlock_other),
        case("attr-type", "pthread_mutexattr_settype reads back each type and refuses an invalid one with EINVAL", mx_attr_type),
        case("timedlock-free", "pthread_mutex_timedlock of a free mutex returns 0 at once", mx_timedlock_free),
        case("timedlock-timeout", "pthread_mutex_timedlock of a held mutex returns ETIMEDOUT at its CLOCK_REALTIME deadline", mx_timedlock_timeout),
        case("timedlock-acquire", "pthread_mutex_timedlock returns 0 when the mutex is released before the deadline", mx_timedlock_acquire),
        case("timedlock-einval", "pthread_mutex_timedlock with tv_nsec of a billion returns EINVAL", mx_timedlock_einval),
        case("robust-ownerdead", "A robust mutex whose owner ended holding it returns EOWNERDEAD and recovers with pthread_mutex_consistent", mx_robust_ownerdead),
        case("robust-notrecoverable", "A robust mutex unlocked without pthread_mutex_consistent returns ENOTRECOVERABLE", mx_robust_notrecoverable),
        case("robust-attr", "pthread_mutexattr_getrobust starts at PTHREAD_MUTEX_STALLED and reads back PTHREAD_MUTEX_ROBUST", mx_robust_attr),
        case("prio-inherit", "A PTHREAD_PRIO_INHERIT mutex's low-priority owner runs ahead of a middle-priority thread, where offered", mx_prio_inherit),
        case("futex-eagain", "Linux ABI: FUTEX_WAIT on a word that does not hold the value returns EAGAIN", mx_futex_eagain),
        case("futex-wake-count", "Linux ABI: FUTEX_WAKE wakes exactly as many waiters as asked", mx_futex_wake_count),
        case("futex-timeout", "Linux ABI: FUTEX_WAIT with a 100 ms timeout returns ETIMEDOUT on time", mx_futex_timeout),
        case("futex-private", "Linux ABI: FUTEX_WAKE_PRIVATE wakes a FUTEX_WAIT_PRIVATE waiter", mx_futex_private),
    ]),
    category("cond", "condition variables", &[
        case("signal-wakes", "pthread_cond_signal wakes a waiting thread, which returns holding the mutex", cv_signal_wakes),
        case("signal-one", "pthread_cond_signal unblocks a waiter and a broadcast the rest, with predicate loops", cv_signal_one),
        case("broadcast-all", "pthread_cond_broadcast wakes all four waiting threads", cv_broadcast_all),
        case("wait-releases", "pthread_cond_wait releases the mutex while it waits", cv_wait_releases),
        case("wait-reacquires", "pthread_cond_wait returns only once it has the mutex again", cv_wait_reacquires),
        case("wakes-waiting", "pthread_cond_signal wakes a thread that was waiting, not one that starts waiting afterwards", cv_wakes_waiting),
        case("timedwait-realtime", "pthread_cond_timedwait returns ETIMEDOUT at its CLOCK_REALTIME deadline holding the mutex", cv_timedwait_realtime),
        case("timedwait-monotonic", "pthread_condattr_setclock(CLOCK_MONOTONIC) reads back and times the wait by that clock", cv_timedwait_monotonic),
        case("timedwait-realtime-set", "Setting CLOCK_REALTIME past the deadline ends a CLOCK_REALTIME timed wait", cv_timedwait_realtime_set),
        case("timedwait-monotonic-set", "Setting CLOCK_REALTIME does not end a CLOCK_MONOTONIC timed wait early", cv_timedwait_monotonic_set),
        case("timedwait-signaled", "pthread_cond_timedwait signalled before its deadline returns 0", cv_timedwait_signaled),
        case("timedwait-past", "pthread_cond_timedwait with a deadline already past returns ETIMEDOUT at once", cv_timedwait_past),
        case("timedwait-einval", "pthread_cond_timedwait with tv_nsec of a billion returns EINVAL", cv_timedwait_einval),
        case("wait-eperm", "pthread_cond_wait with an error-checking mutex the caller does not hold returns EPERM", cv_wait_eperm),
        case("pingpong-cpus", "A token passed round threads on every processor by signal loses no wakeup in 2000 passes", cv_pingpong),
        case("queue-cpus", "A bounded queue between threads on every processor delivers 4000 items exactly once", cv_queue),
        case("broadcast-cycles", "Four waiters see every one of 200 broadcasts", cv_broadcast_cycles),
        case("static-init", "Linux ABI: a zero-filled condition (PTHREAD_COND_INITIALIZER) wakes its waiter", cv_static_init),
        case("futex-bitset-realtime", "Linux ABI: FUTEX_WAIT_BITSET with FUTEX_CLOCK_REALTIME times out at its absolute deadline", cv_futex_bitset),
        case("futex-requeue", "Linux ABI: FUTEX_CMP_REQUEUE wakes one waiter and moves the rest to another word", cv_futex_requeue),
    ]),
    category("rwlock-barrier", "rwlocks, barriers & once", &[
        case("readers-share", "Three threads hold one read lock at once", rw_readers_share),
        case("writer-excludes-readers", "While a thread holds the write lock, tryrdlock returns EBUSY and rdlock waits", rw_writer_excludes_readers),
        case("writer-excludes-writers", "While a thread holds the write lock, trywrlock returns EBUSY and wrlock waits", rw_writer_excludes_writers),
        case("reader-blocks-writer", "While a thread holds a read lock, trywrlock returns EBUSY and wrlock waits", rw_reader_blocks_writer),
        case("read-recursive", "A thread holding two read locks keeps writers out until it releases both", rw_read_recursive),
        case("try-free", "pthread_rwlock_tryrdlock and trywrlock take a free lock", rw_try_free),
        case("writer-preference", "With SCHED_FIFO threads, a reader cannot take the lock while a writer of equal priority waits", rw_writer_preference),
        case("timedwrlock", "pthread_rwlock_timedwrlock returns ETIMEDOUT at its deadline while a reader holds the lock", rw_timedwrlock),
        case("timedrdlock", "pthread_rwlock_timedrdlock returns ETIMEDOUT at its deadline while a writer holds the lock", rw_timedrdlock),
        case("contention-cpus", "Readers on every processor never see a writer's update half done", rw_contention),
        case("barrier-serial", "pthread_barrier_wait gives PTHREAD_BARRIER_SERIAL_THREAD to exactly one of four threads in each of 100 cycles", br_serial_once),
        case("barrier-holds", "No thread passes a barrier of four until the fourth arrives", br_holds),
        case("barrier-cpus", "Threads on every processor keep in step through 2000 barrier cycles", br_cpus),
        case("barrier-einval", "pthread_barrier_init with a count of 0 returns EINVAL", br_init_einval),
        case("barrier-one", "Every wait on a barrier of one returns PTHREAD_BARRIER_SERIAL_THREAD", br_one),
        case("once-race", "Eight threads racing into pthread_once run the routine once, and none returns before it finishes", on_once_race),
        case("once-again", "pthread_once on a used control runs no routine", on_once_again),
    ]),
    category("tls", "thread-local storage", &[
        case("key-distinct", "pthread_key_create gives distinct keys, each starting NULL and holding its own value", tls_key_distinct),
        case("set-get", "pthread_getspecific returns what pthread_setspecific set", tls_set_get),
        case("per-thread", "Each thread has its own value for a key, starting NULL", tls_per_thread),
        case("destructor-runs", "A key's destructor runs once in the exiting thread with that thread's value", tls_destructor_runs),
        case("destructor-null", "No destructor runs for a key whose value is NULL", tls_destructor_null),
        case("destructor-cleared", "Inside a destructor the key's value is already NULL", tls_destructor_cleared),
        case("destructor-rounds", "A value a destructor sets for another key is destroyed in a later round", tls_destructor_rounds),
        case("destructor-iterations", "A destructor that keeps setting its value runs at least PTHREAD_DESTRUCTOR_ITERATIONS times", tls_destructor_iterations),
        case("destructor-exit", "Destructors run for a thread that ends with pthread_exit", tls_destructor_exit),
        case("key-delete", "A deleted key's destructor never runs, while another key's does", tls_key_delete),
        case("keys-max", "PTHREAD_KEYS_MAX keys can be created and each keeps its own value", tls_keys_max),
        case("errno-per-thread", "errno is per thread: a failing call in one thread leaves another's alone", tls_errno),
        case("thread-pointer", "Linux ABI: every thread has a thread pointer (TPIDR_EL0, the FS base) of its own", tls_thread_pointer),
        case("compiler-initial", "Compiler TLS starts at its initial values in the main thread and every new thread", tls_compiler_initial),
        case("compiler-isolated", "Compiler TLS written by four threads stays each thread's own", tls_compiler_isolated),
    ]),
    category("signals", "per-thread signals", &[
        case("pthread-kill", "pthread_kill runs the handler in the target thread and only there", sg_pthread_kill),
        case("pthread-kill-zero", "pthread_kill with signal 0 of a live thread returns 0", sg_pthread_kill_zero),
        case("pthread-kill-einval", "pthread_kill with an invalid signal returns EINVAL", sg_pthread_kill_einval),
        case("sigmask-per-thread", "pthread_sigmask changes only the calling thread's mask", sg_sigmask_per_thread),
        case("sigmask-inherited", "A new thread starts with its creator's signal mask", sg_sigmask_inherited),
        case("sigmask-einval", "pthread_sigmask with an invalid how returns EINVAL", sg_sigmask_einval),
        case("pending-per-thread", "A blocked signal sent to one thread is pending there alone and delivered there when unblocked", sg_pending_per_thread),
        case("process-to-unblocked", "A signal sent to the process runs in the one thread that does not block it", sg_process_to_unblocked),
        case("process-pending", "A process signal every thread blocks waits until a thread unblocks it, and runs there", sg_process_pending),
        case("sigwait-process", "sigwait in a dedicated thread takes a signal sent to the process, with no handler run", sg_sigwait_process),
        case("sigwait-thread", "sigwait takes a signal pthread_kill sent to its thread", sg_sigwait_thread),
        case("sigwait-pending", "sigwait returns at once for a signal already pending and clears it", sg_sigwait_pending),
        case("raise-thread", "raise in a thread runs the handler in that thread", sg_raise_thread),
        case("default-whole-process", "A signal whose default action is to terminate ends the whole process even when sent to one thread", sg_default_whole_process),
        case("sigkill-threads", "SIGKILL ends a process with four running threads", sg_sigkill_threads),
        case("tgkill", "Linux ABI: tgkill runs the handler in the named thread", sg_tgkill),
        case("rt-sigprocmask", "Linux ABI: rt_sigprocmask in one thread leaves another thread's mask unchanged", sg_rt_sigprocmask),
    ]),
    category("scheduling", "thread scheduling parameters", &[
        case("getschedparam-self", "Linux policy: a thread started normally reports SCHED_OTHER with priority 0", sc_getschedparam_self),
        case("priority-range", "SCHED_FIFO and SCHED_RR each have at least 32 priorities", sc_priority_range),
        case("priority-einval", "sched_get_priority_max of an invalid policy fails with EINVAL", sc_priority_einval),
        case("setschedparam-fifo", "pthread_setschedparam to SCHED_FIFO reads back through pthread_getschedparam", sc_setschedparam_fifo),
        case("setschedparam-rr", "pthread_setschedparam to SCHED_RR reads back and sched_rr_get_interval gives a quantum", sc_setschedparam_rr),
        case("setschedparam-thread", "pthread_setschedparam of another thread changes what that thread reports", sc_setschedparam_thread),
        case("setschedparam-eperm", "Linux policy: an unprivileged thread's pthread_setschedparam to SCHED_FIFO returns EPERM when RLIMIT_RTPRIO is 0", sc_setschedparam_eperm),
        case("setschedprio", "pthread_setschedprio changes the thread's priority", sc_setschedprio),
        case("inherit-sched", "A thread created with PTHREAD_INHERIT_SCHED takes its creator's SCHED_FIFO priority", sc_inherit_sched),
        case("explicit-sched", "A thread created with PTHREAD_EXPLICIT_SCHED runs with the attribute's SCHED_RR priority", sc_explicit_sched),
        case("inheritsched-default", "Linux policy: a new thread attribute object is PTHREAD_INHERIT_SCHED", sc_inheritsched_default),
        case("yield-other", "Linux policy: two SCHED_OTHER threads on one processor hand it over 2000 times with sched_yield in 500 ms", sc_yield_other),
        case("yield-fifo", "Two SCHED_FIFO threads of one priority on one processor hand it over 2000 times with sched_yield", sc_yield_fifo),
        case("fifo-no-preempt", "A SCHED_FIFO thread waking does not preempt an equal-priority thread computing on its processor", sc_fifo_no_preempt),
        case("fifo-preempt", "A higher-priority SCHED_FIFO thread waking preempts a lower-priority one at once", sc_fifo_preempt),
        case("kernel-getscheduler", "Linux ABI: sched_getscheduler(0) reports SCHED_OTHER", sc_kernel_getscheduler),
        case("kernel-setscheduler", "Linux ABI: sched_setscheduler and sched_getparam set and read a thread's SCHED_FIFO priority", sc_kernel_setscheduler),
    ]),
]);

fn main() { SUITE.run() }
