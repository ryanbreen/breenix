//! TTY ioctl request codes and handlers
//!
//! This module implements ioctl operations for TTY devices, including:
//! - TCGETS/TCSETS: Get/set terminal attributes (termios)
//! - TIOCGPGRP/TIOCSPGRP: Get/set foreground process group
//! - TIOCGWINSZ: Get window size (stub for now)

use super::termios::Termios;
use super::TtyDevice;
use alloc::sync::Arc;

// =============================================================================
// ioctl Request Codes (matching Linux/libbreenix values)
// =============================================================================

/// Get termios structure
pub const TCGETS: u64 = 0x5401;

/// Set termios structure immediately
pub const TCSETS: u64 = 0x5402;

/// Set termios structure after draining output
pub const TCSETSW: u64 = 0x5403;

/// Set termios structure after flushing input and draining output
pub const TCSETSF: u64 = 0x5404;

/// Get foreground process group
pub const TIOCGPGRP: u64 = 0x540F;

/// Set foreground process group
pub const TIOCSPGRP: u64 = 0x5410;

/// Get window size
pub const TIOCGWINSZ: u64 = 0x5413;

// =============================================================================
// Error Codes
// =============================================================================

/// Invalid argument
const EINVAL: i32 = 22;

/// Not a typewriter (not a TTY)
const ENOTTY: i32 = 25;

/// Bad address
const EFAULT: i32 = 14;

// =============================================================================
// Window Size Structure
// =============================================================================

/// Terminal window size (for TIOCGWINSZ)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Winsize {
    /// Number of rows
    pub ws_row: u16,
    /// Number of columns
    pub ws_col: u16,
    /// Horizontal size in pixels (unused)
    pub ws_xpixel: u16,
    /// Vertical size in pixels (unused)
    pub ws_ypixel: u16,
}

// =============================================================================
// ioctl Handler Functions
// =============================================================================

/// Handle TCGETS - get termios attributes
///
/// Copies the current termios structure to the userspace buffer.
pub fn handle_tcgets(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    // Get the termios from the TTY
    let termios = tty.get_termios();

    // Copy to userspace
    // SAFETY: We validate that arg is non-null above. The caller is responsible
    // for ensuring the pointer is valid in the current address space.
    unsafe {
        let user_termios = arg as *mut Termios;
        core::ptr::write_volatile(user_termios, termios);
    }

    Ok(())
}

/// Handle TCSETS - set termios attributes immediately
///
/// Copies the termios structure from userspace and applies it immediately.
pub fn handle_tcsets(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    // Read from userspace
    // SAFETY: We validate that arg is non-null above. The caller is responsible
    // for ensuring the pointer is valid in the current address space.
    let termios = unsafe {
        let user_termios = arg as *const Termios;
        core::ptr::read_volatile(user_termios)
    };

    // Apply the termios settings
    tty.set_termios(&termios);

    log::debug!("TTY{}: TCSETS applied - lflag={:#x}", tty.num, termios.c_lflag);

    Ok(())
}

/// Handle TCSETSW - set termios after draining output
///
/// Same as TCSETS for now since we don't buffer output.
pub fn handle_tcsetsw(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    // For now, we don't buffer output, so this is the same as TCSETS
    // In a full implementation, we would wait for output to drain first
    handle_tcsets(tty, arg)
}

/// Handle TCSETSF - set termios after flushing input and draining output
///
/// Flushes input queue before applying settings.
pub fn handle_tcsetsf(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    // Flush input first
    tty.flush_input();

    // Then apply the settings (same as TCSETS)
    handle_tcsets(tty, arg)
}

/// Handle TIOCGPGRP - get foreground process group
///
/// Returns the foreground process group ID for the terminal.
pub fn handle_tiocgpgrp(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    let pgrp = tty.get_foreground_pgrp().unwrap_or(0);

    // Write to userspace
    // SAFETY: We validate that arg is non-null above.
    unsafe {
        let user_pgrp = arg as *mut i32;
        core::ptr::write_volatile(user_pgrp, pgrp as i32);
    }

    Ok(())
}

