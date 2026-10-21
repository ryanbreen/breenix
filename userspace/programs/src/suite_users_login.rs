// Login, SSH, su and sudo cases (`docs/users/design.md`, sections 3 and 6). Each case
// makes its accounts through the accounts library (useradd -m, then a random password
// set as root), runs the installed programs as they run for a person: getty and login
// on a fresh pseudo-terminal line, su, sudo and passwd typed into a terminal as the
// caller's account, bsshd and the bssh client over loopback. The accounts' login shell
// is the probe (`users_login_probe.rs`), which prints one `PROBE` line describing the
// session it was given. A refusal case also shows the same step succeeding once the one
// condition it tests is lifted, so the refusal has that cause.
use libbreenix::accounts::session as ses;

const PROBE: &str = "/usr/libexec/breenix/users-login-probe_test";
const AUTH_LOG: &str = "/var/log/auth.log";
const TEST_KEY: &str = "/etc/breenix/test-ssh/id_ed25519";
const TEST_KEY_PUBLIC: &str = "/etc/breenix/test-ssh/home/.ssh/authorized_keys";
const SSH_PORT: u16 = 2299;
const O_NOCTTY: u32 = 0o400;
const TTY_GID: u32 = 5;

/// /etc files a case changes, put back byte for byte (and mode) when it ends.
struct EtcGuard(Vec<(&'static str, Vec<u8>, u32)>);

impl EtcGuard {
    fn save(paths: &[&'static str]) -> Result<EtcGuard, String> {
        let mut saved = Vec::new();
        for path in paths {
            let bytes = fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            saved.push((*path, bytes, mode_of(path)? & 0o7777));
        }
        Ok(EtcGuard(saved))
    }
}

impl Drop for EtcGuard {
    fn drop(&mut self) {
        for (path, bytes, mode) in &self.0 {
            let _ = fs::write(path, bytes);
            let _ = chmod(path, *mode);
        }
    }
}

/// Append `line` to the /etc file `path` (the caller's guard restores it).
fn append_etc(path: &str, line: &str) -> Checked {
    let mut text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    if !text.ends_with('\n') && !text.is_empty() { text.push('\n'); }
    text.push_str(line);
    text.push('\n');
    fs::write(path, text).map_err(|e| format!("{path}: {e}"))
}

/// The probe listed as a login shell, for the case's lifetime.
fn probe_shells() -> Result<EtcGuard, String> {
    let guard = EtcGuard::save(&["/etc/shells"])?;
    append_etc("/etc/shells", PROBE)?;
    Ok(guard)
}

fn random_word(n: usize) -> Result<String, String> {
    let mut bytes = vec![0u8; n];
    let mut at = 0;
    while at < n {
        at += libbreenix::crypto::rand::getrandom_bytes(&mut bytes[at..]).map_err(|e| format!("getrandom: {e}"))?;
    }
    Ok(bytes.iter().map(|b| b"abcdefghijklmnopqrstuvwxyz0123456789"[(*b % 36) as usize] as char).collect())
}

/// A per-case account with a home, a password and the probe (or `shell`) as login shell.
struct User { name: String, password: String, uid: u32, gid: u32, groups: Vec<u32>, home: String }

impl User {
    fn add(tag: &str, groups: &[&str], shell: &str) -> Result<User, String> {
        let root = Path::new("/");
        let name = format!("{tag}{}", pid());
        let options = ac::UserAdd {
            groups: groups.iter().map(|g| g.to_string()).collect(),
            shell: Some(shell.into()),
            create_home: true,
            ..Default::default()
        };
        let account = ac::useradd(root, &name, &options)?;
        let password = random_word(14)?;
        ac::change_password(root, &name, b"", password.as_bytes())?;
        Ok(User { name, password, uid: account.uid, gid: account.gid, groups: account.groups, home: account.home })
    }

    fn probe(tag: &str, groups: &[&str]) -> Result<User, String> { User::add(tag, groups, PROBE) }

    /// Become this account for good, as become_account does for the ID-only fixtures.
    fn become_it(&self) -> Checked {
        ok("setgroups", setgroups(&self.groups))?;
        ok("setresgid", setresgid(self.gid, self.gid, self.gid))?;
        ok("setresuid", setresuid(self.uid, self.uid, self.uid))?;
        refused("setuid(0) after becoming the account", setuid(0), EPERM)
    }

    fn groups_text(&self) -> String {
        let mut groups = self.groups.clone();
        groups.sort_unstable();
        groups.dedup();
        groups.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
    }
}

impl Drop for User {
    fn drop(&mut self) {
        let _ = ac::userdel(Path::new("/"), &self.name, true);
        let _ = ac::groupdel(Path::new("/"), &self.name);
    }
}

/// A pseudo-terminal line: the master, the slave's name and a slave descriptor the
/// suite keeps open so the line outlives each session on it.
struct Pty { master: Fd, keeper: Fd, path: String }

impl Pty {
    fn open() -> Result<Pty, String> {
        let (master, path) = libbreenix::pty::openpty().map_err(|e| format!("openpty: {e}"))?;
        let path = String::from_utf8_lossy(libbreenix::pty::slave_path_bytes(&path)).into_owned();
        let keeper = libbreenix::fs::open(&path, libbreenix::fs::O_RDWR | O_NOCTTY).map_err(|e| format!("open {path}: {e}"))?;
        Ok(Pty { master, keeper, path })
    }

    /// The line's name as getty takes it: `pts/N`.
    fn line(&self) -> &str { self.path.trim_start_matches("/dev/") }

    fn owner(&self) -> Result<(u32, u32, u32), String> {
        let st = stat(&self.path)?;
        Ok((st.st_uid, st.st_gid, st.st_mode & 0o7777))
    }
}

impl Drop for Pty {
    fn drop(&mut self) { let _ = io::close(self.keeper); let _ = io::close(self.master); }
}

/// What a program run on the line printed, and how it ended.
struct Run { output: String, status: i32 }

impl Run {
    /// The probe's report as `key -> value`.
    fn probe(&self) -> Result<HashMap<String, String>, String> {
        let line = self.output.lines().map(|l| l.trim_matches('\r')).find(|l| l.starts_with("PROBE "))
            .ok_or_else(|| format!("no probe report ({}); output: {}", status_text(self.status), self.tail()))?;
        Ok(line.split_whitespace().skip(1).filter_map(|w| w.split_once('=')).map(|(k, v)| (k.into(), v.into())).collect())
    }

    fn tail(&self) -> String {
        let flat: String = self.output.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
        flat.chars().rev().take(120).collect::<Vec<_>>().into_iter().rev().collect()
    }

    fn exited(&self, code: i32) -> Checked {
        want(self.status & 0x7f == 0 && (self.status >> 8) & 0xff == code,
            format!("ended with {}, expected exit {code}; output: {}", status_text(self.status), self.tail()))
    }

    fn says(&self, text: &str) -> Checked {
        want(self.output.contains(text), format!("expected '{text}'; output: {}", self.tail()))
    }
}

/// Require each `key=value` of `expected` in a probe report.
fn report_has(report: &HashMap<String, String>, expected: &[(&str, String)]) -> Checked {
    for (key, value) in expected {
        let got = report.get(*key).map(String::as_str).unwrap_or("(none)");
        want(got == value, format!("{key} is {got}, expected {value}"))?;
    }
    Ok(())
}

/// How a run's process starts.
enum Start<'a> {
    /// getty serves the line itself (it makes the session and takes the terminal).
    Getty,
    /// A new session on the line as its controlling terminal, as `who` (root without
    /// one), in `cwd`, with exactly `env`.
    Session { who: Option<&'a User>, cwd: &'a str, env: &'a [String] },
}

/// Run `argv` on `pty`, answering each `(prompt, reply)` of `script` in order once the
/// output since the last answer shows the prompt, until the process ends.
fn run_on(pty: &Pty, start: Start, argv: &[&str], script: &[(&str, &str)]) -> Result<Run, String> {
    let child = match process::fork().map_err(|e| format!("fork: {e}"))? {
        ForkResult::Child => {
            let result = (|| -> Checked {
                if let Start::Session { who, cwd, .. } = &start {
                    process::setsid().map_err(|e| format!("setsid: {e}"))?;
                    let fd = libbreenix::fs::open(&pty.path, libbreenix::fs::O_RDWR).map_err(|e| format!("open line: {e}"))?;
                    libbreenix::termios::set_controlling_terminal(fd).map_err(|e| format!("controlling terminal: {e}"))?;
                    for n in 0..3 { io::dup2(fd, Fd::from_raw(n)).map_err(|e| format!("dup2: {e}"))?; }
                    if fd.raw() > 2 { let _ = io::close(fd); }
                    if let Some(who) = who { who.become_it()?; }
                    ok("chdir", chdir(cwd))?;
                }
                let _ = io::close(pty.master);
                let _ = io::close(pty.keeper);
                let env: Vec<String> = match &start { Start::Session { env, .. } => env.to_vec(), Start::Getty => Vec::new() };
                let argv: Vec<CString> = argv.iter().map(|a| cpath(a)).collect();
                ses::exec(argv[0].to_str().unwrap_or(""), &argv, &env).map(|_| ())
            })();
            if let Err(msg) = result { let _ = io::write(Fd::from_raw(2), msg.as_bytes()); }
            process::exit(127)
        }
        ForkResult::Parent(child) => child.raw() as i32,
    };
    let mut output = Vec::new();
    let mut answered = 0;
    let mut step = 0;
    let mut status = None;
    let mut quiet_since = None;
    loop {
        if libbreenix::suite::case_ms_left() < 400 {
            kill(child, SIGKILL);
            let _ = wait(child);
            return Err(format!("{} did not finish; output: {}", argv[0],
                Run { output: String::from_utf8_lossy(&output).into_owned(), status: 0 }.tail()));
        }
        let mut fds = [io::PollFd::new(pty.master, io::poll_events::POLLIN)];
        let ready = io::poll(&mut fds, 20).unwrap_or(0);
        if ready > 0 && fds[0].revents & io::poll_events::POLLIN != 0 {
            let mut buf = [0u8; 512];
            match io::read(pty.master, &mut buf) {
                Ok(n) if n > 0 => { output.extend_from_slice(&buf[..n]); quiet_since = None; }
                _ => {}
            }
        }
        while let Some((prompt, reply)) = script.get(step) {
            let seen = String::from_utf8_lossy(&output[answered..]).into_owned();
            let Some(at) = seen.find(prompt) else { break };
            answered += at + prompt.len();
            io::write(pty.master, format!("{reply}\n").as_bytes()).map_err(|e| format!("type: {e}"))?;
            step += 1;
        }
        if status.is_none() {
            let mut s = 0;
            if sc(nr::WAIT4, &[child as i64 as u64, &mut s as *mut i32 as u64, WNOHANG as u64, 0]) == child as i64 {
                status = Some(s);
            }
        }
        if let Some(status) = status {
            // The process ended: keep reading until the line has been quiet for 100 ms.
            let since = *quiet_since.get_or_insert(libbreenix::suite::case_ms_left());
            if since.saturating_sub(libbreenix::suite::case_ms_left()) >= 100 {
                return Ok(Run { output: String::from_utf8_lossy(&output).into_owned(), status });
            }
        }
    }
}

/// A login on a fresh line through getty: name, then password.
fn getty_login(pty: &Pty, name: &str, password: &str) -> Result<Run, String> {
    run_on(pty, Start::Getty, &["/bin/getty", pty.line()], &[("login: ", name), ("Password: ", password)])
}

/// A caller's environment for su and sudo runs.
fn caller_env(user: &User, extra: &[&str]) -> Vec<String> {
    let mut env = vec![
        format!("HOME={}", user.home), format!("USER={}", user.name), format!("LOGNAME={}", user.name),
        "PATH=/usr/bin:/bin".to_string(), "TERM=vt100".to_string(),
    ];
    env.extend(extra.iter().map(|e| e.to_string()));
    env
}

/// Run `argv` on a fresh line as `user`, answering `script`.
fn as_caller(user: &User, argv: &[&str], extra_env: &[&str], cwd: &str, script: &[(&str, &str)]) -> Result<Run, String> {
    let pty = Pty::open()?;
    let env = caller_env(user, extra_env);
    run_on(&pty, Start::Session { who: Some(user), cwd, env: &env }, argv, script)
}

/// The authentication log's records from `program` for `user` acting on `target`
/// with `result` (`ok` or `refused:<reason>`).
fn logged(program: &str, user: &str, target: &str, result: &str) -> Checked {
    let log = fs::read_to_string(AUTH_LOG).map_err(|e| format!("{AUTH_LOG}: {e}"))?;
    let found = log.lines().any(|l| {
        l.contains(&format!(" {program}[")) && l.contains(&format!(" user={user} "))
            && l.contains(&format!(" target={target} ")) && l.contains(&format!(" result={result} "))
    });
    want(found, format!("{AUTH_LOG} has no {program} record user={user} target={target} result={result}"))
}

/// Give root `password` (the caller's guard on /etc/shadow restores root's line).
fn root_password(password: &str) -> Checked {
    ac::change_password(Path::new("/"), "root", b"", password.as_bytes())
}

fn identity(user: &User) -> Vec<(&'static str, String)> {
    let ids = |n: u32| format!("{n},{n},{n}");
    vec![("uid", ids(user.uid)), ("gid", ids(user.gid)), ("groups", user.groups_text())]
}

// ---------------------------------------------------------------------------
// login

fn login_user() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("lgu", &["ug", "users"])?;
    let pty = Pty::open()?;
    let run = getty_login(&pty, &user.name, &user.password)?;
    let report = run.probe()?;
    let mut expected = identity(&user);
    expected.extend([
        ("HOME", user.home.clone()), ("USER", user.name.clone()), ("LOGNAME", user.name.clone()),
        ("SHELL", PROBE.to_string()), ("cwd", user.home.clone()),
        ("argv0", "-users-login-probe_test".to_string()), ("TERM", "breenix".to_string()),
        ("PATH", "/usr/bin:/bin:/usr/local/bin".to_string()),
    ]);
    report_has(&report, &expected)?;
    want(user.groups.contains(&2100) && user.groups.contains(&100), "the account lacks its supplementary groups")?;
    Ok(logged("login", &user.name, &user.name, "ok")?)
}

fn login_bad_password() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("lgb", &[])?;
    let pty = Pty::open()?;
    let run = getty_login(&pty, &user.name, "not-the-password")?;
    run.says("Login incorrect")?;
    want(run.probe().is_err(), "a wrong password started a session")?;
    logged("login", &user.name, &user.name, "refused:password")?;
    Ok(report_has(&getty_login(&pty, &user.name, &user.password)?.probe()?, &identity(&user))?)
}

fn login_root_refused() -> CaseResult {
    let _shells = probe_shells()?;
    let _etc = EtcGuard::save(&["/etc/shadow", "/etc/passwd", "/etc/login.defs"])?;
    let password = random_word(14)?;
    root_password(&password)?;
    ac::usermod(Path::new("/"), "root", &ac::UserMod { shell: Some(PROBE.into()), ..Default::default() })?;
    // A pseudo-terminal line is a remote login's: root is refused there under the
    // shipped default, and logs in only on the local console and virtual terminals
    // (ruling S14; the console suite's getty/root-login-on-vt).
    let pty = Pty::open()?;
    let run = getty_login(&pty, "root", &password)?;
    run.says("Login incorrect")?;
    want(run.probe().is_err(), "root logged in on a pseudo-terminal under ROOT_LOGIN no")?;
    logged("login", "root", "root", "refused:root-login")?;
    // The same password and shell log in on that line once login.defs permits root.
    let defs = fs::read_to_string("/etc/login.defs").map_err(|e| e.to_string())?;
    want(defs.lines().any(|l| l.split_whitespace().collect::<Vec<_>>() == ["ROOT_LOGIN", "no"]), "login.defs does not say ROOT_LOGIN no")?;
    fs::write("/etc/login.defs", defs.replace("ROOT_LOGIN no", "ROOT_LOGIN yes")).map_err(|e| e.to_string())?;
    Ok(report_has(&getty_login(&pty, "root", &password)?.probe()?, &[("uid", "0,0,0".to_string())])?)
}

fn login_locked_refused() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("lgl", &[])?;
    let root = Path::new("/");
    ac::usermod(root, &user.name, &ac::UserMod { lock: true, ..Default::default() })?;
    let pty = Pty::open()?;
    let run = getty_login(&pty, &user.name, &user.password)?;
    run.says("Login incorrect")?;
    want(run.probe().is_err(), "a locked account logged in")?;
    logged("login", &user.name, &user.name, "refused:password")?;
    ac::usermod(root, &user.name, &ac::UserMod { unlock: true, ..Default::default() })?;
    Ok(report_has(&getty_login(&pty, &user.name, &user.password)?.probe()?, &identity(&user))?)
}

