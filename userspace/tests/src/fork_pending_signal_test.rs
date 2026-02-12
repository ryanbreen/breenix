//! Fork pending signal non-inheritance test (std version)
//!
//! POSIX requires that pending signals are NOT inherited by the child after fork().

use libbreenix::signal::{SIGUSR1, SIG_BLOCK, sigmask};
use libbreenix::{kill, sigprocmask, sigpending};
use libbreenix::process::{self, ForkResult, getpid, wifexited, wexitstatus};

fn main() {
    println!("=== Fork Pending Signal Test ===");

    // Block SIGUSR1 so it becomes pending when sent
    let mask: u64 = sigmask(SIGUSR1);
    let mut old_mask: u64 = 0;
    if sigprocmask(SIG_BLOCK, Some(&mask), Some(&mut old_mask)).is_err() {
        println!("sigprocmask failed");
        std::process::exit(1);
    }

    // Send SIGUSR1 to self - it will be pending since it's blocked
    let my_pid = getpid().unwrap().raw() as i32;
    if kill(my_pid, SIGUSR1).is_err() {
        println!("kill failed");
        std::process::exit(1);
    }

    // Verify SIGUSR1 is pending in parent
    let mut pending: u64 = 0;
    if sigpending(&mut pending).is_err() {
        println!("sigpending failed");
        std::process::exit(1);
    }

    if (pending & sigmask(SIGUSR1)) == 0 {
        println!("Parent: SIGUSR1 not pending - test setup failed");
        std::process::exit(1);
    }
    println!("Parent: SIGUSR1 is pending (expected)");

    match process::fork() {
        Err(_) => {
            println!("fork failed");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            let mut child_pending: u64 = 0;
            if sigpending(&mut child_pending).is_err() {
                println!("Child: sigpending failed");
                std::process::exit(1);
            }

            if (child_pending & sigmask(SIGUSR1)) != 0 {
                println!("Child: SIGUSR1 is pending - FAIL (should not inherit pending signals)");
                println!("FORK_PENDING_SIGNAL_TEST_FAILED");
                std::process::exit(1);
            }

            println!("Child: No pending signals (correct POSIX behavior)");
            println!("FORK_PENDING_SIGNAL_TEST_PASSED");
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            let child_pid_raw = child_pid.raw() as i32;
            let mut status: i32 = 0;
            let waited = process::waitpid(child_pid_raw, &mut status, 0);
            match waited {
                Ok(pid) if pid.raw() as i32 == child_pid_raw => {
                    if wifexited(status) {
                        std::process::exit(wexitstatus(status));
                    }
                    println!("Child did not exit normally");
                    std::process::exit(1);
                }
                _ => {
                    println!("waitpid failed");
                    std::process::exit(1);
                }
            }
        }
    }
}
