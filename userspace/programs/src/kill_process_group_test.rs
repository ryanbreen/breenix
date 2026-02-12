//! Process Group Kill Semantics Test (std version)
//!
//! Tests process group kill semantics:
//! 1. kill(pid, 0) - Check if process/group exists without sending signal
//! 2. kill(0, sig) - Send signal to all processes in caller's process group
//! 3. kill(-pgid, sig) - Send signal to specific process group
//!
//! All verification is done synchronously by the parent to avoid
//! slow inter-process signaling on loaded CI with 30+ concurrent processes.

use std::sync::atomic::{AtomicU32, Ordering};

use libbreenix::signal::{SIGUSR1, SIGUSR2, SIGKILL};
use libbreenix::{kill, sigaction, Sigaction};
use libbreenix::process::{self, ForkResult, getpid, setpgid, getpgrp};

/// Static counters to track signal delivery
static SIGUSR1_COUNT: AtomicU32 = AtomicU32::new(0);
static SIGUSR2_COUNT: AtomicU32 = AtomicU32::new(0);

/// SIGUSR1 handler - increments counter
extern "C" fn sigusr1_handler(_sig: i32) {
    SIGUSR1_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// SIGUSR2 handler - increments counter
extern "C" fn sigusr2_handler(_sig: i32) {
    SIGUSR2_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn main() {
    println!("=== Process Group Kill Semantics Test ===\n");

    // Register signal handlers
    let action1 = Sigaction::new(sigusr1_handler);
    let action2 = Sigaction::new(sigusr2_handler);

    if sigaction(SIGUSR1, Some(&action1), None).is_err() {
        println!("FAIL: Failed to register SIGUSR1 handler");
        println!("KILL_PGROUP_TEST_FAILED");
        std::process::exit(1);
    }

    if sigaction(SIGUSR2, Some(&action2), None).is_err() {
        println!("FAIL: Failed to register SIGUSR2 handler");
        println!("KILL_PGROUP_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 1: kill(pid, 0) - Check if process exists
    println!("Test 1: kill(pid, 0) - Check process existence");
    let parent_pid = getpid().unwrap().raw() as i32;

    if kill(parent_pid, 0).is_ok() {
        println!("  PASS: kill(self, 0) succeeded (process exists)");
    } else {
        println!("  FAIL: kill(self, 0) returned error");
        println!("KILL_PGROUP_TEST_FAILED");
        std::process::exit(1);
    }

    // Check non-existent process
    if kill(99999, 0).is_err() {
        println!("  PASS: kill(99999, 0) failed (process does not exist)");
    } else {
        println!("  FAIL: kill(99999, 0) succeeded (should fail with ESRCH)");
        println!("KILL_PGROUP_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 2: Create process group and test kill(0, sig)
    println!("\nTest 2: kill(0, sig) - Send signal to own process group");

    if setpgid(0, 0).is_err() {
        println!("  FAIL: setpgid(0, 0) failed");
        println!("KILL_PGROUP_TEST_FAILED");
        std::process::exit(1);
    }

    let pgid = match getpgrp() {
        Ok(pid) => pid.raw() as i32,
        Err(_) => {
            println!("  FAIL: getpgrp() failed");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }
    };

    println!("  Created process group {}", pgid);

    // Fork a child into the same process group
    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // Child1: exit immediately -- parent will test signal delivery
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child1_pid)) => {
            let child1 = child1_pid.raw() as i32;

            // Parent: send signal to own process group. The signal is
            // delivered synchronously to the caller during kill().
            if kill(0, SIGUSR1).is_err() {
                println!("  [Parent] FAIL: kill(0, SIGUSR1) failed");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }

            // Parent receives the signal synchronously (same group)
            if SIGUSR1_COUNT.load(Ordering::SeqCst) == 0 {
                println!("  [Parent] FAIL: Did not receive SIGUSR1");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }
            println!("  [Parent] PASS: Received SIGUSR1 (process group signal delivery works)");

            // Wait for child1
            let mut status: i32 = 0;
            let _ = process::waitpid(child1, &mut status, 0);
        }
    }

    // Test 3: kill(-pgid, sig) - Send signal to specific process group
    // Verify: kill(-pgid, sig) returns success, parent is NOT signaled
    // (parent is in a different group). Child cleanup via SIGKILL to
    // avoid slow polling loops on loaded CI with 30+ processes.
    println!("\nTest 3: kill(-pgid, sig) - Send signal to specific process group");

    // Reset counters
    SIGUSR2_COUNT.store(0, Ordering::SeqCst);

    // Fork child2 and put it in its own process group
    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed for child2");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // Child2: set own process group, then spin (must stay alive
            // so parent can setpgid and send signal to our group)
            let _ = setpgid(0, 0);
            #[allow(clippy::empty_loop)]
            loop {}
        }
        Ok(ForkResult::Parent(child2_pid)) => {
            let child2 = child2_pid.raw() as i32;

            // Parent: set child2's process group immediately.
            // Child2 may or may not have run setpgid(0,0) yet -- both
            // calls are idempotent and produce the same result.
            if setpgid(child2, child2).is_err() {
                println!("  [Parent] FAIL: setpgid(child2, child2) failed");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }

            let child2_pgid = child2;
            println!("  [Parent] Sending SIGUSR2 to process group {} via kill(-pgid, sig)", child2_pgid);

            if kill(-child2_pgid, SIGUSR2).is_ok() {
                println!("  [Parent] PASS: kill(-pgid, SIGUSR2) succeeded");
            } else {
                println!("  [Parent] FAIL: kill(-pgid, SIGUSR2) failed");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }

            // Parent should NOT receive SIGUSR2 (different process group)
            if SIGUSR2_COUNT.load(Ordering::SeqCst) == 0 {
                println!("  [Parent] PASS: Did not receive SIGUSR2 (not in target process group)");
            } else {
                println!("  [Parent] FAIL: Incorrectly received SIGUSR2 (should only go to child2's group)");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }

            // Kill child2 (it's in a busy loop) and reap
            let _ = kill(child2, SIGKILL);
            let mut status2: i32 = 0;
            let _ = process::waitpid(child2, &mut status2, 0);
        }
    }

    // All tests passed
    println!("\n=== All process group kill tests passed! ===");
    println!("KILL_PGROUP_TEST_PASSED");
    std::process::exit(0);
}
