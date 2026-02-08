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

const WNOHANG: i32 = 1;

const TELNETD_PATH: &[u8] = b"/sbin/telnetd\0";
const SHELL_PATH: &[u8] = b"/bin/init_shell\0";

extern "C" {
    fn fork() -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn getpid() -> i32;
    fn sched_yield() -> i32;
}

/// Fork and exec a binary. Returns the child PID on success, -1 on failure.
fn spawn(path: &[u8], name: &str) -> i64 {
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: exec the binary
        let argv: [*const u8; 2] = [path.as_ptr(), std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];
        unsafe { execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        // exec failed
        print!("[init] ERROR: exec failed for {}\n", name);
        std::process::exit(127);
    }
    if pid < 0 {
        print!("[init] ERROR: fork failed for {}\n", name);
        return -1;
    }
    pid as i64
}

fn main() {
    let pid = unsafe { getpid() };
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
        let reaped = unsafe { waitpid(-1, &mut status as *mut i32, WNOHANG) };
        if reaped > 0 {
            let reaped_i64 = reaped as i64;
            if reaped_i64 == shell_pid {
                // Shell exited -- respawn it
                print!("[init] Shell exited, respawning...\n");
                shell_pid = spawn(SHELL_PATH, "init_shell");
            } else if reaped_i64 == telnetd_pid {
                // Telnetd crashed -- respawn it
                print!("[init] telnetd exited, respawning...\n");
                telnetd_pid = spawn(TELNETD_PATH, "telnetd");
            }
        }
        unsafe { sched_yield(); }
    }
}
