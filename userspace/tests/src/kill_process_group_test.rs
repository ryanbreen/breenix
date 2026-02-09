//! Process Group Kill Semantics Test (std version)
//!
//! Tests process group kill semantics:
//! 1. kill(pid, 0) - Check if process/group exists without sending signal
//! 2. kill(0, sig) - Send signal to all processes in caller's process group
//! 3. kill(-pgid, sig) - Send signal to specific process group
//!
//! Simplified to use polling loops instead of sigsuspend for robustness
//! in concurrent test environments with many competing processes.

use std::sync::atomic::{AtomicU32, Ordering};

/// Static counters to track signal delivery
static SIGUSR1_COUNT: AtomicU32 = AtomicU32::new(0);
static SIGUSR2_COUNT: AtomicU32 = AtomicU32::new(0);

// Signal constants
const SIGUSR1: i32 = 10;
const SIGUSR2: i32 = 12;
const SA_RESTORER: u64 = 0x04000000;

// Syscall numbers
const SYS_SIGACTION: u64 = 13;
const SYS_SETPGID: u64 = 109;
const SYS_GETPGID: u64 = 121;

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

#[cfg(target_arch = "x86_64")]
unsafe fn raw_setpgid(pid: u64, pgid: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") SYS_SETPGID,
        in("rdi") pid,
        in("rsi") pgid,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_setpgid(pid: u64, pgid: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") SYS_SETPGID,
        inlateout("x0") pid => ret,
        in("x1") pgid,
        options(nostack),
    );
    ret as i64
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_getpgid(pid: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") SYS_GETPGID,
        in("rdi") pid,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_getpgid(pid: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") SYS_GETPGID,
        inlateout("x0") pid => ret,
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

// --- Helper functions ---

fn setpgid(pid: i32, pgid: i32) -> i32 {
    unsafe { raw_setpgid(pid as u64, pgid as u64) as i32 }
}

fn getpgrp() -> i32 {
    unsafe { raw_getpgid(0) as i32 }
}

fn getpgid(pid: i32) -> i32 {
    unsafe { raw_getpgid(pid as u64) as i32 }
}

/// POSIX wait status macros
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// SIGUSR1 handler - increments counter
extern "C" fn sigusr1_handler(_sig: i32) {
    SIGUSR1_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// SIGUSR2 handler - increments counter
extern "C" fn sigusr2_handler(_sig: i32) {
    SIGUSR2_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// Poll for signal delivery by checking atomic counter with yield
fn poll_for_signal(counter: &AtomicU32, max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        if counter.load(Ordering::SeqCst) > 0 {
            return true;
        }
        unsafe { sched_yield(); }
    }
    counter.load(Ordering::SeqCst) > 0
}

fn main() {
    unsafe {
        println!("=== Process Group Kill Semantics Test ===\n");

        // Register signal handlers
        let action1 = KernelSigaction {
            handler: sigusr1_handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };
        let action2 = KernelSigaction {
            handler: sigusr2_handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };

        if raw_sigaction(SIGUSR1, &action1, std::ptr::null_mut()) < 0 {
            println!("FAIL: Failed to register SIGUSR1 handler");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        if raw_sigaction(SIGUSR2, &action2, std::ptr::null_mut()) < 0 {
            println!("FAIL: Failed to register SIGUSR2 handler");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        // Test 1: kill(pid, 0) - Check if process exists
        println!("Test 1: kill(pid, 0) - Check process existence");
        let parent_pid = getpid();

        let ret = kill(parent_pid, 0);
        if ret == 0 {
            println!("  PASS: kill(self, 0) succeeded (process exists)");
        } else {
            println!("  FAIL: kill(self, 0) returned error {}", -ret);
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        // Check non-existent process
        let ret = kill(99999, 0);
        if ret != 0 {
            println!("  PASS: kill(99999, 0) failed with errno {} (process does not exist)", -ret);
        } else {
            println!("  FAIL: kill(99999, 0) succeeded (should fail with ESRCH)");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        // Test 2: Create process group and test kill(0, sig)
        println!("\nTest 2: kill(0, sig) - Send signal to own process group");

        if setpgid(0, 0) < 0 {
            println!("  FAIL: setpgid(0, 0) failed");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        let pgid = getpgrp();
        if pgid < 0 {
            println!("  FAIL: getpgrp() failed");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        println!("  Created process group {}", pgid);

        // Fork a child into the same process group
        let child1 = fork();
        if child1 < 0 {
            println!("  FAIL: fork() failed");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        if child1 == 0 {
            // Child 1: Poll for SIGUSR1 (handler increments counter)
            if !poll_for_signal(&SIGUSR1_COUNT, 5000) {
                println!("  [Child1] FAIL: Did not receive SIGUSR1");
                std::process::exit(1);
            }
            println!("  [Child1] PASS: Received SIGUSR1 via kill(0, sig)");
            std::process::exit(0);
        } else {
            // Parent: Brief yield to let child start, then send signal
            for _ in 0..50 { sched_yield(); }

            let ret = kill(0, SIGUSR1);
            if ret != 0 {
                println!("  [Parent] FAIL: kill(0, SIGUSR1) failed with errno {}", -ret);
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }

            // Parent also receives the signal (same group)
            if !poll_for_signal(&SIGUSR1_COUNT, 1000) {
                println!("  [Parent] FAIL: Did not receive SIGUSR1");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }
            println!("  [Parent] PASS: Received SIGUSR1 (process group signal delivery works)");

            // Wait for child
            let mut status: i32 = 0;
            let wait_result = waitpid(child1, &mut status, 0);
            if wait_result != child1 {
                println!("  [Parent] WARNING: waitpid returned {}", wait_result);
            } else if !wifexited(status) || wexitstatus(status) != 0 {
                println!("  [Parent] FAIL: Child1 exited with non-zero status");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }
        }

        // Test 3: kill(-pgid, sig) - Send signal to specific process group
        println!("\nTest 3: kill(-pgid, sig) - Send signal to specific process group");

        // Reset counters
        SIGUSR2_COUNT.store(0, Ordering::SeqCst);

        // Fork child2 that creates its own process group
        let child2 = fork();
        if child2 < 0 {
            println!("  FAIL: fork() failed for child2");
            println!("KILL_PGROUP_TEST_FAILED");
            std::process::exit(1);
        }

        if child2 == 0 {
            // Child 2: Create new process group
            if setpgid(0, 0) < 0 {
                println!("  [Child2] FAIL: setpgid(0, 0) failed");
                std::process::exit(1);
            }

            let child2_pgid = getpgrp();
            println!("  [Child2] Created new process group {}", child2_pgid);

            // Fork grandchild into child2's process group
            let grandchild = fork();
            if grandchild < 0 {
                println!("  [Child2] FAIL: fork() failed for grandchild");
                std::process::exit(1);
            }

            if grandchild == 0 {
                // Grandchild: Poll for SIGUSR2
                if !poll_for_signal(&SIGUSR2_COUNT, 5000) {
                    println!("  [Grandchild] FAIL: Did not receive SIGUSR2");
                    std::process::exit(1);
                }
                println!("  [Grandchild] PASS: Received SIGUSR2 via kill(-pgid, sig)");
                std::process::exit(0);
            } else {
                // Child2: Poll for SIGUSR2
                if !poll_for_signal(&SIGUSR2_COUNT, 5000) {
                    println!("  [Child2] FAIL: Did not receive SIGUSR2");
                    std::process::exit(1);
                }
                println!("  [Child2] PASS: Received SIGUSR2 via kill(-pgid, sig)");

                // Wait for grandchild
                let mut gc_status: i32 = 0;
                let gc_wait = waitpid(grandchild, &mut gc_status, 0);
                if gc_wait != grandchild {
                    println!("  [Child2] WARNING: waitpid(grandchild) returned {}", gc_wait);
                } else if !wifexited(gc_status) || wexitstatus(gc_status) != 0 {
                    println!("  [Child2] FAIL: Grandchild exited with non-zero status");
                    std::process::exit(1);
                }

                std::process::exit(0);
            }
        } else {
            // Parent: Wait for child2 to create its own process group by
            // polling getpgid() until it differs from the parent's pgid.
            let parent_pgid = getpgrp();
            let mut child2_pgid;
            let mut attempts = 0;
            loop {
                child2_pgid = getpgid(child2);
                if child2_pgid > 0 && child2_pgid != parent_pgid {
                    break;
                }
                attempts += 1;
                if attempts > 5000 {
                    println!("  [Parent] FAIL: child2 did not create its own process group after {} attempts", attempts);
                    println!("KILL_PGROUP_TEST_FAILED");
                    std::process::exit(1);
                }
                sched_yield();
            }

            // Brief yield to let grandchild start polling
            for _ in 0..50 { sched_yield(); }

            println!("  [Parent] Sending SIGUSR2 to process group {} via kill(-pgid, sig)", child2_pgid);

            let ret = kill(-child2_pgid, SIGUSR2);
            if ret == 0 {
                println!("  [Parent] kill(-pgid, SIGUSR2) succeeded");
            } else {
                println!("  [Parent] FAIL: kill(-pgid, SIGUSR2) failed with errno {}", -ret);
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }

            // Parent should NOT receive SIGUSR2 (different process group)
            for _ in 0..50 { sched_yield(); }

            if SIGUSR2_COUNT.load(Ordering::SeqCst) == 0 {
                println!("  [Parent] PASS: Did not receive SIGUSR2 (not in target process group)");
            } else {
                println!("  [Parent] FAIL: Incorrectly received SIGUSR2 (should only go to child2's group)");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }

            // Wait for child2
            let mut status2: i32 = 0;
            let wait_result2 = waitpid(child2, &mut status2, 0);
            if wait_result2 != child2 {
                println!("  [Parent] WARNING: waitpid(child2) returned {}", wait_result2);
            } else if !wifexited(status2) || wexitstatus(status2) != 0 {
                println!("  [Parent] FAIL: Child2 exited with non-zero status");
                println!("KILL_PGROUP_TEST_FAILED");
                std::process::exit(1);
            }
        }

        // All tests passed
        println!("\n=== All process group kill tests passed! ===");
        println!("KILL_PGROUP_TEST_PASSED");
        std::process::exit(0);
    }
}
