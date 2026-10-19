//! A login session on the console or a numbered virtual terminal.
use libbreenix::{io, process, signal, termios, types::Fd};
use std::io::Write;

const O_NOCTTY: u32 = 0o400;

fn check<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = match args.as_slice() {
        [] => "/dev/console".to_string(),
        [tty] if matches!(tty.as_str(), "tty1" | "tty2" | "tty3" | "tty4" | "tty5" | "tty6") => format!("/dev/{tty}"),
        _ => return Err("usage: getty [tty1..tty6]".into()),
    };
    // Init detached its session before spawning us. Repeating this on a
    // respawn also acquires the terminal after the previous session leader exits.
    let sid = check(process::setsid())?;
    let fd = check(libbreenix::fs::open(&path, libbreenix::fs::O_RDWR | O_NOCTTY))?;
    check(termios::set_controlling_terminal(fd))?;
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
    check(termios::console_input_proof(Fd::STDIN, 0))?;
    check(std::io::stdout().write_all(b"[getty] console acquired\n\nBreenix login: "))?;
    check(std::io::stdout().flush())?;
    // Unbuffered: no bytes that belong to login may be stranded in a getty buffer.
    let mut name = Vec::new();
    loop {
        let mut byte = [0];
        if check(libbreenix::io::read(Fd::STDIN, &mut byte))? == 0 { return Ok(()); }
        if byte[0] == b'\n' { break; }
        if name.len() == 64 || !byte[0].is_ascii_alphanumeric() && byte[0] != b'_' && byte[0] != b'-' {
            return Err("invalid login name".into());
        }
        name.push(byte[0]);
    }
    if name.is_empty() { return Ok(()); }
    name.push(0);
    let argv = [b"login\0".as_ptr(), name.as_ptr(), core::ptr::null()];
    check(process::execv(b"/bin/login\0", argv.as_ptr()))?;
    Ok(())
}

fn main() {
    if let Err(e) = run() { eprintln!("getty: {}", e); std::process::exit(1); }
}
