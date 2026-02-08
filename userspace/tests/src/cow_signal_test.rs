//! Copy-on-Write Signal Delivery Test (std version)
//!
//! This test specifically verifies that signal delivery works correctly
//! when the user stack is a CoW-shared page. This was the root cause of
//! a deadlock bug where:
//!
//! 1. Signal delivery acquires PROCESS_MANAGER lock
//! 2. Signal delivery writes to user stack (signal frame + trampoline)
//! 3. User stack is a CoW page (shared with parent after fork)
//! 4. CoW page fault handler needs PROCESS_MANAGER lock
//! 5. DEADLOCK - spinning forever waiting for a lock we already hold
//!
//! The fix uses `try_manager()` and falls back to direct page table
//! manipulation via CR3 when the lock is already held.
//!
//! Test markers:
//! - COW_SIGNAL_TEST_PASSED: All tests passed
//! - COW_SIGNAL_TEST_FAILED: A test failed

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::ptr;

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

/// Static variable that handler will modify (on CoW page)
static HANDLER_MODIFIED_VALUE: AtomicU64 = AtomicU64::new(0);

// Signal constants
const SIGUSR1: i32 = 10;
const SA_RESTORER: u64 = 0x04000000;

// Sigaction struct matching the kernel layout (all u64 fields)
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
    fn getpid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn sched_yield() -> i32;
}

/// Raw syscall for sigaction
#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 13u64,  // SYS_SIGACTION
        in("rdi") sig as u64,
        in("rsi") act as u64,
        in("rdx") oldact as u64,
        in("r10") 8u64,   // sigsetsize
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
        in("x8") 13u64,  // SYS_SIGACTION
        inlateout("x0") sig as u64 => ret,
        in("x1") act as u64,
        in("x2") oldact as u64,
        in("x3") 8u64,   // sigsetsize
        options(nostack),
    );
    ret as i64
}

// Signal restorer trampoline
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov rax, 15",  // SYS_rt_sigreturn
        "int 0x80",
        "ud2",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov x8, 15",  // SYS_rt_sigreturn
        "svc #0",
        "brk #1",
    )
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Signal handler for SIGUSR1
/// This handler writes to stack and static memory - both may be CoW pages
extern "C" fn sigusr1_handler(_sig: i32) {
    // This write happens while signal delivery context is active
    // If CoW handling deadlocks, we'll never reach here
    HANDLER_CALLED.store(true, Ordering::SeqCst);
    HANDLER_MODIFIED_VALUE.store(0xCAFEBABE, Ordering::SeqCst);

    // Write to stack (local variable) - this is a CoW page write
    // during signal handler execution
    let mut stack_var: u64 = 0xDEADBEEF;
    // Prevent optimization
    unsafe { ptr::write_volatile(&mut stack_var, 0x12345678) };

    println!("  HANDLER: Signal received, wrote to stack!");
}

fn main() {
    println!("=== CoW Signal Delivery Test ===");
    println!("Tests signal delivery when user stack is CoW-shared\n");

    // Step 1: Touch the stack to ensure the page is present before fork
    // This ensures the page will be CoW-shared (not demand-paged)
    let mut stack_marker: u64 = 0xDEADBEEF;
    unsafe { ptr::write_volatile(&mut stack_marker, 0xDEADBEEF) };
    println!("Step 1: Touched stack page before fork");

    // Step 2: Fork - child inherits parent's address space with CoW
    println!("Step 2: Forking process...");
    let fork_result = unsafe { fork() };

    if fork_result < 0 {
        println!("  FAIL: fork() failed with error {}", fork_result);
        println!("COW_SIGNAL_TEST_FAILED");
        std::process::exit(1);
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        println!("[CHILD] Process started");

        let my_pid = unsafe { getpid() };
        println!("[CHILD] PID: {}", my_pid);

        // Step 3: Register signal handler
        println!("[CHILD] Step 3: Register SIGUSR1 handler");
        let action = KernelSigaction {
            handler: sigusr1_handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };

        let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
        if ret < 0 {
            println!("[CHILD]   FAIL: sigaction returned error {}", -ret);
            println!("COW_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        }
        println!("[CHILD]   sigaction registered handler");

        // Step 4: Send SIGUSR1 to self
        // This triggers the critical path:
        // - Signal delivery holds PROCESS_MANAGER lock
        // - Signal delivery writes to user stack (signal frame)
        // - User stack is CoW page (shared with parent)
        // - CoW fault must be handled WITHOUT deadlocking
        println!("[CHILD] Step 4: Sending SIGUSR1 to self (triggers CoW on stack)...");
        let ret = unsafe { kill(my_pid, SIGUSR1) };
        if ret != 0 {
            println!("[CHILD]   FAIL: kill() returned error");
            println!("COW_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        }
        println!("[CHILD]   kill() succeeded");

        // Step 5: Yield to allow signal delivery
        println!("[CHILD] Step 5: Yielding for signal delivery...");
        for i in 0..20 {
            unsafe { sched_yield() };
            if HANDLER_CALLED.load(Ordering::SeqCst) {
                println!("[CHILD]   Handler called after {} yields", i + 1);
                break;
            }
        }

        // Step 6: Verify handler was called
        println!("[CHILD] Step 6: Verify handler execution");
        if HANDLER_CALLED.load(Ordering::SeqCst)
            && HANDLER_MODIFIED_VALUE.load(Ordering::SeqCst) == 0xCAFEBABE
        {
            println!("[CHILD]   PASS: Handler executed and modified memory!");
            println!("[CHILD]   CoW fault during signal delivery was handled correctly");
            std::process::exit(0);
        } else if !HANDLER_CALLED.load(Ordering::SeqCst) {
            println!("[CHILD]   FAIL: Handler was NOT called");
            println!("[CHILD]   This could indicate deadlock in CoW fault handling");
            println!("COW_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        } else {
            println!("[CHILD]   FAIL: Handler called but didn't modify value");
            println!("COW_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        }
    } else {
        // ========== PARENT PROCESS ==========
        println!("[PARENT] Forked child PID: {}", fork_result);

        // Wait for child to complete
        println!("[PARENT] Waiting for child...");
        let mut status: i32 = 0;
        let wait_result = unsafe { waitpid(fork_result, &mut status, 0) };

        if wait_result != fork_result {
            println!("[PARENT] FAIL: waitpid returned wrong PID");
            println!("COW_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        }

        // Check if child exited normally with code 0
        if wifexited(status) {
            let exit_code = wexitstatus(status);
            if exit_code == 0 {
                println!("[PARENT] Child exited successfully");
                println!("\n=== CoW Signal Delivery Test PASSED ===");
                println!("COW_SIGNAL_TEST_PASSED");
                std::process::exit(0);
            } else {
                println!("[PARENT] Child exited with non-zero code: {}", exit_code);
                println!("COW_SIGNAL_TEST_FAILED");
                std::process::exit(1);
            }
        } else {
            println!("[PARENT] Child did not exit normally");
            println!("COW_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        }
    }
}
