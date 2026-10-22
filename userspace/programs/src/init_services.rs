//! init's service supervisor (`libbreenix::services` defines the table and
//! the restart policy).
//!
//! Each pass starts the services whose needs hold, in table order, so a
//! service that waits (bsshd on the random pool) does not hold up the ones after
//! it. It reaps init's children, restarts services by policy with backoff, reports
//! crash loops, stops services on request (SIGTERM, then SIGKILL after the
//! stop timeout), and publishes the state in `/run/breenix/services`.
//!
//! `svc` asks for changes on `/run/breenix/svc.sock`, one line per
//! connection (`start NAME`, `stop NAME`, `restart NAME`, `reload`), and gets
//! one line back (`ok ...` or `error ...`). The caller is identified by
//! SO_PEERCRED: root may do anything; another account may start, stop and
//! restart the services that run as that account.

use std::ffi::CString;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use libbreenix::process::{self, ForkResult};
use libbreenix::types::{Fd, OwnedFd};
use libbreenix::services::{self, AfterExit, Need, Service, Status};
use libbreenix::signal;
use libbreenix::service_logs::{Log, Lines};
use libbreenix::ssh::account::{self, Account, Credentials};

extern "C" {
    fn setgroups(count: usize, groups: *const u32) -> i32;
    fn getgroups(count: i32, groups: *mut u32) -> i32;
    fn setgid(gid: u32) -> i32;
    fn setuid(uid: u32) -> i32;
    fn getuid() -> u32;
    fn geteuid() -> u32;
    fn getgid() -> u32;
    fn getegid() -> u32;
    fn getsockopt(fd: i32, level: i32, name: i32, value: *mut u8, length: *mut u32) -> i32;
}

const SOL_SOCKET: i32 = 1;
const SO_PEERCRED: i32 = 17;
const GRND_NONBLOCK: u64 = 1;
/// One pass per 50 ms, the pace of the getty loop this replaces.
const TICK_MS: u64 = 50;
/// A control connection that sends no whole request line in this time is closed.
const REQUEST_MS: u64 = 2_000;
/// `/proc/net/pnp` is read at most this often while a service waits for the network.
const NETWORK_POLL_MS: u64 = 250;

/// Which of the table's services this init supervises.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The whole table: the system's init.
    All,
    /// Only the login terminals (`init console`).
    Logins,
}

/// The process's own credentials, through the C library's system calls.
struct System;

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

impl Credentials for System {
    fn set_groups(&mut self, groups: &[u32]) -> Result<(), i32> {
        if unsafe { setgroups(groups.len(), groups.as_ptr()) } == 0 { Ok(()) } else { Err(errno()) }
    }
    fn set_gid(&mut self, gid: u32) -> Result<(), i32> {
        if unsafe { setgid(gid) } == 0 { Ok(()) } else { Err(errno()) }
    }
    fn set_uid(&mut self, uid: u32) -> Result<(), i32> {
        if unsafe { setuid(uid) } == 0 { Ok(()) } else { Err(errno()) }
    }
    fn ids(&self) -> (u32, u32, u32, u32) {
        unsafe { (getuid(), geteuid(), getgid(), getegid()) }
    }
    fn groups(&self) -> Vec<u32> {
        let mut groups = vec![0u32; 64];
        let count = unsafe { getgroups(groups.len() as i32, groups.as_mut_ptr()) };
        groups.truncate(count.max(0) as usize);
        groups
    }
}

fn now_ms() -> u64 {
    libbreenix::time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0)
}

/// Whether the kernel's random pool is ready: a non-blocking getrandom succeeds.
fn entropy_ready() -> bool {
    use libbreenix::syscall::{nr, raw};
    let mut byte = [0u8; 1];
    let ret = unsafe { raw::syscall3(nr::GETRANDOM, byte.as_mut_ptr() as u64, 1, GRND_NONBLOCK) } as i64;
    ret == 1
}

fn network_up() -> bool {
    std::fs::read_to_string("/proc/net/pnp").map(|text| services::network_configured(&text)).unwrap_or(false)
}

