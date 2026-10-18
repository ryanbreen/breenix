//! TTY ioctl request codes and handlers
//!
//! This module implements ioctl operations for TTY devices, including:
//! - TCGETS/TCSETS: Get/set terminal attributes (termios)
//! - TIOCGPGRP/TIOCSPGRP: Get/set foreground process group
//! - TIOCGWINSZ: Get window size (stub for now)

use super::ctty::TtyRef;
use super::jobctl::JobOp;
use super::pty::PtyPair;
use super::termios::{Termios, IoctlTermios};
use super::TtyDevice;
use crate::syscall::userptr::{copy_from_user, copy_to_user};
use alloc::sync::Arc;

// =============================================================================
// ioctl Request Codes (matching Linux/libbreenix values)
// =============================================================================

/// Breenix console source window: begin (0), authentication refused (1), accepted (2).
/// Root only.
pub const CONSOLE_INPUT_PROOF: u64 = 0x425201;

/// Get termios structure
pub const TCGETS: u64 = 0x5401;
/// Drain output or transmit a break.
pub const TCSBRK: u64 = 0x5409;
pub const TCSBRKP: u64 = 0x5425;
/// Suspend/resume transmission.
pub const TCXONC: u64 = 0x540a;
/// Discard pending input/output.
pub const TCFLSH: u64 = 0x540b;

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

/// Set window size
pub const TIOCSWINSZ: u64 = 0x5414;

/// Set controlling terminal
pub const TIOCSCTTY: u64 = 0x540E;

/// Release controlling terminal
pub const TIOCNOTTY: u64 = 0x5422;

/// Get the session of the terminal
pub const TIOCGSID: u64 = 0x5429;

// =============================================================================
// PTY-Specific ioctl Request Codes
// =============================================================================

/// Get PTY number (for master fd)
pub const TIOCGPTN: u64 = 0x80045430;

/// Lock/unlock PTY slave
pub const TIOCSPTLCK: u64 = 0x40045431;

/// Get PTY lock status
pub const TIOCGPTLCK: u64 = 0x80045439;

// =============================================================================
// Error Codes
// =============================================================================

/// Operation not permitted
const EPERM: i32 = 1;

/// Invalid argument
const EINVAL: i32 = 22;

/// Not a typewriter (not a TTY)
const ENOTTY: i32 = 25;

/// Bad address
const EFAULT: i32 = 14;

// =============================================================================
// Window Size Structure
// =============================================================================

/// Terminal window size (for TIOCGWINSZ/TIOCSWINSZ)
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

impl Winsize {
    /// Create a new Winsize with specified dimensions
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }

    /// Default window size for PTY devices (24x80)
    pub fn default_pty() -> Self {
        Self::new(24, 80)
    }
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

    copy_to_user(arg as *mut IoctlTermios, &IoctlTermios::from(termios)).map_err(|_| EFAULT)?;

    Ok(())
}

/// Handle TCSETS - set termios attributes immediately
///
/// Copies the termios structure from userspace and applies it immediately.
pub fn handle_tcsets(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    let termios: Termios = copy_from_user(arg as *const IoctlTermios).map_err(|_| EFAULT)?.into();

    // Apply the termios settings
    tty.set_termios(&termios);

    log::debug!(
        "TTY{}: TCSETS applied - lflag={:#x}",
        tty.num,
        termios.c_lflag
    );

    Ok(())
}

/// Handle TCSETSW - set termios after draining output
///
/// Waits for the output already written to leave the UART, then applies the
/// settings as TCSETS does.
pub fn handle_tcsetsw(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    let attrs: Termios = copy_from_user(arg as *const IoctlTermios).map_err(|_| EFAULT)?.into();
    crate::serial::drain_output();
    tty.set_termios(&attrs);
    Ok(())
}

/// Handle TCSETSF - set termios after flushing input and draining output
///
/// Waits for the output already written to leave the UART and flushes the
/// input queue before applying the settings.
pub fn handle_tcsetsf(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    let attrs: Termios = copy_from_user(arg as *const IoctlTermios).map_err(|_| EFAULT)?.into();
    crate::serial::drain_output();
    tty.flush_input();
    tty.set_termios(&attrs);
    Ok(())
}

/// Handle TIOCGPGRP - get foreground process group
///
/// Returns the foreground process group ID for the terminal.
pub fn handle_tiocgpgrp(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    let pgrp = tty.get_foreground_pgrp().unwrap_or(0);

    copy_to_user(arg as *mut i32, &(pgrp as i32)).map_err(|_| EFAULT)?;

    Ok(())
}

