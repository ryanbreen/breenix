//! Fork pending signal non-inheritance test (std version)
//!
//! POSIX requires that pending signals are NOT inherited by the child after fork().

const SIGUSR1: i32 = 10;
const SIG_BLOCK: i32 = 0;
const SYS_RT_SIGPENDING: i64 = 127;

extern "C" {
    fn fork() -> i32;
    fn getpid() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn sigprocmask(how: i32, set: *const u64, oldset: *mut u64) -> i32;
    fn syscall(num: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, a6: i64) -> i64;
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Convert signal number to bitmask
fn sigmask(sig: i32) -> u64 {
    1u64 << (sig - 1)
}

fn sigpending(set: &mut u64) -> Result<(), i32> {
    let ret = unsafe {
        syscall(
            SYS_RT_SIGPENDING,
            set as *mut u64 as i64,
            8,
            0,
            0,
            0,
            0,
        )
    };
    if ret < 0 {
        Err((-ret) as i32)
    } else {
        Ok(())
    }
}

fn main() {
    println!("=== Fork Pending Signal Test ===");

    // Block SIGUSR1 so it becomes pending when sent
    let mask: u64 = sigmask(SIGUSR1);
    let mut old_mask: u64 = 0;
    if unsafe { sigprocmask(SIG_BLOCK, &mask, &mut old_mask) } != 0 {
        println!("sigprocmask failed");
        std::process::exit(1);
    }

    // Send SIGUSR1 to self - it will be pending since it's blocked
    let my_pid = unsafe { getpid() };
    if unsafe { kill(my_pid, SIGUSR1) } != 0 {
        println!("kill failed");
        std::process::exit(1);
    }

    // Verify SIGUSR1 is pending in parent
    let mut pending: u64 = 0;
    if sigpending(&mut pending).is_err() {
        println!("sigpending failed");
        std::process::exit(1);
    }

    if (pending & sigmask(SIGUSR1)) == 0 {
        println!("Parent: SIGUSR1 not pending - test setup failed");
        std::process::exit(1);
    }
    println!("Parent: SIGUSR1 is pending (expected)");

    let pid = unsafe { fork() };
    if pid == 0 {
        // ========== CHILD PROCESS ==========
        let mut child_pending: u64 = 0;
        if sigpending(&mut child_pending).is_err() {
            println!("Child: sigpending failed");
            std::process::exit(1);
        }

        if (child_pending & sigmask(SIGUSR1)) != 0 {
            println!("Child: SIGUSR1 is pending - FAIL (should not inherit pending signals)");
            println!("FORK_PENDING_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        }

        println!("Child: No pending signals (correct POSIX behavior)");
        println!("FORK_PENDING_SIGNAL_TEST_PASSED");
        std::process::exit(0);
    } else if pid > 0 {
        // ========== PARENT PROCESS ==========
        let mut status: i32 = 0;
        let waited = unsafe { waitpid(pid, &mut status, 0) };
        if waited != pid {
            println!("waitpid failed");
            std::process::exit(1);
        }
        if wifexited(status) {
            std::process::exit(wexitstatus(status));
        }
        println!("Child did not exit normally");
        std::process::exit(1);
    } else {
        println!("fork failed");
        std::process::exit(1);
    }
}
