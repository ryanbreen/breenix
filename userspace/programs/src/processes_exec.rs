//! Exec target for the Processes suite. No serial output: every result goes back to
//! the case through the descriptor named on the command line, or the exit status.
//!
//! - `report FD`: write argv (each NUL-terminated), a 0x01 byte, then the environment
//!   (each NUL-terminated) to FD.
//! - run as `#!HELPER script` or `#!HELPER`: the same report, to the descriptor named
//!   by the argument after the script path.
//! - `state FD [N...]`: write `key=value` lines describing the process image exec kept,
//!   including whether each descriptor N is open and at what offset.
//! - `hold FD`: read FD until end of file, then exit 0.
//! - `saved REAL SAVED`: switch the effective user ID to REAL and back to SAVED with
//!   setuid, keeping the real user ID REAL; exit 0 if both succeed, else the step that
//!   failed (10-15).
//! - `savedgid REAL SAVED`: the same for the group IDs with setgid (20-25).
//! - `ran`: exit 77, for exec calls that should have failed.
use libbreenix::signal::{self, Sigaction, SIG_DFL, SIG_IGN, SIGUSR1, SIGUSR2};
use libbreenix::syscall::raw;
use libbreenix::{io, process, types::Fd};
use std::fmt::Write as _;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const TIMES: u64 = 100;
    pub const FCNTL: u64 = 72;
    pub const LSEEK: u64 = 8;
    pub const GETPPID: u64 = 110;
    pub const GETPGID: u64 = 121;
    pub const GETSID: u64 = 124;
    pub const GETUID: u64 = 102;
    pub const GETEUID: u64 = 107;
    pub const GETGID: u64 = 104;
    pub const GETEGID: u64 = 108;
    pub const GETRESUID: u64 = 118;
    pub const GETRESGID: u64 = 120;
    pub const SETUID: u64 = 105;
    pub const SETGID: u64 = 106;
    pub const GETGROUPS: u64 = 115;
    pub const UMASK: u64 = 95;
    pub const GETPRIORITY: u64 = 140;
    pub const PRLIMIT64: u64 = 302;
    pub const GETITIMER: u64 = 36;
    pub const GETCWD: u64 = 79;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const TIMES: u64 = 153;
    pub const FCNTL: u64 = 25;
    pub const LSEEK: u64 = 62;
    pub const GETPPID: u64 = 173;
    pub const GETPGID: u64 = 155;
    pub const GETSID: u64 = 156;
    pub const GETUID: u64 = 174;
    pub const GETEUID: u64 = 175;
    pub const GETGID: u64 = 176;
    pub const GETEGID: u64 = 177;
    pub const GETRESUID: u64 = 148;
    pub const GETRESGID: u64 = 150;
    pub const SETUID: u64 = 146;
    pub const SETGID: u64 = 144;
    pub const GETGROUPS: u64 = 158;
    pub const UMASK: u64 = 166;
    pub const GETPRIORITY: u64 = 141;
    pub const PRLIMIT64: u64 = 261;
    pub const GETITIMER: u64 = 102;
    pub const GETCWD: u64 = 17;
}

const EBADF: i64 = 9;

fn sys(n: u64, a: [u64; 4]) -> i64 {
    // SAFETY: every caller passes pointers to buffers that live through the call.
    unsafe { raw::syscall4(n, a[0], a[1], a[2], a[3]) as i64 }
}

fn write_all(fd: Fd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        match io::write(fd, bytes) {
            Ok(n) if n > 0 => bytes = &bytes[n..],
            _ => process::exit(3),
        }
    }
}

fn fd_arg(arg: Option<&String>) -> Fd {
    match arg.and_then(|s| s.parse::<u64>().ok()) {
        Some(fd) => Fd::from_raw(fd),
        None => process::exit(2),
    }
}

// Touch more than the initially mapped runtime allowance below the arguments.
#[inline(never)]
fn runtime_stack_buffer(fd: Fd) {
    let mut buffer = [0u8; 96 * 1024];
    for i in (0..buffer.len()).step_by(4096) {
        // Volatile accesses keep the demand-growth exercise on the actual stack.
        unsafe {
            core::ptr::write_volatile(buffer.as_mut_ptr().add(i), b'x');
        }
    }
    buffer[..8].copy_from_slice(b"stack-ok");
    write_all(fd, &buffer[..8]);
}

