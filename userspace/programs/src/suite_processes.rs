//! Processes: fork, exec, wait and exit, process groups and sessions, user and group IDs,
//! resource limits and usage, and scheduling priorities, as POSIX specifies them.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! Every wait on another process inside a case is bounded, and stops early enough to
//! leave the case time to clean up and report before that limit. The processes a case
//! starts are killed when the case ends, and the runner, which a suite runs as PID 1,
//! kills and reaps any that remain before the next case, so no case depends on another.
//! Error assertions read the raw syscall return, so an errno the
//! library does not name is still reported. Library-level calls (getrlimit, setrlimit,
//! nice, getpgrp) are made the way a C library makes them: through prlimit64,
//! getpriority/setpriority and getpgid(0).
//!
//! The `wait` category's first twelve cases are the former waitpid suite's, unchanged.
use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::fs;
use libbreenix::io;
use libbreenix::memory::{self, MAP_ANONYMOUS, MAP_PRIVATE, MAP_SHARED, PROT_READ, PROT_WRITE};
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::signal::{
    self, Sigaction, StackT, SA_ONSTACK, SA_RESTART, SIGCHLD, SIGCONT, SIGHUP, SIGKILL, SIGSEGV,
    SIGQUIT, SIGSTOP, SIGTERM, SIGUSR1, SIGUSR2, SIGXCPU, SIGXFSZ, SIG_IGN,
};
use libbreenix::suite::{case, case_ms_left, category, check, fail, skip, suite, CaseError, CaseResult, Suite};
use libbreenix::syscall::raw;
use libbreenix::time;
use libbreenix::types::Fd;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};

static SIGNALS: AtomicUsize = AtomicUsize::new(0);
static HANDLER_STACK: AtomicUsize = AtomicUsize::new(0);

/// Wait until `pid`'s threads are all blocked. Each waiting parent here does
/// nothing between fork and its waitpid, so that block is the wait.
fn wait_until_parked(pid: i32) {
    let path = format!("/proc/{pid}/status");
    while !std::fs::read_to_string(&path)
        .is_ok_and(|status| status.lines().any(|line| line == "State:\tBlocked"))
    {
        let _ = process::yield_now();
    }
}

extern "C" fn child_exited(_: i32) {
    let local = 0u8;
    // Taking a volatile address ensures this is storage on the handler's stack.
    unsafe { core::ptr::read_volatile(&local) };
    HANDLER_STACK.store(core::ptr::addr_of!(local) as usize, Ordering::Relaxed);
    SIGNALS.fetch_add(1, Ordering::Relaxed);
}

fn cow_signal_stack() -> CaseResult {
    let stack = memory::mmap(
        core::ptr::null_mut(), 16384, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0,
    )?;
    // Populate before fork; the parent leaves these pages untouched until delivery.
    // The top sits 128 bytes into a page, so the saved frame crosses two pages.
    unsafe { core::ptr::write_bytes(stack, 0xa5, 16384) };
    let alt = StackT { ss_sp: stack as u64, ss_flags: 0, _pad: 0, ss_size: 8320 };
    signal::sigaltstack(Some(&alt), None)?;
    let mut action = Sigaction::new(child_exited);
    action.flags |= SA_RESTART | SA_ONSTACK;
    signal::sigaction(SIGCHLD, Some(&action), None)?;
    // A second child keeps sharing the stack pages through delivery, so the
    // frame write must copy them, and checks that its own bytes stay intact.
    let (release_r, release_w) = io::pipe()?;
    let sharer = match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(release_w);
            let _ = io::read(release_r, &mut [0; 1]);
            let bytes = unsafe { core::slice::from_raw_parts(stack as *const u8, 16384) };
            process::exit(if bytes.iter().all(|&byte| byte == 0xa5) { 0 } else { 1 })
        }
        ForkResult::Parent(sharer) => sharer,
    };
    io::close(release_r)?;
    match process::fork()? {
        ForkResult::Child => process::exit(42),
        ForkResult::Parent(child) => {
            let mut status = 0;
            let waited = process::waitpid(child.raw() as i32, &mut status, 0)?;
            check(waited == child, "CoW signal stack: wrong child")?;
            check(process::wifexited(status) && process::wexitstatus(status) == 42,
                "CoW signal stack: wrong exit status")?;
            check(SIGNALS.load(Ordering::Relaxed) == 1, "CoW signal stack: missing handler")?;
            let local = HANDLER_STACK.load(Ordering::Relaxed);
            check(local >= stack as usize && local < stack as usize + 8320,
                "handler did not run on the alternate stack")?;
            // Validate the installed frame itself, including the saved mask on
            // the second page. These sizes are the architecture's signal ABI.
            #[cfg(target_arch = "aarch64")]
            let frame_size = 320;
            #[cfg(target_arch = "x86_64")]
            let frame_size = 192;
            let frame = (stack as usize + 8320 - frame_size) & !15;
            check(frame / 4096 != (frame + frame_size - 1) / 4096,
                "signal frame did not span two pages")?;
            check(unsafe { core::ptr::read_volatile((frame + 8) as *const u64) }
                == 0xDEAD_BEEF_CAFE_BABE, "signal frame magic missing")?;
            check(unsafe { core::ptr::read_volatile((frame + frame_size - 8) as *const u64) }
                == 0, "saved signal mask missing on second page")?;
        }
    }
    io::write(release_w, b"r")?;
    io::close(release_w)?;
    let mut status = 0;
    check(process::waitpid(sharer.raw() as i32, &mut status, 0)? == sharer,
        "CoW signal stack: wrong sharer")?;
    check(process::wifexited(status) && process::wexitstatus(status) == 0,
        "signal frame write changed the other sharer's stack")?;
    signal::sigaltstack(Some(&StackT::default()), None)?;
    memory::munmap(stack, 16384)?;
    Ok(())
}

fn repeat_wait(any_child: bool, caught: bool, delayed: bool) -> CaseResult {
    if caught {
        let mut action = Sigaction::new(child_exited);
        action.flags |= SA_RESTART;
        signal::sigaction(SIGCHLD, Some(&action), None)?;
    }
    let count = if delayed { 1 } else { 256 };
    let parent = process::getpid()?.raw() as i32;
    for iteration in 0..count {
        match process::fork()? {
            ForkResult::Child => {
                if delayed { wait_until_parked(parent); }
                process::exit(42)
            }
            ForkResult::Parent(child) => {
                let mut status = 0;
                let waited = process::waitpid(
                    if any_child { -1 } else { child.raw() as i32 }, &mut status, 0,
                )?;
                check(waited == child, &format!("iteration {iteration}: wrong child"))?;
                check(process::wifexited(status) && process::wexitstatus(status) == 42,
                    &format!("iteration {iteration}: wrong exit status"))?;
                if caught {
                    check(SIGNALS.load(Ordering::Relaxed) == iteration + 1,
                        &format!("iteration {iteration}: missing SIGCHLD"))?;
                }
            }
        }
    }
    Ok(())
}

fn specific() -> CaseResult { repeat_wait(false, false, false) }
fn any() -> CaseResult { repeat_wait(true, false, false) }
fn specific_signal() -> CaseResult { repeat_wait(false, true, false) }
fn any_signal() -> CaseResult { repeat_wait(true, true, false) }
fn parked_specific() -> CaseResult { repeat_wait(false, false, true) }
fn parked_any() -> CaseResult { repeat_wait(true, false, true) }
fn parked_specific_signal() -> CaseResult { repeat_wait(false, true, true) }
fn parked_any_signal() -> CaseResult { repeat_wait(true, true, true) }

fn nohang() -> CaseResult {
    // The child exits only once the first WNOHANG has seen it running.
    let (release_r, release_w) = io::pipe()?;
    match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(release_w);
            let _ = io::read(release_r, &mut [0; 1]);
            process::exit(42)
        }
        ForkResult::Parent(child) => {
            io::close(release_r)?;
            let mut status = 0;
            let running = process::waitpid(child.raw() as i32, &mut status, WNOHANG);
            io::close(release_w)?;
            check(running?.raw() == 0, "WNOHANG returned a running child")?;
            let start = time::now_monotonic()?.as_nanos();
            loop {
                let waited = process::waitpid(child.raw() as i32, &mut status, WNOHANG)?;
                if waited.raw() != 0 {
                    check(waited == child && process::wifexited(status)
                        && process::wexitstatus(status) == 42, "WNOHANG returned wrong status")?;
                    break;
                }
                check(time::now_monotonic()?.as_nanos() - start < 1_000_000_000,
                    "WNOHANG never reaped exited child")?;
                process::yield_now()?;
            }
        }
    }
    Ok(())
}

fn interrupted() -> CaseResult {
    signal::sigaction(SIGCHLD, Some(&Sigaction::new(child_exited)), None)?;
    let parent = process::getpid()?;
    // The signal is sent once the parent is parked in waitpid, and the child
    // exits only after the interrupted wait has returned.
    let (release_r, release_w) = io::pipe()?;
    match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(release_w);
            wait_until_parked(parent.raw() as i32);
            signal::kill(parent.raw() as i32, SIGCHLD)?;
            let _ = io::read(release_r, &mut [0; 1]);
            process::exit(42)
        }
        ForkResult::Parent(child) => {
            io::close(release_r)?;
            let mut status = 0;
            check(matches!(process::waitpid(child.raw() as i32, &mut status, 0),
                Err(Error::Os(Errno::EINTR))), "caught SIGCHLD did not interrupt waitpid")?;
            check(SIGNALS.load(Ordering::Relaxed) == 1, "interrupting handler missing")?;
            io::close(release_w)?;
            let waited = process::waitpid(child.raw() as i32, &mut status, 0)?;
            check(waited == child && process::wifexited(status)
                && process::wexitstatus(status) == 42, "wait after EINTR failed")?;
        }
    }
    Ok(())
}

fn stack_trampoline() -> CaseResult {
    let mut action = Sigaction::new_without_restorer(child_exited);
    action.flags |= SA_RESTART;
    signal::sigaction(SIGCHLD, Some(&action), None)?;
    match process::fork()? {
        ForkResult::Child => process::exit(42),
        ForkResult::Parent(child) => {
            let mut status = 0;
            check(process::waitpid(child.raw() as i32, &mut status, 0)? == child,
                "stack trampoline wait failed")?;
            check(SIGNALS.load(Ordering::Relaxed) == 1 && process::wifexited(status)
                && process::wexitstatus(status) == 42, "stack trampoline did not return")?;
        }
    }
    Ok(())
}


// ---------------------------------------------------------------------------
// Everything below is the processes effort's own cases and the helpers they share.

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const WRITE: u64 = 1;
    pub const DUP2: u64 = 33;
    pub const FCNTL: u64 = 72;
    pub const KILL: u64 = 62;
    pub const FORK: u64 = 57;
    pub const EXECVE: u64 = 59;
    pub const WAIT4: u64 = 61;
    pub const WAITID: u64 = 247;
    pub const GETPID: u64 = 39;
    pub const GETPPID: u64 = 110;
    pub const GETPGID: u64 = 121;
    pub const SETPGID: u64 = 109;
    pub const SETSID: u64 = 112;
    pub const GETSID: u64 = 124;
    pub const GETUID: u64 = 102;
    pub const GETEUID: u64 = 107;
    pub const GETGID: u64 = 104;
    pub const GETEGID: u64 = 108;
    pub const SETUID: u64 = 105;
    pub const SETGID: u64 = 106;
    pub const SETREUID: u64 = 113;
    pub const SETREGID: u64 = 114;
    pub const GETGROUPS: u64 = 115;
    pub const SETGROUPS: u64 = 116;
    pub const PRLIMIT64: u64 = 302;
    pub const GETRUSAGE: u64 = 98;
    pub const TIMES: u64 = 100;
    pub const GETPRIORITY: u64 = 140;
    pub const SETPRIORITY: u64 = 141;
    pub const SCHED_YIELD: u64 = 24;
    pub const UMASK: u64 = 95;
    pub const CHDIR: u64 = 80;
    pub const GETCWD: u64 = 79;
    pub const FCHMODAT: u64 = 268;
    pub const FCHOWNAT: u64 = 260;
    pub const MKDIRAT: u64 = 258;
    pub const UNLINKAT: u64 = 263;
    pub const SETITIMER: u64 = 38;
    pub const GETITIMER: u64 = 36;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const WRITE: u64 = 64;
    /// There is no dup2 here; a C library's dup2 calls dup3 with no flags.
    pub const DUP2: u64 = 24;
    pub const FCNTL: u64 = 25;
    pub const KILL: u64 = 129;
    /// There is no fork here; a C library's fork calls clone(SIGCHLD).
    pub const CLONE: u64 = 220;
    pub const EXECVE: u64 = 221;
    pub const WAIT4: u64 = 260;
    pub const WAITID: u64 = 95;
    pub const GETPID: u64 = 172;
    pub const GETPPID: u64 = 173;
    pub const GETPGID: u64 = 155;
    pub const SETPGID: u64 = 154;
    pub const SETSID: u64 = 157;
    pub const GETSID: u64 = 156;
    pub const GETUID: u64 = 174;
    pub const GETEUID: u64 = 175;
    pub const GETGID: u64 = 176;
    pub const GETEGID: u64 = 177;
    pub const SETUID: u64 = 146;
    pub const SETGID: u64 = 144;
    pub const SETREUID: u64 = 145;
    pub const SETREGID: u64 = 143;
    pub const GETGROUPS: u64 = 158;
    pub const SETGROUPS: u64 = 159;
    pub const PRLIMIT64: u64 = 261;
    pub const GETRUSAGE: u64 = 165;
    pub const TIMES: u64 = 153;
    pub const GETPRIORITY: u64 = 141;
    pub const SETPRIORITY: u64 = 140;
    pub const SCHED_YIELD: u64 = 124;
    pub const UMASK: u64 = 166;
    pub const CHDIR: u64 = 49;
    pub const GETCWD: u64 = 17;
    pub const FCHMODAT: u64 = 53;
    pub const FCHOWNAT: u64 = 54;
    pub const MKDIRAT: u64 = 34;
    pub const UNLINKAT: u64 = 35;
    pub const SETITIMER: u64 = 103;
    pub const GETITIMER: u64 = 102;
}

const EPERM: i64 = 1;
const F_GETFD: u64 = 1;
const ENOENT: i64 = 2;
const ESRCH: i64 = 3;
const E2BIG: i64 = 7;
const ENOEXEC: i64 = 8;
const EBADF: i64 = 9;
const ECHILD: i64 = 10;
const EAGAIN: i64 = 11;
const EACCES: i64 = 13;
const ENOTDIR: i64 = 20;
const EINVAL: i64 = 22;
const EFBIG: i64 = 27;
const ENAMETOOLONG: i64 = 36;

const AT_FDCWD: u64 = (-100i64) as u64;
const O_CLOEXEC: i32 = 0x80000;
const O_NONBLOCK: i64 = 0x800;
const WUNTRACED: i32 = 2;
const WCONTINUED: i32 = 8;
const KEEP: u32 = u32::MAX;
const RLIMIT_CPU: u32 = 0;
const RLIMIT_FSIZE: u32 = 1;
const RLIMIT_DATA: u32 = 2;
const RLIMIT_STACK: u32 = 3;
const RLIMIT_CORE: u32 = 4;
const RLIMIT_NPROC: u32 = 6;
const RLIMIT_NOFILE: u32 = 7;
const RLIMIT_AS: u32 = 9;
const RLIM_INFINITY: u64 = u64::MAX;
const PRIO_PROCESS: u64 = 0;
const PRIO_PGRP: u64 = 1;
const PRIO_USER: u64 = 2;

/// How long a case waits for another process to change state. A working kernel
/// takes milliseconds; the bound only keeps a failing case inside the runner's limit.
const WAIT_MS: u64 = 3000;
/// The same for an exec, which loads a program from disk.
const EXEC_MS: u64 = 6000;
/// What a case's waits leave of the runner's limit, for the case to clean up and report.
const CLEANUP_MS: u64 = 1500;
/// What cleanup's own waits leave, for the case to report.
const REPORT_MS: u64 = 300;
/// The helper the exec cases run (`processes_exec.rs`).
const HELPER: &str = "/usr/local/test/bin/processes-exec_test";
/// In an exec case's argument list, replaced by the descriptor the program reports on.
const FD_ARG: &str = "{fd}";

type Checked = Result<(), String>;

fn sc(n: u64, args: &[u64]) -> i64 {
    let a = |i: usize| args.get(i).copied().unwrap_or(0);
    // SAFETY: every caller keeps the buffers its arguments point to alive through the call.
    unsafe { raw::syscall6(n, a(0), a(1), a(2), a(3), a(4), a(5)) as i64 }
}

fn errname(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 7 => "E2BIG", 8 => "ENOEXEC",
        9 => "EBADF", 10 => "ECHILD", 11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES",
        14 => "EFAULT", 20 => "ENOTDIR", 21 => "EISDIR", 22 => "EINVAL", 24 => "EMFILE",
        27 => "EFBIG", 36 => "ENAMETOOLONG", 38 => "ENOSYS",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// A raw return as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if ret < 0 { errname(-ret) } else { ret.to_string() } }

fn err<T>(msg: impl Into<String>) -> Result<T, CaseError> { Err(CaseError::Fail(msg.into())) }

/// The raw return, or a failure naming the call.
fn want(what: &str, ret: i64) -> Result<i64, CaseError> {
    if ret < 0 { err(format!("{what} failed with {}", errname(-ret))) } else { Ok(ret) }
}

fn want_eq(what: &str, got: i64, expected: i64) -> CaseResult {
    check(got == expected, &format!("{what}: expected {expected}, got {}", shown(got)))
}

fn want_err(what: &str, got: i64, errno: i64) -> CaseResult {
    check(got == -errno, &format!("{what}: expected {}, got {}", errname(errno), shown(got)))
}

fn cpath(path: &str) -> CString { CString::new(path).expect("path without NUL") }

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }
fn ppid() -> i32 { sc(nr::GETPPID, &[]) as i32 }
fn kill(pid: i32, sig: i32) -> i64 { sc(nr::KILL, &[pid as i64 as u64, sig as u64]) }
fn wait4(pid: i32, status: *mut i32, options: i32) -> i64 {
    sc(nr::WAIT4, &[pid as i64 as u64, status as u64, options as u64, 0])
}
fn getpgid(pid: i32) -> i64 { sc(nr::GETPGID, &[pid as i64 as u64]) }
fn getsid(pid: i32) -> i64 { sc(nr::GETSID, &[pid as i64 as u64]) }
fn setpgid(pid: i32, pgid: i32) -> i64 { sc(nr::SETPGID, &[pid as i64 as u64, pgid as i64 as u64]) }
fn setsid() -> i64 { sc(nr::SETSID, &[]) }
fn umask(mask: u32) -> i64 { sc(nr::UMASK, &[mask as u64]) }
fn chdir(path: &str) -> i64 { let c = cpath(path); sc(nr::CHDIR, &[c.as_ptr() as u64]) }
fn chmod(path: &str, mode: u32) -> i64 {
    let c = cpath(path);
    sc(nr::FCHMODAT, &[AT_FDCWD, c.as_ptr() as u64, mode as u64, 0])
}
fn chown(path: &str, uid: u32, gid: u32) -> i64 {
    let c = cpath(path);
    sc(nr::FCHOWNAT, &[AT_FDCWD, c.as_ptr() as u64, uid as u64, gid as u64, 0])
}
fn cwd() -> Result<String, String> {
    let mut buf = [0u8; 512];
    let r = sc(nr::GETCWD, &[buf.as_mut_ptr() as u64, buf.len() as u64]);
    if r < 0 { return Err(format!("getcwd failed with {}", errname(-r))); }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
    Ok(String::from_utf8_lossy(&buf[..len]).into_owned())
}