fn login_expired_refused() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("lge", &[])?;
    let root = Path::new("/");
    ac::usermod(root, &user.name, &ac::UserMod { expire: Some(Some(1)), ..Default::default() })?;
    let pty = Pty::open()?;
    let run = getty_login(&pty, &user.name, &user.password)?;
    run.says("Login incorrect")?;
    want(run.probe().is_err(), "an expired account logged in")?;
    logged("login", &user.name, &user.name, "refused:account-expired")?;
    ac::usermod(root, &user.name, &ac::UserMod { expire: Some(None), ..Default::default() })?;
    Ok(report_has(&getty_login(&pty, &user.name, &user.password)?.probe()?, &identity(&user))?)
}

fn login_tty_owned() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("lgt", &[])?;
    let pty = Pty::open()?;
    let (uid, gid, mode) = pty.owner()?;
    want(uid == 0 && mode == 0o620, format!("the fresh line is {uid}:{gid} {mode:o}, expected root's 0620"))?;
    let report = getty_login(&pty, &user.name, &user.password)?.probe()?;
    report_has(&report, &[("tty", format!("{}:{TTY_GID}:620", user.uid))])?;
    // The session is over, but the line stays the account's until getty serves it again.
    Ok(want(pty.owner()? == (user.uid, TTY_GID, 0o620), "the line was not the account's during its session")?)
}

