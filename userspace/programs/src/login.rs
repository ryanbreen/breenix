//! Console authentication for any account (`docs/users/design.md`, section 3).
//!
//! getty passes the login name. login reads the password with echo off, then
//! checks the account through the accounts library: a strict passwd entry, a
//! shadow line that is not expired and whose SHA-512-crypt hash matches, and
//! a shell listed in `/etc/shells`. Root logs in with its own password on the
//! local console and virtual terminals; on any other line (a pseudo-terminal)
//! uid 0 is refused unless `/etc/login.defs` says `ROOT_LOGIN yes` (ruling
//! S14). A refusal waits `FAIL_DELAY` seconds and prints
//! `Login incorrect` whatever the reason. A session gets the terminal (mode
//! 0620, group tty; the system console stays root's), the account's groups
//! and IDs, the umask from login.defs, a clean environment and its home, then
//! the shell. Each attempt is recorded in `/var/log/auth.log`.
use libbreenix::accounts::{self, session};
use libbreenix::{signal, termios, types::Fd};
use std::io::Write;
use std::path::Path;

fn check<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

/// The kernel's console input-provenance report (refused, accepted) exists
/// for the console and the virtual terminals only.
fn input_proof(console: bool, action: i32) -> Result<(), String> {
    if console {
        check(termios::console_input_proof(Fd::STDIN, action))?;
    }
    Ok(())
}

fn log(name: &str, tty: &str, refused: Option<&str>) {
    let record = session::Record { program: "login", user: name, target: name, tty, refused, cmd: "" };
    if let Err(why) = accounts::log(Path::new("/"), &record) {
        eprintln!("login: auth log: {why}");
    }
}

fn run() -> Result<(), String> {
    let root = Path::new("/");
    let user = std::env::args().nth(1).ok_or("missing login name")?;
    let defs = session::LoginDefs::read(root);
    let tty = session::terminal_name(0).ok_or("standard input is not a terminal")?;
    let console = tty == "console" || tty.starts_with("tty");
    for sig in [signal::SIGINT, signal::SIGQUIT, signal::SIGTSTP] {
        let action = signal::Sigaction { handler: signal::SIG_IGN, ..signal::Sigaction::default() };
        check(signal::sigaction(sig, Some(&action), None))?;
    }
    let secret = session::read_password(0, "Password: ")?;
    let entry = session::validate(root, &user);
    let refusal = match &entry {
        Err(why) => Some(why.clone()),
        Ok(entry) if !entry.password_matches(&secret.0) => Some("password".into()),
        Ok(entry) if entry.account.uid == 0 && !session::root_may_log_in(&defs, &tty) => Some("root-login".into()),
        Ok(_) => None,
    };
    drop(secret);
    if let Some(why) = refusal {
        defs.fail_wait();
        input_proof(console, 1)?;
        log(&user, &tty, Some(&why.replace(' ', "-")));
        check(std::io::stdout().write_all(b"Login incorrect\n"))?;
        return Ok(());
    }
    let account = entry.map_err(|e| e.to_string())?.account;
    for sig in [signal::SIGINT, signal::SIGQUIT, signal::SIGTSTP] {
        check(signal::sigaction(sig, Some(&signal::Sigaction::default()), None))?;
    }
    // The provenance report and the log are root's: make them before the drop.
    input_proof(console, 2)?;
    log(&account.name, &tty, None);
    session::become_account(&account, &defs, Some(0))?;
    let environment = session::login_environment(&account, Some("breenix"));
    match session::exec_shell(&account, &environment, true, None)? {}
}

fn main() {
    if let Err(e) = run() {
        eprintln!("login: {}", e);
        std::process::exit(1);
    }
}