fn stack(fd: Fd, args: &[String]) -> ! {
    let [soft, _] = {
        let mut limits = [0u64; 2];
        if sys(nr::PRLIMIT64, [0, 3, 0, limits.as_mut_ptr() as u64]) != 0 {
            process::exit(4);
        }
        limits
    };
    if args.len() != 4
        || args[3].len() != soft as usize / 4 - 64
        || !args[3].bytes().all(|b| b == b'x')
    {
        process::exit(5);
    }
    let mut alt = signal::StackT::default();
    if signal::sigaltstack(None, Some(&mut alt)).is_err()
        || alt.ss_flags != signal::SS_DISABLE || alt.ss_sp != 0 || alt.ss_size != 0
        || signal::sigaltstack(Some(&signal::StackT::default()), None).is_err() {
        process::exit(6);
    }
    runtime_stack_buffer(fd);
    process::exit(0)
}

fn report(fd: Fd, args: &[String]) -> ! {
    let mut out = Vec::new();
    for arg in args {
        out.extend_from_slice(arg.as_bytes());
        out.push(0);
    }
    out.push(1);
    for (key, value) in std::env::vars_os() {
        out.extend_from_slice(key.as_encoded_bytes());
        out.push(b'=');
        out.extend_from_slice(value.as_encoded_bytes());
        out.push(0);
    }
    write_all(fd, &out);
    process::exit(0)
}

fn disposition(sig: i32) -> &'static str {
    let mut old = Sigaction::default();
    match signal::sigaction(sig, None, Some(&mut old)) {
        Ok(()) if old.handler == SIG_DFL => "dfl",
        Ok(()) if old.handler == SIG_IGN => "ign",
        Ok(()) => "handler",
        Err(_) => "error",
    }
}

fn ids(n: u64) -> String {
    let mut v = [0u32; 3];
    let p = v.as_mut_ptr();
    // SAFETY: the three IDs are written into v, which outlives the call.
    let r = sys(n, [p as u64, unsafe { p.add(1) } as u64, unsafe { p.add(2) } as u64, 0]);
    if r < 0 { format!("E{}", -r) } else { format!("{},{},{}", v[0], v[1], v[2]) }
}

fn state(fd: Fd, fds: &[String]) -> ! {
    let mut out = String::new();
    let _ = writeln!(out, "pid={}", process::getpid().map(|p| p.raw() as i64).unwrap_or(-1));
    for (key, n) in [("ppid", nr::GETPPID), ("uid", nr::GETUID), ("euid", nr::GETEUID),
        ("gid", nr::GETGID), ("egid", nr::GETEGID)] {
        let _ = writeln!(out, "{key}={}", sys(n, [0; 4]));
    }
    let _ = writeln!(out, "pgid={}", sys(nr::GETPGID, [0; 4]));
    let _ = writeln!(out, "sid={}", sys(nr::GETSID, [0; 4]));
    let _ = writeln!(out, "resuid={}", ids(nr::GETRESUID));
    let _ = writeln!(out, "resgid={}", ids(nr::GETRESGID));
    let mut groups = [0u32; 64];
    let n = sys(nr::GETGROUPS, [64, groups.as_mut_ptr() as u64, 0, 0]);
    let list: Vec<String> = groups[..n.max(0) as usize].iter().map(u32::to_string).collect();
    let _ = writeln!(out, "groups={}", if n < 0 { format!("E{}", -n) } else { list.join(",") });
    let mask = sys(nr::UMASK, [0o022, 0, 0, 0]);
    sys(nr::UMASK, [mask as u64, 0, 0, 0]);
    let _ = writeln!(out, "umask={mask:o}");
    let mut cwd = [0u8; 512];
    let r = sys(nr::GETCWD, [cwd.as_mut_ptr() as u64, 512, 0, 0]);
    let len = cwd.iter().position(|&b| b == 0).unwrap_or(0);
    let _ = writeln!(out, "cwd={}", if r < 0 { format!("E{}", -r) } else { String::from_utf8_lossy(&cwd[..len]).into_owned() });
    let prio = sys(nr::GETPRIORITY, [0, 0, 0, 0]);
    let _ = writeln!(out, "nice={}", if prio < 0 { format!("E{}", -prio) } else { (20 - prio).to_string() });
    let mut lim = [0u64; 2];
    let r = sys(nr::PRLIMIT64, [0, 7, 0, lim.as_mut_ptr() as u64]);
    let _ = writeln!(out, "nofile={}", if r < 0 { format!("E{}", -r) } else { format!("{},{}", lim[0], lim[1]) });
    let _ = writeln!(out, "usr1={}", disposition(SIGUSR1));
    let _ = writeln!(out, "usr2={}", disposition(SIGUSR2));
    let mut blocked = 0u64;
    let _ = signal::sigprocmask(signal::SIG_BLOCK, None, Some(&mut blocked));
    let _ = writeln!(out, "mask={blocked:x}");
    let mut pending = 0u64;
    let _ = signal::sigpending(&mut pending);
    let _ = writeln!(out, "pending={pending:x}");
    let mut timer = [0i64; 4];
    let r = sys(nr::GETITIMER, [0, timer.as_mut_ptr() as u64, 0, 0]);
    let _ = writeln!(out, "alarm={}", if r < 0 { format!("E{}", -r) } else { format!("{}.{:06}", timer[2], timer[3]) });
    for arg in fds {
        let Ok(n) = arg.parse::<u64>() else { process::exit(2) };
        let flags = sys(nr::FCNTL, [n, 1, 0, 0]);
        if flags == -EBADF {
            let _ = writeln!(out, "fd{n}=closed");
        } else if flags < 0 {
            let _ = writeln!(out, "fd{n}=E{}", -flags);
        } else {
            let _ = writeln!(out, "fd{n}=open@{}", sys(nr::LSEEK, [n, 0, 1, 0]));
        }
    }
    write_all(fd, out.as_bytes());
    process::exit(0)
}