fn login_umask() -> CaseResult {
    let _shells = probe_shells()?;
    let _etc = EtcGuard::save(&["/etc/login.defs"])?;
    let user = User::probe("lgm", &[])?;
    let pty = Pty::open()?;
    report_has(&getty_login(&pty, &user.name, &user.password)?.probe()?, &[("umask", "0022".to_string())])?;
    let defs = fs::read_to_string("/etc/login.defs").map_err(|e| e.to_string())?;
    let changed: String = defs.lines().map(|l| if l.starts_with("UMASK") { "UMASK 027".to_string() } else { l.to_string() })
        .collect::<Vec<_>>().join("\n") + "\n";
    fs::write("/etc/login.defs", changed).map_err(|e| e.to_string())?;
    Ok(report_has(&getty_login(&pty, &user.name, &user.password)?.probe()?, &[("umask", "0027".to_string())])?)
}

fn logout_tty_restored() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("lgo", &[])?;
    let pty = Pty::open()?;
    getty_login(&pty, &user.name, &user.password)?.probe()?;
    want(pty.owner()?.0 == user.uid, "the session did not own its line")?;
    // getty serving the line again returns it to root and records the logout; an empty
    // name then ends it.
    let run = run_on(&pty, Start::Getty, &["/bin/getty", pty.line()], &[("login: ", "")])?;
    run.exited(0)?;
    let (uid, gid, mode) = pty.owner()?;
    want((uid, gid, mode) == (0, TTY_GID, 0o620), format!("after logout the line is {uid}:{gid} {mode:o}, expected 0:5 620"))?;
    Ok(logged("getty", &user.name, &user.name, "ok")?)
}

