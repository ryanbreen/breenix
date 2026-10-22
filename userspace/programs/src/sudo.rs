//! sudo [-u user] [-i] [-n] [command [args...]]: run a command as another
//! account under `/etc/sudo.policy` (`docs/users/design.md`, section 3).
//! Installed set-user-ID root at `/usr/bin/sudo`. The name follows the
//! operator's habit; the policy grammar (`libbreenix::accounts::policy`) is
//! Breenix's own.
//!
//! - The policy must be a regular file, root's, mode 0440, one link, and must
//!   parse, or sudo refuses to run anything.
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

fn read_policy() -> Result<Vec<policy::Rule>, String> {
    let meta = fs::symlink_metadata(POLICY).map_err(|e| format!("{POLICY}: {e}"))?;
    if !meta.is_file() || meta.uid() != 0 || meta.gid() != 0 || meta.mode() & 0o7777 != 0o440 || meta.nlink() != 1 {
        return Err("policy-mode".into());
    }
    policy::parse_policy(&fs::read_to_string(POLICY).map_err(|e| format!("{POLICY}: {e}"))?)
        .map_err(|_| "policy-syntax".into())
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
    let request = parse(&args).map_err(|e| format!("{e}\nusage: sudo [-u user] [-i] [-n] [command [args...]]"))?;
    accounts::require_root().map_err(|_| "sudo must be installed set-user-ID root".to_string())?;
    let defs = session::LoginDefs::read(root);
    let caller_uid = session::real_uid();
    let caller = session::account_by_uid(root, caller_uid);
    let caller_name = caller.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| caller_uid.to_string());
    let tty = session::terminal_name(0).unwrap_or_else(|| "-".into());
    let shown = request.command.join(" ");
    let rules = match read_policy() {
        Ok(rules) => rules,
        Err(why) => refuse(&caller_name, &request.target, &tty, &shown, &why, &format!("{POLICY} is not a valid policy ({why}); refusing")),
    };
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
        let caller_account = caller.as_ref().ok_or("the caller has no account")?;
        let db = accounts::Database::read(root)?;
        let groups: Vec<String> = db.group.lines()
            .filter_map(|l| accounts::parse::Group::parse(l.as_bytes()))
            .filter(|g| caller_account.groups.contains(&g.gid))
            .filter_map(|g| std::str::from_utf8(g.fields[0]).ok().map(str::to_owned))
            .collect();
        let Some(rule) = policy::decide(&rules, &caller_name, &groups, &target.name, &program) else {
            refuse(&caller_name, &target.name, &tty, &cmd, "policy", &format!("{caller_name} may not run {program} as {}", target.name));
        };
        let sid = libbreenix::process::getsid(0).map(|p| p.raw() as i32).unwrap_or(-1);
        if !rule.nopass && !(tty != "-" && cached(caller_uid, sid, &tty)) {
            if request.no_prompt {
                refuse(&caller_name, &target.name, &tty, &cmd, "password-required", "a password is required");
            }
            let own = session::validate(root, &caller_name).map_err(|_| "your account cannot authenticate".to_string())?;
            let terminal = libbreenix::fs::open("/dev/tty", libbreenix::fs::O_RDWR)
                .map_err(|e| format!("no terminal for the password: {e}"))?;
            let secret = session::read_password(terminal.raw() as i32, &format!("[sudo] password for {caller_name}: "))?;
            let _ = libbreenix::io::close(terminal);
            if !own.password_matches(&secret.0) {
                drop(secret);
                defs.fail_wait();
                refuse(&caller_name, &target.name, &tty, &cmd, "password", "Authentication failure");
            }
            if tty != "-" {
                if let Err(why) = remember(caller_uid, sid, &tty) {
                    eprintln!("sudo: authentication not remembered: {why}");
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
