//! The SDK suite's no_std app, on libbreenix alone. libbreenix's runtime supplies
//! `_start`, which calls `main(argc, argv)` and exits with what it returns.
//!
//!   rust-nostd-app hello      prints "hello from rust-nostd-app"
//!   rust-nostd-app syscalls   checks getpid, a file round trip and the monotonic clock
//!   rust-nostd-app exit N     returns N from main
#![no_std]
#![no_main]

use libbreenix::{fs, io, process, time};

fn arg(argv: *const *const u8, index: usize) -> &'static [u8] {
    // SAFETY: the kernel passes argc valid NUL-terminated argument strings.
    unsafe {
        let start = *argv.add(index);
        let mut len = 0;
        while *start.add(len) != 0 {
            len += 1;
        }
        core::slice::from_raw_parts(start, len)
    }
}

fn parse(text: &[u8]) -> Option<i32> {
    let mut value: i32 = 0;
    if text.is_empty() {
        return None;
    }
    for &byte in text {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(i32::from(byte - b'0'))?;
    }
    Some(value)
}

fn syscalls() -> i32 {
    match process::getpid() {
        Ok(pid) if pid.raw() > 0 => io::println("pid ok"),
        _ => {
            io::println("getpid failed");
            return 1;
        }
    }
    let path = "nostd.txt";
    let Ok(fd) = fs::open_with_mode(path, fs::O_CREAT | fs::O_RDWR | fs::O_TRUNC, 0o644) else {
        io::println("open nostd.txt failed");
        return 1;
    };
    let wrote = fs::write(fd, b"no std here");
    let rewound = fs::lseek(fd, 0, 0);
    let mut buf = [0u8; 32];
    let read = fs::read(fd, &mut buf);
    let _ = io::close(fd);
    let _ = fs::unlink(path);
    match (wrote, rewound, read) {
        (Ok(11), Ok(0), Ok(11)) if &buf[..11] == b"no std here" => io::println("file ok"),
        _ => {
            io::println("file round trip failed");
            return 1;
        }
    }
    let (Ok(first), Ok(second)) = (time::now_monotonic(), time::now_monotonic()) else {
        io::println("clock_gettime failed");
        return 1;
    };
    if (second.tv_sec, second.tv_nsec) >= (first.tv_sec, first.tv_nsec) && first.tv_sec >= 0 {
        io::println("clock ok");
        0
    } else {
        io::println("the monotonic clock went backwards");
        1
    }
}

#[no_mangle]
pub extern "C" fn main(argc: usize, argv: *const *const u8) -> i32 {
    if argc < 2 {
        io::println("usage: rust-nostd-app hello|syscalls|exit N");
        return 2;
    }
    match arg(argv, 1) {
        b"hello" => {
            io::println("hello from rust-nostd-app");
            0
        }
        b"syscalls" => syscalls(),
        b"exit" if argc > 2 => parse(arg(argv, 2)).unwrap_or(2),
        _ => {
            io::println("unknown command");
            2
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    io::println("rust-nostd-app panicked");
    process::exit(101)
}