/// fork as a C library makes it: the fork call, or clone(SIGCHLD) where there is none.
fn raw_fork() -> i64 {
    #[cfg(target_arch = "x86_64")]
    { sc(nr::FORK, &[]) }
    #[cfg(target_arch = "aarch64")]
    { sc(nr::CLONE, &[SIGCHLD as u64, 0, 0, 0, 0]) }
}

// Wait statuses, decoded as a C library's macros decode them.
fn exited(s: i32) -> bool { s & 0x7f == 0 }
fn exit_code(s: i32) -> i32 { (s >> 8) & 0xff }
fn signaled(s: i32) -> bool { (((s & 0x7f) + 1) as i8 >> 1) > 0 }
fn term_sig(s: i32) -> i32 { s & 0x7f }
fn stopped(s: i32) -> bool { s & 0xff == 0x7f }
fn stop_sig(s: i32) -> i32 { (s >> 8) & 0xff }
fn continued(s: i32) -> bool { s == 0xffff }
fn status_text(s: i32) -> String {
    if continued(s) { "continued".into() }
    else if stopped(s) { format!("stopped by signal {}", stop_sig(s)) }
    else if exited(s) { format!("exit {}", exit_code(s)) }
    else if signaled(s) { format!("death by signal {}", term_sig(s)) }
    else { format!("status {s:#x}") }
}

/// `ms`, cut short so that `reserve` ms of the case's limit remain afterwards.
fn bounded(ms: u64, reserve: u64) -> u64 { ms.min(case_ms_left().saturating_sub(reserve)) }

fn now_ms() -> u64 { time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0) }
fn nap() { let _ = time::sleep_ms(2); }

/// Use the CPU for `ms` milliseconds of wall-clock time.
fn burn(ms: u64) {
    let start = now_ms();
    let mut x = 1u64;
    while now_ms().saturating_sub(start) < ms {
        for i in 0..20_000u64 { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); }
        core::hint::black_box(x);
    }
}

/// Poll waitpid(pid, options | WNOHANG) until it reports a child, for at most `ms`.
fn wait_within(pid: i32, options: i32, ms: u64) -> Result<(i32, i32), String> {
    poll_wait(pid, options, bounded(ms, CLEANUP_MS))
}

fn poll_wait(pid: i32, options: i32, ms: u64) -> Result<(i32, i32), String> {
    let start = now_ms();
    loop {
        let mut status = 0;
        let r = wait4(pid, &mut status, options | WNOHANG);
        if r > 0 { return Ok((r as i32, status)); }
        if r < 0 { return Err(format!("waitpid({pid}) failed with {}", errname(-r))); }
        if now_ms().saturating_sub(start) >= ms {
            return Err(format!("waitpid({pid}) reported nothing within {ms} ms"));
        }
        nap();
    }
}

/// Read until end of file, for at most `ms`.
fn read_to_eof(fd: Fd, ms: u64) -> Result<Vec<u8>, String> { read_up_to(fd, usize::MAX, ms) }

/// Read until `want` bytes have arrived or end of file, for at most `ms`.
fn read_up_to(fd: Fd, want: usize, ms: u64) -> Result<Vec<u8>, String> {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65536];
    while out.len() < want {
        let left = ms.saturating_sub(now_ms().saturating_sub(start));
        if left == 0 { return Err(format!("no end of file within {ms} ms")); }
        let mut fds = [io::PollFd::new(fd, io::poll_events::POLLIN)];
        match io::poll(&mut fds, left.min(1000) as i32) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(e) => return Err(format!("poll failed: {e}")),
        }
        let room = (want - out.len()).min(buf.len());
        match io::read(fd, &mut buf[..room]) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) => return Err(format!("read failed: {e}")),
        }
    }
    Ok(out)
}

fn read_i32(fd: Fd, what: &str) -> Result<i32, CaseError> {
    let bytes = read_up_to(fd, 4, WAIT_MS)?;
    if bytes.len() != 4 { return err(format!("{what} was never reported")); }
    Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn write_all(fd: Fd, mut bytes: &[u8]) -> Result<(), Error> {
    while !bytes.is_empty() {
        let n = io::write(fd, bytes)?;
        bytes = &bytes[n..];
    }
    Ok(())
}

/// Block on `fd` until end of file; false if a read failed first.
fn drain(fd: Fd) -> bool {
    let mut b = [0u8; 16];
    loop {
        match io::read(fd, &mut b) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

/// A child process. Dropping it while it may still run kills and reaps it.
struct Child { pid: i32, live: bool }

impl Child {
    /// Fork; the child runs `f` and exits with what it returns.
    fn start(f: impl FnOnce() -> i32) -> Result<Child, CaseError> {
        match process::fork()? {
            ForkResult::Child => process::exit(f()),
            ForkResult::Parent(pid) => Ok(Child { pid: pid.raw() as i32, live: true }),
        }
    }

    /// Wait for the child to end; its wait status.
    fn wait(&mut self) -> Result<i32, CaseError> {
        let (_, status) = wait_within(self.pid, 0, WAIT_MS)?;
        self.live = false;
        Ok(status)
    }

    fn expect_exit(&mut self, code: i32, what: &str) -> CaseResult {
        let status = self.wait()?;
        check(exited(status) && exit_code(status) == code,
            &format!("{what} ended with {}, expected exit {code}", status_text(status)))
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.live {
            kill(self.pid, SIGKILL);
            let _ = poll_wait(self.pid, 0, bounded(1000, REPORT_MS));
        }
    }
}

/// A process that is not this one's child; dropping it kills it, and the runner, which
/// it has been reparented to, reaps it.
struct Stray(i32);

impl Drop for Stray {
    fn drop(&mut self) { kill(self.0, SIGKILL); }
}

/// A child running a check; `finish` fails with the message the child reported.
struct Task { child: Child, msg: Fd }

fn task(f: impl FnOnce() -> Checked) -> Result<Task, CaseError> {
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(r);
        match f() {
            Ok(()) => 0,
            Err(msg) => { let _ = io::write(w, msg.as_bytes()); 1 }
        }
    });
    let _ = io::close(w);
    match child {
        Ok(child) => Ok(Task { child, msg: r }),
        Err(e) => { let _ = io::close(r); Err(e) }
    }
}

impl Task {
    fn finish(mut self) -> CaseResult {
        let status = self.child.wait()?;
        if exited(status) && exit_code(status) == 0 { return Ok(()); }
        let said = read_up_to(self.msg, 512, 100).unwrap_or_default();
        if exited(status) && exit_code(status) == 1 && !said.is_empty() {
            return fail(format!("in the child: {}", String::from_utf8_lossy(&said)));
        }
        fail(format!("the child ended with {}", status_text(status)))
    }
}

impl Drop for Task {
    fn drop(&mut self) { let _ = io::close(self.msg); }
}

/// A child that ran `setup`, said so, and now waits to be released.
struct Held { child: Child, release: Option<Fd> }

fn held(setup: impl FnOnce() -> Checked) -> Result<Held, CaseError> {
    let (ready_r, ready_w) = io::pipe2(O_CLOEXEC)?;
    let (rel_r, rel_w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(ready_r);
        let _ = io::close(rel_w);
        let said = match setup() { Ok(()) => "r".to_string(), Err(msg) => format!("!{msg}") };
        let _ = io::write(ready_w, said.as_bytes());
        let _ = io::close(ready_w);
        if !drain(rel_r) { 2 } else if said == "r" { 0 } else { 1 }
    });
    let _ = io::close(ready_w);
    let _ = io::close(rel_r);
    let child = match child {
        Ok(child) => child,
        Err(e) => { let _ = io::close(ready_r); let _ = io::close(rel_w); return Err(e); }
    };
    let held = Held { child, release: Some(rel_w) };
    let said = read_to_eof(ready_r, WAIT_MS);
    let _ = io::close(ready_r);
    let said = said?;
    match said.first() {
        Some(b'r') => Ok(held),
        Some(b'!') => err(format!("in the child: {}", String::from_utf8_lossy(&said[1..]))),
        _ => err("the child never reported that it was ready"),
    }
}

impl Held {
    fn pid(&self) -> i32 { self.child.pid }

    fn release(mut self) -> CaseResult {
        if let Some(fd) = self.release.take() { let _ = io::close(fd); }
        self.child.expect_exit(0, "the held child")
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(fd) = self.release.take() { let _ = io::close(fd); }
    }
}

/// The PID of a child that has exited and been reaped, so names no process.
fn gone_pid() -> Result<i32, CaseError> {
    let mut child = Child::start(|| 0)?;
    child.expect_exit(0, "a short-lived child")?;
    Ok(child.pid)
}

/// A directory for one case's files, removed when the case ends.
struct Tmp { dir: String, paths: RefCell<Vec<String>> }

impl Tmp {
    fn new() -> Result<Tmp, CaseError> {
        let dir = format!("/tmp/processes-{}", pid());
        let c = cpath(&dir);
        want("mkdir", sc(nr::MKDIRAT, &[AT_FDCWD, c.as_ptr() as u64, 0o777]))?;
        want("chmod", chmod(&dir, 0o777))?;
        Ok(Tmp { dir, paths: RefCell::new(Vec::new()) })
    }

    fn path(&self, name: &str) -> String {
        let path = format!("{}/{name}", self.dir);
        self.paths.borrow_mut().push(path.clone());
        path
    }

    /// Create `name` holding `data`, then give it `mode` exactly.
    fn file(&self, name: &str, data: &[u8], mode: u32) -> Result<String, CaseError> {
        let path = self.path(name);
        let fd = fs::open_with_mode(&path, fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY, 0o600)?;
        let written = write_all(fd, data);
        io::close(fd)?;
        written?;
        want("chmod", chmod(&path, mode))?;
        Ok(path)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        for path in self.paths.borrow().iter() {
            let c = cpath(path);
            sc(nr::UNLINKAT, &[AT_FDCWD, c.as_ptr() as u64, 0]);
        }
        let c = cpath(&self.dir);
        sc(nr::UNLINKAT, &[AT_FDCWD, c.as_ptr() as u64, 0x200]);
    }
}

// Signals.
static HITS: AtomicUsize = AtomicUsize::new(0);
extern "C" fn count_signal(_: i32) { HITS.fetch_add(1, Ordering::Relaxed); }
fn handler_address() -> u64 { count_signal as extern "C" fn(i32) as usize as u64 }
fn catch(sig: i32) -> Result<(), Error> { signal::sigaction(sig, Some(&Sigaction::new(count_signal)), None) }
fn ignore(sig: i32) -> Result<(), Error> { signal::sigaction(sig, Some(&Sigaction::ignore()), None) }
fn handler_of(sig: i32) -> Result<u64, Error> {
    let mut old = Sigaction::default();
    signal::sigaction(sig, None, Some(&mut old))?;
    Ok(old.handler)
}
fn bit(sig: i32) -> u64 { 1 << (sig - 1) }
fn block(mask: u64) -> Result<(), Error> { signal::sigprocmask(signal::SIG_BLOCK, Some(&mask), None) }
fn blocked() -> Result<u64, Error> {
    let mut mask = 0;
    signal::sigprocmask(signal::SIG_BLOCK, None, Some(&mut mask))?;
    Ok(mask)
}
fn pending() -> Result<u64, Error> {
    let mut set = 0;
    signal::sigpending(&mut set)?;
    Ok(set)
}
/// ITIMER_REAL, the timer alarm() sets.
fn set_real_timer(secs: i64) -> i64 {
    let value = [0i64, 0, secs, 0];
    sc(nr::SETITIMER, &[0, value.as_ptr() as u64, 0])
}
fn real_timer_left() -> Result<(i64, i64), String> {
    let mut value = [0i64; 4];
    let r = sc(nr::GETITIMER, &[0, value.as_mut_ptr() as u64]);
    if r < 0 { Err(format!("getitimer failed with {}", errname(-r))) } else { Ok((value[2], value[3])) }
}

// User and group IDs.
fn ids() -> [i64; 4] {
    [sc(nr::GETUID, &[]), sc(nr::GETEUID, &[]), sc(nr::GETGID, &[]), sc(nr::GETEGID, &[])]
}
fn ids_are(expected: [i64; 4], what: &str) -> CaseResult {
    let got = ids();
    check(got == expected, &format!("{what}: real/effective user and group IDs are {got:?}, expected {expected:?}"))
}
fn setuid(uid: u32) -> i64 { sc(nr::SETUID, &[uid as u64]) }
fn setgid(gid: u32) -> i64 { sc(nr::SETGID, &[gid as u64]) }
fn setreuid(real: u32, effective: u32) -> i64 { sc(nr::SETREUID, &[real as u64, effective as u64]) }
fn setregid(real: u32, effective: u32) -> i64 { sc(nr::SETREGID, &[real as u64, effective as u64]) }
fn setgroups(list: &[u32]) -> i64 { sc(nr::SETGROUPS, &[list.len() as u64, list.as_ptr() as u64]) }
fn getgroups(list: &mut [u32]) -> i64 { sc(nr::GETGROUPS, &[list.len() as u64, list.as_mut_ptr() as u64]) }
/// Whether a getgroups list holds exactly the groups setgroups was given. POSIX leaves
/// their order, and whether the effective group ID `egid` is listed too, to the system.
fn groups_are(got: &[u32], set: &[u32], egid: u32) -> bool {
    set.iter().all(|g| got.contains(g)) && got.iter().all(|g| set.contains(g) || *g == egid)
}

/// Give up root for good: no supplementary groups, then the group ID, then the user ID.
fn become_user(uid: u32, gid: u32) -> Checked {
    for (what, r) in [("setgroups", setgroups(&[])), ("setgid", setgid(gid))] {
        if r != 0 { return Err(format!("{what} as root failed with {}", shown(r))); }
    }
    let r = setuid(uid);
    if r != 0 { return Err(format!("setuid as root failed with {}", shown(r))); }
    Ok(())
}

// Resource limits, read and set as a C library does: through prlimit64.
fn prlimit(pid: i32, resource: u32, new: Option<[u64; 2]>, old: Option<&mut [u64; 2]>) -> i64 {
    let new_ptr = new.as_ref().map_or(0, |n| n.as_ptr() as u64);
    let old_ptr = old.map_or(0, |o| o.as_mut_ptr() as u64);
    sc(nr::PRLIMIT64, &[pid as i64 as u64, resource as u64, new_ptr, old_ptr])
}
fn getrlimit(resource: u32) -> Result<[u64; 2], String> {
    let mut v = [0u64; 2];
    let r = prlimit(0, resource, None, Some(&mut v));
    if r < 0 { Err(format!("getrlimit failed with {}", errname(-r))) } else { Ok(v) }
}
fn setrlimit(resource: u32, soft: u64, hard: u64) -> i64 { prlimit(0, resource, Some([soft, hard]), None) }
fn limit_is(resource: u32, expected: [u64; 2], what: &str) -> Checked {
    let got = getrlimit(resource)?;
    if got == expected { Ok(()) } else { Err(format!("{what}: limit is {got:?}, expected {expected:?}")) }
}

fn getrusage(who: i64) -> Result<[i64; 18], String> {
    let mut usage = [0i64; 18];
    let r = sc(nr::GETRUSAGE, &[who as u64, usage.as_mut_ptr() as u64]);
    if r < 0 { Err(format!("getrusage failed with {}", errname(-r))) } else { Ok(usage) }
}
/// User plus system CPU time in a struct rusage, in microseconds.
fn cpu_us(usage: &[i64; 18]) -> i64 { (usage[0] + usage[2]) * 1_000_000 + usage[1] + usage[3] }
/// times(): the elapsed-time return and tms_utime, tms_stime, tms_cutime, tms_cstime.
fn times() -> Result<(i64, [i64; 4]), String> {
    let mut t = [0i64; 4];
    let r = sc(nr::TIMES, &[t.as_mut_ptr() as u64]);
    if (-4095..0).contains(&r) { Err(format!("times failed with {}", errname(-r))) } else { Ok((r, t)) }
}

// Priorities, as a C library reports them: the raw call returns 20 - nice.
fn getpriority(which: u64, who: i32) -> Result<i64, i64> {
    let r = sc(nr::GETPRIORITY, &[which, who as i64 as u64]);
    if r < 0 { Err(-r) } else { Ok(20 - r) }
}
fn setpriority(which: u64, who: i32, nice: i64) -> i64 { sc(nr::SETPRIORITY, &[which, who as i64 as u64, nice as u64]) }
fn nice_is(which: u64, who: i32, expected: i64, what: &str) -> CaseResult {
    match getpriority(which, who) {
        Ok(n) if n == expected => Ok(()),
        Ok(n) => fail(format!("{what}: nice value is {n}, expected {expected}")),
        Err(e) => fail(format!("{what}: getpriority failed with {}", errname(e))),
    }
}
/// nice() as glibc implements it: getpriority, add, setpriority, report EPERM where
/// setpriority says EACCES, and return the nice value getpriority then reads.
fn nice(inc: i64) -> Result<i64, i64> {
    let target = getpriority(PRIO_PROCESS, 0)? + inc;
    match setpriority(PRIO_PROCESS, 0, target) {
        0 => getpriority(PRIO_PROCESS, 0),
        r if r == -EACCES => Err(EPERM),
        r => Err(-r),
    }
}

// Exec.
struct CArgs { _owned: Vec<CString>, ptrs: Vec<*const u8> }

fn cargs(items: &[String]) -> CArgs {
    let owned: Vec<CString> = items.iter().map(|s| CString::new(s.as_str()).expect("argument without NUL")).collect();
    let mut ptrs: Vec<*const u8> = owned.iter().map(|c| c.as_ptr() as *const u8).collect();
    ptrs.push(core::ptr::null());
    CArgs { _owned: owned, ptrs }
}

fn strings(items: &[&str]) -> Vec<String> { items.iter().map(|s| s.to_string()).collect() }

