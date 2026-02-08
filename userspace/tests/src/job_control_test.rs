//! Job Control Infrastructure Tests (std version)
//!
//! Tests the underlying infrastructure that fg, bg, and jobs builtins use:
//! - Process group creation with setpgid()
//! - Process group queries with getpgrp()/getpgid()
//! - SIGCONT signal delivery
//! - waitpid with WUNTRACED flag
//! - Terminal process group control with tcgetpgrp()/tcsetpgrp()
//!
//! These tests verify that the building blocks for shell job control work
//! correctly, even though testing the interactive fg/bg/jobs commands
//! themselves would require a full interactive shell session.

const SIGCONT: i32 = 18;
const WUNTRACED: i32 = 2;
const TIOCGPGRP: u64 = 0x540F;

// Syscall numbers
const SYS_SETPGID: u64 = 109;
const SYS_GETPGID: u64 = 121;
const SYS_IOCTL: u64 = 16;

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

// --- Raw syscall wrappers ---

#[cfg(target_arch = "x86_64")]
unsafe fn raw_syscall2(num: u64, arg1: u64, arg2: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        in("rsi") arg2,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_syscall1(num: u64, arg1: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_syscall2(num: u64, arg1: u64, arg2: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") num,
        inlateout("x0") arg1 => ret,
        in("x1") arg2,
        options(nostack),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_syscall1(num: u64, arg1: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") num,
        inlateout("x0") arg1 => ret,
        options(nostack),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") num,
        inlateout("x0") arg1 => ret,
        in("x1") arg2,
        in("x2") arg3,
        options(nostack),
    );
    ret as i64
}

// --- Helper functions ---

fn setpgid(pid: i32, pgid: i32) -> i32 {
    unsafe { raw_syscall2(SYS_SETPGID, pid as u64, pgid as u64) as i32 }
}

fn getpgid(pid: i32) -> i32 {
    unsafe { raw_syscall1(SYS_GETPGID, pid as u64) as i32 }
}

fn getpgrp() -> i32 {
    getpgid(0)
}

fn tcgetpgrp(fd: i32) -> i32 {
    let mut pgrp: i32 = 0;
    let ret = unsafe { raw_syscall3(SYS_IOCTL, fd as u64, TIOCGPGRP, &mut pgrp as *mut i32 as u64) };
    if ret < 0 { ret as i32 } else { pgrp }
}

/// POSIX wait status macros
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn pass(msg: &str) {
    println!("  PASS: {}", msg);
}

fn fail(msg: &str) -> ! {
    println!("  FAIL: {}", msg);
    std::process::exit(1);
}

/// Test 1: Process group creation
fn test_process_group_creation() {
    println!("Test 1: Process group creation");

    let pid = unsafe { getpid() };
    println!("  Current PID: {}", pid);

    // Create a new process group with ourselves as leader
    let result = setpgid(0, 0);
    if result < 0 {
        println!("  setpgid(0,0) returned: {}", result);
        fail("setpgid(0,0) failed");
    }

    // Verify PGID now equals our PID
    let pgid = getpgrp();
    println!("  getpgrp() returned: {}", pgid);

    if pgid != pid {
        fail("PGID should equal PID after setpgid(0,0)");
    }

    pass("setpgid(0,0) created process group, getpgrp() returns our PID");
}

/// Test 2: SIGCONT delivery
fn test_sigcont_delivery() {
    println!("Test 2: SIGCONT delivery");

    let child = unsafe { fork() };

    if child < 0 {
        fail("fork() failed");
    }

    if child == 0 {
        // Child: exit immediately with a known code
        std::process::exit(42);
    }

    // Parent: send SIGCONT to the child
    println!("  Child PID: {}", child);

    let ret = unsafe { kill(child, SIGCONT) };
    if ret == 0 {
        pass("kill(child, SIGCONT) succeeded");
    } else {
        // The child might have already exited, which is ESRCH (3)
        if ret == -3 || (-ret) == 3 {
            println!("  Note: child already exited (ESRCH) - this is OK");
        } else {
            println!("  kill(SIGCONT) returned error: {}", ret);
            fail("kill(SIGCONT) failed unexpectedly");
        }
    }

    // Wait for child to exit
    let mut status: i32 = 0;
    let wait_result = unsafe { waitpid(child, &mut status, 0) };

    if wait_result < 0 {
        println!("  waitpid returned: {}", wait_result);
        fail("waitpid failed");
    }

    if wifexited(status) && wexitstatus(status) == 42 {
        pass("Child exited normally with code 42");
    } else {
        fail("Child did not exit with expected code");
    }
}

/// Test 3: Terminal process group query
fn test_terminal_pgrp() {
    println!("Test 3: Terminal process group query");

    let pgrp = tcgetpgrp(0); // stdin

    println!("  tcgetpgrp(0) returned: {}", pgrp);

    if pgrp < 0 {
        fail("tcgetpgrp(0) failed");
    }

    pass("tcgetpgrp(0) returned a valid process group");
}

/// Test 4: WUNTRACED flag with waitpid
fn test_wuntraced_flag() {
    println!("Test 4: WUNTRACED flag with waitpid");

    let child = unsafe { fork() };

    if child < 0 {
        fail("fork() failed");
    }

    if child == 0 {
        // Child: exit immediately
        std::process::exit(0);
    }

    // Parent: wait with WUNTRACED flag
    println!("  Child PID: {}", child);

    let mut status: i32 = 0;
    let result = unsafe { waitpid(child, &mut status, WUNTRACED) };

    if result < 0 {
        println!("  waitpid with WUNTRACED returned: {}", result);
        fail("waitpid with WUNTRACED failed");
    }

    if result != child {
        fail("waitpid returned wrong PID");
    }

    pass("waitpid with WUNTRACED succeeded");
}

/// Test 5: getpgid on specific process
fn test_getpgid_specific() {
    println!("Test 5: getpgid on specific process");

    // Query our own process group using our PID (not 0)
    let pid = unsafe { getpid() };
    let pgid = getpgid(pid);

    println!("  getpgid(our_pid) returned: {}", pgid);

    if pgid < 0 {
        fail("getpgid(our_pid) failed");
    }

    // Should match getpgrp()
    let pgrp = getpgrp();
    if pgid != pgrp {
        println!("  Expected (getpgrp): {}", pgrp);
        fail("getpgid(pid) should match getpgrp()");
    }

    pass("getpgid(our_pid) matches getpgrp()");
}

fn main() {
    println!("=== Job Control Infrastructure Tests ===\n");

    test_process_group_creation();
    println!();

    test_sigcont_delivery();
    println!();

    test_terminal_pgrp();
    println!();

    test_wuntraced_flag();
    println!();

    test_getpgid_specific();
    println!();

    println!("=== All job control infrastructure tests passed ===");
    println!("JOB_CONTROL_TEST_PASSED");

    std::process::exit(0);
}
