//! TTY subsystem test program (std version)
//!
//! Tests the TTY layer including:
//! - isatty() on stdin/stdout/stderr
//! - tcgetattr() to get terminal attributes
//! - tcsetattr() with raw mode
//! - tcsetattr() to restore cooked mode
//! - TCGETS/TCSETS round-trip preserves termios
//! - tcgetpgrp()/tcsetpgrp()

extern "C" {
    fn isatty(fd: i32) -> i32;
    fn getpid() -> i32;
}

// ioctl request codes
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;

// Termios c_lflag bits
const ICANON: u32 = 0x0002;
const ECHO: u32 = 0x0008;
const ISIG: u32 = 0x0001;
const IEXTEN: u32 = 0x8000;

// Termios c_iflag bits
const IGNBRK: u32 = 0x0001;
const BRKINT: u32 = 0x0002;
const PARMRK: u32 = 0x0008;
const ISTRIP: u32 = 0x0020;
const INLCR: u32 = 0x0040;
const IGNCR: u32 = 0x0080;
const ICRNL: u32 = 0x0100;
const IXON: u32 = 0x0400;

// Termios c_oflag bits
const OPOST: u32 = 0x0001;

// Termios c_cflag bits
const CSIZE: u32 = 0x0030;
const CS8: u32 = 0x0030;
const PARENB: u32 = 0x0100;

// c_cc indices
const VMIN: usize = 6;
const VTIME: usize = 5;
const NCCS: usize = 32;

// TCSANOW = 0 (for tcsetattr)
const TCSANOW: u64 = TCSETS;

/// Termios structure matching kernel layout
#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; NCCS],
    c_ispeed: u32,
    c_ospeed: u32,
}

impl Default for Termios {
    fn default() -> Self {
        Termios {
            c_iflag: 0,
            c_oflag: 0,
            c_cflag: 0,
            c_lflag: 0,
            c_line: 0,
            c_cc: [0; NCCS],
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }
}

// Raw ioctl syscall
#[cfg(target_arch = "x86_64")]
unsafe fn raw_ioctl(fd: i32, request: u64, arg: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 16u64,  // SYS_IOCTL
        in("rdi") fd as u64,
        in("rsi") request,
        in("rdx") arg,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_ioctl(fd: i32, request: u64, arg: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") 16u64,
        inlateout("x0") fd as u64 => ret,
        in("x1") request,
        in("x2") arg,
        options(nostack),
    );
    ret as i64
}

fn tcgetattr(fd: i32, termios: &mut Termios) -> Result<(), i32> {
    let ret = unsafe { raw_ioctl(fd, TCGETS, termios as *mut Termios as u64) };
    if ret < 0 { Err(-ret as i32) } else { Ok(()) }
}

fn tcsetattr(fd: i32, termios: &Termios) -> Result<(), i32> {
    let ret = unsafe { raw_ioctl(fd, TCSANOW, termios as *const Termios as u64) };
    if ret < 0 { Err(-ret as i32) } else { Ok(()) }
}

fn tcgetpgrp(fd: i32) -> i32 {
    let mut pgrp: i32 = 0;
    let ret = unsafe { raw_ioctl(fd, TIOCGPGRP, &mut pgrp as *mut i32 as u64) };
    if ret < 0 { ret as i32 } else { pgrp }
}

fn tcsetpgrp(fd: i32, pgrp: i32) -> Result<(), i32> {
    let ret = unsafe { raw_ioctl(fd, TIOCSPGRP, &pgrp as *const i32 as u64) };
    if ret < 0 { Err(-ret as i32) } else { Ok(()) }
}

fn cfmakeraw(t: &mut Termios) {
    t.c_iflag &= !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON);
    t.c_oflag &= !OPOST;
    t.c_lflag &= !(ECHO | ICANON | ISIG | IEXTEN);
    t.c_cflag &= !(CSIZE | PARENB);
    t.c_cflag |= CS8;
    t.c_cc[VMIN] = 1;
    t.c_cc[VTIME] = 0;
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

