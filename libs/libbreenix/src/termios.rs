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

// =============================================================================
// Unit Tests
//
// Note: Most tests require the kernel to be running. The tests below focus on
// pure logic that can be tested without syscalls, such as:
// - tcsetattr action validation
// - Termios structure manipulation
// - cfmakeraw behavior
//
// Full integration tests are in the tty_test binary which runs on the kernel.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================================
    // Request Code Tests
    // =============================================================================

    #[test]
    fn test_request_codes_match_linux() {
        assert_eq!(request::TCGETS, 0x5401);
        assert_eq!(request::TCSETS, 0x5402);
        assert_eq!(request::TCSETSW, 0x5403);
        assert_eq!(request::TCSETSF, 0x5404);
        assert_eq!(request::TIOCGPGRP, 0x540F);
        assert_eq!(request::TIOCSPGRP, 0x5410);
        assert_eq!(request::TIOCGWINSZ, 0x5413);
    }

    // =============================================================================
    // tcsetattr Action Validation Tests
    // =============================================================================

    #[test]
    fn test_tcsetattr_action_constants() {
        assert_eq!(TCSANOW, 0);
        assert_eq!(TCSADRAIN, 1);
        assert_eq!(TCSAFLUSH, 2);
    }

    // Note: These tests would require mocking the syscall to verify the correct
    // request code is sent. Since libbreenix is a no_std library without mocking
    // infrastructure, we verify the logic through the action mapping directly.

    /// Helper to map action to request code (mirrors tcsetattr logic)
    fn action_to_request(action: i32) -> Result<u64, i32> {
        match action {
            TCSANOW => Ok(request::TCSETS),
            TCSADRAIN => Ok(request::TCSETSW),
            TCSAFLUSH => Ok(request::TCSETSF),
            _ => Err(22), // EINVAL
        }
    }

    #[test]
    fn test_tcsetattr_tcsanow_maps_to_tcsets() {
        assert_eq!(action_to_request(TCSANOW), Ok(request::TCSETS));
    }

    #[test]
    fn test_tcsetattr_tcsadrain_maps_to_tcsetsw() {
        assert_eq!(action_to_request(TCSADRAIN), Ok(request::TCSETSW));
    }

    #[test]
    fn test_tcsetattr_tcsaflush_maps_to_tcsetsf() {
        assert_eq!(action_to_request(TCSAFLUSH), Ok(request::TCSETSF));
    }

    #[test]
    fn test_tcsetattr_invalid_action_negative_returns_einval() {
        assert_eq!(action_to_request(-1), Err(22));
    }

    #[test]
    fn test_tcsetattr_invalid_action_three_returns_einval() {
        assert_eq!(action_to_request(3), Err(22));
    }

    #[test]
    fn test_tcsetattr_invalid_action_large_returns_einval() {
        assert_eq!(action_to_request(100), Err(22));
    }

    #[test]
    fn test_tcsetattr_invalid_action_min_i32_returns_einval() {
        assert_eq!(action_to_request(i32::MIN), Err(22));
    }

    #[test]
    fn test_tcsetattr_invalid_action_max_i32_returns_einval() {
        assert_eq!(action_to_request(i32::MAX), Err(22));
    }

    // =============================================================================
    // Termios Structure Tests
    // =============================================================================

    #[test]
    fn test_termios_default() {
        let t = Termios::default();
        assert_eq!(t.c_iflag, 0);
        assert_eq!(t.c_oflag, 0);
        assert_eq!(t.c_cflag, 0);
        assert_eq!(t.c_lflag, 0);
        assert_eq!(t.c_line, 0);
        assert_eq!(t.c_cc, [0; cc::NCCS]);
        assert_eq!(t.c_ispeed, 0);
        assert_eq!(t.c_ospeed, 0);
    }

    #[test]
    fn test_termios_clone() {
        let mut t1 = Termios::default();
        t1.c_lflag = 0x12345678;
        t1.c_cc[cc::VINTR] = 0x03;

        let t2 = t1.clone();
        assert_eq!(t1.c_lflag, t2.c_lflag);
        assert_eq!(t1.c_cc[cc::VINTR], t2.c_cc[cc::VINTR]);
    }

    #[test]
    fn test_termios_copy() {
        let mut t1 = Termios::default();
        t1.c_iflag = 0xABCDEF;
        let t2: Termios = t1; // Copy
        assert_eq!(t1.c_iflag, t2.c_iflag);
    }

    #[test]
    fn test_termios_size() {
        // Termios should be: 4*u32 (flags) + u8 (line) + 32*u8 (cc) + 2*u32 (speeds)
        // = 16 + 1 + 32 + 8 = 57, but with padding it may be larger
        // Just verify it's a reasonable size
        let size = core::mem::size_of::<Termios>();
        assert!(size >= 57, "Termios should be at least 57 bytes");
        assert!(size <= 64, "Termios should be at most 64 bytes (with padding)");
    }

    // =============================================================================
    // cfmakeraw Tests
    // =============================================================================

    #[test]
    fn test_cfmakeraw_clears_icrnl() {
        let mut t = Termios::default();
        t.c_iflag = iflag::ICRNL;
        cfmakeraw(&mut t);
        assert_eq!(t.c_iflag & iflag::ICRNL, 0, "ICRNL should be cleared");
    }

    #[test]
    fn test_cfmakeraw_clears_ixon() {
        let mut t = Termios::default();
        t.c_iflag = iflag::IXON;
        cfmakeraw(&mut t);
        assert_eq!(t.c_iflag & iflag::IXON, 0, "IXON should be cleared");
    }

    #[test]
    fn test_cfmakeraw_clears_opost() {
        let mut t = Termios::default();
        t.c_oflag = oflag::OPOST;
        cfmakeraw(&mut t);
        assert_eq!(t.c_oflag & oflag::OPOST, 0, "OPOST should be cleared");
    }

    #[test]
    fn test_cfmakeraw_clears_echo() {
        let mut t = Termios::default();
        t.c_lflag = lflag::ECHO;
        cfmakeraw(&mut t);
        assert_eq!(t.c_lflag & lflag::ECHO, 0, "ECHO should be cleared");
    }

    #[test]
    fn test_cfmakeraw_clears_echonl() {
        let mut t = Termios::default();
        t.c_lflag = lflag::ECHONL;
        cfmakeraw(&mut t);
        assert_eq!(t.c_lflag & lflag::ECHONL, 0, "ECHONL should be cleared");
    }

    #[test]
    fn test_cfmakeraw_clears_icanon() {
        let mut t = Termios::default();
        t.c_lflag = lflag::ICANON;
        cfmakeraw(&mut t);
        assert_eq!(t.c_lflag & lflag::ICANON, 0, "ICANON should be cleared");
    }

    #[test]
    fn test_cfmakeraw_clears_isig() {
        let mut t = Termios::default();
        t.c_lflag = lflag::ISIG;
        cfmakeraw(&mut t);
        assert_eq!(t.c_lflag & lflag::ISIG, 0, "ISIG should be cleared");
    }

    #[test]
    fn test_cfmakeraw_clears_iexten() {
        let mut t = Termios::default();
        t.c_lflag = lflag::IEXTEN;
        cfmakeraw(&mut t);
        assert_eq!(t.c_lflag & lflag::IEXTEN, 0, "IEXTEN should be cleared");
    }

    #[test]
    fn test_cfmakeraw_sets_vmin() {
        let mut t = Termios::default();
        t.c_cc[cc::VMIN] = 0;
        cfmakeraw(&mut t);
        assert_eq!(t.c_cc[cc::VMIN], 1, "VMIN should be set to 1");
    }

    #[test]
    fn test_cfmakeraw_sets_vtime() {
        let mut t = Termios::default();
        t.c_cc[cc::VTIME] = 10;
        cfmakeraw(&mut t);
        assert_eq!(t.c_cc[cc::VTIME], 0, "VTIME should be set to 0");
    }

    #[test]
    fn test_cfmakeraw_preserves_other_flags() {
        let mut t = Termios::default();
        t.c_cflag = 0xDEADBEEF;
        t.c_ispeed = 115200;
        t.c_ospeed = 115200;
        t.c_cc[cc::VINTR] = 0x03;

        cfmakeraw(&mut t);

        // These should be preserved
        assert_eq!(t.c_cflag, 0xDEADBEEF, "c_cflag should be preserved");
        assert_eq!(t.c_ispeed, 115200, "c_ispeed should be preserved");
        assert_eq!(t.c_ospeed, 115200, "c_ospeed should be preserved");
        assert_eq!(t.c_cc[cc::VINTR], 0x03, "VINTR should be preserved");
    }

    #[test]
    fn test_cfmakeraw_all_flags_set() {
        let mut t = Termios::default();
        // Set all the flags that should be cleared
        t.c_iflag = iflag::ICRNL | iflag::IXON | 0xFF00; // Include some other bits
        t.c_oflag = oflag::OPOST | oflag::ONLCR; // ONLCR should NOT be touched by cfmakeraw
        t.c_lflag = lflag::ECHO | lflag::ECHONL | lflag::ICANON | lflag::ISIG | lflag::IEXTEN;

        cfmakeraw(&mut t);

        // Verify cleared flags
        assert_eq!(t.c_iflag & iflag::ICRNL, 0);
        assert_eq!(t.c_iflag & iflag::IXON, 0);
        assert_eq!(t.c_oflag & oflag::OPOST, 0);
        assert_eq!(t.c_lflag & lflag::ECHO, 0);
        assert_eq!(t.c_lflag & lflag::ICANON, 0);
        assert_eq!(t.c_lflag & lflag::ISIG, 0);

        // Note: ONLCR is cleared by OPOST being cleared (ONLCR only matters if OPOST is set)
        // but the bit itself may still be set - cfmakeraw only clears OPOST

        // Verify other bits in c_iflag are preserved
        assert_ne!(t.c_iflag & 0xFF00, 0, "Other iflag bits should be preserved");
    }

    // =============================================================================
    // Flag Constants Tests
    // =============================================================================

    #[test]
    fn test_lflag_constants() {
        assert_eq!(lflag::ISIG, 0x0001);
        assert_eq!(lflag::ICANON, 0x0002);
        assert_eq!(lflag::ECHO, 0x0008);
        assert_eq!(lflag::ECHOE, 0x0010);
        assert_eq!(lflag::ECHOK, 0x0020);
        assert_eq!(lflag::ECHONL, 0x0040);
        assert_eq!(lflag::IEXTEN, 0x8000);
    }

    #[test]
    fn test_iflag_constants() {
        assert_eq!(iflag::ICRNL, 0x0100);
        assert_eq!(iflag::IXON, 0x0400);
    }

    #[test]
    fn test_oflag_constants() {
        assert_eq!(oflag::OPOST, 0x0001);
        assert_eq!(oflag::ONLCR, 0x0004);
    }

    #[test]
    fn test_cc_indices() {
        assert_eq!(cc::VINTR, 0);
        assert_eq!(cc::VQUIT, 1);
        assert_eq!(cc::VERASE, 2);
        assert_eq!(cc::VKILL, 3);
        assert_eq!(cc::VEOF, 4);
        assert_eq!(cc::VTIME, 5);
        assert_eq!(cc::VMIN, 6);
        assert_eq!(cc::VSUSP, 10);
        assert_eq!(cc::NCCS, 32);
    }

    // =============================================================================
    // Control Character Array Tests
    // =============================================================================

    #[test]
    fn test_cc_array_bounds() {
        let t = Termios::default();
        // Verify we can access all control characters
        for i in 0..cc::NCCS {
            let _ = t.c_cc[i]; // Should not panic
        }
    }

    #[test]
    fn test_cc_array_modification() {
        let mut t = Termios::default();
        t.c_cc[cc::VINTR] = 0x03;  // Ctrl+C
        t.c_cc[cc::VQUIT] = 0x1C;  // Ctrl+\
        t.c_cc[cc::VEOF] = 0x04;   // Ctrl+D
        t.c_cc[cc::VSUSP] = 0x1A;  // Ctrl+Z

        assert_eq!(t.c_cc[cc::VINTR], 0x03);
        assert_eq!(t.c_cc[cc::VQUIT], 0x1C);
        assert_eq!(t.c_cc[cc::VEOF], 0x04);
        assert_eq!(t.c_cc[cc::VSUSP], 0x1A);
    }
}