/// A process the first session leaves behind keeps the line open for reading and
/// writing; getty serving the line for the next login revokes it from that
/// descriptor, so the next login's input does not reach the old session.
fn logout_revokes_terminal() -> CaseResult {
    let _shells = probe_shells()?;
    let first = User::probe("lgv", &[])?;
    let second = User::probe("lgw", &[])?;
    let pty = Pty::open()?;
    getty_login(&pty, &first.name, &first.password)?.probe()?;
    want(pty.owner()?.0 == first.uid, "the first session did not own its line")?;
    let (r, w) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    let holder = match process::fork().map_err(|e| format!("fork: {e}"))? {
        ForkResult::Child => {
            let _ = io::close(r);
            let report = (|| -> Result<String, String> {
                first.become_it()?;
                let fd = libbreenix::fs::open(&pty.path, libbreenix::fs::O_RDWR | O_NOCTTY)
                    .map_err(|e| format!("open line as {}: {e}", first.name))?;
                io::write(w, b"ready\n").map_err(|e| e.to_string())?;
                let mut buf = [0u8; 256];
                let read = match io::read(fd, &mut buf) {
                    Ok(n) => format!("{n}:{}", String::from_utf8_lossy(&buf[..n]).trim()),
                    Err(e) => format!("error:{e}"),
                };
                let wrote = write_fd(fd.raw() as i64, b"spoofed login: ");
                let reopened = open(&pty.path, 2 | u64::from(O_NOCTTY), 0);
                Ok(format!("read={read} write={} reopen={}", shown(wrote), shown(reopened.min(0))))
            })();
            let line = report.unwrap_or_else(|why| format!("failed {why}"));
            let _ = io::write(w, format!("{line}\n").as_bytes());
            process::exit(0)
        }
        ForkResult::Parent(child) => child.raw() as i32,
    };
    let _ = io::close(w);
    let lines = Lines::new(r);
    let outcome = (|| -> Checked {
        want(lines.next(2000)?.as_deref() == Some("ready"), "the left-behind process did not open the line")?;
        // getty serves the line again; the next account logs in on it.
        let run = getty_login(&pty, &second.name, &second.password)?;
        report_has(&run.probe()?, &identity(&second))?;
        let report = lines.next(2000)?.ok_or("the left-behind process did not report")?;
        want(report == "read=0: write=EIO reopen=EACCES",
            format!("the old session's descriptor still reached the line: {report}"))
    })();
    if outcome.is_err() { kill(holder, SIGKILL); }
    let _ = wait(holder);
    let _ = io::close(r);
    Ok(outcome?)
}