/// For a getty service, the terminal it serves; an inner value without a name is the console.
fn login_terminal(service: &Service) -> Option<Option<&str>> {
    service.command[0].ends_with("/getty").then(|| service.command.get(1).map(String::as_str))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Waiting,
    Running,
    Backoff,
    CrashLoop,
    Stopping,
    Stopped,
    Done,
    Failed,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Running => "running",
            State::Backoff => "backoff",
            State::CrashLoop => "crash-loop",
            State::Stopping => "stopping",
            State::Stopped => "stopped",
            State::Done => "done",
            State::Failed => "failed",
        }
    }
}

/// What follows a requested stop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterStop {
    Stay,
    Start,
    Remove,
}

/// A request whose answer waits for a service to stop or to start.
struct Pending {
    stream: UnixStream,
    started: bool,
}

struct Entry {
    def: Service,
    /// A definition from `svc reload`, taken at the next start.
    replacement: Option<Service>,
    state: State,
    pid: i32,
    started_ms: u64,
    first_start_ms: u64,
    since_ms: u64,
    next_ms: u64,
    starts: u32,
    quick_failures: u32,
    crash_loop_reported: bool,
    last_exit: String,
    stop_deadline_ms: u64,
    killed: bool,
    after_stop: AfterStop,
    pending: Vec<Pending>,
    log: Option<Log>,
    outputs: Vec<Output>,
}

impl Entry {
    fn new(def: Service, now: u64) -> Self {
        Entry {
            def,
            replacement: None,
            state: State::Waiting,
            pid: 0,
            started_ms: 0,
            first_start_ms: 0,
            since_ms: now,
            next_ms: 0,
            starts: 0,
            quick_failures: 0,
            crash_loop_reported: false,
            last_exit: String::new(),
            stop_deadline_ms: 0,
            killed: false,
            after_stop: AfterStop::Stay,
            pending: Vec::new(),
            log: None,
            outputs: Vec::new(),
        }
    }

    fn set(&mut self, state: State, now: u64) {
        self.state = state;
        self.since_ms = now;
    }

    fn answer(&mut self, started: bool, text: &str) {
        let mut kept = Vec::new();
        for mut pending in self.pending.drain(..) {
            if pending.started == started {
                let _ = pending.stream.write_all(format!("{}\n", text).as_bytes());
            } else {
                kept.push(pending);
            }
        }
        self.pending = kept;
    }
}

struct Connection {
    stream: UnixStream,
    uid: u32,
    opened_ms: u64,
    line: Vec<u8>,
}

pub struct Supervisor {
    scope: Scope,
    /// Publish state and serve `svc`: only the system's init (PID 1) does.
    publish: bool,
    entries: Vec<Entry>,
    facts: services::Supervisor,
    listener: Option<UnixListener>,
    connections: Vec<Connection>,
    network_checked_ms: u64,
    changed: bool,
    boot_id: String,
}

fn exit_text(status: i32) -> String {
    if process::wifsignaled(status) {
        format!("signal:{}", process::wtermsig(status))
    } else {
        format!("code:{}", process::wexitstatus(status))
    }
}

fn in_scope(scope: Scope, service: &Service) -> bool {
    scope == Scope::All || login_terminal(service).is_some()
}

impl Supervisor {
    /// Read the table and, for the system's init, open `/run/breenix` and the control socket.
    pub fn load(scope: Scope, publish: bool) -> Self {
        let now = now_ms();
        let (definitions, problems) = services::load(Path::new("/"));
        for problem in &problems {
            print!("[init] service table: {}\n", problem);
        }
        let entries: Vec<Entry> =
            definitions.into_iter().filter(|s| in_scope(scope, s)).map(|s| Entry::new(s, now)).collect();
        print!("[init] supervising {} services from {}\n", entries.len(), services::TABLE);
        let mut supervisor = Supervisor {
            scope,
            publish,
            entries,
            facts: services::Supervisor { pid: std::process::id(), began_ms: now, entropy_ms: 0, network_ms: 0 },
            listener: None,
            connections: Vec::new(),
            network_checked_ms: 0,
            changed: true,
            boot_id: String::new(),
        };
        if publish {
            supervisor.listener = match open_control() {
                Ok(listener) => Some(listener),
                Err(why) => {
                    print!("[init] svc control unavailable: {}\n", why);
                    None
                }
            };
        }
        if publish {
            supervisor.boot_id = std::fs::read_to_string("/run/breenix/boot-id")
                .expect("init log boot ID").trim().to_string();
        }
        supervisor
    }

