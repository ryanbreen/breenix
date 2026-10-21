//! The service table init supervises, the restart policy, and the state init
//! publishes under `/run` for `svc`.
//!
//! **The table**, `/etc/breenix/services`, plus any file in
//! `/run/breenix/services.d` (definitions that last until the next boot), is a
//! list of stanzas:
//!
//! ```text
//! # comment
//! [bsshd]
//! command = /usr/sbin/bsshd
//! user = root
//! restart = always
//! needs = entropy
//! stop-timeout = 5
//! ```
//! claim-lint:ok: `parse_into` and its host tests in libs/libbreenix/src/services.rs.
//!
//! - `command` (required): an absolute program path and its arguments, split
//!   into words as a shell would without expanding anything: `'...'` is
//!   literal, `"..."` honours `\"` and `\\`, and `\` outside quotes keeps the
//!   next character.
//! - `user`: the account the service runs as (default `root`). init takes on
//!   its supplementary groups, gid and uid before running the command.
//! - `restart`: `always` (default), `on-failure` (after a nonzero exit or a
//!   signal) or `never`. claim-lint:ok: `Draft::set` in libs/libbreenix/src/services.rs.
//! - `needs`: what must hold before the service starts, separated by spaces or
//!   commas: `entropy` (the kernel's random pool is ready), `network-up` (the
//!   kernel's interface configuration is complete) or another service's name
//!   (that service is running, or ran to completion).
//! - `stop-timeout`: seconds between SIGTERM and SIGKILL when the service is
//!   stopped (1 to 300, default 5).
//!
//! A malformed stanza disables only that service and is reported; so is a
//! service whose `needs` names no service in the table, or that depends on
//! itself through other services.
//!
//! **Restarts.** A run that fails (nonzero exit or a signal) within
//! [`STABLE_MS`] of starting is a quick failure. The first restart after an exit
//! waits [`BASE_DELAY_MS`]; each further consecutive quick failure doubles the
//! wait. At [`CRASH_LOOP_FAILURES`] consecutive quick failures the service is
//! reported as crash-looping and restarts every [`MAX_DELAY_MS`] until a run
//! lasts [`STABLE_MS`] or exits cleanly. claim-lint:ok: `after_exit` and its host tests in libs/libbreenix/src/services.rs.

use std::collections::BTreeSet;

/// The system's service table.
pub const TABLE: &str = "/etc/breenix/services";
/// Service definitions that last until the next boot (`/run` is emptied at boot).
pub const RUNTIME_TABLES: &str = "/run/breenix/services.d";
/// init's runtime directory.
pub const RUN_DIR: &str = "/run/breenix";
/// The state init publishes, one line per service, readable by any account.
pub const STATE: &str = "/run/breenix/services";
/// The control socket `svc` sends requests to.
pub const SOCKET: &str = "/run/breenix/svc.sock";

/// The first restart delay, and the delay after any run that did not fail quickly.
pub const BASE_DELAY_MS: u64 = 1_000;
/// The longest restart delay, used once a service is crash-looping.
pub const MAX_DELAY_MS: u64 = 60_000;
/// A failure this soon after starting counts toward the backoff and the crash loop.
pub const STABLE_MS: u64 = 10_000;
/// Consecutive quick failures that make a crash loop.
pub const CRASH_LOOP_FAILURES: u32 = 5;
/// The default time between SIGTERM and SIGKILL.
pub const DEFAULT_STOP_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    Always,
    OnFailure,
    Never,
}

