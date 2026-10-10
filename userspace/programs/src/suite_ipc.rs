//! IPC: named pipes (FIFOs), POSIX message queues, POSIX semaphores, POSIX shared memory
//! and System V message queues, semaphore sets and shared memory segments, as POSIX
//! specifies them.
//!
//! IPC is measured through the interface a portable C program uses. A function Breenix's
//! own C library (libs/libbreenix-libc) defines is called there: build.rs reads libc.a's
//! symbol index and sets `libc_has = "<name>"` for each one it has. A function the library
//! lacks that Linux implements as a system call (mkfifo through mknodat, the mq_* calls,
//! msgget and the other XSI calls) is made by its Linux number, as a C library makes it,
//! so a kernel that lacks it fails the case with ENOSYS. A function a C library builds
//! from other calls (sem_*, shm_open, shm_unlink, ftok) fails each case that needs it with
//! `the C library has no <name>` while the library lacks it. The suite uses raw system
//! calls otherwise only to arrange a case: forking and reaping, signals, clocks, switching
//! user and pinning to a processor.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! Processes a case starts report through a shared page and their exit status, and every
//! wait on one is bounded and stops 1.5 seconds before the case's deadline. A process that
//! must be blocked before it is released is observed through /proc/<pid>/status. Every
//! name and key a case creates is removed when the case ends, by the process that made it.
//!
//! Cases that need several processors read the count from /proc/cpuinfo, skip below two
//! and start one process per processor, at most four. Each pins itself to its own
//! processor and the processes show they run at once before they measure: a thousand
//! rounds of a spinning barrier within 750 ms, which processes taking turns on one
//! processor cannot make.
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{self as signal, Sigaction};
use libbreenix::suite::{case, case_ms_left, category, check, fail, skip, suite, value, wait_for, CaseError, CaseResult, Suite};
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use std::cell::UnsafeCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU8, Ordering::Relaxed, Ordering::SeqCst};

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const CLOSE: u64 = 3;
    pub const MMAP: u64 = 9;
    pub const RT_SIGPROCMASK: u64 = 14;
    pub const SHMGET: u64 = 29;
    pub const SHMAT: u64 = 30;
    pub const SHMCTL: u64 = 31;
    pub const NANOSLEEP: u64 = 35;
    pub const GETPID: u64 = 39;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const SEMGET: u64 = 64;
    pub const SEMOP: u64 = 65;
    pub const SEMCTL: u64 = 66;
    pub const SHMDT: u64 = 67;
    pub const MSGGET: u64 = 68;
    pub const MSGSND: u64 = 69;
    pub const MSGRCV: u64 = 70;
    pub const MSGCTL: u64 = 71;
    pub const SETUID: u64 = 105;
    pub const SETGID: u64 = 106;
    pub const SETGROUPS: u64 = 116;
    pub const RT_SIGTIMEDWAIT: u64 = 128;
    pub const SCHED_SETAFFINITY: u64 = 203;
    pub const SEMTIMEDOP: u64 = 220;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231;
    pub const MQ_OPEN: u64 = 240;
    pub const MQ_UNLINK: u64 = 241;
    pub const MQ_TIMEDSEND: u64 = 242;
    pub const MQ_TIMEDRECEIVE: u64 = 243;
    pub const MQ_NOTIFY: u64 = 244;
    pub const MQ_GETSETATTR: u64 = 245;
    pub const MKNODAT: u64 = 259;
    pub const GETCPU: u64 = 309;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const CLOSE: u64 = 57;
    pub const MMAP: u64 = 222;
    pub const RT_SIGPROCMASK: u64 = 135;
    pub const SHMGET: u64 = 194;
    pub const SHMAT: u64 = 196;
    pub const SHMCTL: u64 = 195;
    pub const NANOSLEEP: u64 = 101;
    pub const GETPID: u64 = 172;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const SEMGET: u64 = 190;
    pub const SEMOP: u64 = 193;
    pub const SEMCTL: u64 = 191;
    pub const SHMDT: u64 = 197;
    pub const MSGGET: u64 = 186;
    pub const MSGSND: u64 = 189;
    pub const MSGRCV: u64 = 188;
    pub const MSGCTL: u64 = 187;
    pub const SETUID: u64 = 146;
    pub const SETGID: u64 = 144;
    pub const SETGROUPS: u64 = 159;
    pub const RT_SIGTIMEDWAIT: u64 = 137;
    pub const SCHED_SETAFFINITY: u64 = 122;
    pub const SEMTIMEDOP: u64 = 192;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const EXIT_GROUP: u64 = 94;
    pub const MQ_OPEN: u64 = 180;
    pub const MQ_UNLINK: u64 = 181;
    pub const MQ_TIMEDSEND: u64 = 182;
    pub const MQ_TIMEDRECEIVE: u64 = 183;
    pub const MQ_NOTIFY: u64 = 184;
    pub const MQ_GETSETATTR: u64 = 185;
    pub const MKNODAT: u64 = 33;
    pub const GETCPU: u64 = 168;
}

const ENOENT: i64 = 2;
const EINTR: i64 = 4;
const ENXIO: i64 = 6;
const E2BIG: i64 = 7;
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const EACCES: i64 = 13;
const EBUSY: i64 = 16;
const EEXIST: i64 = 17;
const EINVAL: i64 = 22;
const EFBIG: i64 = 27;
const ESPIPE: i64 = 29;
const EPIPE: i64 = 32;
const ERANGE: i64 = 34;
const ENOMSG: i64 = 42;
const EIDRM: i64 = 43;
const EMSGSIZE: i64 = 90;
const ETIMEDOUT: i64 = 110;

const O_RDONLY: i32 = 0;
const O_WRONLY: i32 = 1;
const O_RDWR: i32 = 2;
const O_CREAT: i32 = 0o100;
const O_EXCL: i32 = 0o200;
const O_TRUNC: i32 = 0o1000;
const O_NONBLOCK: i32 = 0o4000;
const AT_FDCWD: i64 = -100;
const S_IFMT: u32 = 0o170000;
const S_IFIFO: u32 = 0o010000;
const SEEK_SET: i32 = 0;

const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const POLLHUP: i16 = 0x10;

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;
const MAP_SHARED_ANON: u64 = 0x21;

const SIGKILL: i32 = 9;
const SIGUSR1: i32 = 10;
const SIGSEGV: i32 = 11;
const SIGPIPE: i32 = 13;
const SIG_BLOCK: u64 = 0;
const SI_MESGQ: i32 = -3;
const SIGEV_SIGNAL: i32 = 0;
const WNOHANG: u64 = 1;

const IPC_PRIVATE: i32 = 0;
const IPC_CREAT: i32 = 0o1000;
const IPC_EXCL: i32 = 0o2000;
const IPC_NOWAIT: i32 = 0o4000;
const IPC_RMID: i32 = 0;
const IPC_SET: i32 = 1;
const IPC_STAT: i32 = 2;
const MSG_NOERROR: i32 = 0o10000;
const SEM_UNDO: i16 = 0x1000;
const GETPID: i32 = 11;
const GETVAL: i32 = 12;
const GETALL: i32 = 13;
const GETNCNT: i32 = 14;
const GETZCNT: i32 = 15;
const SETVAL: i32 = 16;
const SETALL: i32 = 17;
const SHM_RDONLY: i32 = 0o10000;

const SC_MQ_PRIO_MAX: i32 = 28;
const SC_SEM_VALUE_MAX: i32 = 33;
/// The POSIX minimums of MQ_PRIO_MAX and SEM_VALUE_MAX.
const POSIX_MQ_PRIO_MAX: i64 = 32;
const POSIX_SEM_VALUE_MAX: i64 = 32767;
/// The POSIX minimum of PIPE_BUF: writes of this many bytes or fewer are atomic on any
/// conforming system.
const POSIX_PIPE_BUF: usize = 512;

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const NS: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

/// The timer tick: 200 Hz on x86-64 and 1000 Hz on ARM64. A timed wait may end late by
/// at most two ticks and 20 ms, as the time suite holds its sleeps to.
#[cfg(target_arch = "x86_64")]
const TICK_MS: i64 = 5;
#[cfg(target_arch = "aarch64")]
const TICK_MS: i64 = 1;
const LATE_MS: i64 = 2 * TICK_MS + 20;
/// How soon a process blocked on IPC must come back once it is released. This is a
/// liveness bound, not a performance target.
const WAKE_MS: i64 = 100;
/// Time a case keeps to report and clean up after its last bounded wait.
const CLEANUP_MS: u64 = 1500;
/// A user ID with no privileges.
const USER_A: u32 = 4242;

/// Where struct stat keeps st_mode, st_uid and st_size on each architecture.
#[cfg(target_arch = "x86_64")]
const ST_MODE: usize = 24;
#[cfg(target_arch = "aarch64")]
const ST_MODE: usize = 16;
const ST_SIZE: usize = 48;

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

/// Declare C library functions that Linux implements as system calls: each calls the
/// library's function when libc.a defines it, and otherwise makes the system call by its
/// Linux number with the arguments given, as a C library makes it. Either way the result
/// is the value, or `-errno`.
macro_rules! c_or_sys {
    ($( $sym:literal fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $nr:expr, [$($sa:expr),*]; )*) => { $(
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
            Ok(sc($nr, &[$($sa as u64),*]))
        }
    )* };
}

type Ts = [i64; 2];

/// The Linux ABI's struct mq_attr.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MqAttr { flags: i64, maxmsg: i64, msgsize: i64, curmsgs: i64, _reserved: [i64; 4] }

/// The Linux ABI's struct sigevent: the value, the signal, how to notify, and padding to
/// 64 bytes.
#[repr(C)]
struct SigEvent { value: u64, signo: i32, notify: i32, _pad: [u64; 6] }

/// The Linux ABI's struct sembuf.
#[repr(C)]
#[derive(Clone, Copy)]
struct SemBuf { num: u16, op: i16, flg: i16 }

c_fn! {
    "open" fn open(path: *const u8, flags: i32, mode: u32) -> i32;
    "close" fn close(fd: i32) -> i32;
    "read" fn read(fd: i32, buf: *mut u8, len: usize) -> isize;
    "write" fn write(fd: i32, buf: *const u8, len: usize) -> isize;
    "unlink" fn unlink(path: *const u8) -> i32;
    "stat" fn stat(path: *const u8, buf: *mut u8) -> i32;
    "fstat" fn fstat(fd: i32, buf: *mut u8) -> i32;
    "lseek" fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    "ftruncate" fn ftruncate(fd: i32, len: i64) -> i32;
    "mmap" fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    "munmap" fn munmap(addr: *mut u8, len: usize) -> i32;
    "umask" fn umask(mask: u32) -> u32;
    "poll" fn poll(fds: *mut u8, nfds: u64, timeout: i32) -> i32;
    "select" fn select(nfds: i32, r: *mut u8, w: *mut u8, e: *mut u8, timeout: *mut u8) -> i32;

    "sem_init" fn sem_init(sem: *mut u8, pshared: i32, value: u32) -> i32;
    "sem_destroy" fn sem_destroy(sem: *mut u8) -> i32;
    "sem_close" fn sem_close(sem: *mut u8) -> i32;
    "sem_unlink" fn sem_unlink(name: *const u8) -> i32;
    "sem_wait" fn sem_wait(sem: *mut u8) -> i32;
    "sem_trywait" fn sem_trywait(sem: *mut u8) -> i32;
    "sem_timedwait" fn sem_timedwait(sem: *mut u8, abstime: *const Ts) -> i32;
    "sem_post" fn sem_post(sem: *mut u8) -> i32;
    "sem_getvalue" fn sem_getvalue(sem: *mut u8, value: *mut i32) -> i32;
    "shm_open" fn shm_open(name: *const u8, oflag: i32, mode: u32) -> i32;
    "shm_unlink" fn shm_unlink(name: *const u8) -> i32;
    "ftok" fn ftok(path: *const u8, id: i32) -> i32;
}

c_or_sys! {
    "mkfifo" fn mkfifo(path: *const u8, mode: u32) -> i32 = nr::MKNODAT, [AT_FDCWD, path, S_IFIFO | mode, 0];

    "mq_close" fn mq_close(mqd: i32) -> i32 = nr::CLOSE, [mqd];
    "mq_send" fn mq_send(mqd: i32, msg: *const u8, len: usize, prio: u32) -> i32 = nr::MQ_TIMEDSEND, [mqd, msg, len, prio, 0];
    "mq_receive" fn mq_receive(mqd: i32, msg: *mut u8, len: usize, prio: *mut u32) -> isize = nr::MQ_TIMEDRECEIVE, [mqd, msg, len, prio, 0];
    "mq_timedsend" fn mq_timedsend(mqd: i32, msg: *const u8, len: usize, prio: u32, abstime: *const Ts) -> i32 = nr::MQ_TIMEDSEND, [mqd, msg, len, prio, abstime];
    "mq_timedreceive" fn mq_timedreceive(mqd: i32, msg: *mut u8, len: usize, prio: *mut u32, abstime: *const Ts) -> isize = nr::MQ_TIMEDRECEIVE, [mqd, msg, len, prio, abstime];
    "mq_notify" fn mq_notify(mqd: i32, ev: *const SigEvent) -> i32 = nr::MQ_NOTIFY, [mqd, ev];
    "mq_getattr" fn mq_getattr(mqd: i32, attr: *mut MqAttr) -> i32 = nr::MQ_GETSETATTR, [mqd, 0, attr];
    "mq_setattr" fn mq_setattr(mqd: i32, new: *const MqAttr, old: *mut MqAttr) -> i32 = nr::MQ_GETSETATTR, [mqd, new, old];

    "msgget" fn msgget(key: i32, flags: i32) -> i32 = nr::MSGGET, [key, flags];
    "msgsnd" fn msgsnd(id: i32, msg: *const u8, len: usize, flags: i32) -> i32 = nr::MSGSND, [id, msg, len, flags];
    "msgrcv" fn msgrcv(id: i32, msg: *mut u8, len: usize, kind: i64, flags: i32) -> isize = nr::MSGRCV, [id, msg, len, kind, flags];
    "msgctl" fn msgctl(id: i32, cmd: i32, buf: *mut u8) -> i32 = nr::MSGCTL, [id, cmd, buf];
    "semget" fn semget(key: i32, nsems: i32, flags: i32) -> i32 = nr::SEMGET, [key, nsems, flags];
    "semop" fn semop(id: i32, ops: *const SemBuf, n: usize) -> i32 = nr::SEMOP, [id, ops, n];
    "semtimedop" fn semtimedop(id: i32, ops: *const SemBuf, n: usize, timeout: *const Ts) -> i32 = nr::SEMTIMEDOP, [id, ops, n, timeout];
    "semctl" fn semctl(id: i32, num: i32, cmd: i32, arg: u64) -> i32 = nr::SEMCTL, [id, num, cmd, arg];
    "shmget" fn shmget(key: i32, size: usize, flags: i32) -> i32 = nr::SHMGET, [key, size, flags];
    "shmat" fn shmat(id: i32, addr: *const u8, flags: i32) -> *mut u8 = nr::SHMAT, [id, addr, flags];
    "shmdt" fn shmdt(addr: *const u8) -> i32 = nr::SHMDT, [addr];
    "shmctl" fn shmctl(id: i32, cmd: i32, buf: *mut u8) -> i32 = nr::SHMCTL, [id, cmd, buf];
}

/// mq_open. A C library passes the name to the kernel without its leading slash, and
/// refuses a name that does not begin with one.
#[cfg(libc_has = "mq_open")]
fn mq_open(name: &[u8], oflag: i32, mode: u32, attr: *const MqAttr) -> Result<i64, CaseError> {
    extern "C" {
        #[link_name = "mq_open"]
        fn f(name: *const u8, oflag: i32, mode: u32, attr: *const MqAttr) -> i32;
    }
    // SAFETY: name is NUL-terminated and attr is null or a live MqAttr.
    let r = i64::from(unsafe { f(name.as_ptr(), oflag, mode, attr) });
    Ok(if r == -1 { -errno() } else { r })
}
#[cfg(not(libc_has = "mq_open"))]
fn mq_open(name: &[u8], oflag: i32, mode: u32, attr: *const MqAttr) -> Result<i64, CaseError> {
    if name.first() != Some(&b'/') { return Ok(-EINVAL); }
    Ok(sc(nr::MQ_OPEN, &[name[1..].as_ptr() as u64, oflag as u64, mode as u64, attr as u64]))
}

/// mq_unlink, made as mq_open is.
#[cfg(libc_has = "mq_unlink")]
fn mq_unlink(name: &[u8]) -> Result<i64, CaseError> {
    extern "C" {
        #[link_name = "mq_unlink"]
        fn f(name: *const u8) -> i32;
    }
    // SAFETY: name is NUL-terminated.
    let r = i64::from(unsafe { f(name.as_ptr()) });
    Ok(if r == -1 { -errno() } else { r })
}
#[cfg(not(libc_has = "mq_unlink"))]
fn mq_unlink(name: &[u8]) -> Result<i64, CaseError> {
    if name.first() != Some(&b'/') { return Ok(-EINVAL); }
    Ok(sc(nr::MQ_UNLINK, &[name[1..].as_ptr() as u64]))
}

/// sem_open, which returns SEM_FAILED, a null pointer, on failure: the semaphore's address
/// or `-errno`.
#[cfg(libc_has = "sem_open")]
fn sem_open(name: &[u8], oflag: i32, mode: u32, value: u32) -> Result<i64, CaseError> {
    extern "C" {
        #[link_name = "sem_open"]
        fn f(name: *const u8, oflag: i32, mode: u32, value: u32) -> *mut u8;
    }
    // SAFETY: name is NUL-terminated.
    let r = unsafe { f(name.as_ptr(), oflag, mode, value) } as i64;
    Ok(if r == 0 { -errno() } else { r })
}
#[cfg(not(libc_has = "sem_open"))]
fn sem_open(name: &[u8], oflag: i32, mode: u32, value: u32) -> Result<i64, CaseError> {
    let _ = (name, oflag, mode, value);
    Err(CaseError::Fail("the C library has no sem_open".into()))
}