    if unsafe { isatty(0) } != 0 {
        pass("isatty(0) returns true for stdin");
    } else {
        fail("isatty(0) should return true for stdin");
    }

    if unsafe { isatty(1) } != 0 {
        pass("isatty(1) returns true for stdout");
    } else {
        fail("isatty(1) should return true for stdout");
    }

    if unsafe { isatty(2) } != 0 {
        pass("isatty(2) returns true for stderr");
    } else {
        fail("isatty(2) should return true for stderr");
    }

    if unsafe { isatty(999) } == 0 {
        pass("isatty(999) returns false for invalid fd");
    } else {
        fail("isatty(999) should return false for invalid fd");
    }

    // Phase 2: Test tcgetattr()
    println!("\nPhase 2: Testing tcgetattr()...");

    let mut termios = Termios::default();
    match tcgetattr(0, &mut termios) {
        Ok(()) => pass("tcgetattr(0) succeeded on stdin"),
        Err(e) => {
            println!("  tcgetattr returned error: {:#010x}", e);
            fail("tcgetattr(0) should succeed on stdin");
        }
    }

    println!("  c_lflag = {:#010x}", termios.c_lflag);

    if (termios.c_lflag & ICANON) != 0 {
        pass("Default terminal has ICANON set (canonical mode)");
    } else {
        fail("Default terminal should have ICANON set");
    }

    if (termios.c_lflag & ECHO) != 0 {
        pass("Default terminal has ECHO set");
    } else {
        fail("Default terminal should have ECHO set");
    }

    if (termios.c_lflag & ISIG) != 0 {
        pass("Default terminal has ISIG set (signals enabled)");
    } else {
        fail("Default terminal should have ISIG set");
    }

    // Phase 3: Test tcsetattr() with raw mode
    println!("\nPhase 3: Testing tcsetattr() with raw mode...");

    let original_termios = termios;
    cfmakeraw(&mut termios);

    match tcsetattr(0, &termios) {
        Ok(()) => pass("tcsetattr(0, TCSANOW, raw) succeeded"),
        Err(e) => {
            println!("  tcsetattr returned error: {:#010x}", e);
            fail("tcsetattr with raw mode should succeed");
        }
    }

    let mut verify_termios = Termios::default();
    match tcgetattr(0, &mut verify_termios) {
        Ok(()) => pass("tcgetattr after raw mode succeeded"),
        Err(e) => {
            println!("  tcgetattr returned error: {:#010x}", e);
            fail("tcgetattr should succeed after raw mode");
        }
    }

    println!("  After raw: c_lflag = {:#010x}", verify_termios.c_lflag);

    if (verify_termios.c_lflag & ICANON) == 0 {
        pass("Raw mode disabled ICANON");
    } else {
        fail("Raw mode should disable ICANON");
    }

    if (verify_termios.c_lflag & ECHO) == 0 {
        pass("Raw mode disabled ECHO");
    } else {
        fail("Raw mode should disable ECHO");
    }

    if (verify_termios.c_lflag & ISIG) == 0 {
        pass("Raw mode disabled ISIG");
    } else {
        fail("Raw mode should disable ISIG");
    }

    if verify_termios.c_cc[VMIN] == 1 {
        pass("Raw mode set VMIN = 1");
    } else {
        fail("Raw mode should set VMIN = 1");
    }

    if verify_termios.c_cc[VTIME] == 0 {
        pass("Raw mode set VTIME = 0");
    } else {
        fail("Raw mode should set VTIME = 0");
    }

    // Phase 4: Restore cooked mode
    println!("\nPhase 4: Restoring cooked (default) mode...");

    match tcsetattr(0, &original_termios) {
        Ok(()) => pass("tcsetattr to restore original mode succeeded"),
        Err(e) => {
            println!("  tcsetattr returned error: {:#010x}", e);
            fail("tcsetattr to restore original mode should succeed");
        }
    }

