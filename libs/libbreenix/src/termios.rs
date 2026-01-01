//! Terminal I/O control for userspace
//!
//! Provides tcgetattr(), tcsetattr(), and related functions.

use crate::syscall::raw;

/// Syscall number for ioctl
pub const SYS_IOCTL: u64 = 16;

/// ioctl request codes
pub mod request {
    pub const TCGETS: u64 = 0x5401;
    pub const TCSETS: u64 = 0x5402;
    pub const TCSETSW: u64 = 0x5403;
    pub const TCSETSF: u64 = 0x5404;
    pub const TIOCGPGRP: u64 = 0x540F;
    pub const TIOCSPGRP: u64 = 0x5410;
    pub const TIOCGWINSZ: u64 = 0x5413;
}

/// tcsetattr action values
pub const TCSANOW: i32 = 0;
pub const TCSADRAIN: i32 = 1;
pub const TCSAFLUSH: i32 = 2;

/// Local mode flags
pub mod lflag {
    pub const ISIG: u32 = 0x0001;
    pub const ICANON: u32 = 0x0002;
    pub const ECHO: u32 = 0x0008;
    pub const ECHOE: u32 = 0x0010;
    pub const ECHOK: u32 = 0x0020;
    pub const ECHONL: u32 = 0x0040;
    pub const IEXTEN: u32 = 0x8000;
}

/// Input mode flags
pub mod iflag {
    pub const ICRNL: u32 = 0x0100;
    pub const IXON: u32 = 0x0400;
}

/// Output mode flags
pub mod oflag {
    pub const OPOST: u32 = 0x0001;
    pub const ONLCR: u32 = 0x0004;
}

/// Control character indices
pub mod cc {
    pub const VINTR: usize = 0;
    pub const VQUIT: usize = 1;
    pub const VERASE: usize = 2;
    pub const VKILL: usize = 3;
    pub const VEOF: usize = 4;
    pub const VMIN: usize = 6;
    pub const VTIME: usize = 5;
    pub const VSUSP: usize = 10;
    pub const NCCS: usize = 32;
}

/// Terminal attributes structure (must match kernel's Termios)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; cc::NCCS],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

impl Default for Termios {
    fn default() -> Self {
        Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; cc::NCCS],
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }
}

/// Get terminal attributes
pub fn tcgetattr(fd: i32, termios: &mut Termios) -> Result<(), i32> {
    let ret = unsafe {
        raw::syscall3(SYS_IOCTL, fd as u64, request::TCGETS, termios as *mut _ as u64)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Set terminal attributes
pub fn tcsetattr(fd: i32, action: i32, termios: &Termios) -> Result<(), i32> {
    let request = match action {
        TCSANOW => request::TCSETS,
        TCSADRAIN => request::TCSETSW,
        TCSAFLUSH => request::TCSETSF,
        _ => return Err(22), // EINVAL
    };

    let ret = unsafe {
        raw::syscall3(SYS_IOCTL, fd as u64, request, termios as *const _ as u64)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Check if fd refers to a terminal
pub fn isatty(fd: i32) -> bool {
    let mut termios = Termios::default();
    tcgetattr(fd, &mut termios).is_ok()
}

/// Make raw mode termios settings
pub fn cfmakeraw(termios: &mut Termios) {
    termios.c_iflag &= !(iflag::ICRNL | iflag::IXON);
    termios.c_oflag &= !oflag::OPOST;
    termios.c_lflag &= !(lflag::ECHO | lflag::ECHONL | lflag::ICANON | lflag::ISIG | lflag::IEXTEN);
    termios.c_cc[cc::VMIN] = 1;
    termios.c_cc[cc::VTIME] = 0;
}
