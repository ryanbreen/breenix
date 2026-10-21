//! su [-] [-c command] [name]: run a shell as another account
//! (`docs/users/design.md`, section 3). Installed set-user-ID root at `/bin/su`.
//!
//! - The target is `root` unless named. A caller other than root types the
//!   target's password on its terminal; becoming uid 0 also needs the caller's
//!   account to be in group wheel (checked before any password is asked).
//! - `-` (or `-l`) gives a login environment: the target's clean environment,
//!   its umask from `/etc/login.defs`, its home and a login shell. Without it the
//!   caller's environment and directory stay, with HOME, SHELL, USER and LOGNAME
//!   set to the target's (and PATH reset when the target is root).
//! - `-c command` runs `shell -c command` instead of an interactive shell.
//! - Each attempt is recorded in `/var/log/auth.log`; a refusal waits
//!   FAIL_DELAY seconds and says `su: Authentication failure`.
use libbreenix::accounts::{self, session};
use std::ffi::CString;
use std::path::Path;

struct Request {
    login: bool,
    command: Option<String>,
    target: String,
}

fn parse(args: &[String]) -> Result<Request, String> {
    let mut request = Request { login: false, command: None, target: "root".into() };
    let mut named = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-" | "-l" | "--login" if !named => request.login = true,
            "-c" if request.command.is_none() => {
                i += 1;
                request.command = Some(args.get(i).ok_or("-c needs a command")?.clone());
            }
            name if !named && !name.starts_with('-') => {
                request.target = name.into();
                named = true;
            }
            other => return Err(format!("unexpected argument {other}")),
        }
        i += 1;
    }
    Ok(request)
}

fn refuse(caller: &str, target: &str, tty: &str, why: &str, delay: Option<&session::LoginDefs>) -> ! {
    if let Some(defs) = delay { defs.fail_wait(); }
    let record = session::Record { program: "su", user: caller, target, tty, refused: Some(why), cmd: "" };
    let _ = accounts::log(Path::new("/"), &record);
    eprintln!("su: {}", if why == "not-wheel" { "only members of group wheel may become root" } else { "Authentication failure" });
    std::process::exit(1)
}

fn run() -> Result<std::convert::Infallible, String> {
    let root = Path::new("/");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = parse(&args).map_err(|e| format!("{e}\nusage: su [-] [-c command] [name]"))?;
    accounts::require_root().map_err(|_| "su must be installed set-user-ID root".to_string())?;
    let defs = session::LoginDefs::read(root);
    let caller_uid = session::real_uid();
    let caller = session::caller_name(root);
    let tty = session::terminal_name(0).unwrap_or_else(|| "-".into());
    let entry = match session::validate(root, &request.target) {
        Ok(entry) => entry,
        Err(why) => refuse(&caller, &request.target, &tty, &why.replace(' ', "-"), Some(&defs)),
    };
    let target = entry.account.clone();
    if caller_uid != 0 {
        let wheel = session::account_by_uid(root, caller_uid).is_some_and(|a| a.groups.contains(&session::WHEEL_GROUP));
        if target.uid == 0 && !wheel {
            refuse(&caller, &target.name, &tty, "not-wheel", None);
        }
        let terminal = libbreenix::fs::open("/dev/tty", libbreenix::fs::O_RDWR)
            .map_err(|e| format!("no terminal for the password: {e}"))?;
        let secret = session::read_password(terminal.raw() as i32, "Password: ")?;
        let _ = libbreenix::io::close(terminal);
        if !entry.password_matches(&secret.0) {
            drop(secret);
            refuse(&caller, &target.name, &tty, "password", Some(&defs));
        }
    }
    let cmd = request.command.clone().unwrap_or_else(|| target.shell.clone());
    let record = session::Record { program: "su", user: &caller, target: &target.name, tty: &tty, refused: None, cmd: &cmd };
    accounts::log(root, &record)?;
    session::take_identity(&target)?;
    if request.login {
        session::set_umask(&defs);
        let term = std::env::var("TERM").ok();
        let environment = session::login_environment(&target, term.as_deref());
        return session::exec_shell(&target, &environment, true, request.command.as_deref());
    }
    let replaced = ["HOME", "SHELL", "USER", "LOGNAME", "PATH"];
    let mut environment: Vec<String> = std::env::vars()
        .filter(|(key, _)| !replaced.contains(&key.as_str()))
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let path = if target.uid == 0 { session::SECURE_PATH.to_string() } else { std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()) };
    environment.extend([
        format!("HOME={}", target.home),
        format!("SHELL={}", target.shell),
        format!("USER={}", target.name),
        format!("LOGNAME={}", target.name),
        format!("PATH={path}"),
    ]);
    let text = |s: &str| CString::new(s).map_err(|_| "text contains NUL".to_string());
    let mut argv = vec![text(target.shell.rsplit('/').next().unwrap_or("sh"))?];
    if let Some(command) = &request.command {
        argv.push(text("-c")?);
        argv.push(text(command)?);
    }
    session::exec(&target.shell, &argv, &environment)
}

fn main() {
    match run() {
        Ok(never) => match never {},
        Err(e) => {
            eprintln!("su: {e}");
            std::process::exit(1);
        }
    }
}