/// Set the effective ID to REAL and then to SAVED with `set`, checking after each step that
/// the effective ID took the value and the real ID stayed REAL. Exits `base` plus the step
/// that failed, or 0.
fn switch(set: u64, get_real: u64, get_effective: u64, real: u32, saved: u32, base: i32) -> ! {
    for (step, id) in [(0, real), (3, saved)] {
        if sys(set, [id as u64, 0, 0, 0]) != 0 { process::exit(base + step) }
        if sys(get_effective, [0; 4]) != id as i64 { process::exit(base + step + 1) }
        if sys(get_real, [0; 4]) != real as i64 { process::exit(base + step + 2) }
    }
    process::exit(0)
}

// Run the same fresh-child accounting check in a named exec helper, so the
// optional kernel delay affects only this case, never the handoff deadlines.
fn times_reset() -> Result<(), String> {
    use libbreenix::process::ForkResult;
    fn times() -> Result<[i64; 4], String> {
        let mut t = [0i64; 4];
        let result = sys(nr::TIMES, [t.as_mut_ptr() as u64, 0, 0, 0]);
        if result < 0 { Err(format!("times failed: {result}")) } else { Ok(t) }
    }
    fn burn(ms: u64) {
        let now = || libbreenix::time::now_monotonic().expect("monotonic clock").as_nanos();
        let start = now();
        let mut x = 1u64;
        while now().saturating_sub(start) < i128::from(ms) * 1_000_000 {
            for i in 0..20_000u64 { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); }
            core::hint::black_box(x);
        }
    }
    fn child(work: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        match process::fork().map_err(|e| format!("fork: {e}"))? {
            ForkResult::Child => {
                let result = work();
                if let Err(message) = &result { eprintln!("times-reset: {message}"); }
                process::exit(if result.is_ok() { 0 } else { 1 });
            }
            ForkResult::Parent(pid) => {
                let mut status = 0;
                process::waitpid(pid.raw() as i32, &mut status, 0).map_err(|e| format!("waitpid: {e}"))?;
                if status == 0 { Ok(()) } else { Err(format!("child wait status {status}")) }
            }
        }
    }
    const FRESH: i64 = 2;
    burn(400);
    child(|| { burn(150); Ok(()) })?;
    let parent = times()?;
    let own = parent[0] + parent[1];
    if own <= 4 * FRESH || parent[2] + parent[3] <= 0 {
        return Err(format!("parent times {parent:?} after 400 ms plus a waited 150 ms child"));
    }
    child(move || {
        let t = times()?;
        if t[2] != 0 || t[3] != 0 { return Err(format!("child times cutime/cstime {}/{}", t[2], t[3])); }
        if t[0] + t[1] > FRESH {
            return Err(format!("fresh child has {} ticks before computation (parent {own})", t[0] + t[1]));
        }
        Ok(())
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("times-reset") => {
            let result = times_reset();
            let message = result.err().unwrap_or_else(|| "OK".into());
            write_all(fd_arg(args.get(2)), message.as_bytes());
        }
        Some("report") => report(fd_arg(args.get(2)), &args),
        Some("stack") => stack(fd_arg(args.get(2)), &args),
        Some("script") => report(fd_arg(args.get(3)), &args),
        Some(path) if path.starts_with('/') => report(fd_arg(args.get(2)), &args),
        Some("state") => state(fd_arg(args.get(2)), &args[3..]),
        Some("hold") => {
            let fd = fd_arg(args.get(2));
            let mut buf = [0u8; 16];
            while matches!(io::read(fd, &mut buf), Ok(n) if n > 0) {}
            process::exit(0)
        }
        Some(mode @ ("saved" | "savedgid")) => {
            let id = |i: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| process::exit(2));
            if mode == "saved" {
                switch(nr::SETUID, nr::GETUID, nr::GETEUID, id(2), id(3), 10)
            } else {
                switch(nr::SETGID, nr::GETGID, nr::GETEGID, id(2), id(3), 20)
            }
        }
        Some("ran") => process::exit(77),
        _ => process::exit(2),
    }
}
