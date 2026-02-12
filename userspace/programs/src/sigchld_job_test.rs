//! SIGCHLD/Waitpid Job Control Integration Tests (std version)
//!
//! Tests SIGCHLD handling in the context of job control scenarios:
//! 1. WNOHANG returns immediately when no children have exited
//! 2. WNOHANG collects exited child after delay
//! 3. Multiple children collected in a loop with WNOHANG
//! 4. WIFEXITED/WIFSIGNALED macros work correctly
//!
//! These tests validate the shell's ability to implement non-blocking
//! child process monitoring (check_children, report_done_jobs patterns).

use libbreenix::process::{self, ForkResult, WNOHANG, wifexited, wexitstatus, wifsignaled};

/// Test 1: WNOHANG returns immediately when no children have exited
fn test_wnohang_no_children() {
    println!("\n--- Test 1: WNOHANG with no children ---");

    let mut status: i32 = 0;
    let result = process::waitpid(-1, &mut status, WNOHANG);

    match result {
        Err(_) => {
            // Error (probably ECHILD - no children at all) since we haven't forked yet
            println!("  waitpid(-1, WNOHANG) returned error (ECHILD expected)");
            println!("  PASS: Correctly returned error (no children)");
        }
        Ok(pid) if pid.raw() == 0 => {
            println!("  PASS: Returned 0 (acceptable - no pending children)");
        }
        Ok(pid) => {
            println!("  FAIL: WNOHANG returned positive PID {} with no pending children", pid.raw());
            std::process::exit(1);
        }
    }

    println!("test_wnohang_no_children: PASS");
}

/// Test 2: WNOHANG collects exited child
fn test_wnohang_collects_exited() {
    println!("\n--- Test 2: WNOHANG collects exited child ---");

    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // Child: exit immediately with code 42
            std::process::exit(42);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child = child_pid.raw() as i32;
            println!("  Forked child PID: {}", child);

            // Brief delay to let child exit (spin loop)
            println!("  Waiting for child to exit...");
            for _ in 0..100000u64 {
                core::hint::spin_loop();
            }

            let mut status: i32 = 0;
            let result = process::waitpid(child, &mut status, WNOHANG);

            let final_status;
            match result {
                Ok(pid) if pid.raw() == 0 => {
                    println!("  Child not ready yet, doing blocking wait...");
                    let mut status2: i32 = 0;
                    match process::waitpid(child, &mut status2, 0) {
                        Ok(pid2) if pid2.raw() as i32 == child => {
                            final_status = status2;
                        }
                        _ => {
                            println!("  FAIL: blocking waitpid returned wrong PID");
                            std::process::exit(1);
                        }
                    }
                }
                Ok(pid) if pid.raw() as i32 == child => {
                    println!("  WNOHANG successfully collected child");
                    final_status = status;
                }
                _ => {
                    println!("  FAIL: waitpid returned unexpected value");
                    std::process::exit(1);
                }
            }

            // Verify child exited normally
            if !wifexited(final_status) {
                println!("  FAIL: child should have exited normally");
                std::process::exit(1);
            }

            let code = wexitstatus(final_status);
            println!("  Child exit code: {}", code);

            if code != 42 {
                println!("  FAIL: exit code should be 42");
                std::process::exit(1);
            }

            println!("test_wnohang_collects_exited: PASS");
        }
    }
}

/// Test 3: Multiple children collected in loop
fn test_multiple_children_loop() {
    println!("\n--- Test 3: Multiple children collected in loop ---");

    const NUM_CHILDREN: usize = 3;
    let mut children: [i32; NUM_CHILDREN] = [0; NUM_CHILDREN];

    // Fork 3 children
    for i in 0..NUM_CHILDREN {
        match process::fork() {
            Err(_) => {
                println!("  FAIL: fork() failed");
                std::process::exit(1);
            }
            Ok(ForkResult::Child) => {
                // Child: exit with index as exit code
                std::process::exit(i as i32);
            }
            Ok(ForkResult::Parent(pid)) => {
                children[i] = pid.raw() as i32;
                println!("  Forked child {} with PID: {}", i, children[i]);
            }
        }
    }

    // Brief delay to let children exit
    println!("  Waiting for children to exit...");
    for _ in 0..100000u64 {
        core::hint::spin_loop();
    }

    // Collect all with WNOHANG loop (shell pattern)
    let mut collected: usize = 0;
    let mut attempts = 0;
    println!("  Starting WNOHANG collection loop...");

    while collected < NUM_CHILDREN && attempts < 1000 {
        let mut status: i32 = 0;
        match process::waitpid(-1, &mut status, WNOHANG) {
            Ok(pid) if pid.raw() > 0 => {
                if wifexited(status) {
                    println!("    Collected child PID: {} (exit code: {})", pid.raw(), wexitstatus(status));
                } else {
                    println!("    Collected child PID: {}", pid.raw());
                }
                collected += 1;
            }
            Ok(pid) if pid.raw() == 0 => {
                // No child ready yet, spin briefly
                for _ in 0..1000u64 {
                    core::hint::spin_loop();
                }
            }
            _ => {
                // Error (probably ECHILD - no more children)
                break;
            }
        }
        attempts += 1;
    }

    println!("  Collected {} children via WNOHANG loop", collected);

    // If WNOHANG didn't get them all, use blocking wait
    while collected < NUM_CHILDREN {
        let mut status: i32 = 0;
        match process::waitpid(-1, &mut status, 0) {
            Ok(pid) if pid.raw() > 0 => {
                println!("    (blocking) Collected child PID: {}", pid.raw());
                collected += 1;
            }
            _ => {
                break;
            }
        }
    }

    if collected != NUM_CHILDREN {
        println!("  FAIL: Could not collect all children");
        std::process::exit(1);
    }

    println!("test_multiple_children_loop: PASS");
}

/// Test 4: Status macros work correctly
fn test_status_macros() {
    println!("\n--- Test 4: Status macros verification ---");

    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // Child: exit with code 123
            std::process::exit(123);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child = child_pid.raw() as i32;
            println!("  Forked child PID: {}", child);

            let mut status: i32 = 0;
            match process::waitpid(child, &mut status, 0) {
                Ok(pid) if pid.raw() as i32 == child => {}
                _ => {
                    println!("  FAIL: waitpid returned wrong PID");
                    std::process::exit(1);
                }
            }

            println!("  Raw status value: {} (0x{:08x})", status, status as u32);

            // Test WIFEXITED
            let exited = wifexited(status);
            println!("  wifexited(status) = {}", exited);

            if !exited {
                println!("  FAIL: wifexited should be true");
                std::process::exit(1);
            }

            // Test WIFSIGNALED (should be false for normal exit)
            let signaled = wifsignaled(status);
            println!("  wifsignaled(status) = {}", signaled);

            if signaled {
                println!("  FAIL: wifsignaled should be false for normal exit");
                std::process::exit(1);
            }

            // Test WEXITSTATUS
            let code = wexitstatus(status);
            println!("  wexitstatus(status) = {}", code);

            if code != 123 {
                println!("  FAIL: wexitstatus should return 123");
                std::process::exit(1);
            }

            println!("test_status_macros: PASS");
        }
    }
}

fn main() {
    println!("=== SIGCHLD/Waitpid Job Control Tests ===");

    test_wnohang_no_children();
    test_wnohang_collects_exited();
    test_multiple_children_loop();
    test_status_macros();

    println!("\n=== All SIGCHLD job control tests passed! ===");
    println!("SIGCHLD_JOB_TEST_PASSED");
    std::process::exit(0);
}
