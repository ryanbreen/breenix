//! Signal handler fork inheritance test (std version)
//!
//! Tests that signal handlers are inherited across fork():
//! 1. Parent registers a signal handler for SIGUSR1
//! 2. Parent forks
//! 3. Child sends SIGUSR1 to itself
//! 4. Child's handler is called (inherited from parent)
//!
//! POSIX requires that signal handlers are inherited by the child process.

use std::sync::atomic::{AtomicBool, Ordering};

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

// Signal constants
const SIGUSR1: i32 = 10;
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
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
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

/// Signal handler for SIGUSR1
extern "C" fn sigusr1_handler(_sig: i32) {
    HANDLER_CALLED.store(true, Ordering::SeqCst);
    println!("  HANDLER: SIGUSR1 received!");
}

fn main() {
    unsafe {
        println!("=== Signal Fork Inheritance Test ===");

        // Step 1: Register signal handler in parent
        println!("\nStep 1: Register SIGUSR1 handler in parent");
        let action = KernelSigaction {
            handler: sigusr1_handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };

        let ret = raw_sigaction(SIGUSR1, &action, std::ptr::null_mut());
        if ret < 0 {
            println!("  FAIL: sigaction returned error {}", -ret);
            println!("SIGNAL_FORK_TEST_FAILED");
            std::process::exit(1);
        }
        println!("  PASS: sigaction registered handler");

        // Step 2: Fork
        println!("\nStep 2: Forking process...");
        let fork_result = fork();

        if fork_result < 0 {
            println!("  FAIL: fork() failed with error {}", fork_result);
            println!("SIGNAL_FORK_TEST_FAILED");
            std::process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started");

            let my_pid = getpid();
            println!("[CHILD] PID: {}", my_pid);

            // Step 3: Send SIGUSR1 to self
            println!("[CHILD] Step 3: Sending SIGUSR1 to self...");
            let ret = kill(my_pid, SIGUSR1);
            if ret == 0 {
                println!("[CHILD]   kill() succeeded");
            } else {
                println!("[CHILD]   FAIL: kill() returned error {}", -ret);
                println!("SIGNAL_FORK_TEST_FAILED");
                std::process::exit(1);
            }

            // Step 4: Yield to allow signal delivery
            println!("[CHILD] Step 4: Yielding for signal delivery...");
            for i in 0..10 {
                sched_yield();
                if HANDLER_CALLED.load(Ordering::SeqCst) {
                    println!("[CHILD]   Handler called after {} yields", i + 1);
                    break;
                }
            }

            // Step 5: Verify handler was called
            println!("[CHILD] Step 5: Verify handler execution");
            if HANDLER_CALLED.load(Ordering::SeqCst) {
                println!("[CHILD]   PASS: Inherited handler was called!");
                println!("[CHILD] Exiting with success");
                std::process::exit(0);
            } else {
                println!("[CHILD]   FAIL: Inherited handler was NOT called");
                println!("SIGNAL_FORK_TEST_FAILED");
                std::process::exit(1);
            }
        } else {
            // ========== PARENT PROCESS ==========
            println!("[PARENT] Forked child PID: {}", fork_result);

            // Wait for child to complete
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = waitpid(fork_result, &mut status, 0);

            if wait_result != fork_result {
                println!("[PARENT] FAIL: waitpid returned wrong PID");
                println!("SIGNAL_FORK_TEST_FAILED");
                std::process::exit(1);
            }

            // Check if child exited normally with code 0
            if wifexited(status) {
                let exit_code = wexitstatus(status);
                if exit_code == 0 {
                    println!("[PARENT] Child exited successfully (code 0)");
                    println!("\n=== All signal fork inheritance tests passed! ===");
                    println!("SIGNAL_FORK_TEST_PASSED");
                    std::process::exit(0);
                } else {
                    println!("[PARENT] Child exited with non-zero code: {}", exit_code);
                    println!("SIGNAL_FORK_TEST_FAILED");
                    std::process::exit(1);
                }
            } else {
                println!("[PARENT] Child did not exit normally");
                println!("SIGNAL_FORK_TEST_FAILED");
                std::process::exit(1);
            }
        }
    }
}
