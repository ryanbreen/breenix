//! Case-side client for the runner's sibling session supervisor.
use std::collections::BTreeMap;
use libbreenix::{process, time};
use crate::{channel::{Channel, now}, supervisor::{Message, SessionEnvironment, signal_name}};

#[derive(Clone, Debug)]
pub struct ProcessExit { pub pid: u64, pub status: i32 }

#[derive(Clone, Debug, Default)]
pub struct AppStderr { pub bytes: Vec<u8>, pub dropped: u64 }

pub struct Session {
    environment: SessionEnvironment,
    configured: bool,
    stderr: Option<(u64, AppStderr, bool)>,
    control: Channel,
    supervisor: u64,
    names: BTreeMap<u64, String>,
    output: BTreeMap<u64, Vec<u8>>,
    exits: Vec<ProcessExit>,
    launched: Option<u64>,
    stopped: bool,
}
impl Session {
    pub fn connect() -> Result<Self, String> { Self::connect_with_seed(0) }
    pub fn connect_with_seed(seed: u64) -> Result<Self, String> {
        let fd = breenix_suite::suite::take_session_control().ok_or("case did not declare a session")?;
        let control = Channel::new(fd)?;
        let case_pid = process::getpid().map_err(|e| e.to_string())?.raw();
        let mode = libbreenix::gpu::current_mode().map_err(|e| format!("session mode: {e}"))?;
        let mode = format!("{}x{}@{}", mode.width, mode.height, mode.scale);
        let environment = SessionEnvironment::new(case_pid, seed, &mode)?;
        let mut session = Self { environment, configured: false, stderr: None, control, supervisor: 0, names: BTreeMap::new(), output: BTreeMap::new(), exits: Vec::new(), launched: None, stopped: false };
        let end = now()? + 1000;
        while session.supervisor == 0 { session.poll(end)?; }
        session.control.send(&Message::Configure { case_pid, seed, mode })?;
        let end = now()? + 1000;
        while !session.configured { session.poll(end)?; }
        Ok(session)
    }
    pub fn config_root(&self) -> &str { self.environment.config_root() }
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
        match self.control.receive()? {
            Some(Message::Ready(pid)) if self.supervisor == 0 && pid > 1 => self.supervisor = pid,
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
