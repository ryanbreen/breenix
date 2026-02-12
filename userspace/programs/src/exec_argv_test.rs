//! Exec argv test program (std version)
//!
//! Tests that fork+exec with argv works correctly.
//! - Parent forks
//! - Child execs argv_test with args: ["argv_test", "hello", "world"]
//! - Parent waits and checks exit status (0 = pass)

use libbreenix::process::{fork, waitpid, execv, wifexited, wexitstatus, ForkResult};

fn main() {
    println!("=== Exec Argv Test ===");

    match fork() {
        Ok(ForkResult::Child) => {
            // Child: exec argv_test with specific args.
            let path = b"/bin/argv_test\0";
            let arg0 = b"argv_test\0".as_ptr();
            let arg1 = b"hello\0".as_ptr();
            let arg2 = b"world\0".as_ptr();
            let argv: [*const u8; 4] = [arg0, arg1, arg2, std::ptr::null()];

            let _ = execv(path, argv.as_ptr());

            // If we get here, exec failed.
            println!("exec failed");
            std::process::exit(1);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent: wait for child.
            let mut status: i32 = 0;
            let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

            if wifexited(status) && wexitstatus(status) == 0 {
                println!("EXEC_ARGV_TEST_PASSED");
                std::process::exit(0);
            } else {
                println!("EXEC_ARGV_TEST_FAILED");
                std::process::exit(1);
            }
        }
        Err(_) => {
            println!("fork failed");
            std::process::exit(1);
        }
    }
}
