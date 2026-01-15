//! Test exec from ext2 filesystem
//!
//! Tests that execv() can load programs from the ext2 filesystem:
//! - /bin/hello_world should load and run
//! - /nonexistent should return ENOENT
//! - /hello.txt (not executable) should return EACCES

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

/// Helper to print a number (for debugging)
fn print_num(mut n: u64) {
    if n == 0 {
        libbreenix::io::print("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    // Reverse and print
    while i > 0 {
        i -= 1;
        let c = [buf[i]];
        if let Ok(s) = core::str::from_utf8(&c) {
            libbreenix::io::print(s);
        }
    }
}

/// Test exec with path-based loading from ext2 filesystem.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Exec from Ext2 Filesystem Test ===");
    println("EXEC_EXT2_TEST_START");
    println("DEBUG_MARKER_1");

    let mut tests_failed = 0;
    println("DEBUG_MARKER_2");

    // Test 1: exec("/bin/hello_world", ...) should succeed
    println("Test 1: exec /bin/hello_world");
    let pid = fork();
    if pid == 0 {
        // Child: exec /bin/hello_world from ext2
        let program = b"/bin/hello_world\0";
        let arg0 = b"/bin/hello_world\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let result = execv(program, argv.as_ptr());
        // If we get here, exec failed
        println("exec /bin/hello_world failed");
        // Print the error code for debugging
        libbreenix::io::print("exec returned: ");
        // Manually print the number (simple conversion)
        if result < 0 {
            libbreenix::io::print("-");
            let abs_val = (-result) as u64;
            print_num(abs_val);
        } else {
            print_num(result as u64);
        }
        libbreenix::io::print("\n");
        exit(result as i32);
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        // Print diagnostic info
        libbreenix::io::print("Test 1: waitpid status=");
        print_num(status as u64);
        libbreenix::io::print(", wifexited=");
        if wifexited(status) {
            libbreenix::io::print("true");
        } else {
            libbreenix::io::print("false");
        }
        libbreenix::io::print(", exitcode=");
        print_num(wexitstatus(status) as u64);
        libbreenix::io::print("\n");

        // hello_world exits with code 42 as its signature
        // We just need to verify the program executed and exited cleanly
        if wifexited(status) && wexitstatus(status) == 42 {
            println("EXEC_EXT2_BIN_OK");
            // Test passed - binary loaded from ext2 and ran successfully
        } else {
            println("EXEC_EXT2_BIN_FAILED");
            tests_failed += 1;
        }
    } else {
        println("fork failed");
        tests_failed += 1;
    }
    println("DEBUG_MARKER_AFTER_TEST1");

    // Test 2: exec("/nonexistent_binary", ...) should return ENOENT (2)
    println("Test 2: exec nonexistent file");
    let pid = fork();
    if pid == 0 {
        let program = b"/nonexistent_binary\0";
        let arg0 = b"/nonexistent_binary\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let result = execv(program, argv.as_ptr());
        // exec should fail, check if errno is ENOENT (2)
        if result == -2 {
            exit(0); // ENOENT as expected
        } else {
            exit(result as i32); // Unexpected error
        }
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) && wexitstatus(status) == 0 {
            println("EXEC_EXT2_ENOENT_OK");
            // Test passed
        } else {
            println("EXEC_EXT2_ENOENT_FAILED");
            tests_failed += 1;
        }
    } else {
        println("fork failed");
        tests_failed += 1;
    }

    // Test 3: exec("/hello.txt", ...) should return EACCES (13) - not executable
    println("Test 3: exec non-executable file");
    let pid = fork();
    if pid == 0 {
        let program = b"/hello.txt\0";
        let arg0 = b"/hello.txt\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let result = execv(program, argv.as_ptr());
        // exec should fail with EACCES (13)
        if result == -13 {
            exit(0); // EACCES as expected
        } else {
            exit(result as i32); // Unexpected error
        }
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) && wexitstatus(status) == 0 {
            println("EXEC_EXT2_EACCES_OK");
            // Test passed
        } else {
            println("EXEC_EXT2_EACCES_FAILED");
            tests_failed += 1;
        }
    } else {
        println("fork failed");
        tests_failed += 1;
    }

    // Test 4: exec("/test", ...) should return ENOTDIR/EACCES for directory
    println("Test 4: exec directory");
    let pid = fork();
    if pid == 0 {
        let program = b"/test\0";
        let arg0 = b"/test\0" as *const u8;
        let argv: [*const u8; 2] = [arg0, core::ptr::null()];

        let result = execv(program, argv.as_ptr());
        // exec should fail with ENOTDIR (20) or EACCES (13)
        if result == -20 || result == -13 {
            exit(0); // Expected error
        } else {
            exit(result as i32); // Unexpected error
        }
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) && wexitstatus(status) == 0 {
            println("EXEC_EXT2_ENOTDIR_OK");
            // Test passed
        } else {
            println("EXEC_EXT2_ENOTDIR_FAILED");
            tests_failed += 1;
        }
    } else {
        println("fork failed");
        tests_failed += 1;
    }

    // Test 5: exec("/bin/ls", ...) should succeed and exit 0
    // This tests that ls specifically works, not just hello_world
    println("Test 5: exec /bin/ls");
    let pid = fork();
    if pid == 0 {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0" as *const u8;
        let arg1 = b"/\0" as *const u8;
        let argv: [*const u8; 3] = [arg0, arg1, core::ptr::null()];

        let result = execv(program, argv.as_ptr());
        // If we get here, exec failed
        libbreenix::io::print("exec /bin/ls failed: ");
        print_num((-result) as u64);
        libbreenix::io::print("\n");
        exit(result as i32);
    } else if pid > 0 {
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        // ls should exit with code 0 on success
        if wifexited(status) && wexitstatus(status) == 0 {
            println("EXEC_EXT2_LS_OK");
            // Test passed
        } else {
            libbreenix::io::print("EXEC_EXT2_LS_FAILED (status=");
            print_num(status as u64);
            libbreenix::io::print(", exit=");
            print_num(wexitstatus(status) as u64);
            libbreenix::io::print(")\n");
            tests_failed += 1;
        }
    } else {
        println("fork failed");
        tests_failed += 1;
    }

    // Summary
    if tests_failed == 0 {
        println("EXEC_EXT2_TEST_PASSED");
        exit(0);
    } else {
        println("EXEC_EXT2_TEST_FAILED");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    exit(255);
}