/// sysconf, which returns -1 without setting errno for a value it does not report.
#[cfg(libc_has = "sysconf")]
fn sysconf(name: i32) -> Result<i64, CaseError> {
    extern "C" {
        #[link_name = "sysconf"]
        fn f(name: i32) -> i64;
    }
    // SAFETY: sysconf takes no pointers.
    Ok(unsafe { f(name) })
}
#[cfg(not(libc_has = "sysconf"))]
fn sysconf(name: i32) -> Result<i64, CaseError> {
    let _ = name;
    Err(CaseError::Fail("the C library has no sysconf".into()))
}

// ---------------------------------------------------------------------------
// Results, time and waiting.

fn errname(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 6 => "ENXIO", 7 => "E2BIG", 9 => "EBADF",
        11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES", 14 => "EFAULT", 16 => "EBUSY", 17 => "EEXIST",
        22 => "EINVAL", 24 => "EMFILE", 27 => "EFBIG", 28 => "ENOSPC", 29 => "ESPIPE", 32 => "EPIPE",
        34 => "ERANGE", 38 => "ENOSYS", 42 => "ENOMSG", 43 => "EIDRM", 90 => "EMSGSIZE", 95 => "EOPNOTSUPP",
        110 => "ETIMEDOUT",
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

/// A call that must fail with one of `errnos`.
fn want_any(what: &str, ret: i64, errnos: &[i64]) -> CaseResult {
    if errnos.iter().any(|&e| ret == -e) { return Ok(()); }
    let names: Vec<String> = errnos.iter().map(|&e| errname(e)).collect();
    fail(format!("{what} returned {}, expected {}", shown(ret), names.join(" or ")))
}

fn ts(ns: i64) -> Ts { [ns.div_euclid(NS), ns.rem_euclid(NS)] }
fn clock_ns(clock: i32) -> i64 {
    let mut t: Ts = [0; 2];
    if sc(nr::CLOCK_GETTIME, &[clock as u64, t.as_mut_ptr() as u64]) != 0 { return 0; }
    t[0] * NS + t[1]
}
fn mono() -> i64 { clock_ns(CLOCK_MONOTONIC) }
fn rt() -> i64 { clock_ns(CLOCK_REALTIME) }
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

/// How late a timed wait that had to run to `deadline_ns` ended, `end_ns`, on the same
/// clock: reported, and checked never early and late by at most LATE_MS.
fn on_time(what: &str, end_ns: i64, deadline_ns: i64) -> CaseResult {
    let late_us = (end_ns - deadline_ns) / 1000;
    value("late", late_us, "us", Some((0, LATE_MS * 1000)));
    check(end_ns >= deadline_ns, &format!("{what} returned {} us before its deadline", -late_us))?;
    check(late_us <= LATE_MS * 1000, &format!("{what} returned {late_us} us after its deadline, more than {LATE_MS} ms"))
}

/// How soon a blocked process came back once it was released, `woke_ns` against
/// `released_ns`: reported, and checked not before the release and within WAKE_MS.
fn woken(what: &str, woke_ns: i64, released_ns: i64) -> CaseResult {
    let us = (woke_ns - released_ns) / 1000;
    value("wake", us, "us", Some((0, WAKE_MS * 1000)));
    check(woke_ns >= released_ns, &format!("{what} returned {} us before it was released", -us))?;
    check(us <= WAKE_MS * 1000, &format!("{what} returned {us} us after it was released, more than {WAKE_MS} ms"))
}

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }

/// End the process at once, running no destructors, so a child never removes a name
/// the case made.
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

/// A path under /tmp for this case's process.
fn tmp_path(tag: &str) -> String { format!("/tmp/ipc-{}-{tag}", pid()) }
/// A POSIX IPC name for this case's process.
fn ipc_name(tag: &str) -> String { format!("/ipc-{}-{tag}", pid()) }
/// A System V key for this case's process: unique among processes running at once.
fn ipc_key(n: i32) -> i32 { 0x4950_0000 | ((pid() & 0xfff) << 4) | (n & 0xf) }

/// Processors online, from the `processor` lines of /proc/cpuinfo. A census that cannot
/// be read or counts none fails the case rather than passing for one processor.
fn processors() -> Result<usize, CaseError> {
    let info = std::fs::read_to_string("/proc/cpuinfo").map_err(|e| format!("reading /proc/cpuinfo failed: {e}"))?;
    let n = info.lines().filter(|line| line.starts_with("processor")).count();
    if n == 0 { return err("/proc/cpuinfo lists no processor"); }
    Ok(n)
}

/// Become user and group `id` with no supplementary groups, in a child.
fn become_user(id: u32) -> CaseResult {
    zero("setgroups(0)", sc(nr::SETGROUPS, &[0, 0]))?;
    zero(&format!("setgid({id})"), sc(nr::SETGID, &[id as u64]))?;
    zero(&format!("setuid({id})"), sc(nr::SETUID, &[id as u64]))
}

/// Set the file mode creation mask, for cases that check the modes of what they create.
fn set_umask(mask: u32) -> CaseResult { umask(mask).map(|_| ()) }

/// A `struct stat`, large and aligned enough for the Linux ABI's.
type StatBuf = [u64; 32];
fn st_mode(st: &StatBuf) -> u32 { (st[ST_MODE / 8] >> (8 * (ST_MODE % 8))) as u32 }
fn st_size(st: &StatBuf) -> i64 { st[ST_SIZE / 8] as i64 }

fn fstat_of(fd: i64) -> Result<StatBuf, CaseError> {
    let mut st: StatBuf = [0; 32];
    ok("fstat", fstat(fd as i32, st.as_mut_ptr() as *mut u8)?)?;
    Ok(st)
}

// ---------------------------------------------------------------------------
// Names and keys a case makes, removed when the case ends.

enum Kind { File, Mq, Sem, Shm, MsgQueue, SemSet, Segment }

/// A name or System V identifier the case created. Dropping it removes it, in the process
/// that made it only: a child that inherited a copy never removes it.
struct Made { kind: Kind, name: Vec<u8>, id: i32, owner: i32 }

impl Made {
    fn name(kind: Kind, name: &str) -> Made { Made { kind, name: c(name), id: -1, owner: pid() } }
    fn id(kind: Kind, id: i64) -> Made { Made { kind, name: Vec::new(), id: id as i32, owner: pid() } }
    fn path(&self) -> *const u8 { self.name.as_ptr() }
    fn text(&self) -> String { String::from_utf8_lossy(&self.name[..self.name.len() - 1]).into_owned() }
}

impl Drop for Made {
    fn drop(&mut self) {
        if pid() != self.owner { return; }
        let _ = match self.kind {
            Kind::File => unlink(self.path()),
            Kind::Mq => mq_unlink(&self.name),
            Kind::Sem => sem_unlink(self.path()),
            Kind::Shm => shm_unlink(self.path()),
            Kind::MsgQueue => msgctl(self.id, IPC_RMID, null_mut()),
            Kind::SemSet => semctl(self.id, 0, IPC_RMID, 0),
            Kind::Segment => shmctl(self.id, IPC_RMID, null_mut()),
        };
    }
}

// ---------------------------------------------------------------------------
// Shared pages and the processes a case starts.

/// A page the case shares with the processes it starts: words for results and times,
/// storage for unnamed semaphores, and a message.
#[repr(C, align(64))]
struct Page {
    w: [AtomicI64; 32],
    sems: [UnsafeCell<[u64; 8]>; 4],
    len: AtomicU32,
    text: [AtomicU8; 240],
}

