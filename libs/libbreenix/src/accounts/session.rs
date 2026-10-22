//! Starting a session as an account, shared by login, su, sudo and bsshd
//! (`docs/users/design.md`, section 3, "Identity, PAM-free"). There is no
//! pluggable module layer: each program authenticates its own way (a password,
//! an SSH key), then calls `validate` and `become_account` here.
//!
//! 1. `validate`: a strict passwd entry, a shadow line, not expired, and a shell
//!    listed in `/etc/shells`.
//! 2. `grant_terminal`: the session's terminal becomes the account's, mode 0620,
//!    group tty. The system console stays root's 0600.
//! 3. `take_identity`: supplementary groups, then setresgid, then setresuid,
//!    each read back; for a target other than root, `setuid(0)` must then fail.
//! 4. The umask from `/etc/login.defs`.
//! 5. `exec_shell`: the account's environment, its home, its shell.
use super::{parse, today, Account, Database, Result};
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// The group that owns terminals (`docs/users/design.md`, section 4).
pub const TTY_GROUP: u32 = 5;
/// The group whose members may become root through su and the default sudo policy.
pub const WHEEL_GROUP: u32 = 10;
/// The PATH an administrative command receives (sudo resets to it).
pub const SECURE_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

/// `/etc/login.defs`: one `KEY value` per line, `#` comments. A missing or
/// unreadable file, or a malformed value, keeps the secure default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginDefs {
    /// The file creation mask a session starts with (`UMASK`, octal; default 022).
    pub umask: u32,
    /// Whether login accepts uid 0 on a remote line, a pseudo-terminal
    /// (`ROOT_LOGIN yes`; default no). Root logs in on the local console and
    /// virtual terminals with its own password whatever this says, and bsshd
    /// refuses uid 0 whatever it says (D4, ruling S14; `root_may_log_in`).
    pub root_login: bool,
    /// Seconds a refused authentication waits before it answers (`FAIL_DELAY`; default 2).
    pub fail_delay: u32,
}
impl Default for LoginDefs {
    fn default() -> Self {
        Self {
            umask: 0o022,
            root_login: false,
            fail_delay: 2,
        }
    }
}
impl LoginDefs {
    pub fn parse(text: &str) -> Self {
        let mut defs = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let (Some(key), Some(value), None) = (words.next(), words.next(), words.next()) else {
                continue;
            };
            match key {
                "UMASK" => {
                    if let Ok(mask) = u32::from_str_radix(value, 8) {
                        if mask <= 0o777 {
                            defs.umask = mask;
                        }
                    }
                }
                "ROOT_LOGIN" => defs.root_login = value == "yes",
                "FAIL_DELAY" => {
                    if let Ok(seconds) = value.parse::<u32>() {
                        defs.fail_delay = seconds.min(60);
                    }
                }
                _ => {}
            }
        }
        defs
    }
    pub fn read(root: &Path) -> Self {
        fs::read_to_string(root.join("etc/login.defs"))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }
    /// Wait the failure delay. The wait is fixed: it does not depend on why
    /// the authentication failed.
    pub fn fail_wait(&self) {
        std::thread::sleep(std::time::Duration::from_secs(u64::from(self.fail_delay)));
    }
}

/// The login shells `/etc/shells` lists: absolute paths, one per line.
pub fn shells(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("etc/shells"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && parse::absolute(line.as_bytes()))
        .map(str::to_owned)
        .collect()
}

/// How a session was authenticated. A locked password (`!`) refuses a
/// password but leaves key logins; expiry refuses both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Password,
    Key,
    /// su or sudo started by root, or sudo under a `nopass` rule: no
    /// credential is asked for, the account checks still apply.
    Trusted,
}

/// An account that passed `validate`, with its shadow password field.
pub struct Entry {
    pub account: Account,
    hash: String,
    password_usable: bool,
}
impl Entry {
    /// Whether `password` is this account's: a SHA-512-crypt hash that has
    /// not aged out matches it. Locked, `*`, empty and other hashes do not match.
    pub fn password_matches(&self, password: &[u8]) -> bool {
        self.password_usable && crate::crypto::sha512_crypt::verify(password, &self.hash)
    }
}

/// Step 1: the account `name` may start a session at all. Each refusal says
/// why, for the authentication log; programs show the user only a generic
/// message.
pub fn validate(root: &Path, name: &str) -> Result<Entry> {
    let db = Database::read(root)?;
    let account = db.account(name).ok_or("unknown or malformed account")?;
    let shadow = db.shadow(name).ok_or("no usable shadow entry")?;
    let today = today();
    if shadow.expired(today) {
        return Err("account expired".into());
    }
    if !shells(root).iter().any(|shell| *shell == account.shell) {
        return Err("login shell not listed in /etc/shells".into());
    }
    let hash = std::str::from_utf8(shadow.fields[1]).unwrap_or("").to_owned();
    let password_usable = shadow.password_allowed(today);
    Ok(Entry {
        account,
        hash,
        password_usable,
    })
}