    pub fn run(mut self) -> ! {
        loop {
            self.tick();
            let _ = libbreenix::time::sleep_ms(TICK_MS);
        }
    }

    /// One pass: reap, serve requests, update conditions, stop and start.
    pub fn tick(&mut self) {
        self.drain_logs();
        self.reap();
        self.serve();
        let now = now_ms();
        self.conditions(now);
        self.enforce_stops(now);
        self.start_due(now);
        self.mark_stable(now);
        if self.changed {
            self.changed = false;
            self.write_state();
        }
    }

    fn drain_logs(&mut self) {
        for entry in &mut self.entries {
            let Some(log) = &mut entry.log else { continue };
            log.retry();
            for output in &mut entry.outputs {
                if output.ended { continue; }
                let mut bytes = [0u8; 4096];
                for _ in 0..16 {
                    match output.reader.read(&mut bytes) {
                        Ok(0) => { output.lines.finish(output.stream, log); output.ended = true; break; },
                        Ok(n) => {
                            output.lines.feed(&bytes[..n], output.stream, log);
                        },
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => { log.record(b"[log] lost lines=unknown reason=pipe-read-error\n"); output.ended = true; break; },
                    }
                }
            }
            entry.outputs.retain(|o| !o.ended);
        }
    }

    fn conditions(&mut self, now: u64) {
        let waiting = |need: &Need| {
            self.entries.iter().any(|e| {
                matches!(e.state, State::Waiting | State::Backoff | State::CrashLoop) && e.def.needs.contains(need)
            })
        };
        if self.facts.entropy_ms == 0 && waiting(&Need::Entropy) && entropy_ready() {
            self.facts.entropy_ms = now;
            print!("[init] random pool ready; starting the services that need entropy\n");
            self.changed = true;
        }
        if self.facts.network_ms == 0
            && now >= self.network_checked_ms + NETWORK_POLL_MS
            && waiting(&Need::NetworkUp)
        {
            self.network_checked_ms = now;
            if network_up() {
                self.facts.network_ms = now;
                print!("[init] network up; starting the services that need it\n");
                self.changed = true;
            }
        }
    }

    fn needs_hold(&self, index: usize) -> bool {
        self.entries[index].def.needs.iter().all(|need| match need {
            Need::Entropy => self.facts.entropy_ms != 0,
            Need::NetworkUp => self.facts.network_ms != 0,
            Need::Service(name) => self
                .entries
                .iter()
                .any(|e| &e.def.name == name && matches!(e.state, State::Running | State::Done)),
        })
    }

    fn start_due(&mut self, now: u64) {
        for index in 0..self.entries.len() {
            let entry = &self.entries[index];
            if !matches!(entry.state, State::Waiting | State::Backoff | State::CrashLoop) || now < entry.next_ms {
                continue;
            }
            if !self.needs_hold(index) {
                continue;
            }
            self.start(index, now);
        }
    }

    fn start(&mut self, index: usize, now: u64) {
        let entry = &mut self.entries[index];
        if let Some(def) = entry.replacement.take() {
            entry.def = def;
        }
        if self.publish && entry.log.is_none() {
            entry.log = Some(Log::new(Path::new("/var/log"), &entry.def.name, &self.boot_id)
                .unwrap_or_else(|e| {
                    println!("[init] {}: log unavailable; starting anyway: {}", entry.def.name, e);
                    Log::deferred(Path::new("/var/log"), &entry.def.name, &self.boot_id)
                }));
        }
        let result = account::read_account(Path::new("/"), &entry.def.user)
            .map_err(|why| why.to_string())
            .and_then(|account| launch(&entry.def, &account, self.publish));
        self.changed = true;
        match result {
            Ok((pid, outputs)) => {
                entry.outputs.extend(outputs);
                entry.pid = pid;
                entry.starts += 1;
                entry.started_ms = now;
                if entry.first_start_ms == 0 {
                    entry.first_start_ms = now;
                }
                entry.next_ms = 0;
                entry.killed = false;
                entry.set(State::Running, now);
                match login_terminal(&entry.def) {
                    Some(None) => print!("[init] getty on the console\n"),
                    Some(Some(tty)) => print!("[init] getty on {}\n", tty),
                    None => print!("[init] {} started (PID {})\n", entry.def.name, pid),
                }
                entry.answer(true, &format!("ok started {} pid {}", entry.def.name, pid));
            }
            Err(why) => {
                print!("[init] {}: cannot start: {}\n", entry.def.name, why);
                entry.last_exit = "start-failed".into();
                let decision = services::after_exit(entry.def.restart, 0, true, entry.quick_failures);
                entry.answer(true, &format!("error {} did not start: {}", entry.def.name, why));
                Self::schedule(entry, decision, now);
            }
        }
    }

