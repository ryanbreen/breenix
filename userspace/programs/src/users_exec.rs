//! Exec target for the users suite's set-ID cases. No serial output: results go
//! back to the case through a descriptor named on the command line, or the exit status.
//!
//! - `ids FD`: write `key=value` lines to FD: `resuid=` and `resgid=` (real, effective
//!   and saved IDs), `secure=` (getauxval(AT_SECURE)), and for descriptors 0 to 2 `fdN=`
//!   `null` (open on /dev/null), `open` (open on anything else) or `closed`.
//! - `saved FD`: write the `ids` report, then switch the effective user ID to the real
//!   one and back to the saved one with setuid; exit 0 if both take, else the step that
//!   failed (10-13).
//! - `hold READY FD`: write the `ids` report to READY and close it, then read FD until end
//!   of file and exit 0.
//! - `ran`: exit 77.
//!
//! The program has no Rust `main`: the C library's entry calls `main` below directly, so
//! the standard library's start-up, which reopens closed descriptors 0 to 2 on
//! `/dev/null` itself, does not run, and the descriptors are as the kernel left them.
#![no_main]
use libbreenix::syscall::raw;
use std::ffi::CStr;
use std::fmt::Write as _;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const CLOSE: u64 = 3;
    pub const FSTAT: u64 = 5;
    pub const NEWFSTATAT: u64 = 262;
    pub const SETUID: u64 = 105;
    pub const GETRESUID: u64 = 118;
    pub const GETRESGID: u64 = 120;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: u64 = 63;
    pub const WRITE: u64 = 64;
    pub const CLOSE: u64 = 57;
    pub const FSTAT: u64 = 80;
    pub const NEWFSTATAT: u64 = 79;
    pub const SETUID: u64 = 146;
    pub const GETRESUID: u64 = 148;
    pub const GETRESGID: u64 = 150;
}

const AT_FDCWD: u64 = (-100i64) as u64;
const AT_SECURE: u64 = 23;

extern "C" {
    fn getauxval(kind: u64) -> u64;
}

fn sys(n: u64, a: [u64; 4]) -> i64 {
    // SAFETY: the callers pass buffers that outlive the call.
    unsafe { raw::syscall4(n, a[0], a[1], a[2], a[3]) as i64 }
}

/// A stat buffer large enough for either architecture's layout; the device number
/// (`st_rdev`) is at byte 40 on x86-64 and 32 on AArch64.
fn rdev(stat: &[u64; 16]) -> u64 {
    if cfg!(target_arch = "x86_64") { stat[5] } else { stat[4] }
}

fn ids(n: u64) -> String {
    let mut v = [0u32; 3];
    let r = sys(n, [&mut v[0] as *mut u32 as u64, &mut v[1] as *mut u32 as u64, &mut v[2] as *mut u32 as u64, 0]);
    if r < 0 { format!("E{}", -r) } else { format!("{},{},{}", v[0], v[1], v[2]) }
}

fn report() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "resuid={}", ids(nr::GETRESUID));
    let _ = writeln!(out, "resgid={}", ids(nr::GETRESGID));
    // SAFETY: getauxval reads the auxiliary vector the kernel placed on the stack.
    let _ = writeln!(out, "secure={}", unsafe { getauxval(AT_SECURE) });
    let mut null = [0u64; 16];
    let null_rdev = if sys(nr::NEWFSTATAT, [AT_FDCWD, c"/dev/null".as_ptr() as u64, null.as_mut_ptr() as u64, 0]) == 0 {
        Some(rdev(&null))
    } else {
        None
    };
    for fd in 0..3u64 {
        let mut stat = [0u64; 16];
        let state = if sys(nr::FSTAT, [fd, stat.as_mut_ptr() as u64, 0, 0]) < 0 {
            "closed"
        } else if Some(rdev(&stat)) == null_rdev {
            "null"
        } else {
            "open"
        };
        let _ = writeln!(out, "fd{fd}={state}");
    }
    out
}

fn write_all(fd: u64, mut bytes: &[u8]) -> Result<(), i32> {
    while !bytes.is_empty() {
        let n = sys(nr::WRITE, [fd, bytes.as_ptr() as u64, bytes.len() as u64, 0]);
        if n <= 0 {
            return Err(3);
        }
        bytes = &bytes[n as usize..];
    }
    Ok(())
}

fn fd_arg(args: &[&str], i: usize) -> Result<u64, i32> {
    args.get(i).and_then(|a| a.parse().ok()).ok_or(2)
}

#[no_mangle]
pub extern "C" fn main(argc: i32, argv: *const *const u8, _envp: *const *const u8) -> i32 {
    let args: Vec<&str> = (0..argc.max(0) as usize)
        // SAFETY: the kernel passes argc NUL-terminated strings in argv.
        .map(|i| unsafe { CStr::from_ptr(*argv.add(i) as *const core::ffi::c_char) }.to_str().unwrap_or(""))
        .collect();
    match run(&args) {
        Ok(()) => 0,
        Err(code) => code,
    }
}

fn run(args: &[&str]) -> Result<(), i32> {
    match args.get(1).copied() {
        Some("ids") => write_all(fd_arg(args, 2)?, report().as_bytes()),
        Some("saved") => {
            let fd = fd_arg(args, 2)?;
            write_all(fd, report().as_bytes())?;
            let mut v = [0u32; 3];
            sys(nr::GETRESUID, [&mut v[0] as *mut u32 as u64, &mut v[1] as *mut u32 as u64, &mut v[2] as *mut u32 as u64, 0]);
            let [real, _, saved] = v;
            let effective = || {
                let mut now = [0u32; 3];
                sys(nr::GETRESUID, [&mut now[0] as *mut u32 as u64, &mut now[1] as *mut u32 as u64, &mut now[2] as *mut u32 as u64, 0]);
                now[1]
            };
            if sys(nr::SETUID, [real as u64, 0, 0, 0]) != 0 {
                return Err(10);
            }
            if effective() != real {
                return Err(11);
            }
            if sys(nr::SETUID, [saved as u64, 0, 0, 0]) != 0 {
                return Err(12);
            }
            if effective() != saved {
                return Err(13);
            }
            Ok(())
        }
        Some("hold") => {
            let ready = fd_arg(args, 2)?;
            let hold = fd_arg(args, 3)?;
            write_all(ready, report().as_bytes())?;
            sys(nr::CLOSE, [ready, 0, 0, 0]);
            let mut byte = [0u8; 1];
            while sys(nr::READ, [hold, byte.as_mut_ptr() as u64, 1, 0]) > 0 {}
            Ok(())
        }
        Some("ran") => Err(77),
        _ => Err(1),
    }
}