impl Page {
    fn get(&self, i: usize) -> i64 { self.w[i].load(SeqCst) }
    fn set(&self, i: usize, v: i64) { self.w[i].store(v, SeqCst) }
    /// The address of unnamed semaphore slot `i`: 64 zeroed bytes, more than the Linux
    /// ABI's 32-byte sem_t.
    fn sem(&self, i: usize) -> *mut u8 { self.sems[i].get() as *mut u8 }
    fn say(&self, msg: &str) {
        let n = msg.len().min(self.text.len());
        for (cell, &b) in self.text.iter().zip(&msg.as_bytes()[..n]) { cell.store(b, Relaxed); }
        self.len.store(n as u32, SeqCst);
    }
    fn said(&self) -> String {
        let n = (self.len.load(SeqCst) as usize).min(self.text.len());
        let bytes: Vec<u8> = self.text[..n].iter().map(|cell| cell.load(Relaxed)).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// A fresh zeroed page shared with every process the caller forks afterwards.
fn page() -> Result<&'static Page, CaseError> {
    let ret = sc(nr::MMAP, &[0, 4096, (PROT_READ | PROT_WRITE) as u64, MAP_SHARED_ANON, u64::MAX, 0]);
    if (-4095..0).contains(&ret) { return err(format!("mmap of a shared page failed with {}", errname(-ret))); }
    // SAFETY: a fresh zeroed shared page, large enough for a Page and never unmapped.
    Ok(unsafe { &*(ret as *const Page) })
}

/// A wait status in words.
fn status_text(status: i32) -> String {
    let sig = status & 0x7f;
    if sig == 0 { format!("exited with status {}", (status >> 8) & 0xff) } else { format!("was killed by signal {sig}") }
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

/// Whether every thread of `pid` is blocked, as /proc reports it.
fn is_parked(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .is_ok_and(|status| status.lines().any(|line| line == "State:\tBlocked"))
}

impl Child {
    /// Reap the process if it has ended.
    fn reap(&mut self) -> Option<i32> {
        if self.status.is_none() {
            let mut status = 0i32;
            let r = sc(nr::WAIT4, &[self.pid as u64, &mut status as *mut i32 as u64, WNOHANG, 0]);
            if r == self.pid as i64 { self.status = Some(status); }
        }
        self.status
    }

    /// The process's wait status once it ends, within `ms`.
    fn wait(&mut self, ms: u64, who: &str) -> Result<i32, CaseError> {
        let ms = bounded(ms, CLEANUP_MS);
        if until(ms, || self.reap().is_some()) { return Ok(self.status.unwrap_or(0)); }
        err(format!("{who} did not finish within {ms} ms"))
    }

    /// What an ended process's status says: a pass for status 0, otherwise its message.
    fn outcome(&self, status: i32, who: &str) -> CaseResult {
        if status == 0 { return Ok(()); }
        let said = self.note.said();
        if status == 1 << 8 && !said.is_empty() { return fail(format!("{who}: {said}")); }
        fail(format!("{who} {}", status_text(status)))
    }

    /// Wait up to `ms` for the process to end, passing if it passed.
    fn finish(&mut self, ms: u64, who: &str) -> CaseResult {
        let status = self.wait(ms, who)?;
        self.outcome(status, who)
    }

    /// Wait up to 2 s for the process to block in the kernel. One that ends first fails
    /// the case with its own message, or for returning before it was released.
    fn blocks(&mut self, who: &str) -> CaseResult {
        let ms = bounded(2000, CLEANUP_MS);
        let start = now_ms();
        loop {
            if is_parked(self.pid) { return Ok(()); }
            if let Some(status) = self.reap() {
                self.outcome(status, who)?;
                return fail(format!("{who} returned before it was released"));
            }
            if now_ms().saturating_sub(start) >= ms { return fail(format!("{who} did not block within {ms} ms")); }
            nap();
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.status.is_some() || pid() != self.owner { return; }
        let _ = sc(nr::KILL, &[self.pid as u64, SIGKILL as u64]);
        let start = now_ms();
        while self.reap().is_none() && now_ms().saturating_sub(start) < 1000 { nap(); }
    }
}

/// Run `body` as another user in a process of its own, and pass if it passes.
fn as_user(id: u32, body: impl FnOnce() -> CaseResult) -> CaseResult {
    spawn(|| {
        become_user(id)?;
        body()
    })?
    .finish(3000, &format!("the process running as user {id}"))
}

/// Block `sig` in the calling thread.
fn block(sig: i32) -> CaseResult {
    let set: u64 = 1 << (sig - 1);
    zero("rt_sigprocmask(SIG_BLOCK)", sc(nr::RT_SIGPROCMASK, &[SIG_BLOCK, &set as *const u64 as u64, 0, 8]))
}

/// Wait up to `ms` for blocked signal `sig` with sigtimedwait: its siginfo, or None if
/// it did not come.
fn take_signal(sig: i32, ms: i64) -> Result<Option<[u8; 128]>, CaseError> {
    let set: u64 = 1 << (sig - 1);
    let mut info = [0u8; 128];
    let t = ts(ms * MS);
    let r = sc(nr::RT_SIGTIMEDWAIT, &[&set as *const u64 as u64, info.as_mut_ptr() as u64, t.as_ptr() as u64, 8]);
    if r == -EAGAIN { return Ok(None); }
    if r != sig as i64 { return err(format!("sigtimedwait for signal {sig} returned {}", shown(r))); }
    Ok(Some(info))
}

fn info_i32(info: &[u8; 128], at: usize) -> i32 { i32::from_ne_bytes(info[at..at + 4].try_into().unwrap_or([0; 4])) }
fn info_u64(info: &[u8; 128], at: usize) -> u64 { u64::from_ne_bytes(info[at..at + 8].try_into().unwrap_or([0; 8])) }

/// Set the action for `sig`.
fn set_action(sig: i32, action: &Sigaction) -> CaseResult {
    signal::sigaction(sig, Some(action), None).map_err(|e| CaseError::Fail(format!("sigaction({sig}) failed: {e}")))
}

/// The processor the caller runs on, from getcpu.
fn this_cpu() -> Option<u32> {
    let mut cpu = u32::MAX;
    if sc(nr::GETCPU, &[&mut cpu as *mut u32 as u64, 0, 0]) != 0 { return None; }
    Some(cpu)
}

/// Pin the calling process to processor `cpu` and wait up to a second for it to run there.
fn pin_to(cpu: usize) -> CaseResult {
    let mask = 1u64 << cpu;
    zero(&format!("sched_setaffinity to processor {cpu}"), sc(nr::SCHED_SETAFFINITY, &[0, 8, &mask as *const u64 as u64]))?;
    check(until(1000, || this_cpu() == Some(cpu as u32)), &format!("the process pinned to processor {cpu} did not run there within a second"))
}

/// Rounds of the spinning barrier processes make to show they run at once.
const ROUNDS: i64 = 1000;
const ROUNDS_MS: u64 = 750;

/// Make ROUNDS rounds of a spinning barrier with the `n` processes sharing word `slot`.
/// Each round needs every process to run, and processes taking turns on one processor
/// change turns no faster than the timer tick, so ROUNDS of them within ROUNDS_MS show
/// the `n` running at once.
fn barrier_rounds(page: &Page, slot: usize, n: i64) -> CaseResult {
    let start = now_ms();
    for r in 0..ROUNDS {
        page.w[slot].fetch_add(1, SeqCst);
        let mut spins = 0u64;
        while page.get(slot) < n * (r + 1) {
            core::hint::spin_loop();
            spins += 1;
            if spins % 4096 == 0 && now_ms().saturating_sub(start) >= ROUNDS_MS {
                return fail(format!("the {n} processes made only {r} of {ROUNDS} barrier rounds in {ROUNDS_MS} ms: they do not run at once"));
            }
        }
    }
    Ok(())
}

/// Read up to `want` bytes from nonblocking descriptor `fd`, polling between reads, until
/// end of file or `ms` pass.
fn read_all(fd: i64, want: usize, ms: u64) -> Result<Vec<u8>, CaseError> {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    let mut out = Vec::with_capacity(want);
    let mut buf = vec![0u8; 4096];
    while out.len() < want {
        let room = (want - out.len()).min(buf.len());
        let n = read(fd as i32, buf.as_mut_ptr(), room)?;
        if n == 0 { break; }
        if n > 0 { out.extend_from_slice(&buf[..n as usize]); continue; }
        if n != -EAGAIN { return err(format!("read failed with {} after {} bytes", errname(-n), out.len())); }
        if now_ms().saturating_sub(start) >= ms { return err(format!("only {} of {want} bytes arrived within {ms} ms", out.len())); }
        let mut pfd = PollFd { fd: fd as i32, events: POLLIN, revents: 0 };
        let _ = poll(&mut pfd as *mut PollFd as *mut u8, 1, 10)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Named pipes (FIFOs).

#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }

/// A FIFO this case made under /tmp, removed when the case ends.
fn new_fifo(tag: &str) -> Result<Made, CaseError> {
    let f = Made::name(Kind::File, &tmp_path(tag));
    ok(&format!("mkfifo of {}", f.text()), mkfifo(f.path(), 0o600)?)?;
    Ok(f)
}

fn open_fifo(f: &Made, flags: i32, what: &str) -> Result<i64, CaseError> {
    ok(&format!("open of the FIFO {what}"), open(f.path(), flags, 0)?)
}

/// A FIFO with a nonblocking reader and a nonblocking writer open.
fn fifo_pair(tag: &str) -> Result<(Made, i64, i64), CaseError> {
    let f = new_fifo(tag)?;
    let r = open_fifo(&f, O_RDONLY | O_NONBLOCK, "O_RDONLY|O_NONBLOCK")?;
    let w = open_fifo(&f, O_WRONLY | O_NONBLOCK, "O_WRONLY|O_NONBLOCK with a reader open")?;
    Ok((f, r, w))
}

fn put(fd: i64, bytes: &[u8]) -> Result<i64, CaseError> { write(fd as i32, bytes.as_ptr(), bytes.len()) }
fn put_all(fd: i64, bytes: &[u8], what: &str) -> CaseResult {
    let n = put(fd, bytes)?;
    check(n == bytes.len() as i64, &format!("{what}: write of {} bytes returned {}", bytes.len(), shown(n)))
}
fn get(fd: i64, len: usize) -> Result<(i64, Vec<u8>), CaseError> {
    let mut buf = vec![0u8; len];
    let n = read(fd as i32, buf.as_mut_ptr(), len)?;
    buf.truncate(n.max(0) as usize);
    Ok((n, buf))
}

fn poll_one(fd: i64, events: i16, timeout: i32) -> Result<(i64, i16), CaseError> {
    let mut pfd = PollFd { fd: fd as i32, events, revents: 0 };
    let n = poll(&mut pfd as *mut PollFd as *mut u8, 1, timeout)?;
    Ok((n, pfd.revents))
}

/// A select fd_set as the Linux ABI lays it out, with `fd` in it.
fn fd_set(fd: i64) -> [u64; 16] {
    let mut set = [0u64; 16];
    set[fd as usize / 64] |= 1 << (fd % 64);
    set
}
fn in_set(set: &[u64; 16], fd: i64) -> bool { set[fd as usize / 64] & (1 << (fd % 64)) != 0 }

/// select on one descriptor for reading (`write` false) or writing, with a timeout in
/// milliseconds: the count and whether the descriptor is in the returned set.
fn select_one(fd: i64, write: bool, ms: i64) -> Result<(i64, bool), CaseError> {
    let mut set = fd_set(fd);
    let mut tv: [i64; 2] = [ms / 1000, (ms % 1000) * 1000];
    let (r, w) = if write { (null_mut(), set.as_mut_ptr() as *mut u8) } else { (set.as_mut_ptr() as *mut u8, null_mut()) };
    let n = select(fd as i32 + 1, r, w, null_mut(), tv.as_mut_ptr() as *mut u8)?;
    Ok((n, n > 0 && in_set(&set, fd)))
}

fn fifo_mkfifo() -> CaseResult {
    set_umask(0o022)?;
    let f = Made::name(Kind::File, &tmp_path("fifo"));
    zero("mkfifo(0666)", mkfifo(f.path(), 0o666)?)?;
    let mut st: StatBuf = [0; 32];
    ok("stat of the FIFO", stat(f.path(), st.as_mut_ptr() as *mut u8)?)?;
    let mode = st_mode(&st);
    check(mode & S_IFMT == S_IFIFO, &format!("stat reports st_mode {mode:o}, not a FIFO"))?;
    check(mode & 0o7777 == 0o644, &format!("mkfifo(0666) under umask 022 made mode {:o}, expected 644", mode & 0o7777))
}

fn fifo_mkfifo_eexist() -> CaseResult {
    let f = new_fifo("fifo")?;
    want("a second mkfifo of the same path", mkfifo(f.path(), 0o600)?, EEXIST)
}

fn fifo_mkfifo_enoent() -> CaseResult {
    let path = c(&format!("{}/fifo", tmp_path("missing")));
    want("mkfifo in a missing directory", mkfifo(path.as_ptr(), 0o600)?, ENOENT)
}

/// One end of a FIFO opened with a blocking open must wait for the other: the child's
/// open blocks, a second child opens the other end, and the first child's open returns.
fn open_blocks(first: i32, second: i32, what: &str) -> CaseResult {
    let f = new_fifo("fifo")?;
    let pg = page()?;
    let mut opener = spawn(|| {
        let fd = open_fifo(&f, first, what)?;
        pg.set(0, mono());
        let _ = close(fd as i32)?;
        Ok(())
    })?;
    opener.blocks(&format!("the {what} open"))?;
    let mut other = spawn(|| {
        pg.set(1, mono());
        let fd = open_fifo(&f, second, "for the other end")?;
        sleep_ms(500);
        let _ = close(fd as i32)?;
        Ok(())
    })?;
    opener.finish(3000, &format!("the {what} open"))?;
    other.finish(3000, "the process opening the other end")?;
    woken(&format!("the blocking {what} open"), pg.get(0), pg.get(1))
}

fn fifo_open_read_blocks() -> CaseResult { open_blocks(O_RDONLY, O_WRONLY, "O_RDONLY") }
fn fifo_open_write_blocks() -> CaseResult { open_blocks(O_WRONLY, O_RDONLY, "O_WRONLY") }

fn fifo_open_read_nonblock() -> CaseResult {
    let f = new_fifo("fifo")?;
    let r = open(f.path(), O_RDONLY | O_NONBLOCK, 0)?;
    check(r >= 0, &format!("open(O_RDONLY|O_NONBLOCK) with no writer returned {}", shown(r)))
}

fn fifo_open_write_enxio() -> CaseResult {
    let f = new_fifo("fifo")?;
    want("open(O_WRONLY|O_NONBLOCK) with no reader", open(f.path(), O_WRONLY | O_NONBLOCK, 0)?, ENXIO)
}

fn fifo_open_write_nonblock() -> CaseResult {
    fifo_pair("fifo").map(|_| ())
}

fn fifo_fstat() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    for (fd, end) in [(r, "reader"), (w, "writer")] {
        let mode = st_mode(&fstat_of(fd)?);
        check(mode & S_IFMT == S_IFIFO, &format!("fstat of the {end} reports st_mode {mode:o}, not a FIFO"))?;
    }
    Ok(())
}

fn fifo_lseek_espipe() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    want("lseek on the reader", lseek(r as i32, 0, SEEK_SET)?, ESPIPE)?;
    want("lseek on the writer", lseek(w as i32, 0, SEEK_SET)?, ESPIPE)
}

/// The byte at offset `i` of a test stream.
fn pattern(i: usize) -> u8 { (i * 7 % 251) as u8 }

fn fifo_transfer() -> CaseResult {
    const LEN: usize = 256 * 1024;
    let f = new_fifo("fifo")?;
    let r = open_fifo(&f, O_RDONLY | O_NONBLOCK, "O_RDONLY|O_NONBLOCK")?;
    let pg = page()?;
    let mut writer = spawn(|| {
        let _ = close(r as i32)?;
        let w = open_fifo(&f, O_WRONLY, "O_WRONLY in the writer")?;
        pg.set(0, 1);
        let data: Vec<u8> = (0..LEN).map(pattern).collect();
        let mut at = 0;
        while at < LEN {
            let chunk = (LEN - at).min(8192);
            let n = write(w as i32, data[at..].as_ptr(), chunk)?;
            if n <= 0 { return err(format!("write at offset {at} returned {}", shown(n))); }
            at += n as usize;
        }
        Ok(())
    })?;
    check(until(3000, || pg.get(0) == 1 || writer.reap().is_some()), "the writer did not open the FIFO within 3 s")?;
    let got = read_all(r, LEN + 1, 6000)?;
    writer.finish(2000, "the writer")?;
    value("bytes", got.len() as i64, "", Some((LEN as i64, LEN as i64)));
    check(got.len() == LEN, &format!("the reader got {} bytes before end of file, expected {LEN}", got.len()))?;
    match got.iter().enumerate().find(|&(i, &b)| b != pattern(i)) {
        Some((i, &b)) => fail(format!("byte {i} read as {b}, expected {}", pattern(i))),
        None => Ok(()),
    }
}

fn fifo_partial_read() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    put_all(w, b"0123456789", "the writer")?;
    let (n, got) = get(r, 100)?;
    check(n == 10 && got == b"0123456789", &format!("a read of 100 bytes with 10 buffered returned {}", shown(n)))
}

fn fifo_read_blocks() -> CaseResult {
    let (f, _r, w) = fifo_pair("fifo")?;
    let pg = page()?;
    let mut reader = spawn(|| {
        let r = open_fifo(&f, O_RDONLY, "O_RDONLY in the reader")?;
        let (n, got) = get(r, 64)?;
        pg.set(0, mono());
        check(n == 5 && got == b"hello", &format!("the blocked read returned {}", shown(n)))
    })?;
    reader.blocks("the read of the empty FIFO")?;
    let t0 = mono();
    put_all(w, b"hello", "the writer")?;
    reader.finish(3000, "the reader")?;
    woken("the blocked read", pg.get(0), t0)
}

fn fifo_read_eagain() -> CaseResult {
    let (_f, r, _w) = fifo_pair("fifo")?;
    want("an O_NONBLOCK read of the empty FIFO with a writer", get(r, 16)?.0, EAGAIN)
}

fn fifo_eof_last_writer() -> CaseResult {
    let (f, r, w) = fifo_pair("fifo")?;
    let w2 = open_fifo(&f, O_WRONLY | O_NONBLOCK, "for a second writer")?;
    put_all(w, b"x", "the first writer")?;
    zero("close of the first writer", close(w as i32)?)?;
    let (n, _) = get(r, 16)?;
    check(n == 1, &format!("the read after the first writer closed returned {}, expected the 1 byte written", shown(n)))?;
    want("a read of the empty FIFO while the second writer is open", get(r, 16)?.0, EAGAIN)?;
    zero("close of the second writer", close(w2 as i32)?)?;
    let (n, _) = get(r, 16)?;
    check(n == 0, &format!("the read after the last writer closed returned {}, expected end of file", shown(n)))
}

fn fifo_eof_blocked_reader() -> CaseResult {
    let (f, _r, w) = fifo_pair("fifo")?;
    let pg = page()?;
    let mut reader = spawn(|| {
        let _ = close(w as i32)?;
        let r = open_fifo(&f, O_RDONLY, "O_RDONLY in the reader")?;
        let (n, _) = get(r, 64)?;
        pg.set(0, mono());
        check(n == 0, &format!("the blocked read returned {}, expected end of file", shown(n)))
    })?;
    reader.blocks("the read of the empty FIFO")?;
    let t0 = mono();
    zero("close of the last writer", close(w as i32)?)?;
    reader.finish(3000, "the reader")?;
    woken("the blocked read", pg.get(0), t0)
}

fn fifo_eof_no_writer() -> CaseResult {
    let f = new_fifo("fifo")?;
    let r = open_fifo(&f, O_RDONLY | O_NONBLOCK, "O_RDONLY|O_NONBLOCK")?;
    let (n, _) = get(r, 16)?;
    check(n == 0, &format!("a read with no writer returned {}, expected end of file", shown(n)))
}

fn fifo_sigpipe() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    zero("close of the only reader", close(r as i32)?)?;
    let mut writer = spawn(|| {
        set_action(SIGPIPE, &Sigaction::default_action())?;
        let n = put(w, b"x")?;
        err(format!("the write with no reader returned {}", shown(n)))
    })?;
    let status = writer.wait(3000, "the writer")?;
    check(status & 0x7f == SIGPIPE, &format!("the writer {}, expected to be killed by SIGPIPE", status_text(status)))
        .or_else(|e| writer.outcome(status, "the writer").and(Err(e)))
}

fn fifo_epipe() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    zero("close of the only reader", close(r as i32)?)?;
    set_action(SIGPIPE, &Sigaction::ignore())?;
    want("a write with no reader and SIGPIPE ignored", put(w, b"x")?, EPIPE)
}

/// Fill the FIFO through nonblocking writer `w` with writes of `chunk` bytes until it
/// refuses one: the bytes written. Stops at 4 MiB.
fn fill(w: i64, chunk: usize, fill_byte: u8) -> Result<usize, CaseError> {
    let buf = vec![fill_byte; chunk];
    let mut total = 0;
    while total < 4 << 20 {
        let n = write(w as i32, buf.as_ptr(), chunk)?;
        if n == -EAGAIN { return Ok(total); }
        if n <= 0 { return err(format!("a write filling the FIFO returned {} after {total} bytes", shown(n))); }
        total += n as usize;
    }
    err("the FIFO took 4 MiB without refusing a nonblocking write")
}

fn fifo_write_blocks_full() -> CaseResult {
    let (f, r, w) = fifo_pair("fifo")?;
    let cap = fill(w, POSIX_PIPE_BUF, b'a')? + fill(w, 1, b'a')?;
    value("capacity", cap as i64, "", None);
    let pg = page()?;
    let mut writer = spawn(|| {
        let w2 = open_fifo(&f, O_WRONLY, "O_WRONLY in the writer")?;
        let n = put(w2, b"z")?;
        pg.set(0, mono());
        check(n == 1, &format!("the blocked write returned {}", shown(n)))
    })?;
    writer.blocks("the write to the full FIFO")?;
    let t0 = mono();
    let got = read_all(r, cap, 3000)?;
    check(got.len() == cap, &format!("draining the full FIFO read {} of {cap} bytes", got.len()))?;
    writer.finish(3000, "the writer")?;
    woken("the blocked write", pg.get(0), t0)?;
    let (n, last) = get(r, 16)?;
    check(n == 1 && last == b"z", &format!("the read after the drain returned {}, expected the blocked writer's byte", shown(n)))
}

fn fifo_write_eagain_full() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    let chunks = fill(w, POSIX_PIPE_BUF, b'a')?;
    let record = vec![b'b'; POSIX_PIPE_BUF];
    let n = put(w, &record)?;
    want(&format!("a {POSIX_PIPE_BUF}-byte O_NONBLOCK write to the full FIFO"), n, EAGAIN)?;
    let rest = fill(w, 1, b'c')?;
    let cap = chunks + rest;
    value("capacity", cap as i64, "", None);
    zero("close of the writer", close(w as i32)?)?;
    let got = read_all(r, cap + POSIX_PIPE_BUF, 3000)?;
    check(got.len() == cap, &format!("the FIFO held {} bytes, expected the {cap} accepted: the refused write left bytes behind", got.len()))?;
    check(!got.contains(&b'b'), "bytes of the refused write were found in the FIFO")
}

fn fifo_atomic_writes() -> CaseResult {
    const WRITERS: usize = 4;
    const RECORDS: usize = 64;
    let f = new_fifo("fifo")?;
    let r = open_fifo(&f, O_RDONLY | O_NONBLOCK, "O_RDONLY|O_NONBLOCK")?;
    let pg = page()?;
    let mut writers = Vec::new();
    for id in 1..=WRITERS {
        writers.push(spawn(|| {
            let _ = close(r as i32)?;
            let w = open_fifo(&f, O_WRONLY, "O_WRONLY in a writer")?;
            pg.w[0].fetch_add(1, SeqCst);
            for seq in 0..RECORDS {
                let mut record = vec![id as u8; POSIX_PIPE_BUF];
                record[1] = seq as u8;
                let n = put(w, &record)?;
                if n != POSIX_PIPE_BUF as i64 { return err(format!("writer {id}'s write {seq} returned {}", shown(n))); }
            }
            Ok(())
        })?);
    }
    check(until(3000, || pg.get(0) == WRITERS as i64), "the writers did not all open the FIFO within 3 s")?;
    let total = WRITERS * RECORDS * POSIX_PIPE_BUF;
    let got = read_all(r, total + 1, 6000)?;
    for (i, wr) in writers.iter_mut().enumerate() { wr.finish(2000, &format!("writer {}", i + 1))?; }
    check(got.len() == total, &format!("the reader got {} bytes, expected {total}", got.len()))?;
    let mut next = [0usize; WRITERS + 1];
    let mut torn = 0;
    for rec in got.chunks(POSIX_PIPE_BUF) {
        let id = rec[0] as usize;
        let whole = (1..=WRITERS).contains(&id) && rec[1] as usize == next[id] && rec[2..].iter().all(|&b| b as usize == id);
        if whole { next[id] += 1; } else { torn += 1; }
    }
    value("records", (WRITERS * RECORDS - torn) as i64, "", Some(((WRITERS * RECORDS) as i64, (WRITERS * RECORDS) as i64)));
    check(torn == 0, &format!("{torn} of {} records of {POSIX_PIPE_BUF} bytes were interleaved or out of order", WRITERS * RECORDS))
}

fn fifo_poll_in() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    let (n, ev) = poll_one(r, POLLIN, 0)?;
    check(n == 0 && ev == 0, &format!("poll of the empty reader returned {} with revents {ev:#x}, expected nothing ready", shown(n)))?;
    put_all(w, b"x", "the writer")?;
    let (n, ev) = poll_one(r, POLLIN, 0)?;
    check(n == 1 && ev & POLLIN != 0, &format!("poll of the reader with data returned {} with revents {ev:#x}, expected POLLIN", shown(n)))
}

fn fifo_poll_out() -> CaseResult {
    let (_f, _r, w) = fifo_pair("fifo")?;
    let (n, ev) = poll_one(w, POLLOUT, 0)?;
    check(n == 1 && ev & POLLOUT != 0, &format!("poll of the empty FIFO's writer returned {} with revents {ev:#x}, expected POLLOUT", shown(n)))
}

fn fifo_poll_hup() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    zero("close of the only writer", close(w as i32)?)?;
    let (n, ev) = poll_one(r, POLLIN, 0)?;
    check(n == 1 && ev & POLLHUP != 0, &format!("poll of the reader after the last writer closed returned {} with revents {ev:#x}, expected POLLHUP", shown(n)))
}

fn fifo_poll_wakes() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    let pg = page()?;
    let mut poller = spawn(|| {
        let (n, ev) = poll_one(r, POLLIN, 3000)?;
        pg.set(0, mono());
        check(n == 1 && ev & POLLIN != 0, &format!("the blocked poll returned {} with revents {ev:#x}, expected POLLIN", shown(n)))
    })?;
    poller.blocks("the poll of the empty reader")?;
    let t0 = mono();
    put_all(w, b"x", "the writer")?;
    poller.finish(4000, "the poller")?;
    woken("the blocked poll", pg.get(0), t0)
}

fn fifo_select_read() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    let (n, ready) = select_one(r, false, 0)?;
    check(n == 0 && !ready, &format!("select of the empty reader returned {}, expected nothing ready", shown(n)))?;
    put_all(w, b"x", "the writer")?;
    let (n, ready) = select_one(r, false, 0)?;
    check(n == 1 && ready, &format!("select of the reader with data returned {}, expected it ready", shown(n)))
}

