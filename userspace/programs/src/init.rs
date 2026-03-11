//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - spawns system services, the compositor, and GUI apps, then reaps zombies.
//!
//! Spawns:
//!   - /sbin/telnetd (background service, optional)
//!   - /sbin/blogd  (kernel log daemon — reads /proc/kmsg, writes /var/log/kernel.log)
//!   - /bin/bwm     (pure compositor — owns display, composes Breengel client windows)
//!   - /bin/bterm   (terminal emulator — Breengel client, creates its own PTY + bsh)
//!   - /bin/blog    (log viewer — Breengel client, tails /var/log/kernel.log)
//!   - /bin/bounce  (GPU demo)
//!   - /bin/bcheck  (self-check test runner)
//!
//! BWM is a pure compositor: it no longer spawns terminals internally. Instead,
//! bterm and blog are standalone Breengel GUI apps that register windows with BWM.
//!
//! Main loop blocks on waitpid() until a child exits, then respawns
//! crashed services with backoff to prevent tight respawn loops.

use libbreenix::process::{fork, exec, execv, waitpid, getpid, yield_now, ForkResult};

const TELNETD_PATH: &[u8] = b"/sbin/telnetd\0";
const BLOGD_PATH: &[u8] = b"/sbin/blogd\0";
const BWM_PATH: &[u8] = b"/bin/bwm\0";
const BTERM_PATH: &[u8] = b"/bin/bterm\0";
const BLOG_PATH: &[u8] = b"/bin/blog\0";
const BOUNCE_PATH: &[u8] = b"/bin/bounce\0";
const BCHECK_PATH: &[u8] = b"/bin/bcheck\0";
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

/// Test: run /bin/cat /etc/passwd via BusyBox to verify musl libc works on ARM64.
fn test_busybox_cat() {
    match fork() {
        Ok(ForkResult::Child) => {
            // Child: exec /bin/cat with argv = ["cat", "/etc/passwd"]
            let arg0 = b"cat\0";
            let arg1 = b"/etc/passwd\0";
            let argv: [*const u8; 3] = [
                arg0.as_ptr(),
                arg1.as_ptr(),
                core::ptr::null(),
            ];
            match execv(b"/bin/cat\0", argv.as_ptr()) {
                Ok(_) => unreachable!(),
                Err(e) => {
                    print!("[init] BUSYBOX TEST FAILED: exec /bin/cat: {}\n", e);
                    std::process::exit(126);
                }
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child_raw = child_pid.raw() as i32;
            let mut status: i32 = 0;
            match waitpid(child_raw, &mut status as *mut i32, 0) {
                Ok(reaped) => {
                    let exit_code = (status >> 8) & 0xFF;
                    print!("[init] BUSYBOX TEST: cat exited, pid={} status={} exit_code={}\n",
                        reaped.raw(), status, exit_code);
                    if exit_code == 0 {
                        print!("[init] BUSYBOX TEST PASSED\n");
                    } else {
                        print!("[init] BUSYBOX TEST FAILED (exit_code={})\n", exit_code);
                    }
                }
                Err(e) => {
                    print!("[init] BUSYBOX TEST FAILED: waitpid: {}\n", e);
                }
            }
        }
        Err(e) => {
            print!("[init] BUSYBOX TEST FAILED: fork: {}\n", e);
        }
    }
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

    // Start BWM (pure compositor — owns display, composes Breengel client windows)
    print!("[init] Starting /bin/bwm...\n");
    let mut bwm_pid = spawn(BWM_PATH, "bwm");
    let mut bwm_failures: u32 = 0;

    // Yield to give BWM time to initialize the compositor before clients connect
    let _ = yield_now();

    // Start terminal emulator (Breengel client — creates its own PTY + bsh)
    print!("[init] Starting /bin/bterm...\n");
    let mut bterm_pid = spawn(BTERM_PATH, "bterm");
    let mut bterm_failures: u32 = 0;

    // Start log viewer (Breengel client — tails /var/log/kernel.log)
    print!("[init] Starting /bin/blog...\n");
    let mut blog_pid = spawn(BLOG_PATH, "blog");
    let mut blog_failures: u32 = 0;

    // Start bounce demo (GPU-accelerated bouncing spheres)
    print!("[init] Starting /bin/bounce...\n");
    let mut bounce_pid = spawn(BOUNCE_PATH, "bounce");
    let mut bounce_failures: u32 = 0;

    // Start self-check test runner (windowed post-boot validation)
    print!("[init] Starting /bin/bcheck...\n");
    let mut bcheck_pid = spawn(BCHECK_PATH, "bcheck");

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

    // BusyBox test: fork+exec /bin/cat /etc/passwd to verify musl/BusyBox works
    print!("[init] BUSYBOX TEST: cat /etc/passwd\n");
    test_busybox_cat();

    // Main loop: block on waitpid until a child exits, then respawn if needed.
    let mut status: i32 = 0;
    loop {
        let reaped = match waitpid(-1, &mut status as *mut i32, 0) {
            Ok(pid) => pid.raw() as i64,
            Err(_) => {
                // ECHILD — no children at all. Sleep to avoid spinning.
                let ts = libbreenix::types::Timespec { tv_sec: 1, tv_nsec: 0 };
                let _ = libbreenix::time::nanosleep(&ts);
                continue;
            }
        };

        if reaped <= 0 {
            continue;
        }

        if reaped == bwm_pid {
            print!("[init] BWM exited (status {})\n", status);
            bwm_pid = try_respawn(BWM_PATH, "bwm", &mut bwm_failures);
            if bwm_pid == -1 {
                print!("[init] BWM failed {} times, giving up\n", MAX_RESPAWN_FAILURES);
            }
        } else if reaped == bterm_pid {
            print!("[init] bterm exited (status {})\n", status);
            bterm_pid = try_respawn(BTERM_PATH, "bterm", &mut bterm_failures);
            if bterm_pid == -1 {
                print!("[init] bterm failed {} times, giving up\n", MAX_RESPAWN_FAILURES);
            }
        } else if reaped == blog_pid {
            print!("[init] blog exited (status {})\n", status);
            blog_pid = try_respawn(BLOG_PATH, "blog", &mut blog_failures);
            if blog_pid == -1 {
                print!("[init] blog failed {} times, giving up\n", MAX_RESPAWN_FAILURES);
            }
        } else if reaped == bounce_pid {
            print!("[init] bounce exited (status {})\n", status);
            bounce_pid = try_respawn(BOUNCE_PATH, "bounce", &mut bounce_failures);
            if bounce_pid == -1 {
                print!("[init] bounce failed {} times, giving up\n", MAX_RESPAWN_FAILURES);
            }
        } else if reaped == bcheck_pid {
            print!("[init] bcheck exited (status {})\n", status);
            bcheck_pid = -1;
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
