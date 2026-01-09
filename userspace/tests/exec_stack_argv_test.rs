//! Test for stack-allocated argument buffers through execv.
//!
//! This test verifies that stack-allocated argument buffers work correctly
//! when passed through execv. This is a regression test for a bug where
//! the compiler could optimize away stack-allocated argument buffers before
//! the syscall read from them.
//!
//! Key difference from exec_argv_test.rs:
//! - exec_argv_test.rs uses STATIC string literals (b"hello\0") in .rodata
//! - This test builds arguments DYNAMICALLY on the stack, like a real shell does
//!
//! The fix uses core::hint::black_box() to prevent the compiler from
//! optimizing away the stack buffers.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

/// Build a null-terminated string on the stack from a source string.
/// Returns a pointer to the buffer that is kept alive by black_box.
///
/// CRITICAL: The black_box() call prevents the compiler from optimizing
/// away the stack buffer before the syscall reads from it.
#[inline(never)]
fn build_stack_string<const N: usize>(src: &[u8], buf: &mut [u8; N]) -> *const u8 {
    // Copy source bytes into the buffer
    let len = src.len().min(N - 1);
    buf[..len].copy_from_slice(&src[..len]);
    buf[len] = 0; // Null terminate

    // CRITICAL: black_box prevents the compiler from optimizing away
    // the buffer before the syscall reads from it. Without this, the
    // compiler may reuse this stack memory since buf is only used
    // to get a pointer.
    core::hint::black_box(buf.as_ptr())
}

/// Dynamically build a string by concatenating bytes.
/// This simulates how a shell might build argument strings at runtime.
#[inline(never)]
fn build_dynamic_arg(prefix: &[u8], suffix: &[u8], buf: &mut [u8; 64]) -> *const u8 {
    let prefix_len = prefix.len().min(32);
    let suffix_len = suffix.len().min(31);

    buf[..prefix_len].copy_from_slice(&prefix[..prefix_len]);
    buf[prefix_len..prefix_len + suffix_len].copy_from_slice(&suffix[..suffix_len]);
    buf[prefix_len + suffix_len] = 0; // Null terminate

    core::hint::black_box(buf.as_ptr())
}

/// Test that stack-allocated argv buffers work through execv.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Exec Stack Argv Test ===");
    println("Testing stack-allocated argument buffers through execv");

    let pid = fork();
    if pid == 0 {
        // Child: build argv with STACK-ALLOCATED buffers
        // This mimics how init_shell.rs try_execute_external() builds argv

        // Program name - must be in .rodata for the kernel to find it
        let program = b"argv_test\0";

        // Build argument strings ON THE STACK (not static)
        // These buffers could be optimized away without black_box()
        let mut arg0_buf = [0u8; 64];
        let mut arg1_buf = [0u8; 64];
        let mut arg2_buf = [0u8; 64];

        // Build argv[0]: program name
        let arg0_ptr = build_stack_string(b"argv_test", &mut arg0_buf);

        // Build argv[1]: dynamically constructed argument
        // Simulates: let user_input = "stack"; format!("{}arg", user_input)
        let arg1_ptr = build_dynamic_arg(b"stack", b"arg", &mut arg1_buf);

        // Build argv[2]: another dynamic argument
        // Simulates building "test123" from runtime data
        let arg2_ptr = build_dynamic_arg(b"test", b"123", &mut arg2_buf);

        // Build the argv array on the stack
        // CRITICAL: Also black_box the argv array itself
        let argv: [*const u8; 4] = [arg0_ptr, arg1_ptr, arg2_ptr, core::ptr::null()];
        let argv_ptr = core::hint::black_box(argv.as_ptr());

        // execv should read from our stack buffers
        let _ = execv(program, argv_ptr);

        // If we get here, exec failed
        println("exec failed");
        exit(1);
    } else if pid > 0 {
        // Parent: wait for child
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) && wexitstatus(status) == 0 {
            println("Child process executed successfully with stack-allocated argv");
            println("EXEC_STACK_ARGV_TEST_PASSED");
            exit(0);
        } else {
            println("Child process failed");
            println("EXEC_STACK_ARGV_TEST_FAILED");
            exit(1);
        }
    } else {
        println("fork failed");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    exit(255);
}
