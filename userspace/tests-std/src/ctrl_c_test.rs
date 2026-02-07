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

const SIGINT: i32 = 2;

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn sched_yield() -> i32;
}

/// POSIX wait status macros
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn wifsignaled(status: i32) -> bool {
    let sig = status & 0x7f;
    sig != 0 && sig != 0x7f
}

fn wtermsig(status: i32) -> i32 {
    status & 0x7f
}

fn wifstopped(status: i32) -> bool {
    (status & 0xff) == 0x7f
}

fn wstopsig(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn main() {
    unsafe {
        println!("=== Ctrl-C Signal Test ===");

        let my_pid = getpid();
        println!("Parent PID: {}", my_pid);

        // Fork a child process
        println!("Forking child process...");
        let fork_result = fork();

        if fork_result == 0 {
            // Child process - loop forever, waiting for signal
            println!("  CHILD: Started, waiting for SIGINT...");
            let child_pid = getpid();
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
                    sched_yield();
                }
            }
        } else if fork_result > 0 {
            // Parent process
            let child_pid = fork_result;
            println!("  PARENT: Forked child with PID {}", child_pid);

            // Small delay to let child start
            println!("  PARENT: Waiting for child to start...");
            for i in 0..5 {
                println!("  PARENT: yield {}", i);
                sched_yield();
            }
            println!("  PARENT: Done waiting, about to send SIGINT");

            // Send SIGINT to child (simulating Ctrl-C)
            println!("  PARENT: Sending SIGINT (Ctrl-C) to child");
            let ret = kill(child_pid, SIGINT);
            if ret == 0 {
                println!("  PARENT: kill(SIGINT) syscall succeeded");
            } else {
                println!("  PARENT: kill(SIGINT) failed with error {}", -ret);
                std::process::exit(1);
            }

            // Wait for child to actually terminate using waitpid
            println!("  PARENT: Waiting for child to terminate...");
            let mut status: i32 = 0;
            let result = waitpid(child_pid, &mut status, 0);

            if result == child_pid {
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
            } else if result < 0 {
                println!("  PARENT: waitpid failed with error {}", -result);
                std::process::exit(6);
            } else {
                println!("  PARENT: waitpid returned unexpected PID {}", result);
                std::process::exit(7);
            }
        } else {
            // Fork failed
            println!("  PARENT: fork() failed with error {}", -fork_result);
            std::process::exit(8);
        }
    }
}
