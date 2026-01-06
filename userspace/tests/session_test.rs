//! Session and process group syscall tests
//!
//! Tests POSIX session and process group syscalls:
//! - getpgid()/setpgid() - process group get/set
//! - getpgrp() - get calling process's process group
//! - getsid()/setsid() - session get/create

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::types::fd;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: i64) {
    io::print(prefix);

    if num < 0 {
        io::print("-");
        print_unsigned("", (-num) as u64);
    } else {
        print_unsigned("", num as u64);
    }
}

/// Convert unsigned number to string and print it
unsafe fn print_unsigned(prefix: &str, num: u64) {
    io::print(prefix);

    let mut n = num;
    let mut i = 0;

    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &BUFFER[..i]);
    io::print("\n");
}

/// Helper to exit with error message
fn fail(msg: &str) -> ! {
    io::print("SESSION_TEST: FAIL - ");
    io::print(msg);
    io::print("\n");
    process::exit(1);
}

/// Test getpgid(0) returns current process's pgid
unsafe fn test_getpgid_self() {
    io::print("\nTest 1: getpgid(0) returns current process's pgid\n");

    let pgid = process::getpgid(0);

    if pgid <= 0 {
        print_number("  getpgid(0) returned: ", pgid as i64);
        fail("getpgid(0) should return positive value");
    }

    print_number("  getpgid(0) = ", pgid as i64);
    io::print("  test_getpgid_self: PASS\n");
}

/// Test getpgid(getpid()) returns same as getpgid(0)
unsafe fn test_getpgid_with_pid() {
    io::print("\nTest 2: getpgid(getpid()) returns same as getpgid(0)\n");

    let pid = process::getpid() as i32;
    let pgid_0 = process::getpgid(0);
    let pgid_pid = process::getpgid(pid);

    print_number("  pid = ", pid as i64);
    print_number("  getpgid(0) = ", pgid_0 as i64);
    print_number("  getpgid(pid) = ", pgid_pid as i64);

    if pgid_0 != pgid_pid {
        fail("getpgid(0) should equal getpgid(getpid())");
    }

    io::print("  test_getpgid_with_pid: PASS\n");
}

/// Test setpgid(0, 0) sets pgid to own pid
unsafe fn test_setpgid_self() {
    io::print("\nTest 3: setpgid(0, 0) sets pgid to own pid\n");

    let pid = process::getpid() as i32;

    // Set ourselves as our own process group leader
    let result = process::setpgid(0, 0);

    print_number("  pid = ", pid as i64);
    print_number("  setpgid(0, 0) returned: ", result as i64);

    if result != 0 {
        fail("setpgid(0, 0) should succeed");
    }

    // Verify pgid now equals pid
    let pgid = process::getpgid(0);
    print_number("  getpgid(0) after setpgid = ", pgid as i64);

    if pgid != pid {
        fail("after setpgid(0, 0), pgid should equal pid");
    }

    io::print("  test_setpgid_self: PASS\n");
}

/// Test getpgrp() returns same as getpgid(0)
unsafe fn test_getpgrp() {
    io::print("\nTest 4: getpgrp() returns same as getpgid(0)\n");

    let pgrp = process::getpgrp();
    let pgid = process::getpgid(0);

    print_number("  getpgrp() = ", pgrp as i64);
    print_number("  getpgid(0) = ", pgid as i64);

    if pgrp != pgid {
        fail("getpgrp() should equal getpgid(0)");
    }

    io::print("  test_getpgrp: PASS\n");
}

/// Test getsid(0) returns current session id
unsafe fn test_getsid_self() {
    io::print("\nTest 5: getsid(0) returns current session id\n");

    let sid = process::getsid(0);

    if sid <= 0 {
        print_number("  getsid(0) returned: ", sid as i64);
        fail("getsid(0) should return positive value");
    }

    print_number("  getsid(0) = ", sid as i64);
    io::print("  test_getsid_self: PASS\n");
}