/// Lines from a pipe, each waited for up to a deadline.
struct Lines { fd: Fd, pending: std::cell::RefCell<Vec<u8>> }

impl Lines {
    fn new(fd: Fd) -> Lines { Lines { fd, pending: std::cell::RefCell::new(Vec::new()) } }

    /// The next line, or no line once the writer closed; an error after `ms`.
    fn next(&self, ms: u64) -> Result<Option<String>, String> {
        let until = libbreenix::suite::case_ms_left().saturating_sub(ms);
        loop {
            let mut pending = self.pending.borrow_mut();
            if let Some(at) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=at).collect();
                return Ok(Some(String::from_utf8_lossy(&line[..at]).into_owned()));
            }
            let left = libbreenix::suite::case_ms_left();
            if left <= until || left < 300 {
                return Err("timed out waiting for a line from the pipe".into());
            }
            let mut fds = [io::PollFd::new(self.fd, io::poll_events::POLLIN)];
            if io::poll(&mut fds, 20).unwrap_or(0) > 0 {
                let mut buf = [0u8; 256];
                match io::read(self.fd, &mut buf) {
                    Ok(0) => return Ok(None),
                    Ok(n) => pending.extend_from_slice(&buf[..n]),
                    Err(e) => return Err(format!("read pipe: {e}")),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ssh

/// A bsshd on the loopback port, killed when the case ends.
struct Sshd { pid: i32 }

impl Sshd {
    fn start() -> Result<Sshd, String> {
        let pid = match process::fork().map_err(|e| format!("fork: {e}"))? {
            ForkResult::Child => {
                let argv = [cpath("/usr/sbin/bsshd"), cpath(&SSH_PORT.to_string())];
                let _ = ses::exec("/usr/sbin/bsshd", &argv, &[]);
                process::exit(127)
            }
            ForkResult::Parent(child) => child.raw() as i32,
        };
        let sshd = Sshd { pid };
        while libbreenix::suite::case_ms_left() > 6000 {
            let addr = libbreenix::socket::SockAddrIn::new([127, 0, 0, 1], SSH_PORT);
            if libbreenix::socket::TcpStream::connect(&addr).is_ok() { return Ok(sshd); }
            let _ = libbreenix::time::sleep_ms(50);
        }
        Err("bsshd did not listen on the loopback port".into())
    }
}

impl Drop for Sshd {
    fn drop(&mut self) { kill(self.pid, SIGKILL); let _ = wait(self.pid); }
}

/// The test key authorized for `user`: `~/.ssh` 0700 and `authorized_keys` at `mode`.
fn authorize(user: &User, mode: u32) -> Checked {
    let ssh = format!("{}/.ssh", user.home);
    ok("mkdir .ssh", mkdir(&ssh, 0o700))?;
    let keys = format!("{ssh}/authorized_keys");
    fs::write(&keys, fs::read(TEST_KEY_PUBLIC).map_err(|e| format!("{TEST_KEY_PUBLIC}: {e}"))?).map_err(|e| e.to_string())?;
    for path in [&ssh, &keys] { ok("chown", chown(path, user.uid, user.gid))?; }
    ok("chmod", chmod(&ssh, 0o700))?;
    ok("chmod", chmod(&keys, mode)).map(|_| ())
}

/// bssh to the loopback bsshd as `name`, running `command` (an interactive shell with a
/// PTY without one). Its own stdout and stderr are the run's output.
fn bssh(name: &str, command: Option<&str>) -> Result<Run, String> {
    let known = format!("/tmp/users-{}-known_hosts", pid());
    let port = SSH_PORT.to_string();
    let mut argv = vec!["/usr/bin/bssh", "127.0.0.1", port.as_str(), name, "--identity", TEST_KEY, "--known-hosts", known.as_str()];
    if let Some(command) = command { argv.extend(["--exec", command]); }
    let (r, w) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    let (hold_r, hold_w) = io::pipe().map_err(|e| format!("pipe: {e}"))?;
    let child = match process::fork().map_err(|e| format!("fork: {e}"))? {
        ForkResult::Child => {
            let _ = io::close(r);
            let _ = io::close(hold_w);
            let _ = io::dup2(hold_r, Fd::from_raw(0));
            let _ = io::dup2(w, Fd::from_raw(1));
            let _ = io::dup2(w, Fd::from_raw(2));
            let argv: Vec<CString> = argv.iter().map(|a| cpath(a)).collect();
            let _ = ses::exec(argv[0].to_str().unwrap_or(""), &argv, &[]);
            process::exit(127)
        }
        ForkResult::Parent(child) => child.raw() as i32,
    };
    let _ = io::close(w);
    let _ = io::close(hold_r);
    let output = read_all(r);
    let _ = io::close(r);
    let _ = io::close(hold_w);
    let status = wait(child)?;
    Ok(Run { output: String::from_utf8_lossy(&output).into_owned(), status })
}

fn ssh_identity() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("ssi", &[])?;
    authorize(&user, 0o600)?;
    let _sshd = Sshd::start()?;
    let run = bssh(&user.name, Some("id"))?;
    let mut expected = identity(&user);
    expected.extend([("HOME", user.home.clone()), ("USER", user.name.clone()), ("cwd", user.home.clone()),
        ("args", "-c_id".to_string())]);
    report_has(&run.probe()?, &expected)?;
    Ok(logged("bsshd", &user.name, &user.name, "ok")?)
}

fn ssh_root_refused() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("ssr", &[])?;
    authorize(&user, 0o600)?;
    // The image lists the same test key for root.
    let root_keys = fs::read_to_string("/root/.ssh/authorized_keys").map_err(|e| format!("root's keys: {e}"))?;
    let key = fs::read_to_string(TEST_KEY_PUBLIC).map_err(|e| e.to_string())?;
    want(root_keys.contains(key.trim()), "root's authorized keys do not list the test key")?;
    let _sshd = Sshd::start()?;
    let run = bssh("root", Some("id"))?;
    run.exited(77)?;
    want(run.probe().is_err(), "root ran a command over SSH")?;
    // The same key logs in as an ordinary account.
    Ok(report_has(&bssh(&user.name, Some("id"))?.probe()?, &identity(&user))?)
}

fn ssh_groups() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("ssg", &["ug", "wheel", "users"])?;
    authorize(&user, 0o600)?;
    let _sshd = Sshd::start()?;
    want(user.groups.len() == 4, format!("the account has groups {:?}", user.groups))?;
    Ok(report_has(&bssh(&user.name, Some("id"))?.probe()?, &identity(&user))?)
}

