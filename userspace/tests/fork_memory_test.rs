//! Fork memory isolation test program
//!
//! Tests that fork() correctly implements copy-on-write (CoW) semantics,
//! ensuring that parent and child have isolated memory spaces.
//!
//! This test verifies:
//! 1. Stack isolation - child inherits but is isolated from parent's stack
//! 2. Heap isolation (sbrk) - child has separate heap memory
//! 3. Global/static data isolation - child has copy of parent's globals
//!
//! These tests were added to prevent regression of a bug where fork was
//! copying 707 pages instead of ~20, indicating incorrect CoW behavior.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::memory;
use libbreenix::process;
use libbreenix::types::fd;

/// Global variable for memory isolation test
static mut GLOBAL_VALUE: u64 = 0xDEADBEEF;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to hex string and print it
unsafe fn print_hex(prefix: &str, num: u64) {
    io::print(prefix);

    let hex_chars = b"0123456789ABCDEF";
    let mut hex_buf = [0u8; 18]; // "0x" + 16 hex chars
    hex_buf[0] = b'0';
    hex_buf[1] = b'x';

    for i in 0..16 {
        let nibble = ((num >> (60 - i * 4)) & 0xF) as usize;
        hex_buf[2 + i] = hex_chars[nibble];
    }

    io::write(fd::STDOUT, &hex_buf);
    io::print("\n");
}

/// Convert number to decimal string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    io::print(prefix);

    let mut n = num;
    let mut i = 0;

    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &BUFFER[..i]);
    io::print("\n");
}

/// Print a signed number
unsafe fn print_signed_number(prefix: &str, num: i64) {
    io::print(prefix);

    if num < 0 {
        io::print("-");
        print_number("", (-num) as u64);
    } else {
        print_number("", num as u64);
    }
}

/// Test 1: Stack memory isolation
///
/// Parent writes 0xDEADBEEF to a stack variable, forks, then modifies
/// it to 0xCAFEBABE. Child should still see the original value.
unsafe fn test_stack_isolation() -> bool {
    io::print("\n=== Test 1: Stack Memory Isolation ===\n");

    // Stack variable with known initial value
    let mut stack_value: u64 = 0xDEADBEEF;
    print_hex("Parent: Initial stack value: ", stack_value);

    let fork_result = process::fork();

    if fork_result < 0 {
        print_signed_number("fork() failed with error: ", fork_result);
        return false;
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        // Small delay to ensure parent has time to modify its value
        for _ in 0..100000 {
            core::ptr::read_volatile(&0u8);
        }

        // Child should see the ORIGINAL value (0xDEADBEEF)
        print_hex("Child: Reading stack value: ", stack_value);

        if stack_value == 0xDEADBEEF {
            io::print("Child: Stack value is ORIGINAL (0xDEADBEEF) - CORRECT!\n");
            io::print("FORK_STACK_ISOLATION_PASSED\n");
            process::exit(0); // Success
        } else if stack_value == 0xCAFEBABE {
            io::print("Child: Stack value is MODIFIED (0xCAFEBABE) - ISOLATION FAILED!\n");
            io::print("FORK_STACK_ISOLATION_FAILED\n");
            process::exit(1); // Failure
        } else {
            print_hex("Child: Stack value is UNEXPECTED: ", stack_value);
            io::print("FORK_STACK_ISOLATION_FAILED\n");
            process::exit(1);
        }
    } else {
        // ========== PARENT PROCESS ==========
        // Immediately modify the stack value
        stack_value = 0xCAFEBABE;
        print_hex("Parent: Modified stack value to: ", stack_value);

        // Wait for child to complete
        let mut status: i32 = 0;
        let result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

        if result != fork_result {
            io::print("Parent: waitpid failed\n");
            return false;
        }

        // Check if child exited successfully (exit code 0)
        if process::wifexited(status) && process::wexitstatus(status) == 0 {
            io::print("Parent: Child verified stack isolation - TEST PASSED\n");
            return true;
        } else {
            print_number("Parent: Child exit code: ", process::wexitstatus(status) as u64);
            io::print("Parent: Stack isolation test FAILED\n");
            return false;
        }
    }
}

