//! sudo [-u user] [-i] [-n] [command [args...]]: run a command as another
//! account under `/etc/sudo.policy` (`docs/users/design.md`, section 3).
//! Installed set-user-ID root at `/usr/bin/sudo`. The name follows the
//! operator's habit; the policy grammar (`libbreenix::accounts::policy`) is
//! Breenix's own.
//!
//! - The policy must be a regular root:root file at mode 0440 with one link.
//!   An unavailable or invalid policy grants no authorization.
//! - The command resolves to an absolute path (through PATH
//!   `/usr/sbin:/usr/bin:/sbin:/bin` when it has no `/`) before the policy check.
//! - A caller other than root types its own password unless the rule says
//!   `nopass`. A success is remembered for 5 minutes for that session and
//!   terminal in `/run/sudo/<uid>/<session>`. `-n` does not ask: it fails when a
//!   password would be needed.
//! - The command gets a reset environment (PATH as above, the target's HOME,
//!   USER and LOGNAME, TERM kept, SUDO_USER and SUDO_COMMAND), umask 022, and
//!   the target's groups and IDs. `-i` starts in the target's home with SHELL
//!   set, and runs the target's login shell when no command is given. The
//!   command runs as its argument vector, not through a shell.
//! - Each attempt is recorded in `/var/log/auth.log`.
use libbreenix::accounts::{self, policy, session};
use std::ffi::CString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

const POLICY: &str = "/etc/sudo.policy";
const CACHE: &str = "/run/sudo";
const CACHE_SECONDS: u64 = 300;

struct Request {
    target: String,
    login: bool,
    no_prompt: bool,
    command: Vec<String>,
}

fn parse(args: &[String]) -> Result<Request, String> {
    let mut request = Request { target: "root".into(), login: false, no_prompt: false, command: Vec::new() };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-u" => {
                i += 1;
                request.target = args.get(i).ok_or("-u needs an account")?.clone();
            }
            "-i" => request.login = true,
            "-n" => request.no_prompt = true,
            "--" => {
                i += 1;
                break;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            _ => break,
        }
        i += 1;
    }
    request.command = args[i.min(args.len())..].to_vec();
    if request.command.is_empty() && !request.login {
        return Err("a command is required".into());
    }
    Ok(request)
}

/// An absolute path for `name`: as given when it has a `/` (made absolute
/// against the working directory), else the first executable regular file in
/// the secure PATH.
fn resolve(name: &str) -> Option<String> {
    let executable = |path: &Path| fs::metadata(path).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0);
    if name.contains('/') {
        let path = std::env::current_dir().ok()?.join(name);
        return executable(&path).then(|| path.to_string_lossy().into_owned());
    }
    session::SECURE_PATH
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|path| executable(path))
        .map(|path| path.to_string_lossy().into_owned())
}

fn cache_path(uid: u32, sid: i32) -> String {
    format!("{CACHE}/{uid}/{sid}")
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A remembered authentication for this session and terminal, still fresh.
fn cached(uid: u32, sid: i32, tty: &str) -> bool {
    let path = cache_path(uid, sid);
    let Ok(meta) = fs::symlink_metadata(&path) else { return false };
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o7777 != 0o600 {
        return false;
    }
    let Ok(text) = fs::read_to_string(&path) else { return false };
    let mut words = text.split_whitespace();
    let (Some(Ok(at)), Some(recorded)) = (words.next().map(str::parse::<u64>), words.next()) else { return false };
    let now = now();
    recorded == tty && at <= now && now - at < CACHE_SECONDS
}

fn remember(uid: u32, sid: i32, tty: &str) -> Result<(), String> {
    for dir in [CACHE.to_string(), format!("{CACHE}/{uid}")] {
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{dir}: {e}")),
        }
        let meta = fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(format!("{dir} has an unsafe owner or mode"));
        }
    }
    let path = cache_path(uid, sid);
    let scratch = format!("{path}+");
    let _ = fs::remove_file(&scratch);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&scratch)
        .map_err(|e| format!("{scratch}: {e}"))?;
    file.write_all(format!("{} {tty}\n", now()).as_bytes()).map_err(|e| e.to_string())?;
    fs::rename(&scratch, &path).map_err(|e| format!("{path}: {e}"))
}

fn refuse(caller: &str, target: &str, tty: &str, cmd: &str, why: &str, message: &str) -> ! {
    let record = session::Record { program: "sudo", user: caller, target, tty, refused: Some(why), cmd };
    let _ = accounts::log(Path::new("/"), &record);
    eprintln!("sudo: {message}");
    std::process::exit(1)
}