fn ssh_pty_owner() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("ssp", &[])?;
    authorize(&user, 0o600)?;
    let _sshd = Sshd::start()?;
    let report = bssh(&user.name, None)?.probe()?;
    Ok(report_has(&report, &[("tty", format!("{}:{TTY_GID}:620", user.uid)), ("argv0", "-users-login-probe_test".into()),
        ("uid", format!("{0},{0},{0}", user.uid))])?)
}

fn ssh_bad_key_mode_refused() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("ssk", &[])?;
    authorize(&user, 0o620)?;
    let _sshd = Sshd::start()?;
    let run = bssh(&user.name, Some("id"))?;
    run.exited(77)?;
    want(run.probe().is_err(), "a group-writable authorized_keys granted a login")?;
    ok("chmod", chmod(&format!("{}/.ssh/authorized_keys", user.home), 0o600))?;
    Ok(report_has(&bssh(&user.name, Some("id"))?.probe()?, &identity(&user))?)
}

// ---------------------------------------------------------------------------
// su and passwd

fn su_wrong_password() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("suc", &[])?;
    let target = User::probe("sut", &[])?;
    let run = as_caller(&caller, &["/bin/su", &target.name], &[], "/", &[("Password: ", "not-the-password")])?;
    run.exited(1)?;
    run.says("Authentication failure")?;
    want(run.probe().is_err(), "a wrong password ran the target's shell")?;
    logged("su", &caller.name, &target.name, "refused:password")?;
    let run = as_caller(&caller, &["/bin/su", &target.name], &[], "/", &[("Password: ", &target.password)])?;
    Ok(report_has(&run.probe()?, &identity(&target))?)
}

fn su_root_ok() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sur", &["wheel"])?;
    let _etc = EtcGuard::save(&["/etc/shadow"])?;
    let password = random_word(14)?;
    root_password(&password)?;
    let run = as_caller(&caller, &["/bin/su", "-c", PROBE], &[], "/", &[("Password: ", &password)])?;
    report_has(&run.probe()?, &[("uid", "0,0,0".into()), ("gid", "0,0,0".into()), ("USER", "root".into())])?;
    run.exited(0)?;
    Ok(logged("su", &caller.name, "root", "ok")?)
}

fn su_nonwheel_refused() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sun", &[])?;
    let target = User::probe("suo", &[])?;
    let _etc = EtcGuard::save(&["/etc/shadow"])?;
    let password = random_word(14)?;
    root_password(&password)?;
    let run = as_caller(&caller, &["/bin/su", "-c", PROBE], &[], "/", &[("Password: ", &password)])?;
    run.exited(1)?;
    run.says("only members of group wheel may become root")?;
    want(!run.output.contains("Password: "), "su asked a non-wheel caller for root's password")?;
    want(run.probe().is_err(), "a non-wheel caller became root")?;
    logged("su", &caller.name, "root", "refused:not-wheel")?;
    // The same caller may become another ordinary account with its password.
    let run = as_caller(&caller, &["/bin/su", &target.name], &[], "/", &[("Password: ", &target.password)])?;
    Ok(report_has(&run.probe()?, &identity(&target))?)
}

fn su_dash_env() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sud", &[])?;
    let target = User::probe("sue", &[])?;
    let keep = ["USERS_SUITE_KEEP=kept"];
    let script = [("Password: ", target.password.as_str())];
    let login = as_caller(&caller, &["/bin/su", "-", &target.name], &keep, "/tmp", &script)?.probe()?;
    report_has(&login, &[("HOME", target.home.clone()), ("USER", target.name.clone()), ("LOGNAME", target.name.clone()),
        ("cwd", target.home.clone()), ("argv0", "-users-login-probe_test".into()), ("USERS_SUITE_KEEP", "-".into()),
        ("TERM", "vt100".into()), ("umask", "0022".into())])?;
    let plain = as_caller(&caller, &["/bin/su", &target.name], &keep, "/tmp", &script)?.probe()?;
    Ok(report_has(&plain, &[("HOME", target.home.clone()), ("USER", target.name.clone()), ("cwd", "/tmp".into()),
        ("argv0", "users-login-probe_test".into()), ("USERS_SUITE_KEEP", "kept".into())])?)
}

fn su_logged() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sul", &[])?;
    let target = User::probe("sum", &[])?;
    as_caller(&caller, &["/bin/su", &target.name], &[], "/", &[("Password: ", &target.password)])?.probe()?;
    as_caller(&caller, &["/bin/su", &target.name], &[], "/", &[("Password: ", "not-the-password")])?.exited(1)?;
    logged("su", &caller.name, &target.name, "ok")?;
    logged("su", &caller.name, &target.name, "refused:password")?;
    let log = fs::read_to_string(AUTH_LOG).map_err(|e| e.to_string())?;
    want(!log.contains(&target.password), "the auth log holds a password")?;
    Ok(attrs(Path::new(AUTH_LOG), 0o640, 0, 4, false)?)
}