/// Look up an account (for a caller's name) without session checks.
pub fn account_by_uid(root: &Path, uid: u32) -> Option<Account> {
    let db = Database::read(root).ok()?;
    db.passwd
        .lines()
        .filter_map(|l| parse::Passwd::parse(l.as_bytes()))
        .find(|p| p.uid == uid)
        .and_then(|p| db.account(std::str::from_utf8(p.fields[0]).ok()?))
}

fn raw(n: u64, a: u64, b: u64, c: u64) -> i64 {
    // SAFETY: each caller passes pointers to buffers that outlive the call.
    unsafe { crate::syscall::raw::syscall3(n, a, b, c) as i64 }
}
fn checked(what: &str, ret: i64) -> Result<i64> {
    if ret < 0 {
        Err(format!("{what}: errno {}", -ret))
    } else {
        Ok(ret)
    }
}

/// Whether login may start a root session on the terminal named `tty`
/// (`terminal_name`): on the local console and virtual terminals whatever
/// `ROOT_LOGIN` says, on another line only under `ROOT_LOGIN yes` (ruling S14).
pub fn root_may_log_in(defs: &LoginDefs, tty: &str) -> bool {
    let local = tty == "console"
        || tty.strip_prefix("tty").is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6"));
    local || defs.root_login
}

/// The session's terminal by name: `console`, `tty1`..`tty6` or `pts/N`;
/// no name when `fd` is not one of them.
pub fn terminal_name(fd: i32) -> Option<String> {
    let mut st = crate::fs::Stat::default();
    if raw(crate::syscall::nr::FSTAT, fd as u64, &mut st as *mut _ as u64, 0) < 0
        || st.st_mode & 0o170000 != 0o020000
    {
        return None;
    }
    let same = |path: &Path| fs::metadata(path).is_ok_and(|m| m.rdev() == st.st_rdev);
    if same(Path::new("/dev/console")) {
        return Some("console".into());
    }
    if let Some(n) = (1..=6).find(|n| same(Path::new(&format!("/dev/tty{n}")))) {
        return Some(format!("tty{n}"));
    }
    fs::read_dir("/dev/pts").ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().into_string().ok()?;
        same(&entry.path()).then(|| format!("pts/{name}"))
    })
}

/// Step 2: give the terminal on `fd` to the account, mode 0620, group tty.
/// The system console is root's 0600 and is not given away.
pub fn grant_terminal(fd: i32, account: &Account) -> Result<()> {
    match terminal_name(fd).as_deref() {
        None => Err("standard input is not a terminal".into()),
        Some("console") => Ok(()),
        Some(_) => set_terminal(fd, account.uid),
    }
}

/// Return the terminal on `fd` to root (getty does this when it serves the
/// terminal again, and bsshd when a PTY session ends).
pub fn return_terminal(fd: i32) -> Result<()> {
    match terminal_name(fd).as_deref() {
        None | Some("console") => Ok(()),
        Some(_) => set_terminal(fd, 0),
    }
}

fn set_terminal(fd: i32, uid: u32) -> Result<()> {
    checked(
        "fchown terminal",
        raw(crate::syscall::nr::FCHOWN, fd as u64, uid as u64, TTY_GROUP as u64),
    )?;
    checked(
        "fchmod terminal",
        raw(crate::syscall::nr::FCHMOD, fd as u64, 0o620, 0),
    )
    .map(|_| ())
}

/// The owner of the terminal on `fd`.
pub fn terminal_owner(fd: i32) -> Option<u32> {
    let mut st = crate::fs::Stat::default();
    (raw(crate::syscall::nr::FSTAT, fd as u64, &mut st as *mut _ as u64, 0) == 0).then_some(st.st_uid)
}

fn ids(n: u64) -> [u32; 3] {
    let mut v = [u32::MAX; 3];
    raw(
        n,
        &mut v[0] as *mut u32 as u64,
        &mut v[1] as *mut u32 as u64,
        &mut v[2] as *mut u32 as u64,
    );
    v
}

/// The caller's supplementary groups, sorted.
pub fn groups() -> Vec<u32> {
    let mut list = vec![0u32; 256];
    let n = raw(
        crate::syscall::nr::GETGROUPS,
        list.len() as u64,
        list.as_mut_ptr() as u64,
        0,
    );
    list.truncate(n.max(0) as usize);
    list.sort_unstable();
    list.dedup();
    list
}

