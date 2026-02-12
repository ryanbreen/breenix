//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - spawns system services and the interactive shell, then reaps zombies.
//!
//! Spawns:
//!   - /sbin/telnetd (background service)
//!   - /bin/init_shell (foreground shell on serial console)
//!
//! Main loop reaps terminated children with waitpid(WNOHANG) and respawns
//! crashed services.

use libbreenix::process::{fork, exec, waitpid, getpid, yield_now, ForkResult, WNOHANG};

const TELNETD_PATH: &[u8] = b"/sbin/telnetd\0";
const SHELL_PATH: &[u8] = b"/bin/init_shell\0";

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

fn main() {
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);

    // Start telnetd
    print!("[init] Starting /sbin/telnetd...\n");
    let mut telnetd_pid = spawn(TELNETD_PATH, "telnetd");

    // Start interactive shell
    print!("[init] Starting /bin/init_shell...\n");
    let mut shell_pid = spawn(SHELL_PATH, "init_shell");

    // Main loop: reap zombies and respawn crashed services
    let mut status: i32 = 0;
    loop {
        match waitpid(-1, &mut status as *mut i32, WNOHANG) {
            Ok(reaped_pid) => {
                let reaped = reaped_pid.raw() as i64;
                if reaped > 0 {
                    if reaped == shell_pid {
                        // Shell exited -- respawn it
                        print!("[init] Shell exited, respawning...\n");
                        shell_pid = spawn(SHELL_PATH, "init_shell");
                    } else if reaped == telnetd_pid {
                        // Telnetd crashed -- respawn it
                        print!("[init] telnetd exited, respawning...\n");
                        telnetd_pid = spawn(TELNETD_PATH, "telnetd");
                    }
                }
            }
            Err(_) => {}
        }
        let _ = yield_now();
    }
}
