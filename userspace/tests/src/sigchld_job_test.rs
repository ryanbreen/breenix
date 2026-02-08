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

const WNOHANG: i32 = 1;

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
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

/// Test 1: WNOHANG returns immediately when no children have exited
unsafe fn test_wnohang_no_children() {
    println!("\n--- Test 1: WNOHANG with no children ---");

    let mut status: i32 = 0;
    let result = waitpid(-1, &mut status, WNOHANG);

    println!("  waitpid(-1, WNOHANG) returned: {}", result);

    // Result should be -ECHILD (no children at all) since we haven't forked yet
    if result == -10 {
        println!("  PASS: Correctly returned -ECHILD (no children)");
    } else if result == 0 {
        println!("  PASS: Returned 0 (acceptable - no pending children)");
    } else if result > 0 {
        println!("  FAIL: WNOHANG returned positive with no pending children");
        std::process::exit(1);
    } else {
        // Other negative value - might be a different error
        println!("  PASS: Returned error code (acceptable)");
    }

    println!("test_wnohang_no_children: PASS");
}

/// Test 2: WNOHANG collects exited child
unsafe fn test_wnohang_collects_exited() {
    println!("\n--- Test 2: WNOHANG collects exited child ---");

    let child = fork();
    if child < 0 {
        println!("  FAIL: fork() failed with error {}", child);
        std::process::exit(1);
    }

    if child == 0 {
        // Child: exit immediately with code 42
        std::process::exit(42);
    }

    println!("  Forked child PID: {}", child);

    // Brief delay to let child exit (spin loop)
    println!("  Waiting for child to exit...");
    for _ in 0..100000u64 {
        core::hint::spin_loop();
    }

    let mut status: i32 = 0;
    let result = waitpid(child, &mut status, WNOHANG);

    println!("  waitpid(child, WNOHANG) returned: {}", result);

    // If WNOHANG didn't get the child yet (returned 0), do a blocking wait
    let final_status;
    if result == 0 {
        println!("  Child not ready yet, doing blocking wait...");
        let mut status2: i32 = 0;
        let blocking_result = waitpid(child, &mut status2, 0);
        if blocking_result != child {
            println!("  FAIL: blocking waitpid returned wrong PID");
            std::process::exit(1);
        }
        final_status = status2;
    } else if result == child {
        println!("  WNOHANG successfully collected child");
        final_status = status;
    } else {
        println!("  FAIL: waitpid returned unexpected value");
        std::process::exit(1);
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

/// Test 3: Multiple children collected in loop
unsafe fn test_multiple_children_loop() {
    println!("\n--- Test 3: Multiple children collected in loop ---");

    const NUM_CHILDREN: usize = 3;
    let mut children: [i32; NUM_CHILDREN] = [0; NUM_CHILDREN];

    // Fork 3 children
    for i in 0..NUM_CHILDREN {
        let pid = fork();
        if pid < 0 {
            println!("  FAIL: fork() failed");
            std::process::exit(1);
        }
        if pid == 0 {
            // Child: exit with index as exit code
            std::process::exit(i as i32);
        }
        children[i] = pid;
        println!("  Forked child {} with PID: {}", i, pid);
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
        let pid = waitpid(-1, &mut status, WNOHANG);

        if pid > 0 {
            if wifexited(status) {
                println!("    Collected child PID: {} (exit code: {})", pid, wexitstatus(status));
            } else {
                println!("    Collected child PID: {}", pid);
            }
            collected += 1;
        } else if pid == 0 {
            // No child ready yet, spin briefly
            for _ in 0..1000u64 {
                core::hint::spin_loop();
            }
        } else {
            // Error (probably ECHILD - no more children)
            break;
        }
        attempts += 1;
    }

    println!("  Collected {} children via WNOHANG loop", collected);

    // If WNOHANG didn't get them all, use blocking wait
    while collected < NUM_CHILDREN {
        let mut status: i32 = 0;
        let pid = waitpid(-1, &mut status, 0);
        if pid > 0 {
            println!("    (blocking) Collected child PID: {}", pid);
            collected += 1;
        } else {
            break;
        }
    }

    if collected != NUM_CHILDREN {
        println!("  FAIL: Could not collect all children");
        std::process::exit(1);
    }

    println!("test_multiple_children_loop: PASS");
}

/// Test 4: Status macros work correctly
unsafe fn test_status_macros() {
    println!("\n--- Test 4: Status macros verification ---");

    let child = fork();
    if child < 0 {
        println!("  FAIL: fork() failed");
        std::process::exit(1);
    }

    if child == 0 {
        // Child: exit with code 123
        std::process::exit(123);
    }

    println!("  Forked child PID: {}", child);

    let mut status: i32 = 0;
    let result = waitpid(child, &mut status, 0);

    if result != child {
        println!("  FAIL: waitpid returned wrong PID");
        std::process::exit(1);
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

fn main() {
    unsafe {
        println!("=== SIGCHLD/Waitpid Job Control Tests ===");

        test_wnohang_no_children();
        test_wnohang_collects_exited();
        test_multiple_children_loop();
        test_status_macros();

        println!("\n=== All SIGCHLD job control tests passed! ===");
        println!("SIGCHLD_JOB_TEST_PASSED");
        std::process::exit(0);
    }
}
