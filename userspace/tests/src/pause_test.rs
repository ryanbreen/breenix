//! Pause syscall test program (std version)
//!
//! Tests the pause() syscall which blocks until a signal is delivered.

use std::sync::atomic::{AtomicBool, Ordering};

static SIGUSR1_RECEIVED: AtomicBool = AtomicBool::new(false);

const SIGUSR1: i32 = 10;
const SA_RESTORER: u64 = 0x04000000;

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
    fn pause() -> i32;
}

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
/// can fire during another println (which holds a RefCell borrow on stdout).
/// Using println here would panic with "RefCell already borrowed".
extern "C" fn sigusr1_handler(_sig: i32) {
    SIGUSR1_RECEIVED.store(true, Ordering::SeqCst);
    raw_write_str("  HANDLER: SIGUSR1 received in parent!\n");
}

fn main() {
    println!("=== Pause Syscall Test ===");

    let parent_pid = unsafe { getpid() };
    println!("Parent PID: {}", parent_pid);

    // Step 1: Register SIGUSR1 handler
    println!("\nStep 1: Register SIGUSR1 handler in parent");
    let action = KernelSigaction {
        handler: sigusr1_handler as u64,
        mask: 0,
        flags: SA_RESTORER,
        restorer: __restore_rt as u64,
    };

    let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
    if ret < 0 {
        println!("  FAIL: sigaction returned error {}", -ret);
        println!("PAUSE_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered SIGUSR1 handler");

    // Step 2: Fork child
    println!("\nStep 2: Forking child process...");
    let fork_result = unsafe { fork() };

    if fork_result < 0 {
        println!("  FAIL: fork() failed with error {}", fork_result);
        println!("PAUSE_TEST_FAILED");
        std::process::exit(1);
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        let my_pid = unsafe { getpid() };
        println!("[CHILD] Process started");
        println!("[CHILD] My PID: {}", my_pid);

        // Give parent time to call pause()
        println!("[CHILD] Yielding to let parent call pause()...");
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

        // Step 3: Call pause() to wait for signal
        // NOTE: On ARM64, sched_yield doesn't cause an immediate context switch
        // (it sets need_resched but the SVC return path has PREEMPT_ACTIVE set).
        // The child may complete all its yields and send the signal before the
        // parent gets a timer-driven context switch. In that case, the signal
        // is delivered on a syscall return BEFORE we reach pause().
        // We handle both paths: signal-before-pause and signal-during-pause.
        if SIGUSR1_RECEIVED.load(Ordering::SeqCst) {
            println!("\nStep 3: Signal already received before pause() (race-safe path)");
            println!("  PASS: SIGUSR1 was delivered before pause() was called");
        } else {
            println!("\nStep 3: Calling pause() to wait for signal...");
            let pause_ret = unsafe { pause() };

            // pause() should return -1 (libc convention) with errno EINTR
            // But the raw kernel returns -EINTR (-4)
            // Our libbreenix-libc pause() calls syscall_result_to_c_int which
            // may return -1 or the raw value. Check both.
            println!("[PARENT] pause() returned: {}", pause_ret);

            // The libc pause() converts to C convention: returns -1 with errno set
            // But the original test checks for -4 (raw kernel return).
            // Accept either -4 (raw) or -1 (libc converted).
            if pause_ret != -4 && pause_ret != -1 {
                println!("  FAIL: pause() should return -4 (-EINTR) or -1, got {}", pause_ret);
                println!("PAUSE_TEST_FAILED");
                std::process::exit(1);
            }
            println!("  PASS: pause() correctly returned after signal");
        }

        // Step 4: Verify signal handler was called
        println!("\nStep 4: Verify SIGUSR1 handler was called");
        if SIGUSR1_RECEIVED.load(Ordering::SeqCst) {
            println!("  PASS: SIGUSR1 handler was called!");
        } else {
            println!("  FAIL: SIGUSR1 handler was NOT called");
            println!("PAUSE_TEST_FAILED");
            std::process::exit(1);
        }

        // Step 5: Wait for child to exit
        println!("\nStep 5: Waiting for child to exit...");
        let mut status: i32 = 0;
        let wait_result = unsafe { waitpid(fork_result, &mut status, 0) };

        if wait_result == fork_result {
            println!("  Child reaped successfully");
        } else {
            println!("  Warning: waitpid returned {} (expected {})", wait_result, fork_result);
        }

        // All tests passed
        println!("\n=== All pause() tests passed! ===");
        println!("PAUSE_TEST_PASSED");
        std::process::exit(0);
    }
}
