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
    // Handler Null Pointer Tests (arg == 0)
    //
    // These tests verify that all handlers properly reject null pointers.
    // Full integration tests require a TtyDevice which needs the kernel allocator.
    // =============================================================================

    // Note: The following tests require a TtyDevice, which requires the kernel's
    // allocator and other infrastructure. They are written as integration tests
    // or tested via the userspace tty_test binary.
    //
    // The null pointer checks (arg == 0 -> EFAULT) are tested indirectly
    // through the userspace tests and full system tests.

    // =============================================================================
    // Integration Tests (require full kernel context)
    //
    // These tests are better suited for the userspace test binary (tty_test.rs)
    // which can:
    // - Test TCGETS/TCSETS round-trip
    // - Test tty_ioctl dispatch
    // - Test unknown ioctl returns ENOTTY
    // =============================================================================
}
