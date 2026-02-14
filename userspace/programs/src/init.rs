//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - spawns system services and the shell, then reaps zombies.
//!
//! Spawns:
//!   - /sbin/telnetd (background service, optional)
//!   - /bin/bsh (Breenix Shell)
//!
//! Main loop reaps terminated children with waitpid(WNOHANG) and respawns
//! crashed services with backoff to prevent tight respawn loops.

use libbreenix::process::{fork, exec, waitpid, getpid, yield_now, ForkResult, WNOHANG};
use libbreenix::time::nanosleep;
use libbreenix::types::Timespec;

const TELNETD_PATH: &[u8] = b"/sbin/telnetd\0";
const SHELL_PATH: &[u8] = b"/bin/bsh\0";

/// Maximum number of rapid respawns before giving up on a service.
const MAX_RESPAWN_FAILURES: u32 = 3;

/// Fork and exec a binary. Returns the child PID on success, -1 on failure.
fn spawn(path: &[u8], name: &str) -> i64 {
    match fork() {
        Ok(ForkResult::Child) => {
            // Child: exec the binary
            let _ = exec(path);
            // exec failed
            print!("[init] ERROR: exec failed for {}\n", name);
            std::process::exit(127);
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
    // Brief delay before respawn to avoid tight loops
    let delay = Timespec { tv_sec: 0, tv_nsec: 100_000_000 }; // 100ms
    let _ = nanosleep(&delay);
    print!("[init] Respawning {}... (attempt {})\n", name, *failures);
    spawn(path, name)
}

fn main() {
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);

    // Start telnetd (optional -- may not exist on all disk images)
    print!("[init] Starting /sbin/telnetd...\n");
    let mut telnetd_pid = spawn(TELNETD_PATH, "telnetd");
    let mut telnetd_failures: u32 = 0;

    // Start bsh (Breenix Shell)
    print!("[init] Starting /bin/bsh...\n");
    let mut shell_pid = spawn(SHELL_PATH, "bsh");
    let mut shell_failures: u32 = 0;

    // Main loop: reap zombies and respawn crashed services
    let mut status: i32 = 0;
    loop {
        match waitpid(-1, &mut status as *mut i32, WNOHANG) {
            Ok(reaped_pid) => {
                let reaped = reaped_pid.raw() as i64;
                if reaped > 0 {
                    if reaped == shell_pid {
                        print!("[init] Shell exited (status {})\n", status);
                        shell_pid = try_respawn(SHELL_PATH, "bsh", &mut shell_failures);
                        if shell_pid == -1 {
                            print!("[init] Shell failed {} times, giving up\n", MAX_RESPAWN_FAILURES);
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