/// Fork; the child runs `setup` and then execve(path, args, env). Returns the child and,
/// when the exec failed, its errno. The child reports on a close-on-exec pipe, so end of
/// file with nothing written means the exec replaced it.
fn spawn_exec(path: &str, args: &[String], env: &[String], setup: impl FnOnce() -> Checked)
    -> Result<(Child, Option<i64>), CaseError>
{
    let path = cpath(path);
    let argv = cargs(args);
    let envp = cargs(env);
    let (r, w) = io::pipe2(O_CLOEXEC)?;
    let child = Child::start(|| {
        let _ = io::close(r);
        if let Err(msg) = setup() {
            let _ = io::write(w, format!("S{msg}").as_bytes());
            return 126;
        }
        let ret = sc(nr::EXECVE, &[path.as_ptr() as u64, argv.ptrs.as_ptr() as u64, envp.ptrs.as_ptr() as u64]);
        let _ = io::write(w, format!("E{}", -ret).as_bytes());
        127
    });
    let _ = io::close(w);
    let child = match child {
        Ok(child) => child,
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    let answer = read_to_eof(r, EXEC_MS);
    let _ = io::close(r);
    let answer = answer.map_err(|e| format!("waiting for the exec: {e}"))?;
    match answer.first() {
        None => Ok((child, None)),
        Some(b'E') => Ok((child, std::str::from_utf8(&answer[1..]).ok().and_then(|s| s.parse().ok()))),
        Some(b'S') => err(format!("before the exec: {}", String::from_utf8_lossy(&answer[1..]))),
        _ => err("the exec's report was garbled"),
    }
}

/// The errno an exec in a child fails with, or a failure if it succeeded.
fn exec_errno(path: &str, args: &[String], setup: impl FnOnce() -> Checked) -> Result<i64, CaseError> {
    let (mut child, errno) = spawn_exec(path, args, &[], setup)?;
    match errno {
        Some(errno) => { child.wait()?; Ok(errno) }
        None => {
            let status = child.wait()?;
            err(format!("the exec succeeded; the program ended with {}", status_text(status)))
        }
    }
}

/// Exec `path` in a child after `setup`; FD_ARG in `args` or `env` becomes a pipe the
/// program reports on. Returns what it wrote; the program must exit 0.
fn exec_output(path: &str, args: &[&str], env: &[&str], setup: impl FnOnce() -> Checked) -> Result<Vec<u8>, CaseError> {
    let (r, w) = io::pipe()?;
    let fill = |items: &[&str]| -> Vec<String> {
        items.iter().map(|a| if *a == FD_ARG { w.raw().to_string() } else { a.to_string() }).collect()
    };
    let started = spawn_exec(path, &fill(args), &fill(env), || { let _ = io::close(r); setup() });
    let _ = io::close(w);
    let (mut child, errno) = match started {
        Ok(started) => started,
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    if let Some(errno) = errno {
        let _ = io::close(r);
        return err(format!("exec failed with {}", errname(errno)));
    }
    let out = read_to_eof(r, EXEC_MS);
    let _ = io::close(r);
    let out = out?;
    let status = child.wait()?;
    if !(exited(status) && exit_code(status) == 0) {
        return err(format!("the exec'd program ended with {}", status_text(status)));
    }
    Ok(out)
}

/// The helper's `report` output: its argv and its environment.
fn split_report(out: &[u8]) -> Result<(Vec<String>, Vec<String>), CaseError> {
    let sep = out.iter().position(|&b| b == 1).ok_or("the program's report has no separator")?;
    let list = |bytes: &[u8]| -> Vec<String> {
        let mut items: Vec<String> = bytes.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
        items.pop();
        items
    };
    Ok((list(&out[..sep]), list(&out[sep + 1..])))
}

/// The helper's `state` report after `setup`, as key/value pairs.
fn exec_state(fds: &[Fd], setup: impl FnOnce() -> Checked) -> Result<HashMap<String, String>, CaseError> {
    let fd_args: Vec<String> = fds.iter().map(|fd| fd.raw().to_string()).collect();
    let mut args = vec!["processes-exec", "state", FD_ARG];
    args.extend(fd_args.iter().map(String::as_str));
    let out = exec_output(HELPER, &args, &[], setup)?;
    Ok(String::from_utf8_lossy(&out).lines()
        .filter_map(|line| line.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect())
}

fn state_is(state: &HashMap<String, String>, key: &str, expected: &str, what: &str) -> CaseResult {
    match state.get(key) {
        Some(value) if value == expected => Ok(()),
        Some(value) => fail(format!("{what}: after exec {key} is {value}, expected {expected}")),
        None => fail(format!("the exec'd program did not report {key}")),
    }
}

/// The part of an ELF64 image a loader reads: up to the end of the furthest segment its
/// program headers name, with the section header table (and the symbols after the
/// segments) dropped. Copying only this keeps the set-ID cases well inside their limit.
fn loadable(mut elf: Vec<u8>) -> Result<Vec<u8>, String> {
    let word = |elf: &[u8], at: usize, len: usize| -> Option<usize> {
        let bytes = elf.get(at..at + len)?;
        Some(bytes.iter().rev().fold(0usize, |v, &b| (v << 8) | b as usize))
    };
    let bad = || format!("{HELPER} is not an ELF64 image this case can read");
    if elf.get(..4) != Some(&b"\x7fELF"[..]) { return Err(bad()); }
    let phoff = word(&elf, 0x20, 8).ok_or_else(bad)?;
    let phentsize = word(&elf, 0x36, 2).ok_or_else(bad)?;
    let phnum = word(&elf, 0x38, 2).ok_or_else(bad)?;
    let mut end = phoff + phentsize * phnum;
    for i in 0..phnum {
        let header = phoff + i * phentsize;
        end = end.max(word(&elf, header + 8, 8).ok_or_else(bad)? + word(&elf, header + 32, 8).ok_or_else(bad)?);
    }
    if end > elf.len() { return Err(bad()); }
    elf.truncate(end);
    elf[0x28..0x30].fill(0);
    elf[0x3c..0x40].fill(0);
    Ok(elf)
}

/// A copy of the helper's loadable image at `name`, owned by `uid`:`gid` with `mode`.
fn helper_copy(tmp: &Tmp, name: &str, uid: u32, gid: u32, mode: u32) -> Result<String, CaseError> {
    let bytes = loadable(std::fs::read(HELPER).map_err(|e| format!("reading {HELPER}: {e}"))?)?;
    let path = tmp.file(name, &bytes, 0o755)?;
    want("chown", chown(&path, uid, gid))?;
    want("chmod", chmod(&path, mode))?;
    Ok(path)
}

fn bytes_all(ptr: *const u8, len: usize, value: u8) -> bool {
    // SAFETY: callers pass a mapping of at least `len` readable bytes.
    (0..len).all(|i| unsafe { core::ptr::read_volatile(ptr.add(i)) } == value)
}

// ---------------------------------------------------------------------------
// fork

fn fork_pids() -> CaseResult {
    let parent = pid();
    let (r, w) = io::pipe()?;
    let mut child = Child::start(|| {
        let _ = io::close(r);
        let _ = io::write(w, &pid().to_le_bytes());
        if ppid() == parent { 0 } else { 1 }
    })?;
    io::close(w)?;
    let seen = read_i32(r, "the child's PID");
    io::close(r)?;
    check(child.pid > 0 && child.pid != parent, "fork returned a non-positive PID or the parent's own")?;
    check(seen? == child.pid, "the child's getpid differs from the PID fork returned to the parent")?;
    child.expect_exit(0, "the child, checking that getppid is its parent's PID,")
}

static DATA_WORD: AtomicU64 = AtomicU64::new(0x1111);
static BSS_WORD: AtomicU64 = AtomicU64::new(0);

const ORIGINAL: u64 = 0x5a;
const BY_PARENT: u64 = 0xa5;
const BY_CHILD: u64 = 0xc3;

/// One fork of a copy-on-write case: `fill(v)` writes `v` to every location under test and
/// `differs(v)` names one that does not hold `v`. With `parent_first` the parent writes
/// while every page is still shared and the child then checks it still sees `ORIGINAL`;
/// otherwise the child writes first and the parent checks it still sees `ORIGINAL`. The
/// other side writes afterwards, and each must end holding only its own writes.
fn cow_round(fill: &dyn Fn(u64), differs: &dyn Fn(u64) -> Option<&'static str>, parent_first: bool) -> CaseResult {
    fill(ORIGINAL);
    let (go_r, go_w) = io::pipe()?;
    let (wrote_r, wrote_w) = io::pipe()?;
    let child = task(|| {
        let _ = io::close(go_w);
        let _ = io::close(wrote_r);
        if parent_first {
            drain(go_r);
            if let Some(region) = differs(ORIGINAL) { return Err(format!("saw the parent's write to {region}")); }
            fill(BY_CHILD);
        } else {
            fill(BY_CHILD);
            let _ = io::write(wrote_w, b"w");
            drain(go_r);
            if let Some(region) = differs(BY_CHILD) { return Err(format!("lost its own write to {region} when the parent wrote")); }
        }
        Ok(())
    })?;
    io::close(go_r)?;
    io::close(wrote_w)?;
    let before = if parent_first { Ok(()) } else {
        match read_up_to(wrote_r, 1, WAIT_MS) {
            Ok(said) if said.is_empty() => fail("the child never said it had written"),
            Ok(_) => match differs(ORIGINAL) {
                Some(region) => fail(format!("the child's write to {region}, made first, reached the parent")),
                None => Ok(()),
            },
            Err(e) => fail(e),
        }
    };
    if before.is_ok() { fill(BY_PARENT); }
    io::close(go_w)?;
    io::close(wrote_r)?;
    before?;
    child.finish()?;
    match differs(BY_PARENT) {
        Some(region) => fail(format!("the child's write to {region} reached the parent")),
        None => Ok(()),
    }
}

fn fork_cow() -> CaseResult {
    const HEAP_WORDS: usize = 8192;
    let mut stack = [0u64; 512];
    let mut heap = vec![0u64; HEAP_WORDS];
    let stack_word = unsafe { stack.as_mut_ptr().add(300) };
    let heap_ptr = heap.as_mut_ptr();
    // SAFETY (both closures): the pointers stay inside `stack` and `heap`, which outlive the case.
    let fill = |v: u64| {
        DATA_WORD.store(v, Ordering::Relaxed);
        BSS_WORD.store(v, Ordering::Relaxed);
        unsafe {
            core::ptr::write_volatile(stack_word, v);
            for i in 0..HEAP_WORDS { core::ptr::write_volatile(heap_ptr.add(i), v); }
        }
    };
    let differs = |v: u64| -> Option<&'static str> {
        if DATA_WORD.load(Ordering::Relaxed) != v { Some("initialized data") }
        else if BSS_WORD.load(Ordering::Relaxed) != v { Some("zero-initialized data") }
        else if unsafe { core::ptr::read_volatile(stack_word) } != v { Some("the stack") }
        else if !(0..HEAP_WORDS).all(|i| unsafe { core::ptr::read_volatile(heap_ptr.add(i)) } == v) { Some("the heap") }
        else { None }
    };
    cow_round(&fill, &differs, true)?;
    cow_round(&fill, &differs, false)
}

fn fork_private_mapping() -> CaseResult {
    const LEN: usize = 16384;
    let map = memory::mmap(core::ptr::null_mut(), LEN, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)?;
    // SAFETY: the mapping is LEN bytes and stays mapped through the case.
    let fill = |v: u64| unsafe { core::ptr::write_bytes(map, v as u8, LEN) };
    let differs = |v: u64| (!bytes_all(map, LEN, v as u8)).then_some("a private mapping");
    cow_round(&fill, &differs, true)?;
    cow_round(&fill, &differs, false)
}

fn fork_shared_mapping() -> CaseResult {
    const LEN: usize = 16384;
    let map = memory::mmap(core::ptr::null_mut(), LEN, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0)?;
    unsafe { core::ptr::write_bytes(map, 0x11, LEN) };
    task(|| {
        if !bytes_all(map, LEN, 0x11) { return Err("did not see the shared mapping's contents".into()); }
        unsafe { core::ptr::write_bytes(map, 0x77, LEN) };
        Ok(())
    })?.finish()?;
    check(bytes_all(map, LEN, 0x77), "the child's writes to a MAP_SHARED mapping did not reach the parent")
}

fn fork_descriptors() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = tmp.path("offset");
    let fd = fs::open_with_mode(&path, fs::O_CREAT | fs::O_EXCL | fs::O_RDWR, 0o600)?;
    write_all(fd, b"abc")?;
    let (r, w) = io::pipe()?;
    task(|| {
        write_all(fd, b"def").map_err(|e| format!("write on the inherited descriptor failed: {e}"))?;
        io::close(w).map_err(|e| format!("close of the inherited pipe failed: {e}"))
    })?.finish()?;
    let offset = fs::lseek(fd, 0, 1)?;
    check(offset == 6, &format!("the child's write did not move the shared file offset: the parent is at {offset}, expected 6"))?;
    fs::lseek(fd, 0, 0)?;
    let mut buf = [0u8; 16];
    let n = fs::read(fd, &mut buf)?;
    check(&buf[..n] == b"abcdef", "the file does not hold the parent's and the child's writes in order")?;
    check(io::write(w, b"x").is_ok(), "closing a descriptor in the child closed the parent's")?;
    check(matches!(io::read(r, &mut buf), Ok(1)) && buf[0] == b'x', "the parent's pipe no longer works")?;
    io::close(fd)?;
    Ok(())
}

fn fork_descriptor_flags() -> CaseResult {
    let (cloexec_r, _cloexec_w) = io::pipe2(O_CLOEXEC)?;
    let (plain_r, plain_w) = io::pipe()?;
    task(|| {
        let a = io::fcntl_getfd(cloexec_r).map_err(|e| format!("F_GETFD failed: {e}"))?;
        let b = io::fcntl_getfd(plain_r).map_err(|e| format!("F_GETFD failed: {e}"))?;
        if a & 1 == 0 { return Err("lost FD_CLOEXEC on an inherited descriptor".into()); }
        if b & 1 != 0 { return Err("gained FD_CLOEXEC on an inherited descriptor".into()); }
        io::fcntl_setfl(plain_w, O_NONBLOCK as i32).map_err(|e| format!("F_SETFL failed: {e}"))?;
        Ok(())
    })?.finish()?;
    let flags = io::fcntl_getfl(plain_w)?;
    check(flags & O_NONBLOCK != 0,
        "O_NONBLOCK set through the child's descriptor is not on the parent's: they do not share the open file description")
}

fn fork_signal_dispositions() -> CaseResult {
    catch(SIGUSR1)?;
    ignore(SIGUSR2)?;
    task(|| {
        if handler_of(SIGUSR1).map_err(|e| e.to_string())? != handler_address() { return Err("lost the parent's SIGUSR1 handler".into()); }
        if handler_of(SIGUSR2).map_err(|e| e.to_string())? != SIG_IGN { return Err("lost the parent's SIG_IGN for SIGUSR2".into()); }
        kill(pid(), SIGUSR1);
        if HITS.load(Ordering::Relaxed) != 1 { return Err("the inherited SIGUSR1 handler did not run".into()); }
        kill(pid(), SIGUSR2);
        Ok(())
    })?.finish()
}

fn fork_signal_mask() -> CaseResult {
    block(bit(SIGUSR1) | bit(SIGUSR2))?;
    task(|| {
        let mask = blocked().map_err(|e| e.to_string())?;
        if mask & (bit(SIGUSR1) | bit(SIGUSR2)) == bit(SIGUSR1) | bit(SIGUSR2) { Ok(()) }
        else { Err(format!("the signal mask is {mask:#x}; the parent blocked SIGUSR1 and SIGUSR2")) }
    })?.finish()
}

fn fork_pending_cleared() -> CaseResult {
    block(bit(SIGUSR1))?;
    want("kill", kill(pid(), SIGUSR1))?;
    check(pending()? & bit(SIGUSR1) != 0, "a blocked SIGUSR1 sent to the parent is not pending")?;
    task(|| {
        let set = pending().map_err(|e| e.to_string())?;
        if set & bit(SIGUSR1) == 0 { Ok(()) } else { Err("inherited the parent's pending SIGUSR1".into()) }
    })?.finish()
}

fn fork_timer_cleared() -> CaseResult {
    want("setitimer(ITIMER_REAL)", set_real_timer(30))?;
    task(|| {
        let left = real_timer_left()?;
        if left == (0, 0) { Ok(()) } else { Err(format!("inherited the parent's alarm: {}.{:06} s left", left.0, left.1)) }
    })?.finish()
}

fn fork_cwd_umask() -> CaseResult {
    let tmp = Tmp::new()?;
    want("chdir", chdir(&tmp.dir))?;
    umask(0o037);
    let result = task(|| {
        let dir = cwd()?;
        if dir != tmp.dir { return Err(format!("the working directory is {dir}, expected {}", tmp.dir)); }
        let mask = umask(0o022);
        if mask != 0o037 { return Err(format!("the umask is {mask:o}, expected 37")); }
        Ok(())
    }).and_then(Task::finish);
    chdir("/");
    result
}

fn fork_times_reset() -> CaseResult {
    // A child that has done nothing yet has used well under 2 ticks (20 ms at this ABI's
    // 100 per second) of CPU, so a count above that was carried over from the parent.
    const FRESH: i64 = 2;
    burn(400);
    task(|| { burn(150); Ok(()) })?.finish()?;
    let (_, parent) = times()?;
    let own = parent[0] + parent[1];
    check(own > 4 * FRESH && parent[2] + parent[3] > 0,
        &format!("times() in the parent reports {parent:?} after 400 ms of computation and a waited-for child that did 150 ms"))?;
    task(move || {
        let (_, t) = times()?;
        if t[2] != 0 || t[3] != 0 { return Err(format!("tms_cutime/tms_cstime are {}/{}, not 0", t[2], t[3])); }
        if t[0] + t[1] > FRESH {
            return Err(format!("tms_utime+tms_stime is {} ticks before the child has computed anything (the parent's is {own}): not reset", t[0] + t[1]));
        }
        Ok(())
    })?.finish()
}

fn fork_nproc() -> CaseResult {
    become_user(4321, 4321)?;
    want("setrlimit(RLIMIT_NPROC)", setrlimit(RLIMIT_NPROC, 1, 1))?;
    let r = raw_fork();
    if r == 0 { process::exit(0) }
    if r > 0 {
        let mut child = Child { pid: r as i32, live: true };
        let _ = child.wait();
        return fail("fork succeeded for a user already at RLIMIT_NPROC");
    }
    want_err("fork beyond RLIMIT_NPROC", r, EAGAIN)
}

/// The kernel heap's free space in kB, as /proc/meminfo reports it.
fn kernel_heap_free_kb() -> Result<u64, CaseError> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|e| format!("/proc/meminfo: {e}"))?;
    text.lines().find_map(|line| line.strip_prefix("KernelHeapFree:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| CaseError::Fail("/proc/meminfo has no KernelHeapFree line".into()))
}

/// Wait, for at most a second, until `pid`'s threads are all blocked.
fn parked_within(pid: i32) -> CaseResult {
    let path = format!("/proc/{pid}/status");
    let start = now_ms();
    while !std::fs::read_to_string(&path)
        .is_ok_and(|status| status.lines().any(|line| line == "State:\tBlocked"))
    {
        check(now_ms().saturating_sub(start) < bounded(1000, CLEANUP_MS),
            &format!("child {pid} never blocked"))?;
        process::yield_now()?;
    }
    Ok(())
}

/// SIGKILL `child` and reap it, requiring death by SIGKILL.
fn kill_and_reap(child: &mut Child, what: &str) -> CaseResult {
    want("kill", kill(child.pid, SIGKILL))?;
    let (_, status) = wait_within(child.pid, 0, WAIT_MS)?;
    child.live = false;
    check(signaled(status) && term_sig(status) == SIGKILL,
        &format!("{what} ended with {}, expected death by SIGKILL", status_text(status)))
}

/// Wait, for at most a second, until the kernel heap's free space is back to
/// within `slack_kb` of `before_kb`: a reaped child's last thread is retired
/// a little after the reap.
fn heap_back_within(before_kb: u64, slack_kb: u64, what: &str) -> CaseResult {
    let start = now_ms();
    loop {
        let after_kb = kernel_heap_free_kb()?;
        if after_kb + slack_kb >= before_kb { return Ok(()); }
        if now_ms().saturating_sub(start) >= bounded(1000, REPORT_MS) {
            return fail(&format!("{what}: kernel heap free {before_kb} kB before, {after_kb} kB after"));
        }
        nap();
    }
}

/// Wait, for at most a second, until `pid`, which wrote its last byte before
/// its execve at `ready_ms`, is inside the exec (true) or has ended (false).
/// It is inside once it is seen blocked, or still running 5 ms after that
/// byte: the few instructions from its write to its execve take far less.
#[cfg(not(target_arch = "x86_64"))]
fn inside_exec_or_ended(pid: i32, ready_ms: u64) -> Result<bool, CaseError> {
    let path = format!("/proc/{pid}/status");
    loop {
        if let Ok(status) = std::fs::read_to_string(&path) {
            let state = |s: &str| status.lines().any(|line| line.strip_prefix("State:\t") == Some(s));
            if state("Blocked") || (state("Running") && now_ms().saturating_sub(ready_ms) >= 5) {
                return Ok(true);
            }
            if state("Terminated") { return Ok(false); }
        }
        check(now_ms().saturating_sub(ready_ms) < bounded(1000, CLEANUP_MS),
            &format!("child {pid} neither entered its exec nor ended"))?;
        process::yield_now()?;
    }
}

/// SIGKILL children parked in a 64 KiB pipe read. Killed there, a thread used
/// to be discarded with everything its kernel stack owned: the read's buffer
/// and the pipe it kept alive, about 130 KiB each.
fn kill_readers() -> CaseResult {
    const READERS: usize = 16;
    const SLACK_KB: u64 = 512;
    let before = kernel_heap_free_kb()?;
    for round in 0..READERS {
        let (reader, writer) = io::pipe()?;
        let mut child = Child::start(|| {
            let _ = io::close(writer);
            let _ = io::read(reader, &mut [0u8; 65536]);
            0
        })?;
        io::close(reader)?;
        // The writer stays open until the child is reaped, so its read never
        // sees end of file.
        let result = parked_within(child.pid)
            .and_then(|()| kill_and_reap(&mut child, &format!("reader {round}")));
        io::close(writer)?;
        result?;
    }
    heap_back_within(before, SLACK_KB, &format!("{READERS} readers killed"))
}

/// A copy of the helper with zeros after it, which the loader ignores but an
/// exec reads. Zeros are appended for at most 50 ms of writing, up to 8 MiB:
/// a disk that writes them fast also reads them fast, and needs them to keep
/// an exec long enough to catch, while on a slow one the helper alone is.
/// Written 64 KiB per call, so a kill ends the writer within one.
#[cfg(not(target_arch = "x86_64"))]
fn padded_helper(tmp: &Tmp) -> Result<String, CaseError> {
    const CHUNK: usize = 64 * 1024;
    const MAX_PADDING: usize = 8 << 20;
    let image = std::fs::read(HELPER).map_err(|e| format!("reading {HELPER}: {e}"))?;
    let path = tmp.path("padded-helper");
    let fd = fs::open_with_mode(&path, fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY, 0o600)?;
    let written = (|| -> Result<(), Error> {
        for chunk in image.chunks(CHUNK) { write_all(fd, chunk)?; }
        let zeros = vec![0u8; CHUNK];
        let start = now_ms();
        let mut padding = 0;
        while padding < MAX_PADDING && now_ms().saturating_sub(start) < 50 {
            write_all(fd, &zeros)?;
            padding += CHUNK;
        }
        Ok(())
    })();
    io::close(fd)?;
    written?;
    want("chmod", chmod(&path, 0o755))?;
    Ok(path)
}

/// Fork a child that writes a byte and then execs `path` (the helper, or
/// `padded_helper`) to run `ran`. execve is the child's only syscall after the
/// write, so once `inside_exec_or_ended` finds it inside the exec it is
/// SIGKILLed there and reaped: true. False when the exec finished first and
/// the new program exited 77, before the parent looked or before its kill
/// arrived.
#[cfg(not(target_arch = "x86_64"))]
fn kill_inside_exec(path: &CString, what: &str) -> Result<bool, CaseError> {
    let argv = cargs(&strings(&["processes-exec_test", "ran"]));
    let envp = cargs(&[]);
    let (ready_r, ready_w) = io::pipe()?;
    let mut child = Child::start(|| {
        let _ = io::close(ready_r);
        let _ = io::write(ready_w, b"x");
        sc(nr::EXECVE, &[path.as_ptr() as u64, argv.ptrs.as_ptr() as u64, envp.ptrs.as_ptr() as u64]);
        127
    })?;
    io::close(ready_w)?;
    let said = read_up_to(ready_r, 1, WAIT_MS);
    let ready_ms = now_ms();
    io::close(ready_r)?;
    check(said?.len() == 1, &format!("{what}: the child never reached its exec"))?;
    if inside_exec_or_ended(child.pid, ready_ms)? {
        want("kill", kill(child.pid, SIGKILL))?;
    }
    let status = child.wait()?;
    if exited(status) && exit_code(status) == 77 {
        return Ok(false);
    }
    check(signaled(status) && term_sig(status) == SIGKILL,
        &format!("{what} ended with {}, expected death by SIGKILL or the helper's exit 77", status_text(status)))?;
    Ok(true)
}

/// SIGKILL children inside an exec, while it reads the new program from disk.
/// Killed there, a thread used to leave the old image and the new one's
/// partly read data behind, about 46 KiB each. When an exec finishes before
/// the kill, the kill is tried again with a new child.
///
/// A first, unmeasured kill decides what the children run: the helper, or,
/// when its exec finished before the kill, `padded_helper`. On Parallels the
/// helper's own few reads finished before the parent ever saw the child
/// inside its exec; on ARM64 QEMU even writing the padded copy took most of
/// the case's time.
///
/// Not run on x86-64, which brings up one CPU: there the parent was never
/// seen to run while a child was inside an exec, so no kill can land in one.
#[cfg(not(target_arch = "x86_64"))]
fn kill_execs() -> CaseResult {
    const EXECS: usize = 16;
    const TRIES: usize = 4;
    const SLACK_KB: u64 = 256;
    let tmp = Tmp::new()?;
    let helper = cpath(HELPER);
    let padded;
    let path = if kill_inside_exec(&helper, "exec probe")? {
        &helper
    } else {
        padded = cpath(&padded_helper(&tmp)?);
        &padded
    };
    let before = kernel_heap_free_kb()?;
    for round in 0..EXECS {
        let what = format!("exec {round}");
        let mut killed = false;
        for _ in 0..TRIES {
            if kill_inside_exec(path, &what)? {
                killed = true;
                break;
            }
        }
        check(killed, &format!("{what}: the exec finished before the kill in all {TRIES} tries"))?;
    }
    heap_back_within(before, SLACK_KB, &format!("{EXECS} execs killed"))
}

fn fork_kill_heap() -> CaseResult {
    // Each half has its own workload and allowance, and both run, so a
    // failure reports each.
    #[cfg(not(target_arch = "x86_64"))]
    return match (kill_readers(), kill_execs()) {
        (Err(CaseError::Fail(readers)), Err(CaseError::Fail(execs))) => fail(format!("{readers}; {execs}")),
        (readers, execs) => readers.and(execs),
    };
    #[cfg(target_arch = "x86_64")]
    kill_readers()
}

fn fork_many() -> CaseResult {
    let (r, w) = io::pipe()?;
    let mut children = Vec::new();
    for i in 0..32 {
        // drain returns only at end of file, which needs the parent's write end closed.
        let child = Child::start(|| { let _ = io::close(w); if drain(r) { i + 1 } else { 100 + i } });
        match child {
            Ok(child) => children.push(child),
            Err(e) => return fail(format!("fork of child {i} with {i} running failed: {e:?}")),
        }
    }
    for (i, child) in children.iter_mut().enumerate() {
        let mut status = 0;
        let r = wait4(child.pid, &mut status, WNOHANG);
        if r == child.pid as i64 { child.live = false; }
        if r != 0 {
            return fail(format!("child {i} was not running once all 32 had started: waitpid returned {} with {}",
                shown(r), status_text(status)));
        }
    }
    io::close(w)?;
    io::close(r)?;
    for (i, child) in children.iter_mut().enumerate() {
        child.expect_exit(i as i32 + 1, &format!("child {i} of 32"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// exec

fn exec_argv() -> CaseResult {
    let args = ["argv zero", "report", FD_ARG, "two words", "", "\u{fc}n\u{ef}"];
    let out = exec_output(HELPER, &args, &[], || Ok(()))?;
    let (argv, _) = split_report(&out)?;
    let expected_tail: Vec<String> = strings(&args[3..]);
    check(argv.len() == args.len() && argv[0] == args[0] && argv[1] == "report" && argv[3..] == expected_tail[..],
        &format!("the program received argv {argv:?}"))
}

fn exec_envp() -> CaseResult {
    let env = ["ALPHA=1", "PATH=/bin:/sbin", "EMPTY=", "SPACED=a b"];
    let out = exec_output(HELPER, &["helper", "report", FD_ARG], &env, || Ok(()))?;
    let (_, environ) = split_report(&out)?;
    check(environ == strings(&env), &format!("the program's environment is {environ:?}, expected {env:?}"))
}

fn exec_empty_env() -> CaseResult {
    let out = exec_output(HELPER, &["helper", "report", FD_ARG], &[], || Ok(()))?;
    let (_, environ) = split_report(&out)?;
    check(environ.is_empty(), &format!("an empty envp gave the environment {environ:?}"))
}

fn exec_many_args() -> CaseResult {
    let extra: Vec<String> = (0..256).map(|i| format!("arg-{i}")).collect();
    let mut args = vec!["helper", "report", FD_ARG];
    args.extend(extra.iter().map(String::as_str));
    let out = exec_output(HELPER, &args, &[], || Ok(()))?;
    let (argv, _) = split_report(&out)?;
    check(argv.len() == args.len() && argv[3..] == extra[..],
        &format!("259 arguments arrived as {} (last {:?})", argv.len(), argv.last()))
}

/// {ARG_MAX} as a C library reports it on this ABI (musl's sysconf(_SC_ARG_MAX)): a
/// quarter of the soft RLIMIT_STACK when that is finite and larger, else 131072.
fn arg_max() -> Result<u64, CaseError> {
    let [soft, _] = getrlimit(RLIMIT_STACK)?;
    Ok(if soft != RLIM_INFINITY && soft / 4 > 131072 { soft / 4 } else { 131072 })
}

fn exec_long_arg() -> CaseResult {
    let max = arg_max()?;
    if max < 2 * 65536 { return skip(format!("ARG_MAX is {max}, too small to hold a 64 KiB argument")); }
    let long: String = (0..65536).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    let out = exec_output(HELPER, &["helper", "report", FD_ARG, &long], &[], || Ok(()))?;
    let (argv, _) = split_report(&out)?;
    let got = argv.get(3).map_or(0, String::len);
    check(argv.len() == 4 && argv[3] == long, &format!("a 65536-byte argument arrived as {got} bytes"))?;
    // Leave 64 bytes for three short strings and the argument pointer table.
    let near_limit = "x".repeat(max as usize - 64);
    let mut old_alt_stack = [0u8; 8192];
    let out = exec_output(HELPER, &["helper", "stack", FD_ARG, &near_limit], &[], || {
        let alt = StackT { ss_sp: old_alt_stack.as_mut_ptr() as u64, ss_flags: 0, _pad: 0, ss_size: old_alt_stack.len() };
        signal::sigaltstack(Some(&alt), None).map_err(|e| format!("enable old alternate stack: {e:?}"))?;
        Ok(())
    })?;
    check(out == b"stack-ok", "near-limit exec lost its argument or rejected a runtime stack buffer")
}

fn exec_e2big() -> CaseResult {
    let max = arg_max()?;
    // Strings of 4096 bytes with their NULs, more of them than fit in ARG_MAX.
    let count = max / 4096 + 1;
    if count > 2048 { return skip(format!("ARG_MAX is {max}; more than 8 MiB of arguments does not fit the case's limit")); }
    let big = "x".repeat(4095);
    let mut args = strings(&["helper", "ran"]);
    args.extend((0..count).map(|_| big.clone()));
    let errno = exec_errno(HELPER, &args, || Ok(()))?;
    want_err(&format!("exec with {} bytes of arguments, ARG_MAX being {max}", count * 4096), -errno, E2BIG)
}

fn exec_enoent() -> CaseResult {
    let errno = exec_errno("/usr/local/test/bin/no-such-program", &strings(&["x"]), || Ok(()))?;
    want_err("exec of a missing file", -errno, ENOENT)
}

fn exec_enotdir() -> CaseResult {
    let path = format!("{HELPER}/x");
    let errno = exec_errno(&path, &strings(&["x"]), || Ok(()))?;
    want_err("exec through a path whose prefix is a file", -errno, ENOTDIR)
}

fn exec_enametoolong() -> CaseResult {
    let path = format!("/{}", "a".repeat(5000));
    let errno = exec_errno(&path, &strings(&["x"]), || Ok(()))?;
    want_err("exec of a 5001-byte path", -errno, ENAMETOOLONG)
}

fn exec_eacces_mode() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = tmp.file("noexec", format!("#!{HELPER} ran\n").as_bytes(), 0o644)?;
    let errno = exec_errno(&path, &strings(&["x"]), || Ok(()))?;
    want_err("exec of a file with no execute permission", -errno, EACCES)
}

fn exec_eacces_dir() -> CaseResult {
    let tmp = Tmp::new()?;
    let errno = exec_errno(&tmp.dir, &strings(&["x"]), || Ok(()))?;
    want_err("exec of a directory", -errno, EACCES)
}

fn exec_enoexec() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = tmp.file("garbage", b"this is neither an executable nor a script\n", 0o755)?;
    let errno = exec_errno(&path, &strings(&["x"]), || Ok(()))?;
    want_err("exec of an executable file in no known format", -errno, ENOEXEC)
}

static MARK: AtomicU64 = AtomicU64::new(0x5151_5151);

fn exec_failed_intact() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = tmp.file("garbage", b"\x7fELF but nothing else\n", 0o755)?;
    let heap = vec![0x6262u64; 4096];
    let (r, w) = io::pipe()?;
    catch(SIGUSR1)?;
    let args = cargs(&strings(&["x"]));
    let env = cargs(&[]);
    let c = cpath(&path);
    let ret = sc(nr::EXECVE, &[c.as_ptr() as u64, args.ptrs.as_ptr() as u64, env.ptrs.as_ptr() as u64]);
    want_err("exec of a truncated ELF image", ret, ENOEXEC)?;
    check(MARK.load(Ordering::Relaxed) == 0x5151_5151 && heap.iter().all(|&v| v == 0x6262),
        "the failed exec changed the caller's memory")?;
    check(io::write(w, b"k").is_ok(), "the failed exec closed the caller's descriptors")?;
    let mut b = [0u8; 1];
    check(matches!(io::read(r, &mut b), Ok(1)) && b[0] == b'k', "the caller's pipe no longer works after a failed exec")?;
    kill(pid(), SIGUSR1);
    check(HITS.load(Ordering::Relaxed) == 1, "the caller's signal handler was lost by a failed exec")
}

fn exec_script() -> CaseResult {
    let tmp = Tmp::new()?;
    let script = tmp.file("script", format!("#!{HELPER} script\n").as_bytes(), 0o755)?;
    let out = exec_output(&script, &["ignored-zero", FD_ARG, "x"], &[], || Ok(()))?;
    let (argv, _) = split_report(&out)?;
    let expected = [HELPER, "script", script.as_str()];
    check(argv.len() == 5 && argv[..3] == expected && argv[4] == "x",
        &format!("the interpreter received argv {argv:?}; expected [{HELPER}, script, {script}, fd, x]"))
}

fn exec_script_noarg() -> CaseResult {
    let tmp = Tmp::new()?;
    let script = tmp.file("script", format!("#!{HELPER}\n").as_bytes(), 0o755)?;
    let out = exec_output(&script, &["ignored-zero", FD_ARG, "x"], &[], || Ok(()))?;
    let (argv, _) = split_report(&out)?;
    check(argv.len() == 4 && argv[0] == HELPER && argv[1] == script && argv[3] == "x",
        &format!("the interpreter received argv {argv:?}; expected [{HELPER}, {script}, fd, x]"))
}

fn exec_script_missing() -> CaseResult {
    let tmp = Tmp::new()?;
    let script = tmp.file("script", b"#!/usr/local/test/bin/no-such-interpreter\n", 0o755)?;
    let errno = exec_errno(&script, &strings(&["x"]), || Ok(()))?;
    want_err("exec of a script whose interpreter is missing", -errno, ENOENT)
}

fn exec_relative() -> CaseResult {
    let out = exec_output("./processes-exec_test", &["relative", "report", FD_ARG], &[], || {
        let r = chdir("/usr/local/test/bin");
        if r == 0 { Ok(()) } else { Err(format!("chdir failed with {}", shown(r))) }
    })?;
    let (argv, _) = split_report(&out)?;
    check(argv.first().map(String::as_str) == Some("relative"), &format!("the program received argv {argv:?}"))
}

fn exec_cloexec() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = tmp.file("data", b"abcdef", 0o644)?;
    let closing = fs::open(&path, fs::O_RDONLY | O_CLOEXEC as u32)?;
    let keeping = fs::open(&path, fs::O_RDONLY)?;
    fs::lseek(keeping, 3, 0)?;
    let state = exec_state(&[closing, keeping], || Ok(()))?;
    state_is(&state, &format!("fd{}", closing.raw()), "closed", "a close-on-exec descriptor")?;
    state_is(&state, &format!("fd{}", keeping.raw()), "open@3", "a descriptor without close-on-exec")
}

fn exec_signal_reset() -> CaseResult {
    let state = exec_state(&[], || {
        catch(SIGUSR1).map_err(|e| e.to_string())?;
        ignore(SIGUSR2).map_err(|e| e.to_string())
    })?;
    state_is(&state, "usr1", "dfl", "a caught signal")?;
    state_is(&state, "usr2", "ign", "an ignored signal")
}

fn exec_mask_pending() -> CaseResult {
    let state = exec_state(&[], || {
        block(bit(SIGUSR1) | bit(SIGUSR2)).map_err(|e| e.to_string())?;
        kill(pid(), SIGUSR2);
        Ok(())
    })?;
    let mask = u64::from_str_radix(state.get("mask").map_or("", String::as_str), 16).unwrap_or(0);
    let set = u64::from_str_radix(state.get("pending").map_or("", String::as_str), 16).unwrap_or(0);
    check(mask & (bit(SIGUSR1) | bit(SIGUSR2)) == bit(SIGUSR1) | bit(SIGUSR2),
        &format!("the signal mask after exec is {mask:#x}; SIGUSR1 and SIGUSR2 were blocked"))?;
    check(set & bit(SIGUSR2) != 0, &format!("the pending set after exec is {set:#x}; SIGUSR2 was pending"))
}

fn exec_keeps_process() -> CaseResult {
    let me = pid();
    let (r, w) = io::pipe()?;
    let state = exec_state(&[], || {
        let _ = io::close(r);
        let _ = io::write(w, &pid().to_le_bytes());
        let _ = io::close(w);
        let ret = setpgid(0, 0);
        if ret == 0 { Ok(()) } else { Err(format!("setpgid failed with {}", shown(ret))) }
    });
    io::close(w)?;
    let child = read_i32(r, "the child's PID");
    io::close(r)?;
    let (state, child) = (state?, child?);
    let sid = getsid(0).to_string();
    state_is(&state, "pid", &child.to_string(), "the process ID")?;
    state_is(&state, "ppid", &me.to_string(), "the parent process ID")?;
    state_is(&state, "pgid", &child.to_string(), "the process group")?;
    state_is(&state, "sid", &sid, "the session")
}

fn exec_keeps_cwd_umask() -> CaseResult {
    let tmp = Tmp::new()?;
    let dir = tmp.dir.clone();
    let state = exec_state(&[], || {
        let r = chdir(&dir);
        if r != 0 { return Err(format!("chdir failed with {}", shown(r))); }
        umask(0o027);
        Ok(())
    })?;
    state_is(&state, "cwd", &tmp.dir, "the working directory")?;
    state_is(&state, "umask", "27", "the umask")
}

fn exec_keeps_alarm() -> CaseResult {
    let state = exec_state(&[], || {
        let r = set_real_timer(30);
        if r == 0 { Ok(()) } else { Err(format!("setitimer failed with {}", shown(r))) }
    })?;
    let left = state.get("alarm").cloned().unwrap_or_default();
    let secs: f64 = left.parse().unwrap_or(0.0);
    check(secs > 0.0 && secs <= 30.0, &format!("an alarm 30 s out before exec has {left} s left after it"))
}

fn exec_keeps_ids() -> CaseResult {
    let state = exec_state(&[], || {
        for (what, r) in [("setgroups", setgroups(&[4901])), ("setgid", setgid(4902)), ("setuid", setuid(4903))] {
            if r != 0 { return Err(format!("{what} failed with {}", shown(r))); }
        }
        Ok(())
    })?;
    for (key, value) in [("uid", "4903"), ("euid", "4903"), ("gid", "4902"), ("egid", "4902")] {
        state_is(&state, key, value, "an ordinary program")?;
    }
    let raw = state.get("groups").ok_or("the exec'd program did not report groups")?;
    let got: Option<Vec<u32>> = raw.split(',').filter(|g| !g.is_empty()).map(|g| g.parse().ok()).collect();
    check(got.is_some_and(|got| groups_are(&got, &[4901], 4902)),
        &format!("after exec the supplementary groups are {raw}; setgroups gave 4901"))
}

fn exec_setuid_bit() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = helper_copy(&tmp, "setuid", 4911, 4912, 0o4755)?;
    let state = exec_state_at(&path, || become_user(4914, 4913))?;
    for (key, value) in [("uid", "4914"), ("euid", "4911"), ("gid", "4913"), ("egid", "4913")] {
        state_is(&state, key, value, "a set-user-ID program owned by 4911")?;
    }
    Ok(())
}

