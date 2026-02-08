//! Integration test for mkdir and rmdir coreutils with argv support (std version)
//!
//! This test verifies that mkdir and rmdir correctly parse command-line
//! arguments passed via fork+exec.

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn access(path: *const u8, mode: i32) -> i32;
}

const F_OK: i32 = 0;

/// Check if a path exists using access()
fn path_exists(path: &[u8]) -> bool {
    unsafe { access(path.as_ptr(), F_OK) == 0 }
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Fork and exec mkdir with the test directory as argument
fn run_mkdir() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec mkdir with directory argument
        let program = b"mkdir\0";
        let arg0 = b"mkdir\0".as_ptr();
        let arg1 = b"/test_mkdir_argv\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        // If we get here, exec failed
        println!("  exec mkdir failed");
        std::process::exit(127);
    } else if pid > 0 {
        // Parent: wait for child
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0) };

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        println!("  fork failed");
        -1
    }
}

/// Fork and exec rmdir with the test directory as argument
fn run_rmdir() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec rmdir with directory argument
        let program = b"rmdir\0";
        let arg0 = b"rmdir\0".as_ptr();
        let arg1 = b"/test_mkdir_argv\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        // If we get here, exec failed
        println!("  exec rmdir failed");
        std::process::exit(127);
    } else if pid > 0 {
        // Parent: wait for child
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0) };

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        println!("  fork failed");
        -1
    }
}

fn main() {
    println!("=== Mkdir/Rmdir Argv Integration Test ===");

    // First, ensure the test directory doesn't exist
    if path_exists(b"/test_mkdir_argv\0") {
        println!("Test directory already exists, cleaning up first...");
        run_rmdir();
    }

    // Test 1: Create directory via mkdir with argv
    println!("Test 1: mkdir with directory argument");
    let mkdir_result = run_mkdir();
    if mkdir_result != 0 {
        println!("MKDIR_ARGV_TEST_FAILED: mkdir returned non-zero");
        std::process::exit(1);
    }

    // Verify directory was created
    if !path_exists(b"/test_mkdir_argv\0") {
        println!("MKDIR_ARGV_TEST_FAILED: directory not created");
        std::process::exit(1);
    }
    println!("  mkdir created directory successfully");

    // Test 2: Remove directory via rmdir with argv
    println!("Test 2: rmdir with directory argument");
    let rmdir_result = run_rmdir();
    if rmdir_result != 0 {
        println!("MKDIR_ARGV_TEST_FAILED: rmdir returned non-zero");
        std::process::exit(1);
    }

    // Verify directory was removed
    if path_exists(b"/test_mkdir_argv\0") {
        println!("MKDIR_ARGV_TEST_FAILED: directory not removed");
        std::process::exit(1);
    }
    println!("  rmdir removed directory successfully");

    println!("MKDIR_ARGV_TEST_PASSED");
    std::process::exit(0);
}
