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
    arena: Option<crate::arena::Arena>,
    artifacts: Option<crate::artifacts::Local>,
    startup: Option<crate::startup::Trace>,
    held_window: Option<u64>,
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
        session.start_compositor(kiosk)?;
        Ok(session)
    }
    /// Start after editing the freshly prepared private configuration.
    pub fn start_compositor(&mut self, kiosk: bool) -> Result<u64, String> {
        if self.compositor.is_some() || self.stopped { return Err("session compositor already started or ended".into()); }
        let session = self;
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
        } else {
            session.wait_compositor_clock()?;
        }
        Ok(pid)
    }
    /// Include another relative pointer in the compositor's restricted selection.
    /// Its injector remains owned by the case and releases on descriptor close.
    pub fn start_compositor_with_pointer(&mut self, id: u32) -> Result<u64,String> {
        if self.compositor.is_some() || self.stopped { return Err("session compositor already started or ended".into()); }
        self.control.send(&Message::StartCompositorWithPointer(id))?;
        let pid=self.await_launch(&[b"/usr/bin/bwm"])?;
        self.compositor=Some(pid);
        self.wait_compositor_clock()?;
        Ok(pid)
    }
    fn wait_compositor_clock(&mut self) -> Result<(), String> {
        use libbreenix::ui_clock::{ClockPage, COMPOSITOR, COMPOSITOR_ID};
        let page = ClockPage::open(&std::path::Path::new(self.config_root()).join("ui-clock"))?;
        let deadline = Self::now_ns()?.saturating_add(crate::clock::STEP_BOUND_NS);
        loop {
            if page.participants().iter().any(|p| p.id == COMPOSITOR_ID && p.flags & COMPOSITOR != 0) {
                return Ok(());
            }
            if Self::now_ns()? >= deadline { return Err("clock-registration-timeout compositor=bwm".into()); }
            self.poll_once()?;
        }
    }
    pub fn connect() -> Result<Self, String> { Self::connect_with_seed(0) }
    pub fn connect_with_seed(seed: u64) -> Result<Self, String> {
        let began = crate::startup::now();
        let fd = breenix_suite::suite::take_session_control().ok_or("case did not declare a session")?;
        let control = Channel::new(fd)?;
        let case_pid = process::getpid().map_err(|e| e.to_string())?.raw();
        let mode = libbreenix::gpu::current_mode().map_err(|e| format!("session mode: {e}"))?;
        let mode_at = crate::startup::now();
        let mode = format!("{}x{}@{}", mode.width, mode.height, mode.scale);
        let environment = SessionEnvironment::new(case_pid, seed, &mode)?;
        let mut session = Self { arena: None, artifacts: None, startup: None, held_window: None, environment, configured: false, devices: None, input_done: false, stderr: None, control, supervisor: 0, names: BTreeMap::new(), output: BTreeMap::new(), exits: Vec::new(), launched: None, stopped: false, compositor: None, excluded: BTreeSet::new() };
        let end = now()? + 1000;
        while session.supervisor == 0 || session.devices.is_none() { session.poll(end)?; }
        let ready_at = crate::startup::now();
        session.control.send(&Message::Configure { case_pid, seed, mode })?;
        let end = now()? + 1000;
        while !session.configured { session.poll(end)?; }
        session.startup = Some(crate::startup::Trace::open(std::path::Path::new(session.config_root()))?);
        let trace = session.startup.as_ref().unwrap();
        trace.at("connect-begin", began); trace.at("case-mode-ready", mode_at); trace.at("case-ready", ready_at);
        trace.mark("case-connected");
        session.arena = Some(crate::arena::Arena::open(&std::path::Path::new(session.config_root()).join("evidence.arena"))?);
        Ok(session)
    }
    pub fn startup_times(&self) -> Vec<(&'static str, u64)> {
        self.startup.as_ref().map_or_else(Vec::new, |trace| trace.snapshot())
    }
    pub fn arena_mut(&mut self) -> &mut crate::arena::Arena { self.arena.as_mut().expect("connected evidence arena") }
    pub fn next_checkpoint(&mut self) -> Result<(), String> { self.arena_mut().next_checkpoint() }
    /// Compare the arena capture and retain local evidence before its FRAME record.
    /// Window/surface capture must already have published the check's frame slot.
    pub fn check_stored(&mut self, suite: &str, definition: &crate::schema::Value,
        bindings: &crate::frames::Bindings, references: &std::path::Path, barriers: &[&str],
        client: &mut crate::RecordClient<'_>) -> Result<crate::compare::Verdict, String> {
        use crate::arena::{Slot, SlotRead};
        let check = definition["check"].as_str().ok_or("missing check name")?;
        let case = definition["case"].as_str().ok_or("missing case name")?;
        let arena = self.arena.as_mut().ok_or("session arena not connected")?;
        let (record, bytes) = match arena.read(Slot::Frame)? {
            SlotRead::Final {record,bytes} if record.check == check && !record.oversize => (record,bytes),
            _ => return Err("check frame is not finalized in arena".into()),
        };
        if matches!(arena.read(Slot::Screen)?, SlotRead::Free) {
            let _ = crate::capture::screen(arena, check, libbreenix::window::ObservationKind::Composed);
        }
        let input_log = std::fs::read("/proc/breenix/input-ledger");
        let source = match definition["source"]["kind"].as_str() {
            Some("window") => format!("window:{}", definition["source"]["window"].as_str().ok_or("missing window key")?),
            Some(kind) => kind.to_string(),
            None => return Err("missing source kind".into()),
        };
        let frame = crate::compare::Frame::new(&bytes, record.width as usize, record.height as usize,
            record.width as usize*4, crate::compare::Format::Bgrx).map_err(|e| format!("check frame: {e:?}"))?;
        use crate::artifacts::CheckWriter;
        let destination = std::path::Path::new("/var/uitest").join(suite).join(case);
        let writer = self.artifacts.get_or_insert_with(|| crate::artifacts::Local::new(std::path::Path::new("/var/uitest"),suite,case));
        if writer.path() != destination { return Err("session artifact case changed".into()); }
        // An unreadable input ledger is a save error, not a comparison error.
        struct Unavailable(std::path::PathBuf);
        impl crate::artifacts::CheckWriter for Unavailable {
            fn path(&self) -> &std::path::Path { &self.0 }
            fn write(&mut self, _: &str, _: &[u8]) -> Result<(), crate::artifacts::SaveError> { Err(crate::artifacts::SaveError::Io) }
        }
        let mut unavailable = Unavailable(std::path::Path::new("/var/uitest").join(suite).join(case));
        let mut artifacts = crate::frames::ArtifactContext {
            arena, input_log:input_log.as_deref().unwrap_or(&[]),
            writer:if input_log.is_ok() { writer } else { &mut unavailable },
            save_all:crate::artifacts::save_all(),
        };
        let verdict = crate::frames::check_stored_with_artifacts(suite,definition,
            crate::frames::Capture {frame,source:&source,
                checkpoint:definition["at"]["checkpoint"].as_str().ok_or("missing checkpoint")?,
                commit:record.commit,epoch:record.epoch,barriers},bindings,references,client,&mut artifacts)?;
        #[cfg(target_os = "breenix")]
        if !verdict.comparison.passed() && client.is_outer() {
            let _ = crate::retention::retain(suite, case);
        }
        Ok(verdict)
    }
    /// Wait for the supervisor to witness the live window and register its remnant hold.
    pub fn hold_window(&mut self, id: u64) -> Result<(), String> {
        self.held_window = None;
        self.control.send(&Message::HoldWindow(id))?;
        let end = now()? + 1000;
        while self.held_window != Some(id) { self.poll(end)?; }
        Ok(())
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
        if self.stopped {return Err("session has ended".into());}
        self.input_done=false;
        self.control.send(&Message::Motion {x,y})?;
        let end=now()?+2000;
        while !self.input_done {self.poll(end)?;}
        Ok(())
    }
    pub fn raw_text(&mut self, text: &str) -> Result<(), String> {
        if self.stopped { return Err("session has ended".into()); }
        self.input_done = false;
        self.control.send(&Message::Text(text.into()))?;
        let end = now()? + 2000;
        while !self.input_done { self.poll(end)?; }
        Ok(())
    }
    pub fn raw_wheel(&mut self, delta: i32) -> Result<(), String> { self.raw_input(RawInput { kind: 2, code: 8, value: delta }) }
    pub fn config_root(&self) -> &str { self.environment.config_root() }
    /// Exclude a window from required clock/settle participation (plan 6.3).
    pub fn exclude(&mut self, window: u64) { self.excluded.insert(window); }
    pub fn is_excluded(&self, window: u64) -> bool { self.excluded.contains(&window) }

    /// Advance UI milliseconds and wait for kernel-accepted participant metadata.
    pub fn step(&mut self, dt_ms: u64) -> Result<libbreenix::ui_clock::Stamp, String> {
        if self.stopped { return Err("session has ended".into()); }
        let clock = crate::clock::Clock::open(&std::path::Path::new(self.config_root()).join("ui-clock"))?;
        let mut step = clock.begin(dt_ms, Self::now_ns()?, |id| self.is_excluded(id))?;
        loop {
            step.check(Self::now_ns()?)?;
            for id in step.waiting().to_vec() {
                if id == libbreenix::ui_clock::COMPOSITOR_ID {
                    let mut status = libbreenix::window::abi::CompositorStatus::default();
                    if libbreenix::window::call(libbreenix::window::abi::op::COMPOSITOR_STATUS, &mut status).is_ok()
                        && self.compositor == Some(u64::from(status.owner)) {
                        step.check(Self::now_ns()?)?;
                        step.observe_compositor(&status);
                    }
                    continue;
                }
                if let Ok((_, meta)) = libbreenix::window::read_meta(id) {
                    step.check(Self::now_ns()?)?;
                    step.observe(id, meta.epoch);
                }
            }
            // A metadata read finishing beyond the real bound cannot rescue a timeout.
            step.check(Self::now_ns()?)?;
            if step.complete() { return Ok(step.stamp()); }
            self.poll_once()?;
        }
    }

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
            if let Some(ready) = wait.observe(&registry, &records, Self::now_ns()?)? {
                if let Some(trace) = &self.startup { trace.at("first-composite", ready.presented_ns); }
                return Ok(ready);
            }
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
            Some(Message::WindowHeld(id)) if self.held_window.is_none() => self.held_window = Some(id),
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