fn fifo_select_write() -> CaseResult {
    let (_f, _r, w) = fifo_pair("fifo")?;
    let (n, ready) = select_one(w, true, 0)?;
    check(n == 1 && ready, &format!("select of the empty FIFO's writer returned {}, expected it ready", shown(n)))
}

fn fifo_select_wakes() -> CaseResult {
    let (_f, r, w) = fifo_pair("fifo")?;
    let pg = page()?;
    let mut selecter = spawn(|| {
        let (n, ready) = select_one(r, false, 3000)?;
        pg.set(0, mono());
        check(n == 1 && ready, &format!("the blocked select returned {}, expected the reader ready", shown(n)))
    })?;
    selecter.blocks("the select of the empty reader")?;
    let t0 = mono();
    put_all(w, b"x", "the writer")?;
    selecter.finish(4000, "the selecting process")?;
    woken("the blocked select", pg.get(0), t0)
}

fn fifo_unlink_open() -> CaseResult {
    let (f, r, w) = fifo_pair("fifo")?;
    zero("unlink of the open FIFO", unlink(f.path())?)?;
    put_all(w, b"abc", "the writer after unlink")?;
    let (n, got) = get(r, 16)?;
    check(n == 3 && got == b"abc", &format!("the read after unlink returned {}", shown(n)))?;
    want("open of the unlinked path", open(f.path(), O_RDONLY | O_NONBLOCK, 0)?, ENOENT)
}

fn fifo_reopen() -> CaseResult {
    let (f, r, w) = fifo_pair("fifo")?;
    put_all(w, b"old", "the writer")?;
    zero("close of the writer", close(w as i32)?)?;
    zero("close of the reader", close(r as i32)?)?;
    let r = open_fifo(&f, O_RDONLY | O_NONBLOCK, "O_RDONLY|O_NONBLOCK again")?;
    let _w = open_fifo(&f, O_WRONLY | O_NONBLOCK, "O_WRONLY|O_NONBLOCK again")?;
    want("a read of the reopened FIFO, whose old data must be discarded", get(r, 16)?.0, EAGAIN)
}

// ---------------------------------------------------------------------------
// POSIX message queues.

const MQ_MSGSIZE: usize = 64;

/// A queue this case made with mq_open(O_CREAT|O_EXCL), mode 0600 and `maxmsg` messages
/// of MQ_MSGSIZE bytes; `extra` is added to O_RDWR.
fn new_mq(tag: &str, extra: i32, maxmsg: i64) -> Result<(Made, i32), CaseError> {
    let q = Made::name(Kind::Mq, &ipc_name(tag));
    let attr = MqAttr { maxmsg, msgsize: MQ_MSGSIZE as i64, ..MqAttr::default() };
    let mqd = mq_open(&q.name, O_RDWR | O_CREAT | O_EXCL | extra, 0o600, &attr)?;
    ok(&format!("mq_open(O_CREAT|O_EXCL) of {}", q.text()), mqd)?;
    Ok((q, mqd as i32))
}

fn getattr(mqd: i32) -> Result<MqAttr, CaseError> {
    let mut a = MqAttr::default();
    zero("mq_getattr", mq_getattr(mqd, &mut a)?)?;
    Ok(a)
}

fn send(mqd: i32, msg: &[u8], prio: u32) -> CaseResult {
    zero(&format!("mq_send of {} bytes at priority {prio}", msg.len()), mq_send(mqd, msg.as_ptr(), msg.len(), prio)?)
}

/// mq_receive into a buffer of MQ_MSGSIZE bytes: the length, the bytes and the priority.
fn receive(mqd: i32) -> Result<(i64, Vec<u8>, u32), CaseError> {
    let mut buf = vec![0u8; MQ_MSGSIZE];
    let mut prio = u32::MAX;
    let n = mq_receive(mqd, buf.as_mut_ptr(), buf.len(), &mut prio)?;
    buf.truncate(n.max(0) as usize);
    Ok((n, buf, prio))
}

fn received(mqd: i32, want_msg: &[u8], want_prio: u32) -> CaseResult {
    let (n, got, prio) = receive(mqd)?;
    ok("mq_receive", n)?;
    check(got == want_msg && prio == want_prio, &format!(
        "mq_receive returned {:?} at priority {prio}, expected {:?} at priority {want_prio}",
        String::from_utf8_lossy(&got), String::from_utf8_lossy(want_msg)))
}

fn notify(mqd: i32, sig: i32, cookie: u64) -> Result<i64, CaseError> {
    let ev = SigEvent { value: cookie, signo: sig, notify: SIGEV_SIGNAL, _pad: [0; 6] };
    mq_notify(mqd, &ev)
}

fn mq_open_create() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 4)?;
    let a = getattr(mqd)?;
    check(a.maxmsg == 4 && a.msgsize == MQ_MSGSIZE as i64 && a.curmsgs == 0 && a.flags == 0, &format!(
        "mq_getattr reports flags {:#x}, maxmsg {}, msgsize {}, curmsgs {}; expected 0, 4, {MQ_MSGSIZE}, 0",
        a.flags, a.maxmsg, a.msgsize, a.curmsgs))
}

fn mq_open_eexist() -> CaseResult {
    let (q, _mqd) = new_mq("mq", 0, 4)?;
    want("a second mq_open(O_CREAT|O_EXCL)", mq_open(&q.name, O_RDWR | O_CREAT | O_EXCL, 0o600, null())?, EEXIST)
}

fn mq_open_enoent() -> CaseResult {
    let q = Made::name(Kind::Mq, &ipc_name("mq"));
    want("mq_open of a missing name without O_CREAT", mq_open(&q.name, O_RDWR, 0, null())?, ENOENT)
}

fn mq_open_attr_einval() -> CaseResult {
    let q = Made::name(Kind::Mq, &ipc_name("mq"));
    for (maxmsg, msgsize) in [(0, MQ_MSGSIZE as i64), (4, 0)] {
        let attr = MqAttr { maxmsg, msgsize, ..MqAttr::default() };
        want(&format!("mq_open(O_CREAT) with mq_maxmsg {maxmsg} and mq_msgsize {msgsize}"),
            mq_open(&q.name, O_RDWR | O_CREAT | O_EXCL, 0o600, &attr)?, EINVAL)?;
    }
    Ok(())
}

fn mq_open_default_attr() -> CaseResult {
    let q = Made::name(Kind::Mq, &ipc_name("mq"));
    let mqd = ok("mq_open(O_CREAT) with no attributes", mq_open(&q.name, O_RDWR | O_CREAT | O_EXCL, 0o600, null())?)? as i32;
    let a = getattr(mqd)?;
    value("maxmsg", a.maxmsg, "", None);
    check(a.maxmsg > 0 && a.msgsize > 0, &format!("the default queue has maxmsg {} and msgsize {}", a.maxmsg, a.msgsize))
}

fn mq_close_case() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 4)?;
    zero("mq_close", mq_close(mqd)?)?;
    let mut a = MqAttr::default();
    want("mq_getattr of the closed descriptor", mq_getattr(mqd, &mut a)?, EBADF)
}

fn mq_unlink_case() -> CaseResult {
    let (q, _mqd) = new_mq("mq", 0, 4)?;
    zero("mq_unlink", mq_unlink(&q.name)?)?;
    want("mq_open of the unlinked name", mq_open(&q.name, O_RDWR, 0, null())?, ENOENT)?;
    want("a second mq_unlink", mq_unlink(&q.name)?, ENOENT)
}

fn mq_unlink_open() -> CaseResult {
    let (q, mqd) = new_mq("mq", 0, 4)?;
    send(mqd, b"before", 1)?;
    zero("mq_unlink", mq_unlink(&q.name)?)?;
    received(mqd, b"before", 1)?;
    send(mqd, b"after", 2)?;
    received(mqd, b"after", 2)
}

fn mq_send_receive() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 4)?;
    send(mqd, b"hello ipc", 7)?;
    received(mqd, b"hello ipc", 7)
}

fn mq_priority_order() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 8)?;
    let sent: [(&[u8], u32); 6] = [(b"a1", 1), (b"b5", 5), (b"c1", 1), (b"d5", 5), (b"e0", 0), (b"f31", 31)];
    for (msg, prio) in sent { send(mqd, msg, prio)?; }
    let mut order = Vec::new();
    for _ in 0..sent.len() {
        let (n, got, _) = receive(mqd)?;
        ok("mq_receive", n)?;
        order.push(String::from_utf8_lossy(&got).into_owned());
    }
    value("messages", order.len() as i64, "", Some((6, 6)));
    let order = order.join(" ");
    check(order == "f31 b5 d5 a1 c1 e0", &format!("messages came in the order {order}, expected f31 b5 d5 a1 c1 e0"))
}

fn mq_curmsgs() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 4)?;
    for i in 0..3u8 { send(mqd, &[b'0' + i], 0)?; }
    let depth = getattr(mqd)?.curmsgs;
    value("depth", depth, "", Some((3, 3)));
    check(depth == 3, &format!("mq_getattr counts {depth} messages after 3 were sent"))?;
    ok("mq_receive", receive(mqd)?.0)?;
    let depth = getattr(mqd)?.curmsgs;
    check(depth == 2, &format!("mq_getattr counts {depth} messages after one of 3 was received"))
}

fn mq_setattr_case() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 4)?;
    let new = MqAttr { flags: O_NONBLOCK as i64, maxmsg: 99, msgsize: 99, curmsgs: 99, ..MqAttr::default() };
    let mut old = MqAttr::default();
    zero("mq_setattr(O_NONBLOCK)", mq_setattr(mqd, &new, &mut old)?)?;
    check(old.flags == 0 && old.maxmsg == 4 && old.msgsize == MQ_MSGSIZE as i64, &format!(
        "mq_setattr returned old flags {:#x}, maxmsg {}, msgsize {}", old.flags, old.maxmsg, old.msgsize))?;
    let a = getattr(mqd)?;
    check(a.flags == O_NONBLOCK as i64 && a.maxmsg == 4 && a.msgsize == MQ_MSGSIZE as i64, &format!(
        "after mq_setattr the queue has flags {:#x}, maxmsg {}, msgsize {}; only O_NONBLOCK may change", a.flags, a.maxmsg, a.msgsize))?;
    want("mq_receive of the empty queue once O_NONBLOCK is set", receive(mqd)?.0, EAGAIN)
}

fn mq_emsgsize_receive() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 4)?;
    send(mqd, b"x", 0)?;
    let mut buf = vec![0u8; MQ_MSGSIZE - 1];
    let mut prio = 0u32;
    want(&format!("mq_receive into {} bytes with mq_msgsize {MQ_MSGSIZE}", MQ_MSGSIZE - 1),
        mq_receive(mqd, buf.as_mut_ptr(), buf.len(), &mut prio)?, EMSGSIZE)?;
    let depth = getattr(mqd)?.curmsgs;
    check(depth == 1, &format!("the refused mq_receive left {depth} messages, expected 1"))
}

fn mq_emsgsize_send() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 4)?;
    let big = vec![b'x'; MQ_MSGSIZE + 1];
    want(&format!("mq_send of {} bytes with mq_msgsize {MQ_MSGSIZE}", big.len()), mq_send(mqd, big.as_ptr(), big.len(), 0)?, EMSGSIZE)
}

fn mq_prio_max() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 4)?;
    send(mqd, b"x", (POSIX_MQ_PRIO_MAX - 1) as u32)?;
    let max = sysconf(SC_MQ_PRIO_MAX)?;
    value("prio-max", max, "", Some((POSIX_MQ_PRIO_MAX, i64::from(u32::MAX))));
    check(max >= POSIX_MQ_PRIO_MAX, &format!("sysconf(_SC_MQ_PRIO_MAX) returned {max}, POSIX requires at least {POSIX_MQ_PRIO_MAX}"))?;
    want("mq_send at priority MQ_PRIO_MAX", mq_send(mqd, b"y".as_ptr(), 1, max as u32)?, EINVAL)
}

fn mq_eagain_full() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 2)?;
    send(mqd, b"1", 0)?;
    send(mqd, b"2", 0)?;
    want("mq_send to the full O_NONBLOCK queue", mq_send(mqd, b"3".as_ptr(), 1, 0)?, EAGAIN)?;
    let depth = getattr(mqd)?.curmsgs;
    value("depth", depth, "", Some((2, 2)));
    check(depth == 2, &format!("the full queue counts {depth} messages, expected 2"))
}

fn mq_eagain_empty() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 2)?;
    want("mq_receive of the empty O_NONBLOCK queue", receive(mqd)?.0, EAGAIN)
}

fn mq_ebadf_mode() -> CaseResult {
    let (q, mqd) = new_mq("mq", O_NONBLOCK, 2)?;
    send(mqd, b"x", 0)?;
    let rd = ok("mq_open(O_RDONLY)", mq_open(&q.name, O_RDONLY | O_NONBLOCK, 0, null())?)? as i32;
    let wr = ok("mq_open(O_WRONLY)", mq_open(&q.name, O_WRONLY | O_NONBLOCK, 0, null())?)? as i32;
    want("mq_send on an O_RDONLY descriptor", mq_send(rd, b"y".as_ptr(), 1, 0)?, EBADF)?;
    want("mq_receive on an O_WRONLY descriptor", receive(wr)?.0, EBADF)
}

fn mq_timedreceive_timeout() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 2)?;
    let deadline = rt() + 200 * MS;
    let abs = ts(deadline);
    let mut buf = [0u8; MQ_MSGSIZE];
    let mut prio = 0u32;
    wait_for(200, "late");
    let r = mq_timedreceive(mqd, buf.as_mut_ptr(), buf.len(), &mut prio, &abs)?;
    let end = rt();
    want("mq_timedreceive of the empty queue", r, ETIMEDOUT)?;
    on_time("mq_timedreceive", end, deadline)
}

fn mq_timedsend_timeout() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 1)?;
    send(mqd, b"full", 0)?;
    let deadline = rt() + 200 * MS;
    let abs = ts(deadline);
    wait_for(200, "late");
    let r = mq_timedsend(mqd, b"x".as_ptr(), 1, 0, &abs)?;
    let end = rt();
    want("mq_timedsend to the full queue", r, ETIMEDOUT)?;
    on_time("mq_timedsend", end, deadline)
}

fn mq_timed_einval() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 2)?;
    let bad: Ts = [rt() / NS + 1, NS];
    let mut buf = [0u8; MQ_MSGSIZE];
    let mut prio = 0u32;
    want("mq_timedreceive of the empty queue with tv_nsec 1000000000",
        mq_timedreceive(mqd, buf.as_mut_ptr(), buf.len(), &mut prio, &bad)?, EINVAL)
}

fn mq_timed_ready() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 2)?;
    send(mqd, b"ready", 3)?;
    let past = ts(rt() - NS);
    let mut buf = [0u8; MQ_MSGSIZE];
    let mut prio = 0u32;
    let n = mq_timedreceive(mqd, buf.as_mut_ptr(), buf.len(), &mut prio, &past)?;
    check(n == 5 && &buf[..5] == b"ready", &format!("mq_timedreceive with a message queued and a deadline past returned {}", shown(n)))
}

fn mq_receive_blocks() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 2)?;
    let pg = page()?;
    let mut receiver = spawn(|| {
        let (n, got, _) = receive(mqd)?;
        pg.set(0, mono());
        check(n == 4 && got == b"wake", &format!("the blocked mq_receive returned {}", shown(n)))
    })?;
    receiver.blocks("the mq_receive of the empty queue")?;
    let t0 = mono();
    send(mqd, b"wake", 0)?;
    receiver.finish(3000, "the receiver")?;
    woken("the blocked mq_receive", pg.get(0), t0)
}

fn mq_send_blocks() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 1)?;
    send(mqd, b"first", 0)?;
    let pg = page()?;
    let mut sender = spawn(|| {
        let r = mq_send(mqd, b"second".as_ptr(), 6, 0)?;
        pg.set(0, mono());
        zero("the blocked mq_send", r)
    })?;
    sender.blocks("the mq_send to the full queue")?;
    let t0 = mono();
    received(mqd, b"first", 0)?;
    sender.finish(3000, "the sender")?;
    woken("the blocked mq_send", pg.get(0), t0)?;
    received(mqd, b"second", 0)
}

/// Check the siginfo of a message-queue notification.
fn notified(info: &[u8; 128], cookie: u64) -> CaseResult {
    let code = info_i32(info, 8);
    let val = info_u64(info, 24);
    check(code == SI_MESGQ, &format!("the notification's si_code is {code}, expected SI_MESGQ ({SI_MESGQ})"))?;
    check(val == cookie, &format!("the notification's si_value is {val:#x}, expected {cookie:#x}"))
}

fn mq_notify_signal() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 2)?;
    block(SIGUSR1)?;
    zero("mq_notify(SIGEV_SIGNAL)", notify(mqd, SIGUSR1, 0x1234_5678)?)?;
    send(mqd, b"x", 0)?;
    match take_signal(SIGUSR1, 2000)? {
        Some(info) => notified(&info, 0x1234_5678),
        None => fail("no SIGUSR1 came within 2 s of a message arriving on the empty queue"),
    }
}

fn mq_notify_once() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 2)?;
    block(SIGUSR1)?;
    zero("mq_notify(SIGEV_SIGNAL)", notify(mqd, SIGUSR1, 7)?)?;
    send(mqd, b"x", 0)?;
    check(take_signal(SIGUSR1, 2000)?.is_some(), "no SIGUSR1 came for the first message")?;
    ok("mq_receive", receive(mqd)?.0)?;
    send(mqd, b"y", 0)?;
    check(take_signal(SIGUSR1, 300)?.is_none(), "a second SIGUSR1 came after the registration was used")?;
    zero("mq_notify registering again", notify(mqd, SIGUSR1, 8)?)
}

