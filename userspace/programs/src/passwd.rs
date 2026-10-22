//! passwd [name]: change a password (`docs/users/design.md`, section 3).
//! Installed set-user-ID root at `/usr/bin/passwd`.
//!
//! A user changes their own password after typing the current one; root may
//! set any account's. The new password is typed twice and must hold 8 to 1024
//! bytes. The accounts library checks the caller from the kernel's real user ID,
//! writes a fresh 100000-round SHA-512-crypt hash under the password lock and
//! records the change in `/var/log/auth.log`.
use libbreenix::accounts::{self, session};
use std::path::Path;

fn run() -> Result<(), String> {
    let root = Path::new("/");
    let args: Vec<String> = std::env::args().skip(1).collect();
    accounts::require_root().map_err(|_| "passwd must be installed set-user-ID root".to_string())?;
    let caller = session::real_uid();
    let name = match args.as_slice() {
        [] => session::account_by_uid(root, caller).ok_or("you have no account")?.name,
        [name] if !name.starts_with('-') => name.clone(),
        _ => return Err("usage: passwd [name]".into()),
    };
    let terminal = libbreenix::fs::open("/dev/tty", libbreenix::fs::O_RDWR)
        .map_err(|e| format!("no terminal for the password: {e}"))?;
    let fd = terminal.raw() as i32;
    let old = if caller == 0 { session::Secret(Vec::new()) } else { session::read_password(fd, "Current password: ")? };
    let new = session::read_password(fd, "New password: ")?;
    let again = session::read_password(fd, "Retype new password: ")?;
    let _ = libbreenix::io::close(terminal);
    if new.0 != again.0 {
        return Err("the passwords do not match".into());
    }
    accounts::change_password(root, &name, &old.0, &new.0)?;
    println!("passwd: password updated for {name}");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("passwd: {e}");
        std::process::exit(1);
    }
}
