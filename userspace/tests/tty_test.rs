//! TTY subsystem test program
//!
//! Tests the TTY layer including:
//! - isatty() on stdin/stdout/stderr
//! - tcgetattr() to get terminal attributes
//! - tcsetattr() with raw mode
//! - tcsetattr() to restore cooked mode
//! - TCGETS/TCSETS round-trip preserves termios

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::termios::{
    cc, cfmakeraw, isatty, lflag, tcgetattr, tcgetpgrp, tcsetattr, tcsetpgrp, Termios, TCSANOW,
};

/// Write a test status message
fn write_str(s: &str) {
    io::print(s);
}

/// Write a hexadecimal number
fn write_hex(n: u32) {
    let mut buf = [0u8; 10];
    buf[0] = b'0';
    buf[1] = b'x';

    for i in 0..8 {
        let nibble = (n >> (28 - i * 4)) & 0xF;
        buf[2 + i] = if nibble < 10 {
            b'0' + nibble as u8
        } else {
            b'a' + (nibble - 10) as u8
        };
    }

    if let Ok(s) = core::str::from_utf8(&buf) {
        write_str(s);
    }
}

/// Fail the test with a message
fn fail(msg: &str) -> ! {
    write_str("TTY_TEST: FAIL - ");
    write_str(msg);
    write_str("\n");
    process::exit(1);
}

