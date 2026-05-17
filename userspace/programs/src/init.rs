//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - runs bsh (no arguments), then starts background services and reaps zombies.
//! bsh detects it's the init shell (PID 2) and loads /etc/init.js.

#[cfg(target_arch = "aarch64")]
use libbreenix::fs;
use libbreenix::process::{getpid, spawn, waitpid};
#[cfg(target_arch = "aarch64")]
use libbreenix::process::{spawnv, yield_now};

fn main() {
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);

    #[cfg(target_arch = "aarch64")]
    run_wait_stress_if_enabled();
    run_boot_script();
    start_bsshd();
    #[cfg(target_arch = "aarch64")]
    start_bounce();

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
                let ts = libbreenix::types::Timespec {
                    tv_sec: 1,
                    tv_nsec: 0,
                };
                let _ = libbreenix::time::nanosleep(&ts);
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn run_wait_stress_if_enabled() {
    if fs::access("/etc/wait_stress.enabled", fs::F_OK).is_err() {
        return;
    }

    print!("[init] wait_stress enabled; starting 60s waitqueue stress\n");
    let path = b"/bin/wait_stress\0";
    let arg0 = b"wait_stress\0";
    let arg1 = b"60\0";
    let argv = [arg0.as_ptr(), arg1.as_ptr(), core::ptr::null()];

    match spawnv(path, argv.as_ptr()) {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!(
                "[init] wait_stress exited pid={} code={}\n",
                child_pid.raw(),
                exit_code
            );
        }
        Err(e) => {
            print!("[init] Warning: failed to start wait_stress: {}\n", e);
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
        // proof clear within the Parallels validation window. Start the
        // heartbeat first so scheduler liveness is visible even if BWM wedges
        // immediately after spawn.
        const SERVICES: &[&[u8]] = &[b"/bin/heartbeat\0", b"/bin/bwm\0", b"/sbin/telnetd\0"];
        for path in SERVICES {
            if let Err(e) = spawn(path) {
                print!("[init] Warning: failed to spawn service: {}\n", e);
            }
            let _ = yield_now();
            let ts = libbreenix::types::Timespec {
                tv_sec: 0,
                tv_nsec: 75_000_000,
            };
            let _ = libbreenix::time::nanosleep(&ts);
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

#[cfg(target_arch = "aarch64")]
fn start_bounce() {
    // Start the animated GUI demo last. It continuously presents frames, so
    // keeping it after the early service execs avoids overlapping AHCI-backed
    // ELF reads with active compositor traffic.
    let _ = yield_now();
    let ts = libbreenix::types::Timespec {
        tv_sec: 0,
        tv_nsec: 75_000_000,
    };
    let _ = libbreenix::time::nanosleep(&ts);
    match spawn(b"/bin/bounce\0") {
        Ok(child_pid) => {
            print!("[init] bounce started (PID {})\n", child_pid.raw());
        }
        Err(_) => {
            print!("[init] Warning: failed to start bounce\n");
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
