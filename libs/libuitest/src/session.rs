//! Case-side client for the runner's sibling session supervisor.
use std::collections::{BTreeMap, BTreeSet};
use libbreenix::{process, time};
use crate::{channel::{Channel, now}, supervisor::{Message, SessionEnvironment, signal_name}};
use crate::supervisor::devices::RawInput;
use crate::window_wait::{WaitWindow, WindowReady};

#[derive(Clone, Debug)]
pub struct ProcessExit { pub pid: u64, pub status: i32 }

#[derive(Clone, Debug, Default)]
pub struct AppStderr { pub bytes: Vec<u8>, pub dropped: u64 }

pub struct Session {
    environment: SessionEnvironment,
    configured: bool,
    devices: Option<[u32; 2]>,
    input_done: bool,
    stderr: Option<(u64, AppStderr, bool)>,
    control: Channel,
    supervisor: u64,
    names: BTreeMap<u64, String>,
    output: BTreeMap<u64, Vec<u8>>,
    exits: Vec<ProcessExit>,
    launched: Option<u64>,
    stopped: bool,
    compositor: Option<u64>,
    excluded: BTreeSet<u64>,
}
impl Session {
    /// Start bwm under the runner's sibling supervisor, with the session defaults.
    pub fn desktop() -> Result<Self, String> { Self::start(false) }
    /// Start the automatic test compositor with a live supervisor-owned stdin.
    pub fn kiosk() -> Result<Self, String> { Self::start(true) }
    fn start(kiosk: bool) -> Result<Self, String> {
        let mut session = Self::connect()?;
        session.control.send(&Message::StartCompositor { kiosk })?;
        let path: &[u8] = if kiosk { b"/usr/libexec/breenix/kiosk_test" } else { b"/usr/bin/bwm" };
        let pid = session.await_launch(&[path])?;
        session.compositor = Some(pid);
        if kiosk {
            let line = session.read_line(pid)?;
            if !line.strip_prefix("ready ").and_then(|s| s.parse::<u64>().ok()).is_some_and(|epoch| epoch != 0) {
                return Err(format!("kiosk startup: {line}"));
            }
            // The queued auto command is answered after EVENT_FD is open. This
            // closes the race where an app commits before the scene fd exists.
            let line = session.read_line(pid)?;
            if line != "ok" { return Err(format!("kiosk automatic composition: {line}")); }
        }
        Ok(session)
    }
    pub fn connect() -> Result<Self, String> { Self::connect_with_seed(0) }
    pub fn connect_with_seed(seed: u64) -> Result<Self, String> {
        let fd = breenix_suite::suite::take_session_control().ok_or("case did not declare a session")?;
        let control = Channel::new(fd)?;
        let case_pid = process::getpid().map_err(|e| e.to_string())?.raw();
        let mode = libbreenix::gpu::current_mode().map_err(|e| format!("session mode: {e}"))?;
        let mode = format!("{}x{}@{}", mode.width, mode.height, mode.scale);
        let environment = SessionEnvironment::new(case_pid, seed, &mode)?;
        let mut session = Self { environment, configured: false, devices: None, input_done: false, stderr: None, control, supervisor: 0, names: BTreeMap::new(), output: BTreeMap::new(), exits: Vec::new(), launched: None, stopped: false, compositor: None, excluded: BTreeSet::new() };
        let end = now()? + 1000;
        while session.supervisor == 0 || session.devices.is_none() { session.poll(end)?; }
        session.control.send(&Message::Configure { case_pid, seed, mode })?;
        let end = now()? + 1000;
        while !session.configured { session.poll(end)?; }
        Ok(session)
    }
    pub fn input_devices(&self) -> [u32; 2] { self.devices.expect("connected session devices") }
    fn raw_input(&mut self, event: RawInput) -> Result<(), String> {
        if self.stopped { return Err("session has ended".into()); }
        self.input_done = false;
        self.control.send(&Message::Input(event))?;
        let end = now()? + 2000;
        while !self.input_done { self.poll(end)?; }
        Ok(())
    }
    pub fn raw_press(&mut self, code: u16) -> Result<(), String> { self.raw_input(RawInput { kind: 1, code, value: 1 }) }
    pub fn raw_release(&mut self, code: u16) -> Result<(), String> { self.raw_input(RawInput { kind: 1, code, value: 0 }) }
    pub fn raw_motion(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.raw_input(RawInput { kind: 2, code: 0, value: x })?;
        self.raw_input(RawInput { kind: 2, code: 1, value: y })
    }
    pub fn raw_wheel(&mut self, delta: i32) -> Result<(), String> { self.raw_input(RawInput { kind: 2, code: 8, value: delta }) }
    pub fn config_root(&self) -> &str { self.environment.config_root() }
    /// Exclude a window from required clock/settle participation (plan 6.3).
    pub fn exclude(&mut self, window: u64) { self.excluded.insert(window); }
    pub fn is_excluded(&self, window: u64) -> bool { self.excluded.contains(&window) }