impl Restart {
    pub fn name(self) -> &'static str {
        match self {
            Restart::Always => "always",
            Restart::OnFailure => "on-failure",
            Restart::Never => "never",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    Entropy,
    NetworkUp,
    Service(String),
}

impl Need {
    pub fn name(&self) -> &str {
        match self {
            Need::Entropy => "entropy",
            Need::NetworkUp => "network-up",
            Need::Service(name) => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub command: Vec<String>,
    pub user: String,
    pub restart: Restart,
    pub needs: Vec<Need>,
    pub stop_timeout_ms: u64,
}

/// A service name: a lowercase letter or digit, then up to 31 of those and `-`.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Split a command into words: quotes and backslashes as described in the
/// module documentation, no expansion. An unterminated quote or a trailing
/// backslash is an error.
pub fn split_words(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(core::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("unterminated double quote".into()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => return Err("trailing backslash".into()),
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

#[derive(Default)]
struct Draft {
    name: String,
    line: usize,
    command: Option<Vec<String>>,
    user: Option<String>,
    restart: Option<Restart>,
    needs: Option<Vec<Need>>,
    stop_timeout_ms: Option<u64>,
    error: Option<String>,
}

impl Draft {
    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let duplicate = || Err(format!("'{}' is given twice", key));
        match key {
            "command" => {
                if self.command.is_some() {
                    return duplicate();
                }
                let words = split_words(value)?;
                match words.first() {
                    None => return Err("command is empty".into()),
                    Some(path) if !path.starts_with('/') => {
                        return Err(format!("command '{}' is not an absolute path", path))
                    }
                    Some(_) => {}
                }
                if words.iter().any(|w| w.contains('\0')) {
                    return Err("command contains NUL".into());
                }
                self.command = Some(words);
            }
            "user" => {
                if self.user.is_some() {
                    return duplicate();
                }
                if !crate::ssh::account::valid_name(value) {
                    return Err(format!("user '{}' is not an account name", value));
                }
                self.user = Some(value.to_string());
            }
            "restart" => {
                if self.restart.is_some() {
                    return duplicate();
                }
                self.restart = Some(match value {
                    "always" => Restart::Always,
                    "on-failure" => Restart::OnFailure,
                    "never" => Restart::Never,
                    _ => return Err(format!("restart '{}' is not always, on-failure or never", value)),
                });
            }
            "needs" => {
                if self.needs.is_some() {
                    return duplicate();
                }
                let mut needs: Vec<Need> = Vec::new();
                for word in value.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|w| !w.is_empty()) {
                    let need = match word {
                        "entropy" => Need::Entropy,
                        "network-up" => Need::NetworkUp,
                        name if valid_name(name) => Need::Service(name.to_string()),
                        other => return Err(format!("need '{}' is not entropy, network-up or a service name", other)),
                    };
                    if !needs.contains(&need) {
                        needs.push(need);
                    }
                }
                self.needs = Some(needs);
            }
            "stop-timeout" => {
                if self.stop_timeout_ms.is_some() {
                    return duplicate();
                }
                let seconds: u64 = value
                    .parse()
                    .ok()
                    .filter(|s| (1..=300).contains(s))
                    .ok_or_else(|| format!("stop-timeout '{}' is not 1 to 300 seconds", value))?;
                self.stop_timeout_ms = Some(seconds * 1000);
            }
            _ => return Err(format!("unknown key '{}'", key)),
        }
        Ok(())
    }

    fn finish(self, source: &str, services: &mut Vec<Service>, problems: &mut Vec<String>) {
        let at = format!("{}:{}: [{}]", source, self.line, self.name);
        if let Some(error) = self.error {
            problems.push(format!("{} disabled: {}", at, error));
            return;
        }
        let Some(command) = self.command else {
            problems.push(format!("{} disabled: no command", at));
            return;
        };
        if services.iter().any(|s| s.name == self.name) {
            problems.push(format!("{} disabled: the name is already defined", at));
            return;
        }
        services.push(Service {
            name: self.name,
            command,
            user: self.user.unwrap_or_else(|| "root".to_string()),
            restart: self.restart.unwrap_or(Restart::Always),
            needs: self.needs.unwrap_or_default(),
            stop_timeout_ms: self.stop_timeout_ms.unwrap_or(DEFAULT_STOP_TIMEOUT_MS),
        });
    }
}

/// Add the stanzas of one table file (`source` names it in problems) to
/// `services`. A name already in `services` is refused.
pub fn parse_into(text: &str, source: &str, services: &mut Vec<Service>, problems: &mut Vec<String>) {
    let mut draft: Option<Draft> = None;
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(done) = draft.take() {
                done.finish(source, services, problems);
            }
            let Some(name) = rest.strip_suffix(']') else {
                problems.push(format!("{}:{}: a stanza header must be [name]", source, number));
                draft = Some(Draft { line: number, error: Some("malformed header".into()), ..Draft::default() });
                continue;
            };
            let mut next = Draft { name: name.to_string(), line: number, ..Draft::default() };
            if !valid_name(name) {
                next.error = Some(format!("'{}' is not a service name", name));
            }
            draft = Some(next);
            continue;
        }
        let Some(current) = draft.as_mut() else {
            problems.push(format!("{}:{}: a setting outside any [service] stanza is ignored", source, number));
            continue;
        };
        if current.error.is_some() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            current.error = Some(format!("line {} is not key = value", number));
            continue;
        };
        if let Err(why) = current.set(key.trim(), value.trim()) {
            current.error = Some(format!("line {}: {}", number, why));
        }
    }
    if let Some(done) = draft {
        done.finish(source, services, problems);
    }
}

/// Disable each service whose `needs` names a service that is not defined, or
/// that depends on itself, until the remaining dependencies resolve.
pub fn check_needs(services: &mut Vec<Service>, problems: &mut Vec<String>) {
    loop {
        let names: BTreeSet<String> = services.iter().map(|s| s.name.clone()).collect();
        let missing = services.iter().position(|s| {
            s.needs.iter().any(|n| matches!(n, Need::Service(name) if !names.contains(name.as_str())))
        });
        if let Some(index) = missing {
            let service = services.remove(index);
            let absent: Vec<&str> = service
                .needs
                .iter()
                .filter_map(|n| match n {
                    Need::Service(name) if !names.contains(name.as_str()) => Some(name.as_str()),
                    _ => None,
                })
                .collect();
            problems.push(format!("[{}] disabled: needs {}, which is not defined", service.name, absent.join(", ")));
            continue;
        }
        if let Some(index) = (0..services.len()).find(|&i| depends_on(services, i, &services[i].name)) {
            let service = services.remove(index);
            problems.push(format!("[{}] disabled: its needs lead back to itself", service.name));
            continue;
        }
        return;
    }
}

fn depends_on(services: &[Service], start: usize, target: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start];
    while let Some(index) = stack.pop() {
        for need in &services[index].needs {
            let Need::Service(name) = need else { continue };
            if name == target {
                return true;
            }
            if let Some(next) = services.iter().position(|s| &s.name == name) {
                if seen.insert(next) {
                    stack.push(next);
                }
            }
        }
    }
    false
}

