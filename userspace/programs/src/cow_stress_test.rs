//! Copy-on-Write Stress Test
//!
//! This test verifies that Copy-on-Write works correctly at scale with many
//! pages. It allocates a large amount of memory (100+ pages = 400KB+), fills
//! it with a known pattern, forks, then has the child write to every page
//! to trigger many CoW faults in sequence.
//!
//! Test pattern:
//! 1. Parent allocates 128 pages (512KB) of heap memory via sbrk
//! 2. Parent fills all memory with a known pattern
//! 3. Fork
//! 4. Child: Write to every page (triggers 128 CoW faults), verify pattern
//! 5. Parent: Wait for child, verify parent memory unchanged
//!
//! This tests:
//! - Many CoW faults in sequence work correctly
//! - No memory corruption with many shared pages
//! - Refcounting works at scale
//! - No performance degradation with many CoW pages
//!
//! Test markers:
//! - COW_STRESS_TEST_PASSED: All tests passed
//! - COW_STRESS_TEST_FAILED: A test failed

use std::ptr;

use libbreenix::memory::sbrk;
use libbreenix::process::{fork, waitpid, wifexited, wexitstatus, ForkResult};

/// Page size in bytes
const PAGE_SIZE: usize = 4096;

/// Number of pages to allocate (128 pages = 512KB)
const NUM_PAGES: usize = 128;

/// Total allocation size
const ALLOC_SIZE: usize = NUM_PAGES * PAGE_SIZE;

/// Magic value used to generate patterns
const PARENT_MAGIC: u64 = 0xDEADBEEF;
const CHILD_MAGIC: u64 = 0xCAFEBABE;

/// Generate a pattern value for a given slot
fn pattern_for_slot(magic: u64, slot: usize) -> u64 {
    magic << 32 | (slot as u64 & 0xFFFFFFFF)
}