fn passwd_command() -> CaseResult {
    let _shells = probe_shells()?;
    let user = User::probe("pwc", &[])?;
    let caller = User::probe("pwd", &[])?;
    let new = random_word(14)?;
    let run = as_caller(&user, &["/usr/bin/passwd"], &[], "/", &[("Current password: ", &user.password),
        ("New password: ", &new), ("Retype new password: ", &new)])?;
    run.exited(0)?;
    run.says("password updated")?;
    logged("passwd", &user.uid.to_string(), &user.name, "ok")?;
    // The new password now authenticates the account; the old one does not.
    let su = |password: &str| as_caller(&caller, &["/bin/su", &user.name], &[], "/", &[("Password: ", password)]);
    report_has(&su(&new)?.probe()?, &identity(&user))?;
    su(&user.password)?.exited(1)?;
    // Another account's password is not the caller's to change.
    let run = as_caller(&user, &["/usr/bin/passwd", &caller.name], &[], "/", &[("Current password: ", &new),
        ("New password: ", "another-password"), ("Retype new password: ", "another-password")])?;
    Ok(run.exited(1)?)
}

// ---------------------------------------------------------------------------
// sudo

fn sudo_prompt(user: &User) -> String { format!("[sudo] password for {}: ", user.name) }

fn sudo_wheel_ok() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sdw", &["wheel"])?;
    let prompt = sudo_prompt(&caller);
    let run = as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &[(&prompt, &caller.password)])?;
    report_has(&run.probe()?, &[("uid", "0,0,0".into()), ("gid", "0,0,0".into()), ("SUDO_USER", caller.name.clone())])?;
    Ok(run.exited(0)?)
}

fn sudo_nonwheel_refused() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sdn", &[])?;
    let prompt = sudo_prompt(&caller);
    let run = as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &[(&prompt, &caller.password)])?;
    run.exited(1)?;
    run.says("may not run")?;
    want(!run.output.contains(&prompt), "sudo asked a caller no rule permits for a password")?;
    want(run.probe().is_err(), "a caller outside wheel ran a command as root")?;
    Ok(logged("sudo", &caller.name, "root", "refused:policy")?)
}

fn sudo_wrong_password() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sdp", &["wheel"])?;
    let prompt = sudo_prompt(&caller);
    let run = as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &[(&prompt, "not-the-password")])?;
    run.exited(1)?;
    run.says("Authentication failure")?;
    want(run.probe().is_err(), "a wrong password ran a command as root")?;
    Ok(logged("sudo", &caller.name, "root", "refused:password")?)
}

fn sudo_env_reset() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sde", &["wheel"])?;
    let prompt = sudo_prompt(&caller);
    let script = format!("umask 077; PATH=/tmp/elsewhere:/usr/bin; exec /usr/bin/sudo {PROBE}");
    let run = as_caller(&caller, &["/bin/sh", "-c", &script], &["USERS_SUITE_KEEP=kept"], "/tmp", &[(&prompt, &caller.password)])?;
    Ok(report_has(&run.probe()?, &[
        ("PATH", ses::SECURE_PATH.into()), ("HOME", "/root".into()), ("USER", "root".into()), ("LOGNAME", "root".into()),
        ("TERM", "vt100".into()), ("SUDO_USER", caller.name.clone()), ("SUDO_COMMAND", PROBE.into()),
        ("USERS_SUITE_KEEP", "-".into()), ("umask", "0022".into()), ("cwd", "/tmp".into()), ("SHELL", "-".into()),
    ])?)
}

fn sudo_bad_policy_mode() -> CaseResult {
    let _shells = probe_shells()?;
    let _etc = EtcGuard::save(&["/etc/sudo.policy"])?;
    let caller = User::probe("sdb", &["wheel"])?;
    let prompt = sudo_prompt(&caller);
    let script = [(prompt.as_str(), caller.password.as_str())];
    ok("chmod", chmod("/etc/sudo.policy", 0o644))?;
    let run = as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &script)?;
    run.exited(1)?;
    run.says("not a valid policy")?;
    want(run.probe().is_err() && !run.output.contains(&prompt), "sudo ran under a policy any user may write")?;
    logged("sudo", &caller.name, "root", "refused:policy-mode")?;
    ok("chmod", chmod("/etc/sudo.policy", 0o440))?;
    append_etc("/etc/sudo.policy", "permit %wheel as")?;
    let run = as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &script)?;
    run.exited(1)?;
    want(run.probe().is_err(), "sudo ran under a policy that does not parse")?;
    logged("sudo", &caller.name, "root", "refused:policy-syntax")?;
    // The shipped policy, at its mode, permits the same call.
    drop(_etc);
    Ok(report_has(&as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &script)?.probe()?, &[("uid", "0,0,0".into())])?)
}

fn sudo_cache_session() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sdc", &["wheel"])?;
    let prompt = sudo_prompt(&caller);
    let both = format!("/usr/bin/sudo {PROBE} && /usr/bin/sudo -n {PROBE}");
    let run = as_caller(&caller, &["/bin/sh", "-c", &both], &[], "/", &[(&prompt, &caller.password)])?;
    run.exited(0)?;
    let reports = run.output.matches("PROBE uid=0,0,0").count();
    want(reports == 2, format!("{reports} root reports from two sudo runs in one session; output: {}", run.tail()))?;
    want(run.output.matches(&prompt).count() == 1, "the session's second sudo asked for the password again")?;
    // Another session on another terminal has no remembered authentication.
    let run = as_caller(&caller, &["/usr/bin/sudo", "-n", PROBE], &[], "/", &[])?;
    run.exited(1)?;
    run.says("a password is required")?;
    Ok(logged("sudo", &caller.name, "root", "refused:password-required")?)
}

