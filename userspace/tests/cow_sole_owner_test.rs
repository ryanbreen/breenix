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

/// Size of heap allocation
const HEAP_SIZE: usize = 32;

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== CoW Sole Owner Optimization Test ===\n");
        io::print("Tests that parent becomes sole owner when child exits without writing\n\n");

        // Step 0: Read initial CoW stats
        io::print("Step 0: Reading initial CoW statistics\n");
        let initial_stats = match memory::cow_stats() {
            Some(s) => s,
            None => {
                io::print("  FAIL: Could not get initial CoW stats\n");
                io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                process::exit(1);
            }
        };
        io::print("  Initial sole_owner_opt counter: ");
        print_number(initial_stats.sole_owner_opt);
        io::print("\n");

        // Step 1: Allocate heap memory
        io::print("\nStep 1: Allocating heap memory via sbrk\n");
        let heap_ptr = memory::sbrk(HEAP_SIZE) as *mut u64;

        if heap_ptr.is_null() || (heap_ptr as usize) == usize::MAX {
            io::print("  FAIL: sbrk failed\n");
            io::print("COW_SOLE_OWNER_TEST_FAILED\n");
            process::exit(1);
        }

        print_hex("  Heap allocated at: ", heap_ptr as u64);

        // Write initial value (this makes the page present and writable before fork)
        core::ptr::write_volatile(heap_ptr, 0xDEADBEEF12345678);
        io::print("  Initial value written: 0xDEADBEEF12345678\n");

        // Step 2: Fork a child
        io::print("\nStep 2: Forking child (will exit immediately without writing)\n");

        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("COW_SOLE_OWNER_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Started - exiting immediately WITHOUT writing to heap\n");

            // Child does NOT write to heap - just exits
            // This means parent should become sole owner of the frames

            io::print("[CHILD] Exiting with success\n");
            process::exit(0);
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Step 3: Wait for child to complete
            io::print("\nStep 3: Waiting for child to exit\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            if wait_result != fork_result {
                io::print("  FAIL: waitpid returned wrong PID\n");
                io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                process::exit(1);
            }

            if !process::wifexited(status) || process::wexitstatus(status) != 0 {
                io::print("  FAIL: Child exited abnormally\n");
                io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("  Child exited successfully\n");

            // Step 4: Verify initial value is still correct
            io::print("\nStep 4: Verifying heap memory is intact\n");
            let current_value = core::ptr::read_volatile(heap_ptr);
            print_hex("  Current value: ", current_value);

            if current_value != 0xDEADBEEF12345678 {
                io::print("  FAIL: Memory value changed unexpectedly!\n");
                io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("  Memory is intact\n");

            // Step 5: Write to heap - should trigger sole owner optimization
            // Since child exited without writing, parent is sole owner (refcount=1)
            // The kernel should just mark the page writable without copying
            io::print("\nStep 5: Parent writing to heap (sole owner optimization)\n");

            let new_value: u64 = 0xCAFEBABE87654321;
            core::ptr::write_volatile(heap_ptr, new_value);
            print_hex("  Wrote new value: ", new_value);

            // Verify the write
            let verify_value = core::ptr::read_volatile(heap_ptr);
            print_hex("  Read back value: ", verify_value);

            if verify_value != new_value {
                io::print("  FAIL: Write verification failed!\n");
                io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("  Write succeeded\n");

            // Step 6: Read CoW stats after the write and verify sole_owner_opt incremented
            io::print("\nStep 6: Verifying SOLE_OWNER_OPT counter incremented\n");
            let after_stats = match memory::cow_stats() {
                Some(s) => s,
                None => {
                    io::print("  FAIL: Could not get CoW stats after write\n");
                    io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                    process::exit(1);
                }
            };

            io::print("  sole_owner_opt before: ");
            print_number(initial_stats.sole_owner_opt);
            io::print("\n");
            io::print("  sole_owner_opt after:  ");
            print_number(after_stats.sole_owner_opt);
            io::print("\n");

            // CRITICAL: Verify the sole owner optimization counter increased
            // This proves the optimization path was actually taken, not just that the write succeeded
            if after_stats.sole_owner_opt <= initial_stats.sole_owner_opt {
                io::print("  FAIL: SOLE_OWNER_OPT counter did NOT increment!\n");
                io::print("  This means the sole-owner optimization path was NOT taken.\n");
                io::print("  The page may have been copied instead of just made writable.\n");
                io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                process::exit(1);
            }

            let increment = after_stats.sole_owner_opt - initial_stats.sole_owner_opt;
            io::print("  SOLE_OWNER_OPT incremented by: ");
            print_number(increment);
            io::print("\n");
            io::print("  SUCCESS: Sole owner optimization path was taken!\n");

            // Step 7: Write to additional slots to trigger more sole-owner optimizations
            io::print("\nStep 7: Writing to additional heap slots\n");
            let num_slots = HEAP_SIZE / 8;
            for i in 1..num_slots {
                let ptr = heap_ptr.add(i);
                core::ptr::write_volatile(ptr, 0x501E0000000000 + i as u64);
            }

            // Verify all writes
            for i in 1..num_slots {
                let ptr = heap_ptr.add(i);
                let expected = 0x501E0000000000 + i as u64;
                let actual = core::ptr::read_volatile(ptr);

                if actual != expected {
                    io::print("  FAIL: Write verification failed at slot ");
                    print_number(i as u64);
                    io::print("\n");
                    io::print("COW_SOLE_OWNER_TEST_FAILED\n");
                    process::exit(1);
                }
            }

            io::print("  All writes succeeded\n");

            // All tests passed - including verification of the optimization path
            io::print("\n=== CoW Sole Owner Optimization Test PASSED ===\n");
            io::print("Verified: SOLE_OWNER_OPT counter incremented, proving optimization worked\n");
            io::print("COW_SOLE_OWNER_TEST_PASSED\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in cow_sole_owner_test!\n");
    io::print("COW_SOLE_OWNER_TEST_FAILED\n");
    process::exit(255);
}
