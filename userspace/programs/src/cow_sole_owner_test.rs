//! Copy-on-Write Sole Owner Optimization Test
//!
//! This test verifies that the sole-owner optimization works correctly.
//! When a forked child exits without writing to shared pages, the parent
//! becomes the sole owner of those frames. When the parent writes, the
//! kernel should detect refcount==1 and just mark the page writable
//! (no copy needed).
//!
//! Test pattern:
//! 1. Read initial SOLE_OWNER_OPT counter
//! 2. Parent allocates heap memory and writes initial value
//! 3. Fork a child
//! 4. Child immediately exits WITHOUT writing to the heap
//! 5. Parent waits for child
//! 6. Parent writes to the heap - should trigger sole-owner optimization
//! 7. Read SOLE_OWNER_OPT counter again and verify it incremented
//! 8. Verify the write succeeded
//!
//! Test markers:
//! - COW_SOLE_OWNER_TEST_PASSED: Sole owner optimization working AND counter incremented
//! - COW_SOLE_OWNER_TEST_FAILED: Test failed

use std::ptr;

use libbreenix::memory::{sbrk, cow_stats};
use libbreenix::process::{fork, waitpid, wifexited, wexitstatus, ForkResult};

/// Size of heap allocation
const HEAP_SIZE: usize = 32;

fn main() {
    println!("=== CoW Sole Owner Optimization Test ===");
    println!("Tests that parent becomes sole owner when child exits without writing\n");

    // Step 0: Read initial CoW stats
    println!("Step 0: Reading initial CoW statistics");
    let initial_stats = match cow_stats() {
        Ok(s) => s,
        Err(_) => {
            println!("  FAIL: Could not get initial CoW stats");
            println!("COW_SOLE_OWNER_TEST_FAILED");
            std::process::exit(1);
        }
    };
    println!("  Initial sole_owner_opt counter: {}", initial_stats.sole_owner_opt);

    // Step 1: Allocate heap memory
    println!("\nStep 1: Allocating heap memory via sbrk");
    let heap_ptr = sbrk(HEAP_SIZE) as *mut u64;

    if heap_ptr.is_null() || (heap_ptr as usize) == usize::MAX {
        println!("  FAIL: sbrk failed");
        println!("COW_SOLE_OWNER_TEST_FAILED");
        std::process::exit(1);
    }

    println!("  Heap allocated at: {:#018X}", heap_ptr as u64);

    // Write initial value (this makes the page present and writable before fork)
    unsafe { ptr::write_volatile(heap_ptr, 0xDEADBEEF12345678u64) };
    println!("  Initial value written: 0xDEADBEEF12345678");

    // Step 2: Fork a child
    println!("\nStep 2: Forking child (will exit immediately without writing)");

    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Started - exiting immediately WITHOUT writing to heap");

            // Child does NOT write to heap - just exits
            // This means parent should become sole owner of the frames

            println!("[CHILD] Exiting with success");
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            println!("[PARENT] Forked child PID: {}", child_pid.raw());

            // Step 3: Wait for child to complete
            println!("\nStep 3: Waiting for child to exit");
            let mut status: i32 = 0;
            let wait_result = waitpid(child_pid.raw() as i32, &mut status, 0);

            match wait_result {
                Ok(pid) if pid.raw() as i32 == child_pid.raw() as i32 => {}
                _ => {
                    println!("  FAIL: waitpid returned wrong PID");
                    println!("COW_SOLE_OWNER_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            if !wifexited(status) || wexitstatus(status) != 0 {
                println!("  FAIL: Child exited abnormally");
                println!("COW_SOLE_OWNER_TEST_FAILED");
                std::process::exit(1);
            }

            println!("  Child exited successfully");

            // Step 4: Verify initial value is still correct
            println!("\nStep 4: Verifying heap memory is intact");
            let current_value = unsafe { ptr::read_volatile(heap_ptr) };
            println!("  Current value: {:#018X}", current_value);

            if current_value != 0xDEADBEEF12345678u64 {
                println!("  FAIL: Memory value changed unexpectedly!");
                println!("COW_SOLE_OWNER_TEST_FAILED");
                std::process::exit(1);
            }

            println!("  Memory is intact");

            // Step 5: Write to heap - should trigger sole owner optimization
            // Since child exited without writing, parent is sole owner (refcount=1)
            // The kernel should just mark the page writable without copying
            println!("\nStep 5: Parent writing to heap (sole owner optimization)");

            let new_value: u64 = 0xCAFEBABE87654321;
            unsafe { ptr::write_volatile(heap_ptr, new_value) };
            println!("  Wrote new value: {:#018X}", new_value);

            // Verify the write
            let verify_value = unsafe { ptr::read_volatile(heap_ptr) };
            println!("  Read back value: {:#018X}", verify_value);

            if verify_value != new_value {
                println!("  FAIL: Write verification failed!");
                println!("COW_SOLE_OWNER_TEST_FAILED");
                std::process::exit(1);
            }

            println!("  Write succeeded");

            // Step 6: Read CoW stats after the write and verify sole_owner_opt incremented
            println!("\nStep 6: Verifying SOLE_OWNER_OPT counter incremented");
            let after_stats = match cow_stats() {
                Ok(s) => s,
                Err(_) => {
                    println!("  FAIL: Could not get CoW stats after write");
                    println!("COW_SOLE_OWNER_TEST_FAILED");
                    std::process::exit(1);
                }
            };

            println!("  sole_owner_opt before: {}", initial_stats.sole_owner_opt);
            println!("  sole_owner_opt after:  {}", after_stats.sole_owner_opt);

            // CRITICAL: Verify the sole owner optimization counter increased
            // This proves the optimization path was actually taken, not just that the write succeeded
            if after_stats.sole_owner_opt <= initial_stats.sole_owner_opt {
                println!("  FAIL: SOLE_OWNER_OPT counter did NOT increment!");
                println!("  This means the sole-owner optimization path was NOT taken.");
                println!("  The page may have been copied instead of just made writable.");
                println!("COW_SOLE_OWNER_TEST_FAILED");
                std::process::exit(1);
            }

            let increment = after_stats.sole_owner_opt - initial_stats.sole_owner_opt;
            println!("  SOLE_OWNER_OPT incremented by: {}", increment);
            println!("  SUCCESS: Sole owner optimization path was taken!");

            // Step 7: Write to additional slots to trigger more sole-owner optimizations
            println!("\nStep 7: Writing to additional heap slots");
            let num_slots = HEAP_SIZE / 8;
            for i in 1..num_slots {
                unsafe {
                    let p = heap_ptr.add(i);
                    ptr::write_volatile(p, 0x501E0000000000u64 + i as u64);
                }
            }

            // Verify all writes
            for i in 1..num_slots {
                let expected = 0x501E0000000000u64 + i as u64;
                let actual = unsafe { ptr::read_volatile(heap_ptr.add(i)) };

                if actual != expected {
                    println!("  FAIL: Write verification failed at slot {}", i);
                    println!("COW_SOLE_OWNER_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            println!("  All writes succeeded");

            // All tests passed - including verification of the optimization path
            println!("\n=== CoW Sole Owner Optimization Test PASSED ===");
            println!("Verified: SOLE_OWNER_OPT counter incremented, proving optimization worked");
            println!("COW_SOLE_OWNER_TEST_PASSED");
            std::process::exit(0);
        }
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("COW_SOLE_OWNER_TEST_FAILED");
            std::process::exit(1);
        }
    }
}
