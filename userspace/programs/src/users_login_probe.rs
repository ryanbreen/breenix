//! The users suite's login shell (`suite_users_login.rs`). login, su, sudo and
//! bsshd start it as an account's shell or command; it prints one line with
//! what the session gave it and exits 0:
//!
//! `PROBE uid=R,E,S gid=R,E,S groups=G,... umask=0MMM cwd=... argv0=... tty=UID:GID:MODE
//!  args=... HOME=... USER=...` and each other environment variable the suite reads.
//!
//! Values with a space or `=` are shown with `_` there, so each `key=value`
//! stays one word.
use libbreenix::syscall::{nr, raw};

fn ids(n: u64) -> String {
    let mut v = [u32::MAX; 3];
    // SAFETY: the three pointers name this frame's array.
    unsafe {
        raw::syscall3(n, &mut v[0] as *mut u32 as u64, &mut v[1] as *mut u32 as u64, &mut v[2] as *mut u32 as u64);
    }
    format!("{},{},{}", v[0], v[1], v[2])
}

fn word(text: &str) -> String {
    let w: String = text.chars().map(|c| if c.is_ascii_graphic() && c != '=' { c } else { '_' }).collect();
    if w.is_empty() { "-".into() } else { w }
}

fn main() {
    let groups = libbreenix::accounts::session::groups();
    // SAFETY: umask takes and returns a plain mask.
    let mask = unsafe { raw::syscall1(nr::UMASK, 0) };
    // SAFETY: as above, restoring the mask read.
    unsafe { raw::syscall1(nr::UMASK, mask) };
    let mut st = libbreenix::fs::Stat::default();
    // SAFETY: `st` outlives the call.
    let tty = if unsafe { raw::syscall2(nr::FSTAT, 0, &mut st as *mut _ as u64) } == 0 && st.st_mode & 0o170000 == 0o020000 {
        format!("{}:{}:{:o}", st.st_uid, st.st_gid, st.st_mode & 0o7777)
    } else {
        "-".into()
    };
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "-".into());
    let mut line = format!(
        "PROBE uid={} gid={} groups={} umask={:04o} cwd={} argv0={} tty={} args={}",
        ids(nr::GETRESUID),
        ids(nr::GETRESGID),
        groups.iter().map(u32::to_string).collect::<Vec<_>>().join(","),
        mask,
        word(&cwd),
        word(args.first().map(String::as_str).unwrap_or("")),
        tty,
        word(&args[1..].join(" ")),
    );
    for key in ["HOME", "USER", "LOGNAME", "SHELL", "PATH", "TERM", "SUDO_USER", "SUDO_COMMAND", "USERS_SUITE_KEEP"] {
        line.push_str(&format!(" {key}={}", std::env::var(key).map(|v| word(&v)).unwrap_or_else(|_| "-".into())));
    }
    println!("{line}");
}
