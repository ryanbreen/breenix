//! SIGCHLD delivery test (std version)
//!
//! Tests that SIGCHLD is delivered to parent when child exits:
//! 1. Parent registers SIGCHLD handler
//! 2. Parent forks child
//! 3. Child exits
//! 4. Parent's SIGCHLD handler is called
//!
//! POSIX requires that the parent receive SIGCHLD when a child terminates.

use std::sync::atomic::{AtomicBool, Ordering};

/// Static flag to track if SIGCHLD handler was called
static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);

// Signal constants
const SIGCHLD: i32 = 17;
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
    fn sched_yield() -> i32;
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

/// SIGCHLD handler
extern "C" fn sigchld_handler(_sig: i32) {
    SIGCHLD_RECEIVED.store(true, Ordering::SeqCst);
    println!("  SIGCHLD_HANDLER: Child termination signal received!");
}

fn main() {
    unsafe {
        println!("=== SIGCHLD Delivery Test ===");

        // Step 1: Register SIGCHLD handler
        println!("\nStep 1: Register SIGCHLD handler in parent");
        let action = KernelSigaction {
            handler: sigchld_handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };

        let ret = raw_sigaction(SIGCHLD, &action, std::ptr::null_mut());
        if ret < 0 {
            println!("  FAIL: sigaction returned error {}", -ret);
            println!("SIGCHLD_TEST_FAILED");
            std::process::exit(1);
        }
        println!("  PASS: sigaction registered SIGCHLD handler");

        // Step 2: Fork child
        println!("\nStep 2: Forking child process...");
        let fork_result = fork();

        if fork_result < 0 {
            println!("  FAIL: fork() failed with error {}", fork_result);
            println!("SIGCHLD_TEST_FAILED");
            std::process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started, exiting immediately with code 42");
            std::process::exit(42);
        } else {
            // ========== PARENT PROCESS ==========
            println!("[PARENT] Forked child PID: {}", fork_result);

            // Step 3: Wait for child with waitpid
            println!("\nStep 3: Waiting for child with waitpid (blocking)...");
            let mut status: i32 = 0;
            let wait_result = waitpid(fork_result, &mut status, 0);

            if wait_result != fork_result {
                println!("[PARENT] FAIL: waitpid returned wrong PID: {}", wait_result);
                println!("SIGCHLD_TEST_FAILED");
                std::process::exit(1);
            }

            // Verify child exit code
            if wifexited(status) {
                let exit_code = wexitstatus(status);
                println!("[PARENT] Child exited with code: {}", exit_code);
            }

            // Step 4: Check if SIGCHLD was already delivered
            println!("\nStep 4: Verify SIGCHLD was delivered");

            if !SIGCHLD_RECEIVED.load(Ordering::SeqCst) {
                println!("  SIGCHLD not yet received, yielding once...");
                sched_yield();
            }

            // Final check
            if SIGCHLD_RECEIVED.load(Ordering::SeqCst) {
                println!("  PASS: SIGCHLD handler was called!");
                println!("\n=== All SIGCHLD delivery tests passed! ===");
                println!("SIGCHLD_TEST_PASSED");
                std::process::exit(0);
            } else {
                println!("  FAIL: SIGCHLD handler was NOT called");
                println!("  (Note: This may indicate the kernel doesn't send SIGCHLD on child exit)");
                println!("SIGCHLD_TEST_FAILED");
                std::process::exit(1);
            }
        }
    }
}
