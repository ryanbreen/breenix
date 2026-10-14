#![feature(c_variadic)]
use std::cell::Cell;
mod pthread {
    use super::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};
    thread_local! {static ERROR:Cell<i32>=const{Cell::new(0)};static ID:u64=NEXT.fetch_add(1,Ordering::Relaxed);}
    static NEXT: AtomicU64 = AtomicU64::new(1);
    pub fn set_errno(e: i32) {
        ERROR.with(|s| s.set(e));
    }
    pub fn location() -> *mut i32 {
        ERROR.with(|s| s.as_ptr())
    }
    pub fn errno() -> i32 {
        ERROR.with(|s| s.get())
    }
    thread_local! {static LOCALE: Cell<*mut core::ffi::c_void> = const { Cell::new(std::ptr::null_mut()) };}
    /// This thread's uselocale slot; null selects the process locale. Called by the
    /// locale service's objects module, which only the integrated tree has.
    #[allow(dead_code)]
    pub unsafe fn locale_slot() -> *mut *mut core::ffi::c_void {
        LOCALE.with(|s| s.as_ptr())
    }
    pub struct RecursiveLock {
        owner: AtomicU64,
        depth: std::cell::UnsafeCell<usize>,
    }
    unsafe impl Sync for RecursiveLock {}
    impl RecursiveLock {
        pub const fn new() -> Self {
            Self {
                owner: AtomicU64::new(0),
                depth: std::cell::UnsafeCell::new(0),
            }
        }
        pub fn try_lock(&self) -> bool {
            let id = ID.with(|&x| x);
            if self.owner.load(Ordering::Relaxed) == id
                || self
                    .owner
                    .compare_exchange(0, id, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                unsafe {
                    *self.depth.get() += 1;
                }
                true
            } else {
                false
            }
        }
        pub fn lock(&self) {
            while !self.try_lock() {
                std::thread::yield_now();
            }
        }
        pub fn unlock(&self) {
            unsafe {
                *self.depth.get() -= 1;
                if *self.depth.get() == 0 {
                    self.owner.store(0, Ordering::Release);
                }
            }
        }
    }
}
extern "C" {
    #[link_name = "malloc"]
    fn host_malloc(n: usize) -> *mut u8;
    #[link_name = "calloc"]
    fn host_calloc(n: usize, s: usize) -> *mut u8;
    #[link_name = "realloc"]
    fn host_realloc(p: *mut u8, n: usize) -> *mut u8;
    #[link_name = "free"]
    fn host_free(p: *mut u8);
    #[link_name = "read"]
    fn host_read(fd: i32, p: *mut u8, n: usize) -> isize;
    #[link_name = "write"]
    fn host_write(fd: i32, p: *const u8, n: usize) -> isize;
    #[link_name = "close"]
    fn host_close(fd: i32) -> i32;
    #[link_name = "fcntl"]
    fn host_fcntl(fd: i32, cmd: i32, ...) -> i32;
    #[link_name = "open"]
    fn host_open(p: *const u8, flags: i32, ...) -> i32;
    #[link_name = "lseek"]
    fn host_lseek(fd: i32, o: i64, w: i32) -> i64;
    #[link_name = "unlink"]
    fn host_unlink(p: *const u8) -> i32;
    #[link_name = "dup2"]
    fn host_dup2(a: i32, b: i32) -> i32;
    #[link_name = "isatty"]
    fn host_isatty(fd: i32) -> i32;
    #[link_name = "getpid"]
    fn host_getpid() -> i32;
    #[link_name = "__error"]
    fn host_error() -> *mut i32;
}
unsafe fn capture<T: Copy + PartialOrd + From<i8>>(r: T) -> T {
    if r < T::from(0) {
        pthread::set_errno(*host_error());
    }
    r
}
pub unsafe fn malloc(n: usize) -> *mut u8 {
    host_malloc(n)
}
pub unsafe fn calloc(n: usize, s: usize) -> *mut u8 {
    host_calloc(n, s)
}
thread_local! {static FAIL_REALLOC:Cell<bool>=const{Cell::new(false)};}
pub unsafe fn realloc(p: *mut u8, n: usize) -> *mut u8 {
    if FAIL_REALLOC.with(|f| f.replace(false)) {
        pthread::set_errno(12);
        std::ptr::null_mut()
    } else {
        host_realloc(p, n)
    }
}
pub unsafe fn free(p: *mut u8) {
    host_free(p)
}
pub unsafe fn read(fd: i32, p: *mut u8, n: usize) -> isize {
    capture(host_read(fd, p, n))
}
pub unsafe fn write(fd: i32, p: *const u8, n: usize) -> isize {
    capture(host_write(fd, p, n))
}
pub unsafe fn close(fd: i32) -> i32 {
    capture(host_close(fd))
}
pub unsafe fn fcntl(fd: i32, cmd: i32, a: u64) -> i32 {
    let (cmd, a) = match cmd {
        3 => (3, 0),
        4 => (4, if a & 1024 != 0 { 8 } else { 0 }),
        2 => (2, a),
        _ => (cmd, a),
    };
    let v = capture(host_fcntl(fd, cmd, a));
    if cmd == 3 && v >= 0 {
        (v & 3) | if v & 8 != 0 { 1024 } else { 0 }
    } else {
        v
    }
}
pub unsafe fn open(p: *const u8, flags: i32, mode: u32) -> i32 {
    let translated = (flags & 3)
        | if flags & 64 != 0 { 0x200 } else { 0 }
        | if flags & 128 != 0 { 0x800 } else { 0 }
        | if flags & 512 != 0 { 0x400 } else { 0 }
        | if flags & 1024 != 0 { 8 } else { 0 };
    capture(host_open(p, translated, mode))
}
pub unsafe fn lseek(fd: i32, o: i64, w: i32) -> i64 {
    capture(host_lseek(fd, o, w))
}
pub unsafe fn unlink(p: *const u8) -> i32 {
    capture(host_unlink(p))
}
pub unsafe fn dup2(a: i32, b: i32) -> i32 {
    capture(host_dup2(a, b))
}
pub unsafe fn isatty(fd: i32) -> i32 {
    host_isatty(fd)
}
pub fn getpid() -> i32 {
    unsafe { host_getpid() }
}
// The harness includes complete public APIs; each focused case exercises a subset.
#[cfg(test)]
mod cases;
#[allow(dead_code)]
mod locale;
#[allow(dead_code)]
mod math;
#[allow(dead_code)]
mod stdio;
#[allow(dead_code)]
mod stdlib;
#[allow(dead_code)]
mod string;

