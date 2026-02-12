//! TTY subsystem test program (std version)
//!
//! Tests the TTY layer including:
//! - isatty() on stdin/stdout/stderr
//! - tcgetattr() to get terminal attributes
//! - tcsetattr() with raw mode
//! - tcsetattr() to restore cooked mode
//! - TCGETS/TCSETS round-trip preserves termios
//! - tcgetpgrp()/tcsetpgrp()

use libbreenix::error::Error;
use libbreenix::process;
use libbreenix::termios::{self, Termios, TCSANOW};
use libbreenix::types::Fd;

/// Extract the raw errno code from a libbreenix Error
fn errno_code(e: &Error) -> i64 {
    match e {
        Error::Os(errno) => *errno as i64,
    }
}

fn fail(msg: &str) -> ! {
    println!("TTY_TEST: FAIL - {}", msg);
    std::process::exit(1);
}

fn pass(msg: &str) {
    println!("TTY_TEST: PASS - {}", msg);
}

fn main() {
    println!("=== TTY Test Program ===");

    // Phase 1: Test isatty()
    println!("\nPhase 1: Testing isatty()...");

    if termios::isatty(Fd::STDIN) {
        pass("isatty(0) returns true for stdin");
    } else {
        fail("isatty(0) should return true for stdin");
    }

    if termios::isatty(Fd::STDOUT) {
        pass("isatty(1) returns true for stdout");
    } else {
        fail("isatty(1) should return true for stdout");
    }

    if termios::isatty(Fd::STDERR) {
        pass("isatty(2) returns true for stderr");
    } else {
        fail("isatty(2) should return true for stderr");
    }

    if !termios::isatty(Fd::from_raw(999)) {
        pass("isatty(999) returns false for invalid fd");
    } else {
        fail("isatty(999) should return false for invalid fd");
    }

    // Phase 2: Test tcgetattr()
    println!("\nPhase 2: Testing tcgetattr()...");

    let mut t = Termios::default();
    match termios::tcgetattr(Fd::STDIN, &mut t) {
        Ok(()) => pass("tcgetattr(0) succeeded on stdin"),
        Err(e) => {
            println!("  tcgetattr returned error: {:#010x}", errno_code(&e));
            fail("tcgetattr(0) should succeed on stdin");
        }
    }

    println!("  c_lflag = {:#010x}", t.c_lflag);

    if (t.c_lflag & termios::lflag::ICANON) != 0 {
        pass("Default terminal has ICANON set (canonical mode)");
    } else {
        fail("Default terminal should have ICANON set");
    }

    if (t.c_lflag & termios::lflag::ECHO) != 0 {
        pass("Default terminal has ECHO set");
    } else {
        fail("Default terminal should have ECHO set");
    }

    if (t.c_lflag & termios::lflag::ISIG) != 0 {
        pass("Default terminal has ISIG set (signals enabled)");
    } else {
        fail("Default terminal should have ISIG set");
    }

    // Phase 3: Test tcsetattr() with raw mode
    println!("\nPhase 3: Testing tcsetattr() with raw mode...");

    let original_termios = t;
    termios::cfmakeraw(&mut t);

    match termios::tcsetattr(Fd::STDIN, TCSANOW, &t) {
        Ok(()) => pass("tcsetattr(0, TCSANOW, raw) succeeded"),
        Err(e) => {
            println!("  tcsetattr returned error: {:#010x}", errno_code(&e));
            fail("tcsetattr with raw mode should succeed");
        }
    }

    let mut verify_termios = Termios::default();
    match termios::tcgetattr(Fd::STDIN, &mut verify_termios) {
        Ok(()) => pass("tcgetattr after raw mode succeeded"),
        Err(e) => {
            println!("  tcgetattr returned error: {:#010x}", errno_code(&e));
            fail("tcgetattr should succeed after raw mode");
        }
    }

    println!("  After raw: c_lflag = {:#010x}", verify_termios.c_lflag);

    if (verify_termios.c_lflag & termios::lflag::ICANON) == 0 {
        pass("Raw mode disabled ICANON");
    } else {
        fail("Raw mode should disable ICANON");
    }

    if (verify_termios.c_lflag & termios::lflag::ECHO) == 0 {
        pass("Raw mode disabled ECHO");
    } else {
        fail("Raw mode should disable ECHO");
    }

    if (verify_termios.c_lflag & termios::lflag::ISIG) == 0 {
        pass("Raw mode disabled ISIG");
    } else {
        fail("Raw mode should disable ISIG");
    }

    if verify_termios.c_cc[termios::cc::VMIN] == 1 {
        pass("Raw mode set VMIN = 1");
    } else {
        fail("Raw mode should set VMIN = 1");
    }

    if verify_termios.c_cc[termios::cc::VTIME] == 0 {
        pass("Raw mode set VTIME = 0");
    } else {
        fail("Raw mode should set VTIME = 0");
    }

    // Phase 4: Restore cooked mode
    println!("\nPhase 4: Restoring cooked (default) mode...");

    match termios::tcsetattr(Fd::STDIN, TCSANOW, &original_termios) {
        Ok(()) => pass("tcsetattr to restore original mode succeeded"),
        Err(e) => {
            println!("  tcsetattr returned error: {:#010x}", errno_code(&e));
            fail("tcsetattr to restore original mode should succeed");
        }
    }

    let mut restored_termios = Termios::default();
    match termios::tcgetattr(Fd::STDIN, &mut restored_termios) {
        Ok(()) => pass("tcgetattr after restore succeeded"),
        Err(e) => {
            println!("  tcgetattr returned error: {:#010x}", errno_code(&e));
            fail("tcgetattr should succeed after restore");
        }
    }

    println!("  After restore: c_lflag = {:#010x}", restored_termios.c_lflag);

    if (restored_termios.c_lflag & termios::lflag::ICANON) != 0 {
        pass("Restored mode has ICANON enabled");
    } else {
        fail("Restored mode should have ICANON enabled");
    }

    if (restored_termios.c_lflag & termios::lflag::ECHO) != 0 {
        pass("Restored mode has ECHO enabled");
    } else {
        fail("Restored mode should have ECHO enabled");
    }

    // Phase 5: Test TCGETS/TCSETS round-trip
    println!("\nPhase 5: Testing TCGETS/TCSETS round-trip...");

    let mut t1 = Termios::default();
    if termios::tcgetattr(Fd::STDIN, &mut t1).is_err() {
        fail("tcgetattr failed in round-trip test");
    }

    let original_lflag = t1.c_lflag;
    t1.c_lflag &= !termios::lflag::ECHO;

    if termios::tcsetattr(Fd::STDIN, TCSANOW, &t1).is_err() {
        fail("tcsetattr failed in round-trip test");
    }

    let mut t2 = Termios::default();
    if termios::tcgetattr(Fd::STDIN, &mut t2).is_err() {
        fail("tcgetattr (second) failed in round-trip test");
    }

    if (t2.c_lflag & termios::lflag::ECHO) == 0 {
        pass("Round-trip preserved ECHO=0 modification");
    } else {
        fail("Round-trip did not preserve ECHO modification");
    }

    t1.c_lflag = original_lflag;
    if termios::tcsetattr(Fd::STDIN, TCSANOW, &t1).is_err() {
        fail("tcsetattr (restore) failed in round-trip test");
    }

    pass("TCGETS/TCSETS round-trip complete");

    // Phase 6: Test tcgetpgrp()/tcsetpgrp()
    println!("\nPhase 6: Testing tcgetpgrp()/tcsetpgrp()...");

    let my_pid = process::getpid().unwrap_or_else(|_| fail("getpid failed"));
    let my_pid_i32 = my_pid.raw() as i32;
    println!("  Our PID: {:#010x}", my_pid_i32);

    match termios::tcgetpgrp(Fd::STDIN) {
        Ok(initial_pgrp) => {
            println!("  Initial foreground pgrp: {:#010x}", initial_pgrp);
            pass("tcgetpgrp(0) succeeded");
        }
        Err(_) => {
            println!("  (error)");
            println!("  Note: No foreground pgrp set initially (this is OK)");
        }
    }

    match termios::tcsetpgrp(Fd::STDIN, my_pid_i32) {
        Ok(()) => pass("tcsetpgrp(0, our_pid) succeeded"),
        Err(e) => {
            println!("  tcsetpgrp returned error: {:#010x}", errno_code(&e));
            fail("tcsetpgrp should succeed with our PID");
        }
    }

    match termios::tcgetpgrp(Fd::STDIN) {
        Ok(set_pgrp) => {
            println!("  After tcsetpgrp: foreground pgrp = {:#010x}", set_pgrp);

            if set_pgrp == my_pid_i32 {
                pass("tcgetpgrp returns the value we set");
            } else {
                println!("  Expected: {:#010x}, got: {:#010x}", my_pid_i32, set_pgrp);
                fail("tcgetpgrp should return the pgrp we set");
            }
        }
        Err(_) => {
            fail("tcgetpgrp failed after tcsetpgrp");
        }
    }

    let test_pgrp = 12345;
    match termios::tcsetpgrp(Fd::STDIN, test_pgrp) {
        Ok(()) => pass("tcsetpgrp(0, 12345) succeeded"),
        Err(e) => {
            println!("  tcsetpgrp returned error: {:#010x}", errno_code(&e));
            fail("tcsetpgrp should succeed with arbitrary pgrp");
        }
    }

    match termios::tcgetpgrp(Fd::STDIN) {
        Ok(verify_pgrp) => {
            if verify_pgrp == test_pgrp {
                pass("tcgetpgrp returns arbitrary pgrp value");
            } else {
                fail("tcgetpgrp should return the arbitrary pgrp we set");
            }
        }
        Err(_) => {
            fail("tcgetpgrp failed after setting arbitrary pgrp");
        }
    }

    if termios::tcsetpgrp(Fd::STDIN, my_pid_i32).is_err() {
        fail("tcsetpgrp failed to restore our pgrp");
    }
    pass("Restored our process as foreground pgrp");

    // All tests passed
    println!("\n=== TTY Test Results ===");
    println!("TTY_TEST: ALL TESTS PASSED");
    println!("TTY_TEST_PASSED");

    std::process::exit(0);
}
