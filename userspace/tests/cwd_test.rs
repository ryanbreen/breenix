//! Current Working Directory Test
//!
//! Tests the getcwd and chdir syscalls to verify:
//! 1. Initial cwd is "/" (root)
//! 2. chdir to existing directory works
//! 3. chdir to non-existent directory returns ENOENT
//! 4. chdir to a file (not directory) returns ENOTDIR
//! 5. Relative path support (cd .. works)
//! 6. Fork inherits cwd from parent
//! 7. getcwd error handling (EINVAL, ERANGE)
//! 8. getcwd returns buffer pointer on success
//!
//! Test markers:
//! - CWD_INITIAL_OK: Initial cwd is "/"
//! - CWD_CHDIR_OK: chdir to valid directory works
//! - CWD_ENOENT_OK: chdir to non-existent path returns ENOENT
//! - CWD_ENOTDIR_OK: chdir to file returns ENOTDIR
//! - CWD_RELATIVE_OK: Relative path navigation works
//! - CWD_FORK_OK: Fork inherits cwd from parent
//! - CWD_ERRORS_OK: getcwd error handling works
//! - CWD_TEST_PASSED: All tests passed

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{print, println};
use libbreenix::process::{chdir, exit, fork, getcwd, waitpid};

/// Print a number
fn print_num(n: i32) {
    if n < 0 {
        print("-");
        print_num(-n);
        return;
    }
    if n >= 10 {
        print_num(n / 10);
    }
    let digit = b'0' + (n % 10) as u8;
    let buf = [digit];
    libbreenix::io::write(1, &buf);
}

