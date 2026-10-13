//! Host harness for the C library's terminal functions
//! (libs/libbreenix-libc/src/termios.rs), whose unit tests cannot run in
//! their own crate: libbreenix-libc defines a panic handler and the C entry
//! points, so it does not build as a host test.
//!
//! The libc module is compiled here unchanged, against libbreenix's real
//! termios, error and errno modules. Syscalls are stubbed: `ioctl` succeeds
//! without effect, so a -1 from a wrapper can only be its own validation,
//! and errno is per thread, as in the library.

extern crate self as libbreenix;

// The libc module uses only libbreenix's termios types; the rest of the
// included libbreenix API, and the syscall stubs it is written against, go
// unused here.
#[allow(dead_code)]
mod syscall {
    pub mod nr {
        pub const CLOSE: u64 = 57;
        pub const IOCTL: u64 = 29;
    }
    pub mod raw {
        pub unsafe fn syscall1(_: u64, _: u64) -> u64 {
            0
        }
        pub unsafe fn syscall3(_: u64, _: u64, _: u64, _: u64) -> u64 {
            0
        }
    }
}

#[path = "../libs/libbreenix/src/errno.rs"]
#[allow(dead_code)]
mod errno;
#[path = "../libs/libbreenix/src/error.rs"]
#[allow(dead_code)]
mod error;
#[path = "../libs/libbreenix/src/types.rs"]
#[allow(dead_code)]
mod types;
#[path = "../libs/libbreenix/src/termios.rs"]
#[allow(dead_code)]
mod termios;

thread_local! {
    static ERRNO: std::cell::UnsafeCell<i32> = const { std::cell::UnsafeCell::new(0) };
}

fn __errno_location() -> *mut i32 {
    ERRNO.with(|errno| errno.get())
}

unsafe fn ioctl(_: i32, _: u64, _: u64) -> i32 {
    0
}

#[path = "../libs/libbreenix-libc/src/termios.rs"]
mod libc_termios;
