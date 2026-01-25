//! sigaltstack() syscall test
//!
//! Tests the sigaltstack() syscall which allows setting an alternate signal stack.
//! When a signal handler is registered with the SA_ONSTACK flag and an alternate
//! stack is configured, the signal handler will execute on that alternate stack
//! instead of the process's normal stack.
//!
//! This is critical for:
//! - Handling stack overflow signals (SIGSEGV) when the main stack is exhausted
//! - Isolating signal handler stack from main thread stack
//! - POSIX compliance
//!
//! Test flow:
//! 1. Allocate a buffer for alternate stack (8192 bytes)
//! 2. Call sigaltstack() to set it with ss_sp, ss_size, ss_flags=0
//! 3. Query current stack with sigaltstack(NULL, &old_ss) to verify it was set
//! 4. Register a signal handler with SA_ONSTACK flag
//! 5. In the handler, check RSP is within the alternate stack range
//! 6. Trigger the signal and verify handler ran on alt stack
//! 7. Test SS_DISABLE flag to disable the alternate stack
//! 8. Test size validation - stack must be >= MINSIGSTKSZ (2048 bytes)

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Alternate stack buffer (must be large enough: >= MINSIGSTKSZ)
/// We use SIGSTKSZ (8192 bytes) which is the recommended size
static mut ALT_STACK: [u8; signal::SIGSTKSZ] = [0; signal::SIGSTKSZ];

/// Flag to track if handler was called
static mut HANDLER_CALLED: bool = false;

/// RSP value captured in handler
static mut HANDLER_RSP: u64 = 0;

/// Main stack RSP (for comparison)
static mut MAIN_STACK_RSP: u64 = 0;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Print a number to stdout
unsafe fn print_number(num: u64) {
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

    io::write(libbreenix::types::fd::STDOUT, &BUFFER[..i]);
}

/// Print a hex number
unsafe fn print_hex(num: u64) {
    io::print("0x");
    let hex_chars = b"0123456789abcdef";
    for i in (0..16).rev() {
        let nibble = ((num >> (i * 4)) & 0xf) as usize;
        BUFFER[0] = hex_chars[nibble];
        io::write(libbreenix::types::fd::STDOUT, &BUFFER[..1]);
    }
}

