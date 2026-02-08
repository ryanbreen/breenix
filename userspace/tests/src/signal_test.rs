//! Signal test program (std version)
//!
//! Tests basic signal functionality:
//! 1. kill() syscall to send SIGTERM to child
//! 2. Default signal handler (terminate)

extern "C" {
    fn fork() -> i32;
    fn getpid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn sched_yield() -> i32;
}

const SIGTERM: i32 = 15;

fn main() {
    println!("=== Signal Test ===");

    let my_pid = unsafe { getpid() };
    println!("My PID: {}", my_pid);

    // Test 1: Check if process exists using kill(pid, 0)
    println!("\nTest 1: Check process exists with kill(pid, 0)");
    let ret = unsafe { kill(my_pid, 0) };
    if ret == 0 {
        println!("  PASS: Process exists");
    } else {
        println!("  FAIL: kill returned error {}", ret);
    }

    // Test 2: Fork and send SIGTERM to child
    println!("\nTest 2: Fork and send SIGTERM to child");
    let fork_result = unsafe { fork() };

    if fork_result == 0 {
        // Child process - loop forever, waiting for signal
        println!("  CHILD: Started, waiting for signal...");
        let child_pid = unsafe { getpid() };
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
                unsafe { sched_yield(); }
            }
        }
    } else {
        // Parent process
        let child_pid = fork_result;
        println!("  PARENT: Forked child with PID {}", child_pid);

        // Small delay to let child start
        println!("  PARENT: Waiting for child to start...");
        for i in 0..5 {
            println!("  PARENT: yield {}", i);
            unsafe { sched_yield(); }
        }
        println!("  PARENT: Done waiting, about to send signal");

        // Send SIGTERM to child
        println!("  PARENT: Sending SIGTERM to child");
        let ret = unsafe { kill(child_pid, SIGTERM) };
        if ret == 0 {
            println!("  PARENT: kill() syscall succeeded");
        } else {
            println!("  PARENT: kill() failed with error {}", ret);
            std::process::exit(1);
        }

        // Wait for child to actually terminate using waitpid
        println!("  PARENT: Waiting for child to terminate...");
        let mut status: i32 = 0;
        let result = unsafe { waitpid(child_pid, &mut status, 0) };

        if result == child_pid {
            // Check if child was terminated by signal
            // WIFSIGNALED: (status & 0x7f) != 0
            // WTERMSIG: status & 0x7f
            let termsig = status & 0x7f;
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
            println!("  PARENT: waitpid returned unexpected value: {}", result);
            std::process::exit(4);
        }

        println!("  PARENT: Test complete, exiting");
        std::process::exit(0);
    }
}
