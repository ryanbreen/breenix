//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - runs bsh as the init shell, then starts background services and reaps zombies.
//! Init confers the init-shell role by passing `--init-shell`; no PID value is part of that
//! contract.

#[cfg(target_arch = "aarch64")]
use libbreenix::fs;
use libbreenix::process::spawnv;
use libbreenix::process::{getpid, spawn, waitpid};

#[cfg(target_arch = "aarch64")]
const INIT_GROUP_PROBE_STACK_SIZE: usize = 4096;

#[cfg(target_arch = "aarch64")]
#[repr(align(16))]
struct InitGroupProbeStack([u8; INIT_GROUP_PROBE_STACK_SIZE]);

#[cfg(target_arch = "aarch64")]
static mut INIT_GROUP_PROBE_STACK_ONE: InitGroupProbeStack =
    InitGroupProbeStack([0; INIT_GROUP_PROBE_STACK_SIZE]);
#[cfg(target_arch = "aarch64")]
static mut INIT_GROUP_PROBE_STACK_TWO: InitGroupProbeStack =
    InitGroupProbeStack([0; INIT_GROUP_PROBE_STACK_SIZE]);

/// The refused clone's entry point. It exists only so a regression is loud: if the kernel
/// ever admits an init-group clone the child writes `[INIT_GROUP_CHILD_RAN]` and parks,
/// which every gate rejects. It deliberately does not exit -- init must stay legible when
/// the refusal is absent.
#[cfg(target_arch = "aarch64")]
extern "C" fn init_group_probe_child(_arg: *mut u8) -> ! {
    const CHILD_RAN: &[u8] = b"[INIT_GROUP_CHILD_RAN]\n";

    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") 64u64,
            inlateout("x0") 1u64 => _,
            in("x1") CHILD_RAN.as_ptr() as u64,
            in("x2") CHILD_RAN.len() as u64,
            options(nostack),
        );
        loop {
            core::arch::asm!(
                "svc #0",
                in("x8") 124u64,
                inlateout("x0") 0u64 => _,
                options(nostack),
            );
        }
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn init_group_probe_clone(stack: *mut u8, flags: u64) -> i64 {
    let stack_top = (stack as usize + INIT_GROUP_PROBE_STACK_SIZE) & !0xF;
    let ret: i64;
    core::arch::asm!(
        "svc #0",
        in("x8") 220u64,
        inlateout("x0") flags => ret,
        in("x1") stack_top as u64,
        in("x2") init_group_probe_child as usize as u64,
        in("x3") 0u64,
        in("x4") 0u64,
        options(nostack),
    );
    ret
}

/// The designated init cannot acquire `CLONE_VM` siblings; both probes must return
/// `-EINVAL` (-22). The kernel emits the process-map walk from the refusal arm, so the
/// "early" pair drives walks 1-2 and the "quiesce" pair drives walks 3-4 of every boot.
#[cfg(target_arch = "aarch64")]
fn run_init_group_refusal_probe(phase: &str) {
    const CLONE_VM: u64 = 0x100;
    const CLONE_FILES: u64 = 0x400;

    let ret_one = unsafe {
        init_group_probe_clone(
            core::ptr::addr_of_mut!(INIT_GROUP_PROBE_STACK_ONE.0).cast::<u8>(),
            CLONE_VM,
        )
    };
    let ret_two = unsafe {
        init_group_probe_clone(
            core::ptr::addr_of_mut!(INIT_GROUP_PROBE_STACK_TWO.0).cast::<u8>(),
            CLONE_VM | CLONE_FILES,
        )
    };
    print!(
        "[INIT_GROUP_REFUSAL:aarch64:phase={}:probe1={}:probe2={}:expected=-22]\n",
        phase, ret_one, ret_two
    );
}

fn main() {
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);

    // The boot gates accept on the liveness service's marker: spawn it before the
    // exec smoke so gate acceptance never sits behind a spawn+exec+wait round trip.
    #[cfg(target_arch = "aarch64")]
    start_liveness_service();
    #[cfg(target_arch = "aarch64")]
    run_init_group_refusal_probe("early");
    #[cfg(target_arch = "aarch64")]
    run_block_eintr_oracle();
    #[cfg(target_arch = "aarch64")]
    run_futex_handoff_oracle();
    #[cfg(target_arch = "aarch64")]
    run_poll_tcp_oracle();
    #[cfg(target_arch = "aarch64")]
    run_tty_oracle();
    #[cfg(target_arch = "aarch64")]
    run_exec_smoke();
    #[cfg(target_arch = "aarch64")]
    run_clonevm_exec_test();
    #[cfg(target_arch = "aarch64")]
    run_wait_stress_if_enabled();
    #[cfg(target_arch = "aarch64")]
    run_trace_diag_probe_if_enabled();
    #[cfg(target_arch = "x86_64")]
    run_spawn_smoke();
    start_bsshd();
    run_boot_script();
    #[cfg(target_arch = "aarch64")]
    start_bounce();
    #[cfg(target_arch = "aarch64")]
    run_bssh_autorun_if_enabled();
    #[cfg(target_arch = "aarch64")]
    run_init_group_refusal_probe("quiesce");

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

/// Run the #575 block-EINTR oracle first: its marker is a hard gate condition on every
/// `boot_tests` boot, and #589/#576 intercept init later in the sequence, so nothing
/// already-filed may preempt it.
#[cfg(target_arch = "aarch64")]
fn run_block_eintr_oracle() {
    match spawn(b"/bin/block_eintr_oracle\0") {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!(
                "[init] block_eintr_oracle exited pid={} code={}\n",
                child_pid.raw(),
                exit_code
            );
        }
        Err(error) => {
            print!(
                "[init] Warning: failed to start block_eintr_oracle: {}\n",
                error
            );
        }
    }
}