fn sudo_policy_rules() -> CaseResult {
    let _shells = probe_shells()?;
    let _etc = EtcGuard::save(&["/etc/sudo.policy"])?;
    let caller = User::probe("sdr", &[])?;
    let target = User::probe("sds", &[])?;
    fs::write("/etc/sudo.policy", format!("permit {} as {} cmd {PROBE} nopass\npermit %wheel as root\n", caller.name, target.name))
        .map_err(|e| e.to_string())?;
    let run = as_caller(&caller, &["/usr/bin/sudo", "-n", "-u", &target.name, PROBE], &[], "/", &[])?;
    report_has(&run.probe()?, &identity(&target))?;
    // The rule names one command and one target.
    let run = as_caller(&caller, &["/usr/bin/sudo", "-n", "-u", &target.name, "/bin/sh", "-c", "true"], &[], "/", &[])?;
    run.exited(1)?;
    run.says("may not run /bin/sh")?;
    let run = as_caller(&caller, &["/usr/bin/sudo", "-n", PROBE], &[], "/", &[])?;
    run.exited(1)?;
    Ok(run.says("may not run")?)
}

/// Two rules both permit the caller to run the probe as root, one without a
/// password: the first decides, in each order.
fn sudo_first_match() -> CaseResult {
    let _shells = probe_shells()?;
    let _etc = EtcGuard::save(&["/etc/sudo.policy"])?;
    let caller = User::probe("sdf", &["wheel"])?;
    let prompt = sudo_prompt(&caller);
    let nopass = format!("permit {} as root cmd {PROBE} nopass", caller.name);
    let wheel = "permit %wheel as root";
    fs::write("/etc/sudo.policy", format!("{nopass}\n{wheel}\n")).map_err(|e| e.to_string())?;
    let run = as_caller(&caller, &["/usr/bin/sudo", "-n", PROBE], &[], "/", &[])?;
    report_has(&run.probe()?, &[("uid", "0,0,0".into())])?;
    fs::write("/etc/sudo.policy", format!("{wheel}\n{nopass}\n")).map_err(|e| e.to_string())?;
    let run = as_caller(&caller, &["/usr/bin/sudo", "-n", PROBE], &[], "/", &[])?;
    run.exited(1)?;
    run.says("a password is required")?;
    want(run.probe().is_err(), "a later nopass rule overrode the first matching rule")?;
    // The same order runs the probe once the caller's password is typed.
    let run = as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &[(&prompt, &caller.password)])?;
    Ok(report_has(&run.probe()?, &[("uid", "0,0,0".into())])?)
}

/// The permitted command gets its arguments as they were given, with -i too: words
/// a shell would read as another command or a substitution start no other program.
fn sudo_argv_no_shell() -> CaseResult {
    let _shells = probe_shells()?;
    let _etc = EtcGuard::save(&["/etc/sudo.policy"])?;
    let caller = User::probe("sda", &[])?;
    fs::write("/etc/sudo.policy", format!("permit {} as root cmd {PROBE} nopass\n", caller.name)).map_err(|e| e.to_string())?;
    let substitution = format!("$({PROBE})");
    let words = [";", PROBE, "injected", substitution.as_str(), "&&", "id"];
    let args: String = words.join(" ").chars().map(|c| if c == ' ' || c == '=' { '_' } else { c }).collect();
    for (login, cwd) in [(true, "/root"), (false, "/")] {
        let mut argv = vec!["/usr/bin/sudo", "-n"];
        if login { argv.push("-i"); }
        argv.push(PROBE);
        argv.extend(words);
        let run = as_caller(&caller, &argv, &[], "/", &[])?;
        run.exited(0)?;
        let reports = run.output.matches("PROBE uid=").count();
        want(reports == 1, format!("sudo{} ran {reports} probes from one command; output: {}", if login { " -i" } else { "" }, run.tail()))?;
        report_has(&run.probe()?, &[("uid", "0,0,0".into()), ("argv0", PROBE.into()), ("args", args.clone()), ("cwd", cwd.into())])?;
    }
    Ok(logged("sudo", &caller.name, "root", "ok")?)
}

fn sudo_logged() -> CaseResult {
    let _shells = probe_shells()?;
    let caller = User::probe("sdl", &["wheel"])?;
    let prompt = sudo_prompt(&caller);
    as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &[(&prompt, &caller.password)])?.probe()?;
    as_caller(&caller, &["/usr/bin/sudo", PROBE], &[], "/", &[(&prompt, "not-the-password")])?.exited(1)?;
    let log = fs::read_to_string(AUTH_LOG).map_err(|e| e.to_string())?;
    let mine: Vec<&str> = log.lines().filter(|l| l.contains(" sudo[") && l.contains(&format!(" user={} ", caller.name))).collect();
    want(mine.iter().any(|l| l.contains(" result=ok ") && l.ends_with(&format!(" cmd={PROBE}"))),
        format!("no successful sudo record naming the command: {mine:?}"))?;
    want(mine.iter().any(|l| l.contains(" result=refused:password ")), "no refused sudo record")?;
    want(!log.contains(&caller.password), "the auth log holds a password")?;
    Ok(attrs(Path::new(AUTH_LOG), 0o640, 0, 4, false)?)
}
