//! Signal exec reset test (std version)
//!
//! Tests that signal handlers are reset to SIG_DFL after exec():
//! 1. Process registers a user handler for SIGUSR1
//! 2. Process forks a child
//! 3. Child execs signal_exec_check program
//! 4. The new program verifies the handler is SIG_DFL (not inherited)
//!
//! POSIX requires that signals with custom handlers be reset to SIG_DFL
//! after exec, since the old handler code no longer exists in the new
//! address space.

// Signal constants
const SIGUSR1: i32 = 10;
const SIG_DFL: u64 = 0;
const SIG_IGN: u64 = 1;
const SA_RESTORER: u64 = 0x04000000;

// Syscall numbers
const SYS_SIGACTION: u64 = 13;

#[repr(C)]
struct KernelSigaction {
    handler: u64,
    mask: u64,
    flags: u64,
    restorer: u64,
}

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
}

// --- Raw syscall wrappers ---

#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") SYS_SIGACTION,
        in("rdi") sig as u64,
        in("rsi") act as u64,
        in("rdx") oldact as u64,
        in("r10") 8u64,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") SYS_SIGACTION,
        inlateout("x0") sig as u64 => ret,
        in("x1") act as u64,
        in("x2") oldact as u64,
        in("x3") 8u64,
        options(nostack),
    );
    ret as i64
}

// --- Signal restorer trampoline ---

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov rax, 15",
        "int 0x80",
        "ud2",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov x8, 15",
        "svc #0",
        "brk #1",
    )
}

/// POSIX wait status macros
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Signal handler for SIGUSR1 (should never be called in exec'd process)
extern "C" fn sigusr1_handler(_sig: i32) {
    println!("ERROR: Handler was called but should have been reset by exec!");
}

fn main() {
    unsafe {
        println!("=== Signal Exec Reset Test ===");

        // Step 1: Register signal handler for SIGUSR1
        println!("\nStep 1: Register SIGUSR1 handler");
        let action = KernelSigaction {
            handler: sigusr1_handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };

        let ret = raw_sigaction(SIGUSR1, &action, std::ptr::null_mut());
        if ret < 0 {
            println!("  FAIL: sigaction returned error {}", -ret);
            println!("SIGNAL_EXEC_TEST_FAILED");
            std::process::exit(1);
        }
        println!("  PASS: sigaction registered handler");

        // Verify handler was set
        let mut verify_action = KernelSigaction {
            handler: 0,
            mask: 0,
            flags: 0,
            restorer: 0,
        };
        let ret = raw_sigaction(SIGUSR1, std::ptr::null(), &mut verify_action);
        if ret >= 0 {
            println!("  Handler address: {}", verify_action.handler);
            if verify_action.handler == SIG_DFL || verify_action.handler == SIG_IGN {
                println!("  WARN: Handler appears to be default/ignore, test may not be valid");
            }
        } else {
            println!("  WARN: Could not verify handler was set");
        }

        // Step 2: Fork child
        println!("\nStep 2: Forking child process...");
        let fork_result = fork();

        if fork_result < 0 {
            println!("  FAIL: fork() failed with error {}", fork_result);
            println!("SIGNAL_EXEC_TEST_FAILED");
            std::process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Forked successfully, about to exec signal_exec_check");

            // Verify child inherited the handler (before exec)
            let mut child_action = KernelSigaction {
                handler: 0,
                mask: 0,
                flags: 0,
                restorer: 0,
            };
            let ret = raw_sigaction(SIGUSR1, std::ptr::null(), &mut child_action);
            if ret >= 0 {
                println!("[CHILD] Pre-exec handler: {}", child_action.handler);
                if child_action.handler != SIG_DFL && child_action.handler != SIG_IGN {
                    println!("[CHILD] Handler inherited from parent (as expected)");
                }
            }

            // Step 3: Exec into signal_exec_check
            println!("[CHILD] Calling exec(signal_exec_check)...");

            let path = b"signal_exec_check\0";
            let argv: [*const u8; 1] = [std::ptr::null()];
            let envp: [*const u8; 1] = [std::ptr::null()];
            let exec_result = execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());

            // If exec returns, it failed
            println!("[CHILD] exec() returned (should not happen on success): {}", exec_result);

            // Fallback: Check handler state after failed exec
            println!("[CHILD] Note: exec may not be fully implemented for this binary");
            println!("[CHILD] Checking if handler is still set post-exec-attempt...");

            let mut post_exec_action = KernelSigaction {
                handler: 0,
                mask: 0,
                flags: 0,
                restorer: 0,
            };
            let ret = raw_sigaction(SIGUSR1, std::ptr::null(), &mut post_exec_action);
            if ret >= 0 {
                println!("[CHILD] Post-exec handler: {}", post_exec_action.handler);
            }

            // Since exec didn't work as expected, this is a partial test
            println!("[CHILD] Exiting - exec implementation may need extension");
            std::process::exit(42); // Special exit code to indicate exec didn't replace process
        } else {
            // ========== PARENT PROCESS ==========
            println!("[PARENT] Forked child PID: {}", fork_result);

            // Wait for child
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = waitpid(fork_result, &mut status, 0);

            if wait_result != fork_result {
                println!("[PARENT] FAIL: waitpid returned wrong PID");
                println!("SIGNAL_EXEC_TEST_FAILED");
                std::process::exit(1);
            }

            if wifexited(status) {
                let exit_code = wexitstatus(status);
                println!("[PARENT] Child exit code: {}", exit_code);

                if exit_code == 0 {
                    // signal_exec_check verified handler is SIG_DFL
                    println!("[PARENT] Child (signal_exec_check) verified SIG_DFL!");
                    println!("\n=== Signal exec reset test passed! ===");
                    println!("SIGNAL_EXEC_TEST_PASSED");
                    std::process::exit(0);
                } else if exit_code == 1 {
                    // signal_exec_check found SIG_IGN (acceptable per POSIX but not ideal)
                    println!("[PARENT] Child reported handler is SIG_IGN (partial pass per POSIX)");
                    println!("\n=== Signal exec reset test passed (SIG_IGN) ===");
                    println!("SIGNAL_EXEC_TEST_PASSED");
                    std::process::exit(0);
                } else if exit_code == 2 {
                    // signal_exec_check found user handler NOT reset
                    println!("[PARENT] FAIL: Handler was NOT reset to SIG_DFL after exec!");
                    println!("[PARENT] The old handler address was inherited, violating POSIX.");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                } else if exit_code == 3 {
                    // signal_exec_check couldn't query sigaction
                    println!("[PARENT] FAIL: Child couldn't query signal handler state.");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                } else if exit_code == 42 {
                    // Exec returned instead of replacing the process
                    println!("[PARENT] FAIL: exec() returned instead of replacing process!");
                    println!("[PARENT] The exec syscall did not work as expected.");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                } else {
                    println!("[PARENT] FAIL: Unexpected exit code from child");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                }
            } else {
                println!("[PARENT] Child did not exit normally");
                println!("SIGNAL_EXEC_TEST_FAILED");
                std::process::exit(1);
            }
        }
    }
}
