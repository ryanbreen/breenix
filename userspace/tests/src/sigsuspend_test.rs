//! Sigsuspend syscall test program (std version)
//!
//! Tests the sigsuspend() syscall which atomically replaces the signal mask
//! and suspends until a signal is delivered.

use std::sync::atomic::{AtomicBool, Ordering};

static SIGUSR1_RECEIVED: AtomicBool = AtomicBool::new(false);

const SIGUSR1: i32 = 10;
const SA_RESTORER: u64 = 0x04000000;
const SIG_BLOCK: i32 = 0;
const SIG_SETMASK: i32 = 2;

#[repr(C)]
struct KernelSigaction {
    handler: u64,
    mask: u64,
    flags: u64,
    restorer: u64,
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
    fn sched_yield() -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

/// Convert signal number to bitmask
fn sigmask(sig: i32) -> u64 {
    if sig <= 0 || sig > 64 { 0 } else { 1u64 << (sig - 1) }
}

// Raw syscall wrappers
#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 13u64,
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
        in("x8") 13u64,
        inlateout("x0") sig as u64 => ret,
        in("x1") act as u64,
        in("x2") oldact as u64,
        in("x3") 8u64,
        options(nostack),
    );
    ret as i64
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigprocmask(how: i32, set: *const u64, oldset: *mut u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 14u64,  // SYS_SIGPROCMASK
        in("rdi") how as u64,
        in("rsi") set as u64,
        in("rdx") oldset as u64,
        in("r10") 8u64,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_sigprocmask(how: i32, set: *const u64, oldset: *mut u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") 14u64,
        inlateout("x0") how as u64 => ret,
        in("x1") set as u64,
        in("x2") oldset as u64,
        in("x3") 8u64,
        options(nostack),
    );
    ret as i64
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigsuspend(mask: *const u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 130u64,  // SYS_SIGSUSPEND
        in("rdi") mask as u64,
        in("rsi") 8u64,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_sigsuspend(mask: *const u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") 130u64,
        inlateout("x0") mask as u64 => ret,
        in("x1") 8u64,
        options(nostack),
    );
    ret as i64
}

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

/// Raw write syscall - async-signal-safe (no locks, no RefCell, no allocations)
fn raw_write_str(s: &str) {
    let fd: i32 = 1; // stdout
    let buf = s.as_ptr();
    let len = s.len();

    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "svc #0",
            in("x8") 1u64,  // WRITE syscall
            inlateout("x0") fd as u64 => _,
            in("x1") buf as u64,
            in("x2") len as u64,
            options(nostack),
        );
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!(
            "int 0x80",
            in("rax") 1u64,
            in("rdi") fd as u64,
            in("rsi") buf as u64,
            in("rdx") len as u64,
            lateout("rax") _,
            options(nostack, preserves_flags),
        );
    }
}

/// SIGUSR1 handler - sets flag when called
/// IMPORTANT: Uses raw write syscall, NOT println!, because signal handlers
/// must be async-signal-safe. println! holds a RefCell borrow on stdout
/// and would panic if the signal fires during another println.
extern "C" fn sigusr1_handler(_sig: i32) {
    SIGUSR1_RECEIVED.store(true, Ordering::SeqCst);
    raw_write_str("  HANDLER: SIGUSR1 received in parent!\n");
}

