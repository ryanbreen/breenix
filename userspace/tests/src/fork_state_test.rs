//! Fork state inheritance test (std version)
//!
//! Tests that copy_process_state correctly copies all inherited state:
//! 1. File descriptors - pipe FD works across fork (shared file position)
//! 2. Signal handlers - child inherits parent's handler
//! 3. Process group ID - child inherits parent's pgid
//! 4. Session ID - child inherits parent's sid
//!
//! POSIX requires all of these to be inherited by the child process.

use std::sync::atomic::{AtomicBool, Ordering};

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

const SIGUSR1: i32 = 10;
const SA_RESTORER: u64 = 0x04000000;

// Syscall numbers
const SYS_SIGACTION: u64 = 13;
const SYS_GETPGID: u64 = 121;
const SYS_GETSID: u64 = 124;

#[repr(C)]
struct KernelSigaction {
    handler: u64,
    mask: u64,
    flags: u64,
    restorer: u64,
}

extern "C" {
    fn fork() -> i32;
    fn getpid() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn sched_yield() -> i32;
    fn pipe(pipefd: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
}

// Raw syscall helpers for sigaction, getpgid, getsid

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
unsafe fn raw_syscall1(num: u64, arg1: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_syscall1(num: u64, arg1: u64) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") num,
        inlateout("x0") arg1 => ret,
        options(nostack),
    );
    ret as i64
}

// Signal restorer trampoline
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

fn getpgid(pid: i32) -> i32 {
    unsafe { raw_syscall1(SYS_GETPGID, pid as u64) as i32 }
}

