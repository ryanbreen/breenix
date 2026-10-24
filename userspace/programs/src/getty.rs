//! A login session on the console, a numbered virtual terminal or a
//! pseudo-terminal line (`pts/N`).
//!
//! Before the prompt a terminal that the last session left owned by its
//! account is returned to root (mode 0620, group tty) and the logout is
//! recorded in the authentication log (`docs/users/design.md`, section 3); then
//! the terminal is revoked from the descriptions still open on it (vhangup), so
//! a process left from the last session cannot read the next login's input.
use libbreenix::{io, process, signal, termios, types::Fd};
use std::io::Write;
use std::os::unix::io::FromRawFd;

const O_NOCTTY: u32 = 0o400;

fn check<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

fn run(log: &mut Option<std::fs::File>) -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = match args.as_slice() {
        [] => "/dev/console".to_string(),
        [tty] if matches!(tty.as_str(), "tty1" | "tty2" | "tty3" | "tty4" | "tty5" | "tty6") => format!("/dev/{tty}"),
        [tty] if tty.strip_prefix("pts/").is_some_and(|n| !n.is_empty() && n.len() < 6 && n.bytes().all(|b| b.is_ascii_digit())) => format!("/dev/{tty}"),
        _ => return Err("usage: getty [tty1..tty6 | pts/N]".into()),
    };
    let console = !path.starts_with("/dev/pts/");
    // Init detached its session before spawning us. Repeating this on a
    // respawn also acquires the terminal after the previous session leader exits.
    let sid = check(process::setsid())?;
    let fd = check(libbreenix::fs::open(&path, libbreenix::fs::O_RDWR | O_NOCTTY))?;
    check(termios::set_controlling_terminal(fd))?;
    // Give the line back to root first, so the last session's account cannot
    // open it again, then revoke it from the descriptions still open on it, so
    // a process left from that session can neither read the next login's input
    // nor write to its screen, and open it again for this session.
    return_terminal(fd.raw() as i32)?;
    check(termios::vhangup().map_err(|e| format!("revoke {path}: {e}")))?;
    check(io::close(fd))?;
    let fd = check(libbreenix::fs::open(&path, libbreenix::fs::O_RDWR | O_NOCTTY))?;
    for target in [Fd::STDIN, Fd::STDOUT, Fd::STDERR] { check(io::dup2(fd, target))?; }
    if fd.raw() > 2 { check(io::close(fd))?; }
    check(termios::tcsetpgrp(Fd::STDIN, sid.raw() as i32))?;
    if check(termios::tcgetpgrp(Fd::STDIN))? != sid.raw() as i32 { return Err("console foreground group differs".into()); }
    for sig in [signal::SIGHUP, signal::SIGINT, signal::SIGQUIT, signal::SIGTSTP] {
        check(signal::sigaction(sig, Some(&signal::Sigaction::default()), None))?;
    }
    let mut t = termios::Termios::default();
    check(termios::tcgetattr(Fd::STDIN, &mut t))?;
    t.c_iflag = termios::iflag::ICRNL;
    t.c_oflag = termios::oflag::OPOST | termios::oflag::ONLCR;
    t.c_lflag = termios::lflag::ICANON | termios::lflag::ISIG | termios::lflag::IEXTEN
        | termios::lflag::ECHO | termios::lflag::ECHOE | termios::lflag::ECHOK;
    t.c_cc[0] = 3; t.c_cc[1] = 28; t.c_cc[2] = 127; t.c_cc[3] = 21;
    t.c_cc[4] = 4; t.c_cc[10] = 26;
    check(termios::tcsetattr(Fd::STDIN, termios::TCSAFLUSH, &t))?;
    if console { check(termios::console_input_proof(Fd::STDIN, 0))?; }
    let acquired = b"[getty] console acquired\n";
    if let Some(log) = log { check(log.write_all(acquired))?; }
    check(std::io::stdout().write_all(acquired))?;
    // An empty name refreshes the prompt on this same terminal session.
    // Exiting would make the respawn revoke and flush a name typed behind it.
    let mut name = Vec::new();
    loop {
        let prompt = b"\nBreenix login: ";
        if let Some(log) = log { check(log.write_all(prompt))?; }
        check(std::io::stdout().write_all(prompt))?;
        check(std::io::stdout().flush())?;
        // Unbuffered: no bytes belonging to login are retained by getty.
        loop {
            let mut byte = [0];
            if check(libbreenix::io::read(Fd::STDIN, &mut byte))? == 0 { return Ok(()); }
            if byte[0] == b'\n' { break; }
            if name.len() == 64 || !byte[0].is_ascii_alphanumeric() && byte[0] != b'_' && byte[0] != b'-' {
                return Err("invalid login name".into());
            }
            name.push(byte[0]);
        }
        if !name.is_empty() { break; }
    }
    name.push(0);
    let argv = [b"login\0".as_ptr(), name.as_ptr(), core::ptr::null()];
    check(process::execv(b"/bin/login\0", argv.as_ptr()))?;
    Ok(())
}

/// The previous session's account still owns the terminal: give it back to
/// root and record the logout.
fn return_terminal(fd: i32) -> Result<(), String> {
    use libbreenix::accounts::{self, session};
    let Some(owner) = session::terminal_owner(fd).filter(|uid| *uid != 0) else { return Ok(()); };
    session::return_terminal(fd)?;
    let root = std::path::Path::new("/");
    let name = session::account_by_uid(root, owner).map(|a| a.name).unwrap_or_else(|| owner.to_string());
    let tty = session::terminal_name(fd).unwrap_or_else(|| "-".into());
    let record = session::Record { program: "getty", user: &name, target: &name, tty: &tty, refused: None, cmd: "logout" };
    if let Err(why) = accounts::log(root, &record) { eprintln!("getty: auth log: {why}"); }
    Ok(())
}

fn main() {
    let captured = std::env::var_os("BREENIX_SERVICE_LOG").is_some();
    std::env::remove_var("BREENIX_SERVICE_LOG");
    let copy = |fd| -> Option<std::fs::File> {
        if !captured { return None; }
        let fd = io::fcntl(fd, io::fcntl_cmd::F_DUPFD_CLOEXEC, 3).expect("getty log descriptor");
        Some(unsafe { std::fs::File::from_raw_fd(fd as i32) })
    };
    // Getty owns the terminal while asking for a login name. Keep a copy of
    // its own output in the supervisor pipes; the copies close when login execs.
    let mut output = copy(Fd::STDOUT);
    let mut errors = copy(Fd::STDERR);
    if let Err(e) = run(&mut output) {
        if let Some(log) = &mut errors { let _ = writeln!(log, "getty: {}", e); }
        eprintln!("getty: {}", e);
        std::process::exit(1);
    }
}