pub fn __errno_location() -> *mut i32 {
    pthread::location()
}

extern "C" {
    #[link_name = "pipe"]
    fn host_pipe(p: *mut i32) -> i32;
    #[link_name = "fork"]
    fn host_fork() -> i32;
    #[link_name = "execve"]
    fn host_execve(p: *const u8, a: *const *const u8, e: *const *const u8) -> i32;
    #[link_name = "waitpid"]
    fn host_waitpid(pid: i32, s: *mut i32, o: i32) -> i32;
    #[link_name = "rmdir"]
    fn host_rmdir(p: *const u8) -> i32;
    #[link_name = "_exit"]
    fn host_exit(s: i32) -> !;
    #[link_name = "_NSGetEnviron"]
    fn host_environ() -> *mut *mut *mut u8;
}
pub unsafe fn pipe2(p: *mut i32, flags: i32) -> i32 {
    let r = capture(host_pipe(p));
    if r == 0 && flags & 0x80000 != 0 {
        host_fcntl(*p, 2, 1i32);
        host_fcntl(*p.add(1), 2, 1i32);
    }
    r
}
pub fn fork() -> i32 {
    unsafe { capture(host_fork()) }
}
pub unsafe fn execve(p: *const u8, a: *const *const u8, e: *const *const u8) -> i32 {
    capture(host_execve(p, a, e))
}
pub unsafe fn waitpid(pid: i32, s: *mut i32, o: i32) -> i32 {
    capture(host_waitpid(pid, s, o))
}
pub unsafe fn rmdir(p: *const u8) -> i32 {
    capture(host_rmdir(p))
}
pub fn _exit(s: i32) -> ! {
    unsafe { host_exit(s) }
}
#[allow(non_upper_case_globals)]
mod env {
    pub static mut environ: *mut *mut u8 = std::ptr::null_mut();
    /// Host tests run in the C locale whatever the host environment says. Called by
    /// the locale service's objects module, which only the integrated tree has.
    #[allow(dead_code)]
    pub fn getenv_bytes(_name: &[u8]) -> Option<&'static [u8]> {
        None
    }
}
extern "C" {
    #[link_name = "sched_yield"]
    fn host_sched_yield() -> i32;
    #[link_name = "getentropy"]
    fn host_getentropy(p: *mut u8, n: usize) -> i32;
    #[link_name = "lstat"]
    fn host_lstat(p: *const u8, s: *mut u8) -> i32;
    #[link_name = "renameat"]
    fn host_renameat(a: i32, p: *const u8, b: i32, q: *const u8) -> i32;
}
pub fn sched_yield() -> i32 {
    unsafe { host_sched_yield() }
}
pub unsafe fn getrandom(p: *mut u8, n: usize, _flags: u32) -> isize {
    if capture(host_getentropy(p, n)) == 0 {
        n as isize
    } else {
        -1
    }
}
pub unsafe fn lstat(p: *const u8, s: *mut u8) -> i32 {
    capture(host_lstat(p, s))
}
/// Only renameat goes through syscall in the stdio module; the host's AT_FDCWD
/// differs from Linux's -100, so it is translated.
pub unsafe fn syscall(n: i64, a: i64, b: i64, c: i64, d: i64, _: i64, _: i64) -> i64 {
    assert_eq!(n, 38);
    let dir = |fd: i64| if fd == -100 { -2 } else { fd as i32 };
    capture(host_renameat(
        dir(a),
        b as *const u8,
        dir(c),
        d as *const u8,
    )) as i64
}
extern "C" {
    #[link_name = "clock_gettime"]
    fn host_clock_gettime(clock: u32, t: *mut u8) -> i32;
}
/// The locale service's clocks, translated from Linux's numbering to the host's.
pub unsafe fn clock_gettime(clock: i32, t: *mut u8) -> i32 {
    let host = match clock {
        1 => 6,  // CLOCK_MONOTONIC
        2 => 12, // CLOCK_PROCESS_CPUTIME_ID
        c => c as u32,
    };
    capture(host_clock_gettime(host, t))
}