    /// Put an ended run into its next state by the policy's decision.
    fn schedule(entry: &mut Entry, decision: AfterExit, now: u64) {
        entry.quick_failures = decision.quick_failures;
        if decision.quick_failures == 0 {
            entry.crash_loop_reported = false;
        }
        if !decision.restart {
            let failed = entry.last_exit != "code:0";
            entry.set(if failed { State::Failed } else { State::Done }, now);
            return;
        }
        entry.next_ms = now + decision.delay_ms;
        if decision.crash_loop {
            entry.set(State::CrashLoop, now);
            if !entry.crash_loop_reported {
                entry.crash_loop_reported = true;
                print!(
                    "[init] {} is crash-looping: {} failures in a row, each within {} s of starting; restarting every {} s\n",
                    entry.def.name,
                    decision.quick_failures,
                    services::STABLE_MS / 1000,
                    services::MAX_DELAY_MS / 1000
                );
            }
        } else {
            entry.set(State::Backoff, now);
        }
    }

    fn reap(&mut self) {
        loop {
            let mut status = 0;
            let pid = match process::waitpid(-1, &mut status, process::WNOHANG) {
                Ok(pid) if pid.raw() != 0 => pid.raw() as i32,
                _ => return,
            };
            let now = now_ms();
            let Some(index) = self.entries.iter().position(|e| e.pid == pid && e.state.is_live()) else {
                if process::wifsignaled(status) {
                    print!("[init] Process {} killed by signal {}\n", pid, process::wtermsig(status));
                } else {
                    print!("[init] Process {} exited (code {})\n", pid, process::wexitstatus(status));
                }
                continue;
            };
            self.changed = true;
            let entry = &mut self.entries[index];
            entry.pid = 0;
            entry.last_exit = exit_text(status);
            let failed = entry.last_exit != "code:0";
            let run_ms = now.saturating_sub(entry.started_ms);
            let terminal = login_terminal(&entry.def);
            if entry.state == State::Stopping {
                let text = format!("ok stopped {} ({})", entry.def.name, entry.last_exit.replace(':', " "));
                print!("[init] {} stopped ({})\n", entry.def.name, entry.last_exit.replace(':', " "));
                entry.quick_failures = 0;
                entry.crash_loop_reported = false;
                entry.set(State::Stopped, now);
                entry.answer(false, &text);
                match entry.after_stop {
                    AfterStop::Stay => {}
                    AfterStop::Start => {
                        entry.next_ms = 0;
                        entry.set(State::Waiting, now);
                    }
                    AfterStop::Remove => {
                        self.entries.remove(index);
                    }
                }
                continue;
            }
            let decision = services::after_exit(entry.def.restart, run_ms, failed, entry.quick_failures);
            match terminal {
                Some(None) if decision.restart => print!("[init] console session ended; restarting getty\n"),
                Some(None) => print!("[init] console session ended\n"),
                Some(Some(tty)) => print!("[init] {} session ended\n", tty),
                None if process::wifsignaled(status) => {
                    print!("[init] {} killed by signal {}\n", entry.def.name, process::wtermsig(status))
                }
                None => print!("[init] {} exited (code {})\n", entry.def.name, process::wexitstatus(status)),
            }
            if terminal.is_none() && decision.restart && !decision.crash_loop {
                print!("[init] {} restarts in {} ms\n", entry.def.name, decision.delay_ms);
            }
            Self::schedule(entry, decision, now);
        }
    }

    fn enforce_stops(&mut self, now: u64) {
        for entry in &mut self.entries {
            if entry.state == State::Stopping && !entry.killed && now >= entry.stop_deadline_ms {
                entry.killed = true;
                print!(
                    "[init] {} did not stop within {} s of SIGTERM; sending SIGKILL\n",
                    entry.def.name,
                    entry.def.stop_timeout_ms / 1000
                );
                let _ = signal::kill(entry.pid, signal::SIGKILL);
            }
        }
    }