fn exec_setgid_bit() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = helper_copy(&tmp, "setgid", 4911, 4912, 0o2755)?;
    let state = exec_state_at(&path, || become_user(4914, 4913))?;
    for (key, value) in [("uid", "4914"), ("euid", "4914"), ("gid", "4913"), ("egid", "4912")] {
        state_is(&state, key, value, "a set-group-ID program of group 4912")?;
    }
    Ok(())
}

/// Exec a set-ID copy of the helper at `path` with `args`, as user `uid` and group `gid`;
/// its exit status is 0 or the step that failed, which `steps` describes.
fn exec_set_id(path: &str, args: &[&str], uid: u32, gid: u32, steps: &[(i32, &str)]) -> CaseResult {
    let (mut child, errno) = spawn_exec(path, &strings(args), &[], || become_user(uid, gid))?;
    if let Some(errno) = errno { return fail(format!("exec failed with {}", errname(errno))); }
    let status = child.wait()?;
    if exited(status) && exit_code(status) == 0 { return Ok(()); }
    match steps.iter().find(|(code, _)| exited(status) && exit_code(status) == *code) {
        Some((_, step)) => fail(*step),
        None => fail(format!("the set-ID program ended with {}", status_text(status))),
    }
}

fn exec_setuid_saved() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = helper_copy(&tmp, "setuid", 4921, 4922, 0o4755)?;
    exec_set_id(&path, &["setuid", "saved", "4924", "4921"], 4924, 4923, &[
        (10, "setuid to the real user ID failed in a set-user-ID program"),
        (11, "setuid to the real user ID left the effective ID unchanged"),
        (12, "setuid to the real user ID changed the real user ID"),
        (13, "setuid back to the saved set-user-ID (the owner, 4921) failed"),
        (14, "setuid back to the saved set-user-ID left the effective ID unchanged"),
        (15, "setuid back to the saved set-user-ID changed the real user ID"),
    ])
}

