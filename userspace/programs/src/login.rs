//! Console authentication. Only staged shadow credentials can grant a session.
use libbreenix::{process, signal, termios, types::Fd};
use libbreenix::crypto::sha512_crypt::verify;
use std::ffi::CString;
use std::io::Write;

extern "C" {
    fn setgroups(count: usize, groups: *const u32) -> i32;
    fn setgid(gid: u32) -> i32;
    fn setuid(uid: u32) -> i32;
}
fn check<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, String> { result.map_err(|e| e.to_string()) }

struct Password(Vec<u8>);
impl Drop for Password {
    fn drop(&mut self) { for b in &mut self.0 { unsafe { core::ptr::write_volatile(b, 0); } } }
}
struct Echo(termios::Termios);
impl Drop for Echo {
    fn drop(&mut self) {
        if termios::tcsetattr(Fd::STDIN, termios::TCSANOW, &self.0).is_err() {
            eprintln!("login: cannot restore terminal attributes");
        }
    }
}

fn password() -> Result<Password, String> {
    for sig in [signal::SIGINT, signal::SIGQUIT, signal::SIGTSTP] {
        let action = signal::Sigaction { handler: signal::SIG_IGN, ..signal::Sigaction::default() };
        check(signal::sigaction(sig, Some(&action), None))?;
    }
    let mut saved = termios::Termios::default();
    check(termios::tcgetattr(Fd::STDIN, &mut saved))?;
    let mut hidden = saved;
    hidden.c_lflag &= !(termios::lflag::ECHO | termios::lflag::ECHONL);
    check(termios::tcsetattr(Fd::STDIN, termios::TCSAFLUSH, &hidden))?;
    let restore = Echo(saved);
    check(std::io::stdout().write_all(b"Password: "))?;
    check(std::io::stdout().flush())?;
    let mut secret = Password(Vec::with_capacity(256));
    loop {
        let mut byte = [0];
        if check(libbreenix::io::read(Fd::STDIN, &mut byte))? == 0 { return Err("password input ended".into()); }
        if byte[0] == b'\n' { break; }
        if secret.0.len() == 256 || byte[0] == 0 { return Err("password input invalid".into()); }
        secret.0.push(byte[0]);
    }
    check(termios::tcsetattr(Fd::STDIN, termios::TCSANOW, &saved))?;
    core::mem::forget(restore);
    check(std::io::stdout().write_all(b"\n"))?;
    Ok(secret)
}

fn run() -> Result<(), String> {
    let user = std::env::args().nth(1).ok_or("missing login name")?;
    let secret = password()?;
    let shadow = std::fs::read_to_string("/etc/shadow").unwrap_or_default();
    let hash = shadow.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next() == Some(user.as_str()) { fields.next() } else { None }
    }).unwrap_or("!");
    // The staged account is root. Missing/locked/empty/unsupported hashes refuse.
    let accepted = user == "root" && verify(&secret.0, hash);
    drop(secret);
    if !accepted {
        check(libbreenix::time::sleep_ms(2000))?;
        check(std::io::stdout().write_all(b"Login incorrect\n"))?;
        return Ok(());
    }
    let passwd = check(std::fs::read_to_string("/etc/passwd"))?;
    let fields: Vec<&str> = passwd.lines().find(|l| l.starts_with("root:")).ok_or("missing account")?.split(':').collect();
    if fields.len() != 7 { return Err("invalid account".into()); }
    let uid: u32 = check(fields[2].parse())?;
    let gid: u32 = check(fields[3].parse())?;
    let home = check(CString::new(fields[5]))?;
    let shell = check(CString::new(fields[6]))?;
    if !fields[5].starts_with('/') || !fields[6].starts_with('/') { return Err("invalid account paths".into()); }
    // Discard inherited environment and install only the login environment.
    let environment = [format!("HOME={}", fields[5]), format!("USER={}", user), format!("LOGNAME={}", user),
        format!("SHELL={}", fields[6]), "PATH=/bin:/sbin:/usr/bin".into(), "TERM=vt100".into()];
    let environment: Vec<CString> = check(environment.iter().map(|s| CString::new(s.as_bytes())).collect::<Result<_, _>>())?;
    let mut envp: Vec<*const u8> = environment.iter().map(|s| s.as_ptr().cast()).collect();
    envp.push(core::ptr::null());
    for sig in [signal::SIGINT, signal::SIGQUIT, signal::SIGTSTP] { check(signal::sigaction(sig, Some(&signal::Sigaction::default()), None))?; }
    if unsafe { setgroups(0, core::ptr::null()) } != 0 { return Err("setgroups failed".into()); }
    if unsafe { setgid(gid) } != 0 { return Err("setgid failed".into()); }
    if unsafe { setuid(uid) } != 0 { return Err("setuid failed".into()); }
    check(process::chdir(home.as_bytes_with_nul()))?;
    let arg0 = check(CString::new(format!("-{}", fields[6].rsplit('/').next().ok_or("invalid shell")?)))?;
    let argv = [arg0.as_ptr().cast(), core::ptr::null()];
    check(process::execve(shell.as_bytes_with_nul(), argv.as_ptr(), envp.as_ptr()))?;
    Ok(())
}
fn main() {
    if let Err(e) = run() { eprintln!("login: {}", e); std::process::exit(1); }
}