/// Handle TIOCSPGRP - set foreground process group
///
/// Sets the foreground process group ID for the terminal.
pub fn handle_tiocspgrp(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    // Read from userspace
    // SAFETY: We validate that arg is non-null above.
    let pgrp = unsafe {
        let user_pgrp = arg as *const i32;
        core::ptr::read_volatile(user_pgrp)
    };

    if pgrp < 0 {
        return Err(EINVAL);
    }

    tty.set_foreground_pgrp(pgrp as u64);

    log::debug!("TTY{}: Set foreground pgrp to {}", tty.num, pgrp);

    Ok(())
}

/// Handle TIOCGWINSZ - get window size
///
/// Returns a default window size (80x25) for console.
pub fn handle_tiocgwinsz(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    // Default console window size
    let winsize = Winsize {
        ws_row: 25,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // Write to userspace
    // SAFETY: We validate that arg is non-null above.
    unsafe {
        let user_winsize = arg as *mut Winsize;
        core::ptr::write_volatile(user_winsize, winsize);
    }

    // Suppress unused warning - num is used for identification
    let _ = tty.num;

    Ok(())
}

/// Dispatch a TTY ioctl request to the appropriate handler
///
/// # Arguments
/// * `tty` - Reference to the TTY device
/// * `request` - The ioctl request code
/// * `arg` - The argument (typically a pointer to a structure)
///
/// # Returns
/// * `Ok(0)` on success
/// * `Err(errno)` on failure
pub fn tty_ioctl(tty: &Arc<TtyDevice>, request: u64, arg: u64) -> Result<i32, i32> {
    match request {
        TCGETS => {
            handle_tcgets(tty, arg)?;
            Ok(0)
        }
        TCSETS => {
            handle_tcsets(tty, arg)?;
            Ok(0)
        }
        TCSETSW => {
            handle_tcsetsw(tty, arg)?;
            Ok(0)
        }
        TCSETSF => {
            handle_tcsetsf(tty, arg)?;
            Ok(0)
        }
        TIOCGPGRP => {
            handle_tiocgpgrp(tty, arg)?;
            Ok(0)
        }
        TIOCSPGRP => {
            handle_tiocspgrp(tty, arg)?;
            Ok(0)
        }
        TIOCGWINSZ => {
            handle_tiocgwinsz(tty, arg)?;
            Ok(0)
        }
        _ => {
            log::warn!("TTY{}: Unknown ioctl request {:#x}", tty.num, request);
            Err(ENOTTY)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================================
    // ioctl Request Code Tests
    // =============================================================================

    #[test]
    fn test_ioctl_request_codes_match_linux() {
        // These values must match Linux/libbreenix to ensure compatibility
        assert_eq!(TCGETS, 0x5401);
        assert_eq!(TCSETS, 0x5402);
        assert_eq!(TCSETSW, 0x5403);
        assert_eq!(TCSETSF, 0x5404);
        assert_eq!(TIOCGPGRP, 0x540F);
        assert_eq!(TIOCSPGRP, 0x5410);
        assert_eq!(TIOCGWINSZ, 0x5413);
    }

    #[test]
    fn test_ioctl_request_codes_are_unique() {
        let codes = [TCGETS, TCSETS, TCSETSW, TCSETSF, TIOCGPGRP, TIOCSPGRP, TIOCGWINSZ];

        // Check all codes are unique
        for (i, &code1) in codes.iter().enumerate() {
            for &code2 in codes.iter().skip(i + 1) {
                assert_ne!(code1, code2, "Duplicate ioctl request codes");
            }
        }
    }

    // =============================================================================
    // Error Code Tests
    // =============================================================================

    #[test]
    fn test_error_codes_match_posix() {
        // POSIX error codes must be consistent
        assert_eq!(EINVAL, 22);  // Invalid argument
        assert_eq!(ENOTTY, 25);  // Not a typewriter
        assert_eq!(EFAULT, 14);  // Bad address
    }

    // =============================================================================
    // Winsize Structure Tests
    // =============================================================================

    #[test]
    fn test_winsize_default() {
        let ws = Winsize::default();
        assert_eq!(ws.ws_row, 0);
        assert_eq!(ws.ws_col, 0);
        assert_eq!(ws.ws_xpixel, 0);
        assert_eq!(ws.ws_ypixel, 0);
    }

    #[test]
    fn test_winsize_clone() {
        let ws1 = Winsize {
            ws_row: 25,
            ws_col: 80,
            ws_xpixel: 640,
            ws_ypixel: 480,
        };
        let ws2 = ws1.clone();

        assert_eq!(ws1.ws_row, ws2.ws_row);
        assert_eq!(ws1.ws_col, ws2.ws_col);
        assert_eq!(ws1.ws_xpixel, ws2.ws_xpixel);
        assert_eq!(ws1.ws_ypixel, ws2.ws_ypixel);
    }

    #[test]
    fn test_winsize_copy() {
        let ws1 = Winsize {
            ws_row: 50,
            ws_col: 120,
            ws_xpixel: 1920,
            ws_ypixel: 1080,
        };
        let ws2: Winsize = ws1; // Copy, not move

        assert_eq!(ws1.ws_row, ws2.ws_row);
        assert_eq!(ws1.ws_col, ws2.ws_col);
    }

    #[test]
    fn test_winsize_size() {
        // Winsize should be 8 bytes (4 x u16)
        assert_eq!(core::mem::size_of::<Winsize>(), 8);
    }

    #[test]
    fn test_winsize_alignment() {
        // Winsize alignment should be 2 (u16 alignment)
        assert_eq!(core::mem::align_of::<Winsize>(), 2);
    }

    // =============================================================================
    // Handler Function Tests
    //
    // These tests verify that the ioctl handlers correctly:
    // - Reject null pointers (arg == 0 -> EFAULT)
    // - Read from and write to userspace pointers
    // - Interact with TtyDevice state correctly
    //
    // Note: These tests use stack-allocated buffers to simulate userspace memory.
    // The handlers use raw pointer operations, so we can test them by passing
    // the address of local variables.
    // =============================================================================

    /// Helper to create a test TtyDevice wrapped in Arc
    fn create_test_tty() -> Arc<TtyDevice> {
        Arc::new(TtyDevice::new(99)) // Use TTY 99 for tests
    }

    // =============================================================================
    // Null Pointer Rejection Tests
    // =============================================================================

    #[test]
    fn test_handle_tcgets_null_pointer() {
        let tty = create_test_tty();
        let result = handle_tcgets(&tty, 0);
        assert_eq!(result, Err(EFAULT));
    }

    #[test]
    fn test_handle_tcsets_null_pointer() {
        let tty = create_test_tty();
        let result = handle_tcsets(&tty, 0);
        assert_eq!(result, Err(EFAULT));
    }

    #[test]
    fn test_handle_tcsetsw_null_pointer() {
        let tty = create_test_tty();
        let result = handle_tcsetsw(&tty, 0);
        assert_eq!(result, Err(EFAULT));
    }

    #[test]
    fn test_handle_tcsetsf_null_pointer() {
        let tty = create_test_tty();
        let result = handle_tcsetsf(&tty, 0);
        assert_eq!(result, Err(EFAULT));
    }

    #[test]
    fn test_handle_tiocgpgrp_null_pointer() {
        let tty = create_test_tty();
        let result = handle_tiocgpgrp(&tty, 0);
        assert_eq!(result, Err(EFAULT));
    }

    #[test]
    fn test_handle_tiocspgrp_null_pointer() {
        let tty = create_test_tty();
        let result = handle_tiocspgrp(&tty, 0);
        assert_eq!(result, Err(EFAULT));
    }

    #[test]
    fn test_handle_tiocgwinsz_null_pointer() {
        let tty = create_test_tty();
        let result = handle_tiocgwinsz(&tty, 0);
        assert_eq!(result, Err(EFAULT));
    }

    // =============================================================================
    // TCGETS Tests - Get termios attributes
    // =============================================================================

    #[test]
    fn test_handle_tcgets_writes_termios() {
        let tty = create_test_tty();

        // Create a zeroed termios buffer
        let mut termios_buf = Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; 32],
            c_ispeed: 0,
            c_ospeed: 0,
        };

        // Get the expected termios from the TTY
        let expected = tty.get_termios();

        // Call handle_tcgets with pointer to our buffer
        let result = handle_tcgets(&tty, &mut termios_buf as *mut Termios as u64);
        assert!(result.is_ok());

        // Verify the buffer was written with TTY's termios
        assert_eq!(termios_buf.c_iflag, expected.c_iflag);
        assert_eq!(termios_buf.c_oflag, expected.c_oflag);
        assert_eq!(termios_buf.c_cflag, expected.c_cflag);
        assert_eq!(termios_buf.c_lflag, expected.c_lflag);
        assert_eq!(termios_buf.c_line, expected.c_line);
        assert_eq!(termios_buf.c_ispeed, expected.c_ispeed);
        assert_eq!(termios_buf.c_ospeed, expected.c_ospeed);
    }

    #[test]
    fn test_handle_tcgets_preserves_default_flags() {
        let tty = create_test_tty();

        let mut termios_buf = Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; 32],
            c_ispeed: 0,
            c_ospeed: 0,
        };

        handle_tcgets(&tty, &mut termios_buf as *mut Termios as u64).unwrap();

        // A new TTY should have default termios with canonical mode, echo, and signals
        use super::super::termios::{ICANON, ECHO, ISIG};
        assert_ne!(termios_buf.c_lflag & ICANON, 0, "ICANON should be set");
        assert_ne!(termios_buf.c_lflag & ECHO, 0, "ECHO should be set");
        assert_ne!(termios_buf.c_lflag & ISIG, 0, "ISIG should be set");
    }

    // =============================================================================
    // TCSETS Tests - Set termios attributes
    // =============================================================================

    #[test]
    fn test_handle_tcsets_reads_and_applies_termios() {
        let tty = create_test_tty();

        // Create a custom termios with raw mode settings
        let mut custom_termios = Termios::default();
        custom_termios.set_raw();
        let expected_lflag = custom_termios.c_lflag;

        // Call handle_tcsets with pointer to our custom termios
        let result = handle_tcsets(&tty, &custom_termios as *const Termios as u64);
        assert!(result.is_ok());

        // Verify the TTY now has our custom termios
        let actual = tty.get_termios();
        assert_eq!(actual.c_lflag, expected_lflag);
    }

    #[test]
    fn test_handle_tcsets_round_trip() {
        let tty = create_test_tty();

        // Get original termios
        let mut original = Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; 32],
            c_ispeed: 0,
            c_ospeed: 0,
        };
        handle_tcgets(&tty, &mut original as *mut Termios as u64).unwrap();

        // Modify and set new termios
        let mut modified = original;
        modified.c_lflag = 0x12345678; // Arbitrary test value

        handle_tcsets(&tty, &modified as *const Termios as u64).unwrap();

        // Read back and verify
        let mut readback = Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; 32],
            c_ispeed: 0,
            c_ospeed: 0,
        };
        handle_tcgets(&tty, &mut readback as *mut Termios as u64).unwrap();

        assert_eq!(readback.c_lflag, 0x12345678);
    }

    // =============================================================================
    // TCSETSW/TCSETSF Tests - Set termios with drain/flush
    // =============================================================================

    #[test]
    fn test_handle_tcsetsw_applies_termios() {
        let tty = create_test_tty();

        let mut custom_termios = Termios::default();
        custom_termios.c_lflag = 0xABCDEF01;

        let result = handle_tcsetsw(&tty, &custom_termios as *const Termios as u64);
        assert!(result.is_ok());

        let actual = tty.get_termios();
        assert_eq!(actual.c_lflag, 0xABCDEF01);
    }

    #[test]
    fn test_handle_tcsetsf_applies_termios_after_flush() {
        let tty = create_test_tty();

        let mut custom_termios = Termios::default();
        custom_termios.c_lflag = 0xDEADBEEF;

        let result = handle_tcsetsf(&tty, &custom_termios as *const Termios as u64);
        assert!(result.is_ok());

        let actual = tty.get_termios();
        assert_eq!(actual.c_lflag, 0xDEADBEEF);
    }

    // =============================================================================
    // TIOCGPGRP Tests - Get foreground process group
    // =============================================================================

    #[test]
    fn test_handle_tiocgpgrp_no_foreground_pgrp() {
        let tty = create_test_tty();

        // New TTY has no foreground pgrp set
        let mut pgrp: i32 = -1;
        let result = handle_tiocgpgrp(&tty, &mut pgrp as *mut i32 as u64);
        assert!(result.is_ok());

        // Should return 0 when no pgrp is set
        assert_eq!(pgrp, 0);
    }

    #[test]
    fn test_handle_tiocgpgrp_with_foreground_pgrp() {
        let tty = create_test_tty();

        // Set a foreground pgrp
        tty.set_foreground_pgrp(42);

        let mut pgrp: i32 = 0;
        let result = handle_tiocgpgrp(&tty, &mut pgrp as *mut i32 as u64);
        assert!(result.is_ok());

        assert_eq!(pgrp, 42);
    }

    // =============================================================================
    // TIOCSPGRP Tests - Set foreground process group
    // =============================================================================

    #[test]
    fn test_handle_tiocspgrp_sets_foreground_pgrp() {
        let tty = create_test_tty();

        let pgrp: i32 = 123;
        let result = handle_tiocspgrp(&tty, &pgrp as *const i32 as u64);
        assert!(result.is_ok());

        // Verify it was set
        assert_eq!(tty.get_foreground_pgrp(), Some(123));
    }

    #[test]
    fn test_handle_tiocspgrp_negative_pgrp_rejected() {
        let tty = create_test_tty();

        let pgrp: i32 = -1;
        let result = handle_tiocspgrp(&tty, &pgrp as *const i32 as u64);
        assert_eq!(result, Err(EINVAL));
    }

    #[test]
    fn test_handle_tiocspgrp_round_trip() {
        let tty = create_test_tty();

        // Set pgrp
        let set_pgrp: i32 = 456;
        handle_tiocspgrp(&tty, &set_pgrp as *const i32 as u64).unwrap();

        // Get pgrp back
        let mut get_pgrp: i32 = 0;
        handle_tiocgpgrp(&tty, &mut get_pgrp as *mut i32 as u64).unwrap();

        assert_eq!(get_pgrp, 456);
    }

    // =============================================================================
    // TIOCGWINSZ Tests - Get window size
    // =============================================================================

    #[test]
    fn test_handle_tiocgwinsz_returns_default_size() {
        let tty = create_test_tty();

        let mut winsize = Winsize::default();
        let result = handle_tiocgwinsz(&tty, &mut winsize as *mut Winsize as u64);
        assert!(result.is_ok());

        // Default console size is 80x25
        assert_eq!(winsize.ws_col, 80);
        assert_eq!(winsize.ws_row, 25);
        assert_eq!(winsize.ws_xpixel, 0);
        assert_eq!(winsize.ws_ypixel, 0);
    }

    // =============================================================================
    // tty_ioctl Dispatch Tests
    // =============================================================================

    #[test]
    fn test_tty_ioctl_dispatch_tcgets() {
        let tty = create_test_tty();
        let mut termios_buf = Termios::default();

        let result = tty_ioctl(&tty, TCGETS, &mut termios_buf as *mut Termios as u64);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn test_tty_ioctl_dispatch_tcsets() {
        let tty = create_test_tty();
        let termios = Termios::default();

        let result = tty_ioctl(&tty, TCSETS, &termios as *const Termios as u64);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn test_tty_ioctl_dispatch_tcsetsw() {
        let tty = create_test_tty();
        let termios = Termios::default();

        let result = tty_ioctl(&tty, TCSETSW, &termios as *const Termios as u64);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn test_tty_ioctl_dispatch_tcsetsf() {
        let tty = create_test_tty();
        let termios = Termios::default();

        let result = tty_ioctl(&tty, TCSETSF, &termios as *const Termios as u64);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn test_tty_ioctl_dispatch_tiocgpgrp() {
        let tty = create_test_tty();
        let mut pgrp: i32 = 0;

        let result = tty_ioctl(&tty, TIOCGPGRP, &mut pgrp as *mut i32 as u64);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn test_tty_ioctl_dispatch_tiocspgrp() {
        let tty = create_test_tty();
        let pgrp: i32 = 100;

        let result = tty_ioctl(&tty, TIOCSPGRP, &pgrp as *const i32 as u64);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn test_tty_ioctl_dispatch_tiocgwinsz() {
        let tty = create_test_tty();
        let mut winsize = Winsize::default();

        let result = tty_ioctl(&tty, TIOCGWINSZ, &mut winsize as *mut Winsize as u64);
        assert_eq!(result, Ok(0));
    }

    #[test]
    fn test_tty_ioctl_unknown_request_returns_enotty() {
        let tty = create_test_tty();

        // Use an unknown ioctl request code
        let result = tty_ioctl(&tty, 0xFFFF, 0);
        assert_eq!(result, Err(ENOTTY));
    }

    #[test]
    fn test_tty_ioctl_propagates_efault() {
        let tty = create_test_tty();

        // All handlers should return EFAULT for null pointer
        assert_eq!(tty_ioctl(&tty, TCGETS, 0), Err(EFAULT));
        assert_eq!(tty_ioctl(&tty, TCSETS, 0), Err(EFAULT));
        assert_eq!(tty_ioctl(&tty, TIOCGPGRP, 0), Err(EFAULT));
        assert_eq!(tty_ioctl(&tty, TIOCSPGRP, 0), Err(EFAULT));
        assert_eq!(tty_ioctl(&tty, TIOCGWINSZ, 0), Err(EFAULT));
    }

    // =============================================================================
    // Additional EINVAL Edge Case Tests - Invalid Arguments
    // =============================================================================

    #[test]
    fn test_handle_tiocspgrp_negative_max_value_rejected() {
        let tty = create_test_tty();

        // Test with i32::MIN (most negative value)
        let pgrp: i32 = i32::MIN;
        let result = handle_tiocspgrp(&tty, &pgrp as *const i32 as u64);
        assert_eq!(result, Err(EINVAL));
    }

    #[test]
    fn test_handle_tiocspgrp_minus_one_rejected() {
        let tty = create_test_tty();

        // -1 is a common sentinel value, should be rejected
        let pgrp: i32 = -1;
        let result = handle_tiocspgrp(&tty, &pgrp as *const i32 as u64);
        assert_eq!(result, Err(EINVAL));
    }

    #[test]
    fn test_handle_tiocspgrp_zero_accepted() {
        let tty = create_test_tty();

        // 0 is valid (process group 0 typically means own process group)
        let pgrp: i32 = 0;
        let result = handle_tiocspgrp(&tty, &pgrp as *const i32 as u64);
        assert_eq!(result, Ok(()));
        assert_eq!(tty.get_foreground_pgrp(), Some(0));
    }

    #[test]
    fn test_handle_tiocspgrp_max_positive_value_accepted() {
        let tty = create_test_tty();

        // Large positive values should be accepted
        let pgrp: i32 = i32::MAX;
        let result = handle_tiocspgrp(&tty, &pgrp as *const i32 as u64);
        assert_eq!(result, Ok(()));
        assert_eq!(tty.get_foreground_pgrp(), Some(i32::MAX as u64));
    }

    // =============================================================================
    // Additional ENOTTY Edge Case Tests - Unknown Request Codes
    // =============================================================================

    #[test]
    fn test_tty_ioctl_request_zero_returns_enotty() {
        let tty = create_test_tty();

        // Request code 0 is not a valid TTY ioctl
        let result = tty_ioctl(&tty, 0, 0);
        assert_eq!(result, Err(ENOTTY));
    }

    #[test]
    fn test_tty_ioctl_request_one_below_tcgets_returns_enotty() {
        let tty = create_test_tty();

        // 0x5400 is one below TCGETS (0x5401)
        let result = tty_ioctl(&tty, TCGETS - 1, 0);
        assert_eq!(result, Err(ENOTTY));
    }

    #[test]
    fn test_tty_ioctl_request_one_above_tiocgwinsz_returns_enotty() {
        let tty = create_test_tty();

        // 0x5414 is one above TIOCGWINSZ (0x5413)
        let result = tty_ioctl(&tty, TIOCGWINSZ + 1, 0);
        assert_eq!(result, Err(ENOTTY));
    }

    #[test]
    fn test_tty_ioctl_request_gap_between_tcsetsf_and_tiocgpgrp_returns_enotty() {
        let tty = create_test_tty();

        // Test values in the gap between TCSETSF (0x5404) and TIOCGPGRP (0x540F)
        for code in (TCSETSF + 1)..TIOCGPGRP {
            let result = tty_ioctl(&tty, code, 0);
            assert_eq!(result, Err(ENOTTY), "Request code {:#x} should return ENOTTY", code);
        }
    }

    #[test]
    fn test_tty_ioctl_request_gap_between_tiocspgrp_and_tiocgwinsz_returns_enotty() {
        let tty = create_test_tty();

        // Test values in the gap between TIOCSPGRP (0x5410) and TIOCGWINSZ (0x5413)
        for code in (TIOCSPGRP + 1)..TIOCGWINSZ {
            let result = tty_ioctl(&tty, code, 0);
            assert_eq!(result, Err(ENOTTY), "Request code {:#x} should return ENOTTY", code);
        }
    }

    #[test]
    fn test_tty_ioctl_max_u64_returns_enotty() {
        let tty = create_test_tty();

        // u64::MAX is not a valid request code
        let result = tty_ioctl(&tty, u64::MAX, 0);
        assert_eq!(result, Err(ENOTTY));
    }

    // =============================================================================
    // TCSETSW/TCSETSF Null Pointer Tests via tty_ioctl
    // =============================================================================

    #[test]
    fn test_tty_ioctl_tcsetsw_null_returns_efault() {
        let tty = create_test_tty();
        let result = tty_ioctl(&tty, TCSETSW, 0);
        assert_eq!(result, Err(EFAULT));
    }

    #[test]
    fn test_tty_ioctl_tcsetsf_null_returns_efault() {
        let tty = create_test_tty();
        let result = tty_ioctl(&tty, TCSETSF, 0);
        assert_eq!(result, Err(EFAULT));
    }

    // =============================================================================
    // TIOCSPGRP EINVAL via tty_ioctl
    // =============================================================================

    #[test]
    fn test_tty_ioctl_tiocspgrp_negative_returns_einval() {
        let tty = create_test_tty();

        let pgrp: i32 = -5;
        let result = tty_ioctl(&tty, TIOCSPGRP, &pgrp as *const i32 as u64);
        assert_eq!(result, Err(EINVAL));
    }
}
