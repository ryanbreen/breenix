//! Copy-on-Write Cleanup Test
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

/// Print signed number (available for debugging, currently unused)
#[allow(dead_code)]
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

/// Number of children to fork
const NUM_CHILDREN: usize = 3;

/// Size of heap allocation (multiple of 8)
const HEAP_SIZE: usize = 64;

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== CoW Cleanup Test ===\n");
        io::print("Tests that forked children properly release CoW frames on exit\n\n");

        // Step 1: Allocate heap memory
        io::print("Step 1: Allocating heap memory via sbrk\n");
        let heap_ptr = memory::sbrk(HEAP_SIZE) as *mut u64;

        if heap_ptr.is_null() || (heap_ptr as usize) == usize::MAX {
            io::print("  FAIL: sbrk failed\n");
            io::print("COW_CLEANUP_TEST_FAILED\n");
            process::exit(1);
        }

        print_hex("  Heap allocated at: ", heap_ptr as u64);

        // Write initial values
        let num_slots = HEAP_SIZE / 8;
        for i in 0..num_slots {
            let ptr = heap_ptr.add(i);
            core::ptr::write_volatile(ptr, 0xDEADBEEF00000000 + i as u64);
        }
        io::print("  Initial values written\n");

        // Step 2: Fork children, each writes to heap and exits
        io::print("\nStep 2: Forking children (each will write to heap)\n");

        for child_num in 0..NUM_CHILDREN {
            io::print("  Forking child ");
            print_number(child_num as u64 + 1);
            io::print("...\n");

            let fork_result = process::fork();

            if fork_result < 0 {
                io::print("  FAIL: fork() failed\n");
                io::print("COW_CLEANUP_TEST_FAILED\n");
                process::exit(1);
            }

            if fork_result == 0 {
                // ========== CHILD PROCESS ==========
                io::print("    [CHILD ");
                print_number(child_num as u64 + 1);
                io::print("] Writing to heap (triggers CoW copy)\n");

                // Write child-specific values to heap (triggers CoW)
                for i in 0..num_slots {
                    let ptr = heap_ptr.add(i);
                    let child_value = 0xCAFE000000000000 + ((child_num as u64) << 32) + i as u64;
                    core::ptr::write_volatile(ptr, child_value);
                }

                // Verify write
                let test_val = core::ptr::read_volatile(heap_ptr);
                if (test_val >> 48) != 0xCAFE {
                    io::print("    [CHILD] Write verification failed\n");
                    process::exit(1);
                }

                io::print("    [CHILD ");
                print_number(child_num as u64 + 1);
                io::print("] Exiting\n");
                process::exit(0);
            } else {
                // ========== PARENT PROCESS ==========
                // Wait for this child before forking next
                let mut status: i32 = 0;
                let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

                if wait_result != fork_result {
                    io::print("  FAIL: waitpid returned wrong PID\n");
                    io::print("COW_CLEANUP_TEST_FAILED\n");
                    process::exit(1);
                }

                if !process::wifexited(status) || process::wexitstatus(status) != 0 {
                    io::print("  FAIL: Child exited abnormally\n");
                    io::print("COW_CLEANUP_TEST_FAILED\n");
                    process::exit(1);
                }

                io::print("    Child ");
                print_number(child_num as u64 + 1);
                io::print(" completed successfully\n");
            }
        }

        // Step 3: Verify parent's heap memory is still intact
        io::print("\nStep 3: Verifying parent's heap memory\n");

        let mut parent_ok = true;
        for i in 0..num_slots {
            let ptr = heap_ptr.add(i);
            let expected = 0xDEADBEEF00000000 + i as u64;
            let actual = core::ptr::read_volatile(ptr);

            if actual != expected {
                io::print("  FAIL: Memory corrupted at slot ");
                print_number(i as u64);
                io::print("\n");
                print_hex("    Expected: ", expected);
                print_hex("    Actual:   ", actual);
                parent_ok = false;
            }
        }

        if !parent_ok {
            io::print("COW_CLEANUP_TEST_FAILED\n");
            process::exit(1);
        }

        io::print("  Parent's memory is intact\n");

        // Step 4: Parent writes to heap (should work without crash)
        io::print("\nStep 4: Parent writing to heap (tests cleanup worked)\n");

        for i in 0..num_slots {
            let ptr = heap_ptr.add(i);
            core::ptr::write_volatile(ptr, 0xF1FA100000000000 + i as u64);
        }

        // Verify writes
        for i in 0..num_slots {
            let ptr = heap_ptr.add(i);
            let expected = 0xF1FA100000000000 + i as u64;
            let actual = core::ptr::read_volatile(ptr);

            if actual != expected {
                io::print("  FAIL: Write verification failed at slot ");
                print_number(i as u64);
                io::print("\n");
                io::print("COW_CLEANUP_TEST_FAILED\n");
                process::exit(1);
            }
        }

        io::print("  Parent writes succeeded\n");

        // All tests passed
        io::print("\n=== CoW Cleanup Test PASSED ===\n");
        io::print("COW_CLEANUP_TEST_PASSED\n");
        process::exit(0);
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in cow_cleanup_test!\n");
    io::print("COW_CLEANUP_TEST_FAILED\n");
    process::exit(255);
}