fn exec_setgid_saved() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = helper_copy(&tmp, "setgid", 4931, 4932, 0o2755)?;
    exec_set_id(&path, &["setgid", "savedgid", "4933", "4932"], 4934, 4933, &[
        (20, "setgid to the real group ID failed in a set-group-ID program"),
        (21, "setgid to the real group ID left the effective ID unchanged"),
        (22, "setgid to the real group ID changed the real group ID"),
        (23, "setgid back to the saved set-group-ID (the file's group, 4932) failed"),
        (24, "setgid back to the saved set-group-ID left the effective ID unchanged"),
        (25, "setgid back to the saved set-group-ID changed the real group ID"),
    ])
}

/// exec_state, running a copy of the helper at `path`.
fn exec_state_at(path: &str, setup: impl FnOnce() -> Checked) -> Result<HashMap<String, String>, CaseError> {
    let out = exec_output(path, &["processes-exec", "state", FD_ARG], &[], setup)?;
    Ok(String::from_utf8_lossy(&out).lines()
        .filter_map(|line| line.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect())
}

// ---------------------------------------------------------------------------
// wait (after the twelve former waitpid-suite cases above)

fn wait_echild_none() -> CaseResult {
    let mut status = 0;
    want_err("waitpid(-1, WNOHANG) with no children", wait4(-1, &mut status, WNOHANG), ECHILD)?;
    want_err("waitpid(-1, 0) with no children", wait4(-1, &mut status, 0), ECHILD)
}

fn wait_echild_other() -> CaseResult {
    let kid = held(|| Ok(()))?;
    let mut status = 0;
    want_err("waitpid on the parent's PID", wait4(ppid(), &mut status, WNOHANG), ECHILD)?;
    want_err("waitpid on its own PID", wait4(pid(), &mut status, WNOHANG), ECHILD)?;
    kid.release()
}

fn wait_einval() -> CaseResult {
    let kid = held(|| Ok(()))?;
    let mut status = 0;
    want_err("waitpid with an undefined option bit", wait4(-1, &mut status, WNOHANG | 0x100), EINVAL)?;
    kid.release()
}

fn wait_exit_codes() -> CaseResult {
    for code in [0, 1, 42, 127, 255, 256 + 7] {
        let mut child = Child::start(|| code)?;
        let status = child.wait()?;
        check(exited(status) && exit_code(status) == code & 0xff && !signaled(status),
            &format!("_exit({code}) was reported as {}", status_text(status)))?;
    }
    Ok(())
}

fn wait_signaled() -> CaseResult {
    let mut own = Child::start(|| { kill(pid(), SIGTERM); loop { nap(); } })?;
    let status = own.wait()?;
    check(signaled(status) && term_sig(status) == SIGTERM && !exited(status),
        &format!("a child that sent itself SIGTERM was reported as {}", status_text(status)))?;
    let mut target = held(|| Ok(()))?;
    want("kill", kill(target.pid(), SIGKILL))?;
    let status = target.child.wait()?;
    check(signaled(status) && term_sig(status) == SIGKILL && !exited(status),
        &format!("a child killed with SIGKILL was reported as {}", status_text(status)))
}

fn wait_fault() -> CaseResult {
    // SAFETY: deliberately reads an unmapped address so the child takes SIGSEGV.
    let mut child = Child::start(|| unsafe { core::ptr::read_volatile(16 as *const u64) } as i32)?;
    let status = child.wait()?;
    check(signaled(status) && term_sig(status) == SIGSEGV,
        &format!("a child that read an unmapped address was reported as {}", status_text(status)))
}

fn wait_stopped() -> CaseResult {
    let child = Child::start(|| { kill(pid(), SIGSTOP); 0 })?;
    let (_, status) = wait_within(child.pid, WUNTRACED, WAIT_MS)?;
    check(stopped(status) && stop_sig(status) == SIGSTOP,
        &format!("WUNTRACED reported {} for a child that sent itself SIGSTOP", status_text(status)))?;
    let mut again = 0;
    want_eq("a second WUNTRACED wait for the same stop", wait4(child.pid, &mut again, WUNTRACED | WNOHANG), 0)
}

fn wait_stop_hidden() -> CaseResult {
    let child = Child::start(|| { kill(pid(), SIGSTOP); 5 })?;
    let start = now_ms();
    while now_ms().saturating_sub(start) < 300 {
        let mut status = 0;
        let r = wait4(child.pid, &mut status, WNOHANG);
        if r != 0 {
            return fail(format!("waitpid without WUNTRACED returned {} with {}", shown(r), status_text(status)));
        }
        nap();
    }
    let (_, status) = wait_within(child.pid, WUNTRACED, WAIT_MS)?;
    check(stopped(status), &format!("the child was not stopped after all: {}", status_text(status)))
}

fn wait_continued() -> CaseResult {
    let (rel_r, rel_w) = io::pipe()?;
    let mut child = Child::start(|| { let _ = io::close(rel_w); kill(pid(), SIGSTOP); drain(rel_r); 0 })?;
    io::close(rel_r)?;
    let (_, status) = wait_within(child.pid, WUNTRACED, WAIT_MS)?;
    check(stopped(status), &format!("WUNTRACED reported {}", status_text(status)))?;
    want("kill(SIGCONT)", kill(child.pid, SIGCONT))?;
    let (_, status) = wait_within(child.pid, WCONTINUED, WAIT_MS)?;
    check(continued(status), &format!("WCONTINUED reported {} for a continued child", status_text(status)))?;
    io::close(rel_w)?;
    child.expect_exit(0, "the continued child")
}

/// Two children: `other` moves to its own group and exits; `mine` exits with 7 once
/// released. Polling for 200 ms first gives `other` time to become a zombie.
fn wait_by_group(selector: impl Fn(&Child) -> i32, mine_own_group: bool) -> CaseResult {
    let mut other = Child::start(|| { setpgid(0, 0); 0 })?;
    let (rel_r, rel_w) = io::pipe()?;
    let mut mine = Child::start(|| {
        let _ = io::close(rel_w);
        if mine_own_group { setpgid(0, 0); }
        drain(rel_r);
        7
    })?;
    io::close(rel_r)?;
    if mine_own_group { setpgid(mine.pid, mine.pid); }
    let target = selector(&mine);
    let start = now_ms();
    while now_ms().saturating_sub(start) < 200 {
        let mut status = 0;
        let r = wait4(target, &mut status, WNOHANG);
        if r < 0 { return fail(format!("waitpid({target}) failed with {}", errname(-r))); }
        if r == other.pid as i64 { other.live = false; return fail(format!("waitpid({target}) returned a child in another process group")); }
        if r > 0 { return fail(format!("waitpid({target}) returned {r} before its child exited")); }
        nap();
    }
    io::close(rel_w)?;
    let (got, status) = wait_within(target, 0, WAIT_MS)?;
    if got == other.pid { other.live = false; }
    check(got == mine.pid && exited(status) && exit_code(status) == 7,
        &format!("waitpid({target}) returned {got} with {}, expected {} with exit 7", status_text(status), mine.pid))?;
    mine.live = false;
    other.expect_exit(0, "the child in another group")
}

fn wait_pid_zero() -> CaseResult { wait_by_group(|_| 0, false) }
fn wait_pid_group() -> CaseResult { wait_by_group(|mine| -mine.pid, true) }

fn wait_any_order() -> CaseResult {
    let mut children = Vec::new();
    for code in 1..=3 { children.push(Child::start(move || code)?); }
    for _ in 0..3 {
        let (got, status) = wait_within(-1, 0, WAIT_MS)?;
        let Some(i) = children.iter().position(|c| c.pid == got && c.live) else {
            return fail(format!("waitpid(-1) returned {got}, not an unreaped child"));
        };
        children[i].live = false;
        check(exited(status) && exit_code(status) == i as i32 + 1,
            &format!("waitpid(-1) reported {} for the child that exited {}", status_text(status), i + 1))?;
    }
    let mut status = 0;
    want_err("waitpid(-1) once every child is reaped", wait4(-1, &mut status, WNOHANG), ECHILD)
}

fn wait_zombie() -> CaseResult {
    let (r, w) = io::pipe()?;
    let mut child = Child::start(|| { let _ = io::close(r); 0 })?;
    io::close(w)?;
    let eof = read_to_eof(r, WAIT_MS);
    io::close(r)?;
    eof?;
    let _ = time::sleep_ms(100);
    want_eq("kill(pid, 0) on an exited child not yet waited for", kill(child.pid, 0), 0)?;
    child.expect_exit(0, "the zombie")?;
    want_err("kill(pid, 0) once the child is reaped", kill(child.pid, 0), ESRCH)
}

fn wait_reparent() -> CaseResult {
    let (r, w) = io::pipe()?;
    let (hold_r, hold_w) = io::pipe()?;
    let mut middle = Child::start(|| {
        let _ = io::close(r);
        let me = pid();
        match process::fork() {
            Ok(ForkResult::Child) => {
                let _ = io::close(hold_w);
                drain(hold_r);
                // The middle child has closed its descriptors; reparenting may finish just after.
                let start = now_ms();
                while ppid() == me && now_ms().saturating_sub(start) < 2000 { nap(); }
                let _ = io::write(w, &ppid().to_le_bytes());
                process::exit(0)
            }
            Ok(ForkResult::Parent(_)) => 0,
            Err(_) => 1,
        }
    })?;
    io::close(w)?;
    io::close(hold_r)?;
    io::close(hold_w)?;
    middle.expect_exit(0, "the middle child")?;
    let parent = read_i32(r, "the orphan's new parent");
    io::close(r)?;
    let parent = parent?;
    // POSIX names no PID: the new parent is an implementation-defined system process.
    check(parent > 0 && parent != middle.pid && parent != pid() && kill(parent, 0) == 0,
        &format!("after its parent {} exited, the orphan's parent is {parent}, not a live system process", middle.pid))
}

fn wait_sigchld_ignored() -> CaseResult {
    ignore(SIGCHLD)?;
    let mut child = Child::start(|| 0)?;
    let limit = bounded(WAIT_MS, CLEANUP_MS);
    let start = now_ms();
    loop {
        let mut status = 0;
        let r = wait4(child.pid, &mut status, WNOHANG);
        if r == child.pid as i64 {
            child.live = false;
            return fail("the child was left as a zombie although SIGCHLD is ignored");
        }
        if r == -ECHILD { child.live = false; return Ok(()); }
        if r != 0 { return fail(format!("waitpid failed with {}", shown(r))); }
        if now_ms().saturating_sub(start) >= limit {
            return fail("waitpid never reported ECHILD for an exited child with SIGCHLD ignored");
        }
        nap();
    }
}

const P_PID: u64 = 1;
const WEXITED: u64 = 4;
const WNOWAIT: u64 = 0x0100_0000;

/// waitid(P_PID, pid, WEXITED | extra | WNOHANG) until it reports; the siginfo as words.
fn waitid_within(pid: i32, extra: u64) -> Result<[i32; 32], CaseError> {
    let limit = bounded(WAIT_MS, CLEANUP_MS);
    let start = now_ms();
    loop {
        let mut info = [0i32; 32];
        let r = sc(nr::WAITID, &[P_PID, pid as u64, info.as_mut_ptr() as u64, WEXITED | extra | WNOHANG as u64, 0]);
        if r < 0 { return err(format!("waitid failed with {}", errname(-r))); }
        if info[4] != 0 { return Ok(info); }
        if now_ms().saturating_sub(start) >= limit { return err("waitid reported nothing for an exited child"); }
        nap();
    }
}

/// The fields of a waitid report for `child` that exited with `code`, or what is wrong.
fn waitid_reports(info: &[i32; 32], child: i32, code: i32) -> CaseResult {
    check(info[0] == SIGCHLD && info[2] == 1 && info[4] == child && info[6] == code,
        &format!("waitid gave si_signo {}, si_code {}, si_pid {}, si_status {}; expected SIGCHLD, CLD_EXITED, {child}, {code}",
            info[0], info[2], info[4], info[6]))
}

fn wait_waitid() -> CaseResult {
    let mut child = Child::start(|| 9)?;
    let info = waitid_within(child.pid, 0)?;
    waitid_reports(&info, child.pid, 9)?;
    let mut status = 0;
    let after = wait4(child.pid, &mut status, WNOHANG);
    if after == child.pid as i64 { child.live = false; }
    want_err("waitpid once waitid without WNOWAIT has reported the child", after, ECHILD)?;
    child.live = false;
    Ok(())
}

fn wait_waitid_nowait() -> CaseResult {
    let mut child = Child::start(|| 4)?;
    for _ in 0..2 {
        let info = waitid_within(child.pid, WNOWAIT)?;
        waitid_reports(&info, child.pid, 4)?;
    }
    child.expect_exit(4, "the child waitid looked at twice with WNOWAIT")
}

fn wait_null_status() -> CaseResult {
    let mut child = Child::start(|| 0)?;
    let limit = bounded(WAIT_MS, CLEANUP_MS);
    let start = now_ms();
    loop {
        let r = wait4(child.pid, core::ptr::null_mut(), WNOHANG);
        if r == child.pid as i64 { child.live = false; return Ok(()); }
        if r != 0 { return fail(format!("waitpid with a null status pointer failed with {}", shown(r))); }
        if now_ms().saturating_sub(start) >= limit { return fail("waitpid with a null status pointer never reaped the child"); }
        nap();
    }
}

fn wait_exit_closes() -> CaseResult {
    let (r, w) = io::pipe()?;
    let copy = io::dup(w)?;
    let mut child = Child::start(|| { let _ = io::close(r); 0 })?;
    io::close(w)?;
    io::close(copy)?;
    let eof = read_to_eof(r, WAIT_MS);
    io::close(r)?;
    eof.map_err(|e| format!("the pipe the exited child held: {e}"))?;
    child.expect_exit(0, "the child")
}

// ---------------------------------------------------------------------------
// process groups and sessions

fn groups_inherit() -> CaseResult {
    let group = want("getpgid(0)", getpgid(0))?;
    let session = want("getsid(0)", getsid(0))?;
    want_eq("getpgid(getpid())", getpgid(pid()), group)?;
    let child = held(|| {
        if getpgid(0) != group { return Err("the child is not in its parent's process group".into()); }
        if getsid(0) != session { return Err("the child is not in its parent's session".into()); }
        Ok(())
    })?;
    want_eq("getpgid of the child", getpgid(child.pid()), group)?;
    want_eq("getsid of the child", getsid(child.pid()), session)?;
    child.release()
}

fn groups_setpgid_self() -> CaseResult {
    let child = held(|| {
        let r = setpgid(0, 0);
        if r != 0 { return Err(format!("setpgid(0, 0) failed with {}", shown(r))); }
        if getpgid(0) != pid() as i64 { return Err("after setpgid(0, 0) it does not lead its own group".into()); }
        Ok(())
    })?;
    want_eq("getpgid of the child", getpgid(child.pid()), child.pid() as i64)?;
    child.release()
}

fn groups_setpgid_child() -> CaseResult {
    let child = held(|| Ok(()))?;
    want_eq("setpgid(child, child)", setpgid(child.pid(), child.pid()), 0)?;
    want_eq("getpgid of the child", getpgid(child.pid()), child.pid() as i64)?;
    child.release()
}

fn groups_setpgid_join() -> CaseResult {
    let leader = held(|| Ok(()))?;
    want_eq("setpgid(leader, leader)", setpgid(leader.pid(), leader.pid()), 0)?;
    let member = held(|| Ok(()))?;
    want_eq("setpgid(member, leader's group)", setpgid(member.pid(), leader.pid()), 0)?;
    want_eq("getpgid of the member", getpgid(member.pid()), leader.pid() as i64)?;
    member.release()?;
    leader.release()
}

fn groups_setpgid_esrch() -> CaseResult {
    let (r, w) = io::pipe()?;
    let (rel_r, rel_w) = io::pipe()?;
    let mut middle = Child::start(|| {
        let _ = io::close(r);
        let _ = io::close(rel_w);
        match process::fork() {
            Ok(ForkResult::Child) => { let _ = io::close(w); drain(rel_r); process::exit(0) }
            Ok(ForkResult::Parent(g)) => { let _ = io::write(w, &(g.raw() as i32).to_le_bytes()); drain(rel_r); 0 }
            Err(_) => 1,
        }
    })?;
    io::close(w)?;
    io::close(rel_r)?;
    let grandchild = read_i32(r, "the grandchild's PID");
    io::close(r)?;
    let grandchild = grandchild?;
    let result = want_err("setpgid on a grandchild", setpgid(grandchild, grandchild), ESRCH)
        .and_then(|_| {
            let gone = gone_pid()?;
            want_err("setpgid on a PID with no process", setpgid(gone, gone), ESRCH)
        });
    io::close(rel_w)?;
    middle.expect_exit(0, "the middle child")?;
    result
}

fn groups_setpgid_eacces() -> CaseResult {
    let (rel_r, rel_w) = io::pipe()?;
    let args = strings(&["processes-exec", "hold", &rel_r.raw().to_string()]);
    let started = spawn_exec(HELPER, &args, &[], || { let _ = io::close(rel_w); Ok(()) });
    io::close(rel_r)?;
    let (mut child, errno) = match started {
        Ok(started) => started,
        Err(e) => { let _ = io::close(rel_w); return Err(e); }
    };
    let r = setpgid(child.pid, child.pid);
    io::close(rel_w)?;
    if let Some(errno) = errno { return fail(format!("exec of the helper failed with {}", errname(errno))); }
    child.expect_exit(0, "the exec'd child")?;
    want_err("setpgid on a child that has called exec", r, EACCES)
}

fn groups_setpgid_session_leader() -> CaseResult {
    let group = getpgid(0) as i32;
    task(move || {
        let s = setsid();
        if s < 0 { return Err(format!("setsid failed with {}", errname(-s))); }
        let a = setpgid(0, 0);
        if a != -EPERM { return Err(format!("setpgid(0, 0) by a session leader: expected EPERM, got {}", shown(a))); }
        let b = setpgid(0, group);
        if b != -EPERM { return Err(format!("setpgid into another group by a session leader: expected EPERM, got {}", shown(b))); }
        Ok(())
    })?.finish()
}

fn groups_setpgid_other_session() -> CaseResult {
    let other = held(|| {
        let s = setsid();
        if s < 0 { Err(format!("setsid failed with {}", errname(-s))) } else { Ok(()) }
    })?;
    let mine = held(|| Ok(()))?;
    want_err("setpgid on a child in another session", setpgid(other.pid(), other.pid()), EPERM)?;
    want_err("setpgid into a group in another session", setpgid(mine.pid(), other.pid()), EPERM)?;
    mine.release()?;
    other.release()
}

fn groups_setpgid_no_group() -> CaseResult {
    let gone = gone_pid()?;
    let child = held(|| Ok(()))?;
    want_err("setpgid into a process group that does not exist", setpgid(child.pid(), gone), EPERM)?;
    child.release()
}

fn groups_setpgid_einval() -> CaseResult {
    let child = held(|| Ok(()))?;
    want_err("setpgid with a negative process group", setpgid(child.pid(), -5), EINVAL)?;
    child.release()
}

fn groups_setsid() -> CaseResult {
    let child = held(|| {
        let me = pid() as i64;
        let s = setsid();
        if s != me { return Err(format!("setsid returned {} in process {me}", shown(s))); }
        if getsid(0) != me || getpgid(0) != me { return Err("after setsid it does not lead its session and group".into()); }
        Ok(())
    })?;
    want_eq("getsid of the child", getsid(child.pid()), child.pid() as i64)?;
    want_eq("getpgid of the child", getpgid(child.pid()), child.pid() as i64)?;
    child.release()
}

fn groups_setsid_leader() -> CaseResult {
    task(|| {
        let r = setpgid(0, 0);
        if r != 0 { return Err(format!("setpgid(0, 0) failed with {}", shown(r))); }
        let s = setsid();
        if s != -EPERM { return Err(format!("setsid by a process group leader: expected EPERM, got {}", shown(s))); }
        Ok(())
    })?.finish()?;
    task(|| {
        let s = setsid();
        if s < 0 { return Err(format!("setsid failed with {}", errname(-s))); }
        let again = setsid();
        if again != -EPERM { return Err(format!("a second setsid by a session leader: expected EPERM, got {}", shown(again))); }
        Ok(())
    })?.finish()
}

fn groups_lookup_esrch() -> CaseResult {
    let gone = gone_pid()?;
    want_err("getpgid of a PID with no process", getpgid(gone), ESRCH)?;
    want_err("getsid of a PID with no process", getsid(gone), ESRCH)
}

fn groups_outlives_leader() -> CaseResult {
    let (r, w) = io::pipe()?;
    let (rel_r, rel_w) = io::pipe()?;
    let mut leader = Child::start(|| {
        let _ = io::close(r);
        let _ = io::close(rel_w);
        if setpgid(0, 0) != 0 { return 1; }
        match process::fork() {
            Ok(ForkResult::Child) => { let _ = io::close(w); drain(rel_r); process::exit(0) }
            Ok(ForkResult::Parent(m)) => { let _ = io::write(w, &(m.raw() as i32).to_le_bytes()); 0 }
            Err(_) => 2,
        }
    })?;
    io::close(w)?;
    io::close(rel_r)?;
    let member = read_i32(r, "the member's PID");
    io::close(r)?;
    let member = Stray(member?);
    let result = leader.expect_exit(0, "the group leader").and_then(|_| {
        want_eq("getpgid of the member once its leader has exited", getpgid(member.0), leader.pid as i64)?;
        let child = held(|| Ok(()))?;
        want_eq("setpgid into the group of a leader that has exited", setpgid(child.pid(), leader.pid), 0)?;
        child.release()
    });
    io::close(rel_w)?;
    result
}

static REPORT_FD: AtomicI32 = AtomicI32::new(-1);
extern "C" fn note_hup(_: i32) { sc(nr::WRITE, &[REPORT_FD.load(Ordering::Relaxed) as u64, b"H".as_ptr() as u64, 1]); }
extern "C" fn note_cont(_: i32) { sc(nr::WRITE, &[REPORT_FD.load(Ordering::Relaxed) as u64, b"C".as_ptr() as u64, 1]); }

fn groups_orphaned_stopped() -> CaseResult {
    // A session leader starts a member in a new group; the member stops itself; the
    // leader exits once the stop is seen. That orphans a group with a stopped member,
    // which must then be sent SIGHUP and SIGCONT.
    let (r, w) = io::pipe()?;
    let mut leader = Child::start(|| {
        let _ = io::close(r);
        if setsid() < 0 { return 1; }
        let member = match process::fork() {
            Ok(ForkResult::Child) => {
                REPORT_FD.store(w.raw() as i32, Ordering::Relaxed);
                let ready = setpgid(0, 0) == 0
                    && signal::sigaction(SIGHUP, Some(&Sigaction::new(note_hup)), None).is_ok()
                    && signal::sigaction(SIGCONT, Some(&Sigaction::new(note_cont)), None).is_ok();
                if !ready { process::exit(1) }
                let _ = io::write(w, &pid().to_le_bytes());
                kill(pid(), SIGSTOP);
                let _ = time::sleep_ms(100);
                process::exit(0)
            }
            Ok(ForkResult::Parent(m)) => m.raw() as i32,
            Err(_) => return 2,
        };
        match wait_within(member, WUNTRACED, 2000) {
            Ok((_, status)) if stopped(status) => 0,
            _ => 3,
        }
    })?;
    io::close(w)?;
    let member = read_i32(r, "the member's PID");
    let _member = match member {
        Ok(member) => Stray(member),
        Err(e) => { let _ = io::close(r); return Err(e); }
    };
    let status = leader.wait();
    let events = read_to_eof(r, WAIT_MS);
    io::close(r)?;
    let status = status?;
    match (exited(status), exit_code(status)) {
        (true, 0) => {}
        (true, 1) => return fail("setsid failed in the would-be session leader"),
        (true, 2) => return fail("fork failed in the session leader"),
        (true, 3) => return fail("the member's stop was never reported to WUNTRACED, so its group could not be orphaned while it was stopped"),
        _ => return fail(format!("the session leader ended with {}", status_text(status))),
    }
    match events {
        Ok(events) if events.contains(&b'H') && events.contains(&b'C') => Ok(()),
        Ok(events) => fail(format!("the orphaned group's stopped member saw only {:?} of SIGHUP and SIGCONT",
            String::from_utf8_lossy(&events))),
        Err(_) => fail("the orphaned group's stopped member was never sent SIGHUP and SIGCONT: it is still stopped"),
    }
}

// ---------------------------------------------------------------------------
// user and group IDs

fn creds_root() -> CaseResult { ids_are([0, 0, 0, 0], "the suite") }

fn creds_setuid_root() -> CaseResult {
    want_eq("setgroups", setgroups(&[]), 0)?;
    want_eq("setuid(4101) as root", setuid(4101), 0)?;
    ids_are([4101, 4101, 0, 0], "after setuid(4101) as root")?;
    want_err("setuid(0) after root changed its user ID with setuid", setuid(0), EPERM)
}

fn creds_setuid_user() -> CaseResult {
    become_user(4102, 4102)?;
    want_err("setuid to another user as non-root", setuid(4103), EPERM)?;
    want_err("setuid(0) as non-root", setuid(0), EPERM)?;
    want_eq("setuid to its own user ID", setuid(4102), 0)
}

fn creds_setuid_saved() -> CaseResult {
    // The saved set-user-ID takes the new effective user ID, 4105.
    want_eq("setreuid(4104, 4105) as root", setreuid(4104, 4105), 0)?;
    want_eq("setuid to the real user ID as non-root", setuid(4104), 0)?;
    ids_are([4104, 4104, 0, 0], "after setuid to the real user ID")?;
    want_eq("setuid back to the saved set-user-ID", setuid(4105), 0)?;
    ids_are([4104, 4105, 0, 0], "after setuid back to the saved set-user-ID")
}

fn creds_setgid_root() -> CaseResult {
    want_eq("setgid(4201) as root", setgid(4201), 0)?;
    ids_are([0, 0, 4201, 4201], "after setgid(4201) as root")?;
    want_eq("setgid(0) while still root", setgid(0), 0)?;
    ids_are([0, 0, 0, 0], "after setgid(0)")
}

fn creds_setgid_user() -> CaseResult {
    become_user(4202, 4202)?;
    want_err("setgid to another group as non-root", setgid(4203), EPERM)?;
    want_eq("setgid to its own group ID", setgid(4202), 0)
}

fn creds_setgid_saved() -> CaseResult {
    // The saved set-group-ID takes the new effective group ID, 4107; setuid then gives up root.
    want_eq("setregid(4106, 4107) as root", setregid(4106, 4107), 0)?;
    want_eq("setuid(4108) as root", setuid(4108), 0)?;
    want_eq("setgid to the real group ID as non-root", setgid(4106), 0)?;
    ids_are([4108, 4108, 4106, 4106], "after setgid to the real group ID")?;
    want_eq("setgid back to the saved set-group-ID", setgid(4107), 0)?;
    ids_are([4108, 4108, 4106, 4107], "after setgid back to the saved set-group-ID")
}

fn creds_setgid_after_setuid() -> CaseResult {
    want_eq("setuid(4204) as root", setuid(4204), 0)?;
    want_err("setgid after giving up root with setuid", setgid(4204), EPERM)
}

fn creds_setreuid_root() -> CaseResult {
    want_eq("setreuid(4301, 4302) as root", setreuid(4301, 4302), 0)?;
    ids_are([4301, 4302, 0, 0], "after setreuid(4301, 4302)")
}

fn creds_setreuid_temporary() -> CaseResult {
    want_eq("setreuid(-1, 4303) as root", setreuid(KEEP, 4303), 0)?;
    ids_are([0, 4303, 0, 0], "after setreuid(-1, 4303)")?;
    want_eq("setreuid(-1, -1)", setreuid(KEEP, KEEP), 0)?;
    ids_are([0, 4303, 0, 0], "after setreuid(-1, -1)")?;
    want_eq("setreuid(-1, 0) to take root back", setreuid(KEEP, 0), 0)?;
    ids_are([0, 0, 0, 0], "after setreuid(-1, 0)")
}

fn creds_setreuid_permanent() -> CaseResult {
    want_eq("setreuid(4304, 4304) as root", setreuid(4304, 4304), 0)?;
    want_err("setreuid(-1, 0) after dropping root for good", setreuid(KEEP, 0), EPERM)?;
    want_err("setuid(0) after dropping root for good", setuid(0), EPERM)
}

fn creds_setreuid_swap() -> CaseResult {
    want_eq("setreuid(4305, 4306) as root", setreuid(4305, 4306), 0)?;
    want_eq("swapping the real and effective user IDs as non-root", setreuid(4306, 4305), 0)?;
    ids_are([4306, 4305, 0, 0], "after the swap")
}

fn creds_setreuid_user() -> CaseResult {
    become_user(4307, 4307)?;
    want_err("setreuid to another real user ID as non-root", setreuid(4308, KEEP), EPERM)?;
    want_err("setreuid to another effective user ID as non-root", setreuid(KEEP, 4308), EPERM)
}

fn creds_setregid_root() -> CaseResult {
    want_eq("setregid(4401, 4402) as root", setregid(4401, 4402), 0)?;
    ids_are([0, 0, 4401, 4402], "after setregid(4401, 4402)")
}

fn creds_setregid_swap() -> CaseResult {
    want_eq("setregid(4403, 4404) as root", setregid(4403, 4404), 0)?;
    want_eq("setuid(4400) as root", setuid(4400), 0)?;
    want_eq("swapping the real and effective group IDs as non-root", setregid(4404, 4403), 0)?;
    ids_are([4400, 4400, 4404, 4403], "after the swap")
}

fn creds_setregid_user() -> CaseResult {
    become_user(4405, 4405)?;
    want_err("setregid to another real group ID as non-root", setregid(4406, KEEP), EPERM)?;
    want_err("setregid to another effective group ID as non-root", setregid(KEEP, 4406), EPERM)
}

fn creds_getgroups() -> CaseResult {
    want_eq("setgroups of three groups", setgroups(&[4501, 4502, 4503]), 0)?;
    let count = want("getgroups(0, NULL)", sc(nr::GETGROUPS, &[0, 0]))?;
    let mut list = [0u32; 8];
    want_eq("getgroups(8, list)", getgroups(&mut list), count)?;
    let got = &list[..(count as usize).min(list.len())];
    check(groups_are(got, &[4501, 4502, 4503], 0),
        &format!("getgroups returned {got:?} after setgroups of 4501, 4502 and 4503"))?;
    let mut small = [0u32; 2];
    want_err("getgroups with a list shorter than its count", getgroups(&mut small), EINVAL)
}

fn creds_setgroups_user() -> CaseResult {
    become_user(4504, 4504)?;
    want_err("setgroups as non-root", setgroups(&[4505]), EPERM)
}

fn creds_fork() -> CaseResult {
    want_eq("setgroups", setgroups(&[4601, 4602]), 0)?;
    want_eq("setgid", setgid(4603), 0)?;
    want_eq("setuid", setuid(4604), 0)?;
    task(|| {
        let got = ids();
        if got != [4604, 4604, 4603, 4603] { return Err(format!("the child's IDs are {got:?}")); }
        let mut list = [0u32; 8];
        let n = getgroups(&mut list);
        if n < 0 { return Err(format!("getgroups in the child failed with {}", errname(-n))); }
        let got = &list[..n as usize];
        if !groups_are(got, &[4601, 4602], 4603) { return Err(format!("the child's supplementary groups are {got:?}, expected 4601 and 4602")); }
        Ok(())
    })?.finish()
}

// ---------------------------------------------------------------------------
// resource limits and usage

fn limits_getrlimit() -> CaseResult {
    let [soft, hard] = getrlimit(RLIMIT_NOFILE)?;
    check(soft <= hard && soft >= 20, &format!("RLIMIT_NOFILE is soft {soft}, hard {hard}"))?;
    for (name, resource) in [("RLIMIT_CORE", RLIMIT_CORE), ("RLIMIT_CPU", RLIMIT_CPU), ("RLIMIT_DATA", RLIMIT_DATA),
        ("RLIMIT_FSIZE", RLIMIT_FSIZE), ("RLIMIT_STACK", RLIMIT_STACK), ("RLIMIT_AS", RLIMIT_AS)] {
        let [soft, hard] = getrlimit(resource).map_err(|e| format!("{name}: {e}"))?;
        check(soft <= hard, &format!("{name} is soft {soft}, above hard {hard}"))?;
    }
    Ok(())
}

fn limits_getrlimit_einval() -> CaseResult {
    let mut v = [0u64; 2];
    want_err("getrlimit of an unknown resource", prlimit(0, 1000, None, Some(&mut v)), EINVAL)
}

fn limits_setrlimit_soft() -> CaseResult {
    let [_, hard] = getrlimit(RLIMIT_NOFILE)?;
    want_eq("setrlimit(RLIMIT_NOFILE, 64)", setrlimit(RLIMIT_NOFILE, 64, hard), 0)?;
    limit_is(RLIMIT_NOFILE, [64, hard], "after lowering the soft limit")?;
    Ok(())
}

fn limits_soft_above_hard() -> CaseResult {
    want_err("setrlimit with the soft limit above the hard", setrlimit(RLIMIT_NOFILE, 200, 100), EINVAL)
}

fn limits_root_hard() -> CaseResult {
    want_eq("lowering the hard limit", setrlimit(RLIMIT_NOFILE, 256, 512), 0)?;
    want_eq("raising the hard limit as root", setrlimit(RLIMIT_NOFILE, 256, 1024), 0)?;
    limit_is(RLIMIT_NOFILE, [256, 1024], "after root raised the hard limit")?;
    Ok(())
}

fn limits_user_hard() -> CaseResult {
    want_eq("lowering the hard limit", setrlimit(RLIMIT_NOFILE, 256, 512), 0)?;
    become_user(4701, 4701)?;
    want_err("raising the hard limit as non-root", setrlimit(RLIMIT_NOFILE, 256, 1024), EPERM)?;
    limit_is(RLIMIT_NOFILE, [256, 512], "after the refused raise")?;
    Ok(())
}

fn limits_user_soft() -> CaseResult {
    want_eq("lowering the limits", setrlimit(RLIMIT_NOFILE, 64, 512), 0)?;
    become_user(4702, 4702)?;
    want_eq("raising the soft limit to the hard as non-root", setrlimit(RLIMIT_NOFILE, 512, 512), 0)?;
    limit_is(RLIMIT_NOFILE, [512, 512], "after raising the soft limit")?;
    Ok(())
}

fn limits_nofile_open() -> CaseResult {
    let [_, hard] = getrlimit(RLIMIT_NOFILE)?;
    want_eq("setrlimit(RLIMIT_NOFILE, 16)", setrlimit(RLIMIT_NOFILE, 16, hard), 0)?;
    for _ in 0..64 {
        match fs::open("/dev/null", fs::O_RDONLY) {
            Ok(fd) if fd.raw() < 16 => {}
            Ok(fd) => return fail(format!("open returned descriptor {} with RLIMIT_NOFILE at 16", fd.raw())),
            Err(Error::Os(Errno::EMFILE)) => {
                // open returns the lowest free descriptor, so by now every one below 16 is in use.
                let free: Vec<u64> = (0..16).filter(|&fd| sc(nr::FCNTL, &[fd, F_GETFD]) == -EBADF).collect();
                return check(free.is_empty(),
                    &format!("open failed with EMFILE while descriptors {free:?}, below RLIMIT_NOFILE at 16, were free"));
            }
            Err(e) => return fail(format!("open failed with {e}, expected EMFILE")),
        }
    }
    fail("64 opens succeeded with RLIMIT_NOFILE at 16")
}

fn limits_nofile_dup() -> CaseResult {
    let [_, hard] = getrlimit(RLIMIT_NOFILE)?;
    want_eq("setrlimit(RLIMIT_NOFILE, 16)", setrlimit(RLIMIT_NOFILE, 16, hard), 0)?;
    want_err("dup2 to descriptor 16 with RLIMIT_NOFILE at 16", sc(nr::DUP2, &[0, 16, 0]), EBADF)?;
    want_err("F_DUPFD from 16 with RLIMIT_NOFILE at 16", sc(nr::FCNTL, &[0, 0, 16]), EINVAL)?;
    want_eq("F_DUPFD from 15", sc(nr::FCNTL, &[0, 0, 15]), 15)
}

fn limits_fsize() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = tmp.path("big");
    let fd = fs::open_with_mode(&path, fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY, 0o600)?;
    ignore(SIGXFSZ)?;
    want_eq("setrlimit(RLIMIT_FSIZE, 4096)", setrlimit(RLIMIT_FSIZE, 4096, RLIM_INFINITY), 0)?;
    let data = [0x42u8; 8192];
    let first = sc(nr::WRITE, &[fd.raw(), data.as_ptr() as u64, 8192]);
    want_eq("a write of 8192 bytes from offset 0 with RLIMIT_FSIZE at 4096", first, 4096)?;
    want_err("a write at the RLIMIT_FSIZE limit", sc(nr::WRITE, &[fd.raw(), data.as_ptr() as u64, 10]), EFBIG)?;
    let size = fs::fstat(fd)?.st_size;
    io::close(fd)?;
    check(size == 4096, &format!("the file grew to {size} bytes with RLIMIT_FSIZE at 4096"))
}