/// Pass a test phase
fn pass(msg: &str) {
    write_str("TTY_TEST: PASS - ");
    write_str(msg);
    write_str("\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== TTY Test Program ===\n");

    // ==========================================================================
    // Phase 1: Test isatty() on standard file descriptors
    // ==========================================================================
    write_str("\nPhase 1: Testing isatty()...\n");

    // stdin (fd 0) should be a TTY
    if isatty(0) {
        pass("isatty(0) returns true for stdin");
    } else {
        fail("isatty(0) should return true for stdin");
    }

    // stdout (fd 1) should be a TTY
    if isatty(1) {
        pass("isatty(1) returns true for stdout");
    } else {
        fail("isatty(1) should return true for stdout");
    }

    // stderr (fd 2) should be a TTY
    if isatty(2) {
        pass("isatty(2) returns true for stderr");
    } else {
        fail("isatty(2) should return true for stderr");
    }

    // Invalid fd should NOT be a TTY
    if !isatty(999) {
        pass("isatty(999) returns false for invalid fd");
    } else {
        fail("isatty(999) should return false for invalid fd");
    }

    // ==========================================================================
    // Phase 2: Test tcgetattr() on stdin
    // ==========================================================================
    write_str("\nPhase 2: Testing tcgetattr()...\n");

    let mut termios = Termios::default();
    match tcgetattr(0, &mut termios) {
        Ok(()) => {
            pass("tcgetattr(0) succeeded on stdin");
        }
        Err(e) => {
            write_str("  tcgetattr returned error: ");
            write_hex(e as u32);
            write_str("\n");
            fail("tcgetattr(0) should succeed on stdin");
        }
    }

    // Verify we got reasonable default values
    // Default terminal should have canonical mode and echo enabled
    write_str("  c_lflag = ");
    write_hex(termios.c_lflag);
    write_str("\n");

    if (termios.c_lflag & lflag::ICANON) != 0 {
        pass("Default terminal has ICANON set (canonical mode)");
    } else {
        fail("Default terminal should have ICANON set");
    }

    if (termios.c_lflag & lflag::ECHO) != 0 {
        pass("Default terminal has ECHO set");
    } else {
        fail("Default terminal should have ECHO set");
    }

    if (termios.c_lflag & lflag::ISIG) != 0 {
        pass("Default terminal has ISIG set (signals enabled)");
    } else {
        fail("Default terminal should have ISIG set");
    }

    // ==========================================================================
    // Phase 3: Test tcsetattr() with raw mode
    // ==========================================================================
    write_str("\nPhase 3: Testing tcsetattr() with raw mode...\n");

    // Save original termios for restoration
    let original_termios = termios;

    // Set raw mode
    cfmakeraw(&mut termios);

    match tcsetattr(0, TCSANOW, &termios) {
        Ok(()) => {
            pass("tcsetattr(0, TCSANOW, raw) succeeded");
        }
        Err(e) => {
            write_str("  tcsetattr returned error: ");
            write_hex(e as u32);
            write_str("\n");
            fail("tcsetattr with raw mode should succeed");
        }
    }

    // Verify raw mode was applied by reading back
    let mut verify_termios = Termios::default();
    match tcgetattr(0, &mut verify_termios) {
        Ok(()) => {
            pass("tcgetattr after raw mode succeeded");
        }
        Err(e) => {
            write_str("  tcgetattr returned error: ");
            write_hex(e as u32);
            write_str("\n");
            fail("tcgetattr should succeed after raw mode");
        }
    }

    // Check that canonical mode is now disabled
    write_str("  After raw: c_lflag = ");
    write_hex(verify_termios.c_lflag);
    write_str("\n");

    if (verify_termios.c_lflag & lflag::ICANON) == 0 {
        pass("Raw mode disabled ICANON");
    } else {
        fail("Raw mode should disable ICANON");
    }

    if (verify_termios.c_lflag & lflag::ECHO) == 0 {
        pass("Raw mode disabled ECHO");
    } else {
        fail("Raw mode should disable ECHO");
    }

    if (verify_termios.c_lflag & lflag::ISIG) == 0 {
        pass("Raw mode disabled ISIG");
    } else {
        fail("Raw mode should disable ISIG");
    }

    // Check VMIN and VTIME
    if verify_termios.c_cc[cc::VMIN] == 1 {
        pass("Raw mode set VMIN = 1");
    } else {
        fail("Raw mode should set VMIN = 1");
    }

    if verify_termios.c_cc[cc::VTIME] == 0 {
        pass("Raw mode set VTIME = 0");
    } else {
        fail("Raw mode should set VTIME = 0");
    }

    // ==========================================================================
    // Phase 4: Restore cooked mode
    // ==========================================================================
    write_str("\nPhase 4: Restoring cooked (default) mode...\n");

    match tcsetattr(0, TCSANOW, &original_termios) {
        Ok(()) => {
            pass("tcsetattr to restore original mode succeeded");
        }
        Err(e) => {
            write_str("  tcsetattr returned error: ");
            write_hex(e as u32);
            write_str("\n");
            fail("tcsetattr to restore original mode should succeed");
        }
    }

    // Verify restoration
    let mut restored_termios = Termios::default();
    match tcgetattr(0, &mut restored_termios) {
        Ok(()) => {
            pass("tcgetattr after restore succeeded");
        }
        Err(e) => {
            write_str("  tcgetattr returned error: ");
            write_hex(e as u32);
            write_str("\n");
            fail("tcgetattr should succeed after restore");
        }
    }

    write_str("  After restore: c_lflag = ");
    write_hex(restored_termios.c_lflag);
    write_str("\n");

    if (restored_termios.c_lflag & lflag::ICANON) != 0 {
        pass("Restored mode has ICANON enabled");
    } else {
        fail("Restored mode should have ICANON enabled");
    }

    if (restored_termios.c_lflag & lflag::ECHO) != 0 {
        pass("Restored mode has ECHO enabled");
    } else {
        fail("Restored mode should have ECHO enabled");
    }

    // ==========================================================================
    // Phase 5: Test TCGETS/TCSETS round-trip
    // ==========================================================================
    write_str("\nPhase 5: Testing TCGETS/TCSETS round-trip...\n");

    // Get current termios
    let mut t1 = Termios::default();
    if tcgetattr(0, &mut t1).is_err() {
        fail("tcgetattr failed in round-trip test");
    }

    // Modify a field
    let original_lflag = t1.c_lflag;
    t1.c_lflag &= !lflag::ECHO; // Disable echo temporarily

    // Set modified termios
    if tcsetattr(0, TCSANOW, &t1).is_err() {
        fail("tcsetattr failed in round-trip test");
    }

    // Get it back
    let mut t2 = Termios::default();
    if tcgetattr(0, &mut t2).is_err() {
        fail("tcgetattr (second) failed in round-trip test");
    }

    // Verify the modification stuck
    if (t2.c_lflag & lflag::ECHO) == 0 {
        pass("Round-trip preserved ECHO=0 modification");
    } else {
        fail("Round-trip did not preserve ECHO modification");
    }

    // Restore original
    t1.c_lflag = original_lflag;
    if tcsetattr(0, TCSANOW, &t1).is_err() {
        fail("tcsetattr (restore) failed in round-trip test");
    }

    pass("TCGETS/TCSETS round-trip complete");

    // ==========================================================================
    // Phase 6: Test tcgetpgrp()/tcsetpgrp() for terminal control
    // ==========================================================================
    write_str("\nPhase 6: Testing tcgetpgrp()/tcsetpgrp()...\n");

    // Get our own PID to use as a process group
    let my_pid = process::getpid() as i32;
    write_str("  Our PID: ");
    write_hex(my_pid as u32);
    write_str("\n");

    // Get current foreground process group
    let initial_pgrp = tcgetpgrp(0);
    write_str("  Initial foreground pgrp: ");
    if initial_pgrp >= 0 {
        write_hex(initial_pgrp as u32);
        write_str("\n");
        pass("tcgetpgrp(0) succeeded");
    } else {
        write_str("(error)\n");
        // Not a fatal error - pgrp might not be set initially
        write_str("  Note: No foreground pgrp set initially (this is OK)\n");
    }

    // Set our process as the foreground process group
    match tcsetpgrp(0, my_pid as i32) {
        Ok(()) => {
            pass("tcsetpgrp(0, our_pid) succeeded");
        }
        Err(e) => {
            write_str("  tcsetpgrp returned error: ");
            write_hex(e as u32);
            write_str("\n");
            fail("tcsetpgrp should succeed with our PID");
        }
    }

    // Verify it was set
    let set_pgrp = tcgetpgrp(0);
    write_str("  After tcsetpgrp: foreground pgrp = ");
    write_hex(set_pgrp as u32);
    write_str("\n");

    if set_pgrp == my_pid as i32 {
        pass("tcgetpgrp returns the value we set");
    } else {
        write_str("  Expected: ");
        write_hex(my_pid as u32);
        write_str(", got: ");
        write_hex(set_pgrp as u32);
        write_str("\n");
        fail("tcgetpgrp should return the pgrp we set");
    }

    // Test setting a different process group (simulating job control)
    let test_pgrp = 12345;
    match tcsetpgrp(0, test_pgrp) {
        Ok(()) => {
            pass("tcsetpgrp(0, 12345) succeeded");
        }
        Err(e) => {
            write_str("  tcsetpgrp returned error: ");
            write_hex(e as u32);
            write_str("\n");
            fail("tcsetpgrp should succeed with arbitrary pgrp");
        }
    }

    // Verify the arbitrary pgrp was set
    let verify_pgrp = tcgetpgrp(0);
    if verify_pgrp == test_pgrp {
        pass("tcgetpgrp returns arbitrary pgrp value");
    } else {
        fail("tcgetpgrp should return the arbitrary pgrp we set");
    }

    // Restore our process as foreground
    if tcsetpgrp(0, my_pid as i32).is_err() {
        fail("tcsetpgrp failed to restore our pgrp");
    }
    pass("Restored our process as foreground pgrp");

    // ==========================================================================
    // All tests passed
    // ==========================================================================
    write_str("\n=== TTY Test Results ===\n");
    write_str("TTY_TEST: ALL TESTS PASSED\n");
    write_str("TTY_TEST_PASSED\n");

    process::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in TTY test!\n");
    process::exit(1);
}