    let mut restored_termios = Termios::default();
    match tcgetattr(0, &mut restored_termios) {
        Ok(()) => pass("tcgetattr after restore succeeded"),
        Err(e) => {
            println!("  tcgetattr returned error: {:#010x}", e);
            fail("tcgetattr should succeed after restore");
        }
    }

    println!("  After restore: c_lflag = {:#010x}", restored_termios.c_lflag);

    if (restored_termios.c_lflag & ICANON) != 0 {
        pass("Restored mode has ICANON enabled");
    } else {
        fail("Restored mode should have ICANON enabled");
    }

    if (restored_termios.c_lflag & ECHO) != 0 {
        pass("Restored mode has ECHO enabled");
    } else {
        fail("Restored mode should have ECHO enabled");
    }

    // Phase 5: Test TCGETS/TCSETS round-trip
    println!("\nPhase 5: Testing TCGETS/TCSETS round-trip...");

    let mut t1 = Termios::default();
    if tcgetattr(0, &mut t1).is_err() {
        fail("tcgetattr failed in round-trip test");
    }

    let original_lflag = t1.c_lflag;
    t1.c_lflag &= !ECHO;

    if tcsetattr(0, &t1).is_err() {
        fail("tcsetattr failed in round-trip test");
    }

    let mut t2 = Termios::default();
    if tcgetattr(0, &mut t2).is_err() {
        fail("tcgetattr (second) failed in round-trip test");
    }

    if (t2.c_lflag & ECHO) == 0 {
        pass("Round-trip preserved ECHO=0 modification");
    } else {
        fail("Round-trip did not preserve ECHO modification");
    }

    t1.c_lflag = original_lflag;
    if tcsetattr(0, &t1).is_err() {
        fail("tcsetattr (restore) failed in round-trip test");
    }

    pass("TCGETS/TCSETS round-trip complete");

    // Phase 6: Test tcgetpgrp()/tcsetpgrp()
    println!("\nPhase 6: Testing tcgetpgrp()/tcsetpgrp()...");

    let my_pid = unsafe { getpid() };
    println!("  Our PID: {:#010x}", my_pid);

    let initial_pgrp = tcgetpgrp(0);
    print!("  Initial foreground pgrp: ");
    if initial_pgrp >= 0 {
        println!("{:#010x}", initial_pgrp);
        pass("tcgetpgrp(0) succeeded");
    } else {
        println!("(error)");
        println!("  Note: No foreground pgrp set initially (this is OK)");
    }

    match tcsetpgrp(0, my_pid) {
        Ok(()) => pass("tcsetpgrp(0, our_pid) succeeded"),
        Err(e) => {
            println!("  tcsetpgrp returned error: {:#010x}", e);
            fail("tcsetpgrp should succeed with our PID");
        }
    }

    let set_pgrp = tcgetpgrp(0);
    println!("  After tcsetpgrp: foreground pgrp = {:#010x}", set_pgrp);

    if set_pgrp == my_pid {
        pass("tcgetpgrp returns the value we set");
    } else {
        println!("  Expected: {:#010x}, got: {:#010x}", my_pid, set_pgrp);
        fail("tcgetpgrp should return the pgrp we set");
    }

    let test_pgrp = 12345;
    match tcsetpgrp(0, test_pgrp) {
        Ok(()) => pass("tcsetpgrp(0, 12345) succeeded"),
        Err(e) => {
            println!("  tcsetpgrp returned error: {:#010x}", e);
            fail("tcsetpgrp should succeed with arbitrary pgrp");
        }
    }

    let verify_pgrp = tcgetpgrp(0);
    if verify_pgrp == test_pgrp {
        pass("tcgetpgrp returns arbitrary pgrp value");
    } else {
        fail("tcgetpgrp should return the arbitrary pgrp we set");
    }

    if tcsetpgrp(0, my_pid).is_err() {
        fail("tcsetpgrp failed to restore our pgrp");
    }
    pass("Restored our process as foreground pgrp");

    // All tests passed
    println!("\n=== TTY Test Results ===");
    println!("TTY_TEST: ALL TESTS PASSED");
    println!("TTY_TEST_PASSED");

    std::process::exit(0);
}