/// Handle TIOCSPGRP - set foreground process group
///
/// Sets the foreground process group ID for the terminal.
pub fn handle_tiocspgrp(tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
    if arg == 0 {
        return Err(EFAULT);
    }

    let pgrp: i32 = copy_from_user(arg as *const i32).map_err(|_| EFAULT)?;

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
pub fn handle_tiocgwinsz(_tty: &Arc<TtyDevice>, arg: u64) -> Result<(), i32> {
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

    copy_to_user(arg as *mut Winsize, &winsize).map_err(|_| EFAULT)?;

    Ok(())
}

// =============================================================================
// Dispatch
// =============================================================================

/// Which side of a terminal an ioctl was made on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Master,
    Slave,
    Console,
}

/// Dispatch an ioctl on terminal `t`, made on `side`.
pub fn dispatch(t: &TtyRef, side: Side, request: u64, arg: u64) -> Result<i32, i32> {
    if side == Side::Slave && matches!(t, TtyRef::Pty(pair) if pair.master_closed()) {
        return Err(if request == TIOCSPGRP { ENOTTY } else { crate::syscall::errno::EIO });
    }
    match request {
        TIOCSCTTY | TIOCNOTTY | TIOCGSID => super::ctty::ioctl(t, side, request, arg),
        TIOCGPGRP | TIOCSPGRP => super::jobctl::ioctl(t, side, request, arg),
        TIOCGWINSZ | TIOCSWINSZ | TIOCGPTN | TIOCSPTLCK | TIOCGPTLCK => match t {
            TtyRef::Pty(pair) => super::pty::ioctl::ioctl(pair, side, request, arg),
            TtyRef::Console(tty) => tty_ioctl(tty, request, arg),
        },
        _ => match t {
            TtyRef::Console(tty) => {
                if matches!(request, TCSETS | TCSETSW | TCSETSF | TCFLSH | TCXONC | TCSBRK | TCSBRKP) {
                    super::jobctl::check(t, JobOp::Change)?;
                }
                tty_ioctl(tty, request, arg)
            }
            TtyRef::Pty(pair) => pty_termios_ioctl(t, pair, side, request, arg),
        },
    }
}

