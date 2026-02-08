//! Copy-on-Write Cleanup Test (std version)
//!
//! This test verifies that when forked children exit, the shared CoW frame
//! reference counts are properly decremented. If cleanup is broken, the
//! parent would see corrupted memory or crash.
//!
//! Test pattern:
//! 1. Parent allocates heap memory via sbrk and writes initial values
//! 2. Fork 3 children in sequence
//! 3. Each child writes to the heap (triggers CoW copy), then exits
//! 4. Parent waits for all children
//! 5. Parent writes to the same heap locations (should work without crash)
//! 6. Verify parent can still read/write its memory correctly
//!
//! Test markers:
//! - COW_CLEANUP_TEST_PASSED: Cleanup working correctly
//! - COW_CLEANUP_TEST_FAILED: Cleanup broken

use std::ptr;

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn sbrk(incr: isize) -> *mut u8;
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Number of children to fork
const NUM_CHILDREN: usize = 3;

/// Size of heap allocation (multiple of 8)
const HEAP_SIZE: usize = 64;

fn main() {
    println!("=== CoW Cleanup Test ===");
    println!("Tests that forked children properly release CoW frames on exit\n");

    // Step 1: Allocate heap memory
    println!("Step 1: Allocating heap memory via sbrk");
    let heap_ptr = unsafe { sbrk(HEAP_SIZE as isize) as *mut u64 };

    if heap_ptr.is_null() || (heap_ptr as usize) == usize::MAX {
        println!("  FAIL: sbrk failed");
        println!("COW_CLEANUP_TEST_FAILED");
        std::process::exit(1);
    }

    println!("  Heap allocated at: {:#018X}", heap_ptr as u64);

    // Write initial values
    let num_slots = HEAP_SIZE / 8;
    for i in 0..num_slots {
        unsafe {
            let p = heap_ptr.add(i);
            ptr::write_volatile(p, 0xDEADBEEF00000000u64 + i as u64);
        }
    }
    println!("  Initial values written");

    // Step 2: Fork children, each writes to heap and exits
    println!("\nStep 2: Forking children (each will write to heap)");

    for child_num in 0..NUM_CHILDREN {
        println!("  Forking child {}...", child_num + 1);

        let fork_result = unsafe { fork() };

        if fork_result < 0 {
            println!("  FAIL: fork() failed");
            println!("COW_CLEANUP_TEST_FAILED");
            std::process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            println!("    [CHILD {}] Writing to heap (triggers CoW copy)", child_num + 1);

            // Write child-specific values to heap (triggers CoW)
            for i in 0..num_slots {
                unsafe {
                    let p = heap_ptr.add(i);
                    let child_value = 0xCAFE000000000000u64
                        + ((child_num as u64) << 32)
                        + i as u64;
                    ptr::write_volatile(p, child_value);
                }
            }

            // Verify write
            let test_val = unsafe { ptr::read_volatile(heap_ptr) };
            if (test_val >> 48) != 0xCAFE {
                println!("    [CHILD] Write verification failed");
                std::process::exit(1);
            }

            println!("    [CHILD {}] Exiting", child_num + 1);
            std::process::exit(0);
        } else {
            // ========== PARENT PROCESS ==========
            // Wait for this child before forking next
            let mut status: i32 = 0;
            let wait_result = unsafe { waitpid(fork_result, &mut status, 0) };

            if wait_result != fork_result {
                println!("  FAIL: waitpid returned wrong PID");
                println!("COW_CLEANUP_TEST_FAILED");
                std::process::exit(1);
            }

            if !wifexited(status) || wexitstatus(status) != 0 {
                println!("  FAIL: Child exited abnormally");
                println!("COW_CLEANUP_TEST_FAILED");
                std::process::exit(1);
            }

            println!("    Child {} completed successfully", child_num + 1);
        }
    }

    // Step 3: Verify parent's heap memory is still intact
    println!("\nStep 3: Verifying parent's heap memory");

    let mut parent_ok = true;
    for i in 0..num_slots {
        let expected = 0xDEADBEEF00000000u64 + i as u64;
        let actual = unsafe { ptr::read_volatile(heap_ptr.add(i)) };

        if actual != expected {
            println!("  FAIL: Memory corrupted at slot {}", i);
            println!("    Expected: {:#018X}", expected);
            println!("    Actual:   {:#018X}", actual);
            parent_ok = false;
        }
    }

    if !parent_ok {
        println!("COW_CLEANUP_TEST_FAILED");
        std::process::exit(1);
    }

    println!("  Parent's memory is intact");

    // Step 4: Parent writes to heap (should work without crash)
    println!("\nStep 4: Parent writing to heap (tests cleanup worked)");

    for i in 0..num_slots {
        unsafe {
            let p = heap_ptr.add(i);
            ptr::write_volatile(p, 0xF1FA100000000000u64 + i as u64);
        }
    }

    // Verify writes
    for i in 0..num_slots {
        let expected = 0xF1FA100000000000u64 + i as u64;
        let actual = unsafe { ptr::read_volatile(heap_ptr.add(i)) };

        if actual != expected {
            println!("  FAIL: Write verification failed at slot {}", i);
            println!("COW_CLEANUP_TEST_FAILED");
            std::process::exit(1);
        }
    }

    println!("  Parent writes succeeded");

    // All tests passed
    println!("\n=== CoW Cleanup Test PASSED ===");
    println!("COW_CLEANUP_TEST_PASSED");
    std::process::exit(0);
}