/// Test getsid(getpid()) returns same as getsid(0)
unsafe fn test_getsid_with_pid() {
    io::print("\nTest 6: getsid(getpid()) returns same as getsid(0)\n");

    let pid = process::getpid() as i32;
    let sid_0 = process::getsid(0);
    let sid_pid = process::getsid(pid);

    print_number("  pid = ", pid as i64);
    print_number("  getsid(0) = ", sid_0 as i64);
    print_number("  getsid(pid) = ", sid_pid as i64);

    if sid_0 != sid_pid {
        fail("getsid(0) should equal getsid(getpid())");
    }

    io::print("  test_getsid_with_pid: PASS\n");
}

/// Test setsid() in child process
unsafe fn test_setsid_in_child() {
    io::print("\nTest 7: setsid() in child creates new session\n");

    let fork_result = process::fork();

    if fork_result < 0 {
        print_number("  fork() failed with error: ", fork_result);
        fail("fork failed");
    }

    if fork_result == 0 {
        // Child process
        let my_pid = process::getpid() as i32;
        print_number("  CHILD: pid = ", my_pid as i64);

        // First put ourselves in our own process group
        // (setsid() requires we're not already a process group leader)
        let setpgid_result = process::setpgid(0, 0);
        print_number("  CHILD: setpgid(0, 0) returned: ", setpgid_result as i64);

        // Now call setsid to create a new session
        let new_sid = process::setsid();
        print_number("  CHILD: setsid() returned: ", new_sid as i64);

        if new_sid < 0 {
            io::print("  CHILD: setsid() failed\n");
            process::exit(1);
        }

        // After setsid(): sid == pgid == pid
        let sid = process::getsid(0);
        let pgid = process::getpgid(0);

        print_number("  CHILD: getsid(0) = ", sid as i64);
        print_number("  CHILD: getpgid(0) = ", pgid as i64);

        // Verify sid == pid and pgid == pid
        if sid != my_pid {
            io::print("  CHILD: ERROR - sid should equal pid after setsid\n");
            process::exit(1);
        }

        if pgid != my_pid {
            io::print("  CHILD: ERROR - pgid should equal pid after setsid\n");
            process::exit(1);
        }

        io::print("  CHILD: setsid test PASS\n");
        process::exit(0);
    } else {
        // Parent: wait for child
        let child_pid = fork_result as i32;
        print_number("  PARENT: waiting for child ", child_pid as i64);

        let mut status: i32 = 0;
        let result = process::waitpid(child_pid, &mut status as *mut i32, 0);

        print_number("  PARENT: waitpid returned: ", result);

        if result != fork_result {
            fail("waitpid returned wrong pid");
        }

        // Check if child exited normally with code 0
        if !process::wifexited(status) {
            print_number("  PARENT: child did not exit normally, status = ", status as i64);
            fail("child did not exit normally");
        }

        let exit_code = process::wexitstatus(status);
        print_number("  PARENT: child exit code = ", exit_code as i64);

        if exit_code != 0 {
            fail("child reported test failure");
        }

        io::print("  test_setsid_in_child: PASS\n");
    }
}

/// Test error cases for invalid PIDs
unsafe fn test_error_cases() {
    io::print("\nTest 8: Error cases for invalid PIDs\n");

    // getpgid with invalid PID should return negative error (ESRCH = 3)
    let result = process::getpgid(-1);
    print_number("  getpgid(-1) = ", result as i64);

    if result >= 0 {
        fail("getpgid(-1) should return error (negative value)");
    }

    // getsid with invalid PID should return negative error
    let result = process::getsid(-1);
    print_number("  getsid(-1) = ", result as i64);

    if result >= 0 {
        fail("getsid(-1) should return error (negative value)");
    }

    io::print("  test_error_cases: PASS\n");
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Session Syscall Tests ===\n");

        // Run all tests
        test_getpgid_self();
        test_getpgid_with_pid();
        test_setpgid_self();
        test_getpgrp();
        test_getsid_self();
        test_getsid_with_pid();
        test_setsid_in_child();
        test_error_cases();

        io::print("\n=== All session tests passed! ===\n");
        io::print("SESSION_TEST_PASSED\n");
        process::exit(0);
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in session_test!\n");
    process::exit(255);
}
