//! Ctrl-C signal handling test (std version)
//!
//! This test validates that:
//! 1. A child process can be forked
//! 2. SIGINT can be sent to the child via kill()
//! 3. The child is terminated by the signal
//! 4. waitpid() correctly reports WIFSIGNALED and WTERMSIG == SIGINT
//!
//! This tests the core signal delivery mechanism that would be triggered
//! by Ctrl-C from the keyboard (the TTY->SIGINT path is tested separately
//! in TTY unit tests).

use libbreenix::signal::SIGINT;
use libbreenix::kill;
use libbreenix::process::{self, ForkResult, getpid, yield_now,
    wifexited, wexitstatus, wifsignaled, wtermsig, wifstopped, wstopsig};

fn main() {
    println!("=== Ctrl-C Signal Test ===");

    let my_pid = getpid().unwrap().raw() as i32;
    println!("Parent PID: {}", my_pid);

    // Fork a child process
    println!("Forking child process...");
    match process::fork() {
        Err(e) => {
            println!("  PARENT: fork() failed with error {:?}", e);
            std::process::exit(8);
        }
        Ok(ForkResult::Child) => {
            // Child process - loop forever, waiting for signal
            println!("  CHILD: Started, waiting for SIGINT...");
            let child_pid = getpid().unwrap().raw() as i32;
            println!("  CHILD: My PID is {}", child_pid);

            // Busy loop - should be killed by SIGINT from parent
            let mut counter = 0u64;
            loop {
                counter = counter.wrapping_add(1);
                if counter % 10_000_000 == 0 {
                    println!("  CHILD: Still alive...");
                }
                // Yield to let parent run
                if counter % 100_000 == 0 {
                    let _ = yield_now();
                }
            }
        }
        Ok(ForkResult::Parent(child_pid_obj)) => {
            // Parent process
            let child_pid = child_pid_obj.raw() as i32;
            println!("  PARENT: Forked child with PID {}", child_pid);

            // Small delay to let child start
            println!("  PARENT: Waiting for child to start...");
            for i in 0..5 {
                println!("  PARENT: yield {}", i);
                let _ = yield_now();
            }
            println!("  PARENT: Done waiting, about to send SIGINT");

            // Send SIGINT to child (simulating Ctrl-C)
            println!("  PARENT: Sending SIGINT (Ctrl-C) to child");
            if kill(child_pid, SIGINT).is_ok() {
                println!("  PARENT: kill(SIGINT) syscall succeeded");
            } else {
                println!("  PARENT: kill(SIGINT) failed");
                std::process::exit(1);
            }

            // Wait for child to actually terminate using waitpid
            println!("  PARENT: Waiting for child to terminate...");
            let mut status: i32 = 0;
            match process::waitpid(child_pid, &mut status, 0) {
                Ok(pid) if pid.raw() as i32 == child_pid => {
                    println!("  PARENT: waitpid returned, status = {}", status);

                    // Check if child was terminated by signal using POSIX macros
                    if wifsignaled(status) {
                        let termsig = wtermsig(status);
                        println!("  PARENT: Child terminated by signal {}", termsig);

                        if termsig == SIGINT {
                            println!("  PARENT: Child correctly terminated by SIGINT!");
                            println!("CTRL_C_TEST_PASSED");
                            std::process::exit(0);
                        } else {
                            println!("  PARENT: FAIL - Child terminated by wrong signal");
                            println!("  Expected SIGINT (2), got: {}", termsig);
                            std::process::exit(2);
                        }
                    } else if wifexited(status) {
                        // Child exited normally (not by signal)
                        let exit_code = wexitstatus(status);
                        println!("  PARENT: FAIL - Child exited normally, not by signal");
                        println!("  Exit code: {}", exit_code);
                        std::process::exit(3);
                    } else if wifstopped(status) {
                        let stopsig = wstopsig(status);
                        println!("  PARENT: FAIL - Child was stopped, not terminated");
                        println!("  Stop signal: {}", stopsig);
                        std::process::exit(4);
                    } else {
                        println!("  PARENT: FAIL - Unknown wait status");
                        std::process::exit(5);
                    }
                }
                Err(e) => {
                    println!("  PARENT: waitpid failed with error {:?}", e);
                    std::process::exit(6);
                }
                Ok(pid) => {
                    println!("  PARENT: waitpid returned unexpected PID {}", pid.raw());
                    std::process::exit(7);
                }
            }
        }
    }
}
