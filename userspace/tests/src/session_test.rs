//! Session and process group syscall tests (std version)
//!
//! Tests POSIX session and process group syscalls:
//! - getpgid()/setpgid() - process group get/set
//! - getpgrp() - get calling process's process group
//! - getsid()/setsid() - session get/create

extern "C" {
    fn getpid() -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

// Raw syscall wrappers for session/process group operations
// These are not in the libbreenix-libc so we use raw syscalls

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
unsafe fn raw_syscall0(num: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") num,
        lateout("rax") ret,
        options(nostack, preserves_flags),
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
unsafe fn raw_syscall0(num: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") num,
        lateout("x0") ret,
        options(nostack),
    );
    ret as i64
}

// Syscall numbers
const SYS_SETPGID: u64 = 109;
const SYS_GETPGID: u64 = 121;
const SYS_SETSID: u64 = 112;
const SYS_GETSID: u64 = 124;

fn getpgid(pid: i32) -> i32 {
    unsafe { raw_syscall1(SYS_GETPGID, pid as u64) as i32 }
}

fn setpgid(pid: i32, pgid: i32) -> i32 {
    unsafe { raw_syscall2(SYS_SETPGID, pid as u64, pgid as u64) as i32 }
}

fn getpgrp() -> i32 {
    getpgid(0)
}

fn setsid() -> i32 {
    unsafe { raw_syscall0(SYS_SETSID) as i32 }
}

fn getsid(pid: i32) -> i32 {
    unsafe { raw_syscall1(SYS_GETSID, pid as u64) as i32 }
}

fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn fail(msg: &str) -> ! {
    println!("SESSION_TEST: FAIL - {}", msg);
    std::process::exit(1);
}

fn test_getpgid_self() {
    println!("\nTest 1: getpgid(0) returns current process's pgid");

    let pgid = getpgid(0);
    if pgid <= 0 {
        println!("  getpgid(0) returned: {}", pgid);
        fail("getpgid(0) should return positive value");
    }

    println!("  getpgid(0) = {}", pgid);
    println!("  test_getpgid_self: PASS");
}

fn test_getpgid_with_pid() {
    println!("\nTest 2: getpgid(getpid()) returns same as getpgid(0)");

    let pid = unsafe { getpid() };
    let pgid_0 = getpgid(0);
    let pgid_pid = getpgid(pid);

    println!("  pid = {}", pid);
    println!("  getpgid(0) = {}", pgid_0);
    println!("  getpgid(pid) = {}", pgid_pid);

    if pgid_0 != pgid_pid {
        fail("getpgid(0) should equal getpgid(getpid())");
    }

    println!("  test_getpgid_with_pid: PASS");
}

fn test_setpgid_self() {
    println!("\nTest 3: setpgid(0, 0) sets pgid to own pid");

    let pid = unsafe { getpid() };
    let result = setpgid(0, 0);

    println!("  pid = {}", pid);
    println!("  setpgid(0, 0) returned: {}", result);

    if result != 0 {
        fail("setpgid(0, 0) should succeed");
    }

    let pgid = getpgid(0);
    println!("  getpgid(0) after setpgid = {}", pgid);

    if pgid != pid {
        fail("after setpgid(0, 0), pgid should equal pid");
    }

    println!("  test_setpgid_self: PASS");
}

fn test_getpgrp() {
    println!("\nTest 4: getpgrp() returns same as getpgid(0)");

    let pgrp = getpgrp();
    let pgid = getpgid(0);

    println!("  getpgrp() = {}", pgrp);
    println!("  getpgid(0) = {}", pgid);

    if pgrp != pgid {
        fail("getpgrp() should equal getpgid(0)");
    }

    println!("  test_getpgrp: PASS");
}

fn test_getsid_self() {
    println!("\nTest 5: getsid(0) returns current session id");

    let sid = getsid(0);
    if sid <= 0 {
        println!("  getsid(0) returned: {}", sid);
        fail("getsid(0) should return positive value");
    }

    println!("  getsid(0) = {}", sid);
    println!("  test_getsid_self: PASS");
}

fn test_getsid_with_pid() {
    println!("\nTest 6: getsid(getpid()) returns same as getsid(0)");

    let pid = unsafe { getpid() };
    let sid_0 = getsid(0);
    let sid_pid = getsid(pid);

    println!("  pid = {}", pid);
    println!("  getsid(0) = {}", sid_0);
    println!("  getsid(pid) = {}", sid_pid);

    if sid_0 != sid_pid {
        fail("getsid(0) should equal getsid(getpid())");
    }

    println!("  test_getsid_with_pid: PASS");
}

fn test_setsid_in_child() {
    println!("\nTest 7: setsid() in child creates new session");

    let fork_result = unsafe { fork() };

    if fork_result < 0 {
        println!("  fork() failed with error: {}", fork_result);
        fail("fork failed");
    }

    if fork_result == 0 {
        // Child process
        let my_pid = unsafe { getpid() };
        println!("  CHILD: pid = {}", my_pid);

        let setpgid_result = setpgid(0, 0);
        println!("  CHILD: setpgid(0, 0) returned: {}", setpgid_result);

        let new_sid = setsid();
        println!("  CHILD: setsid() returned: {}", new_sid);

        if new_sid < 0 {
            println!("  CHILD: setsid() failed");
            std::process::exit(1);
        }

        let sid = getsid(0);
        let pgid = getpgid(0);

        println!("  CHILD: getsid(0) = {}", sid);
        println!("  CHILD: getpgid(0) = {}", pgid);

        if sid != my_pid {
            println!("  CHILD: ERROR - sid should equal pid after setsid");
            std::process::exit(1);
        }

        if pgid != my_pid {
            println!("  CHILD: ERROR - pgid should equal pid after setsid");
            std::process::exit(1);
        }

        println!("  CHILD: setsid test PASS");
        std::process::exit(0);
    } else {
        // Parent: wait for child
        let child_pid = fork_result;
        println!("  PARENT: waiting for child {}", child_pid);

        let mut status: i32 = 0;
        let result = unsafe { waitpid(child_pid, &mut status, 0) };

        println!("  PARENT: waitpid returned: {}", result);

        if result != fork_result {
            fail("waitpid returned wrong pid");
        }

        if !wifexited(status) {
            println!("  PARENT: child did not exit normally, status = {}", status);
            fail("child did not exit normally");
        }

        let exit_code = wexitstatus(status);
        println!("  PARENT: child exit code = {}", exit_code);

        if exit_code != 0 {
            fail("child reported test failure");
        }

        println!("  test_setsid_in_child: PASS");
    }
}

fn test_error_cases() {
    println!("\nTest 8: Error cases for invalid PIDs");

    let result = getpgid(-1);
    println!("  getpgid(-1) = {}", result);

    if result >= 0 {
        fail("getpgid(-1) should return error (negative value)");
    }

    let result = getsid(-1);
    println!("  getsid(-1) = {}", result);

    if result >= 0 {
        fail("getsid(-1) should return error (negative value)");
    }

    println!("  test_error_cases: PASS");
}

fn main() {
    println!("=== Session Syscall Tests ===");

    test_getpgid_self();
    test_getpgid_with_pid();
    test_setpgid_self();
    test_getpgrp();
    test_getsid_self();
    test_getsid_with_pid();
    test_setsid_in_child();
    test_error_cases();

    println!("\n=== All session tests passed! ===");
    println!("SESSION_TEST_PASSED");
    std::process::exit(0);
}
