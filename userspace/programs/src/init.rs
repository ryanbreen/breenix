//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - runs bsh (no arguments), then starts background services and reaps zombies.
//! bsh detects it's the init shell (PID 2) and loads /etc/init.js.

#[cfg(target_arch = "aarch64")]
use libbreenix::fs;
#[cfg(target_arch = "aarch64")]
use libbreenix::process::spawnv;
use libbreenix::process::{getpid, spawn, waitpid};

fn main() {
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);

    #[cfg(target_arch = "aarch64")]
    run_wait_stress_if_enabled();
    #[cfg(target_arch = "aarch64")]
    run_trace_diag_probe_if_enabled();
    start_bsshd();
    run_boot_script();
    #[cfg(target_arch = "aarch64")]
    start_bounce();
    #[cfg(target_arch = "aarch64")]
    run_bssh_autorun_if_enabled();

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
fn run_trace_diag_probe_if_enabled() {
    if option_env!("BREENIX_TRACE_DIAG_EARLY") != Some("1") {
        return;
    }

    print!("[init] trace diag early probe enabled; running btrace\n");
    let path = b"/bin/btrace\0";
    let arg0 = b"btrace\0";
    let argv = [arg0.as_ptr(), core::ptr::null()];

    match spawnv(path, argv.as_ptr()) {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!(
                "[init] trace diag early probe exited pid={} code={}\n",
                child_pid.raw(),
                exit_code
            );
        }
        Err(e) => {
            print!("[init] Warning: failed to start trace diag probe: {}\n", e);
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

#[cfg(target_arch = "aarch64")]
fn run_bssh_autorun_if_enabled() {
    let build_enabled = option_env!("BREENIX_BSSH_AUTORUN") == Some("1");
    if !build_enabled || fs::access("/etc/bssh_autorun.enabled", fs::F_OK).is_err() {
        return;
    }

    print!("[init] bssh autorun enabled by build environment and /etc gate\n");

    let ts = libbreenix::types::Timespec {
        tv_sec: 15,
        tv_nsec: 0,
    };
    let _ = libbreenix::time::nanosleep(&ts);

    run_bssh_exec_autorun("10.0.1.210");
    run_bssh_exec_autorun("10.211.55.2");
}

#[cfg(target_arch = "aarch64")]
fn run_bssh_exec_autorun(host: &str) {
    let path = b"/bin/bssh\0";
    let arg0 = b"bssh\0";
    let port = b"22\0";
    let user = b"wrb\0";
    let auth = b"--publickey\0";
    let exec = b"--exec\0";
    let command = b"uname\0";

    let mut host_buf = [0u8; 32];
    let host_bytes = host.as_bytes();
    if host_bytes.len() + 1 > host_buf.len() {
        print!("[init] bssh autorun host too long: {}\n", host);
        return;
    }
    host_buf[..host_bytes.len()].copy_from_slice(host_bytes);

    let argv = [
        arg0.as_ptr(),
        host_buf.as_ptr(),
        port.as_ptr(),
        user.as_ptr(),
        auth.as_ptr(),
        exec.as_ptr(),
        command.as_ptr(),
        core::ptr::null(),
    ];

    print!("[init] bssh autorun starting host={}\n", host);
    match spawnv(path, argv.as_ptr()) {
        Ok(child_pid) => {
            print!(
                "[init] bssh autorun spawned host={} pid={}\n",
                host,
                child_pid.raw()
            );
        }
        Err(e) => {
            print!("[init] Warning: failed to start bssh autorun: {}\n", e);
        }
    }
}

fn run_boot_script() {
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 Parallels boots from AHCI. Mirror the boot script's service
        // sequence directly from init so the standard desktop services are
        // always started even before bsh runs init.js.
        const SERVICES: &[&[u8]] = &[
            b"/bin/heartbeat\0",
            b"/bin/xhci_counters\0",
            b"/bin/bwm\0",
            b"/sbin/telnetd\0",
        ];
        for path in SERVICES {
            if let Err(e) = spawn(path) {
                print!("[init] Warning: failed to spawn service: {}\n", e);
            }
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
    match spawn(b"/bin/bsshd\0") {
        Ok(child_pid) => {
            print!("[init] bsshd started (PID {})\n", child_pid.raw());
        }
        Err(_) => {
            print!("[init] Warning: failed to start bsshd\n");
        }
    }
}
