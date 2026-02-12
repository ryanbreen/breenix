//! Test exec from ext2 filesystem (std version)
//!
//! Tests that execv() can load programs from the ext2 filesystem:
//! - /bin/hello_world should load and run
//! - /nonexistent should return ENOENT
//! - /hello.txt (not executable) should return EACCES
//! - /test (directory) should return ENOTDIR/EACCES
//! - /bin/ls should load and run

use libbreenix::process::{fork, waitpid, execv, wifexited, wexitstatus, ForkResult};
use libbreenix::error::Error;
use libbreenix::Errno;

fn main() {
    println!("=== Exec from Ext2 Filesystem Test ===");
    println!("EXEC_EXT2_TEST_START");
    println!("DEBUG_MARKER_1");

    let mut tests_failed = 0;
    println!("DEBUG_MARKER_2");

    // Test 1: exec("/bin/hello_world", ...) should succeed
    println!("Test 1: exec /bin/hello_world");
    match fork() {
        Ok(ForkResult::Child) => {
            // Child: exec /bin/hello_world from ext2
            let program = b"/bin/hello_world\0";
            let arg0 = b"/bin/hello_world\0".as_ptr();
            let argv: [*const u8; 2] = [arg0, std::ptr::null()];

            let result = execv(program, argv.as_ptr());
            // If we get here, exec failed
            println!("exec /bin/hello_world failed");
            if let Err(Error::Os(e)) = result {
                println!("exec returned: errno {:?}", e);
            }
            std::process::exit(1);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

            println!("Test 1: waitpid status={}, wifexited={}, exitcode={}",
                status, wifexited(status), wexitstatus(status));

            // hello_world (hello_std_real) exits with code 0 on success
            if wifexited(status) && wexitstatus(status) == 0 {
                println!("EXEC_EXT2_BIN_OK");
            } else {
                println!("EXEC_EXT2_BIN_FAILED");
                tests_failed += 1;
            }
        }
        Err(_) => {
            println!("fork failed");
            tests_failed += 1;
        }
    }
    println!("DEBUG_MARKER_AFTER_TEST1");

    // Test 2: exec("/nonexistent_binary", ...) should return ENOENT (2)
    println!("Test 2: exec nonexistent file");
    match fork() {
        Ok(ForkResult::Child) => {
            let program = b"/nonexistent_binary\0";
            let arg0 = b"/nonexistent_binary\0".as_ptr();
            let argv: [*const u8; 2] = [arg0, std::ptr::null()];

            let result = execv(program, argv.as_ptr());
            // exec should fail, check if errno is ENOENT (2)
            if let Err(Error::Os(Errno::ENOENT)) = result {
                std::process::exit(0); // ENOENT as expected
            } else {
                std::process::exit(1); // Unexpected error
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

            if wifexited(status) && wexitstatus(status) == 0 {
                println!("EXEC_EXT2_ENOENT_OK");
            } else {
                println!("EXEC_EXT2_ENOENT_FAILED");
                tests_failed += 1;
            }
        }
        Err(_) => {
            println!("fork failed");
            tests_failed += 1;
        }
    }

    // Test 3: exec("/hello.txt", ...) should return EACCES (13) - not executable
    println!("Test 3: exec non-executable file");
    match fork() {
        Ok(ForkResult::Child) => {
            let program = b"/hello.txt\0";
            let arg0 = b"/hello.txt\0".as_ptr();
            let argv: [*const u8; 2] = [arg0, std::ptr::null()];

            let result = execv(program, argv.as_ptr());
            if let Err(Error::Os(Errno::EACCES)) = result {
                std::process::exit(0); // EACCES as expected
            } else {
                std::process::exit(1); // Unexpected error
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

            if wifexited(status) && wexitstatus(status) == 0 {
                println!("EXEC_EXT2_EACCES_OK");
            } else {
                println!("EXEC_EXT2_EACCES_FAILED");
                tests_failed += 1;
            }
        }
        Err(_) => {
            println!("fork failed");
            tests_failed += 1;
        }
    }

    // Test 4: exec("/test", ...) should return ENOTDIR/EACCES for directory
    println!("Test 4: exec directory");
    match fork() {
        Ok(ForkResult::Child) => {
            let program = b"/test\0";
            let arg0 = b"/test\0".as_ptr();
            let argv: [*const u8; 2] = [arg0, std::ptr::null()];

            let result = execv(program, argv.as_ptr());
            if let Err(Error::Os(Errno::ENOTDIR)) = result {
                std::process::exit(0); // Expected error
            } else if let Err(Error::Os(Errno::EACCES)) = result {
                std::process::exit(0); // Expected error
            } else {
                std::process::exit(1); // Unexpected error
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

            if wifexited(status) && wexitstatus(status) == 0 {
                println!("EXEC_EXT2_ENOTDIR_OK");
            } else {
                println!("EXEC_EXT2_ENOTDIR_FAILED");
                tests_failed += 1;
            }
        }
        Err(_) => {
            println!("fork failed");
            tests_failed += 1;
        }
    }

    // Test 5: exec("/bin/ls", ...) should succeed and exit 0
    println!("Test 5: exec /bin/ls");
    match fork() {
        Ok(ForkResult::Child) => {
            let program = b"/bin/ls\0";
            let arg0 = b"ls\0".as_ptr();
            let arg1 = b"/\0".as_ptr();
            let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

            let result = execv(program, argv.as_ptr());
            // If we get here, exec failed
            if let Err(Error::Os(e)) = result {
                println!("exec /bin/ls failed: errno={:?}", e);
            }
            std::process::exit(1);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status: i32 = 0;
            let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

            if wifexited(status) && wexitstatus(status) == 0 {
                println!("EXEC_EXT2_LS_OK");
            } else {
                println!("EXEC_EXT2_LS_FAILED (status={}, exit={})", status, wexitstatus(status));
                tests_failed += 1;
            }
        }
        Err(_) => {
            println!("fork failed");
            tests_failed += 1;
        }
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
