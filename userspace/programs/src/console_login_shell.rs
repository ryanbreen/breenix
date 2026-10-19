//! Login-shell oracle used by the console suite's independent getty sessions.
use libbreenix::{error::Error, fs, io, syscall::{nr, raw}};
type Verdict<T = ()> = Result<T, String>;
const O_NOCTTY: u32 = 0o400;
fn require(ok: bool, message: &str) -> Verdict {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn vt_ioctl(fd: libbreenix::types::Fd, request: u64, arg: u64) -> Verdict<u64> {
    let ret = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), request, arg) };
    Error::from_syscall(ret as i64).map_err(|e| format!("ioctl {request:x}: {e}"))
}
// Login invokes this entry point as the staged account's shell.
fn login_shell() -> Verdict {
    use libbreenix::{process, termios, types::Fd};
    let pid = process::getpid().map_err(|e| e.to_string())?.raw();
    let sid = process::getsid(0).map_err(|e| e.to_string())?.raw();
    let pgid = process::getpgrp().map_err(|e| e.to_string())?.raw() as i32;
    let mut tty_sid = 0i32;
    vt_ioctl(Fd::STDIN, 0x5429, &mut tty_sid as *mut i32 as u64)?;
    require(pid == sid && sid as i32 == pgid && tty_sid == pgid
        && termios::tcgetpgrp(Fd::STDIN).map_err(|e| e.to_string())? == pgid, "login session or foreground group differs")?;
    let ctty = fs::open("/dev/tty", fs::O_RDWR | O_NOCTTY).map_err(|e| e.to_string())?;
    let device = fs::fstat(Fd::STDIN).map_err(|e| e.to_string())?.st_rdev;
    require(fs::fstat(ctty).map_err(|e| e.to_string())?.st_rdev == device, "/dev/tty differs from stdin")?;
    let _ = io::close(ctty);
    for fd in [Fd::STDOUT, Fd::STDERR] {
        require(fs::fstat(fd).map_err(|e| e.to_string())?.st_rdev == device, "login standard descriptors differ")?;
    }
    let number = (1..=6).find(|n| {
        let Ok(fd) = fs::open(&format!("/dev/tty{n}"), fs::O_RDWR | O_NOCTTY) else { return false; };
        let matches = fs::fstat(fd).is_ok_and(|s| s.st_rdev == device);
        let _ = io::close(fd);
        matches
    }).ok_or("login terminal is not a numbered VT")?;
    require(std::env::var("TERM").as_deref() == Ok("breenix")
        && std::env::var("USER").as_deref() == Ok("root"), "login environment differs")?;
    println!("VTLOGIN tty{number} sid={sid} pgid={pgid}");
    let mut line = String::new();
    let count = std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    require(count == 0 && line.is_empty(), "login shell expected terminal EOF")?;
    println!("VTLOGOUT tty{number} sid={sid}");
    Ok(())
}

fn main() {
    if let Err(error) = login_shell() {
        eprintln!("login shell: {error}");
        std::process::exit(1);
    }
}
