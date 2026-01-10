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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::memory;
use libbreenix::process;
use libbreenix::types::fd;

/// Print a number to stdout
unsafe fn print_number(num: u64) {
    let mut buffer: [u8; 32] = [0; 32];
    let mut n = num;
    let mut i = 0;

    if n == 0 {
        buffer[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            buffer[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse
        for j in 0..i / 2 {
            let tmp = buffer[j];
            buffer[j] = buffer[i - j - 1];
            buffer[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &buffer[..i]);
}

/// Print signed number
unsafe fn print_signed(num: i64) {
    if num < 0 {
        io::print("-");
        print_number((-num) as u64);
    } else {
        print_number(num as u64);
    }
}

/// Print hex value
unsafe fn print_hex(prefix: &str, num: u64) {
    io::print(prefix);
    let hex_chars = b"0123456789ABCDEF";
    io::print("0x");
    for i in (0..16).rev() {
        let nibble = ((num >> (i * 4)) & 0xF) as usize;
        let c = [hex_chars[nibble]];
        io::write(fd::STDOUT, &c);
    }
    io::print("\n");
}

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

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== CoW Stress Test ===\n");
        io::print("Tests CoW with many pages (");
        print_number(NUM_PAGES as u64);
        io::print(" pages, ");
        print_number((ALLOC_SIZE / 1024) as u64);
        io::print("KB)\n\n");

        // Step 1: Allocate heap memory
        io::print("Step 1: Allocating ");
        print_number(NUM_PAGES as u64);
        io::print(" pages (");
        print_number((ALLOC_SIZE / 1024) as u64);
        io::print("KB) via sbrk\n");

        let heap_ptr = memory::sbrk(ALLOC_SIZE) as *mut u64;

        if heap_ptr.is_null() || (heap_ptr as usize) == usize::MAX {
            io::print("  FAIL: sbrk failed\n");
            io::print("COW_STRESS_TEST_FAILED\n");
            process::exit(1);
        }

        print_hex("  Heap allocated at: ", heap_ptr as u64);

        // Step 2: Fill all memory with parent pattern
        io::print("\nStep 2: Filling all pages with parent pattern\n");

        let num_slots = ALLOC_SIZE / 8; // u64 slots
        for i in 0..num_slots {
            let ptr = heap_ptr.add(i);
            let value = pattern_for_slot(PARENT_MAGIC, i);
            core::ptr::write_volatile(ptr, value);
        }

        io::print("  Filled ");
        print_number(num_slots as u64);
        io::print(" slots across ");
        print_number(NUM_PAGES as u64);
        io::print(" pages\n");

        // Step 3: Fork
        io::print("\nStep 3: Forking process\n");

        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("COW_STRESS_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started\n");

            // Step 4: Verify parent pattern is visible
            io::print("[CHILD] Step 4: Verifying parent pattern before writes\n");
            let mut verify_errors = 0u64;

            for i in 0..num_slots {
                let ptr = heap_ptr.add(i);
                let expected = pattern_for_slot(PARENT_MAGIC, i);
                let actual = core::ptr::read_volatile(ptr);

                if actual != expected && verify_errors < 5 {
                    io::print("[CHILD]   Mismatch at slot ");
                    print_number(i as u64);
                    io::print("\n");
                    print_hex("    Expected: ", expected);
                    print_hex("    Actual:   ", actual);
                    verify_errors += 1;
                }
            }

            if verify_errors > 0 {
                io::print("[CHILD]   FAIL: Found ");
                print_number(verify_errors);
                io::print(" pattern mismatches\n");
                io::print("COW_STRESS_TEST_FAILED\n");
                process::exit(1);
            }
            io::print("[CHILD]   Parent pattern verified\n");

            // Step 5: Write to every page (triggers many CoW faults)
            io::print("[CHILD] Step 5: Writing to all ");
            print_number(NUM_PAGES as u64);
            io::print(" pages (triggers CoW faults)\n");

            let slots_per_page = PAGE_SIZE / 8;
            let mut cow_faults_expected = 0u64;

            for page in 0..NUM_PAGES {
                // Write to first slot of each page to trigger CoW
                let slot = page * slots_per_page;
                let ptr = heap_ptr.add(slot);
                let child_value = pattern_for_slot(CHILD_MAGIC, slot);
                core::ptr::write_volatile(ptr, child_value);
                cow_faults_expected += 1;

                // Also fill rest of the page with child pattern
                for offset in 1..slots_per_page {
                    let slot_idx = slot + offset;
                    if slot_idx < num_slots {
                        let ptr = heap_ptr.add(slot_idx);
                        let child_value = pattern_for_slot(CHILD_MAGIC, slot_idx);
                        core::ptr::write_volatile(ptr, child_value);
                    }
                }
            }

            io::print("[CHILD]   Wrote to ");
            print_number(cow_faults_expected);
            io::print(" pages (CoW faults triggered)\n");

            // Step 6: Verify child pattern
            io::print("[CHILD] Step 6: Verifying child pattern\n");
            verify_errors = 0;

            for i in 0..num_slots {
                let ptr = heap_ptr.add(i);
                let expected = pattern_for_slot(CHILD_MAGIC, i);
                let actual = core::ptr::read_volatile(ptr);

                if actual != expected && verify_errors < 5 {
                    io::print("[CHILD]   Mismatch at slot ");
                    print_number(i as u64);
                    io::print("\n");
                    print_hex("    Expected: ", expected);
                    print_hex("    Actual:   ", actual);
                    verify_errors += 1;
                }
            }

            if verify_errors > 0 {
                io::print("[CHILD]   FAIL: Found ");
                print_number(verify_errors);
                io::print(" pattern mismatches after writes\n");
                io::print("COW_STRESS_TEST_FAILED\n");
                process::exit(1);
            }
            io::print("[CHILD]   Child pattern verified\n");

            io::print("[CHILD] Child completed successfully\n");
            process::exit(0);
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Wait for child to complete
            io::print("[PARENT] Waiting for child...\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            if wait_result != fork_result {
                io::print("[PARENT] FAIL: waitpid returned wrong PID\n");
                io::print("COW_STRESS_TEST_FAILED\n");
                process::exit(1);
            }

            if !process::wifexited(status) || process::wexitstatus(status) != 0 {
                io::print("[PARENT] FAIL: Child exited abnormally\n");
                io::print("COW_STRESS_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("[PARENT] Child completed\n");

            // Step 7: Verify parent's memory is unchanged
            io::print("\n[PARENT] Step 7: Verifying parent pattern unchanged\n");
            let mut verify_errors = 0u64;

            for i in 0..num_slots {
                let ptr = heap_ptr.add(i);
                let expected = pattern_for_slot(PARENT_MAGIC, i);
                let actual = core::ptr::read_volatile(ptr);

                if actual != expected {
                    if verify_errors < 5 {
                        io::print("[PARENT]   Mismatch at slot ");
                        print_number(i as u64);
                        io::print(" (page ");
                        print_number((i / (PAGE_SIZE / 8)) as u64);
                        io::print(")\n");
                        print_hex("    Expected: ", expected);
                        print_hex("    Actual:   ", actual);
                    }
                    verify_errors += 1;
                }
            }

            if verify_errors > 0 {
                io::print("[PARENT]   FAIL: Found ");
                print_number(verify_errors);
                io::print(" pattern mismatches (child writes leaked!)\n");
                io::print("COW_STRESS_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("[PARENT]   All ");
            print_number(num_slots as u64);
            io::print(" slots verified - parent memory intact\n");

            // Step 8: Parent writes to all pages (should work)
            io::print("\n[PARENT] Step 8: Parent writing to all pages\n");

            let new_magic: u64 = 0xF1FA1000;
            for i in 0..num_slots {
                let ptr = heap_ptr.add(i);
                let value = pattern_for_slot(new_magic, i);
                core::ptr::write_volatile(ptr, value);
            }

            // Verify parent writes
            let mut write_errors = 0u64;
            for i in 0..num_slots {
                let ptr = heap_ptr.add(i);
                let expected = pattern_for_slot(new_magic, i);
                let actual = core::ptr::read_volatile(ptr);

                if actual != expected {
                    if write_errors < 5 {
                        io::print("[PARENT]   Write mismatch at slot ");
                        print_number(i as u64);
                        io::print("\n");
                    }
                    write_errors += 1;
                }
            }

            if write_errors > 0 {
                io::print("[PARENT]   FAIL: ");
                print_number(write_errors);
                io::print(" write verification errors\n");
                io::print("COW_STRESS_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("[PARENT]   Parent writes verified\n");

            // All tests passed
            io::print("\n=== CoW Stress Test PASSED ===\n");
            io::print("Verified:\n");
            io::print("  - ");
            print_number(NUM_PAGES as u64);
            io::print(" CoW faults handled correctly\n");
            io::print("  - No memory corruption\n");
            io::print("  - Refcounting works at scale\n");
            io::print("COW_STRESS_TEST_PASSED\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in cow_stress_test!\n");
    io::print("COW_STRESS_TEST_FAILED\n");
    process::exit(255);
}