/// Run the #568 blocking-poll-on-connected-TCP oracle. Nothing else in any boot
/// profile blocks in `poll()` on an `FdKind::TcpConnection`, so without this the
/// path #568 was filed against is never executed. The child prints its own
/// verdict marker; this launcher records the exit for boot-log debuggability.
#[cfg(target_arch = "aarch64")]
fn run_poll_tcp_oracle() {
    match spawn(b"/bin/poll_tcp_oracle\0") {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!(
                "[init] poll_tcp_oracle exited pid={} code={}\n",
                child_pid.raw(),
                exit_code
            );
        }
        Err(error) => {
            print!(
                "[init] Warning: failed to start poll_tcp_oracle: {}\n",
                error
            );
        }
    }
}

/// Run the green-program TTY oracle. Every other TTY proof in the tree lives in the
/// kernel's `boot_tests` registry, so it measures a kernel that is not the one shipped;
/// launching the oracle from init puts the PTY, line-discipline and termios surface on
/// the production profile's own boot. The child prints its own arm verdicts.
#[cfg(target_arch = "aarch64")]
fn run_tty_oracle() {
    match spawn(b"/bin/tty_oracle\0") {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!(
                "[init] tty_oracle exited pid={} code={}\n",
                child_pid.raw(),
                exit_code
            );
        }
        Err(error) => {
            print!("[init] Warning: failed to start tty_oracle: {}\n", error);
        }
    }
}

/// Run the deterministic #584 futex handoff oracle. The kernel emits the verdict marker;
/// this launcher only records the child exit for boot-log debuggability.
#[cfg(target_arch = "aarch64")]
fn run_futex_handoff_oracle() {
    match spawn(b"/bin/futex_handoff_oracle\0") {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!(
                "[init] futex_handoff_oracle exited pid={} code={}\n",
                child_pid.raw(),
                exit_code
            );
        }
        Err(error) => {
            print!(
                "[init] Warning: failed to start futex_handoff_oracle: {}\n",
                error
            );
        }
    }
}

/// Run the boot path's only execve caller. The aarch64 exec gate asserts on the launcher's
/// post-wait marker, the target's success marker, and the kernel's first scheduler commit marker.
#[cfg(target_arch = "aarch64")]
fn run_exec_smoke() {
    match spawn(b"/bin/exec_smoke\0") {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!("[EXEC_SMOKE:LAUNCHER_EXIT code={}]\n", exit_code);
        }
        Err(error) => {
            print!("[EXEC_SMOKE:SPAWN_FAILED {}]\n", error);
        }
    }
}

/// Run the CLONE_VM exec oracle from init because the kernel ext2 test-binary loader is
/// `testing`-only while the aarch64 gates build `boot_tests`. Init is therefore the only
/// launch path present in the gate profile, which pins the program's own
/// `CLONEVM_EXEC_TEST: PASS` marker.
#[cfg(target_arch = "aarch64")]
fn run_clonevm_exec_test() {
    match spawn(b"/usr/local/test/bin/clonevm_exec_test\0") {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!(
                "[init] clonevm_exec_test exited pid={} code={}\n",
                child_pid.raw(),
                exit_code
            );
        }
        Err(error) => {
            print!(
                "[init] Warning: failed to start clonevm_exec_test: {}\n",
                error
            );
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
        const SERVICES: &[&[u8]] = &[b"/bin/xhci_counters\0", b"/bin/bwm\0", b"/sbin/telnetd\0"];
        for path in SERVICES {
            if let Err(e) = spawn(path) {
                print!("[init] Warning: failed to spawn service: {}\n", e);
            }
        }
        print!("[init] Boot script completed\n");
        return;
    }

    #[cfg(not(target_arch = "aarch64"))]
    let path = b"/bin/bsh\0";
    #[cfg(not(target_arch = "aarch64"))]
    let arg0 = b"bsh\0";
    #[cfg(not(target_arch = "aarch64"))]
    let arg1 = b"--init-shell\0";
    #[cfg(not(target_arch = "aarch64"))]
    let argv = [arg0.as_ptr(), arg1.as_ptr(), core::ptr::null()];
    #[cfg(not(target_arch = "aarch64"))]
    match spawnv(path, argv.as_ptr()) {
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

/// #713: a minimal, self-contained proof that spawn() actually creates,
/// execs, and lets init reap a child on x86 -- independent of
/// run_boot_script()'s own bsh/init.js chain, which drags in seven further,
/// previously-unaudited x86 spawns and is deliberately out of scope here
/// (see #722). /bin/spawn_smoke_target is a dedicated, always-built
/// userspace binary (userspace/programs/src/spawn_smoke_target.rs) that
/// exits 0 unconditionally -- deliberately NOT busybox's /bin/true, which
/// depends on a musl-cross toolchain that isn't guaranteed present in
/// every build environment this gate runs in. Spawned fire-and-forget:
/// this function does not wait on it directly, so the ordinary
/// `waitpid(-1, ...)` reap loop at the end of main() is what prints its
/// exit -- proving the full path (spawn -> exec -> run -> exit -> reap)
/// rather than just the spawn call succeeding.
#[cfg(target_arch = "x86_64")]
fn run_spawn_smoke() {
    if let Err(e) = spawn(b"/bin/spawn_smoke_target\0") {
        print!("[init] Warning: failed to start spawn smoke: {}\n", e);
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

#[cfg(target_arch = "aarch64")]
fn start_liveness_service() {
    match spawn(b"/bin/heartbeat\0") {
        Ok(child_pid) => {
            print!("[init] heartbeat started (PID {})\n", child_pid.raw());
        }
        Err(_) => {
            print!("[init] Warning: failed to start heartbeat\n");
        }
    }
}
