//! Integration test for echo coreutil with argv support (std version)
//!
//! This test verifies that echo correctly prints multiple command-line
//! arguments separated by spaces.

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

/// Fork and exec echo with no arguments
fn run_echo_no_args() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        let program = b"echo\0";
        let arg0 = b"echo\0".as_ptr();
        let argv: [*const u8; 2] = [arg0, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        std::process::exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0) };

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        -1
    }
}

/// Fork and exec echo with a single argument
fn run_echo_single_arg() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        let program = b"echo\0";
        let arg0 = b"echo\0".as_ptr();
        let arg1 = b"hello\0".as_ptr();
        let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        std::process::exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0) };

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        -1
    }
}

/// Fork and exec echo with multiple arguments
fn run_echo_multi_args() -> i32 {
    let pid = unsafe { fork() };
    if pid == 0 {
        let program = b"echo\0";
        let arg0 = b"echo\0".as_ptr();
        let arg1 = b"hello\0".as_ptr();
        let arg2 = b"world\0".as_ptr();
        let arg3 = b"test\0".as_ptr();
        let argv: [*const u8; 5] = [arg0, arg1, arg2, arg3, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe { execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        std::process::exit(127);
    } else if pid > 0 {
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0) };

        if wifexited(status) {
            wexitstatus(status)
        } else {
            -1
        }
    } else {
        -1
    }
}

fn main() {
    println!("=== Echo Argv Integration Test ===");

    // Test 1: echo with no arguments (should just print newline)
    println!("Test 1: echo with no arguments");
    let result1 = run_echo_no_args();
    if result1 != 0 {
        println!("ECHO_ARGV_TEST_FAILED: echo with no args returned non-zero");
        std::process::exit(1);
    }
    println!("  echo with no args succeeded");

    // Test 2: echo with single argument
    println!("Test 2: echo with single argument");
    let result2 = run_echo_single_arg();
    if result2 != 0 {
        println!("ECHO_ARGV_TEST_FAILED: echo with single arg returned non-zero");
        std::process::exit(1);
    }
    println!("  echo with single arg succeeded");

    // Test 3: echo with multiple arguments
    println!("Test 3: echo with multiple arguments");
    let result3 = run_echo_multi_args();
    if result3 != 0 {
        println!("ECHO_ARGV_TEST_FAILED: echo with multiple args returned non-zero");
        std::process::exit(1);
    }
    println!("  echo with multiple args succeeded");

    println!("ECHO_ARGV_TEST_PASSED");
    std::process::exit(0);
}
