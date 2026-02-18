//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - spawns system services and the window manager, then reaps zombies.
//!
//! Spawns:
//!   - /sbin/telnetd (background service, optional)
//!   - /bin/bwm (window manager — owns keyboard input, renders right-side display,
//!     spawns its own bsh on a PTY)
//!
//! Main loop reaps terminated children with waitpid(WNOHANG) and respawns
//! crashed services with backoff to prevent tight respawn loops.

use libbreenix::process::{fork, exec, waitpid, getpid, yield_now, ForkResult, WNOHANG};

const TELNETD_PATH: &[u8] = b"/sbin/telnetd\0";
const BLOGD_PATH: &[u8] = b"/sbin/blogd\0";
const BWM_PATH: &[u8] = b"/bin/bwm\0";
/// Maximum number of rapid respawns before giving up on a service.
const MAX_RESPAWN_FAILURES: u32 = 3;

/// Fork and exec a binary. Returns the child PID on success, -1 on failure.
fn spawn(path: &[u8], name: &str) -> i64 {
    match fork() {
        Ok(ForkResult::Child) => {
            // Child: exec the binary
            match exec(path) {
                Ok(_) => unreachable!(),
                Err(e) => {
                    print!("[init] ERROR: exec failed for {} ({})\n", name, e);
                    std::process::exit(127);
                }
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            child_pid.raw() as i64
        }
        Err(_) => {
            print!("[init] ERROR: fork failed for {}\n", name);
            -1
        }
    }
}

/// Try to spawn a service, returning its PID or -1 if max failures reached.
fn try_respawn(path: &[u8], name: &str, failures: &mut u32) -> i64 {
    if *failures >= MAX_RESPAWN_FAILURES {
        return -1; // Gave up
    }
    *failures += 1;
    print!("[init] Respawning {}... (attempt {})\n", name, *failures);
    spawn(path, name)
}

/// Test: simple fork + exit + waitpid to exercise process lifecycle under SMP load.
fn test_fork_exit() {
    match fork() {
        Ok(ForkResult::Child) => {
            // Child: just exit immediately
            libbreenix::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child_raw = child_pid.raw() as i32;
            let mut status: i32 = 0;
            match waitpid(child_raw, &mut status as *mut i32, 0) {
                Ok(reaped) => {
                    print!("[init] TEST: child {} reaped, status={}\n", reaped.raw(), status);
                }
                Err(e) => {
                    print!("[init] TEST: waitpid(pid={}) failed: {}\n", child_raw, e);
                }
            }
        }
        Err(e) => {
            print!("[init] TEST: fork failed: {}\n", e);
        }
    }
}

fn main() {
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);

    // Start telnetd (optional -- may not exist on all disk images)
    print!("[init] Starting /sbin/telnetd...\n");
    let mut telnetd_pid = spawn(TELNETD_PATH, "telnetd");
    let mut telnetd_failures: u32 = 0;

    // Start blogd (kernel log daemon — persists /proc/kmsg to /var/log/kernel.log)
    print!("[init] Starting /sbin/blogd...\n");
    let mut blogd_pid = spawn(BLOGD_PATH, "blogd");
    let mut blogd_failures: u32 = 0;

    // Start BWM (window manager -- owns keyboard stdin, renders right-side display,
    // spawns its own bsh + btop on PTYs)
    print!("[init] Starting /bin/bwm...\n");
    let mut bwm_pid = spawn(BWM_PATH, "bwm");
    let mut bwm_failures: u32 = 0;

    // Test: simple fork + exit + waitpid under SMP load (process lifecycle regression)
    // Run after BWM is started so there's full SMP contention.
    // Keep at 5 iterations — enough to stress-test without delaying BWM init
    // past the strict boot test's 18-second detection window.
    for i in 0..5 {
        print!("[init] TEST {}/5: fork+exit...\n", i + 1);
        test_fork_exit();
        let _ = yield_now();
    }
    print!("[init] TEST: all 5 iterations completed successfully\n");

    // Main loop: reap zombies and respawn crashed services.
    let mut status: i32 = 0;
    loop {
        match waitpid(-1, &mut status as *mut i32, WNOHANG) {
            Ok(reaped_pid) => {
                let reaped = reaped_pid.raw() as i64;
                if reaped > 0 {
                    if reaped == bwm_pid {
                        print!("[init] BWM exited (status {})\n", status);
                        bwm_pid = try_respawn(BWM_PATH, "bwm", &mut bwm_failures);
                        if bwm_pid == -1 {
                            print!("[init] BWM failed {} times, giving up\n", MAX_RESPAWN_FAILURES);
                        }
                    } else if reaped == blogd_pid {
                        print!("[init] blogd exited (status {})\n", status);
                        blogd_pid = try_respawn(BLOGD_PATH, "blogd", &mut blogd_failures);
                        if blogd_pid == -1 {
                            print!("[init] blogd failed {} times, giving up\n", MAX_RESPAWN_FAILURES);
                        }
                    } else if reaped == telnetd_pid {
                        telnetd_pid = try_respawn(TELNETD_PATH, "telnetd", &mut telnetd_failures);
                        if telnetd_pid == -1 && telnetd_failures >= MAX_RESPAWN_FAILURES {
                            print!("[init] telnetd unavailable, continuing without it\n");
                        }
                    }
                }
            }
            Err(_) => {}
        }
        let _ = yield_now();
    }
}
