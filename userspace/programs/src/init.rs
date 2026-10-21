//! Breenix init process (/sbin/init) - std version
//!
//! PID 1 - runs the boot checks and the boot script, then supervises the services in
//! `/etc/breenix/services` (`init_services.rs`): bsshd, the gettys on x86 and the rest.
//! Init confers the init-shell role by passing `--init-shell`; no PID value is part of that
//! contract.

mod init_services;

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
    if std::env::args().nth(1).as_deref() == Some("service-io-check") {
        init_services::check_service_io();
        return;
    }
    // The service table runs the loopback SSH check as `init bssh-oracle`, a child that
    // must not repeat PID 1's boot work or its start-up line.
    if std::env::args().nth(1).as_deref() == Some("bssh-oracle") {
        run_bssh_pubkey_oracle();
        return;
    }
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    print!("[init] Breenix init starting (PID {})\n", pid);
    // Emptying /tmp and /run and reconciling /etc is boot work, done by the system's init
    // alone. An init started as a child (the console suite runs `init console`) shares the
    // running system's /tmp, /run and /etc, so it leaves them as they are.
    if pid == 1 {
        match libbreenix::etc_reconcile::prepare(std::path::Path::new("/")) {
            Ok(conflicts) => for name in conflicts {
                println!("[etc-reconcile] kept local /etc/{}; wrote .factory sibling", name);
            },
            Err(why) => println!("[init] layout preparation failed; continuing boot: {}", why),
        }
    }

    // Explicit shell, console and desktop modes bypass the full startup below.
    // No argument or `default` keeps the services and boot checks.
    match std::env::args().nth(1).as_deref() {
        Some("suite") => {
            use std::os::unix::process::CommandExt;
            let mut args = std::env::args().skip(2);
            let path = args.next().expect("suite path required");
            let error = std::process::Command::new(path).args(args).exec();
            panic!("[init] suite exec failed: {}", error);
        }
        Some("shell") => run_shell_mode(),
        Some("console") => run_console_mode(),
        Some("desktop") => run_desktop_mode(),
        _ => {}
    }

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
    #[cfg(target_arch = "x86_64")]
    run_tty_oracle();
    #[cfg(target_arch = "x86_64")]
    run_exec_smoke();
    #[cfg(target_arch = "x86_64")]
    run_fork_smoke();
    // Detach before starting daemons so the getty handoff cannot hang them up.
    #[cfg(target_arch = "x86_64")]
    detach_console();
    reconcile_machine_state();
    run_boot_script();
    // The services start on the supervisor's first pass; the rest of the ARM64 start-up
    // follows it and does not wait on a service.
    let mut supervisor = init_services::Supervisor::load(init_services::Scope::All, pid == 1);
    supervisor.tick();
    #[cfg(target_arch = "aarch64")]
    start_bounce();
    #[cfg(target_arch = "aarch64")]
    run_bssh_autorun_if_enabled();
    #[cfg(target_arch = "aarch64")]
    run_init_group_refusal_probe("quiesce");
    supervisor.run()
}

/// Boot modes (`init shell`, `init desktop`): reap one child, blocking. Returns its PID,
/// or None after a one-second pause when waitpid fails (no children yet).
fn reap_one() -> Option<i32> {
    let mut status: i32 = 0;
    match waitpid(-1, &mut status as *mut i32, 0) {
        Ok(pid) => {
            let sig = status & 0x7F;
            let exit_code = (status >> 8) & 0xFF;
            if sig != 0 {
                print!("[init] Process {} killed by signal {}\n", pid.raw(), sig);
            } else {
                print!("[init] Process {} exited (code {})\n", pid.raw(), exit_code);
            }
            Some(pid.raw() as i32)
        }
        Err(_) => {
            sleep_one_second();
            None
        }
    }
}

fn reap_forever() -> ! {
    loop {
        reap_one();
    }
}

