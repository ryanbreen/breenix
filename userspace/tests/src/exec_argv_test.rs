//! Exec argv test program (std version)
//!
//! Tests that fork+exec with argv works correctly.
//! - Parent forks
//! - Child execs argv_test with args: ["argv_test", "hello", "world"]
//! - Parent waits and checks exit status (0 = pass)

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
    println!("=== Exec Argv Test ===");

    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec argv_test with specific args.
        let path = b"/bin/argv_test\0";
        let arg0 = b"argv_test\0".as_ptr();
        let arg1 = b"hello\0".as_ptr();
        let arg2 = b"world\0".as_ptr();
        let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe {
            execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
        }

        // If we get here, exec failed.
        println!("exec failed");
        std::process::exit(1);
    } else if pid > 0 {
        // Parent: wait for child.
        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0); }

        if wifexited(status) && wexitstatus(status) == 0 {
            println!("EXEC_ARGV_TEST_PASSED");
            std::process::exit(0);
        } else {
            println!("EXEC_ARGV_TEST_FAILED");
            std::process::exit(1);
        }
    } else {
        println!("fork failed");
        std::process::exit(1);
    }
}