fn mq_notify_ebusy() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 2)?;
    zero("mq_notify(SIGEV_SIGNAL)", notify(mqd, SIGUSR1, 1)?)?;
    spawn(|| want("mq_notify from a second process", notify(mqd, SIGUSR1, 2)?, EBUSY))?
        .finish(3000, "the second process")
}

fn mq_notify_receiver_waiting() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 2)?;
    block(SIGUSR1)?;
    zero("mq_notify(SIGEV_SIGNAL)", notify(mqd, SIGUSR1, 3)?)?;
    let mut receiver = spawn(|| {
        let (n, _, _) = receive(mqd)?;
        check(n == 1, &format!("the blocked mq_receive returned {}", shown(n)))
    })?;
    receiver.blocks("the mq_receive of the empty queue")?;
    send(mqd, b"x", 0)?;
    receiver.finish(3000, "the receiver")?;
    check(take_signal(SIGUSR1, 300)?.is_none(), "SIGUSR1 came although a receiver was waiting for the message")?;
    spawn(|| want("mq_notify from another process while the first stays registered", notify(mqd, SIGUSR1, 4)?, EBUSY))?
        .finish(3000, "the second process")
}

fn mq_notify_remove() -> CaseResult {
    let (_q, mqd) = new_mq("mq", O_NONBLOCK, 2)?;
    block(SIGUSR1)?;
    zero("mq_notify(SIGEV_SIGNAL)", notify(mqd, SIGUSR1, 5)?)?;
    zero("mq_notify(NULL)", mq_notify(mqd, null())?)?;
    send(mqd, b"x", 0)?;
    check(take_signal(SIGUSR1, 300)?.is_none(), "SIGUSR1 came after mq_notify(NULL) removed the registration")?;
    spawn(|| zero("mq_notify from another process after the removal", notify(mqd, SIGUSR1, 6)?))?
        .finish(3000, "the second process")
}

fn mq_fork_shared() -> CaseResult {
    let (_q, mqd) = new_mq("mq", 0, 4)?;
    spawn(|| {
        for i in 0..3u8 { send(mqd, &[b'a' + i], u32::from(i))?; }
        Ok(())
    })?
    .finish(3000, "the child")?;
    let depth = getattr(mqd)?.curmsgs;
    value("depth", depth, "", Some((3, 3)));
    received(mqd, b"c", 2)?;
    received(mqd, b"b", 1)?;
    received(mqd, b"a", 0)
}

fn mq_name_shared() -> CaseResult {
    let pg = page()?;
    let name = ipc_name("mq");
    let mut other = spawn(|| {
        let q = c(&name);
        check(until(3000, || pg.get(0) == 1), "the queue was not created within 3 s")?;
        let mqd = ok("mq_open of the name in the other process", mq_open(&q, O_RDWR, 0, null())?)? as i32;
        received(mqd, b"ping", 1)?;
        send(mqd, b"pong", 2)
    })?;
    let q = Made::name(Kind::Mq, &name);
    let attr = MqAttr { maxmsg: 4, msgsize: MQ_MSGSIZE as i64, ..MqAttr::default() };
    let mqd = ok("mq_open(O_CREAT|O_EXCL)", mq_open(&q.name, O_RDWR | O_CREAT | O_EXCL, 0o600, &attr)?)? as i32;
    send(mqd, b"ping", 1)?;
    pg.set(0, 1);
    other.finish(3000, "the other process")?;
    received(mqd, b"pong", 2)?;
    value("messages", 2, "", Some((2, 2)));
    Ok(())
}

fn mq_permissions() -> CaseResult {
    set_umask(0)?;
    let private = Made::name(Kind::Mq, &ipc_name("private"));
    ok("mq_open(O_CREAT, 0600)", mq_open(&private.name, O_RDWR | O_CREAT | O_EXCL, 0o600, null())?)?;
    let public = Made::name(Kind::Mq, &ipc_name("public"));
    ok("mq_open(O_CREAT, 0644)", mq_open(&public.name, O_RDWR | O_CREAT | O_EXCL, 0o644, null())?)?;
    as_user(USER_A, || {
        want("mq_open(O_RDONLY) of a 0600 queue by another user", mq_open(&private.name, O_RDONLY, 0, null())?, EACCES)?;
        ok("mq_open(O_RDONLY) of a 0644 queue by another user", mq_open(&public.name, O_RDONLY, 0, null())?)?;
        want("mq_open(O_RDWR) of a 0644 queue by another user", mq_open(&public.name, O_RDWR, 0, null())?, EACCES)
    })
}

// ---------------------------------------------------------------------------
// POSIX semaphores.

fn sem_value(sem: *mut u8) -> Result<i32, CaseError> {
    let mut v = i32::MIN;
    zero("sem_getvalue", sem_getvalue(sem, &mut v)?)?;
    Ok(v)
}

fn sem_is(sem: *mut u8, want_value: i32, when: &str) -> CaseResult {
    let v = sem_value(sem)?;
    check(v == want_value, &format!("sem_getvalue reports {v} {when}, expected {want_value}"))
}

/// An unnamed semaphore in a page shared with the processes the case starts.
fn shared_sem(pg: &Page, pshared: i32, init: u32) -> Result<*mut u8, CaseError> {
    let s = pg.sem(0);
    zero(&format!("sem_init(pshared {pshared}, {init})"), sem_init(s, pshared, init)?)?;
    Ok(s)
}

/// SEM_VALUE_MAX as sysconf reports it, which POSIX requires to be at least 32767.
fn sem_value_max() -> Result<i64, CaseError> {
    let max = sysconf(SC_SEM_VALUE_MAX)?;
    check(max >= POSIX_SEM_VALUE_MAX, &format!("sysconf(_SC_SEM_VALUE_MAX) returned {max}, POSIX requires at least {POSIX_SEM_VALUE_MAX}"))?;
    Ok(max)
}

/// A named semaphore this case made with sem_open(O_CREAT|O_EXCL, 0600).
fn new_named(tag: &str, init: u32) -> Result<(Made, *mut u8), CaseError> {
    let n = Made::name(Kind::Sem, &ipc_name(tag));
    let s = ok(&format!("sem_open(O_CREAT|O_EXCL) of {}", n.text()), sem_open(&n.name, O_CREAT | O_EXCL, 0o600, init)?)?;
    Ok((n, s as *mut u8))
}

fn sem_init_case() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 0, 3)?;
    sem_is(s, 3, "after sem_init(3)")
}

fn sem_init_einval() -> CaseResult {
    let pg = page()?;
    shared_sem(pg, 0, 1)?;
    let max = sem_value_max()?;
    if max >= i64::from(u32::MAX) { return skip("SEM_VALUE_MAX is the largest unsigned value"); }
    want("sem_init with SEM_VALUE_MAX + 1", sem_init(pg.sem(1), 0, max as u32 + 1)?, EINVAL)
}

fn sem_post_wait() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 0, 0)?;
    zero("sem_post", sem_post(s)?)?;
    sem_is(s, 1, "after sem_post")?;
    zero("sem_wait", sem_wait(s)?)?;
    sem_is(s, 0, "after sem_wait")
}

fn sem_trywait_case() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 0, 0)?;
    want("sem_trywait at zero", sem_trywait(s)?, EAGAIN)?;
    sem_is(s, 0, "after the refused sem_trywait")?;
    zero("sem_post", sem_post(s)?)?;
    zero("sem_trywait at one", sem_trywait(s)?)?;
    sem_is(s, 0, "after sem_trywait took it")
}

fn sem_wait_blocks() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 1, 0)?;
    let mut waiter = spawn(|| {
        let r = sem_wait(s)?;
        pg.set(0, mono());
        zero("the blocked sem_wait", r)
    })?;
    waiter.blocks("the sem_wait at zero")?;
    let t0 = mono();
    zero("sem_post", sem_post(s)?)?;
    waiter.finish(3000, "the waiter")?;
    woken("the blocked sem_wait", pg.get(0), t0)
}

fn sem_timedwait_timeout() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 0, 0)?;
    let deadline = rt() + 200 * MS;
    let abs = ts(deadline);
    wait_for(200, "late");
    let r = sem_timedwait(s, &abs)?;
    let end = rt();
    want("sem_timedwait at zero", r, ETIMEDOUT)?;
    on_time("sem_timedwait", end, deadline)
}

fn sem_timedwait_einval() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 0, 0)?;
    let bad: Ts = [rt() / NS + 1, NS];
    want("sem_timedwait at zero with tv_nsec 1000000000", sem_timedwait(s, &bad)?, EINVAL)
}

fn sem_timedwait_ready() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 0, 1)?;
    let past = ts(rt() - NS);
    zero("sem_timedwait of an available semaphore with a deadline past", sem_timedwait(s, &past)?)?;
    sem_is(s, 0, "after sem_timedwait took it")
}

extern "C" fn on_usr1(_sig: i32) {}

fn sem_eintr() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 1, 0)?;
    set_action(SIGUSR1, &Sigaction::new(on_usr1))?;
    let me = pid();
    let mut kicker = spawn(|| {
        check(until(2000, || is_parked(me)), "the case's sem_wait did not block within 2 s")?;
        zero("kill(SIGUSR1)", sc(nr::KILL, &[me as u64, SIGUSR1 as u64]))?;
        sleep_ms(1000);
        zero("sem_post after the signal", sem_post(s)?)
    })?;
    let r = sem_wait(s)?;
    kicker.finish(3000, "the signalling process")?;
    check(r == -EINTR, &format!("sem_wait returned {} when a handled signal arrived, expected EINTR", shown(r)))
}

fn sem_destroy_case() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 0, 1)?;
    zero("sem_destroy", sem_destroy(s)?)
}

fn sem_pshared_fork() -> CaseResult {
    const POSTS: i64 = 100;
    let pg = page()?;
    let s = shared_sem(pg, 1, 0)?;
    let mut poster = spawn(|| {
        for _ in 0..POSTS { zero("sem_post in the child", sem_post(s)?)?; }
        Ok(())
    })?;
    let deadline = ts(rt() + 3 * NS);
    let mut taken = 0;
    while taken < POSTS {
        let r = sem_timedwait(s, &deadline)?;
        if r != 0 { break; }
        taken += 1;
    }
    poster.finish(2000, "the posting child")?;
    value("count", taken, "", Some((POSTS, POSTS)));
    check(taken == POSTS, &format!("the parent took {taken} of the child's {POSTS} posts within 3 s"))?;
    sem_is(s, 0, "after every post was taken")
}

fn sem_open_create() -> CaseResult {
    let (_n, s) = new_named("sem", 2)?;
    sem_is(s, 2, "after sem_open(O_CREAT, 2)")
}

fn sem_open_eexist() -> CaseResult {
    let (n, _s) = new_named("sem", 0)?;
    want("a second sem_open(O_CREAT|O_EXCL)", sem_open(&n.name, O_CREAT | O_EXCL, 0o600, 0)?, EEXIST)
}

fn sem_open_enoent() -> CaseResult {
    let n = Made::name(Kind::Sem, &ipc_name("sem"));
    want("sem_open of a missing name without O_CREAT", sem_open(&n.name, 0, 0, 0)?, ENOENT)
}

fn sem_open_same() -> CaseResult {
    let (n, s) = new_named("sem", 0)?;
    let again = ok("sem_open of the same name", sem_open(&n.name, 0, 0, 0)?)?;
    check(again as *mut u8 == s, &format!("the second sem_open returned {:#x}, the first {:#x}", again, s as usize))
}

fn sem_open_einval_value() -> CaseResult {
    let n = Made::name(Kind::Sem, &ipc_name("sem"));
    ok("sem_open(O_CREAT)", sem_open(&n.name, O_CREAT | O_EXCL, 0o600, 0)?)?;
    zero("sem_unlink", sem_unlink(n.path())?)?;
    let max = sem_value_max()?;
    if max >= i64::from(u32::MAX) { return skip("SEM_VALUE_MAX is the largest unsigned value"); }
    want("sem_open(O_CREAT) with SEM_VALUE_MAX + 1", sem_open(&n.name, O_CREAT | O_EXCL, 0o600, max as u32 + 1)?, EINVAL)
}

fn sem_named_processes() -> CaseResult {
    let (n, s) = new_named("sem", 0)?;
    let pg = page()?;
    let mut poster = spawn(|| {
        let other = ok("sem_open of the name in the other process", sem_open(&n.name, 0, 0, 0)?)? as *mut u8;
        pg.set(0, mono());
        zero("sem_post in the other process", sem_post(other)?)
    })?;
    let deadline = ts(rt() + 3 * NS);
    let r = sem_timedwait(s, &deadline)?;
    let woke = mono();
    poster.finish(2000, "the other process")?;
    zero("sem_timedwait for the other process's post", r)?;
    value("wait", (woke - pg.get(0)) / 1000, "us", None);
    Ok(())
}

fn sem_close_case() -> CaseResult {
    let (n, s) = new_named("sem", 1)?;
    zero("sem_close", sem_close(s)?)?;
    let again = ok("sem_open of the name after sem_close", sem_open(&n.name, 0, 0, 0)?)?;
    sem_is(again as *mut u8, 1, "after reopening")
}

fn sem_unlink_case() -> CaseResult {
    let (n, s) = new_named("sem", 0)?;
    zero("sem_unlink", sem_unlink(n.path())?)?;
    zero("sem_post after sem_unlink", sem_post(s)?)?;
    sem_is(s, 1, "after sem_unlink and sem_post")?;
    want("sem_open of the unlinked name", sem_open(&n.name, 0, 0, 0)?, ENOENT)?;
    want("a second sem_unlink", sem_unlink(n.path())?, ENOENT)
}

fn sem_permissions() -> CaseResult {
    set_umask(0)?;
    let (n, _s) = new_named("sem", 0)?;
    as_user(USER_A, || want("sem_open of a 0600 semaphore by another user", sem_open(&n.name, 0, 0, 0)?, EACCES))
}

fn sem_getvalue_waiters() -> CaseResult {
    let pg = page()?;
    let s = shared_sem(pg, 1, 0)?;
    let mut waiter = spawn(|| zero("the blocked sem_wait", sem_wait(s)?))?;
    waiter.blocks("the sem_wait at zero")?;
    let v = sem_value(s)?;
    value("value", i64::from(v), "", Some((i64::from(i32::MIN), 0)));
    check(v <= 0, &format!("sem_getvalue with a waiter reports {v}, expected 0 or a negative count"))?;
    zero("sem_post", sem_post(s)?)?;
    waiter.finish(3000, "the waiter")
}

/// Processes for a several-processor case: the count online, at most four, or a skip
/// below two.
fn cpu_workers() -> Result<usize, CaseError> {
    let n = processors()?;
    if n < 2 { return Err(CaseError::Skip(format!("{n} processor online; the case needs two"))); }
    Ok(n.min(4))
}

/// Start `n` workers, worker i pinned to processor i, which show they run at once and
/// then run `work(i)`.
fn start_workers(pg: &'static Page, n: usize, work: impl Fn(usize) -> CaseResult) -> Result<Vec<Child>, CaseError> {
    let mut workers = Vec::new();
    for i in 0..n {
        let work = &work;
        workers.push(spawn(move || {
            pin_to(i)?;
            barrier_rounds(pg, 31, n as i64)?;
            work(i)
        })?);
    }
    Ok(workers)
}

fn sem_contention_cpus() -> CaseResult {
    const POSTS: i64 = 20_000;
    let n = cpu_workers()?;
    let pg = page()?;
    let s = shared_sem(pg, 1, 0)?;
    let producers = n / 2;
    let consumers = n - producers;
    let total = producers as i64 * POSTS;
    let start = mono();
    let mut workers = start_workers(pg, n, |i| {
        if i < producers {
            for _ in 0..POSTS { zero("sem_post", sem_post(s)?)?; }
            return Ok(());
        }
        let c = (i - producers) as i64;
        let share = total / consumers as i64 + if c == 0 { total % consumers as i64 } else { 0 };
        for _ in 0..share {
            zero("sem_wait", sem_wait(s)?)?;
            pg.w[0].fetch_add(1, SeqCst);
        }
        Ok(())
    })?;
    let done = until(6000, || workers.iter_mut().all(|w| w.reap().is_some()));
    let ms = (mono() - start) / MS;
    let taken = pg.get(0);
    value("posts", total, "", None);
    value("taken", taken, "", Some((total, total)));
    value("elapsed", ms, "ms", None);
    for (i, w) in workers.iter_mut().enumerate() {
        if let Some(status) = w.reap() { w.outcome(status, &format!("worker {i}"))?; }
    }
    if !done {
        let v = sem_value(s).map(|v| v.to_string()).unwrap_or_else(|e| match e { CaseError::Fail(m) | CaseError::Skip(m) => m });
        return fail(format!("waiters took {taken} of {total} posts and are still blocked with the value at {v}: wakeups were lost"));
    }
    check(taken == total, &format!("waiters took {taken} of {total} posts"))?;
    sem_is(s, 0, "after every post was taken")
}

fn sem_mutex_cpus() -> CaseResult {
    const ROUNDS_EACH: i64 = 5_000;
    let n = cpu_workers()?;
    let pg = page()?;
    let s = shared_sem(pg, 1, 1)?;
    let mut workers = start_workers(pg, n, |_| {
        for _ in 0..ROUNDS_EACH {
            zero("sem_wait", sem_wait(s)?)?;
            let v = pg.w[0].load(Relaxed);
            core::hint::spin_loop();
            pg.w[0].store(v + 1, Relaxed);
            zero("sem_post", sem_post(s)?)?;
        }
        Ok(())
    })?;
    for (i, w) in workers.iter_mut().enumerate() { w.finish(8000, &format!("worker {i}"))?; }
    let want_count = n as i64 * ROUNDS_EACH;
    let count = pg.get(0);
    value("increments", count, "", Some((want_count, want_count)));
    check(count == want_count, &format!("{count} of {want_count} increments made under the semaphore survived: two processes were inside at once"))
}