/// The caller's real user ID.
pub fn real_uid() -> u32 {
    ids(crate::syscall::nr::GETRESUID)[0]
}

/// Step 3: take the account's groups, then its group IDs, then its user IDs,
/// and read each back. For a target other than root, regaining uid 0 must
/// then be refused; su and sudo becoming root skip that assertion.
pub fn take_identity(account: &Account) -> Result<()> {
    let (u, g) = (account.uid as u64, account.gid as u64);
    checked(
        "setgroups",
        raw(
            crate::syscall::nr::SETGROUPS,
            account.groups.len() as u64,
            account.groups.as_ptr() as u64,
            0,
        ),
    )?;
    checked("setresgid", raw(crate::syscall::nr::SETRESGID, g, g, g))?;
    checked("setresuid", raw(crate::syscall::nr::SETRESUID, u, u, u))?;
    let mut wanted = account.groups.clone();
    wanted.sort_unstable();
    wanted.dedup();
    if ids(crate::syscall::nr::GETRESUID) != [account.uid; 3]
        || ids(crate::syscall::nr::GETRESGID) != [account.gid; 3]
        || groups() != wanted
    {
        return Err(format!("credentials differ after becoming {}", account.name));
    }
    if account.uid != 0 && raw(crate::syscall::nr::SETUID, 0, 0, 0) >= 0 {
        return Err(format!("{} could take uid 0 back", account.name));
    }
    Ok(())
}

/// Step 4: the session's file creation mask.
pub fn set_umask(defs: &LoginDefs) {
    raw(crate::syscall::nr::UMASK, defs.umask as u64, 0, 0);
}

/// Steps 2 to 4 together: what login, su, sudo and bsshd each call once the
/// caller is authenticated. `terminal` is the descriptor whose terminal the
/// session gets, when it has one.
pub fn become_account(account: &Account, defs: &LoginDefs, terminal: Option<i32>) -> Result<()> {
    if let Some(fd) = terminal {
        grant_terminal(fd, account)?;
    }
    take_identity(account)?;
    set_umask(defs);
    Ok(())
}

/// A login environment, replacing the caller's: HOME, USER, LOGNAME, SHELL,
/// PATH and a TERM made of printable characters.
pub fn login_environment(account: &Account, term: Option<&str>) -> Vec<String> {
    let path = if account.uid == 0 {
        "/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/bin"
    } else {
        "/usr/bin:/bin:/usr/local/bin"
    };
    let mut environment = vec![
        format!("HOME={}", account.home),
        format!("USER={}", account.name),
        format!("LOGNAME={}", account.name),
        format!("SHELL={}", account.shell),
        format!("PATH={path}"),
    ];
    if let Some(term) = term.filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_graphic())) {
        environment.push(format!("TERM={term}"));
    }
    environment
}