    /// A run that lasts STABLE_MS ends a backoff streak and a crash loop.
    fn mark_stable(&mut self, now: u64) {
        for entry in &mut self.entries {
            if entry.state == State::Running
                && entry.quick_failures > 0
                && now.saturating_sub(entry.started_ms) >= services::STABLE_MS
            {
                entry.quick_failures = 0;
                if entry.crash_loop_reported {
                    entry.crash_loop_reported = false;
                    print!("[init] {} recovered: running for {} s\n", entry.def.name, services::STABLE_MS / 1000);
                }
                self.changed = true;
            }
        }
    }

    /// Begin stopping a running service: SIGTERM now, SIGKILL at its stop timeout.
    fn stop(entry: &mut Entry, after: AfterStop, now: u64) {
        entry.after_stop = after;
        entry.stop_deadline_ms = now + entry.def.stop_timeout_ms;
        entry.killed = false;
        entry.set(State::Stopping, now);
        let _ = signal::kill(entry.pid, signal::SIGTERM);
    }

    fn serve(&mut self) {
        let Some(listener) = &self.listener else { return };
        let now = now_ms();
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let uid = peer_uid(&stream);
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    match uid {
                        Some(uid) => self.connections.push(Connection { stream, uid, opened_ms: now, line: Vec::new() }),
                        None => {
                            let mut stream = stream;
                            let _ = stream.write_all(b"error the caller's credentials are unavailable\n");
                        }
                    }
                }
                Err(_) => break,
            }
        }
        let mut index = 0;
        while index < self.connections.len() {
            let connection = &mut self.connections[index];
            let mut buffer = [0u8; 128];
            let mut closed = false;
            loop {
                match connection.stream.read(&mut buffer) {
                    Ok(0) => {
                        closed = true;
                        break;
                    }
                    Ok(n) => connection.line.extend_from_slice(&buffer[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        closed = true;
                        break;
                    }
                }
            }
            if let Some(end) = connection.line.iter().position(|b| *b == b'\n') {
                let connection = self.connections.swap_remove(index);
                let line = String::from_utf8_lossy(&connection.line[..end]).into_owned();
                self.request(connection.stream, connection.uid, line.trim(), now);
                continue;
            }
            if closed || connection.line.len() > 256 || now >= connection.opened_ms + REQUEST_MS {
                let mut connection = self.connections.swap_remove(index);
                let _ = connection.stream.write_all(b"error no request line\n");
                continue;
            }
            index += 1;
        }
    }

    fn request(&mut self, mut stream: UnixStream, uid: u32, line: &str, now: u64) {
        let words: Vec<&str> = line.split_whitespace().collect();
        let reply = match words.as_slice() {
            ["reload"] if uid != 0 => Err("permission denied: reload needs root".to_string()),
            ["reload"] => Ok(Some(self.reload(now))),
            [verb @ ("start" | "stop" | "restart"), name] => self.control(&mut stream, uid, verb, name, now),
            _ => Err(format!("unknown request '{}'", line)),
        };
        self.changed = true;
        match reply {
            Ok(Some(text)) => {
                let _ = stream.write_all(format!("ok {}\n", text).as_bytes());
            }
            Ok(None) => {}
            Err(text) => {
                let _ = stream.write_all(format!("error {}\n", text).as_bytes());
            }
        }
    }

    /// Start, stop or restart one service. An `Ok` without text means the answer
    /// waits for the service, and `stream` has been kept for it.
    fn control(&mut self, stream: &mut UnixStream, uid: u32, verb: &str, name: &str, now: u64) -> Result<Option<String>, String> {
        let index = self.entries.iter().position(|e| e.def.name == name).ok_or_else(|| format!("no service '{}'", name))?;
        if uid != 0 {
            let owner = account::read_account(Path::new("/"), &self.entries[index].def.user).map(|a| a.uid);
            if owner != Ok(uid) {
                return Err(format!("permission denied: {} runs as {}", name, self.entries[index].def.user));
            }
        }
        let keep = |entry: &mut Entry, started: bool, stream: &mut UnixStream| -> Result<Option<String>, String> {
            let kept = stream.try_clone().map_err(|e| format!("cannot keep the request: {}", e))?;
            entry.pending.push(Pending { stream: kept, started });
            Ok(None)
        };
        let blocked = |supervisor: &Self| -> Option<String> {
            let unmet: Vec<&str> = supervisor.entries[index]
                .def
                .needs
                .iter()
                .filter(|need| match need {
                    Need::Entropy => supervisor.facts.entropy_ms == 0,
                    Need::NetworkUp => supervisor.facts.network_ms == 0,
                    Need::Service(other) => !supervisor
                        .entries
                        .iter()
                        .any(|e| &e.def.name == other && matches!(e.state, State::Running | State::Done)),
                })
                .map(Need::name)
                .collect();
            (!unmet.is_empty()).then(|| unmet.join(", "))
        };
        let state = self.entries[index].state;
        match verb {
            "stop" => match state {
                State::Running => {
                    Self::stop(&mut self.entries[index], AfterStop::Stay, now);
                    keep(&mut self.entries[index], false, stream)
                }
                State::Stopping => {
                    self.entries[index].after_stop = AfterStop::Stay;
                    keep(&mut self.entries[index], false, stream)
                }
                State::Waiting | State::Backoff | State::CrashLoop => {
                    let entry = &mut self.entries[index];
                    entry.next_ms = 0;
                    entry.quick_failures = 0;
                    entry.crash_loop_reported = false;
                    entry.set(State::Stopped, now);
                    print!("[init] {} stopped before its next start\n", name);
                    entry.answer(true, &format!("error {} was stopped before it started", name));
                    Ok(Some(format!("stopped {} (not running)", name)))
                }
                State::Stopped | State::Done | State::Failed => Ok(Some(format!("{} is not running ({})", name, state.name()))),
            },
            "start" | "restart" => match state {
                State::Running if verb == "start" => Ok(Some(format!("{} is already running (pid {})", name, self.entries[index].pid))),
                State::Running => {
                    Self::stop(&mut self.entries[index], AfterStop::Start, now);
                    keep(&mut self.entries[index], true, stream)
                }
                State::Stopping => {
                    self.entries[index].after_stop = AfterStop::Start;
                    keep(&mut self.entries[index], true, stream)
                }
                _ => {
                    let entry = &mut self.entries[index];
                    entry.next_ms = 0;
                    entry.quick_failures = 0;
                    entry.crash_loop_reported = false;
                    entry.set(State::Waiting, now);
                    match blocked(self) {
                        Some(unmet) => Ok(Some(format!("{} waits for {}", name, unmet))),
                        None => keep(&mut self.entries[index], true, stream),
                    }
                }
            },
            _ => Err(format!("unknown request '{}'", verb)),
        }
    }

    /// Read the tables again: add new services, take changed definitions at
    /// their next start, and stop and drop services that are gone.
    fn reload(&mut self, now: u64) -> String {
        let (definitions, problems) = services::load(Path::new("/"));
        for problem in &problems {
            print!("[init] service table: {}\n", problem);
        }
        let definitions: Vec<Service> = definitions.into_iter().filter(|s| in_scope(self.scope, s)).collect();
        let (mut added, mut changed, mut removed) = (0, 0, 0);
        let mut index = 0;
        while index < self.entries.len() {
            let name = self.entries[index].def.name.clone();
            if definitions.iter().any(|d| d.name == name) {
                index += 1;
                continue;
            }
            removed += 1;
            let entry = &mut self.entries[index];
            match entry.state {
                State::Running => Self::stop(entry, AfterStop::Remove, now),
                State::Stopping => entry.after_stop = AfterStop::Remove,
                _ => {
                    entry.answer(true, &format!("error {} was removed from the table", name));
                    self.entries.remove(index);
                    continue;
                }
            }
            index += 1;
        }
        for def in definitions {
            match self.entries.iter_mut().find(|e| e.def.name == def.name) {
                Some(entry) if entry.def != def => {
                    changed += 1;
                    if entry.state.is_live() {
                        entry.replacement = Some(def);
                    } else {
                        entry.def = def;
                    }
                }
                Some(_) => {}
                None => {
                    added += 1;
                    self.entries.push(Entry::new(def, now));
                }
            }
        }
        print!("[init] service table reloaded: {} added, {} changed, {} removed\n", added, changed, removed);
        format!(
            "reloaded {} services: {} added, {} changed, {} removed, {} problems",
            self.entries.len(),
            added,
            changed,
            removed,
            problems.len()
        )
    }

    fn write_state(&self) {
        if !self.publish {
            return;
        }
        let statuses: Vec<Status> = self
            .entries
            .iter()
            .map(|e| Status {
                name: e.def.name.clone(),
                state: e.state.name().into(),
                user: e.def.user.clone(),
                restart: e.def.restart.name().into(),
                needs: e.def.needs.iter().map(|n| n.name().to_string()).collect(),
                pid: e.pid.max(0) as u32,
                starts: e.starts,
                first_start_ms: e.first_start_ms,
                since_ms: e.since_ms,
                next_ms: e.next_ms,
                quick_failures: e.quick_failures,
                last_exit: e.last_exit.clone(),
            })
            .collect();
        let text = services::render(&self.facts, &statuses);
        let staged = format!("{}.new", services::STATE);
        let written = std::fs::write(&staged, text.as_bytes())
            .and_then(|_| std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o644)))
            .and_then(|_| std::fs::rename(&staged, services::STATE));
        if let Err(e) = written {
            print!("[init] cannot publish service state: {}\n", e);
        }
    }
}