fn run() -> Result<std::convert::Infallible, String> {
    let root = Path::new("/");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let caller_uid = session::real_uid();
    let caller = session::account_by_uid(root, caller_uid);
    let caller_name = caller.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| caller_uid.to_string());
    let tty = session::terminal_name(0).unwrap_or_else(|| "-".into());
    let request = parse(&args).unwrap_or_else(|e| {
        refuse(&caller_name, "-", &tty, &args.join(" "), "usage",
            &format!("{e}\nusage: sudo [-u user] [-i] [-n] [command [args...]]"))
    });
    let shown = request.command.join(" ");
    if accounts::require_root().is_err() {
        refuse(&caller_name, &request.target, &tty, &shown, "setuid", "sudo must be installed set-user-ID root");
    }
    let defs = session::LoginDefs::read(root);
    let entry = match session::validate(root, &request.target) {
        Ok(entry) => entry,
        Err(why) => refuse(&caller_name, &request.target, &tty, &shown, &why.replace(' ', "-"), &format!("cannot run as {}", request.target)),
    };
    let target = entry.account.clone();
    let program = match request.command.first() {
        Some(name) => match resolve(name) {
            Some(path) => path,
            None => refuse(&caller_name, &target.name, &tty, &shown, "not-found", &format!("{name}: command not found")),
        },
        None => target.shell.clone(),
    };
    let cmd = std::iter::once(program.clone()).chain(request.command.iter().skip(1).cloned()).collect::<Vec<_>>().join(" ");
    if caller_uid != 0 {
        let db = accounts::Database::read(root).unwrap_or_else(|e| {
            refuse(&caller_name, &target.name, &tty, &cmd, "accounts", &e)
        });
        let caller_account = db.account(&caller_name).filter(|a| a.uid == caller_uid).unwrap_or_else(|| {
            refuse(&caller_name, &target.name, &tty, &cmd, "caller", "the caller has no account")
        });
        let groups = policy::group_names(&db.group, &caller_name, caller_account.gid);
        let rules = policy::read_policy(Path::new(POLICY));
        if let Err(why) = &rules {
            eprintln!("sudo: {why}");
        }
        let authentication = policy::authentication(rules.as_deref().ok(), &caller_name, &groups, &target.name, &program);
        let denied = authentication == policy::Authentication::Denied;
        let refuse_policy = || {
            refuse(&caller_name, &target.name, &tty, &cmd, "policy", &format!("{caller_name} may not run {program} as {}", target.name));
        };
        if denied && request.no_prompt {
            refuse_policy();
        }
        if authentication.requires_password() {
            let sid = libbreenix::process::getsid(0).map(|p| p.raw() as i32).unwrap_or(-1);
            if denied || tty == "-" || !cached(caller_uid, sid, &tty) {
                if request.no_prompt {
                    refuse(&caller_name, &target.name, &tty, &cmd, "password-required", "a password is required");
                }
                let own = session::validate(root, &caller_name).unwrap_or_else(|_| {
                    refuse(&caller_name, &target.name, &tty, &cmd, "caller", "your account cannot authenticate")
                });
                let terminal = libbreenix::fs::open("/dev/tty", libbreenix::fs::O_RDWR).unwrap_or_else(|e| {
                    refuse(&caller_name, &target.name, &tty, &cmd, "terminal", &format!("no terminal for the password: {e}"))
                });
                let password = session::read_password(terminal.raw() as i32, &format!("[sudo] password for {caller_name}: "));
                let _ = libbreenix::io::close(terminal);
                let secret = password.unwrap_or_else(|e| {
                    refuse(&caller_name, &target.name, &tty, &cmd, "password-input", &e)
                });
                let authenticated = own.password_matches(&secret.0);
                drop(secret);
                if !authenticated {
                    defs.fail_wait();
                    refuse(&caller_name, &target.name, &tty, &cmd, "password", "Authentication failure");
                }
                if denied {
                    refuse_policy();
                }
                if tty != "-" {
                    if let Err(why) = remember(caller_uid, sid, &tty) {
                        eprintln!("sudo: authentication not remembered: {why}");
                    }
                }
            }
        }
    }
    let record = session::Record { program: "sudo", user: &caller_name, target: &target.name, tty: &tty, refused: None, cmd: &cmd };
    accounts::log(root, &record)?;
    let mut environment = vec![
        format!("PATH={}", session::SECURE_PATH),
        format!("HOME={}", target.home),
        format!("USER={}", target.name),
        format!("LOGNAME={}", target.name),
        format!("SUDO_USER={caller_name}"),
        format!("SUDO_COMMAND={cmd}"),
    ];
    if let Ok(term) = std::env::var("TERM") {
        if !term.is_empty() && term.bytes().all(|b| b.is_ascii_graphic()) {
            environment.push(format!("TERM={term}"));
        }
    }
    session::take_identity(&target)?;
    session::set_umask(&session::LoginDefs { umask: 0o022, ..defs });
    if request.login {
        environment.push(format!("SHELL={}", target.shell));
        if request.command.is_empty() {
            return session::exec_shell(&target, &environment, true, None);
        }
        // The permitted program runs with its own arguments: a shell would
        // read them as a command line and run whatever else they name.
        session::enter_home(&target)?;
    }
    let argv: Vec<CString> = std::iter::once(program.as_str())
        .chain(request.command.iter().skip(1).map(String::as_str))
        .map(|a| CString::new(a).map_err(|_| "argument contains NUL".to_string()))
        .collect::<Result<_, _>>()?;
    session::exec(&program, &argv, &environment)
}

fn main() {
    match run() {
        Ok(never) => match never {},
        Err(e) => {
            eprintln!("sudo: {e}");
            std::process::exit(1);
        }
    }
}
