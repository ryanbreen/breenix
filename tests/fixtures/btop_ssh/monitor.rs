extern crate self as libbreenix;

use std::cell::RefCell;

struct Terminal {
    key: u8,
    input_polled: bool,
    mode: u32,
    output: Vec<u8>,
}
thread_local! {
    static TERMINAL: RefCell<Terminal> = const { RefCell::new(Terminal {
        key: 0, input_polled: false, mode: INITIAL_MODE, output: Vec::new(),
    }) };
}
const INITIAL_MODE: u32 = termios::lflag::ICANON | termios::lflag::ECHO | termios::lflag::ISIG;

pub mod types {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Fd(u64);
    impl Fd {
        pub const STDIN: Self = Self(0);
        pub const STDOUT: Self = Self(1);
        pub fn from_raw(n: u64) -> Self { Self(n) }
    }
    pub struct Timespec { pub tv_sec: i64, pub tv_nsec: i64 }
}
pub mod error {
    pub enum Error { Os(super::errno::Errno) }
}
pub mod errno {
    pub enum Errno { EINTR }
}
pub mod fs {
    pub fn open(_: &str, _: u32) -> Result<crate::types::Fd, ()> { Err(()) }
}
pub mod time {
    use crate::types::Timespec;
    pub fn now_monotonic() -> Result<Timespec, ()> {
        Ok(Timespec { tv_sec: 1, tv_nsec: 0 })
    }
    pub fn nanosleep(_: &Timespec) -> Result<(), ()> {
        panic!("monitor waited for another refresh without polling the PTY")
    }
    pub fn sleep_ms(_: u64) -> Result<(), ()> { Ok(()) }
}
pub mod process {
    pub fn yield_now() -> Result<(), ()> { Ok(()) }
}
pub mod io {
    use crate::{error::Error, types::Fd, TERMINAL};
    pub mod poll_events {
        pub const POLLIN: i16 = 1;
        pub const POLLERR: i16 = 8;
        pub const POLLHUP: i16 = 16;
        pub const POLLNVAL: i16 = 32;
    }
    pub struct PollFd { pub revents: i16 }
    impl PollFd {
        pub fn new(_: Fd, _: i16) -> Self { Self { revents: 0 } }
    }
    pub fn poll(fds: &mut [PollFd], _: i32) -> Result<usize, Error> {
        TERMINAL.with_borrow_mut(|tty| {
            assert!(!tty.input_polled, "monitor missed its quit byte");
            assert_eq!(tty.mode & crate::INITIAL_MODE, 0, "quit must reach the reader as a byte");
            assert!(!tty.output.is_empty(), "first frame must be visible before waiting");
            tty.input_polled = true;
        });
        fds[0].revents = poll_events::POLLIN;
        Ok(1)
    }
    pub fn read(fd: Fd, bytes: &mut [u8]) -> Result<usize, Error> {
        assert!(fd == Fd::STDIN);
        TERMINAL.with_borrow(|tty| bytes[0] = tty.key);
        Ok(1)
    }
    pub fn write(fd: Fd, bytes: &[u8]) -> Result<usize, Error> {
        assert!(fd == Fd::STDOUT);
        TERMINAL.with_borrow_mut(|tty| tty.output.extend_from_slice(bytes));
        Ok(bytes.len())
    }
    pub fn close(_: Fd) -> Result<(), Error> { Ok(()) }
}
pub mod termios {
    use crate::{types::Fd, TERMINAL};
    pub mod lflag {
        pub const ICANON: u32 = 1;
        pub const ECHO: u32 = 2;
        pub const ISIG: u32 = 4;
        pub const ECHONL: u32 = 8;
    }
    pub mod iflag { pub const IXON: u32 = 1; }
    pub mod cc { pub const VMIN: usize = 0; pub const VTIME: usize = 1; }
    pub const TCSANOW: i32 = 0;
    #[derive(Clone, Copy, Default)]
    pub struct Termios { pub c_lflag: u32, pub c_iflag: u32, pub c_cc: [u8; 32] }
    #[derive(Clone, Copy, Default)]
    pub struct Winsize { pub ws_col: u16, pub ws_row: u16 }
    pub fn get_winsize(_: Fd) -> Result<Winsize, ()> {
        Ok(Winsize { ws_col: 17, ws_row: 5 })
    }
    pub fn tcgetattr(_: Fd, out: &mut Termios) -> Result<(), ()> {
        TERMINAL.with_borrow(|tty| out.c_lflag = tty.mode);
        Ok(())
    }
    pub fn tcsetattr(_: Fd, _: i32, attrs: &Termios) -> Result<(), ()> {
        TERMINAL.with_borrow_mut(|tty| tty.mode = attrs.c_lflag);
        Ok(())
    }
}
mod monitor {
    // @MONITOR@
    pub fn run() { main(); }
}

fn main() {
    for key in [b'q', 3] {
        TERMINAL.with_borrow_mut(|tty| {
            tty.key = key;
            tty.input_polled = false;
            tty.output.clear();
        });
        monitor::run();
        TERMINAL.with_borrow(|tty| {
            assert!(tty.input_polled);
            assert_eq!(tty.mode, INITIAL_MODE);
            let mut text = Vec::new();
            let mut bytes = tty.output.iter().copied();
            while let Some(byte) = bytes.next() {
                if byte == 27 {
                    assert_eq!(bytes.next(), Some(b'['));
                    for end in bytes.by_ref() {
                        if (0x40..=0x7e).contains(&end) { break; }
                    }
                } else if byte != b'\r' {
                    text.push(byte);
                }
            }
            assert!(text.starts_with(b"btop - Breenix Sy"), "{text:?}");
            let lines: Vec<_> = text.split(|&b| b == b'\n').collect();
            assert!(lines.len() <= 5, "frame would scroll the terminal: {lines:?}");
            assert!(lines.iter().all(|line| line.len() <= 17), "{lines:?}");
        });
    }
}