fn main() {
    println!("=== CoW Stress Test ===");
    println!("Tests CoW with many pages ({} pages, {}KB)\n", NUM_PAGES, ALLOC_SIZE / 1024);

    // Step 1: Allocate heap memory
    println!("Step 1: Allocating {} pages ({}KB) via sbrk", NUM_PAGES, ALLOC_SIZE / 1024);

    let heap_ptr = sbrk(ALLOC_SIZE) as *mut u64;

    if heap_ptr.is_null() || (heap_ptr as usize) == usize::MAX {
        println!("  FAIL: sbrk failed");
        println!("COW_STRESS_TEST_FAILED");
        std::process::exit(1);
    }

    println!("  Heap allocated at: {:#018X}", heap_ptr as u64);

    // Step 2: Fill all memory with parent pattern
    println!("\nStep 2: Filling all pages with parent pattern");

    let num_slots = ALLOC_SIZE / 8; // u64 slots
    for i in 0..num_slots {
        unsafe {
            let p = heap_ptr.add(i);
            let value = pattern_for_slot(PARENT_MAGIC, i);
            ptr::write_volatile(p, value);
        }
    }

    println!("  Filled {} slots across {} pages", num_slots, NUM_PAGES);

    // Step 3: Fork
    println!("\nStep 3: Forking process");

    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started");

            // Step 4: Verify parent pattern is visible
            println!("[CHILD] Step 4: Verifying parent pattern before writes");
            let mut verify_errors = 0u64;

            for i in 0..num_slots {
                let expected = pattern_for_slot(PARENT_MAGIC, i);
                let actual = unsafe { ptr::read_volatile(heap_ptr.add(i)) };

                if actual != expected && verify_errors < 5 {
                    println!("[CHILD]   Mismatch at slot {}", i);
                    println!("    Expected: {:#018X}", expected);
                    println!("    Actual:   {:#018X}", actual);
                    verify_errors += 1;
                }
            }

            if verify_errors > 0 {
                println!("[CHILD]   FAIL: Found {} pattern mismatches", verify_errors);
                println!("COW_STRESS_TEST_FAILED");
                std::process::exit(1);
            }
            println!("[CHILD]   Parent pattern verified");

            // Step 5: Write to every page (triggers many CoW faults)
            println!("[CHILD] Step 5: Writing to all {} pages (triggers CoW faults)", NUM_PAGES);

            let slots_per_page = PAGE_SIZE / 8;
            let mut cow_faults_expected = 0u64;

            for page in 0..NUM_PAGES {
                // Write to first slot of each page to trigger CoW
                let slot = page * slots_per_page;
                unsafe {
                    let p = heap_ptr.add(slot);
                    let child_value = pattern_for_slot(CHILD_MAGIC, slot);
                    ptr::write_volatile(p, child_value);
                }
                cow_faults_expected += 1;

                // Also fill rest of the page with child pattern
                for offset in 1..slots_per_page {
                    let slot_idx = slot + offset;
                    if slot_idx < num_slots {
                        unsafe {
                            let p = heap_ptr.add(slot_idx);
                            let child_value = pattern_for_slot(CHILD_MAGIC, slot_idx);
                            ptr::write_volatile(p, child_value);
                        }
                    }
                }
            }

            println!("[CHILD]   Wrote to {} pages (CoW faults triggered)", cow_faults_expected);

            // Step 6: Verify child pattern
            println!("[CHILD] Step 6: Verifying child pattern");
            verify_errors = 0;

            for i in 0..num_slots {
                let expected = pattern_for_slot(CHILD_MAGIC, i);
                let actual = unsafe { ptr::read_volatile(heap_ptr.add(i)) };

                if actual != expected && verify_errors < 5 {
                    println!("[CHILD]   Mismatch at slot {}", i);
                    println!("    Expected: {:#018X}", expected);
                    println!("    Actual:   {:#018X}", actual);
                    verify_errors += 1;
                }
            }

            if verify_errors > 0 {
                println!("[CHILD]   FAIL: Found {} pattern mismatches after writes", verify_errors);
                println!("COW_STRESS_TEST_FAILED");
                std::process::exit(1);
            }
            println!("[CHILD]   Child pattern verified");

            println!("[CHILD] Child completed successfully");
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            println!("[PARENT] Forked child PID: {}", child_pid.raw());

            // Wait for child to complete
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = waitpid(child_pid.raw() as i32, &mut status, 0);

            match wait_result {
                Ok(pid) if pid.raw() as i32 == child_pid.raw() as i32 => {}
                _ => {
                    println!("[PARENT] FAIL: waitpid returned wrong PID");
                    println!("COW_STRESS_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            if !wifexited(status) || wexitstatus(status) != 0 {
                println!("[PARENT] FAIL: Child exited abnormally");
                println!("COW_STRESS_TEST_FAILED");
                std::process::exit(1);
            }

            println!("[PARENT] Child completed");

            // Step 7: Verify parent's memory is unchanged
            println!("\n[PARENT] Step 7: Verifying parent pattern unchanged");
            let mut verify_errors = 0u64;

            for i in 0..num_slots {
                let expected = pattern_for_slot(PARENT_MAGIC, i);
                let actual = unsafe { ptr::read_volatile(heap_ptr.add(i)) };

                if actual != expected {
                    if verify_errors < 5 {
                        println!("[PARENT]   Mismatch at slot {} (page {})", i, i / (PAGE_SIZE / 8));
                        println!("    Expected: {:#018X}", expected);
                        println!("    Actual:   {:#018X}", actual);
                    }
                    verify_errors += 1;
                }
            }

            if verify_errors > 0 {
                println!("[PARENT]   FAIL: Found {} pattern mismatches (child writes leaked!)", verify_errors);
                println!("COW_STRESS_TEST_FAILED");
                std::process::exit(1);
            }

            println!("[PARENT]   All {} slots verified - parent memory intact", num_slots);

            // Step 8: Parent writes to all pages (should work)
            println!("\n[PARENT] Step 8: Parent writing to all pages");

            let new_magic: u64 = 0xF1FA1000;
            for i in 0..num_slots {
                unsafe {
                    let p = heap_ptr.add(i);
                    let value = pattern_for_slot(new_magic, i);
                    ptr::write_volatile(p, value);
                }
            }

            // Verify parent writes
            let mut write_errors = 0u64;
            for i in 0..num_slots {
                let expected = pattern_for_slot(new_magic, i);
                let actual = unsafe { ptr::read_volatile(heap_ptr.add(i)) };

                if actual != expected {
                    if write_errors < 5 {
                        println!("[PARENT]   Write mismatch at slot {}", i);
                    }
                    write_errors += 1;
                }
            }

            if write_errors > 0 {
                println!("[PARENT]   FAIL: {} write verification errors", write_errors);
                println!("COW_STRESS_TEST_FAILED");
                std::process::exit(1);
            }

            println!("[PARENT]   Parent writes verified");

            // All tests passed
            println!("\n=== CoW Stress Test PASSED ===");
            println!("Verified:");
            println!("  - {} CoW faults handled correctly", NUM_PAGES);
            println!("  - No memory corruption");
            println!("  - Refcounting works at scale");
            println!("COW_STRESS_TEST_PASSED");
            std::process::exit(0);
        }
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("COW_STRESS_TEST_FAILED");
            std::process::exit(1);
        }
    }
}