/// Read the system table and the runtime definitions under `root` (`/` at
/// run time): the table first, then the runtime files in name order. A
/// missing table gives no services.
pub fn load(root: &std::path::Path) -> (Vec<Service>, Vec<String>) {
    let mut services = Vec::new();
    let mut problems = Vec::new();
    let table = root.join(TABLE.trim_start_matches('/'));
    match std::fs::read_to_string(&table) {
        Ok(text) => parse_into(&text, TABLE, &mut services, &mut problems),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => problems.push(format!("{}: {}", TABLE, e)),
    }
    let runtime = root.join(RUNTIME_TABLES.trim_start_matches('/'));
    if let Ok(entries) = std::fs::read_dir(&runtime) {
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let shown = format!("{}/{}", RUNTIME_TABLES, path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
            match std::fs::read_to_string(&path) {
                Ok(text) => parse_into(&text, &shown, &mut services, &mut problems),
                Err(e) => problems.push(format!("{}: {}", shown, e)),
            }
        }
    }
    check_needs(&mut services, &mut problems);
    (services, problems)
}

/// What the supervisor does after a run ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfterExit {
    /// Restart at all, by the service's policy.
    pub restart: bool,
    /// Wait this long before the restart.
    pub delay_ms: u64,
    /// Consecutive quick failures, including this run.
    pub quick_failures: u32,
    /// The service has failed quickly [`CRASH_LOOP_FAILURES`] times in a row.
    pub crash_loop: bool,
}