fn limits_fsize_signal() -> CaseResult {
    let tmp = Tmp::new()?;
    let path = tmp.path("big");
    let mut child = Child::start(|| {
        let Ok(fd) = fs::open_with_mode(&path, fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY, 0o600) else { return 1 };
        if setrlimit(RLIMIT_FSIZE, 4096, RLIM_INFINITY) != 0 { return 2; }
        let data = [0x42u8; 8192];
        sc(nr::WRITE, &[fd.raw(), data.as_ptr() as u64, 8192]);
        sc(nr::WRITE, &[fd.raw(), data.as_ptr() as u64, 8192]);
        3
    })?;
    let status = child.wait()?;
    check(signaled(status) && term_sig(status) == SIGXFSZ,
        &format!("writing past RLIMIT_FSIZE with SIGXFSZ at its default ended the child with {}", status_text(status)))
}

fn limits_cpu() -> CaseResult {
    let mut child = Child::start(|| {
        if setrlimit(RLIMIT_CPU, 1, RLIM_INFINITY) != 0 { return 1; }
        burn(4000);
        0
    })?;
    let (_, status) = wait_within(child.pid, 0, 6000)?;
    child.live = false;
    check(!(exited(status) && exit_code(status) == 1), "setrlimit(RLIMIT_CPU, 1 s) failed in the child")?;
    check(signaled(status) && term_sig(status) == SIGXCPU,
        &format!("a child with RLIMIT_CPU at 1 s that computed for 4 s ended with {}", status_text(status)))
}