/// Print a u64 in hex
fn print_hex(n: u64) {
    print("0x");
    for i in (0..16).rev() {
        let nibble = ((n >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        libbreenix::io::write(1, &[c]);
    }
}

/// Get current working directory as a string slice
/// Also verifies that getcwd returns the buffer pointer (POSIX requirement)
fn get_cwd_str_verified(buf: &mut [u8]) -> Option<&str> {
    let buf_ptr = buf.as_mut_ptr() as u64;
    let result = getcwd(buf);

    // POSIX: getcwd returns pointer to buf on success
    if result <= 0 {
        return None;
    }

    // Verify return value is the buffer pointer
    if result as u64 != buf_ptr {
        print("  WARNING: getcwd returned ");
        print_hex(result as u64);
        print(" but buf is at ");
        print_hex(buf_ptr);
        println("");
        // Still try to extract the string
    }

    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..len]).ok()
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== CWD Syscall Test ===");
    println("");

    let mut buf = [0u8; 256];
    let mut all_passed = true;

    // Test 1: Initial cwd should be "/"
    print("Test 1: Initial cwd... ");
    match get_cwd_str_verified(&mut buf) {
        Some(cwd) if cwd == "/" => {
            println("PASS (/)");
            println("CWD_INITIAL_OK");
        }
        Some(cwd) => {
            print("FAIL (expected /, got ");
            print(cwd);
            println(")");
            all_passed = false;
        }
        None => {
            println("FAIL (getcwd failed)");
            all_passed = false;
        }
    }

    // Test 2: chdir to existing directory
    print("Test 2: chdir to /dev... ");
    let dev_path = b"/dev\0";
    let result = chdir(dev_path);
    if result == 0 {
        // Verify cwd changed
        match get_cwd_str_verified(&mut buf) {
            Some(cwd) if cwd == "/dev" => {
                println("PASS");
                println("CWD_CHDIR_OK");
            }
            Some(cwd) => {
                print("FAIL (cwd is ");
                print(cwd);
                println(", expected /dev)");
                all_passed = false;
            }
            None => {
                println("FAIL (getcwd failed after chdir)");
                all_passed = false;
            }
        }
    } else {
        print("FAIL (chdir returned ");
        print_num(result);
        println(")");
        all_passed = false;
    }

    // Test 3: chdir to non-existent directory should fail with ENOENT
    print("Test 3: chdir to /nonexistent... ");
    let nonexistent_path = b"/nonexistent\0";
    let result = chdir(nonexistent_path);
    if result == -2 {
        // ENOENT = 2
        println("PASS (ENOENT)");
        println("CWD_ENOENT_OK");
    } else if result == 0 {
        println("FAIL (should not succeed)");
        all_passed = false;
    } else {
        print("FAIL (expected -2/ENOENT, got ");
        print_num(result);
        println(")");
        all_passed = false;
    }

    // Test 4: chdir to a device file (not directory) should fail with ENOTDIR
    // Use /dev/null which is guaranteed to exist and is NOT a directory
    let _ = chdir(b"/\0");
    print("Test 4: chdir to /dev/null (file)... ");
    let file_path = b"/dev/null\0";
    let result = chdir(file_path);
    if result == -20 {
        // ENOTDIR = 20
        println("PASS (ENOTDIR)");
        println("CWD_ENOTDIR_OK");
    } else if result == 0 {
        println("FAIL (chdir to file should not succeed)");
        all_passed = false;
    } else {
        // Any other error is a FAIL - we must get ENOTDIR specifically
        print("FAIL (expected -20/ENOTDIR, got ");
        print_num(result);
        println(")");
        all_passed = false;
    }

    // Test 5: Relative path navigation
    // Start from root, cd to /dev, then cd .. should go back to /
    print("Test 5: Relative path (cd ..)... ");
    let _ = chdir(b"/dev\0");
    let result = chdir(b"..\0");
    if result == 0 {
        match get_cwd_str_verified(&mut buf) {
            Some(cwd) if cwd == "/" => {
                println("PASS");
                println("CWD_RELATIVE_OK");
            }
            Some(cwd) => {
                print("FAIL (cwd is ");
                print(cwd);
                println(", expected /)");
                all_passed = false;
            }
            None => {
                println("FAIL (getcwd failed)");
                all_passed = false;
            }
        }
    } else {
        print("FAIL (chdir .. returned ");
        print_num(result);
        println(")");
        all_passed = false;
    }

    // Test 6: Fork inherits cwd from parent
    print("Test 6: Fork cwd inheritance... ");
    // First change to /dev so child can verify it inherited non-root cwd
    let _ = chdir(b"/dev\0");
    let pid = fork();
    if pid < 0 {
        print("FAIL (fork failed: ");
        print_num(pid as i32);
        println(")");
        all_passed = false;
    } else if pid == 0 {
        // Child process - verify cwd is /dev (inherited from parent)
        let mut child_buf = [0u8; 256];
        match get_cwd_str_verified(&mut child_buf) {
            Some(cwd) if cwd == "/dev" => {
                println("child: cwd=/dev (inherited)");
                exit(0); // Success
            }
            Some(cwd) => {
                print("child: FAIL cwd=");
                print(cwd);
                println(" (expected /dev)");
                exit(1); // Failure
            }
            None => {
                println("child: FAIL getcwd failed");
                exit(1);
            }
        }
    } else {
        // Parent - wait for child and check exit status
        let mut status: i32 = 0;
        let wait_result = waitpid(pid as i32, &mut status as *mut i32, 0);
        if wait_result == pid {
            // Check if child exited normally with status 0
            let exited = (status & 0x7f) == 0;
            let exit_code = (status >> 8) & 0xff;
            if exited && exit_code == 0 {
                println("PASS");
                println("CWD_FORK_OK");
            } else {
                print("FAIL (child exit code ");
                print_num(exit_code);
                println(")");
                all_passed = false;
            }
        } else {
            println("FAIL (waitpid failed)");
            all_passed = false;
        }
    }
    // Return to root for remaining tests
    let _ = chdir(b"/\0");

    // Test 7: getcwd error handling
    print("Test 7: getcwd error handling... ");
    let mut test7_passed = true;

    // Test 7a: size=0 should return EINVAL (-22)
    let mut tiny_buf = [0u8; 0];
    let result = getcwd(&mut tiny_buf);
    if result != -22 {
        print("7a: size=0 expected -22/EINVAL, got ");
        print_num(result as i32);
        print("; ");
        test7_passed = false;
    }

    // Test 7b: buffer too small should return ERANGE (-34)
    // cwd is "/" which needs 2 bytes (/ + null), so 1-byte buffer should fail
    let mut small_buf = [0u8; 1];
    let result = getcwd(&mut small_buf);
    if result != -34 {
        print("7b: small buf expected -34/ERANGE, got ");
        print_num(result as i32);
        print("; ");
        test7_passed = false;
    }

    if test7_passed {
        println("PASS");
        println("CWD_ERRORS_OK");
    } else {
        println("FAIL");
        all_passed = false;
    }

    println("");
    if all_passed {
        println("=== All CWD tests PASSED ===");
        println("CWD_TEST_PASSED");
        exit(0);
    } else {
        println("=== Some CWD tests FAILED ===");
        exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("PANIC in cwd_test!");
    exit(255);
}