/// The restart decision for a run of `run_ms` that `failed` (nonzero exit or a
/// signal), after `quick_failures` consecutive quick failures before it.
pub fn after_exit(policy: Restart, run_ms: u64, failed: bool, quick_failures: u32) -> AfterExit {
    let quick_failures = if failed && run_ms < STABLE_MS { quick_failures.saturating_add(1) } else { 0 };
    let crash_loop = quick_failures >= CRASH_LOOP_FAILURES;
    let delay_ms = if crash_loop {
        MAX_DELAY_MS
    } else if quick_failures <= 1 {
        BASE_DELAY_MS
    } else {
        (BASE_DELAY_MS << (quick_failures - 1)).min(MAX_DELAY_MS)
    };
    let restart = match policy {
        Restart::Always => true,
        Restart::OnFailure => failed,
        Restart::Never => false,
    };
    AfterExit { restart, delay_ms, quick_failures, crash_loop }
}

/// A service's published state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub name: String,
    /// waiting, running, backoff, crash-loop, stopping, stopped, done, failed.
    pub state: String,
    pub user: String,
    pub restart: String,
    pub needs: Vec<String>,
    /// The running process, or 0.
    pub pid: u32,
    /// Starts since boot.
    pub starts: u32,
    /// Monotonic milliseconds of the first start, of the current state, and of
    /// the next start (0: not scheduled).
    pub first_start_ms: u64,
    pub since_ms: u64,
    pub next_ms: u64,
    pub quick_failures: u32,
    /// How the last run ended: `-`, `code:N` or `signal:N`.
    pub last_exit: String,
}

/// The supervisor's own facts: when it began and when each condition first held (0: not yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Supervisor {
    pub pid: u32,
    pub began_ms: u64,
    pub entropy_ms: u64,
    pub network_ms: u64,
}

fn token(text: &str) -> String {
    if text.is_empty() {
        "-".into()
    } else {
        text.chars().map(|c| if c.is_ascii_whitespace() || c == '=' { '_' } else { c }).collect()
    }
}

/// The state file's text.
pub fn render(supervisor: &Supervisor, statuses: &[Status]) -> String {
    let mut text = format!(
        "supervisor pid={} began={} entropy={} network-up={}\n",
        supervisor.pid, supervisor.began_ms, supervisor.entropy_ms, supervisor.network_ms
    );
    for s in statuses {
        text.push_str(&format!(
            "service {} state={} user={} restart={} needs={} pid={} starts={} first={} since={} next={} quick-failures={} exit={}\n",
            s.name,
            token(&s.state),
            token(&s.user),
            token(&s.restart),
            token(&s.needs.join(",")),
            s.pid,
            s.starts,
            s.first_start_ms,
            s.since_ms,
            s.next_ms,
            s.quick_failures,
            token(&s.last_exit)
        ));
    }
    text
}