/// Enter the account's home, or `/` with a message when it is unavailable.
pub fn enter_home(account: &Account) -> Result<()> {
    if std::env::set_current_dir(&account.home).is_err() {
        eprintln!("home directory {} is not available; starting in /", account.home);
        std::env::set_current_dir("/").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Step 5: enter the home (or `/`, with a message, when it is unavailable)
/// and replace this process with the account's shell. A login shell's
/// `argv[0]` is `-` and the shell's name; `command` runs `shell -c command`.
pub fn exec_shell(account: &Account, environment: &[String], login: bool, command: Option<&str>) -> Result<std::convert::Infallible> {
    enter_home(account)?;
    let text = |s: &str| CString::new(s).map_err(|_| "text contains NUL".to_string());
    let base = account.shell.rsplit('/').next().unwrap_or("sh");
    let mut argv = vec![text(&if login { format!("-{base}") } else { base.to_owned() })?];
    if let Some(command) = command {
        argv.push(text("-c")?);
        argv.push(text(command)?);
    }
    exec(&account.shell, &argv, environment)
}

/// Replace this process with `path`, `argv` and exactly `environment`.
pub fn exec(path: &str, argv: &[CString], environment: &[String]) -> Result<std::convert::Infallible> {
    let path = CString::new(path).map_err(|_| "path contains NUL".to_string())?;
    let env: Vec<CString> = environment
        .iter()
        .map(|e| CString::new(e.as_str()).map_err(|_| "environment contains NUL".to_string()))
        .collect::<Result<_>>()?;
    let mut argp: Vec<*const u8> = argv.iter().map(|a| a.as_ptr().cast()).collect();
    argp.push(core::ptr::null());
    let mut envp: Vec<*const u8> = env.iter().map(|e| e.as_ptr().cast()).collect();
    envp.push(core::ptr::null());
    let ret = raw(
        crate::syscall::nr::EXEC,
        path.as_ptr() as u64,
        argp.as_ptr() as u64,
        envp.as_ptr() as u64,
    );
    Err(format!("exec {}: errno {}", path.to_string_lossy(), -ret))
}

/// Read a password from the terminal on `fd` with echo off, after printing
/// `prompt` there. Input ends at a newline; at most 1024 bytes.
pub fn read_password(fd: i32, prompt: &str) -> Result<Secret> {
    use crate::termios::{self, lflag};
    use crate::types::Fd;
    let fd = Fd::from_raw(fd as u64);
    let mut saved = termios::Termios::default();
    termios::tcgetattr(fd, &mut saved).map_err(|e| format!("terminal: {e}"))?;
    let mut hidden = saved;
    hidden.c_lflag &= !(lflag::ECHO | lflag::ECHONL);
    termios::tcsetattr(fd, termios::TCSAFLUSH, &hidden).map_err(|e| format!("terminal: {e}"))?;
    let _ = crate::io::write(fd, prompt.as_bytes());
    let mut secret = Secret(Vec::with_capacity(128));
    let result = loop {
        let mut byte = [0u8];
        match crate::io::read(fd, &mut byte) {
            Ok(0) => break Err("password input ended".to_string()),
            Ok(_) if byte[0] == b'\n' || byte[0] == b'\r' => break Ok(()),
            Ok(_) if byte[0] == 0 || secret.0.len() == 1024 => break Err("password input invalid".to_string()),
            Ok(_) => secret.0.push(byte[0]),
            Err(crate::error::Error::Os(crate::errno::Errno::EINTR)) => continue,
            Err(e) => break Err(format!("read password: {e}")),
        }
    };
    let _ = termios::tcsetattr(fd, termios::TCSANOW, &saved);
    let _ = crate::io::write(fd, b"\n");
    result.map(|()| secret)
}

/// A password in memory, overwritten when dropped.
pub struct Secret(pub Vec<u8>);
impl Drop for Secret {
    fn drop(&mut self) {
        for b in &mut self.0 {
            // SAFETY: `b` points into this vector's live buffer.
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }
}

/// One authentication-log record (`docs/users/design.md`, section 3,
/// "Logging"): `<ISO time> <program>[pid] user= target= tty= result= cmd=`.
pub struct Record<'a> {
    pub program: &'a str,
    /// The account acting: the real user's name, or its uid.
    pub user: &'a str,
    pub target: &'a str,
    pub tty: &'a str,
    /// The refusal's reason (`refused:<reason>`), or no reason for `ok`.
    pub refused: Option<&'a str>,
    pub cmd: &'a str,
}

/// The real user's name for a record, or the number when no account names it.
pub fn caller_name(root: &Path) -> String {
    let uid = real_uid();
    account_by_uid(root, uid).map(|a| a.name).unwrap_or_else(|| uid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_defs_keep_secure_defaults() {
        assert_eq!(LoginDefs::parse(""), LoginDefs::default());
        let defs = LoginDefs::parse("# site\nUMASK 027\nROOT_LOGIN yes\nFAIL_DELAY 3\n");
        assert_eq!((defs.umask, defs.root_login, defs.fail_delay), (0o027, true, 3));
        // Malformed values and anything but an exact `yes` keep the defaults.
        let defs = LoginDefs::parse("UMASK 99\nROOT_LOGIN Yes\nFAIL_DELAY -1\nROOT_LOGIN yes extra\n");
        assert_eq!(defs, LoginDefs::default());
    }

    #[test]
    fn root_logs_in_locally_and_remotely_only_when_permitted() {
        let shipped = LoginDefs::parse("ROOT_LOGIN no\n");
        for tty in ["console", "tty1", "tty6"] {
            assert!(root_may_log_in(&shipped, tty), "{tty}");
        }
        for tty in ["pts/0", "tty7", "tty", "ttyS0", "-"] {
            assert!(!root_may_log_in(&shipped, tty), "{tty}");
        }
        assert!(root_may_log_in(&LoginDefs::parse("ROOT_LOGIN yes\n"), "pts/3"));
    }

    #[test]
    fn login_environment_is_the_accounts() {
        let account = Account {
            name: "ua".into(),
            uid: 2001,
            gid: 2001,
            gecos: String::new(),
            home: "/home/ua".into(),
            shell: "/bin/bsh".into(),
            groups: vec![2001, 2100],
        };
        let env = login_environment(&account, Some("xterm"));
        assert!(env.contains(&"HOME=/home/ua".to_string()) && env.contains(&"TERM=xterm".to_string()));
        assert!(env.iter().any(|e| e == "PATH=/usr/bin:/bin:/usr/local/bin"));
        assert!(!login_environment(&account, Some("bad term")).iter().any(|e| e.starts_with("TERM=")));
    }
}
