//! Fork pending signal non-inheritance test
//!
//! POSIX requires that pending signals are NOT inherited by the child after fork().

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::println;
use libbreenix::process::{exit, fork, getpid, waitpid, wexitstatus, wifexited};
use libbreenix::signal::{kill, sigprocmask, SIG_BLOCK, SIGUSR1};
use libbreenix::syscall::raw;

const SYS_RT_SIGPENDING: u64 = 127;

#[derive(Clone, Copy)]
struct SigSet {
    mask: u64,
}

impl SigSet {
    const fn empty() -> Self {
        Self { mask: 0 }
    }

    fn add(&mut self, sig: i32) {
        self.mask |= libbreenix::signal::sigmask(sig);
    }

    fn contains(&self, sig: i32) -> bool {
        (self.mask & libbreenix::signal::sigmask(sig)) != 0
    }
}

fn sigpending(set: &mut SigSet) -> Result<(), i32> {
    let mut raw_set = set.mask;
    let ret = unsafe {
        raw::syscall2(
            SYS_RT_SIGPENDING,
            &mut raw_set as *mut u64 as u64,
            8,
        )
    };
    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        set.mask = raw_set;
        Ok(())
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Fork Pending Signal Test ===");

    let mut mask = SigSet::empty();
    mask.add(SIGUSR1);
    let mut old_mask: u64 = 0;
    if sigprocmask(SIG_BLOCK, Some(&mask.mask), Some(&mut old_mask)).is_err() {
        println("sigprocmask failed");
        exit(1);
    }

    if kill(getpid() as i32, SIGUSR1).is_err() {
        println("kill failed");
        exit(1);
    }

    let mut pending = SigSet::empty();
    if sigpending(&mut pending).is_err() {
        println("sigpending failed");
        exit(1);
    }

    if !pending.contains(SIGUSR1) {
        println("Parent: SIGUSR1 not pending - test setup failed");
        exit(1);
    }
    println("Parent: SIGUSR1 is pending (expected)");

    let pid = fork();
    if pid == 0 {
        let mut child_pending = SigSet::empty();
        if sigpending(&mut child_pending).is_err() {
            println("Child: sigpending failed");
            exit(1);
        }

        if child_pending.contains(SIGUSR1) {
            println("Child: SIGUSR1 is pending - FAIL (should not inherit pending signals)");
            println("FORK_PENDING_SIGNAL_TEST_FAILED");
            exit(1);
        }

        println("Child: No pending signals (correct POSIX behavior)");
        println("FORK_PENDING_SIGNAL_TEST_PASSED");
        exit(0);
    } else if pid > 0 {
        let mut status: i32 = 0;
        let waited = waitpid(pid as i32, &mut status, 0);
        if waited != pid {
            println("waitpid failed");
            exit(1);
        }
        if wifexited(status) {
            exit(wexitstatus(status));
        }
        println("Child did not exit normally");
        exit(1);
    } else {
        println("fork failed");
        exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(255);
}
