//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - runs bsh (no arguments), then starts background services and reaps zombies.
//! bsh detects it's the init shell (PID 2) and loads /etc/init.js.

use libbreenix::process::{spawn, waitpid, getpid, yield_now};

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
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 Parallels boots from AHCI. Loading the large bsh ELF during the
        // early single-CPU boot window can stall before init.js runs, so mirror
        // the boot script's service sequence directly from init. Start bwm
        // before network services so the compositor replaces the kernel VirGL
        // proof clear within the Parallels validation window.
        const SERVICES: &[&[u8]] = &[
            b"/bin/bwm\0",
            b"/sbin/telnetd\0",
        ];
        for path in SERVICES {
            if let Err(e) = spawn(path) {
                print!("[init] Warning: failed to spawn service: {}\n", e);
            }
            let _ = yield_now();
        }
        print!("[init] Boot script completed\n");
        return;
    }

    #[cfg(not(target_arch = "aarch64"))]
    match spawn(b"/bin/bsh\0") {
        Ok(child_pid) => {
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
            print!("[init] Failed to spawn boot script: {}\n", e);
        }
    }
}

fn start_bsshd() {
    // Start bsshd after the boot script to avoid overlapping early exec reads
    // against the AHCI-backed ext2 root during initial userspace bring-up.
    match spawn(b"/bin/bsshd\0") {
        Ok(child_pid) => {
            print!("[init] bsshd started (PID {})\n", child_pid.raw());
        }
        Err(_) => {
            print!("[init] Warning: failed to start bsshd\n");
        }
    }
}