fn sleep_one_second() {
    let ts = libbreenix::types::Timespec {
        tv_sec: 1,
        tv_nsec: 0,
    };
    let _ = libbreenix::time::nanosleep(&ts);
}

/// `init shell`: an interactive bsh on the console, respawned whenever it exits. No
/// services, oracles or smoke tests.
fn run_shell_mode() -> ! {
    print!("[init] mode shell\n");
    print!("[init] shell ready\n");
    loop {
        match spawn(b"/bin/bsh\0") {
            Ok(shell) => {
                let shell = shell.raw() as i32;
                while reap_one() != Some(shell) {}
                print!("[init] shell exited; respawning\n");
            }
            Err(e) => {
                print!("[init] Failed to spawn /bin/bsh: {}\n", e);
            }
        }
        // A shell that dies at once must not turn this loop into a spin.
        sleep_one_second();
    }
}

/// Explicit console mode skips the default boot script and boot checks, and supervises
/// only the table's login terminals.
fn run_console_mode() -> ! {
    #[cfg(target_arch = "x86_64")]
    console_text_mode();
    detach_console();
    let pid = getpid().map(|p| p.raw()).unwrap_or(0);
    init_services::Supervisor::load(init_services::Scope::Logins, pid == 1).run()
}

/// Release init's terminal before starting daemons or independent login sessions.
fn detach_console() {
    use libbreenix::signal::{self, Sigaction};
    extern "C" { fn ioctl(fd: i32, request: u64, arg: u64) -> i32; }
    let ignored = Sigaction { handler: signal::SIG_IGN, ..Sigaction::default() };
    if let Err(e) = signal::sigaction(signal::SIGHUP, Some(&ignored), None) {
        print!("[init] console detach failed: {}\n", e);
        reap_forever();
    }
    if unsafe { ioctl(0, 0x5422, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() != Some(25) {
        print!("[init] console detach failed\n");
        reap_forever();
    }
}

/// `init desktop`: ARM64 starts the desktop; x86 falls back to console gettys.
/// Neither explicit mode runs boot checks.
#[cfg(target_arch = "x86_64")]
fn run_desktop_mode() -> ! {
    print!("Desktop mode requires a GPU; using the framebuffer console.\n");
    run_console_mode()
}

#[cfg(target_arch = "aarch64")]
fn run_desktop_mode() -> ! {
    print!("[init] mode desktop\n");
    const DESKTOP: &[&[u8]] = &[b"/usr/bin/bwm\0", b"/usr/bin/bterm\0"];
    let mut started = 0;
    for path in DESKTOP {
        match spawn(path) {
            Ok(_) => started += 1,
            Err(e) => print!("[init] Warning: failed to spawn desktop service: {}\n", e),
        }
    }
    // The boot-path marker claims the desktop came up, so it prints only when it did.
    if started == DESKTOP.len() {
        print!("[init] desktop services started\n");
    } else {
        print!("[init] desktop services incomplete: {} of {} started\n", started, DESKTOP.len());
    }
    reap_forever();
}

/// Run the #575 block-EINTR oracle first: its marker is a hard gate condition on every
/// `boot_tests` boot, and #589/#576 intercept init later in the sequence, so nothing
/// already-filed may preempt it.
#[cfg(target_arch = "aarch64")]
fn run_block_eintr_oracle() {
    match spawn(b"/usr/bin/block_eintr_oracle\0") {
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
    match spawn(b"/usr/bin/poll_tcp_oracle\0") {
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
///
/// Like `run_spawn_smoke()`, `waitpid`'s `Result` is checked honestly: only a
/// genuine `Ok` reap prints the exited-record literal the gates pin on, and a
/// `waitpid` failure prints a distinct, differently-worded literal instead of
/// silently reusing the pre-zeroed `status` to fabricate `code=0` (review
/// finding B3: this launcher used to discard the `Result` with `let _ =`).
#[cfg(target_arch = "aarch64")]
fn run_tty_oracle() {
    match spawn(b"/usr/bin/tty_oracle\0") {
        Ok(child_pid) => {
            let mut status: i32 = 0;
            match waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0) {
                Ok(_) => {
                    let exit_code = (status >> 8) & 0xFF;
                    print!(
                        "[init] tty_oracle exited pid={} code={}\n",
                        child_pid.raw(),
                        exit_code
                    );
                }
                Err(e) => {
                    print!("[init] Warning: tty_oracle reap failed: {}\n", e);
                }
            }
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
    match spawn(b"/usr/bin/futex_handoff_oracle\0") {
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

/// Run the boot path's only execve caller, on both architectures (#721 re-admits the x86_64
/// call site alongside the pre-existing aarch64 one). The aarch64 exec gate asserts on the
/// launcher's post-wait marker, the target's success marker, and the kernel's first scheduler
/// commit marker; the x86_64 prod-profile gate asserts the same four userspace markers plus
/// the kernel-side EXEC_LOCK_ORDER receipt oracles (#721 K7).
fn run_exec_smoke() {
    match spawn(b"/usr/bin/exec_smoke\0") {
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
    match spawn(b"/usr/libexec/breenix/clonevm_exec_test\0") {
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
    let path = b"/usr/bin/btrace\0";
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
    let path = b"/usr/bin/wait_stress\0";
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
    let path = b"/usr/bin/bssh\0";
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

/// The boot script is done: give the console's text the screen, which ends the
/// boot splash (`KDSETMODE(KD_TEXT)` on tty1, `docs/console/contract.md` section 7).
fn console_text_mode() {
    extern "C" { fn ioctl(fd: i32, request: u64, arg: u64) -> i32; }
    const KDSETMODE: u64 = 0x4B3A;
    const KD_TEXT: u64 = 0;
    if unsafe { ioctl(0, KDSETMODE, KD_TEXT) } != 0 {
        print!("[init] KDSETMODE(KD_TEXT) on the console failed: {}\n", std::io::Error::last_os_error());
    }
}

fn run_boot_script() {
    #[cfg(target_arch = "aarch64")]
    {
        // Start background services before handing the splash to the console.
        const SERVICES: &[&[u8]] = &[b"/usr/bin/xhci_counters\0", b"/usr/sbin/telnetd\0"];
        for path in SERVICES {
            if let Err(e) = spawn(path) {
                print!("[init] Warning: failed to spawn service: {}\n", e);
            }
        }
        console_text_mode();
        // On ARM64 this stage marks services and KD_TEXT, before bwm is spawned.
        print!("[init] Boot script completed\n");
        // A graphical child may take the display after init's text-mode request.
        if let Err(e) = spawn(b"/usr/bin/bwm\0") {
            print!("[init] Warning: failed to spawn service: {}\n", e);
        }
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
            match waitpid(child_raw, &mut status as *mut i32, 0) {
                Ok(_) if libbreenix::process::wifexited(status)
                    && libbreenix::process::wexitstatus(status) == 0 => {
                    console_text_mode();
                    print!("[init] Boot script completed\n");
                }
                Ok(_) => print!("[init] Boot script failed with status {}\n", status),
                Err(e) => print!("[init] Boot script reap failed: {}\n", e),
            }
        }
        Err(e) => {
            print!("[init] Failed to spawn boot script: {}\n", e);
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn start_bounce() {
    match spawn(b"/usr/bin/bounce\0") {
        Ok(child_pid) => {
            print!("[init] bounce started (PID {})\n", child_pid.raw());
        }
        Err(_) => {
            print!("[init] Warning: failed to start bounce\n");
        }
    }
}

/// #713: exercise x86 spawn, execution, exit and reap before the boot-script chain.
/// The dedicated /usr/bin/spawn_smoke_target exits 0; it does not depend on BusyBox.
/// Wait for this child directly: the default x86 boot script enters the getty
/// supervisor, so it does not return to main's general reap loop.
///
/// Unlike this file's other fire-and-forget `let _ = waitpid(...)` call
/// sites, this one's own success literal is a gate pin claiming a genuine
/// reap ("the full create+exec+run+exit+reap path end to end") -- so the
/// `waitpid` return value is checked honestly: only an `Ok` reap prints the
/// exited-code literal the gate asserts on, and a `waitpid` failure (e.g.
/// the child was silently never registered as this process's child) prints
/// a distinct, differently-worded literal instead of silently reusing the
/// pre-zeroed `status` to fabricate "exited (code 0)".
#[cfg(target_arch = "x86_64")]
fn run_spawn_smoke() {
    match spawn(b"/usr/bin/spawn_smoke_target\0") {
        Ok(child_pid) => {
            let mut status: i32 = 0;
            match waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0) {
                Ok(_) => {
                    let exit_code = (status >> 8) & 0xFF;
                    print!("[init] spawn smoke: exited (code {})\n", exit_code);
                }
                Err(e) => {
                    print!("[init] Warning: spawn smoke reap failed: {}\n", e);
                }
            }
        }
        Err(e) => {
            print!("[init] Warning: failed to start spawn smoke: {}\n", e);
        }
    }
}

/// Run the green-program TTY oracle (x86 port). Identical body to the
/// aarch64 `run_tty_oracle()` above (aarch64-gated) -- setsid/ioctl/PTY/
/// termios are production-safe on x86, so the launcher itself needs no
/// divergent behavior, only the arch gate.
///
/// fork() is now ALSO production-safe on x86 (#745) -- this comment
/// previously claimed that already, which was compile-time presence, not a
/// runtime-proven fact (#745 precheck C13, correcting spec section 3.10 --
/// NOT C10, which is the TLS-registration item; mislabeled in round 1, fixed
/// per review round 2 M5): `/etc/init.js` never exercised
/// any of `bsh`'s three `fork()` call sites, and `sys_fork_with_parent_context`
/// unconditionally refused with `Cannot implement fork without testing
/// feature` in every zero-feature x86 build before #745 landed. It is now
/// proven by `run_fork_smoke()` below (arch-neutral, `fork_smoke.rs`) and by
/// `tty_oracle.rs`'s own arm 14 (`cloexec_exec`), re-admitted on x86 in the
/// same round -- see that file's #721/#745 comment.
/// claim-lint:ok: the observed effect of the refusal this replaced
/// (`[FORK_SMOKE:FORK_FAILED ENOMEM]`) is in the pre-fix negative control,
/// and the post-fix markers are in the passing run:
/// docs/planning/745-x86-fork/serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt
/// Placed after `run_spawn_smoke()` and strictly before `start_bsshd()`, so
/// the oracle stays fully independent of init's boot-script chain (#722)
/// and the production processes that already run sequentially before it
/// never overlap with bsshd's own ext2 reads (#728).
///
/// Like `run_spawn_smoke()`, `waitpid`'s `Result` is checked honestly: only
/// a genuine `Ok` reap prints the exited-record literal the gates pin on,
/// and a `waitpid` failure prints a distinct, differently-worded literal
/// instead of silently reusing the pre-zeroed `status` to fabricate
/// `code=0` (review finding B3).
#[cfg(target_arch = "x86_64")]
fn run_tty_oracle() {
    match spawn(b"/usr/bin/tty_oracle\0") {
        Ok(child_pid) => {
            let mut status: i32 = 0;
            match waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0) {
                Ok(_) => {
                    let exit_code = (status >> 8) & 0xFF;
                    print!(
                        "[init] tty_oracle exited pid={} code={}\n",
                        child_pid.raw(),
                        exit_code
                    );
                }
                Err(e) => {
                    print!("[init] Warning: tty_oracle reap failed: {}\n", e);
                }
            }
        }
        Err(error) => {
            print!("[init] Warning: failed to start tty_oracle: {}\n", error);
        }
    }
}

/// claim-lint:ok: the round's own negative control shows this launcher's
/// markers absent on the pre-fix kernel, docs/planning/745-x86-fork/serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt
/// Run the fork smoke test (#745) -- proves a real fork()+CoW+forced-
/// reschedule+exit+reap round trip works end to end in production. Placed
/// after `run_exec_smoke()` (this file's own launch order, not this
/// function's cfg block) and strictly before `start_bsshd()`, the same slot
/// convention `run_spawn_smoke()`/`run_tty_oracle()`/`run_exec_smoke()`
/// already occupy (#713/#721). Waited on directly rather than left to the
/// end-of-main() reap loop, mirroring `run_exec_smoke()`'s own rationale
/// (that loop does not run until after `run_boot_script()`'s much slower
/// chain completes, #722).
#[cfg(target_arch = "x86_64")]
fn run_fork_smoke() {
    match spawn(b"/usr/bin/fork_smoke\0") {
        Ok(child_pid) => {
            let mut status = 0i32;
            let _ = waitpid(child_pid.raw() as i32, &mut status as *mut i32, 0);
            let exit_code = (status >> 8) & 0xFF;
            print!("[FORK_SMOKE:LAUNCHER_EXIT code={}]\n", exit_code);
        }
        Err(error) => {
            print!("[FORK_SMOKE:SPAWN_FAILED {}]\n", error);
        }
    }
}

/// The loopback SSH oracle's client key: a test-only key pair generated when the
/// test image is built, authorized in that image only, for its test account and for
/// root. The persistent install carries neither half.
const TEST_IDENTITY: &str = "/etc/breenix/test-ssh/id_ed25519";
/// The test image's SSH account (scripts/create_ext2_disk.sh) and the start of the
/// `id` line its session prints.
const TEST_ACCOUNT: &[u8] = b"sshtest\0";
const TEST_ID: &str = "uid=2000(sshtest) gid=2000(sshtest)";

/// Run bssh against the local bsshd as `user`. Returns its exit status and its
/// standard output, which is also passed on to init's.
fn bssh_arm(user: &[u8], extra: &[&[u8]]) -> (i32, String) {
    use libbreenix::io;
    use libbreenix::types::Fd;
    let mut argv: Vec<*const u8> = vec![
        b"bssh\0".as_ptr(),
        b"127.0.0.1\0".as_ptr(),
        b"22\0".as_ptr(),
        user.as_ptr(),
        b"--identity\0".as_ptr(),
        b"/etc/breenix/test-ssh/id_ed25519\0".as_ptr(),
    ];
    argv.extend(extra.iter().map(|arg| arg.as_ptr()));
    argv.push(core::ptr::null());
    let Ok((read_end, write_end)) = io::pipe() else { return (-1, String::new()) };
    let pid = match libbreenix::process::fork() {
        Ok(libbreenix::process::ForkResult::Child) => {
            let _ = io::close(read_end);
            let _ = io::dup2(write_end, Fd::STDOUT);
            let _ = io::close(write_end);
            let _ = libbreenix::process::execv(b"/usr/bin/bssh\0", argv.as_ptr());
            libbreenix::process::exit(127);
        }
        Ok(libbreenix::process::ForkResult::Parent(pid)) => pid,
        Err(_) => {
            let _ = io::close(read_end);
            let _ = io::close(write_end);
            return (-1, String::new());
        }
    };
    let _ = io::close(write_end);
    let mut output = Vec::new();
    let mut buffer = [0u8; 512];
    while let Ok(n) = io::read(read_end, &mut buffer) {
        if n == 0 || output.len() > 64 * 1024 {
            break;
        }
        output.extend_from_slice(&buffer[..n]);
    }
    let _ = io::close(read_end);
    let _ = io::write(Fd::STDOUT, &output);
    let mut status = 0;
    let code = match waitpid(pid.raw() as i32, &mut status, 0) {
        Ok(_) if libbreenix::process::wifexited(status) => libbreenix::process::wexitstatus(status),
        _ => -1,
    };
    (code, String::from_utf8_lossy(&output).into_owned())
}

// Run the actual CLI arms and require a dedicated USERAUTH_FAILURE status (77)
// for each refusal: a forged signature, and root, whose authorized keys list the
// test key but whom bsshd refuses. The account arm's session prints `id`.
fn run_bssh_pubkey_oracle() {
    if !std::path::Path::new(TEST_IDENTITY).exists() {
        print!("[BSSH_PUBKEY_ORACLE:not-run:no test identity]\n");
        return;
    }
    let _ = libbreenix::time::sleep_ms(1000);
    let (right, _) = bssh_arm(TEST_ACCOUNT, &[b"--smoke\0"]);
    let (wrong, _) = bssh_arm(TEST_ACCOUNT, &[b"--publickey-wrong\0", b"--smoke\0"]);
    let (root, _) = bssh_arm(b"root\0", &[b"--smoke\0"]);
    let (id_status, id_output) = bssh_arm(TEST_ACCOUNT, &[b"--exec\0", b"id\0"]);
    let marker = if right == 0 && wrong == 77 {
        "[BSSH_PUBKEY_ORACLE:right=ok:wrong=refused:PASS]".to_string()
    } else {
        format!("[BSSH_PUBKEY_ORACLE:FAIL:right={}:wrong={}]", right, wrong)
    };
    let id_matches = id_output.lines().any(|line| line.trim_start().starts_with(TEST_ID));
    let account = if root == 77 && id_status == 0 && id_matches {
        "[BSSH_ACCOUNT_ORACLE:root=refused:id=sshtest:PASS]".to_string()
    } else {
        format!("[BSSH_ACCOUNT_ORACLE:FAIL:root={}:id-status={}:id-matches={}]", root, id_status, id_matches)
    };
    // The refused arms leave bsshd reporting their handshake failures, which can
    // interleave with ordinary stdout. Flush first, then write the markers with one
    // system call each, so the serial transcript carries each whole line whatever
    // bsshd has written so far (#1849).
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.flush();
    let _ = libbreenix::io::write(libbreenix::types::Fd::STDOUT, format!("\n{}\n", marker).as_bytes());
    let _ = libbreenix::io::write(libbreenix::types::Fd::STDOUT, format!("\n{}\n", account).as_bytes());
}

/// On a persistent install, bring the machine state store (SSH host keys,
/// account lines and staged authorized keys) into this generation's `/etc`
/// before bsshd starts. Images without a store skip this.
fn reconcile_machine_state() {
    use std::os::unix::fs::MetadataExt;
    use libbreenix::ssh::machine_state;
    let root = std::path::Path::new("/");
    let home_mounted = match (std::fs::metadata("/"), std::fs::metadata("/home")) {
        (Ok(root), Ok(home)) => root.dev() != home.dev(),
        _ => false,
    };
    match machine_state::reconcile(root, home_mounted) {
        Ok(None) => {}
        Ok(Some(report)) => {
            print!(
                "[init] machine state: accounts [{}], host key files [{}]\n",
                report.accounts.join(", "),
                report.host_keys.join(", ")
            );
            for (account, fingerprint) in &report.authorized {
                print!("[init] machine state: staged key {} authorized for {}\n", fingerprint, account);
            }
            for problem in &report.problems {
                print!("[init] machine state: {}\n", problem);
            }
        }
        Err(why) => print!("[init] machine state refused: {}\n", why),
    }
}

#[cfg(target_arch = "aarch64")]
fn start_liveness_service() {
    match spawn(b"/usr/bin/heartbeat\0") {
        Ok(child_pid) => {
            print!("[init] heartbeat started (PID {})\n", child_pid.raw());
        }
        Err(_) => {
            print!("[init] Warning: failed to start heartbeat\n");
        }
    }
}
