//! Test for stack-allocated argument buffers through execv (std version)
//!
//! This test verifies that stack-allocated argument buffers work correctly
//! when passed through execv. This is a regression test for a bug where
//! the compiler could optimize away stack-allocated argument buffers before
//! the syscall read from them.
//!
//! Key difference from exec_argv_test:
//! - exec_argv_test uses STATIC string literals (b"hello\0") in .rodata
//! - This test builds arguments DYNAMICALLY on the stack, like a real shell does
//!
//! The fix uses std::hint::black_box() to prevent the compiler from
//! optimizing away the stack buffers.

use libbreenix::process::{fork, waitpid, execv, wifexited, wexitstatus, ForkResult};

/// Build a null-terminated string on the stack from a source string.
/// Returns a pointer to the buffer that is kept alive by black_box.
///
/// CRITICAL: The black_box() call prevents the compiler from optimizing
/// away the stack buffer before the syscall reads from it.
#[inline(never)]
fn build_stack_string<const N: usize>(src: &[u8], buf: &mut [u8; N]) -> *const u8 {
    let len = src.len().min(N - 1);
    buf[..len].copy_from_slice(&src[..len]);
    buf[len] = 0; // Null terminate
    std::hint::black_box(buf.as_ptr())
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

    std::hint::black_box(buf.as_ptr())
}

fn main() {
    println!("=== Exec Stack Argv Test ===");
    println!("Testing stack-allocated argument buffers through execv");

    match fork() {
        Ok(ForkResult::Child) => {
            // Child: build argv with STACK-ALLOCATED buffers
            // This mimics how init_shell.rs try_execute_external() builds argv

            // Program path - must be null-terminated for the kernel
            let program = b"argv_test\0";

            // Build argument strings ON THE STACK (not static)
            let mut arg0_buf = [0u8; 64];
            let mut arg1_buf = [0u8; 64];
            let mut arg2_buf = [0u8; 64];

            // Build argv[0]: program name
            let arg0_ptr = build_stack_string(b"argv_test", &mut arg0_buf);

            // Build argv[1]: dynamically constructed argument
            let arg1_ptr = build_dynamic_arg(b"stack", b"arg", &mut arg1_buf);

            // Build argv[2]: another dynamic argument
            let arg2_ptr = build_dynamic_arg(b"test", b"123", &mut arg2_buf);

            // Build the argv array on the stack
            let argv: [*const u8; 4] = [arg0_ptr, arg1_ptr, arg2_ptr, std::ptr::null()];
            let argv_ptr = std::hint::black_box(argv.as_ptr());

            let _ = execv(program, argv_ptr);

            // If we get here, exec failed
            println!("exec failed");
            std::process::exit(1);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent: wait for child
            let mut status: i32 = 0;
            let _ = waitpid(child_pid.raw() as i32, &mut status, 0);

            if wifexited(status) && wexitstatus(status) == 0 {
                println!("Child process executed successfully with stack-allocated argv");
                println!("EXEC_STACK_ARGV_TEST_PASSED");
                std::process::exit(0);
            } else {
                println!("Child process failed");
                println!("EXEC_STACK_ARGV_TEST_FAILED");
                std::process::exit(1);
            }
        }
        Err(_) => {
            println!("fork failed");
            std::process::exit(1);
        }
    }
}