impl State {
    /// A process exists for the service.
    fn is_live(self) -> bool {
        matches!(self, State::Running | State::Stopping)
    }
}

fn open_control() -> Result<UnixListener, String> {
    for (directory, mode) in [(services::RUN_DIR, 0o755), (services::RUNTIME_TABLES, 0o755)] {
        match std::fs::create_dir(directory) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {}", directory, e)),
        }
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode)).map_err(|e| format!("{}: {}", directory, e))?;
    }
    match std::fs::remove_file(services::SOCKET) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {}", services::SOCKET, e)),
    }
    let listener = UnixListener::bind(services::SOCKET).map_err(|e| format!("{}: {}", services::SOCKET, e))?;
    // Any account may ask; each request is checked against the caller's uid.
    std::fs::set_permissions(services::SOCKET, std::fs::Permissions::from_mode(0o666))
        .map_err(|e| format!("{}: {}", services::SOCKET, e))?;
    listener.set_nonblocking(true).map_err(|e| format!("{}: {}", services::SOCKET, e))?;
    Ok(listener)
}

/// The connecting process's uid, from SO_PEERCRED (pid, uid, gid).
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut credentials = [0u32; 3];
    let mut length = 12u32;
    let ret = unsafe {
        getsockopt(stream.as_raw_fd(), SOL_SOCKET, SO_PEERCRED, credentials.as_mut_ptr().cast(), &mut length)
    };
    (ret == 0 && length == 12).then_some(credentials[1])
}