// ---------------------------------------------------------------------------
// POSIX shared memory.

/// An object this case made with shm_open(O_RDWR|O_CREAT|O_EXCL, `mode`).
fn new_shm(tag: &str, mode: u32) -> Result<(Made, i64), CaseError> {
    let o = Made::name(Kind::Shm, &ipc_name(tag));
    let fd = ok(&format!("shm_open(O_CREAT|O_EXCL) of {}", o.text()), shm_open(o.path(), O_RDWR | O_CREAT | O_EXCL, mode)?)?;
    Ok((o, fd))
}

fn map_shared(fd: i64, len: usize, prot: i32) -> Result<*mut u8, CaseError> {
    let p = mmap(null_mut(), len, prot, MAP_SHARED, fd as i32, 0)?;
    ok(&format!("mmap(MAP_SHARED) of {len} bytes"), p)?;
    Ok(p as *mut u8)
}

/// A word at index `i` of a mapping, read and written so the compiler keeps every access.
fn peek(p: *mut u8, i: usize) -> u64 {
    // SAFETY: callers pass a live mapping at least (i + 1) * 8 bytes long.
    unsafe { core::ptr::read_volatile((p as *const u64).add(i)) }
}
fn poke(p: *mut u8, i: usize, v: u64) {
    // SAFETY: callers pass a live writable mapping at least (i + 1) * 8 bytes long.
    unsafe { core::ptr::write_volatile((p as *mut u64).add(i), v) }
}

fn shm_open_create() -> CaseResult {
    set_umask(0o022)?;
    let (_o, fd) = new_shm("shm", 0o666)?;
    let st = fstat_of(fd)?;
    check(st_size(&st) == 0, &format!("a new object has size {}, expected 0", st_size(&st)))?;
    let mode = st_mode(&st) & 0o777;
    check(mode == 0o644, &format!("shm_open(0666) under umask 022 made mode {mode:o}, expected 644"))
}

fn shm_open_eexist() -> CaseResult {
    let (o, _fd) = new_shm("shm", 0o600)?;
    want("a second shm_open(O_CREAT|O_EXCL)", shm_open(o.path(), O_RDWR | O_CREAT | O_EXCL, 0o600)?, EEXIST)
}

fn shm_open_enoent() -> CaseResult {
    let o = Made::name(Kind::Shm, &ipc_name("shm"));
    want("shm_open of a missing name without O_CREAT", shm_open(o.path(), O_RDWR, 0)?, ENOENT)
}

fn shm_ftruncate() -> CaseResult {
    const LEN: usize = 8192;
    let (_o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate to 8192", ftruncate(fd as i32, LEN as i64)?)?;
    let size = st_size(&fstat_of(fd)?);
    check(size == LEN as i64, &format!("fstat reports size {size} after ftruncate to {LEN}"))?;
    let p = map_shared(fd, LEN, PROT_READ)?;
    let nonzero = (0..LEN / 8).filter(|&i| peek(p, i) != 0).count();
    check(nonzero == 0, &format!("{nonzero} words of the new object are not zero"))
}

fn shm_map_fork() -> CaseResult {
    let (_o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    let p = map_shared(fd, 4096, PROT_READ | PROT_WRITE)?;
    poke(p, 0, 0xA11CE);
    spawn(|| {
        check(peek(p, 0) == 0xA11CE, &format!("the child read {:#x}, not the parent's store", peek(p, 0)))?;
        poke(p, 1, 0xB0B);
        Ok(())
    })?
    .finish(3000, "the child")?;
    check(peek(p, 1) == 0xB0B, &format!("the parent read {:#x}, not the child's store", peek(p, 1)))
}

fn shm_map_name() -> CaseResult {
    let (o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    let p = map_shared(fd, 4096, PROT_READ | PROT_WRITE)?;
    poke(p, 0, 0xCAFE);
    let name = o.text();
    spawn(|| {
        let n = c(&name);
        let fd2 = ok("shm_open of the name in the other process", shm_open(n.as_ptr(), O_RDWR, 0)?)?;
        let q = map_shared(fd2, 4096, PROT_READ | PROT_WRITE)?;
        check(peek(q, 0) == 0xCAFE, &format!("the other process read {:#x}, not the creator's store", peek(q, 0)))?;
        poke(q, 1, 0xF00D);
        Ok(())
    })?
    .finish(3000, "the other process")?;
    check(peek(p, 1) == 0xF00D, &format!("the creator read {:#x}, not the other process's store", peek(p, 1)))
}

fn shm_persist() -> CaseResult {
    let (o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    let p = map_shared(fd, 4096, PROT_READ | PROT_WRITE)?;
    poke(p, 3, 0x5EED);
    zero("munmap", munmap(p, 4096)?)?;
    zero("close", close(fd as i32)?)?;
    let fd = ok("shm_open of the name again", shm_open(o.path(), O_RDWR, 0)?)?;
    let size = st_size(&fstat_of(fd)?);
    check(size == 4096, &format!("the reopened object has size {size}, expected 4096"))?;
    let p = map_shared(fd, 4096, PROT_READ)?;
    check(peek(p, 3) == 0x5EED, &format!("the reopened object holds {:#x}, not what was stored", peek(p, 3)))
}

fn shm_close_keeps_mapping() -> CaseResult {
    let (_o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    let p = map_shared(fd, 4096, PROT_READ | PROT_WRITE)?;
    zero("close", close(fd as i32)?)?;
    poke(p, 0, 0x1234);
    spawn(|| {
        poke(p, 1, peek(p, 0) + 1);
        Ok(())
    })?
    .finish(3000, "the child")?;
    check(peek(p, 1) == 0x1235, &format!("after close the mapping read {:#x}, expected 0x1235 stored by the child", peek(p, 1)))
}

fn shm_unlink_case() -> CaseResult {
    let (o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    let p = map_shared(fd, 4096, PROT_READ | PROT_WRITE)?;
    zero("shm_unlink", shm_unlink(o.path())?)?;
    poke(p, 0, 77);
    check(peek(p, 0) == 77, "the mapping did not keep a store after shm_unlink")?;
    want("shm_open of the unlinked name", shm_open(o.path(), O_RDWR, 0)?, ENOENT)?;
    want("a second shm_unlink", shm_unlink(o.path())?, ENOENT)
}

fn shm_unlink_recreate() -> CaseResult {
    let (o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    zero("shm_unlink", shm_unlink(o.path())?)?;
    let fd2 = ok("shm_open(O_CREAT|O_EXCL) after shm_unlink", shm_open(o.path(), O_RDWR | O_CREAT | O_EXCL, 0o600)?)?;
    let size = st_size(&fstat_of(fd2)?);
    check(size == 0, &format!("the new object has size {size}, expected 0"))
}

fn shm_rdonly_map() -> CaseResult {
    let (o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    let rd = ok("shm_open(O_RDONLY)", shm_open(o.path(), O_RDONLY, 0)?)?;
    want("mmap(PROT_READ|PROT_WRITE, MAP_SHARED) of an O_RDONLY descriptor",
        mmap(null_mut(), 4096, PROT_READ | PROT_WRITE, MAP_SHARED, rd as i32, 0)?, EACCES)?;
    map_shared(rd, 4096, PROT_READ).map(|_| ())
}

fn shm_rdonly_ftruncate() -> CaseResult {
    let (o, _fd) = new_shm("shm", 0o600)?;
    let rd = ok("shm_open(O_RDONLY)", shm_open(o.path(), O_RDONLY, 0)?)?;
    want_any("ftruncate of an O_RDONLY descriptor", ftruncate(rd as i32, 4096)?, &[EINVAL, EBADF])
}

fn shm_permissions() -> CaseResult {
    set_umask(0)?;
    let (private, _a) = new_shm("private", 0o600)?;
    let (public, _b) = new_shm("public", 0o644)?;
    as_user(USER_A, || {
        want("shm_open(O_RDONLY) of a 0600 object by another user", shm_open(private.path(), O_RDONLY, 0)?, EACCES)?;
        ok("shm_open(O_RDONLY) of a 0644 object by another user", shm_open(public.path(), O_RDONLY, 0)?)?;
        want("shm_open(O_RDWR) of a 0644 object by another user", shm_open(public.path(), O_RDWR, 0)?, EACCES)
    })
}

fn shm_trunc() -> CaseResult {
    let (o, fd) = new_shm("shm", 0o600)?;
    zero("ftruncate", ftruncate(fd as i32, 4096)?)?;
    let fd2 = ok("shm_open(O_RDWR|O_TRUNC)", shm_open(o.path(), O_RDWR | O_TRUNC, 0)?)?;
    let size = st_size(&fstat_of(fd2)?);
    check(size == 0, &format!("after O_TRUNC the object has size {size}, expected 0"))
}

// ---------------------------------------------------------------------------
// System V IPC.

/// The Linux ABI's struct ipc64_perm, as IPC_STAT and IPC_SET pass it on 64-bit targets.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IpcPerm { key: i32, uid: u32, gid: u32, cuid: u32, cgid: u32, mode: u32, _seq: u16, _pad: u16, _unused: [u64; 2] }

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MsqidDs { perm: IpcPerm, _stime: i64, _rtime: i64, _ctime: i64, cbytes: u64, qnum: u64, qbytes: u64, lspid: i32, lrpid: i32, _unused: [u64; 2] }

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SemidDs { perm: IpcPerm, _otime: i64, _ctime: i64, nsems: u64, _unused: [u64; 2] }

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ShmidDs { perm: IpcPerm, segsz: u64, _atime: i64, _dtime: i64, _ctime: i64, cpid: i32, _lpid: i32, nattch: u64, _unused: [u64; 2] }

/// A message as msgsnd and msgrcv take it: the type, then the text.
#[repr(C)]
struct Msg { kind: i64, text: [u8; 64] }

fn new_msgq(key: i32, mode: i32) -> Result<(Made, i32), CaseError> {
    let id = ok("msgget(IPC_CREAT|IPC_EXCL)", msgget(key, IPC_CREAT | IPC_EXCL | mode)?)?;
    Ok((Made::id(Kind::MsgQueue, id), id as i32))
}

fn msg_stat(id: i32) -> Result<MsqidDs, CaseError> {
    let mut ds = MsqidDs::default();
    zero("msgctl(IPC_STAT)", msgctl(id, IPC_STAT, &mut ds as *mut MsqidDs as *mut u8)?)?;
    Ok(ds)
}

fn msg_send(id: i32, kind: i64, text: &[u8], flags: i32) -> Result<i64, CaseError> {
    let mut m = Msg { kind, text: [0; 64] };
    m.text[..text.len()].copy_from_slice(text);
    msgsnd(id, &m as *const Msg as *const u8, text.len(), flags)
}

fn msg_sent(id: i32, kind: i64, text: &[u8]) -> CaseResult {
    zero(&format!("msgsnd of type {kind}"), msg_send(id, kind, text, IPC_NOWAIT)?)
}

/// msgrcv into a buffer of `size` bytes: the length, the type and the text.
fn msg_receive(id: i32, size: usize, kind: i64, flags: i32) -> Result<(i64, i64, Vec<u8>), CaseError> {
    let mut m = Msg { kind: 0, text: [0; 64] };
    let n = msgrcv(id, &mut m as *mut Msg as *mut u8, size, kind, flags)?;
    Ok((n, m.kind, m.text[..n.clamp(0, 64) as usize].to_vec()))
}

fn msg_got(id: i32, kind: i64, want_kind: i64, want_text: &[u8]) -> CaseResult {
    let (n, got_kind, text) = msg_receive(id, 64, kind, IPC_NOWAIT)?;
    ok(&format!("msgrcv with msgtyp {kind}"), n)?;
    check(got_kind == want_kind && text == want_text, &format!(
        "msgrcv with msgtyp {kind} returned type {got_kind} {:?}, expected type {want_kind} {:?}",
        String::from_utf8_lossy(&text), String::from_utf8_lossy(want_text)))
}

fn new_semset(key: i32, nsems: i32, mode: i32) -> Result<(Made, i32), CaseError> {
    let id = ok(&format!("semget({nsems}, IPC_CREAT|IPC_EXCL)"), semget(key, nsems, IPC_CREAT | IPC_EXCL | mode)?)?;
    Ok((Made::id(Kind::SemSet, id), id as i32))
}

fn set_val(id: i32, num: i32, v: u64) -> CaseResult { zero(&format!("semctl(SETVAL {v})"), semctl(id, num, SETVAL, v)?) }
fn get_val(id: i32, num: i32) -> Result<i64, CaseError> { ok("semctl(GETVAL)", semctl(id, num, GETVAL, 0)?) }

fn sem_ops(id: i32, ops: &[SemBuf]) -> Result<i64, CaseError> { semop(id, ops.as_ptr(), ops.len()) }
fn op(num: u16, op: i16, flg: i16) -> SemBuf { SemBuf { num, op, flg } }

fn new_segment(key: i32, size: usize, mode: i32) -> Result<(Made, i32), CaseError> {
    let id = ok(&format!("shmget({size}, IPC_CREAT|IPC_EXCL)"), shmget(key, size, IPC_CREAT | IPC_EXCL | mode)?)?;
    Ok((Made::id(Kind::Segment, id), id as i32))
}

fn shm_stat(id: i32) -> Result<ShmidDs, CaseError> {
    let mut ds = ShmidDs::default();
    zero("shmctl(IPC_STAT)", shmctl(id, IPC_STAT, &mut ds as *mut ShmidDs as *mut u8)?)?;
    Ok(ds)
}

fn attach(id: i32, flags: i32) -> Result<*mut u8, CaseError> {
    let p = shmat(id, null(), flags)?;
    ok("shmat", p)?;
    Ok(p as *mut u8)
}

fn sysv_ftok() -> CaseResult {
    let f = Made::name(Kind::File, &tmp_path("key"));
    ok("creating the key file", open(f.path(), O_RDWR | O_CREAT | O_EXCL, 0o600)?)?;
    let a = ok("ftok(path, 'A')", ftok(f.path(), b'A' as i32)?)?;
    let again = ok("ftok(path, 'A') again", ftok(f.path(), b'A' as i32)?)?;
    let b = ok("ftok(path, 'B')", ftok(f.path(), b'B' as i32)?)?;
    check(a == again, &format!("ftok gave {a:#x} and then {again:#x} for the same file and id"))?;
    check(a != b, &format!("ftok gave {a:#x} for ids 'A' and 'B' alike"))
}

fn sysv_ftok_enoent() -> CaseResult {
    let path = c(&tmp_path("missing"));
    let r = ftok(path.as_ptr(), b'A' as i32)?;
    check(r < 0, &format!("ftok of a missing file returned {r:#x}, expected -1"))
}

fn sysv_key_private() -> CaseResult {
    let (_a, a) = new_msgq(IPC_PRIVATE, 0o600)?;
    let (_b, b) = new_msgq(IPC_PRIVATE, 0o600)?;
    check(a != b, &format!("two msgget(IPC_PRIVATE) calls returned the same id {a}"))
}

fn sysv_msgget() -> CaseResult {
    let key = ipc_key(1);
    let (_q, id) = new_msgq(key, 0o640)?;
    let ds = msg_stat(id)?;
    check(ds.perm.key == key, &format!("IPC_STAT reports key {:#x}, expected {key:#x}", ds.perm.key))?;
    check(ds.perm.mode & 0o777 == 0o640, &format!("IPC_STAT reports mode {:o}, expected 640", ds.perm.mode & 0o777))?;
    check(ds.perm.uid == 0 && ds.perm.cuid == 0 && ds.perm.gid == 0 && ds.perm.cgid == 0, &format!(
        "IPC_STAT reports uid {}, gid {}, cuid {} and cgid {}, expected the root creator's 0", ds.perm.uid, ds.perm.gid, ds.perm.cuid, ds.perm.cgid))?;
    check(ds.qnum == 0 && ds.qbytes > 0, &format!("IPC_STAT reports msg_qnum {} and msg_qbytes {}", ds.qnum, ds.qbytes))
}

fn sysv_msgget_excl() -> CaseResult {
    let key = ipc_key(1);
    let (_q, id) = new_msgq(key, 0o600)?;
    want("a second msgget(IPC_CREAT|IPC_EXCL) of the key", msgget(key, IPC_CREAT | IPC_EXCL | 0o600)?, EEXIST)?;
    let again = msgget(key, 0)?;
    check(again == i64::from(id), &format!("msgget of the key returned {}, expected the queue's id {id}", shown(again)))
}

fn sysv_msgget_enoent() -> CaseResult {
    want("msgget of an unused key without IPC_CREAT", msgget(ipc_key(2), 0o600)?, ENOENT)
}

fn sysv_msg_send_receive() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    msg_sent(id, 5, b"hello")?;
    let (n, kind, text) = msg_receive(id, 64, 0, IPC_NOWAIT)?;
    check(n == 5 && kind == 5 && text == b"hello", &format!("msgrcv returned {} bytes of type {kind}", shown(n)))
}

fn sysv_msg_types() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    for (kind, text) in [(3, b"a"), (1, b"b"), (2, b"c"), (1, b"d")] { msg_sent(id, kind, text)?; }
    msg_got(id, 1, 1, b"b")?;
    msg_got(id, -2, 1, b"d")?;
    msg_got(id, 0, 3, b"a")?;
    msg_got(id, 0, 2, b"c")?;
    value("messages", 4, "", Some((4, 4)));
    Ok(())
}

fn sysv_msg_nowait() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    want("msgrcv(IPC_NOWAIT) of the empty queue", msg_receive(id, 64, 0, IPC_NOWAIT)?.0, ENOMSG)
}

fn sysv_msg_full() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    let mut ds = msg_stat(id)?;
    ds.qbytes = 256;
    zero("msgctl(IPC_SET) of msg_qbytes 256", msgctl(id, IPC_SET, &mut ds as *mut MsqidDs as *mut u8)?)?;
    let text = [b'q'; 64];
    for i in 0..4 { zero(&format!("msgsnd(IPC_NOWAIT) of 64-byte message {}", i + 1), msg_send(id, 1, &text, IPC_NOWAIT)?)?; }
    want("msgsnd(IPC_NOWAIT) of a fifth 64-byte message", msg_send(id, 1, &text, IPC_NOWAIT)?, EAGAIN)?;
    let ds = msg_stat(id)?;
    value("depth", ds.qnum as i64, "", Some((4, 4)));
    check(ds.qnum == 4 && ds.cbytes == 256, &format!("IPC_STAT reports msg_qnum {} and msg_cbytes {} on the full queue, expected 4 and 256", ds.qnum, ds.cbytes))
}

fn sysv_msg_e2big() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    msg_sent(id, 1, &[b'x'; 32])?;
    want("msgrcv of a 32-byte message into 8 bytes", msg_receive(id, 8, 0, IPC_NOWAIT)?.0, E2BIG)?;
    let depth = msg_stat(id)?.qnum;
    check(depth == 1, &format!("the refused msgrcv left msg_qnum {depth}, expected 1"))?;
    let (n, _, text) = msg_receive(id, 8, 0, IPC_NOWAIT | MSG_NOERROR)?;
    check(n == 8 && text == [b'x'; 8], &format!("msgrcv(MSG_NOERROR) into 8 bytes returned {}", shown(n)))?;
    let depth = msg_stat(id)?.qnum;
    check(depth == 0, &format!("the truncated message was left on the queue: msg_qnum {depth}"))
}

fn sysv_msg_blocks() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    let pg = page()?;
    let mut receiver = spawn(|| {
        let (n, _, text) = msg_receive(id, 64, 0, 0)?;
        pg.set(0, mono());
        check(n == 4 && text == b"wake", &format!("the blocked msgrcv returned {}", shown(n)))
    })?;
    receiver.blocks("the msgrcv of the empty queue")?;
    let t0 = mono();
    msg_sent(id, 1, b"wake")?;
    receiver.finish(3000, "the receiver")?;
    woken("the blocked msgrcv", pg.get(0), t0)
}

fn sysv_msg_stat() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    let mut sender = spawn(|| msg_sent(id, 1, b"x"))?;
    let sender_pid = sender.pid;
    sender.finish(3000, "the sender")?;
    let ds = msg_stat(id)?;
    value("depth", ds.qnum as i64, "", Some((1, 1)));
    check(ds.qnum == 1, &format!("IPC_STAT reports msg_qnum {} after one msgsnd", ds.qnum))?;
    check(ds.lspid == sender_pid, &format!("IPC_STAT reports msg_lspid {}, expected the sender {sender_pid}", ds.lspid))?;
    ok("msgrcv", msg_receive(id, 64, 0, IPC_NOWAIT)?.0)?;
    let ds = msg_stat(id)?;
    check(ds.qnum == 0, &format!("IPC_STAT reports msg_qnum {} after the message was received", ds.qnum))?;
    check(ds.lrpid == pid(), &format!("IPC_STAT reports msg_lrpid {}, expected the receiver {}", ds.lrpid, pid()))
}

fn sysv_msg_set() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o644)?;
    let mut ds = msg_stat(id)?;
    ds.perm.mode = 0o600;
    ds.qbytes = 1000;
    zero("msgctl(IPC_SET)", msgctl(id, IPC_SET, &mut ds as *mut MsqidDs as *mut u8)?)?;
    let ds = msg_stat(id)?;
    check(ds.perm.mode & 0o777 == 0o600, &format!("after IPC_SET of mode 600 IPC_STAT reports {:o}", ds.perm.mode & 0o777))?;
    check(ds.qbytes == 1000, &format!("after IPC_SET of msg_qbytes 1000 IPC_STAT reports {}", ds.qbytes))
}

