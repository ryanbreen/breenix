//! Test exec from ext2 filesystem (std version)
//!
//! Tests that execv() can load programs from the ext2 filesystem:
//! - /bin/hello_world should load and run
//! - /nonexistent should return ENOENT
//! - /hello.txt (not executable) should return EACCES
//! - /test (directory) should return ENOTDIR/EACCES
//! - /bin/ls should load and run

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn main() {
    println!("=== Exec from Ext2 Filesystem Test ===");
    println!("EXEC_EXT2_TEST_START");
    println!("DEBUG_MARKER_1");

    let mut tests_failed = 0;
    println!("DEBUG_MARKER_2");

    // Test 1: exec("/bin/hello_world", ...) should succeed
    println!("Test 1: exec /bin/hello_world");
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec /bin/hello_world from ext2
        let program = b"/bin/hello_world\0";
        let arg0 = b"/bin/hello_world\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let result = unsafe {
            execve(program.as_ptr(), argv.as_ptr(), std::ptr::null())
        };
        // If we get here, exec failed
        println!("exec /bin/hello_world failed");
        println!("exec returned: {}", result);
        std::process::exit(result);
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0); }

        println!("Test 1: waitpid status={}, wifexited={}, exitcode={}",
            status, wifexited(status), wexitstatus(status));

        // hello_world exits with code 42 as its signature
        if wifexited(status) && wexitstatus(status) == 42 {
            println!("EXEC_EXT2_BIN_OK");
        } else {
            println!("EXEC_EXT2_BIN_FAILED");
            tests_failed += 1;
        }
    } else {
        println!("fork failed");
        tests_failed += 1;
    }
    println!("DEBUG_MARKER_AFTER_TEST1");

    // Test 2: exec("/nonexistent_binary", ...) should return ENOENT (2)
    println!("Test 2: exec nonexistent file");
    let pid = unsafe { fork() };
    if pid == 0 {
        let program = b"/nonexistent_binary\0";
        let arg0 = b"/nonexistent_binary\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let result = unsafe {
            execve(program.as_ptr(), argv.as_ptr(), std::ptr::null())
        };
        // exec should fail, check if errno is ENOENT (2)
        if result == -2 {
            std::process::exit(0); // ENOENT as expected
        } else {
            std::process::exit(result); // Unexpected error
        }
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0); }

        if wifexited(status) && wexitstatus(status) == 0 {
            println!("EXEC_EXT2_ENOENT_OK");
        } else {
            println!("EXEC_EXT2_ENOENT_FAILED");
            tests_failed += 1;
        }
    } else {
        println!("fork failed");
        tests_failed += 1;
    }

    // Test 3: exec("/hello.txt", ...) should return EACCES (13) - not executable
    println!("Test 3: exec non-executable file");
    let pid = unsafe { fork() };
    if pid == 0 {
        let program = b"/hello.txt\0";
        let arg0 = b"/hello.txt\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let result = unsafe {
            execve(program.as_ptr(), argv.as_ptr(), std::ptr::null())
        };
        if result == -13 {
            std::process::exit(0); // EACCES as expected
        } else {
            std::process::exit(result); // Unexpected error
        }
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0); }

        if wifexited(status) && wexitstatus(status) == 0 {
            println!("EXEC_EXT2_EACCES_OK");
        } else {
            println!("EXEC_EXT2_EACCES_FAILED");
            tests_failed += 1;
        }
    } else {
        println!("fork failed");
        tests_failed += 1;
    }

    // Test 4: exec("/test", ...) should return ENOTDIR/EACCES for directory
    println!("Test 4: exec directory");
    let pid = unsafe { fork() };
    if pid == 0 {
        let program = b"/test\0";
        let arg0 = b"/test\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];

        let result = unsafe {
            execve(program.as_ptr(), argv.as_ptr(), std::ptr::null())
        };
        if result == -20 || result == -13 {
            std::process::exit(0); // Expected error
        } else {
            std::process::exit(result); // Unexpected error
        }
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0); }

        if wifexited(status) && wexitstatus(status) == 0 {
            println!("EXEC_EXT2_ENOTDIR_OK");
        } else {
            println!("EXEC_EXT2_ENOTDIR_FAILED");
            tests_failed += 1;
        }
    } else {
        println!("fork failed");
        tests_failed += 1;
    }

    // Test 5: exec("/bin/ls", ...) should succeed and exit 0
    println!("Test 5: exec /bin/ls");
    let pid = unsafe { fork() };
    if pid == 0 {
        let program = b"/bin/ls\0";
        let arg0 = b"ls\0".as_ptr();
        let arg1 = b"/\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

        let result = unsafe {
            execve(program.as_ptr(), argv.as_ptr(), std::ptr::null())
        };
        // If we get here, exec failed
        println!("exec /bin/ls failed: {}", -result);
        std::process::exit(result);
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0); }

        if wifexited(status) && wexitstatus(status) == 0 {
            println!("EXEC_EXT2_LS_OK");
        } else {
            println!("EXEC_EXT2_LS_FAILED (status={}, exit={})", status, wexitstatus(status));
            tests_failed += 1;
        }
    } else {
        println!("fork failed");
        tests_failed += 1;
    }

    // Summary
    if tests_failed == 0 {
        println!("EXEC_EXT2_TEST_PASSED");
        std::process::exit(0);
    } else {
        println!("EXEC_EXT2_TEST_FAILED");
        std::process::exit(1);
    }
}
