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
pub unsafe fn realloc(p: *mut u8, n: usize) -> *mut u8 {
    host_realloc(p, n)
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
mod stdio;

pub fn __errno_location() -> *mut i32 {
    pthread::location()
}