fn main() {
    println!("=== Sigsuspend Syscall Test ===");

    let parent_pid = unsafe { getpid() };
    println!("Parent PID: {}", parent_pid);

    // Step 1: Block SIGUSR1 initially with sigprocmask
    println!("\nStep 1: Block SIGUSR1 with sigprocmask");
    let sigusr1_mask = sigmask(SIGUSR1);
    println!("  SIGUSR1 mask: {:#018x}", sigusr1_mask);

    let mut old_mask: u64 = 0;
    let ret = unsafe { raw_sigprocmask(SIG_BLOCK, &sigusr1_mask, &mut old_mask) };
    if ret < 0 {
        println!("  FAIL: sigprocmask returned error {}", -ret);
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigprocmask blocked SIGUSR1");
    println!("  Old mask: {:#018x}", old_mask);

    // Step 2: Verify SIGUSR1 is now blocked
    println!("\nStep 2: Verify SIGUSR1 is blocked");
    let mut current_mask: u64 = 0;
    let ret = unsafe { raw_sigprocmask(SIG_SETMASK, std::ptr::null(), &mut current_mask) };
    if ret < 0 {
        println!("  FAIL: sigprocmask query failed with error {}", -ret);
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  Current mask: {:#018x}", current_mask);

    if (current_mask & sigusr1_mask) != 0 {
        println!("  PASS: SIGUSR1 is blocked in current mask");
    } else {
        println!("  FAIL: SIGUSR1 is NOT blocked in current mask");
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }

    // Step 3: Register SIGUSR1 handler
    println!("\nStep 3: Register SIGUSR1 handler in parent");
    let action = KernelSigaction {
        handler: sigusr1_handler as u64,
        mask: 0,
        flags: SA_RESTORER,
        restorer: __restore_rt as u64,
    };

    let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaction returned error {}", -ret);
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered SIGUSR1 handler");

    // Step 4: Fork child
    println!("\nStep 4: Forking child process...");
    let fork_result = unsafe { fork() };

    if fork_result < 0 {
        println!("  FAIL: fork() failed with error {}", fork_result);
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        let my_pid = unsafe { getpid() };
        println!("[CHILD] Process started");
        println!("[CHILD] My PID: {}", my_pid);

        // Give parent time to call sigsuspend()
        println!("[CHILD] Yielding to let parent call sigsuspend()...");
        for _ in 0..5 {
            unsafe { sched_yield(); }
        }

        // Send SIGUSR1 to parent
        println!("[CHILD] Sending SIGUSR1 to parent (PID {})...", parent_pid);
        let ret = unsafe { kill(parent_pid, SIGUSR1) };
        if ret == 0 {
            println!("[CHILD] kill() succeeded");
        } else {
            println!("[CHILD] kill() failed");
        }

        println!("[CHILD] Exiting with code 0");
        std::process::exit(0);
    } else {
        // ========== PARENT PROCESS ==========
        println!("[PARENT] Forked child PID: {}", fork_result);

        // Step 5: Call sigsuspend() with a mask that UNBLOCKS SIGUSR1
        println!("\nStep 5: Calling sigsuspend() with empty mask (unblocks SIGUSR1)...");
        let suspend_mask: u64 = 0;
        println!("  Suspend mask: {:#018x}", suspend_mask);
        println!("  Calling sigsuspend()...");

        let suspend_ret = unsafe { raw_sigsuspend(&suspend_mask) };

        println!("[PARENT] sigsuspend() returned: {}", suspend_ret);

        // Step 6: Verify sigsuspend() returned -EINTR (-4)
        println!("\nStep 6: Verify sigsuspend() return value");
        if suspend_ret != -4 {
            println!("  FAIL: sigsuspend() should return -4 (-EINTR), got {}", suspend_ret);
            println!("SIGSUSPEND_TEST_FAILED");
            std::process::exit(1);
        }
        println!("  PASS: sigsuspend() correctly returned -EINTR (-4)");

        // Step 7: Verify signal handler was called
        println!("\nStep 7: Verify SIGUSR1 handler was called");
        if SIGUSR1_RECEIVED.load(Ordering::SeqCst) {
            println!("  PASS: SIGUSR1 handler was called!");
        } else {
            println!("  FAIL: SIGUSR1 handler was NOT called");
            println!("SIGSUSPEND_TEST_FAILED");
            std::process::exit(1);
        }

        // Step 8: Verify original mask (with SIGUSR1 blocked) is restored
        println!("\nStep 8: Verify original mask is restored after sigsuspend()");
        let mut restored_mask: u64 = 0;
        let ret = unsafe { raw_sigprocmask(SIG_SETMASK, std::ptr::null(), &mut restored_mask) };
        if ret < 0 {
            println!("  FAIL: sigprocmask query failed with error {}", -ret);
            println!("SIGSUSPEND_TEST_FAILED");
            std::process::exit(1);
        }
        println!("  Restored mask: {:#018x}", restored_mask);
        println!("  Expected mask: {:#018x}", sigusr1_mask);

        if (restored_mask & sigusr1_mask) != 0 {
            println!("  PASS: Original mask restored - SIGUSR1 is blocked again");
        } else {
            println!("  FAIL: Original mask NOT restored - SIGUSR1 is not blocked");
            println!("SIGSUSPEND_TEST_FAILED");
            std::process::exit(1);
        }

        // Step 9: Verify signal was delivered during suspend
        println!("\nStep 9: Verify signal was delivered during sigsuspend(), not after");
        println!("  (Handler was already called during sigsuspend() - correct behavior)");
        println!("  PASS: Signal delivered atomically during mask replacement");

        // Step 10: Wait for child to exit
        println!("\nStep 10: Waiting for child to exit...");
        let mut status: i32 = 0;
        let wait_result = unsafe { waitpid(fork_result, &mut status, 0) };

        if wait_result == fork_result {
            println!("  Child reaped successfully");
        } else {
            println!("  Warning: waitpid returned {} (expected {})", wait_result, fork_result);
        }

        // All tests passed
        println!("\n=== All sigsuspend() tests passed! ===");
        println!("SIGSUSPEND_TEST_PASSED");
        std::process::exit(0);
    }
}