fn limits_data() -> CaseResult {
    let layout = std::alloc::Layout::from_size_align(16 << 20, 16).map_err(|e| e.to_string())?;
    // SAFETY: a nonzero size; the block is freed at once.
    let block = unsafe { std::alloc::alloc(layout) };
    check(!block.is_null(), "a 16 MiB allocation failed with RLIMIT_DATA at its default")?;
    unsafe { std::alloc::dealloc(block, layout) };
    task(move || {
        let r = setrlimit(RLIMIT_DATA, 4 << 20, RLIM_INFINITY);
        if r != 0 { return Err(format!("setrlimit(RLIMIT_DATA) failed with {}", shown(r))); }
        // SAFETY: as above; it is freed if it was made.
        let block = unsafe { std::alloc::alloc(layout) };
        if block.is_null() { return Ok(()); }
        unsafe { std::alloc::dealloc(block, layout) };
        Err("a 16 MiB allocation succeeded with RLIMIT_DATA at 4 MiB".into())
    })?.finish()
}

/// Use about `kib` KiB of stack, in 16 KiB frames.
#[inline(never)]
fn deep(kib: usize) -> u8 {
    let frame = core::hint::black_box([kib as u8; 16384]);
    if kib <= 16 { frame[0] } else { deep(kib - 16).wrapping_add(frame[kib % 16384]) }
}

fn limits_stack() -> CaseResult {
    const DEPTH_KIB: usize = 1024;
    let [soft, _] = getrlimit(RLIMIT_STACK)?;
    if soft < 2 * 1024 * DEPTH_KIB as u64 {
        return skip(format!("RLIMIT_STACK is {soft}, too small for the {DEPTH_KIB} KiB the case uses under it"));
    }
    // Default SIGSEGV handling, so an overflow is reported as SIGSEGV whatever the runtime installed.
    let run = |limit: Option<u64>| -> i32 {
        if signal::sigaction(SIGSEGV, Some(&Sigaction::default()), None).is_err() { return 1; }
        if let Some(limit) = limit {
            if setrlimit(RLIMIT_STACK, limit, RLIM_INFINITY) != 0 { return 2; }
        }
        core::hint::black_box(deep(DEPTH_KIB));
        0
    };
    Child::start(|| run(None))?.expect_exit(0,
        &format!("a child using {DEPTH_KIB} KiB of stack with RLIMIT_STACK at {soft}"))?;
    let mut child = Child::start(|| run(Some(256 << 10)))?;
    let status = child.wait()?;
    check(signaled(status) && term_sig(status) == SIGSEGV,
        &format!("a child using {DEPTH_KIB} KiB of stack with RLIMIT_STACK at 256 KiB ended with {}", status_text(status)))
}