fn sysv_msg_rmid() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    let mut receiver = spawn(|| want("the blocked msgrcv when the queue is removed", msg_receive(id, 64, 0, 0)?.0, EIDRM))?;
    receiver.blocks("the msgrcv of the empty queue")?;
    zero("msgctl(IPC_RMID)", msgctl(id, IPC_RMID, null_mut())?)?;
    receiver.finish(3000, "the receiver")?;
    want_any("msgsnd to the removed queue", msg_send(id, 1, b"x", IPC_NOWAIT)?, &[EINVAL, EIDRM])
}

fn sysv_msg_perm() -> CaseResult {
    let (_q, id) = new_msgq(IPC_PRIVATE, 0o600)?;
    as_user(USER_A, || want("msgsnd by another user to a 0600 queue", msg_send(id, 1, b"x", IPC_NOWAIT)?, EACCES))
}

fn sysv_semget() -> CaseResult {
    let (_s, id) = new_semset(ipc_key(3), 3, 0o640)?;
    let mut ds = SemidDs::default();
    zero("semctl(IPC_STAT)", semctl(id, 0, IPC_STAT, &mut ds as *mut SemidDs as u64)?)?;
    check(ds.nsems == 3, &format!("IPC_STAT reports sem_nsems {}, expected 3", ds.nsems))?;
    check(ds.perm.mode & 0o777 == 0o640, &format!("IPC_STAT reports mode {:o}, expected 640", ds.perm.mode & 0o777))
}

fn sysv_semget_einval() -> CaseResult {
    let key = ipc_key(3);
    let (_s, id) = new_semset(key, 2, 0o600)?;
    want("semget of the key asking for 3 semaphores of a set of 2", semget(key, 3, 0)?, EINVAL)?;
    let again = semget(key, 2, 0)?;
    check(again == i64::from(id), &format!("semget of the key asking for 2 returned {}, expected {id}", shown(again)))
}

fn sysv_semctl_val() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 3, 0o600)?;
    set_val(id, 1, 5)?;
    let v = get_val(id, 1)?;
    check(v == 5, &format!("GETVAL returned {v} after SETVAL 5"))?;
    let mut vals: [u16; 3] = [1, 2, 3];
    zero("semctl(SETALL)", semctl(id, 0, SETALL, vals.as_mut_ptr() as u64)?)?;
    let mut got: [u16; 3] = [9; 3];
    zero("semctl(GETALL)", semctl(id, 0, GETALL, got.as_mut_ptr() as u64)?)?;
    check(got == vals, &format!("GETALL returned {got:?} after SETALL {vals:?}"))
}

fn sysv_semctl_erange() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    want("semctl(SETVAL 65536)", semctl(id, 0, SETVAL, 65536)?, ERANGE)
}

fn sysv_semop() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    set_val(id, 0, 0)?;
    for (o, after) in [(2, 2), (-1, 1), (-1, 0), (0, 0)] {
        zero(&format!("semop({o:+})"), sem_ops(id, &[op(0, o, IPC_NOWAIT as i16)])?)?;
        let v = get_val(id, 0)?;
        check(v == after, &format!("the value is {v} after semop({o:+}), expected {after}"))?;
    }
    Ok(())
}

fn sysv_semop_nowait() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    set_val(id, 0, 0)?;
    want("semop(-1, IPC_NOWAIT) at zero", sem_ops(id, &[op(0, -1, IPC_NOWAIT as i16)])?, EAGAIN)?;
    set_val(id, 0, 1)?;
    want("semop(0, IPC_NOWAIT) at one", sem_ops(id, &[op(0, 0, IPC_NOWAIT as i16)])?, EAGAIN)
}

fn sysv_semop_atomic() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 2, 0o600)?;
    set_val(id, 0, 1)?;
    set_val(id, 1, 0)?;
    want("semop taking from both semaphores when the second is zero",
        sem_ops(id, &[op(0, -1, IPC_NOWAIT as i16), op(1, -1, IPC_NOWAIT as i16)])?, EAGAIN)?;
    let v = get_val(id, 0)?;
    check(v == 1, &format!("the refused semop left the first semaphore at {v}, expected 1: it applied part of the operations"))
}

fn sysv_semop_blocks() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    set_val(id, 0, 0)?;
    let pg = page()?;
    let mut waiter = spawn(|| {
        let r = sem_ops(id, &[op(0, -1, 0)])?;
        pg.set(0, mono());
        zero("the blocked semop(-1)", r)
    })?;
    waiter.blocks("the semop(-1) at zero")?;
    let t0 = mono();
    zero("semop(+1)", sem_ops(id, &[op(0, 1, 0)])?)?;
    waiter.finish(3000, "the waiter")?;
    woken("the blocked semop", pg.get(0), t0)
}

fn sysv_semop_zero_blocks() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    set_val(id, 0, 1)?;
    let pg = page()?;
    let mut waiter = spawn(|| {
        let r = sem_ops(id, &[op(0, 0, 0)])?;
        pg.set(0, mono());
        zero("the blocked semop(0)", r)
    })?;
    waiter.blocks("the semop(0) at one")?;
    let t0 = mono();
    zero("semop(-1)", sem_ops(id, &[op(0, -1, 0)])?)?;
    waiter.finish(3000, "the waiter")?;
    woken("the blocked wait for zero", pg.get(0), t0)
}

fn sysv_semctl_counts() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 3, 0o600)?;
    set_val(id, 0, 0)?;
    set_val(id, 1, 1)?;
    let mut taker = spawn(|| zero("the blocked semop(-1)", sem_ops(id, &[op(0, -1, 0)])?))?;
    taker.blocks("the semop(-1) at zero")?;
    let mut zeroer = spawn(|| zero("the blocked semop(0)", sem_ops(id, &[op(1, 0, 0)])?))?;
    zeroer.blocks("the semop(0) at one")?;
    let ncnt = ok("semctl(GETNCNT)", semctl(id, 0, GETNCNT, 0)?)?;
    let zcnt = ok("semctl(GETZCNT)", semctl(id, 1, GETZCNT, 0)?)?;
    value("ncnt", ncnt, "", Some((1, 1)));
    value("zcnt", zcnt, "", Some((1, 1)));
    check(ncnt == 1 && zcnt == 1, &format!("GETNCNT reports {ncnt} and GETZCNT {zcnt}, expected one waiter each"))?;
    zero("semop(+1, -1)", sem_ops(id, &[op(0, 1, 0), op(1, -1, 0)])?)?;
    taker.finish(3000, "the waiter for a decrement")?;
    zeroer.finish(3000, "the waiter for zero")?;
    zero("semop(+1) on the third semaphore", sem_ops(id, &[op(2, 1, 0)])?)?;
    let last = ok("semctl(GETPID)", semctl(id, 2, GETPID, 0)?)?;
    check(last == i64::from(pid()), &format!("GETPID reports {last}, expected the last semop's process {}", pid()))
}

fn sysv_sem_undo() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    set_val(id, 0, 0)?;
    let pg = page()?;
    let mut child = spawn(|| {
        zero("semop(+3, SEM_UNDO)", sem_ops(id, &[op(0, 3, SEM_UNDO)])?)?;
        pg.set(0, 1);
        check(until(3000, || pg.get(1) == 1), "the case did not read the value within 3 s")
    })?;
    check(until(3000, || pg.get(0) == 1 || child.reap().is_some()), "the child did not make its semop within 3 s")?;
    let during = get_val(id, 0)?;
    pg.set(1, 1);
    child.finish(3000, "the child")?;
    check(during == 3, &format!("the value is {during} while the child lives, expected 3"))?;
    let after = get_val(id, 0)?;
    check(after == 0, &format!("the value is {after} after the child exited, expected its SEM_UNDO adjustment undone to 0"))
}

fn sysv_sem_rmid() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    set_val(id, 0, 0)?;
    let mut waiter = spawn(|| want("the blocked semop when the set is removed", sem_ops(id, &[op(0, -1, 0)])?, EIDRM))?;
    waiter.blocks("the semop(-1) at zero")?;
    zero("semctl(IPC_RMID)", semctl(id, 0, IPC_RMID, 0)?)?;
    waiter.finish(3000, "the waiter")?;
    want_any("semop on the removed set", sem_ops(id, &[op(0, 1, 0)])?, &[EINVAL, EIDRM])
}

fn sysv_sem_perm() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    as_user(USER_A, || want("semop by another user on a 0600 set", sem_ops(id, &[op(0, 1, IPC_NOWAIT as i16)])?, EACCES))
}

fn sysv_semop_efbig() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 2, 0o600)?;
    want("semop on semaphore 5 of a set of 2", sem_ops(id, &[op(5, 1, IPC_NOWAIT as i16)])?, EFBIG)
}

fn sysv_semtimedop() -> CaseResult {
    let (_s, id) = new_semset(IPC_PRIVATE, 1, 0o600)?;
    set_val(id, 0, 0)?;
    let ops = [op(0, -1, 0)];
    let timeout = ts(200 * MS);
    let start = mono();
    wait_for(200, "late");
    let r = semtimedop(id, ops.as_ptr(), ops.len(), &timeout)?;
    let end = mono();
    want("semtimedop(-1) at zero with a 200 ms timeout", r, EAGAIN)?;
    on_time("semtimedop", end, start + 200 * MS)
}

fn sysv_shmget() -> CaseResult {
    const SIZE: usize = 4096 + 100;
    let (_m, id) = new_segment(ipc_key(4), SIZE, 0o640)?;
    let ds = shm_stat(id)?;
    check(ds.segsz == SIZE as u64, &format!("IPC_STAT reports shm_segsz {}, expected {SIZE}", ds.segsz))?;
    check(ds.perm.mode & 0o777 == 0o640, &format!("IPC_STAT reports mode {:o}, expected 640", ds.perm.mode & 0o777))?;
    check(ds.nattch == 0, &format!("IPC_STAT reports shm_nattch {} for a new segment", ds.nattch))?;
    check(ds.cpid == pid(), &format!("IPC_STAT reports shm_cpid {}, expected {}", ds.cpid, pid()))
}

fn sysv_shm_zero() -> CaseResult {
    const SIZE: usize = 8192;
    let (_m, id) = new_segment(IPC_PRIVATE, SIZE, 0o600)?;
    let p = attach(id, 0)?;
    let nonzero = (0..SIZE / 8).filter(|&i| peek(p, i) != 0).count();
    check(nonzero == 0, &format!("{nonzero} words of a new segment are not zero"))
}

fn sysv_shmat_fork() -> CaseResult {
    let (_m, id) = new_segment(IPC_PRIVATE, 4096, 0o600)?;
    let p = attach(id, 0)?;
    poke(p, 0, 0xFEED);
    spawn(|| {
        check(peek(p, 0) == 0xFEED, &format!("the child read {:#x}, not the parent's store", peek(p, 0)))?;
        let n = shm_stat(id)?.nattch;
        check(n == 2, &format!("shm_nattch is {n} in the child, expected 2 after fork"))?;
        poke(p, 1, 0xBEEF);
        Ok(())
    })?
    .finish(3000, "the child")?;
    check(peek(p, 1) == 0xBEEF, &format!("the parent read {:#x}, not the child's store", peek(p, 1)))
}

fn sysv_shmat_other() -> CaseResult {
    let (_m, id) = new_segment(IPC_PRIVATE, 4096, 0o600)?;
    let pg = page()?;
    let mut other = spawn(|| {
        let q = attach(id, 0)?;
        poke(q, 0, 0xD00D);
        pg.set(0, 1);
        check(until(3000, || pg.get(1) == 1), "the case did not look within 3 s")
    })?;
    check(until(3000, || pg.get(0) == 1 || other.reap().is_some()), "the other process did not attach within 3 s")?;
    let p = attach(id, 0)?;
    let nattch = shm_stat(id)?.nattch;
    pg.set(1, 1);
    other.finish(3000, "the other process")?;
    value("nattch", nattch as i64, "", Some((2, 2)));
    check(peek(p, 0) == 0xD00D, &format!("the case read {:#x}, not the other process's store", peek(p, 0)))?;
    check(nattch == 2, &format!("shm_nattch is {nattch} with two processes attached"))
}

fn sysv_shmdt() -> CaseResult {
    let (_m, id) = new_segment(IPC_PRIVATE, 4096, 0o600)?;
    let p = attach(id, 0)?;
    zero("shmdt", shmdt(p)?)?;
    let n = shm_stat(id)?.nattch;
    check(n == 0, &format!("shm_nattch is {n} after shmdt"))?;
    let mut toucher = spawn(|| {
        let q = attach(id, 0)?;
        zero("shmdt in the child", shmdt(q)?)?;
        poke(q, 0, 1);
        fail("a store to the detached address did not fault")
    })?;
    let status = toucher.wait(3000, "the child")?;
    check(status & 0x7f == SIGSEGV, &format!("the child storing to a detached address {}, expected SIGSEGV", status_text(status)))
        .or_else(|e| toucher.outcome(status, "the child").and(Err(e)))
}

fn sysv_shm_rmid() -> CaseResult {
    let key = ipc_key(5);
    let (_m, id) = new_segment(key, 4096, 0o600)?;
    let p = attach(id, 0)?;
    zero("shmctl(IPC_RMID) while attached", shmctl(id, IPC_RMID, null_mut())?)?;
    poke(p, 0, 42);
    check(peek(p, 0) == 42, "the attached segment did not keep a store after IPC_RMID")?;
    want("shmget of the removed segment's key", shmget(key, 4096, 0)?, ENOENT)?;
    zero("shmdt", shmdt(p)?)?;
    let mut ds = ShmidDs::default();
    want_any("shmctl(IPC_STAT) after the last detach", shmctl(id, IPC_STAT, &mut ds as *mut ShmidDs as *mut u8)?, &[EINVAL, EIDRM])
}