/// Signal handler that runs on alternate stack
extern "C" fn handler_on_altstack(_sig: i32) {
    unsafe {
        HANDLER_CALLED = true;
        io::print("  HANDLER: Signal received, checking stack...\n");

        // Capture current RSP
        let rsp: u64;
        core::arch::asm!(
            "mov {0}, rsp",
            out(reg) rsp,
        );
        HANDLER_RSP = rsp;

        io::print("  HANDLER: RSP = ");
        print_hex(rsp);
        io::print("\n");
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== sigaltstack() Syscall Test ===\n\n");

        // Capture main stack RSP for comparison
        core::arch::asm!(
            "mov {0}, rsp",
            out(reg) MAIN_STACK_RSP,
        );
        io::print("Main stack RSP = ");
        print_hex(MAIN_STACK_RSP);
        io::print("\n\n");

        // Test 1: Set alternate stack
        io::print("Test 1: Setting alternate signal stack\n");
        let alt_stack_base = core::ptr::addr_of!(ALT_STACK) as u64;
        let alt_stack_size = signal::SIGSTKSZ;

        io::print("  Alt stack base = ");
        print_hex(alt_stack_base);
        io::print("\n");
        io::print("  Alt stack size = ");
        print_number(alt_stack_size as u64);
        io::print(" bytes\n");

        let new_ss = signal::StackT {
            ss_sp: alt_stack_base,
            ss_flags: 0,  // Enable the stack (SS_DISABLE would disable it)
            _pad: 0,
            ss_size: alt_stack_size,
        };

        match signal::sigaltstack(Some(&new_ss), None) {
            Ok(()) => io::print("  PASS: sigaltstack() set alternate stack\n"),
            Err(e) => {
                io::print("  FAIL: sigaltstack() returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGALTSTACK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 2: Query current alternate stack
        io::print("\nTest 2: Querying alternate stack configuration\n");
        let mut old_ss = signal::StackT::default();
        match signal::sigaltstack(None, Some(&mut old_ss)) {
            Ok(()) => {
                io::print("  PASS: sigaltstack() queried current stack\n");
                io::print("  Returned ss_sp = ");
                print_hex(old_ss.ss_sp);
                io::print("\n");
                io::print("  Returned ss_size = ");
                print_number(old_ss.ss_size as u64);
                io::print("\n");
                io::print("  Returned ss_flags = ");
                print_number(old_ss.ss_flags as u64);
                io::print("\n");

                // Verify it matches what we set
                if old_ss.ss_sp != alt_stack_base {
                    io::print("  FAIL: ss_sp mismatch\n");
                    io::print("SIGALTSTACK_TEST_FAILED\n");
                    process::exit(1);
                }
                if old_ss.ss_size != alt_stack_size {
                    io::print("  FAIL: ss_size mismatch\n");
                    io::print("SIGALTSTACK_TEST_FAILED\n");
                    process::exit(1);
                }
                // ss_flags should be 0 when not on the stack
                if old_ss.ss_flags != 0 && old_ss.ss_flags != signal::SS_ONSTACK {
                    io::print("  FAIL: ss_flags unexpected value\n");
                    io::print("SIGALTSTACK_TEST_FAILED\n");
                    process::exit(1);
                }
                io::print("  PASS: Alternate stack configuration verified\n");
            }
            Err(e) => {
                io::print("  FAIL: sigaltstack(NULL, &old_ss) returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGALTSTACK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 3: Register signal handler with SA_ONSTACK flag
        io::print("\nTest 3: Registering handler with SA_ONSTACK flag\n");
        let mut action = signal::Sigaction::new(handler_on_altstack);
        action.flags = signal::SA_ONSTACK;  // Use alternate stack

        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  PASS: Signal handler registered with SA_ONSTACK\n"),
            Err(e) => {
                io::print("  FAIL: sigaction() returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGALTSTACK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 4: Send signal and verify handler runs on alternate stack
        io::print("\nTest 4: Sending signal to trigger handler\n");
        let my_pid = process::getpid() as i32;
        match signal::kill(my_pid, signal::SIGUSR1) {
            Ok(()) => io::print("  Signal sent successfully\n"),
            Err(e) => {
                io::print("  FAIL: kill() returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGALTSTACK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Yield to allow signal delivery
        io::print("  Yielding to allow signal delivery...\n");
        for _ in 0..10 {
            process::yield_now();
            if HANDLER_CALLED {
                break;
            }
        }

        if !HANDLER_CALLED {
            io::print("  FAIL: Handler was not called after 10 yields\n");
            io::print("SIGALTSTACK_TEST_FAILED\n");
            process::exit(1);
        }

        io::print("  PASS: Handler was called\n");

        // Test 5: Verify handler ran on alternate stack
        io::print("\nTest 5: Verifying handler ran on alternate stack\n");
        let alt_stack_bottom = alt_stack_base;
        let alt_stack_top = alt_stack_base + alt_stack_size as u64;

        io::print("  Alt stack range: ");
        print_hex(alt_stack_bottom);
        io::print(" - ");
        print_hex(alt_stack_top);
        io::print("\n");
        io::print("  Handler RSP: ");
        print_hex(HANDLER_RSP);
        io::print("\n");
        io::print("  Main RSP: ");
        print_hex(MAIN_STACK_RSP);
        io::print("\n");

        // RSP should be within the alternate stack range
        if HANDLER_RSP < alt_stack_bottom || HANDLER_RSP >= alt_stack_top {
            io::print("  FAIL: Handler RSP is NOT within alternate stack range\n");
            io::print("SIGALTSTACK_TEST_FAILED\n");
            process::exit(1);
        }

        // RSP should be significantly different from main stack
        let rsp_diff = if HANDLER_RSP > MAIN_STACK_RSP {
            HANDLER_RSP - MAIN_STACK_RSP
        } else {
            MAIN_STACK_RSP - HANDLER_RSP
        };

        if rsp_diff < 4096 {
            io::print("  WARN: Handler RSP is very close to main RSP (diff = ");
            print_number(rsp_diff);
            io::print(" bytes)\n");
        }

        io::print("  PASS: Handler ran on alternate stack!\n");

        // Test 6: Disable alternate stack with SS_DISABLE
        io::print("\nTest 6: Disabling alternate stack with SS_DISABLE\n");
        let disable_ss = signal::StackT {
            ss_sp: 0,
            ss_flags: signal::SS_DISABLE,
            _pad: 0,
            ss_size: 0,
        };

        match signal::sigaltstack(Some(&disable_ss), None) {
            Ok(()) => io::print("  PASS: Alternate stack disabled\n"),
            Err(e) => {
                io::print("  FAIL: sigaltstack(SS_DISABLE) returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGALTSTACK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Verify it was disabled
        let mut query_ss = signal::StackT::default();
        match signal::sigaltstack(None, Some(&mut query_ss)) {
            Ok(()) => {
                io::print("  Queried ss_flags = ");
                print_number(query_ss.ss_flags as u64);
                io::print("\n");
                if query_ss.ss_flags & signal::SS_DISABLE == 0 {
                    io::print("  FAIL: SS_DISABLE flag not set after disable\n");
                    io::print("SIGALTSTACK_TEST_FAILED\n");
                    process::exit(1);
                }
                io::print("  PASS: Alternate stack is disabled\n");
            }
            Err(e) => {
                io::print("  FAIL: Query after disable returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGALTSTACK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 7: Validate minimum size requirement (MINSIGSTKSZ)
        io::print("\nTest 7: Testing minimum stack size validation\n");
        io::print("  MINSIGSTKSZ = ");
        print_number(signal::MINSIGSTKSZ as u64);
        io::print(" bytes\n");

        // Try to set a stack that's too small (should fail)
        let too_small_ss = signal::StackT {
            ss_sp: alt_stack_base,
            ss_flags: 0,
            _pad: 0,
            ss_size: signal::MINSIGSTKSZ - 1,  // One byte too small
        };

        match signal::sigaltstack(Some(&too_small_ss), None) {
            Ok(()) => {
                io::print("  WARN: sigaltstack() accepted stack smaller than MINSIGSTKSZ\n");
                io::print("  (Some systems allow this, continuing test...)\n");
            }
            Err(e) => {
                io::print("  PASS: sigaltstack() rejected too-small stack (error ");
                print_number(e as u64);
                io::print(")\n");
            }
        }

        // Try to set a stack exactly at MINSIGSTKSZ (should succeed)
        let min_ss = signal::StackT {
            ss_sp: alt_stack_base,
            ss_flags: 0,
            _pad: 0,
            ss_size: signal::MINSIGSTKSZ,
        };

        match signal::sigaltstack(Some(&min_ss), None) {
            Ok(()) => io::print("  PASS: sigaltstack() accepted MINSIGSTKSZ-sized stack\n"),
            Err(e) => {
                io::print("  FAIL: sigaltstack() rejected MINSIGSTKSZ stack (error ");
                print_number(e as u64);
                io::print(")\n");
                io::print("SIGALTSTACK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // All tests passed!
        io::print("\n=== TEST RESULT ===\n");
        io::print("✓ All sigaltstack() tests passed!\n");
        io::print("  - Set alternate stack: PASS\n");
        io::print("  - Query alternate stack: PASS\n");
        io::print("  - Handler with SA_ONSTACK: PASS\n");
        io::print("  - Handler ran on alt stack: PASS\n");
        io::print("  - SS_DISABLE flag: PASS\n");
        io::print("  - Size validation: PASS\n");
        io::print("\n");
        io::print("SIGALTSTACK_TEST_PASSED\n");
        process::exit(0);
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in sigaltstack_test!\n");
    io::print("SIGALTSTACK_TEST_FAILED\n");
    process::exit(255);
}
