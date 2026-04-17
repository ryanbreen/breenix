//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - runs bsh (no arguments), then starts background services and reaps zombies.
//! bsh detects it's the init shell (PID 2) and loads /etc/init.js.

use libbreenix::process::{fork, execv, waitpid, getpid, ForkResult};

fn main() {
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);

    run_boot_script();
    start_bsshd();

    // Reap zombies forever
    let mut status: i32 = 0;
    loop {
        match waitpid(-1, &mut status as *mut i32, 0) {
            Ok(pid) => {
                let sig = status & 0x7F;
                let exit_code = (status >> 8) & 0xFF;
                if sig != 0 {
                    print!("[init] Process {} killed by signal {}\n", pid.raw(), sig);
                } else {
                    print!("[init] Process {} exited (code {})\n", pid.raw(), exit_code);
                }
            }
            Err(_) => {
                let ts = libbreenix::types::Timespec { tv_sec: 1, tv_nsec: 0 };
                let _ = libbreenix::time::nanosleep(&ts);
            }
        }
    }
}

fn run_boot_script() {
    match fork() {
        Ok(ForkResult::Child) => {
            let arg0 = b"bsh\0";
            let argv: [*const u8; 2] = [
                arg0.as_ptr(),
                core::ptr::null(),
            ];
            match execv(b"/bin/bsh\0", argv.as_ptr()) {
                Ok(_) => unreachable!(),
                Err(e) => {
                    print!("[init] Failed to exec bsh: {}\n", e);
                    std::process::exit(127);
                }
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let child_raw = child_pid.raw() as i32;
            let mut status: i32 = 0;
            let _ = waitpid(child_raw, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            if exit_code != 0 {
                print!("[init] Boot script exited with code {}\n", exit_code);
            } else {
                print!("[init] Boot script completed\n");
            }
        }
        Err(e) => {
            print!("[init] Failed to fork for boot script: {}\n", e);
        }
    }
}

fn start_bsshd() {
    // Start bsshd after the boot script to avoid overlapping early exec reads
    // against the AHCI-backed ext2 root during initial userspace bring-up.
    match fork() {
        Ok(ForkResult::Child) => {
            let arg0 = b"bsshd\0";
            let argv: [*const u8; 2] = [
                arg0.as_ptr(),
                core::ptr::null(),
            ];
            match execv(b"/bin/bsshd\0", argv.as_ptr()) {
                Ok(_) => unreachable!(),
                Err(_) => {
                    // bsshd not installed — silently exit
                    std::process::exit(0);
                }
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            print!("[init] bsshd started (PID {})\n", child_pid.raw());
        }
        Err(_) => {
            print!("[init] Warning: failed to start bsshd\n");
        }
    }
}