fn sysv_shm_rdonly() -> CaseResult {
    let (_m, id) = new_segment(IPC_PRIVATE, 4096, 0o600)?;
    let mut toucher = spawn(|| {
        let q = attach(id, SHM_RDONLY)?;
        check(peek(q, 0) == 0, "the read-only attachment read a nonzero word")?;
        poke(q, 0, 1);
        fail("a store through a SHM_RDONLY attachment did not fault")
    })?;
    let status = toucher.wait(3000, "the child")?;
    check(status & 0x7f == SIGSEGV, &format!("the child storing through SHM_RDONLY {}, expected SIGSEGV", status_text(status)))
        .or_else(|e| toucher.outcome(status, "the child").and(Err(e)))
}

fn sysv_shmget_einval() -> CaseResult {
    let key = ipc_key(6);
    let (_m, id) = new_segment(key, 4096, 0o600)?;
    want("shmget of the key asking for 8192 bytes of a 4096-byte segment", shmget(key, 8192, 0)?, EINVAL)?;
    let again = shmget(key, 4096, 0)?;
    check(again == i64::from(id), &format!("shmget of the key for 4096 bytes returned {}, expected {id}", shown(again)))
}

fn sysv_shm_perm() -> CaseResult {
    let (_m, id) = new_segment(IPC_PRIVATE, 4096, 0o600)?;
    as_user(USER_A, || want("shmat by another user of a 0600 segment", shmat(id, null(), 0)?, EACCES))
}

static SUITE: Suite = suite("ipc", "IPC", &[
    category("fifos", "Named pipes (FIFOs)", &[
        case("mkfifo", "mkfifo creates a FIFO that stat reports as S_IFIFO with the mode less the umask", fifo_mkfifo),
        case("mkfifo-eexist", "mkfifo of an existing path fails with EEXIST", fifo_mkfifo_eexist),
        case("mkfifo-enoent", "mkfifo in a missing directory fails with ENOENT", fifo_mkfifo_enoent),
        case("open-read-blocks", "A blocking O_RDONLY open waits until a writer opens the FIFO", fifo_open_read_blocks),
        case("open-write-blocks", "A blocking O_WRONLY open waits until a reader opens the FIFO", fifo_open_write_blocks),
        case("open-read-nonblock", "O_RDONLY|O_NONBLOCK opens at once with no writer", fifo_open_read_nonblock),
        case("open-write-enxio", "O_WRONLY|O_NONBLOCK with no reader fails with ENXIO", fifo_open_write_enxio),
        case("open-write-nonblock", "O_WRONLY|O_NONBLOCK opens at once when a reader has the FIFO open", fifo_open_write_nonblock),
        case("fstat", "fstat of either end of an open FIFO reports S_IFIFO", fifo_fstat),
        case("lseek-espipe", "lseek on either end of a FIFO fails with ESPIPE", fifo_lseek_espipe),
        case("transfer", "256 KiB written in one process are read in order in another, then end of file", fifo_transfer),
        case("partial-read", "A read asking for more than is buffered returns what is there", fifo_partial_read),
        case("read-blocks", "A blocking read of an empty FIFO waits for a writer's data", fifo_read_blocks),
        case("read-eagain", "An O_NONBLOCK read of an empty FIFO with a writer fails with EAGAIN", fifo_read_eagain),
        case("eof-last-writer", "Read returns end of file only once the last writer has closed", fifo_eof_last_writer),
        case("eof-blocked-reader", "A reader blocked on an empty FIFO returns end of file when the last writer closes", fifo_eof_blocked_reader),
        case("eof-no-writer", "A read of a FIFO no process has open for writing returns end of file", fifo_eof_no_writer),
        case("sigpipe", "A write with no reader raises SIGPIPE", fifo_sigpipe),
        case("epipe", "With SIGPIPE ignored, a write with no reader fails with EPIPE", fifo_epipe),
        case("write-blocks-full", "A blocking write to a full FIFO waits until the reader makes room", fifo_write_blocks_full),
        case("write-eagain-full", "An O_NONBLOCK write of 512 bytes to a full FIFO fails with EAGAIN and writes nothing", fifo_write_eagain_full),
        case("atomic-writes", "Writes of 512 bytes from four writers at once are never interleaved", fifo_atomic_writes),
        case("poll-in", "poll reports POLLIN on a reader once data is buffered and not before", fifo_poll_in),
        case("poll-out", "poll reports POLLOUT on a writer with room", fifo_poll_out),
        case("poll-hup", "poll reports POLLHUP on a reader once the last writer has closed", fifo_poll_hup),
        case("poll-wakes", "A poll blocked on a reader wakes when another process writes", fifo_poll_wakes),
        case("select-read", "select reports a reader ready once data is buffered and not before", fifo_select_read),
        case("select-write", "select reports a writer with room ready", fifo_select_write),
        case("select-wakes", "A select blocked on a reader wakes when another process writes", fifo_select_wakes),
        case("unlink-open", "Unlinking a FIFO leaves its open ends working", fifo_unlink_open),
        case("reopen", "Data left when every end closed is discarded; the FIFO opens again empty", fifo_reopen),
    ]),
    category("mq", "POSIX message queues", &[
        case("open-create", "mq_open with O_CREAT|O_EXCL creates a queue with the attributes given", mq_open_create),
        case("open-eexist", "mq_open with O_CREAT|O_EXCL of an existing name fails with EEXIST", mq_open_eexist),
        case("open-enoent", "mq_open of a missing name without O_CREAT fails with ENOENT", mq_open_enoent),
        case("open-attr-einval", "mq_open with mq_maxmsg or mq_msgsize of 0 fails with EINVAL", mq_open_attr_einval),
        case("open-default-attr", "mq_open with no attributes creates a queue with positive limits", mq_open_default_attr),
        case("close", "mq_close ends the descriptor and later use fails with EBADF", mq_close_case),
        case("unlink", "mq_unlink removes the name, and a second mq_unlink fails with ENOENT", mq_unlink_case),
        case("unlink-open", "A queue unlinked while open keeps working through its descriptor", mq_unlink_open),
        case("send-receive", "mq_receive returns a sent message's bytes, length and priority", mq_send_receive),
        case("priority-order", "Messages are received highest priority first, oldest first within a priority", mq_priority_order),
        case("curmsgs", "mq_getattr counts the messages queued", mq_curmsgs),
        case("setattr", "mq_setattr changes only O_NONBLOCK and returns the old attributes", mq_setattr_case),
        case("emsgsize-receive", "mq_receive with a buffer smaller than mq_msgsize fails with EMSGSIZE and leaves the message", mq_emsgsize_receive),
        case("emsgsize-send", "mq_send of more than mq_msgsize bytes fails with EMSGSIZE", mq_emsgsize_send),
        case("prio-max", "Priority 31 is accepted, sysconf reports MQ_PRIO_MAX, and mq_send at MQ_PRIO_MAX fails with EINVAL", mq_prio_max),
        case("eagain-full", "In O_NONBLOCK mode mq_send to a full queue fails with EAGAIN", mq_eagain_full),
        case("eagain-empty", "In O_NONBLOCK mode mq_receive of an empty queue fails with EAGAIN", mq_eagain_empty),
        case("ebadf-mode", "mq_send on a read-only descriptor and mq_receive on a write-only one fail with EBADF", mq_ebadf_mode),
        case("timedreceive-timeout", "mq_timedreceive of an empty queue fails with ETIMEDOUT at its deadline", mq_timedreceive_timeout),
        case("timedsend-timeout", "mq_timedsend to a full queue fails with ETIMEDOUT at its deadline", mq_timedsend_timeout),
        case("timed-einval", "mq_timedreceive that would block with tv_nsec out of range fails with EINVAL", mq_timed_einval),
        case("timed-ready", "mq_timedreceive returns a queued message even when its deadline has passed", mq_timed_ready),
        case("receive-blocks", "A receiver blocked on an empty queue wakes when another process sends", mq_receive_blocks),
        case("send-blocks", "A sender blocked on a full queue wakes when another process receives", mq_send_blocks),
        case("notify-signal", "mq_notify with SIGEV_SIGNAL sends the signal with SI_MESGQ and the value when a message arrives", mq_notify_signal),
        case("notify-once", "A notification is removed once sent and can be registered again", mq_notify_once),
        case("notify-ebusy", "mq_notify from a second process while one is registered fails with EBUSY", mq_notify_ebusy),
        case("notify-receiver-waiting", "No notification is sent while a receiver waits, and the registration stays", mq_notify_receiver_waiting),
        case("notify-remove", "mq_notify with no sigevent removes the registration", mq_notify_remove),
        case("fork-shared", "A queue descriptor inherited across fork reaches the same queue", mq_fork_shared),
        case("name-shared", "A process that opens the queue by name exchanges messages with its creator", mq_name_shared),
        case("permissions", "Another user's mq_open of a 0600 queue fails with EACCES, and of a 0644 queue opens it read-only only", mq_permissions),
    ]),
    category("semaphores", "POSIX semaphores", &[
        case("init", "sem_init sets the value sem_getvalue reports", sem_init_case),
        case("init-einval", "sem_init with a value above SEM_VALUE_MAX fails with EINVAL", sem_init_einval),
        case("post-wait", "sem_post adds one and sem_wait takes one", sem_post_wait),
        case("trywait", "sem_trywait at zero fails with EAGAIN and leaves the value", sem_trywait_case),
        case("wait-blocks", "sem_wait at zero waits for a sem_post from another process", sem_wait_blocks),
        case("timedwait-timeout", "sem_timedwait at zero fails with ETIMEDOUT at its deadline", sem_timedwait_timeout),
        case("timedwait-einval", "sem_timedwait at zero with tv_nsec out of range fails with EINVAL", sem_timedwait_einval),
        case("timedwait-ready", "sem_timedwait takes an available semaphore even when its deadline has passed", sem_timedwait_ready),
        case("eintr", "A caught signal interrupts sem_wait with EINTR", sem_eintr),
        case("destroy", "sem_destroy of a semaphore no one waits on returns 0", sem_destroy_case),
        case("pshared-fork", "An unnamed semaphore with pshared set in shared memory carries 100 posts across fork", sem_pshared_fork),
        case("open-create", "sem_open with O_CREAT|O_EXCL creates a named semaphore with the value given", sem_open_create),
        case("open-eexist", "sem_open with O_CREAT|O_EXCL of an existing name fails with EEXIST", sem_open_eexist),
        case("open-enoent", "sem_open of a missing name without O_CREAT fails with ENOENT", sem_open_enoent),
        case("open-same", "Opening one name twice in a process returns the same address", sem_open_same),
        case("open-einval", "sem_open with O_CREAT and a value above SEM_VALUE_MAX fails with EINVAL", sem_open_einval_value),
        case("named-processes", "A named semaphore opened by name in another process is the same semaphore", sem_named_processes),
        case("close", "sem_close returns 0 and the name still opens", sem_close_case),
        case("unlink", "sem_unlink removes the name, open handles keep working, and a second sem_unlink fails with ENOENT", sem_unlink_case),
        case("permissions", "Another user's sem_open of a 0600 semaphore fails with EACCES", sem_permissions),
        case("getvalue-waiters", "sem_getvalue with a waiter reports zero or a negative count", sem_getvalue_waiters),
        case("contention-cpus", "Processes on several processors posting and waiting lose no wakeups", sem_contention_cpus),
        case("mutex-cpus", "A semaphore of value 1 keeps processes on several processors out of each other's critical section", sem_mutex_cpus),
    ]),
    category("shm", "POSIX shared memory", &[
        case("open-create", "shm_open with O_CREAT|O_EXCL creates an empty object with the mode less the umask", shm_open_create),
        case("open-eexist", "shm_open with O_CREAT|O_EXCL of an existing name fails with EEXIST", shm_open_eexist),
        case("open-enoent", "shm_open of a missing name without O_CREAT fails with ENOENT", shm_open_enoent),
        case("ftruncate", "ftruncate sizes the object and the new bytes read as zero", shm_ftruncate),
        case("map-fork", "Stores through a MAP_SHARED mapping are seen across fork both ways", shm_map_fork),
        case("map-name", "A process that opens the object by name sees its creator's stores, and the creator sees its", shm_map_name),
        case("persist", "An object keeps its size and contents with no descriptor or mapping until it is unlinked", shm_persist),
        case("close-keeps-mapping", "A mapping stays usable after its descriptor is closed", shm_close_keeps_mapping),
        case("unlink", "shm_unlink removes the name, mappings keep working, and a second shm_unlink fails with ENOENT", shm_unlink_case),
        case("unlink-recreate", "After shm_unlink, shm_open with O_CREAT|O_EXCL makes a new empty object", shm_unlink_recreate),
        case("rdonly-map", "A writable MAP_SHARED mapping of an O_RDONLY descriptor fails with EACCES", shm_rdonly_map),
        case("rdonly-ftruncate", "ftruncate of an O_RDONLY descriptor fails with EINVAL or EBADF", shm_rdonly_ftruncate),
        case("permissions", "Another user's shm_open of a 0600 object fails with EACCES, and of a 0644 object opens it read-only only", shm_permissions),
        case("trunc", "shm_open with O_TRUNC sets an existing object's size to 0", shm_trunc),
    ]),
    category("sysv", "System V IPC", &[
        case("ftok", "ftok gives the same key for one file and id and different keys for different ids", sysv_ftok),
        case("ftok-enoent", "ftok of a missing file returns -1", sysv_ftok_enoent),
        case("key-private", "msgget of IPC_PRIVATE makes a new queue every time", sysv_key_private),
        case("msgget", "msgget with IPC_CREAT makes a queue IPC_STAT reports with its key, mode, owner and no messages", sysv_msgget),
        case("msgget-excl", "msgget with IPC_CREAT|IPC_EXCL of a used key fails with EEXIST, and without it returns the queue", sysv_msgget_excl),
        case("msgget-enoent", "msgget of an unused key without IPC_CREAT fails with ENOENT", sysv_msgget_enoent),
        case("msg-send-receive", "msgrcv returns a message's type, bytes and length", sysv_msg_send_receive),
        case("msg-types", "msgrcv with msgtyp 0, positive and negative takes the message POSIX names", sysv_msg_types),
        case("msg-nowait", "msgrcv with IPC_NOWAIT of an empty queue fails with ENOMSG", sysv_msg_nowait),
        case("msg-full", "msgsnd with IPC_NOWAIT to a queue at msg_qbytes fails with EAGAIN", sysv_msg_full),
        case("msg-e2big", "msgrcv into a short buffer fails with E2BIG, and MSG_NOERROR truncates", sysv_msg_e2big),
        case("msg-blocks", "msgrcv blocked on an empty queue wakes when another process sends", sysv_msg_blocks),
        case("msg-stat", "IPC_STAT reports msg_qnum, msg_lspid and msg_lrpid", sysv_msg_stat),
        case("msg-set", "IPC_SET changes the mode and msg_qbytes IPC_STAT then reports", sysv_msg_set),
        case("msg-rmid", "IPC_RMID wakes a blocked msgrcv with EIDRM and the id stops working", sysv_msg_rmid),
        case("msg-perm", "Another user's msgsnd to a 0600 queue fails with EACCES", sysv_msg_perm),
        case("semget", "semget makes a set IPC_STAT reports with its number of semaphores and mode", sysv_semget),
        case("semget-einval", "semget of an existing key asking for more semaphores than the set has fails with EINVAL", sysv_semget_einval),
        case("semctl-val", "SETVAL and GETVAL, SETALL and GETALL set and read the values", sysv_semctl_val),
        case("semctl-erange", "semctl SETVAL above the largest semaphore value fails with ERANGE", sysv_semctl_erange),
        case("semop", "semop adds to, takes from and waits for zero on a semaphore", sysv_semop),
        case("semop-nowait", "A semop that would wait fails with EAGAIN under IPC_NOWAIT", sysv_semop_nowait),
        case("semop-atomic", "A semop of several operations applies all of them or none", sysv_semop_atomic),
        case("semop-blocks", "A semop waiting to take from a semaphore wakes when another process adds", sysv_semop_blocks),
        case("semop-zero-blocks", "A semop waiting for zero wakes when the value reaches zero", sysv_semop_zero_blocks),
        case("semctl-counts", "GETNCNT and GETZCNT count waiting processes, and GETPID names the last semop's process", sysv_semctl_counts),
        case("sem-undo", "SEM_UNDO adjustments are undone when the process exits", sysv_sem_undo),
        case("sem-rmid", "IPC_RMID wakes a blocked semop with EIDRM and the id stops working", sysv_sem_rmid),
        case("sem-perm", "Another user's semop on a 0600 set fails with EACCES", sysv_sem_perm),
        case("semop-efbig", "semop on a semaphore number past the set's size fails with EFBIG", sysv_semop_efbig),
        case("semtimedop", "Linux ABI: semtimedop that would wait fails with EAGAIN when its timeout passes", sysv_semtimedop),
        case("shmget", "shmget makes a segment IPC_STAT reports with its size, mode, creator and no attachments", sysv_shmget),
        case("shm-zero", "A new segment reads as zero", sysv_shm_zero),
        case("shmat-fork", "A segment attached before fork is attached in the child, and stores are seen both ways", sysv_shmat_fork),
        case("shmat-other", "A process that attaches the segment by id sees another's stores, and shm_nattch counts both", sysv_shmat_other),
        case("shmdt", "shmdt detaches: shm_nattch drops and a store to the address faults", sysv_shmdt),
        case("shm-rmid", "IPC_RMID removes an attached segment's key at once and leaves its memory usable until the last detach", sysv_shm_rmid),
        case("shm-rdonly", "A store through a SHM_RDONLY attachment faults", sysv_shm_rdonly),
        case("shmget-einval", "shmget of an existing key asking for more than the segment's size fails with EINVAL", sysv_shmget_einval),
        case("shm-perm", "Another user's shmat of a 0600 segment fails with EACCES", sysv_shm_perm),
    ]),
]);

fn main() { SUITE.run() }
