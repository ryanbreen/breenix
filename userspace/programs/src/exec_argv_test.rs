//! Exec argv test program (std version)
//!
//! Tests that fork+exec with argv works correctly.
//! - Parent forks
//! - Child execs argv_test with args: ["argv_test", "hello", "world"]
//! - Parent waits, then checks the exit status is 0 and that argv_test reported
//!   "hello world" back on the fd-3 pipe

use libbreenix::io::{close, dup2, pipe, read};
use libbreenix::process::{fork, waitpid, execv, wifexited, wexitstatus, ForkResult};
use libbreenix::types::Fd;

fn main() {
    println!("=== Exec Argv Test ===");

    // argv_test writes the arguments it received to fd 3; the parent reads them
    // back through this pipe and compares them with what it passed.
    let (report_read, report_write) = match pipe() {
        Ok(fds) => fds,
        Err(_) => {
            println!("pipe failed");
            std::process::exit(1);
        }
    };
    let report_fd = Fd::from_raw(3);

    match fork() {
        Ok(ForkResult::Child) => {
            if report_read != report_fd {
                let _ = close(report_read);
            }
            if report_write != report_fd {
                if dup2(report_write, report_fd).is_err() {
                    println!("dup2 failed");
                    std::process::exit(1);
                }
                let _ = close(report_write);
            }
            // Child: exec argv_test with specific args.
            let path = b"/usr/local/test/bin/argv_test\0";
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
            let _ = close(report_write);
            let mut status: i32 = 0;
            let waited = waitpid(child_pid.raw() as i32, &mut status, 0);
            let mut report = Vec::new();
            let mut buf = [0u8; 64];
            while let Ok(n) = read(report_read, &mut buf) {
                if n == 0 {
                    break;
                }
                report.extend_from_slice(&buf[..n]);
            }
            let _ = close(report_read);
            println!("Child reported arguments: {:?}", String::from_utf8_lossy(&report));

            let reaped = matches!(waited, Ok(pid) if pid.raw() == child_pid.raw());
            if !reaped {
                println!("waitpid failed: {:?}", waited);
            }
            if reaped
                && report == b"hello world\n"
                && wifexited(status)
                && wexitstatus(status) == 0
            {
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