/// One captured service stream; retained across exits until its pipe reaches EOF.
struct Output {
    reader: std::fs::File,
    stream: &'static str,
    lines: Lines,
    ended: bool,
}

/// Fork, take the account credentials and exec with a system PATH, HOME, USER,
/// LOGNAME and init's TERM. The child's working directory is `/`.
fn launch(def: &Service, account: &Account, capture: bool) -> Result<(i32, Vec<Output>), String> {
    let words: Vec<CString> =
        def.command.iter().map(|w| CString::new(w.as_str())).collect::<Result<_, _>>().map_err(|_| "command contains NUL")?;
    let mut environment = vec![
        format!("HOME={}", account.home),
        format!("USER={}", account.name),
        format!("LOGNAME={}", account.name),
        "PATH=/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
    ];
    if capture && login_terminal(def).is_some() {
        environment.push("BREENIX_SERVICE_LOG=1".into());
    }
    if let Ok(term) = std::env::var("TERM") {
        environment.push(format!("TERM={}", term));
    }
    let environment: Vec<CString> =
        environment.into_iter().map(CString::new).collect::<Result<_, _>>().map_err(|_| "environment contains NUL")?;
    let path = words[0].as_bytes_with_nul().to_vec();
    let mut argv: Vec<*const u8> = words.iter().map(|w| w.as_ptr().cast()).collect();
    argv.push(core::ptr::null());
    let mut envp: Vec<*const u8> = environment.iter().map(|e| e.as_ptr().cast()).collect();
    envp.push(core::ptr::null());
    let mut outputs = Vec::new();
    let mut writers = Vec::new();
    if capture {
        for stream in ["stdout", "stderr"] {
            let (read, write) = libbreenix::io::pipe2(libbreenix::io::status_flags::O_CLOEXEC)
                .map_err(|e| format!("log pipe: {}", e))?;
            let reader = unsafe { std::fs::File::from_raw_fd(read.raw() as i32) };
            libbreenix::io::fcntl(read, libbreenix::io::fcntl_cmd::F_SETFL,
                libbreenix::io::status_flags::O_NONBLOCK as i64).map_err(|e| format!("log reader: {}", e))?;
            outputs.push(Output { reader, stream, lines: Lines::new(), ended: false });
            writers.push(OwnedFd::new(write));
        }
    }
    // The writer closes on successful exec; setup and exec failures send an
    // error instead. Bound the wait so a stalled child cannot wedge init.
    let (read, write) = libbreenix::io::pipe2(libbreenix::io::status_flags::O_CLOEXEC | libbreenix::io::status_flags::O_NONBLOCK)
        .map_err(|e| format!("exec pipe: {}", e))?;
    let mut reader = unsafe { std::fs::File::from_raw_fd(read.raw() as i32) };
    let writer = OwnedFd::new(write);
    match process::fork().map_err(|e| format!("fork: {}", e))? {
        ForkResult::Child => {
            drop(reader);
            drop(outputs);
            let fail = |why: String, code: i32| -> ! {
                let _ = libbreenix::io::write(writer.fd(), why.as_bytes());
                process::exit(code);
            };
            for (output, target) in writers.iter().zip([Fd::STDOUT, Fd::STDERR]) {
                if let Err(e) = libbreenix::io::dup2(output.fd(), target) { fail(format!("log output: {}", e), 126); }
            }
            drop(writers);
            // Getty acquires its own terminal. Other services get EOF on
            // stdin and write their output into the captured pipes.
            if login_terminal(def).is_none() {
                let redirect = || -> Result<(), String> {
                    let input = OwnedFd::new(libbreenix::fs::open("/dev/null", libbreenix::fs::O_RDONLY)
                        .map_err(|e| format!("open service stdin: {}", e))?);
                    libbreenix::io::dup2(input.fd(), Fd::STDIN).map_err(|e| format!("service stdin: {}", e))?;
                    Ok(())
                };
                if let Err(why) = redirect() {
                    fail(why, 126);
                }
            }
            if let Err(why) = account::become_account(account, &mut System) {
                fail(format!("cannot become {}: {}", account.name, why), 126);
            }
            if let Err(why) = process::chdir(b"/\0") {
                fail(format!("chdir: {}", why), 126);
            }
            let error = process::execve(&path, argv.as_ptr(), envp.as_ptr());
            fail(format!("exec {}: {:?}", def.command[0], error.err()), 127);
        }
        ForkResult::Parent(pid) => {
            drop(writer);
            drop(writers);
            let deadline = now_ms() + REQUEST_MS;
            let mut buffer = [0u8; 512];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return Ok((pid.raw() as i32, outputs)),
                    Ok(n) => return Err(String::from_utf8_lossy(&buffer[..n]).into_owned()),
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == ErrorKind::WouldBlock && now_ms() < deadline => {
                        let _ = libbreenix::time::sleep_ms(1);
                    }
                    Err(why) => {
                        let _ = signal::kill(pid.raw() as i32, signal::SIGKILL);
                        return Err(format!("exec acknowledgement: {}", why));
                    }
                }
            }
        }
    }
}

/// Run by the services suite as a non-root service with no terminal input.
pub fn check_service_io() {
    use libbreenix::io::{fcntl, fcntl_cmd, status_flags};
    let mut byte = [0u8; 1];
    for fd in [Fd::STDIN, Fd::STDOUT, Fd::STDERR] {
        let flags = fcntl(fd, fcntl_cmd::F_GETFL, 0).expect("service descriptor flags");
        fcntl(fd, fcntl_cmd::F_SETFL, flags | status_flags::O_NONBLOCK as i64).expect("nonblocking service descriptor");
        let result = unsafe {
            libbreenix::syscall::raw::syscall3(libbreenix::syscall::nr::READ, fd.raw(), byte.as_mut_ptr() as u64, 1)
        } as i64;
        assert_eq!(result, if fd == Fd::STDIN { 0 } else { -9 }, "service console input on {:?}", fd);
    }
    println!("service-io-ok");
}
