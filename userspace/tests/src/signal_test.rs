//! Signal test program (std version)
//!
//! Tests basic signal functionality:
//! 1. kill() syscall to send SIGTERM to child
//! 2. Default signal handler (terminate)

use libbreenix::process::{fork, getpid, waitpid, yield_now, wtermsig, ForkResult};
use libbreenix::signal::{kill, SIGTERM};

fn main() {
    println!("=== Signal Test ===");

    let my_pid = getpid().unwrap().raw() as i32;
    println!("My PID: {}", my_pid);

    // Test 1: Check if process exists using kill(pid, 0)
    println!("\nTest 1: Check process exists with kill(pid, 0)");
    let ret = kill(my_pid, 0);
    if ret.is_ok() {
        println!("  PASS: Process exists");
    } else {
        println!("  FAIL: kill returned error");
    }

    // Test 2: Fork and send SIGTERM to child
    println!("\nTest 2: Fork and send SIGTERM to child");

    match fork() {
        Ok(ForkResult::Child) => {
            // Child process - loop forever, waiting for signal
            println!("  CHILD: Started, waiting for signal...");
            let child_pid = getpid().unwrap().raw() as i32;
            println!("  CHILD: My PID is {}", child_pid);

            // Busy loop - should be killed by parent
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
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent process
            let child_pid_i32 = child_pid.raw() as i32;
            println!("  PARENT: Forked child with PID {}", child_pid_i32);

            // Small delay to let child start
            println!("  PARENT: Waiting for child to start...");
            for i in 0..5 {
                println!("  PARENT: yield {}", i);
                let _ = yield_now();
            }
            println!("  PARENT: Done waiting, about to send signal");

            // Send SIGTERM to child
            println!("  PARENT: Sending SIGTERM to child");
            let ret = kill(child_pid_i32, SIGTERM);
            if ret.is_ok() {
                println!("  PARENT: kill() syscall succeeded");
            } else {
                println!("  PARENT: kill() failed");
                std::process::exit(1);
            }

            // Wait for child to actually terminate using waitpid
            println!("  PARENT: Waiting for child to terminate...");
            let mut status: i32 = 0;
            let result = waitpid(child_pid_i32, &mut status, 0).unwrap();

            if result.raw() as i32 == child_pid_i32 {
                // Check if child was terminated by signal
                // WTERMSIG: status & 0x7f
                let termsig = wtermsig(status);
                if termsig == SIGTERM {
                    println!("  PARENT: Child terminated by SIGTERM!");
                    println!("SIGNAL_KILL_TEST_PASSED");
                } else if termsig != 0 {
                    println!("  PARENT: Child terminated by wrong signal: {}", termsig);
                    std::process::exit(2);
                } else {
                    // Child exited normally (WIFEXITED)
                    let exit_code = (status >> 8) & 0xff;
                    println!("  PARENT: Child exited normally (not by signal), exit code: {}", exit_code);
                    std::process::exit(3);
                }
            } else {
                println!("  PARENT: waitpid returned unexpected value: {}", result.raw() as i32);
                std::process::exit(4);
            }

            println!("  PARENT: Test complete, exiting");
            std::process::exit(0);
        }
        Err(_) => {
            println!("fork failed");
            std::process::exit(1);
        }
    }
}