/// Parse the state file's text; a malformed line is an error.
pub fn parse_state(text: &str) -> Result<(Supervisor, Vec<Status>), String> {
    let mut supervisor = None;
    let mut statuses = Vec::new();
    for line in text.lines() {
        let mut words = line.split(' ');
        let kind = words.next().unwrap_or("");
        let name = if kind == "service" { words.next().unwrap_or("").to_string() } else { String::new() };
        let mut fields = std::collections::BTreeMap::new();
        for word in words {
            let (key, value) = word.split_once('=').ok_or_else(|| format!("malformed state field '{}'", word))?;
            fields.insert(key, value);
        }
        let number = |key: &str| -> Result<u64, String> {
            fields.get(key).and_then(|v| v.parse().ok()).ok_or_else(|| format!("state line lacks {}", key))
        };
        let text_field = |key: &str| -> String {
            match fields.get(key) {
                Some(&"-") | None => String::new(),
                Some(value) => value.to_string(),
            }
        };
        match kind {
            "supervisor" => {
                supervisor = Some(Supervisor {
                    pid: number("pid")? as u32,
                    began_ms: number("began")?,
                    entropy_ms: number("entropy")?,
                    network_ms: number("network-up")?,
                })
            }
            "service" => statuses.push(Status {
                name,
                state: text_field("state"),
                user: text_field("user"),
                restart: text_field("restart"),
                needs: text_field("needs").split(',').filter(|n| !n.is_empty()).map(str::to_string).collect(),
                pid: number("pid")? as u32,
                starts: number("starts")? as u32,
                first_start_ms: number("first")?,
                since_ms: number("since")?,
                next_ms: number("next")?,
                quick_failures: number("quick-failures")? as u32,
                last_exit: text_field("exit"),
            }),
            "" => {}
            other => return Err(format!("unknown state line '{}'", other)),
        }
    }
    Ok((supervisor.ok_or("state has no supervisor line")?, statuses))
}