    /// Wait at most two real seconds for a registered title's completed commit.
    pub fn wait_window(&mut self, title: &str) -> Result<WindowReady, String> {
        self.wait_window_until(title, Self::now_ns()?.saturating_add(2_000_000_000))
    }
    /// Absolute monotonic deadline, so a startup budget includes connect/launch.
    pub fn wait_window_until(&mut self, title: &str, deadline_ns: u64) -> Result<WindowReady, String> {
        if self.stopped { return Err("session has ended".into()); }
        let wait = WaitWindow::new(title, deadline_ns);
        let mut infos = vec![libbreenix::window::abi::WinInfo::default(); 64];
        loop {
            let at = Self::now_ns()?;
            if at > deadline_ns { return Err(format!("wait-window-timeout title={title:?} deadline_ns={deadline_ns}")); }
            let total = libbreenix::window::list(&mut infos).map_err(|e| format!("wait-window title={title:?} registry: {e}"))?;
            if total > infos.len() {
                infos.resize(total, libbreenix::window::abi::WinInfo::default());
                continue;
            }
            let registry: Vec<_> = infos[..total].iter().map(|w| {
                w.title.get(..w.title_len as usize).map(|title| (w.id, title)).ok_or("invalid registry title length")
            }).collect::<Result<_, _>>()?;
            let records = std::fs::read_to_string("/proc/breenix/composites")
                .map_err(|e| format!("wait-window title={title:?} composites: {e}"))?;
            if let Some(ready) = wait.observe(&registry, &records, Self::now_ns()?)? { return Ok(ready); }
            self.poll_once().map_err(|e| format!("wait-window title={title:?}: {e}"))?;
            if let Some(pid) = self.compositor {
                if let Some(exit) = self.exits.iter().find(|exit| exit.pid == pid) {
                    return Err(format!("wait-window title={title:?} compositor={} pid={pid} exited status={}", self.names[&pid], exit.status));
                }
            }
        }
    }
    fn now_ns() -> Result<u64, String> {
        time::now_monotonic().map(|t| t.as_nanos() as u64).map_err(|e| e.to_string())
    }
    /// Session-default environment, with the uid switch confined to the child.
    pub fn launch_app(&mut self, argv: &[&[u8]], uid: u32) -> Result<u64, String> {
        if self.stopped { return Err("session has ended".into()); }
        self.control.send(&Message::LaunchApp { argv: argv.iter().map(|s| s.to_vec()).collect(), uid })?;
        self.await_launch(argv)
    }
    pub fn stderr(&mut self, pid: u64) -> Result<AppStderr, String> {
        self.stderr = Some((pid, AppStderr::default(), false));
        self.control.send(&Message::ReadStderr(pid))?;
        let end = now()? + 1000;
        loop {
            if self.stderr.as_ref().is_some_and(|(_, _, done)| *done) {
                return Ok(self.stderr.take().unwrap().1);
            }
            self.poll(end)?;
        }
    }
    pub fn supervisor_pid(&self) -> u64 { self.supervisor }
    /// argv[0] is the executable path. env is the complete environment, with no
    /// implicit variables or shell expansion; each entry is KEY=value.
    pub fn launch(&mut self, argv: &[&[u8]], env: &[&[u8]]) -> Result<u64, String> {
        if self.stopped { return Err("session has ended".into()); }
        self.control.send(&Message::Launch { argv: argv.iter().map(|s| s.to_vec()).collect(), env: env.iter().map(|s| s.to_vec()).collect() })?;
        self.await_launch(argv)
    }
    fn await_launch(&mut self, argv: &[&[u8]]) -> Result<u64, String> {
        let end = now()? + 1000;
        loop {
            if let Some(pid) = self.launched.take() {
                self.names.insert(pid, String::from_utf8_lossy(argv[0]).into_owned());
                return Ok(pid);
            }
            self.poll(end)?;
        }
    }
    fn poll(&mut self, end: i128) -> Result<(), String> {
        if now()? >= end { return Err("session control deadline".into()); }
        self.poll_once()
    }
    fn poll_once(&mut self) -> Result<(), String> {
        match self.control.receive()? {
            Some(Message::Ready(pid)) if self.supervisor == 0 && pid > 1 => self.supervisor = pid,
            Some(Message::Devices(ids)) if self.devices.is_none() => self.devices = Some(ids),
            Some(Message::InputDone) if !self.input_done => self.input_done = true,
            Some(Message::Configured) if !self.configured => self.configured = true,
            Some(Message::Stderr { pid, dropped, bytes, done }) => {
                let (wanted, snapshot, complete) = self.stderr.as_mut().ok_or("unsolicited stderr")?;
                if pid != *wanted || *complete || snapshot.bytes.len() + bytes.len() > 64 * 1024 {
                    return Err("invalid stderr snapshot".into());
                }
                if !snapshot.bytes.is_empty() && snapshot.dropped != dropped { return Err("inconsistent stderr snapshot".into()); }
                snapshot.bytes.extend_from_slice(&bytes); snapshot.dropped = dropped; *complete = done;
            }
            Some(Message::Launched(pid)) if self.launched.is_none() && pid > 1 => self.launched = Some(pid),
            Some(Message::Exited { pid, status }) => self.exits.push(ProcessExit { pid, status }),
            Some(Message::Output { pid, bytes }) => {
                let output = self.output.entry(pid).or_default();
                if output.len() + bytes.len() > 64 * 1024 { return Err("session stdout exceeds limit".into()); }
                output.extend_from_slice(&bytes);
            }
            Some(Message::Stopped) => self.stopped = true,
            Some(Message::Error(error)) => return Err(error),
            Some(_) => return Err("unexpected supervisor reply".into()),
            None => time::sleep_ms(1).map_err(|e| e.to_string())?,
        }
        Ok(())
    }
    pub fn read_line(&mut self, pid: u64) -> Result<String, String> {
        let end = now()? + 2000;
        loop {
            if let Some(bytes) = self.output.get_mut(&pid) {
                if let Some(at) = bytes.iter().position(|&b| b == b'\n') {
                    let line = String::from_utf8(bytes[..at].to_vec()).map_err(|_| "app output is not UTF-8")?;
                    bytes.drain(..at+1); return Ok(line);
                }
            }
            self.poll(end)?;
        }
    }
    pub fn wait_exit(&mut self, pid: u64) -> Result<ProcessExit, String> {
        let end = now()? + 2000;
        loop {
            if let Some(exit) = self.exits.iter().find(|exit| exit.pid == pid) { return Ok(exit.clone()); }
            self.poll(end)?;
        }
    }
    /// A signaled app fails the scenario with its executable, pid and signal name.
    pub fn wait(&mut self, pid: u64) -> Result<ProcessExit, String> {
        let exit = self.wait_exit(pid)?;
        let name = self.names.get(&pid).map_or("adopted helper", String::as_str);
        if process::wifsignaled(exit.status) {
            let signal = process::wtermsig(exit.status);
            return Err(format!("app-crashed app={name} pid={pid} signal={}({signal})", signal_name(signal)));
        }
        if !process::wifexited(exit.status) || process::wexitstatus(exit.status) != 0 {
            return Err(format!("app-exited app={name} pid={pid} status={}", exit.status));
        }
        Ok(exit)
    }
    pub fn teardown(&mut self) -> Result<Vec<ProcessExit>, String> {
        if !self.stopped { self.control.send(&Message::Teardown)?; }
        let end = now()? + 1500;
        while !self.stopped { self.poll(end)?; }
        Ok(self.exits.clone())
    }
}