fn limits_as() -> CaseResult {
    // A mapping larger than the limit by itself, whatever the process already uses.
    const LEN: usize = 32 << 20;
    let map = |len: usize| memory::mmap(core::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    let unlimited = map(LEN).map_err(|e| format!("a 32 MiB mapping failed with RLIMIT_AS at its default: {e}"))?;
    memory::munmap(unlimited, LEN)?;
    task(|| {
        let r = setrlimit(RLIMIT_AS, 16 << 20, RLIM_INFINITY);
        if r != 0 { return Err(format!("setrlimit(RLIMIT_AS) failed with {}", shown(r))); }
        match map(LEN) {
            Err(Error::Os(Errno::ENOMEM)) => Ok(()),
            Err(e) => Err(format!("a 32 MiB mapping with RLIMIT_AS at 16 MiB failed with {e}, expected ENOMEM")),
            Ok(_) => Err("a 32 MiB mapping succeeded with RLIMIT_AS at 16 MiB".into()),
        }
    })?.finish()
}

fn limits_core() -> CaseResult {
    want_eq("setrlimit(RLIMIT_CORE, 1 MiB)", setrlimit(RLIMIT_CORE, 1 << 20, RLIM_INFINITY), 0)?;
    limit_is(RLIMIT_CORE, [1 << 20, RLIM_INFINITY], "after setting RLIMIT_CORE to 1 MiB")?;
    want_eq("setrlimit(RLIMIT_CORE, 0)", setrlimit(RLIMIT_CORE, 0, RLIM_INFINITY), 0)?;
    limit_is(RLIMIT_CORE, [0, RLIM_INFINITY], "after setting RLIMIT_CORE to 0")?;
    let tmp = Tmp::new()?;
    let core_file = tmp.path("core");
    let dir = tmp.dir.clone();
    let mut child = Child::start(|| {
        if chdir(&dir) != 0 { return 1; }
        kill(pid(), SIGQUIT);
        loop { nap(); }
    })?;
    let status = child.wait()?;
    check(signaled(status) && term_sig(status) == SIGQUIT,
        &format!("a child that sent itself SIGQUIT ended with {}", status_text(status)))?;
    check(std::fs::metadata(&core_file).is_err(), "a core file was written with RLIMIT_CORE at 0")
}

fn limits_fork() -> CaseResult {
    want_eq("setrlimit(RLIMIT_NOFILE, 100, 200)", setrlimit(RLIMIT_NOFILE, 100, 200), 0)?;
    task(|| limit_is(RLIMIT_NOFILE, [100, 200], "the inherited RLIMIT_NOFILE"))?.finish()
}

fn limits_exec() -> CaseResult {
    let state = exec_state(&[], || {
        let r = setrlimit(RLIMIT_NOFILE, 100, 200);
        if r == 0 { Ok(()) } else { Err(format!("setrlimit failed with {}", shown(r))) }
    })?;
    state_is(&state, "nofile", "100,200", "RLIMIT_NOFILE")
}

fn limits_prlimit_child() -> CaseResult {
    let (rel_r, rel_w) = io::pipe()?;
    let mut child = Child::start(|| {
        let _ = io::close(rel_w);
        drain(rel_r);
        match getrlimit(RLIMIT_NOFILE) { Ok([50, 60]) => 0, _ => 1 }
    })?;
    io::close(rel_r)?;
    let set = prlimit(child.pid, RLIMIT_NOFILE, Some([50, 60]), None);
    let mut seen = [0u64; 2];
    let get = prlimit(child.pid, RLIMIT_NOFILE, None, Some(&mut seen));
    io::close(rel_w)?;
    want_eq("prlimit setting a child's RLIMIT_NOFILE", set, 0)?;
    want_eq("prlimit reading a child's RLIMIT_NOFILE", get, 0)?;
    check(seen == [50, 60], &format!("prlimit read the child's limit as {seen:?}, expected [50, 60]"))?;
    child.expect_exit(0, "the child, checking its own limit,")
}

fn limits_prlimit_swap() -> CaseResult {
    want_eq("setrlimit(RLIMIT_NOFILE, 70, 300)", setrlimit(RLIMIT_NOFILE, 70, 300), 0)?;
    let mut old = [0u64; 2];
    want_eq("prlimit with a new and an old limit", prlimit(0, RLIMIT_NOFILE, Some([80, 300]), Some(&mut old)), 0)?;
    check(old == [70, 300], &format!("prlimit returned the old limit as {old:?}, expected [70, 300]"))?;
    limit_is(RLIMIT_NOFILE, [80, 300], "after prlimit installed the new limit")?;
    Ok(())
}

fn limits_prlimit_errors() -> CaseResult {
    let gone = gone_pid()?;
    let mut v = [0u64; 2];
    want_err("prlimit on a PID with no process", prlimit(gone, RLIMIT_NOFILE, None, Some(&mut v)), ESRCH)?;
    let target = held(|| Ok(()))?;
    become_user(4703, 4703)?;
    want_err("prlimit on another user's process as non-root", prlimit(target.pid(), RLIMIT_NOFILE, Some([10, 10]), None), EPERM)?;
    target.release()
}

fn limits_rusage_self() -> CaseResult {
    let before = getrusage(0)?;
    burn(200);
    let after = getrusage(0)?;
    check(cpu_us(&after) > cpu_us(&before) && cpu_us(&after) > 0,
        &format!("RUSAGE_SELF CPU time went from {} us to {} us across 200 ms of computation", cpu_us(&before), cpu_us(&after)))
}

fn limits_rusage_children() -> CaseResult {
    let before = getrusage(-1)?;
    check(cpu_us(&before) == 0, &format!("RUSAGE_CHILDREN is {} us before any child ran", cpu_us(&before)))?;
    let (r, w) = io::pipe()?;
    let mut child = Child::start(|| { let _ = io::close(r); burn(200); 0 })?;
    io::close(w)?;
    let eof = read_to_eof(r, WAIT_MS);
    io::close(r)?;
    eof?;
    let _ = time::sleep_ms(50);
    let unwaited = getrusage(-1)?;
    child.expect_exit(0, "the child")?;
    let waited = getrusage(-1)?;
    check(cpu_us(&unwaited) == 0, "RUSAGE_CHILDREN counted a child that had not been waited for")?;
    check(cpu_us(&waited) > 0, "RUSAGE_CHILDREN does not count a waited-for child's 200 ms of computation")
}

fn limits_rusage_einval() -> CaseResult {
    let mut usage = [0i64; 18];
    want_err("getrusage with an unknown who", sc(nr::GETRUSAGE, &[7, usage.as_mut_ptr() as u64]), EINVAL)
}

fn limits_times() -> CaseResult {
    let (start, before) = times()?;
    burn(200);
    let (end, after) = times()?;
    check(end > start, &format!("times() elapsed time went from {start} to {end} across 200 ms"))?;
    check(after[0] + after[1] > before[0] + before[1],
        &format!("tms_utime+tms_stime went from {} to {} across 200 ms of computation", before[0] + before[1], after[0] + after[1]))
}

fn limits_times_children() -> CaseResult {
    let (_, before) = times()?;
    check(before[2] + before[3] == 0, "tms_cutime+tms_cstime is not 0 before any child ran")?;
    task(|| { burn(200); Ok(()) })?.finish()?;
    let (_, after) = times()?;
    check(after[2] + after[3] > 0, "tms_cutime+tms_cstime does not count a waited-for child's 200 ms of computation")
}

// ---------------------------------------------------------------------------
// priorities and sched_yield

/// Processors online, from the `processor` lines of /proc/cpuinfo; 1 if it cannot be read.
fn processors() -> usize {
    std::fs::read_to_string("/proc/cpuinfo")
        .map(|info| info.lines().filter(|line| line.starts_with("processor")).count())
        .unwrap_or(0)
        .max(1)
}

/// A ring of `n` processes passes a token: each spins until the token reaches it and
/// hands it on, calling sched_yield between looks when `yielding`. Returns the passes
/// made in `window` ms.
fn ring_passes(n: usize, yielding: bool, window: u64) -> Result<u64, CaseError> {
    const STOP: u64 = u64::MAX;
    let map = memory::mmap(core::ptr::null_mut(), 4096, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0)?;
    // SAFETY: the shared mapping is page-aligned, zero-filled and outlives the ring.
    let token = unsafe { &*(map as *const AtomicU64) };
    let mut ring = Vec::new();
    for i in 0..n as u64 {
        ring.push(Child::start(|| loop {
            let t = token.load(Ordering::Acquire);
            if t == STOP { return 0; }
            if t % n as u64 == i { let _ = token.compare_exchange(t, t + 1, Ordering::AcqRel, Ordering::Acquire); }
            else if yielding { sc(nr::SCHED_YIELD, &[]); }
            else { core::hint::spin_loop(); }
        })?);
    }
    let _ = time::sleep_ms(50);
    let first = token.load(Ordering::Acquire);
    let _ = time::sleep_ms(window);
    let passes = token.load(Ordering::Acquire) - first;
    token.store(STOP, Ordering::Release);
    for child in ring.iter_mut() { child.expect_exit(0, "a process of the ring")?; }
    memory::munmap(map, 4096)?;
    Ok(passes)
}

fn sched_yield() -> CaseResult {
    for _ in 0..16 { want_eq("sched_yield", sc(nr::SCHED_YIELD, &[]), 0)?; }
    // More spinning processes than processors, so a token holder is often waiting for one:
    // sched_yield should hand it the processor, where a bare spin holds it until preempted.
    let n = (4 * processors()).max(8);
    let spinning = ring_passes(n, false, 300)?;
    let yielding = ring_passes(n, true, 300)?;
    check(yielding > 4 * spinning.max(1),
        &format!("a ring of {n} processes passed its token {yielding} times in 300 ms with sched_yield and {spinning} times spinning without it"))
}

fn sched_getpriority() -> CaseResult {
    let own = getpriority(PRIO_PROCESS, 0).map_err(|e| format!("getpriority failed with {}", errname(e)))?;
    check((-20..=19).contains(&own), &format!("getpriority returned nice value {own}"))?;
    nice_is(PRIO_PROCESS, pid(), own, "getpriority by PID")
}

fn sched_setpriority() -> CaseResult {
    want_eq("setpriority(PRIO_PROCESS, 0, 10)", setpriority(PRIO_PROCESS, 0, 10), 0)?;
    nice_is(PRIO_PROCESS, 0, 10, "after setpriority to 10")
}

fn sched_clamp() -> CaseResult {
    want_eq("setpriority to 100", setpriority(PRIO_PROCESS, 0, 100), 0)?;
    nice_is(PRIO_PROCESS, 0, 19, "after setpriority to 100")?;
    want_eq("setpriority to -100 as root", setpriority(PRIO_PROCESS, 0, -100), 0)?;
    nice_is(PRIO_PROCESS, 0, -20, "after setpriority to -100")
}

fn sched_root_lowers() -> CaseResult {
    want_eq("setpriority to 5", setpriority(PRIO_PROCESS, 0, 5), 0)?;
    want_eq("setpriority to -5 as root", setpriority(PRIO_PROCESS, 0, -5), 0)?;
    nice_is(PRIO_PROCESS, 0, -5, "after root lowered its nice value")
}

fn sched_user_raise() -> CaseResult {
    want_eq("setpriority to 5", setpriority(PRIO_PROCESS, 0, 5), 0)?;
    become_user(4801, 4801)?;
    want_eq("raising its own nice value as non-root", setpriority(PRIO_PROCESS, 0, 10), 0)?;
    want_err("lowering its own nice value as non-root", setpriority(PRIO_PROCESS, 0, 3), EACCES)?;
    nice_is(PRIO_PROCESS, 0, 10, "after the refused change")
}

fn sched_other_user() -> CaseResult {
    let target = held(|| Ok(()))?;
    become_user(4802, 4802)?;
    want_err("setpriority on another user's process as non-root", setpriority(PRIO_PROCESS, target.pid(), 15), EPERM)?;
    target.release()
}

fn sched_errors() -> CaseResult {
    let gone = gone_pid()?;
    want_err("setpriority on a PID with no process", setpriority(PRIO_PROCESS, gone, 5), ESRCH)?;
    check(getpriority(PRIO_PROCESS, gone) == Err(ESRCH), "getpriority on a PID with no process did not fail with ESRCH")?;
    want_err("setpriority with an unknown which", setpriority(99, 0, 5), EINVAL)?;
    check(getpriority(99, 0) == Err(EINVAL), "getpriority with an unknown which did not fail with EINVAL")
}

fn sched_child() -> CaseResult {
    let (rel_r, rel_w) = io::pipe()?;
    let mut child = Child::start(|| {
        let _ = io::close(rel_w);
        drain(rel_r);
        match getpriority(PRIO_PROCESS, 0) { Ok(n) => (n + 20) as i32, Err(_) => 100 }
    })?;
    io::close(rel_r)?;
    let set = setpriority(PRIO_PROCESS, child.pid, 7);
    let seen = getpriority(PRIO_PROCESS, child.pid);
    io::close(rel_w)?;
    want_eq("setpriority on a child", set, 0)?;
    check(seen == Ok(7), &format!("getpriority on the child returned {seen:?}, expected 7"))?;
    child.expect_exit(27, "the child, reporting its nice value plus 20,")
}

fn sched_group() -> CaseResult {
    let leader = held(|| Ok(()))?;
    want_eq("setpgid(leader, leader)", setpgid(leader.pid(), leader.pid()), 0)?;
    let member = held(|| Ok(()))?;
    want_eq("setpgid(member, leader)", setpgid(member.pid(), leader.pid()), 0)?;
    want_eq("setpriority(PRIO_PGRP) to 9", setpriority(PRIO_PGRP, leader.pid(), 9), 0)?;
    nice_is(PRIO_PROCESS, leader.pid(), 9, "the group's leader")?;
    nice_is(PRIO_PROCESS, member.pid(), 9, "the group's other member")?;
    want_eq("setpriority on the member to 4", setpriority(PRIO_PROCESS, member.pid(), 4), 0)?;
    nice_is(PRIO_PGRP, leader.pid(), 4, "getpriority(PRIO_PGRP), the lowest nice value in the group,")?;
    member.release()?;
    leader.release()
}

fn sched_user() -> CaseResult {
    let first = held(|| become_user(4811, 4811))?;
    let second = held(|| become_user(4811, 4811))?;
    let other = held(|| become_user(4812, 4812))?;
    let other_nice = getpriority(PRIO_PROCESS, other.pid())
        .map_err(|e| format!("getpriority of another user's process failed with {}", errname(e)))?;
    want_eq("setpriority(PRIO_USER, 4811) to 6", setpriority(PRIO_USER, 4811, 6), 0)?;
    nice_is(PRIO_USER, 4811, 6, "getpriority(PRIO_USER)")?;
    nice_is(PRIO_PROCESS, first.pid(), 6, "the user's first process")?;
    nice_is(PRIO_PROCESS, second.pid(), 6, "the user's second process")?;
    nice_is(PRIO_PROCESS, other.pid(), other_nice, "another user's process, after setpriority(PRIO_USER)")?;
    want_eq("setpriority on the user's second process to 3", setpriority(PRIO_PROCESS, second.pid(), 3), 0)?;
    nice_is(PRIO_USER, 4811, 3, "getpriority(PRIO_USER), the lowest nice value of the user's processes,")?;
    other.release()?;
    second.release()?;
    first.release()
}

fn sched_nice() -> CaseResult {
    want_eq("setpriority to 2", setpriority(PRIO_PROCESS, 0, 2), 0)?;
    check(nice(3) == Ok(5), "nice(3) from 2 did not return 5")?;
    nice_is(PRIO_PROCESS, 0, 5, "after nice(3)")
}

fn sched_nice_user() -> CaseResult {
    want_eq("setpriority to 0", setpriority(PRIO_PROCESS, 0, 0), 0)?;
    become_user(4821, 4821)?;
    let lowered = nice(-1);
    check(lowered == Err(EPERM), &format!("nice(-1) as non-root returned {lowered:?}, expected Err(EPERM)"))?;
    nice_is(PRIO_PROCESS, 0, 0, "after the refused nice(-1)")?;
    let raised = nice(1);
    check(raised == Ok(1), &format!("nice(1) as non-root returned {raised:?}, expected Ok(1)"))?;
    nice_is(PRIO_PROCESS, 0, 1, "after nice(1)")
}

fn sched_fork() -> CaseResult {
    want_eq("setpriority to 6", setpriority(PRIO_PROCESS, 0, 6), 0)?;
    task(|| match getpriority(PRIO_PROCESS, 0) {
        Ok(6) => Ok(()),
        other => Err(format!("the child's nice value is {other:?}, expected 6")),
    })?.finish()
}

fn sched_exec() -> CaseResult {
    let state = exec_state(&[], || {
        let r = setpriority(PRIO_PROCESS, 0, 6);
        if r == 0 { Ok(()) } else { Err(format!("setpriority failed with {}", shown(r))) }
    })?;
    state_is(&state, "nice", "6", "the nice value")
}

static SUITE: Suite = suite(
    "processes", "Processes", &[
        category("fork", "fork & copy-on-write", &[
            case("pids", "fork returns the child's PID to the parent, and the child's getppid is the parent", fork_pids),
            case("cow-memory", "Writes after fork to data, stack and heap stay in the process that made them, whichever writes first", fork_cow),
            case("private-mapping", "A private anonymous mapping is copied on write across fork, whichever side writes first", fork_private_mapping),
            case("shared-mapping", "A shared anonymous mapping stays shared across fork", fork_shared_mapping),
            case("descriptors", "The child inherits descriptors sharing file offsets, and closing its copy leaves the parent's", fork_descriptors),
            case("descriptor-flags", "FD_CLOEXEC is inherited and status flags are shared through the open file description", fork_descriptor_flags),
            case("signal-dispositions", "Signal handlers and ignored signals are inherited and work in the child", fork_signal_dispositions),
            case("signal-mask", "The signal mask is inherited", fork_signal_mask),
            case("pending-cleared", "The child starts with no pending signals", fork_pending_cleared),
            case("timer-cleared", "The child does not inherit the parent's alarm (ITIMER_REAL)", fork_timer_cleared),
            case("cwd-umask", "The working directory and umask are inherited", fork_cwd_umask),
            case("times-reset", "The child's times() counts start at zero", fork_times_reset),
            case("nproc-eagain", "fork fails with EAGAIN when the user is at RLIMIT_NPROC", fork_nproc),
            case("many-children", "32 children are alive at once, and each exits with its own status", fork_many),
            case("kill-heap", "Children killed inside a read or an exec leave the kernel heap as it was", fork_kill_heap),
        ]),
        category("exec", "exec, argv & environment", &[
            case("argv", "execve passes argv exactly, including an arbitrary argv[0] and empty arguments", exec_argv),
            case("envp", "execve passes envp exactly as the new environment", exec_envp),
            case("empty-env", "An empty envp gives an empty environment", exec_empty_env),
            case("many-args", "259 arguments arrive intact", exec_many_args),
            case("long-arg", "A 64 KiB argument, within the ARG_MAX a C library reports, arrives intact", exec_long_arg),
            case("e2big", "Arguments beyond the ARG_MAX a C library reports fail with E2BIG", exec_e2big),
            case("enoent", "A missing program fails with ENOENT", exec_enoent),
            case("enotdir", "A path through a regular file fails with ENOTDIR", exec_enotdir),
            case("enametoolong", "A path longer than PATH_MAX fails with ENAMETOOLONG", exec_enametoolong),
            case("eacces-mode", "A file without execute permission fails with EACCES", exec_eacces_mode),
            case("eacces-dir", "A directory fails with EACCES", exec_eacces_dir),
            case("enoexec", "An executable file in no known format fails with ENOEXEC", exec_enoexec),
            case("failed-intact", "An exec failing with ENOEXEC leaves the caller's memory, descriptors and handlers intact", exec_failed_intact),
            case("script", "Compatibility, not POSIX: a #! script runs its interpreter with the optional argument, the script path and the arguments", exec_script),
            case("script-noarg", "Compatibility, not POSIX: a #! script without an optional argument runs its interpreter with the script path", exec_script_noarg),
            case("script-missing", "Compatibility, not POSIX: a #! script whose interpreter is missing fails with ENOENT", exec_script_missing),
            case("relative-path", "A relative program path is resolved against the working directory", exec_relative),
            case("cloexec", "Close-on-exec descriptors are closed; others stay open at their offsets", exec_cloexec),
            case("signal-reset", "Caught signals reset to default; ignored signals stay ignored", exec_signal_reset),
            case("mask-pending", "The signal mask and pending signals are kept", exec_mask_pending),
            case("keeps-process", "The process ID, parent, process group and session are kept", exec_keeps_process),
            case("keeps-cwd-umask", "The working directory and umask are kept", exec_keeps_cwd_umask),
            case("keeps-alarm", "A pending alarm is kept", exec_keeps_alarm),
            case("keeps-ids", "An ordinary program keeps the user, group and supplementary group IDs", exec_keeps_ids),
            case("setuid-bit", "A set-user-ID program runs with its owner as the effective user ID", exec_setuid_bit),
            case("setgid-bit", "A set-group-ID program runs with its group as the effective group ID", exec_setgid_bit),
            case("setuid-saved", "A set-user-ID program can switch between its real and saved user IDs", exec_setuid_saved),
            case("setgid-saved", "A set-group-ID program can switch between its real and saved group IDs", exec_setgid_saved),
        ]),
        category("wait", "wait, waitpid & exit status", &[
            case("specific", "256 immediate exits with waitpid of a specific child", specific),
            case("any", "256 immediate exits with waitpid of any child", any),
            case("specific-signal", "256 specific-child waits with a caught SIGCHLD", specific_signal),
            case("any-signal", "256 any-child waits with a caught SIGCHLD", any_signal),
            case("parked-specific", "Specific-child wait before a delayed exit", parked_specific),
            case("parked-any", "Any-child wait before a delayed exit", parked_any),
            case("parked-specific-signal", "Specific-child wait with SIGCHLD before a delayed exit", parked_specific_signal),
            case("parked-any-signal", "Any-child wait with SIGCHLD before a delayed exit", parked_any_signal),
            case("nohang", "WNOHANG returns zero before exit and the PID afterward", nohang),
            case("interrupted", "Caught SIGCHLD without SA_RESTART interrupts waitpid", interrupted),
            case("stack-trampoline", "SIGCHLD returns through the stack trampoline", stack_trampoline),
            case("cow-signal-stack", "SIGCHLD writes a fork-shared signal frame across two stack pages", cow_signal_stack),
            case("echild-none", "waitpid with no children fails with ECHILD", wait_echild_none),
            case("echild-other", "waitpid on a process that is not a child fails with ECHILD", wait_echild_other),
            case("einval", "waitpid with an undefined option fails with EINVAL", wait_einval),
            case("exit-codes", "WIFEXITED and WEXITSTATUS report the low 8 bits of the exit status", wait_exit_codes),
            case("signaled", "WIFSIGNALED and WTERMSIG report death by SIGTERM and SIGKILL", wait_signaled),
            case("fault", "A child that reads an unmapped address is reported killed by SIGSEGV", wait_fault),
            case("stopped", "WUNTRACED reports a stopped child once, with WIFSTOPPED and WSTOPSIG", wait_stopped),
            case("stop-hidden", "Without WUNTRACED a stopped child is not reported", wait_stop_hidden),
            case("continued", "WCONTINUED reports a stopped child that was continued", wait_continued),
            case("pid-zero", "waitpid(0) waits only for children in the caller's process group", wait_pid_zero),
            case("pid-group", "waitpid(-pgid) waits only for children in that process group", wait_pid_group),
            case("any-order", "waitpid(-1) reaps each exited child once, then fails with ECHILD", wait_any_order),
            case("zombie", "An exited child stays a zombie until waited for, then its PID is gone", wait_zombie),
            case("reparent", "When a parent exits, its child is reparented to a system process", wait_reparent),
            case("sigchld-ignored", "With SIGCHLD ignored, exited children leave no zombie to wait for", wait_sigchld_ignored),
            case("waitid", "waitid reports an exited child's PID, CLD_EXITED and status, and reaps it", wait_waitid),
            case("waitid-nowait", "waitid with WNOWAIT reports the child and leaves it waitable", wait_waitid_nowait),
            case("null-status", "waitpid with a null status pointer reaps the child", wait_null_status),
            case("exit-closes", "_exit closes every descriptor the child held", wait_exit_closes),
        ]),
        category("groups-sessions", "process groups & sessions", &[
            case("inherit", "getpgrp and getsid agree with getpgid, and a child inherits both", groups_inherit),
            case("setpgid-self", "setpgid(0, 0) makes the caller a process group leader", groups_setpgid_self),
            case("setpgid-child", "A parent can put its child in a new group", groups_setpgid_child),
            case("setpgid-join", "A child can join another group in the same session", groups_setpgid_join),
            case("setpgid-esrch", "setpgid on a process that is neither the caller nor its child fails with ESRCH", groups_setpgid_esrch),
            case("setpgid-eacces", "setpgid on a child that has called exec fails with EACCES", groups_setpgid_eacces),
            case("setpgid-session-leader", "setpgid on a session leader fails with EPERM", groups_setpgid_session_leader),
            case("setpgid-other-session", "setpgid across sessions fails with EPERM", groups_setpgid_other_session),
            case("setpgid-no-group", "setpgid into a group that does not exist fails with EPERM", groups_setpgid_no_group),
            case("setpgid-einval", "setpgid with a negative group fails with EINVAL", groups_setpgid_einval),
            case("setsid", "setsid makes the caller leader of a new session and group", groups_setsid),
            case("setsid-leader", "setsid by a group or session leader fails with EPERM", groups_setsid_leader),
            case("lookup-esrch", "getpgid and getsid of a missing process fail with ESRCH", groups_lookup_esrch),
            case("outlives-leader", "A process group outlives its leader while members remain", groups_outlives_leader),
            case("orphaned-stopped", "A newly orphaned group with a stopped member is sent SIGHUP and SIGCONT", groups_orphaned_stopped),
        ]),
        category("credentials", "user & group IDs", &[
            case("root", "The suite runs with real and effective user and group IDs 0", creds_root),
            case("setuid-root", "setuid as root sets the real, effective and saved user IDs", creds_setuid_root),
            case("setuid-user", "setuid as non-root to another user fails with EPERM", creds_setuid_user),
            case("setuid-saved", "setuid as non-root switches between the real and saved user IDs", creds_setuid_saved),
            case("setgid-saved", "setgid as non-root switches between the real and saved group IDs", creds_setgid_saved),
            case("setgid-root", "setgid as root sets the real and effective group IDs", creds_setgid_root),
            case("setgid-user", "setgid as non-root to another group fails with EPERM", creds_setgid_user),
            case("setgid-after-setuid", "setgid after setuid has given up root fails with EPERM", creds_setgid_after_setuid),
            case("setreuid-root", "setreuid as root sets distinct real and effective user IDs", creds_setreuid_root),
            case("setreuid-temporary", "setreuid(-1, uid) drops root temporarily and setreuid(-1, 0) restores it", creds_setreuid_temporary),
            case("setreuid-permanent", "setreuid(uid, uid) drops root for good", creds_setreuid_permanent),
            case("setreuid-swap", "A non-root process can swap its real and effective user IDs", creds_setreuid_swap),
            case("setreuid-user", "setreuid as non-root to another user fails with EPERM", creds_setreuid_user),
            case("setregid-root", "setregid as root sets distinct real and effective group IDs", creds_setregid_root),
            case("setregid-swap", "A non-root process can swap its real and effective group IDs", creds_setregid_swap),
            case("setregid-user", "setregid as non-root to another group fails with EPERM", creds_setregid_user),
            case("getgroups", "getgroups counts and lists the supplementary groups, and EINVAL for a short list", creds_getgroups),
            case("setgroups-user", "setgroups as non-root fails with EPERM", creds_setgroups_user),
            case("fork", "A child inherits the user, group and supplementary group IDs", creds_fork),
        ]),
        category("limits", "resource limits & usage", &[
            case("getrlimit", "getrlimit reports every POSIX resource with soft at most hard", limits_getrlimit),
            case("getrlimit-einval", "getrlimit of an unknown resource fails with EINVAL", limits_getrlimit_einval),
            case("setrlimit-soft", "setrlimit lowers the soft limit and getrlimit reads it back", limits_setrlimit_soft),
            case("soft-above-hard", "setrlimit with soft above hard fails with EINVAL", limits_soft_above_hard),
            case("root-raises-hard", "Root may raise a hard limit", limits_root_hard),
            case("user-raises-hard", "Raising a hard limit as non-root fails with EPERM", limits_user_hard),
            case("user-raises-soft", "A non-root process may raise its soft limit up to the hard", limits_user_soft),
            case("nofile-open", "RLIMIT_NOFILE bounds the descriptors open returns, then EMFILE once all below it are used", limits_nofile_open),
            case("nofile-dup", "dup2 and F_DUPFD at or above RLIMIT_NOFILE fail with EBADF and EINVAL", limits_nofile_dup),
            case("fsize", "Writing past RLIMIT_FSIZE is cut short, then fails with EFBIG", limits_fsize),
            case("fsize-signal", "Writing past RLIMIT_FSIZE raises SIGXFSZ", limits_fsize_signal),
            case("cpu", "Exceeding RLIMIT_CPU raises SIGXCPU", limits_cpu),
            case("data", "An allocation beyond RLIMIT_DATA fails", limits_data),
            case("stack", "Using more stack than RLIMIT_STACK raises SIGSEGV", limits_stack),
            case("as", "A mapping beyond RLIMIT_AS fails with ENOMEM", limits_as),
            case("core", "RLIMIT_CORE is set and read back, and at 0 a core-dumping signal writes no core file", limits_core),
            case("fork", "Resource limits are inherited across fork", limits_fork),
            case("exec", "Resource limits are kept across exec", limits_exec),
            case("prlimit-child", "prlimit sets and reads another process's limit", limits_prlimit_child),
            case("prlimit-swap", "prlimit returns the old limit while installing a new one", limits_prlimit_swap),
            case("prlimit-errors", "prlimit fails with ESRCH for a missing process and EPERM for another user's", limits_prlimit_errors),
            case("rusage-self", "getrusage(RUSAGE_SELF) counts the caller's CPU time", limits_rusage_self),
            case("rusage-children", "getrusage(RUSAGE_CHILDREN) counts only children that were waited for", limits_rusage_children),
            case("rusage-einval", "getrusage with an unknown who fails with EINVAL", limits_rusage_einval),
            case("times", "times reports advancing elapsed time and the caller's CPU time", limits_times),
            case("times-children", "times counts the CPU time of waited-for children", limits_times_children),
        ]),
        category("scheduling", "priorities & sched_yield", &[
            case("sched-yield", "sched_yield returns 0 and hands the processor to a waiting process", sched_yield),
            case("getpriority", "getpriority reports the caller's nice value by 0 and by PID", sched_getpriority),
            case("setpriority", "setpriority sets the caller's nice value", sched_setpriority),
            case("clamp", "Nice values beyond the range are clamped to 19 and -20", sched_clamp),
            case("root-lowers", "Root may lower its nice value", sched_root_lowers),
            case("user-raise", "A non-root process may raise but not lower its nice value (EACCES)", sched_user_raise),
            case("other-user", "setpriority on another user's process as non-root fails with EPERM", sched_other_user),
            case("errors", "getpriority and setpriority fail with ESRCH and EINVAL", sched_errors),
            case("child", "setpriority and getpriority work on a child by PID", sched_child),
            case("process-group", "PRIO_PGRP sets every member and reports the lowest nice value", sched_group),
            case("user", "PRIO_USER sets every process of the user, no other, and reports the lowest nice value", sched_user),
            case("nice", "nice adds to the nice value and returns the new value", sched_nice),
            case("nice-user", "nice with a negative increment as non-root fails with EPERM and keeps the nice value", sched_nice_user),
            case("fork", "The nice value is inherited across fork", sched_fork),
            case("exec", "The nice value is kept across exec", sched_exec),
        ]),
    ],
);

fn main() { SUITE.run() }