/// Whether `/proc/net/pnp`'s text says the interface configuration is
/// complete: a DHCP lease (its server is listed) or a manual configuration.
pub fn network_configured(pnp: &str) -> bool {
    pnp.lines().any(|line| line.trim() == "#MANUAL" || line.starts_with("bootserver "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> (Vec<Service>, Vec<String>) {
        let mut services = Vec::new();
        let mut problems = Vec::new();
        parse_into(text, "t", &mut services, &mut problems);
        check_needs(&mut services, &mut problems);
        (services, problems)
    }

    #[test]
    fn stanzas_take_defaults_and_settings() {
        let (services, problems) = parse(
            "# table\n[bsshd]\ncommand = /usr/sbin/bsshd\nneeds = entropy\n\n[ntp]\ncommand=/usr/sbin/ntp_client\n\
             restart = on-failure\nneeds = network-up, bsshd\nuser = nobody\nstop-timeout = 2\n",
        );
        assert!(problems.is_empty(), "{:?}", problems);
        assert_eq!(services[0].command, ["/usr/sbin/bsshd"]);
        assert_eq!((services[0].user.as_str(), services[0].restart), ("root", Restart::Always));
        assert_eq!(services[0].stop_timeout_ms, DEFAULT_STOP_TIMEOUT_MS);
        assert_eq!(services[1].needs, [Need::NetworkUp, Need::Service("bsshd".into())]);
        assert_eq!((services[1].user.as_str(), services[1].restart, services[1].stop_timeout_ms), ("nobody", Restart::OnFailure, 2000));
    }

    #[test]
    fn command_words_follow_quoting() {
        assert_eq!(split_words("/bin/sh -c 'trap \"\" TERM; exec sleep 9'").unwrap(), ["/bin/sh", "-c", "trap \"\" TERM; exec sleep 9"]);
        assert_eq!(split_words(r#"/bin/echo "a \"b\" \\ \n" c\ d '' x"#).unwrap(), ["/bin/echo", "a \"b\" \\ \\n", "c d", "", "x"]);
        assert!(split_words("/bin/echo 'open").is_err());
        assert!(split_words("/bin/echo \"open").is_err());
        assert!(split_words("/bin/echo \\").is_err());
    }

    #[test]
    fn a_bad_stanza_disables_only_itself() {
        let (services, problems) = parse(
            "stray = 1\n[ok]\ncommand = /bin/true\n[relative]\ncommand = true\n[twice]\ncommand = /bin/a\ncommand = /bin/b\n\
             [Bad]\ncommand = /bin/true\n[policy]\ncommand = /bin/true\nrestart = sometimes\n[nocommand]\nuser = root\n\
             [ok]\ncommand = /bin/false\n[timeout]\ncommand = /bin/true\nstop-timeout = 0\n[key]\ncommand = /bin/true\ncolour = red\n",
        );
        assert_eq!(services.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["ok"]);
        assert_eq!(services[0].command, ["/bin/true"]);
        assert_eq!(problems.len(), 9, "{:?}", problems);
        assert!(problems[0].contains("outside any"));
    }

    #[test]
    fn missing_and_circular_needs_disable_their_services() {
        let (services, problems) = parse(
            "[a]\ncommand = /bin/a\nneeds = b\n[b]\ncommand = /bin/b\nneeds = a\n[c]\ncommand = /bin/c\nneeds = ghost\n\
             [d]\ncommand = /bin/d\nneeds = c\n[e]\ncommand = /bin/e\nneeds = entropy\n[f]\ncommand = /bin/f\nneeds = e\n",
        );
        assert_eq!(services.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["e", "f"]);
        assert_eq!(problems.len(), 4, "{:?}", problems);
    }

    #[test]
    fn quick_failures_double_the_delay_until_a_crash_loop() {
        let mut quick = 0;
        let mut delays = Vec::new();
        for _ in 0..7 {
            let next = after_exit(Restart::Always, 50, true, quick);
            quick = next.quick_failures;
            delays.push((next.delay_ms, next.crash_loop));
        }
        assert_eq!(delays, [(1000, false), (2000, false), (4000, false), (8000, false), (60000, true), (60000, true), (60000, true)]);
        // A run that lasts, or a clean exit, starts over.
        assert_eq!(after_exit(Restart::Always, STABLE_MS, true, quick), AfterExit { restart: true, delay_ms: 1000, quick_failures: 0, crash_loop: false });
        assert_eq!(after_exit(Restart::Always, 5, false, quick).quick_failures, 0);
    }

    #[test]
    fn the_policy_decides_whether_to_restart() {
        assert!(after_exit(Restart::OnFailure, 5, true, 0).restart);
        assert!(!after_exit(Restart::OnFailure, 5, false, 0).restart);
        assert!(!after_exit(Restart::Never, 5, true, 0).restart);
        assert!(after_exit(Restart::Always, 50_000, false, 0).restart);
    }

    #[test]
    fn state_round_trips() {
        let supervisor = Supervisor { pid: 1, began_ms: 10, entropy_ms: 11, network_ms: 0 };
        let statuses = vec![Status {
            name: "bsshd".into(),
            state: "running".into(),
            user: "root".into(),
            restart: "always".into(),
            needs: vec!["entropy".into(), "network-up".into()],
            pid: 42,
            starts: 2,
            first_start_ms: 12,
            since_ms: 900,
            next_ms: 0,
            quick_failures: 0,
            last_exit: "signal:9".into(),
        }];
        let text = render(&supervisor, &statuses);
        assert_eq!(parse_state(&text).unwrap(), (supervisor, statuses));
        assert!(parse_state("service x state\n").is_err());
        assert!(parse_state("").is_err());
    }

    #[test]
    fn network_configuration_reads_proc_net_pnp() {
        assert!(network_configured("#PROTO: DHCP\nnameserver 192.0.2.1\nbootserver 192.0.2.1\n"));
        assert!(network_configured("#MANUAL\n"));
        assert!(!network_configured("#PROTO: DHCP\n"));
    }

    #[test]
    fn shipped_tables_parse_cleanly() {
        for (arch, text) in [("x86_64", include_str!("service-tables/x86_64")), ("aarch64", include_str!("service-tables/aarch64"))] {
            let (services, problems) = parse(text);
            assert!(problems.is_empty(), "{}: {:?}", arch, problems);
            assert!(services.iter().any(|s| s.name == "bsshd" && s.needs == [Need::Entropy]), "{}", arch);
        }
    }
}