fn getsid(pid: i32) -> i32 {
    unsafe { raw_syscall1(SYS_GETSID, pid as u64) as i32 }
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
extern "C" fn sigusr1_handler(_sig: i32) {
    HANDLER_CALLED.store(true, Ordering::SeqCst);
}

fn fail(msg: &str) -> ! {
    println!("FORK_STATE_TEST FAIL: {}", msg);
    println!("FORK_STATE_COPY_FAILED");
    std::process::exit(1);
}

fn main() {
    println!("=== Fork State Copy Test ===\n");

    // =============================================
    // STEP 1: Set up parent state before fork
    // =============================================
    println!("Step 1: Setting up parent state...");

    // 1a. Create a pipe (FD inheritance test)
    println!("  1a. Creating pipe for FD inheritance test");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };
    if ret < 0 {
        fail("pipe creation failed");
    }
    let read_fd = pipefd[0];
    let write_fd = pipefd[1];
    println!("      Pipe created: read_fd={}, write_fd={}", read_fd, write_fd);

    // 1b. Register signal handler (signal inheritance test)
    println!("  1b. Registering SIGUSR1 handler");
    let action = KernelSigaction {
        handler: sigusr1_handler as u64,
        mask: 0,
        flags: SA_RESTORER,
        restorer: __restore_rt as u64,
    };

    let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
    if ret < 0 {
        fail("sigaction failed");
    }
    println!("      SIGUSR1 handler registered");

    // 1c. Get parent's pgid and sid
    println!("  1c. Getting parent pgid and sid");
    let parent_pid = unsafe { getpid() };
    let parent_pgid = getpgid(0);
    let parent_sid = getsid(0);
    println!("      Parent PID={}, PGID={}, SID={}", parent_pid, parent_pgid, parent_sid);

    // Write test data to pipe before fork
    println!("  1d. Writing test data to pipe");
    let test_data = b"FORK_TEST_DATA";
    let written = unsafe { write(write_fd, test_data.as_ptr(), test_data.len()) };
    if written != test_data.len() as isize {
        fail("pipe write failed");
    }
    println!("      Wrote {} bytes to pipe", written);

    // =============================================
    // STEP 2: Fork
    // =============================================
    println!("\nStep 2: Forking process...");
    let fork_result = unsafe { fork() };

    if fork_result < 0 {
        fail("fork failed");
    }

    if fork_result == 0 {
        // =============================================
        // CHILD PROCESS
        // =============================================
        let child_pid = unsafe { getpid() };
        println!("\n[CHILD] Started (fork returned 0)");
        println!("[CHILD] PID={}", child_pid);

        let mut tests_passed = 0;
        let total_tests = 4;

        // Test 1: FD inheritance - read from inherited pipe
        println!("\n[CHILD] Test 1: File descriptor inheritance");
        let mut read_buf = [0u8; 32];
        // Retry on EAGAIN
        let mut bytes_read: isize = -11;
        let mut retries = 0;
        while bytes_read == -11 && retries < 50 {
            bytes_read = unsafe { read(read_fd, read_buf.as_mut_ptr(), read_buf.len()) };
            if bytes_read == -11 {
                unsafe { sched_yield(); }
                retries += 1;
            }
        }
        if bytes_read == test_data.len() as isize {
            // Verify content
            let read_slice = &read_buf[..bytes_read as usize];
            if read_slice == test_data {
                println!("[CHILD]   PASS: Read correct data from inherited pipe FD");
                tests_passed += 1;
            } else {
                println!("[CHILD]   FAIL: Data mismatch on inherited FD");
            }
        } else {
            println!("[CHILD]   FAIL: Could not read from inherited pipe (bytes={})", bytes_read);
        }

        // Test 2: Signal handler inheritance - send SIGUSR1 to self
        println!("\n[CHILD] Test 2: Signal handler inheritance");
        let kill_ret = unsafe { kill(child_pid, SIGUSR1) };
        if kill_ret == 0 {
            // Yield to allow signal delivery
            for _ in 0..20 {
                unsafe { sched_yield(); }
                if HANDLER_CALLED.load(Ordering::SeqCst) {
                    break;
                }
            }
            if HANDLER_CALLED.load(Ordering::SeqCst) {
                println!("[CHILD]   PASS: Inherited signal handler was called");
                tests_passed += 1;
            } else {
                println!("[CHILD]   FAIL: Inherited signal handler was NOT called");
            }
        } else {
            println!("[CHILD]   FAIL: kill() failed");
        }

        // Test 3: PGID inheritance
        println!("\n[CHILD] Test 3: Process group ID inheritance");
        let child_pgid = getpgid(0);
        println!("[CHILD]   Parent PGID={}, Child PGID={}", parent_pgid, child_pgid);
        if child_pgid == parent_pgid {
            println!("[CHILD]   PASS: Child inherited parent's PGID");
            tests_passed += 1;
        } else {
            println!("[CHILD]   FAIL: PGID mismatch");
        }

        // Test 4: Session ID inheritance
        println!("\n[CHILD] Test 4: Session ID inheritance");
        let child_sid = getsid(0);
        println!("[CHILD]   Parent SID={}, Child SID={}", parent_sid, child_sid);
        if child_sid == parent_sid {
            println!("[CHILD]   PASS: Child inherited parent's SID");
            tests_passed += 1;
        } else {
            println!("[CHILD]   FAIL: SID mismatch");
        }

        // Close child's pipe FDs
        unsafe {
            close(read_fd);
            close(write_fd);
        }

        // Summary
        println!("\n[CHILD] Tests passed: {}/{}", tests_passed, total_tests);

        if tests_passed == total_tests {
            println!("[CHILD] All tests PASSED!");
            std::process::exit(0);
        } else {
            println!("[CHILD] Some tests FAILED!");
            std::process::exit(1);
        }
    } else {
        // =============================================
        // PARENT PROCESS
        // =============================================
        println!("[PARENT] Forked child PID: {}", fork_result);

        // Close parent's pipe FDs (child has them)
        unsafe {
            close(read_fd);
            close(write_fd);
        }

        // Wait for child to complete
        println!("[PARENT] Waiting for child...");
        let mut status: i32 = 0;
        let wait_result = unsafe { waitpid(fork_result, &mut status, 0) };

        if wait_result != fork_result {
            println!("[PARENT] waitpid returned wrong PID");
            println!("FORK_STATE_COPY_FAILED");
            std::process::exit(1);
        }

        // Check if child exited normally with code 0
        if wifexited(status) {
            let exit_code = wexitstatus(status);
            if exit_code == 0 {
                println!("\n=== All fork state copy tests passed! ===");
                println!("FORK_STATE_COPY_PASSED");
                std::process::exit(0);
            } else {
                println!("[PARENT] Child exited with error code {}", exit_code);
                println!("FORK_STATE_COPY_FAILED");
                std::process::exit(1);
            }
        } else {
            println!("[PARENT] Child did not exit normally");
            println!("FORK_STATE_COPY_FAILED");
            std::process::exit(1);
        }
    }
}
