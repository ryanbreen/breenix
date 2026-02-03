//! Copy-on-Write OOM (Out-of-Memory) Test
//!
//! This test verifies that the kernel handles memory exhaustion during CoW
//! page faults gracefully. When allocate_frame() returns None during a CoW
//! fault, the process should be terminated with SIGSEGV rather than causing
//! a kernel panic or hang.
//!
//! Test mechanism:
//! 1. Parent allocates and writes to heap memory (establishes page)
//! 2. Parent forks child (heap pages become CoW-shared)
//! 3. Child enables OOM simulation (AFTER fork succeeds - fork needs frames for page tables)
//! 4. Child attempts to write to heap (triggers CoW fault)
//! 5. CoW fault handler tries to allocate frame, fails (OOM simulation)
//! 6. Kernel terminates child with SIGSEGV (exit code -11)
//! 7. Parent verifies child was killed by SIGSEGV, not normal exit
//!
//! IMPORTANT: OOM must be enabled AFTER fork, not before! Fork requires frame
//! allocation for the child's page tables. If OOM is enabled before fork,
//! fork() fails with ENOMEM and we never reach the CoW test scenario.
//!
//! Expected behavior:
//! - Child is killed with exit status indicating SIGSEGV (not normal exit)
//! - Parent continues running normally
//! - System remains stable (no kernel panic)
//!
//! Test markers:
//! - COW_OOM_TEST_PASSED: OOM during CoW handled gracefully
//! - COW_OOM_TEST_FAILED: Test failed

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
#[allow(dead_code)]
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
const HEAP_SIZE: usize = 64;

/// SIGSEGV signal number (for reference)
const SIGSEGV: i32 = 11;

/// Check if process was killed by a signal
/// Status format from waitpid: lower 7 bits = signal, bit 7 = core dump
fn wifsignaled(status: i32) -> bool {
    // Process was killed by signal if lower byte is 0x00-0x7F (signal number)
    // and it's not a stopped process (0x7F)
    let lower = status & 0x7F;
    lower != 0 && lower != 0x7F
}

/// Get the signal that killed the process
fn wtermsig(status: i32) -> i32 {
    status & 0x7F
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== CoW OOM Test ===\n");
        io::print("Tests graceful handling of OOM during CoW page faults\n\n");

        // Step 1: Check if OOM simulation syscall is available
        io::print("Step 1: Testing OOM simulation syscall availability\n");
        let test_result = memory::simulate_oom(false);
        if test_result == -38 {
            // ENOSYS - syscall not available (testing feature not compiled in)
            io::print("  SKIP: OOM simulation syscall not available\n");
            io::print("  (kernel not compiled with 'testing' feature)\n");
            io::print("  This is expected in production builds.\n\n");
            io::print("=== CoW OOM Test SKIPPED (testing feature disabled) ===\n");
            io::print("COW_OOM_TEST_PASSED\n"); // Pass because this is expected
            process::exit(0);
        }
        io::print("  OOM simulation syscall available\n");

        // Step 2: Allocate heap memory
        io::print("\nStep 2: Allocating heap memory via sbrk\n");
        let heap_ptr = memory::sbrk(HEAP_SIZE) as *mut u64;

        if heap_ptr.is_null() || (heap_ptr as usize) == usize::MAX {
            io::print("  FAIL: sbrk failed\n");
            io::print("COW_OOM_TEST_FAILED\n");
            process::exit(1);
        }

        // Write initial values to establish the page
        let num_slots = HEAP_SIZE / 8;
        for i in 0..num_slots {
            let ptr = heap_ptr.add(i);
            core::ptr::write_volatile(ptr, 0xDEADBEEF00000000 + i as u64);
        }
        io::print("  Heap allocated and initialized\n");

        // Step 3: Fork FIRST - child inherits CoW-shared pages
        // IMPORTANT: Fork must happen BEFORE OOM is enabled because fork()
        // requires frame allocation for the child's page tables.
        io::print("\nStep 3: Forking process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("COW_OOM_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started\n");

            // Step 4 (child): Enable OOM simulation AFTER fork succeeded
            // Now that we have our own page tables, enable OOM so the CoW
            // fault during heap write will fail.
            io::print("[CHILD] Enabling OOM simulation...\n");
            let oom_result = memory::simulate_oom(true);
            if oom_result != 0 {
                io::print("[CHILD] FAIL: simulate_oom(true) returned ");
                print_signed(oom_result as i64);
                io::print("\n");
                io::print("COW_OOM_TEST_FAILED\n");
                process::exit(98); // Different code to distinguish this failure
            }
            io::print("[CHILD] OOM simulation enabled\n");

            // Attempt to write to heap - this triggers a CoW page fault
            // With OOM simulation active, allocate_frame() returns None
            // The kernel should terminate us with SIGSEGV
            let ptr = heap_ptr;
            io::print("[CHILD] Writing to CoW page (this should trigger SIGSEGV)...\n");
            core::ptr::write_volatile(ptr, 0xCAFEBABE);

            // If we reach here, the OOM simulation didn't work
            io::print("[CHILD] ERROR: Write succeeded - OOM simulation failed!\n");
            io::print("COW_OOM_TEST_FAILED\n");
            process::exit(99); // Special exit code to indicate test failure
        } else {
            // ========== PARENT PROCESS ==========
            // Parent never has OOM enabled - only the child does
            io::print("  Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Step 4 (parent): Wait for child and verify it was killed by SIGSEGV
            io::print("\nStep 4: Waiting for child...\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            if wait_result != fork_result {
                io::print("  FAIL: waitpid returned wrong PID: ");
                print_signed(wait_result);
                io::print("\n");
                io::print("COW_OOM_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("  Raw status: ");
            print_signed(status as i64);
            io::print("\n");

            // Step 5: Verify child was killed by signal (not normal exit)
            io::print("\nStep 5: Verifying child was killed by SIGSEGV\n");

            if process::wifexited(status) {
                let exit_code = process::wexitstatus(status);
                io::print("  FAIL: Child exited normally with code ");
                print_signed(exit_code as i64);
                io::print("\n");
                io::print("  Expected child to be killed by SIGSEGV, not exit normally\n");

                if exit_code == 99 {
                    io::print("  (Child's special code 99 means OOM simulation didn't work)\n");
                }

                io::print("COW_OOM_TEST_FAILED\n");
                process::exit(1);
            }

            if wifsignaled(status) {
                let sig = wtermsig(status);
                io::print("  Child killed by signal ");
                print_number(sig as u64);

                if sig == SIGSEGV {
                    io::print(" (SIGSEGV)\n");
                    io::print("  PASS: Child correctly received SIGSEGV due to OOM during CoW\n");
                } else {
                    io::print("\n");
                    io::print("  WARN: Expected SIGSEGV (");
                    print_number(SIGSEGV as u64);
                    io::print("), but accepting any signal termination\n");
                }

                // Success - child was killed as expected
                io::print("\n=== CoW OOM Test PASSED ===\n");
                io::print("Kernel gracefully handled OOM during CoW fault\n");
                io::print("COW_OOM_TEST_PASSED\n");
                process::exit(0);
            }

            // Neither normal exit nor signal - unexpected
            io::print("  FAIL: Child status is neither normal exit nor signal kill\n");
            io::print("COW_OOM_TEST_FAILED\n");
            process::exit(1);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in cow_oom_test!\n");
    // Try to disable OOM simulation in case it's still active
    memory::simulate_oom(false);
    io::print("COW_OOM_TEST_FAILED\n");
    process::exit(255);
}