/// Test 2: Heap memory isolation (using sbrk)
///
/// Parent allocates heap memory, writes 0xDEADBEEF, forks, then modifies
/// it to 0xCAFEBABE. Child should still see the original value.
unsafe fn test_heap_isolation() -> bool {
    io::print("\n=== Test 2: Heap Memory Isolation (sbrk) ===\n");

    // Allocate 8 bytes on the heap
    let heap_ptr = memory::sbrk(8) as *mut u64;

    if heap_ptr.is_null() {
        io::print("Parent: sbrk failed - cannot allocate heap memory\n");
        return false;
    }

    print_hex("Parent: Allocated heap at address: ", heap_ptr as u64);

    // Write initial value to heap
    *heap_ptr = 0xDEADBEEF;
    print_hex("Parent: Initial heap value: ", *heap_ptr);

    let fork_result = process::fork();

    if fork_result < 0 {
        print_signed_number("fork() failed with error: ", fork_result);
        return false;
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        // Small delay to ensure parent has time to modify its value
        for _ in 0..100000 {
            core::ptr::read_volatile(&0u8);
        }

        // Child should see the ORIGINAL value (0xDEADBEEF)
        let child_value = *heap_ptr;
        print_hex("Child: Reading heap value: ", child_value);

        if child_value == 0xDEADBEEF {
            io::print("Child: Heap value is ORIGINAL (0xDEADBEEF) - CORRECT!\n");
            io::print("FORK_HEAP_ISOLATION_PASSED\n");
            process::exit(0); // Success
        } else if child_value == 0xCAFEBABE {
            io::print("Child: Heap value is MODIFIED (0xCAFEBABE) - ISOLATION FAILED!\n");
            io::print("FORK_HEAP_ISOLATION_FAILED\n");
            process::exit(1); // Failure
        } else {
            print_hex("Child: Heap value is UNEXPECTED: ", child_value);
            io::print("FORK_HEAP_ISOLATION_FAILED\n");
            process::exit(1);
        }
    } else {
        // ========== PARENT PROCESS ==========
        // Immediately modify the heap value
        *heap_ptr = 0xCAFEBABE;
        print_hex("Parent: Modified heap value to: ", *heap_ptr);

        // Wait for child to complete
        let mut status: i32 = 0;
        let result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

        if result != fork_result {
            io::print("Parent: waitpid failed\n");
            return false;
        }

        // Check if child exited successfully (exit code 0)
        if process::wifexited(status) && process::wexitstatus(status) == 0 {
            io::print("Parent: Child verified heap isolation - TEST PASSED\n");
            return true;
        } else {
            print_number("Parent: Child exit code: ", process::wexitstatus(status) as u64);
            io::print("Parent: Heap isolation test FAILED\n");
            return false;
        }
    }
}

/// Test 3: Global/static memory isolation
///
/// Uses a global static variable to verify isolation across fork.
unsafe fn test_global_isolation() -> bool {
    io::print("\n=== Test 3: Global/Static Memory Isolation ===\n");

    // Global variable is already initialized to 0xDEADBEEF
    print_hex("Parent: Initial global value: ", GLOBAL_VALUE);

    let fork_result = process::fork();

    if fork_result < 0 {
        print_signed_number("fork() failed with error: ", fork_result);
        return false;
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        // Small delay to ensure parent has time to modify its value
        for _ in 0..100000 {
            core::ptr::read_volatile(&0u8);
        }

        // Child should see the ORIGINAL value (0xDEADBEEF)
        print_hex("Child: Reading global value: ", GLOBAL_VALUE);

        if GLOBAL_VALUE == 0xDEADBEEF {
            io::print("Child: Global value is ORIGINAL (0xDEADBEEF) - CORRECT!\n");
            io::print("FORK_GLOBAL_ISOLATION_PASSED\n");
            process::exit(0); // Success
        } else if GLOBAL_VALUE == 0xCAFEBABE {
            io::print("Child: Global value is MODIFIED (0xCAFEBABE) - ISOLATION FAILED!\n");
            io::print("FORK_GLOBAL_ISOLATION_FAILED\n");
            process::exit(1); // Failure
        } else {
            print_hex("Child: Global value is UNEXPECTED: ", GLOBAL_VALUE);
            io::print("FORK_GLOBAL_ISOLATION_FAILED\n");
            process::exit(1);
        }
    } else {
        // ========== PARENT PROCESS ==========
        // Immediately modify the global value
        GLOBAL_VALUE = 0xCAFEBABE;
        print_hex("Parent: Modified global value to: ", GLOBAL_VALUE);

        // Wait for child to complete
        let mut status: i32 = 0;
        let result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

        if result != fork_result {
            io::print("Parent: waitpid failed\n");
            return false;
        }

        // Check if child exited successfully (exit code 0)
        if process::wifexited(status) && process::wexitstatus(status) == 0 {
            io::print("Parent: Child verified global isolation - TEST PASSED\n");
            return true;
        } else {
            print_number("Parent: Child exit code: ", process::wexitstatus(status) as u64);
            io::print("Parent: Global isolation test FAILED\n");
            return false;
        }
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Fork Memory Isolation Test Suite ===\n");
        io::print("Verifying copy-on-write (CoW) semantics for fork()\n");
        print_number("Parent PID: ", process::getpid());

        let mut all_passed = true;

        // Run all isolation tests
        if !test_stack_isolation() {
            all_passed = false;
        }

        if !test_heap_isolation() {
            all_passed = false;
        }

        if !test_global_isolation() {
            all_passed = false;
        }

        // Final summary
        io::print("\n=== Fork Memory Isolation Test Summary ===\n");
        if all_passed {
            io::print("All memory isolation tests PASSED!\n");
            io::print("FORK_MEMORY_ISOLATION_PASSED\n");
            process::exit(0);
        } else {
            io::print("Some memory isolation tests FAILED!\n");
            io::print("FORK_MEMORY_ISOLATION_FAILED\n");
            process::exit(1);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in fork_memory_test!\n");
    process::exit(255);
}