/// TCGETS and TCSETS* on a pseudo-terminal (either side).
fn pty_termios_ioctl(
    t: &TtyRef,
    pair: &Arc<PtyPair>,
    side: Side,
    request: u64,
    arg: u64,
) -> Result<i32, i32> {
    match request {
        TCGETS => {
            if arg == 0 {
                return Err(EFAULT);
            }
            let termios = pair.termios_snapshot();
            copy_to_user(arg as *mut IoctlTermios, &IoctlTermios::from(termios)).map_err(|_| EFAULT)?;
            Ok(0)
        }
        TCSETS | TCSETSW | TCSETSF => {
            super::jobctl::check(t, JobOp::Change)?;
            let termios: Termios = copy_from_user(arg as *const IoctlTermios).map_err(|_| EFAULT)?.into();
            // PTY writes already transferred output to the peer's queue;
            // there is no hardware transmitter to drain.
            {
                let mut ldisc = pair.ldisc.lock();
                if request == TCSETSF { ldisc.flush_input(); }
                ldisc.set_termios(termios);
            }
            pair.update_input_flow();
            pair.slave_readers.wake_up();
            pair.master_writers.wake_up();
            Ok(0)
        }
        TCFLSH => {
            if side != Side::Master { super::jobctl::check(t, JobOp::Change)?; }
            if arg > 2 { return Err(EINVAL); }
            let input = if side == Side::Master { 1 } else { 0 };
            let output = if side == Side::Master { 0 } else { 1 };
            if arg == input || arg == 2 {
                pair.ldisc.lock().flush_input();
                pair.master_writers.wake_up();
            }
            if arg == output || arg == 2 {
                pair.flush_output();
            }
            Ok(0)
        }
        TCXONC => {
            if side != Side::Master { super::jobctl::check(t, JobOp::Change)?; }
            if arg > 3 { return Err(EINVAL); }
            match arg {
                0 | 1 if side == Side::Master => pair.set_master_output_stopped(arg == 0),
                0 | 1 => pair.set_output_stopped(arg == 0),
                2 | 3 => {
                    let attrs = pair.termios_snapshot();
                    let c = attrs.c_cc[if arg == 2 { super::termios::VSTOP } else { super::termios::VSTART }];
                    if c != 0 {
                        // Linux tty_send_xchar returns success even when its
                        // driver cannot enqueue this priority byte.
                        if side == Side::Master { pair.send_flow_char_to_slave(c); }
                        else { pair.send_flow_char_to_master(c); }
                    }
                },
                _ => unreachable!(),
            }
            Ok(0)
        }
        TCSBRK | TCSBRKP => {
            if side != Side::Master { super::jobctl::check(t, JobOp::Change)?; }
            // A PTY has no serial transmitter or physical break signal.
            Ok(0)
        }
        _ => Err(ENOTTY),
    }
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
        CONSOLE_INPUT_PROOF => {
            // It flushes input and reports authentication: getty and login only.
            if crate::fs::permissions::Credentials::current(false).euid != 0 {
                return Err(EPERM);
            }
            tty.input_proof(arg)
        }
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
        TCFLSH => {
            if arg > 2 { return Err(EINVAL); }
            if arg == 0 || arg == 2 { tty.flush_input(); }
            if arg == 1 || arg == 2 { tty.flush_output(); }
            Ok(0)
        }
        TCXONC => {
            match arg {
                0 | 1 => tty.set_output_stopped(arg == 0),
                2 | 3 => tty.output_flow_character(arg == 2),
                _ => return Err(EINVAL),
            }
            Ok(0)
        }
        TCSBRK | TCSBRKP => {
            crate::serial::drain_output();
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
        let codes = [
            TCGETS, TCSETS, TCSETSW, TCSETSF, TIOCGPGRP, TIOCSPGRP, TIOCGWINSZ,
        ];

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
        assert_eq!(EINVAL, 22); // Invalid argument
        assert_eq!(ENOTTY, 25); // Not a typewriter
        assert_eq!(EFAULT, 14); // Bad address
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
    // The host user-copy shim permits these buffers, so we can test by passing
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
        assert_eq!(termios_buf.c_ispeed, 0); // Outside the ioctl wire layout.
        assert_eq!(termios_buf.c_ospeed, 0);
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
        use super::super::termios::{ECHO, ICANON, ISIG};
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
            if matches!(code, TCFLSH | TCSBRK | TCXONC) { continue; }
            let result = tty_ioctl(&tty, code, 0);
            assert_eq!(
                result,
                Err(ENOTTY),
                "Request code {:#x} should return ENOTTY",
                code
            );
        }
    }

    #[test]
    fn test_tty_ioctl_request_gap_between_tiocspgrp_and_tiocgwinsz_returns_enotty() {
        let tty = create_test_tty();

        // Test values in the gap between TIOCSPGRP (0x5410) and TIOCGWINSZ (0x5413)
        for code in (TIOCSPGRP + 1)..TIOCGWINSZ {
            let result = tty_ioctl(&tty, code, 0);
            assert_eq!(
                result,
                Err(ENOTTY),
                "Request code {:#x} should return ENOTTY",
                code
            );
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
    #[test]
    fn pty_flush_discards_selected_queues_and_tcsetsf_discards_input() {
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let t = TtyRef::Pty(pair.clone());
        let mut attrs = pair.termios_snapshot();
        attrs.set_raw();
        pair.ldisc.lock().set_termios(attrs);
        assert_eq!(pair.master_write(b"input"), 5);
        assert_eq!(pair.slave_to_master.lock().write(b"output"), 6);
        assert_eq!(dispatch(&t, Side::Slave, TCFLSH, 0), Ok(0));
        assert!(!pair.ldisc.lock().has_data());
        assert!(!pair.slave_to_master.lock().is_empty());
        assert_eq!(pair.master_write(b"input"), 5);
        assert_eq!(dispatch(&t, Side::Slave, TCFLSH, 1), Ok(0));
        assert!(pair.ldisc.lock().has_data());
        assert!(pair.slave_to_master.lock().is_empty());
        assert_eq!(pair.slave_to_master.lock().write(b"output"), 6);
        assert_eq!(dispatch(&t, Side::Slave, TCFLSH, 2), Ok(0));
        assert!(!pair.ldisc.lock().has_data());
        assert!(pair.slave_to_master.lock().is_empty());
        assert_eq!(pair.master_write(b"input"), 5);
        attrs.c_cc[super::super::termios::VERASE] = 8;
        assert_eq!(dispatch(&t, Side::Slave, TCSETSF, &IoctlTermios::from(attrs) as *const IoctlTermios as u64), Ok(0));
        assert!(!pair.ldisc.lock().has_data());
        assert_eq!(pair.termios_snapshot().c_cc[super::super::termios::VERASE], 8);
    }

    #[test]
    fn pty_control_validates_selectors_and_drain_preserves_peer_output() {
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let t = TtyRef::Pty(pair.clone());
        assert_eq!(dispatch(&t, Side::Slave, TCXONC, 0), Ok(0));
        assert!(pair.output_stopped());
        assert_eq!(dispatch(&t, Side::Slave, TCXONC, 1), Ok(0));
        assert!(!pair.output_stopped());
        for action in [2, 3] { assert_eq!(dispatch(&t, Side::Slave, TCXONC, action), Ok(0)); }
        for action in [4, u64::MAX] { assert_eq!(dispatch(&t, Side::Slave, TCXONC, action), Err(EINVAL)); }
        for selector in [3, u64::MAX] { assert_eq!(dispatch(&t, Side::Slave, TCFLSH, selector), Err(EINVAL)); }
        assert_eq!(pair.slave_to_master.lock().write(b"output"), 6);
        for request in [TCSBRK, TCSBRKP] {
            assert_eq!(dispatch(&t, Side::Slave, request, 0), Ok(0));
            assert!(!pair.slave_to_master.lock().is_empty());
        }
    }

    #[test]
    fn isig_flushes_input_and_output_but_noflsh_preserves_them() {
        use super::super::termios::{ISIG, ECHO, NOFLSH};
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let mut attrs = pair.termios_snapshot();
        attrs.set_raw();
        attrs.c_lflag |= ISIG | ECHO;
        pair.ldisc.lock().set_termios(attrs);
        pair.slave_to_master.lock().write(b"old output");
        assert_eq!(pair.master_write(b"old input\x03fresh"), 10);
        assert_eq!(pair.master_write(b"fresh"), 5);
        let mut buf = [0; 32];
        let n = pair.ldisc.lock().read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"fresh");
        let n = pair.slave_to_master.lock().read(&mut buf);
        assert_eq!(&buf[..n], b"^C\r\nfresh");
        attrs.c_lflag |= NOFLSH;
        pair.ldisc.lock().set_termios(attrs);
        pair.slave_to_master.lock().write(b"kept");
        assert_eq!(pair.master_write(b"xy\x03"), 3);
        let n = pair.ldisc.lock().read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"xy");
        let n = pair.slave_to_master.lock().read(&mut buf);
        assert_eq!(&buf[..n], b"keptxy^C\r\n");
    }

    #[test]
    fn master_flush_selectors_follow_the_master_direction() {
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let terminal = TtyRef::Pty(pair.clone());
        let mut attrs = pair.termios_snapshot();
        attrs.set_raw();
        pair.ldisc.lock().set_termios(attrs);
        assert_eq!(pair.master_write(b"input"), 5);
        pair.slave_to_master.lock().write(b"output");
        assert_eq!(dispatch(&terminal, Side::Master, TCFLSH, 0), Ok(0));
        assert!(pair.ldisc.lock().has_data());
        assert!(pair.slave_to_master.lock().is_empty());
        pair.slave_to_master.lock().write(b"output");
        assert_eq!(dispatch(&terminal, Side::Master, TCFLSH, 1), Ok(0));
        assert!(!pair.ldisc.lock().has_data());
        assert!(!pair.slave_to_master.lock().is_empty());
    }

    #[test]
    fn tcgets_writes_only_the_linux_wire_structure() {
        #[repr(C)]
        struct Guarded { wire: IoctlTermios, guard: [u8; 24] }
        let tty = create_test_tty();
        let mut buffer = Guarded { wire: IoctlTermios::default(), guard: [0xa5; 24] };
        assert_eq!(core::mem::size_of::<IoctlTermios>(), 36);
        assert_eq!(handle_tcgets(&tty, &mut buffer.wire as *mut _ as u64), Ok(()));
        assert_eq!(buffer.guard, [0xa5; 24]);
        let attrs: Termios = buffer.wire.into();
        assert_eq!(attrs.c_ospeed, super::super::termios::B38400);
        assert_eq!(attrs.c_ispeed, attrs.c_ospeed);
    }

    #[test]
    fn master_flow_controls_do_not_stop_slave_output_and_send_peer_characters() {
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let terminal = TtyRef::Pty(pair.clone());
        let mut attrs = pair.termios_snapshot();
        attrs.set_raw();
        pair.ldisc.lock().set_termios(attrs);
        assert_eq!(dispatch(&terminal, Side::Master, TCXONC, 0), Ok(0));
        assert!(pair.master_output_stopped());
        assert!(!pair.output_stopped());
        assert_eq!(pair.master_write(b"x"), 0);
        assert_eq!(dispatch(&terminal, Side::Master, TCXONC, 1), Ok(0));
        assert_eq!(pair.master_write(b"x"), 1);
        for action in [2, 3] { assert_eq!(dispatch(&terminal, Side::Master, TCXONC, action), Ok(0)); }
        let mut buf = [0; 8];
        let n = pair.ldisc.lock().read(&mut buf).unwrap();
        assert_eq!(&buf[..n], &[b'x', attrs.c_cc[super::super::termios::VSTOP], attrs.c_cc[super::super::termios::VSTART]]);
        for action in [2, 3] { assert_eq!(dispatch(&terminal, Side::Slave, TCXONC, action), Ok(0)); }
        let n = pair.slave_to_master.lock().read(&mut buf);
        assert_eq!(&buf[..n], &[attrs.c_cc[super::super::termios::VSTOP], attrs.c_cc[super::super::termios::VSTART]]);
    }

    #[test]
    fn stopped_output_retains_more_than_256_bytes_of_translated_echo() {
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let input = [b'x'; 600];
        pair.set_output_stopped(true);
        assert_eq!(pair.master_write(&input), 600);
        assert_eq!(pair.master_write(b"\n"), 1);
        assert!(pair.slave_to_master.lock().is_empty());
        pair.set_output_stopped(false);
        let mut buf = [0; 700];
        let n = pair.master_read(&mut buf).unwrap();
        assert_eq!(n, 602);
        assert_eq!(&buf[..600], &input);
        assert_eq!(&buf[600..n], b"\r\n");
    }

    #[test]
    fn ixon_stops_output_until_start_or_ixany_and_disabled_ixon_resumes_it() {
        use super::super::termios::{IXON, IXANY};
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let terminal = TtyRef::Pty(pair.clone());
        let mut attrs = pair.termios_snapshot();
        attrs.set_raw();
        attrs.c_iflag |= IXON;
        pair.ldisc.lock().set_termios(attrs);
        assert_eq!(pair.master_write(&[attrs.c_cc[super::super::termios::VSTOP]]), 1);
        assert!(pair.output_stopped());
        assert_eq!(pair.master_write(&[attrs.c_cc[super::super::termios::VSTART]]), 1);
        assert!(!pair.output_stopped());
        attrs.c_iflag |= IXANY;
        let wire = IoctlTermios::from(attrs);
        assert_eq!(dispatch(&terminal, Side::Slave, TCSETS, &wire as *const _ as u64), Ok(0));
        pair.master_write(&[attrs.c_cc[super::super::termios::VSTOP]]);
        assert!(pair.output_stopped());
        pair.master_write(b"x");
        assert!(!pair.output_stopped());
        pair.master_write(&[attrs.c_cc[super::super::termios::VSTOP]]);
        attrs.c_iflag &= !IXON;
        let wire = IoctlTermios::from(attrs);
        assert_eq!(dispatch(&terminal, Side::Slave, TCSETS, &wire as *const _ as u64), Ok(0));
        assert!(!pair.output_stopped());
    }

    #[test]
    fn full_raw_input_still_accepts_flow_controls_without_losing_payload() {
        use super::super::termios::IXON;
        let pair = Arc::new(PtyPair::try_new(0, 0, 0).unwrap());
        let mut attrs = pair.termios_snapshot();
        attrs.set_raw();
        attrs.c_iflag |= IXON;
        pair.ldisc.lock().set_termios(attrs);
        let payload = [b'x'; 4095];
        assert_eq!(pair.master_write(&payload), payload.len());
        assert_eq!(pair.master_write(&[attrs.c_cc[super::super::termios::VSTOP]]), 1);
        assert!(pair.output_stopped());
        assert_eq!(pair.master_write(&[attrs.c_cc[super::super::termios::VSTART]]), 1);
        assert!(!pair.output_stopped());
        let mut received = [0; 4096];
        let n = pair.ldisc.lock().read(&mut received).unwrap();
        assert_eq!(&received[..n], &payload);
    }

}
