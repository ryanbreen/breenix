//! The case controls processes and observes scanout; bwm alone places, routes and composes.
use super::protocol::*;
use breenix_suite::suite::{case_ms_left, check, spawn, CaseResult, Run};
use libbreenix::types::{Fd, Pid};
use libbreenix::{gpu, graphics, io, process, signal, time, window};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use window::abi::{self, op, Plain};

pub fn now() -> u64 {
    time::now_monotonic().map_or(0, |v| v.as_nanos() as u64)
}
pub fn pause() {
    let _ = time::sleep_ms(5);
}
pub fn until(
    ms: u64,
    what: &str,
    test: impl FnMut() -> Result<bool, String>,
) -> Result<(), String> {
    until_from(now(), ms, what, test)
}
pub fn until_from(
    begin: u64,
    ms: u64,
    what: &str,
    mut test: impl FnMut() -> Result<bool, String>,
) -> Result<(), String> {
    let _stall = StallWait::new(begin, ms, what);
    let end = begin + ms * 1_000_000;
    loop {
        if test()? {
            return Ok(());
        }
        if now() >= end || case_ms_left() < 200 {
            return Err(format!("{what}: no acknowledgment within {ms} ms"));
        }
        pause();
    }
}

pub struct Child {
    pub pid: i32,
    reaped: bool,
    pub reap_ns: u64,
}
impl Child {
    pub fn signal(&self, sig: i32) -> Result<(), String> {
        signal::kill(self.pid, sig).map_err(|e| e.to_string())
    }
    pub fn freeze(&self) -> Result<(), String> {
        self.signal(signal::SIGSTOP)?;
        until(
            1000,
            "owned client stopped before retained-frame observation",
            || {
                let mut status = 0;
                let pid =
                    process::waitpid(self.pid, &mut status, process::WNOHANG | process::WUNTRACED)
                        .map_err(|e| e.to_string())?;
                if pid.raw() == 0 {
                    return Ok(false);
                }
                if status & 0xff != 0x7f || (status >> 8) & 0xff != signal::SIGSTOP {
                    return Err(format!("client did not stop: wait status {status}"));
                }
                Ok(true)
            },
        )
    }
    pub fn status(&mut self) -> Result<Option<i32>, String> {
        if self.reaped {
            return Err("child already reaped".into());
        }
        let mut status = 0;
        let p =
            process::waitpid(self.pid, &mut status, process::WNOHANG).map_err(|e| e.to_string())?;
        if p.raw() == 0 {
            Ok(None)
        } else {
            self.reaped = true;
            Ok(Some(status))
        }
    }
    pub fn stop(&mut self) -> Result<i32, String> {
        let requested_ns = now();
        self.signal(signal::SIGKILL)?;
        let returned_ns = now();
        let mut result = None;
        let waited = until(500, "reap owned process", || {
            result = self.status()?;
            Ok(result.is_some())
        });
        let wait_returned_ns = now();
        self.reap_ns = wait_returned_ns.saturating_sub(returned_ns);
        report_reap(self.pid, requested_ns, returned_ns, wait_returned_ns, waited.is_ok());
        waited?;
        let status = result.unwrap();
        if status & 127 != signal::SIGKILL {
            return Err(format!(
                "owned process did not die from requested SIGKILL: wait status {status}"
            ));
        }
        Ok(status)
    }
}
// Called after the wait on both outcomes; no output occurs in the polling loop.
fn report_reap(pid: i32, requested_ns: u64, returned_ns: u64, wait_ns: u64, passed: bool) {
    failure_diagnostic("reap", &format!(
        "pid={pid} requested_ns={requested_ns} kill_returned_ns={returned_ns}"
    ));
    if let Ok(pipeline) = std::fs::read_to_string("/proc/breenix/window-pipeline-full") {
        let rows: Vec<Vec<u64>> = pipeline.lines().filter_map(|line| {
            let row: Vec<u64> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            (row.len() == 5 && (17..=22).contains(&row[0])).then_some(row)
        }).collect();
        // Stage 20 also maps older records whose source column predates group stamps.
        let tids: Vec<_> = rows.iter().filter(|r| r[3] == pid as u64
            || (r[0] == 20 && r[2] == pid as u64)).map(|r| r[1]).collect();
        for row in rows.iter().filter(|r| tids.contains(&r[1])) {
            failure_diagnostic("exit-phase", &format!(
                "tgid={pid} tid={} stage={} pid={} ns={}", row[1], row[0], row[2], row[4]
            ));
        }
    }
    failure_diagnostic("reap-wait", &format!(
        "pid={pid} wait_returned_ns={wait_ns} elapsed_ns={} passed={passed}", wait_ns.saturating_sub(returned_ns)
    ));
}
impl From<Pid> for Child {
    fn from(pid: Pid) -> Self {
        Self {
            pid: pid.raw() as i32,
            reaped: false,
            reap_ns: 0,
        }
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = signal::kill(self.pid, signal::SIGKILL);
            let mut status = 0;
            let _ = process::waitpid(self.pid, &mut status, 0);
        }
    }
}

pub struct Client {
    pub child: Child,
    commands: File,
    replies: File,
    pub id: u64,
    pub commit: u64,
    pub commit_ns: u64,
    pub handling_commit: u64,
    pub width: u32,
    pub height: u32,
    pub seed: u32,
    pub events: Vec<abi::WindowEvent>,
}
fn read_n(file: &mut File, bytes: &mut [u8]) -> Result<(), String> {
    let began = now();
    let _stall = StallWait::new(began, 2000, "client-reply-startup");
    let end = began + 2_000_000_000;
    let mut done = 0;
    while done < bytes.len() {
        let at = now();
        let left = case_ms_left();
        if at >= end || left < 200 {
            let cause = if left < 200 { "case deadline" } else { "reply deadline" };
            return Err(format!(
                "wm-client {cause} waited_ms={} case_left_ms={left} bytes={done}/{}",
                (at - began) / 1_000_000, bytes.len(),
            ));
        }
        let mut fds = [io::PollFd {
            fd: file.as_raw_fd(),
            events: 1,
            revents: 0,
        }];
        io::poll(&mut fds, 10).map_err(|e| e.to_string())?;
        if fds[0].revents == 0 {
            continue;
        }
        match file.read(&mut bytes[done..]) {
            Ok(0) => {
                return Err(
                    "wm-client closed its reply pipe; its operation failed or it crashed".into(),
                )
            }
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("wm-client reply: {e}")),
        }
    }
    Ok(())
}
impl Client {
    pub fn start(seed: u32) -> Result<Self, String> {
        Self::start_mode(seed, 0)
    }
    pub fn unregistered(seed: u32) -> Result<Self, String> {
        Self::start_mode(seed, 2)
    }
    fn start_mode(seed: u32, flags: u32) -> Result<Self, String> {
        let (cr, cw) = io::pipe().map_err(|e| e.to_string())?;
        let (rr, rw) = io::pipe().map_err(|e| e.to_string())?;
        let pid = spawn(
            &[(cr, Fd::from_raw(3)), (rw, Fd::from_raw(4))],
            Run::Exec(
                b"/usr/libexec/breenix/wm-client_test\0",
                &[b"wm-client_test\0"],
            ),
        );
        let _ = (io::close(cr), io::close(rw));
        let pid = match pid {
            Ok(p) => p,
            Err(e) => {
                let _ = (io::close(cw), io::close(rr));
                return Err(e.to_string());
            }
        };
        // SAFETY: these pipe descriptors have one owner in this process.
        let (commands, replies) = unsafe {
            (
                File::from_raw_fd(cw.raw() as i32),
                File::from_raw_fd(rr.raw() as i32),
            )
        };
        let mut client = Self {
            child: pid.into(),
            commands,
            replies,
            id: 0,
            commit: 0,
            commit_ns: 0,
            handling_commit: 0,
            width: 0,
            height: 0,
            seed,
            events: Vec::new(),
        };
        client.command(SNAPSHOT, seed, flags)?;
        Ok(client)
    }
    pub fn send(&mut self, op: u32, a: u32, b: u32) -> Result<(), String> {
        let mut command = [0; COMMAND_SIZE];
        for (offset, v) in [(0, op), (4, a), (8, b)] {
            command[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.commands
            .write_all(&command)
            .map_err(|e| format!("wm-client command: {e}"))
    }
    pub fn command(&mut self, op: u32, a: u32, b: u32) -> Result<(), String> {
        self.send(op, a, b)?;
        let mut h = [0; RESPONSE_SIZE];
        read_n(&mut self.replies, &mut h)?;
        if quad(&h, 0) == u64::MAX {
            let n = word(&h, 16) as usize;
            if n > 1024 {
                return Err("oversize client error".into());
            }
            let mut text = vec![0; n];
            read_n(&mut self.replies, &mut text)?;
            return Err(format!("wm-client: {}", String::from_utf8_lossy(&text)));
        }
        self.id = quad(&h, 0);
        self.commit = quad(&h, 8);
        self.commit_ns = quad(&h, 32);
        self.handling_commit = quad(&h, 40);
        self.width = word(&h, 16);
        self.height = word(&h, 20);
        self.seed = word(&h, 24);
        let n = word(&h, 28) as usize;
        if n > 8192 {
            return Err("client reply event count exceeds its bound".into());
        }
        self.events = vec![abi::WindowEvent::ZERO; n];
        // SAFETY: WindowEvent is plain data, initialized before the read, and exactly 320 bytes.
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(self.events.as_mut_ptr().cast::<u8>(), n * 320)
        };
        read_n(&mut self.replies, bytes)
    }
    pub fn snapshot(&mut self) -> Result<(), String> {
        self.command(SNAPSHOT, 0, 0)
    }
    pub fn clear(&mut self) -> Result<(), String> {
        self.command(CLEAR_EVENTS, 0, 0)
    }
    /// Commit the same pixels after observing bwm's published registry change.
    pub fn barrier(&self) -> Result<u64, String> {
        let mut commands = self.commands.try_clone().map_err(|e| e.to_string())?;
        let mut replies = self.replies.try_clone().map_err(|e| e.to_string())?;
        let mut bytes = [0; COMMAND_SIZE];
        bytes[..4].copy_from_slice(&PAINT.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.seed.to_le_bytes());
        commands.write_all(&bytes).map_err(|e| e.to_string())?;
        let mut h = [0; RESPONSE_SIZE];
        read_n(&mut replies, &mut h)?;
        if quad(&h, 0) != self.id {
            return Err("client barrier failed or named another window".into());
        }
        let count = word(&h, 28) as usize;
        if count > 8192 {
            return Err("barrier event reply exceeds bound".into());
        }
        let mut events = vec![0; count * 320];
        read_n(&mut replies, &mut events)?;
        Ok(quad(&h, 8))
    }
    pub fn clipboard(&mut self, set: bool, text: &[u8]) -> Result<(i32, u64, Vec<u8>), String> {
        self.send(if set { CLIP_SET } else { CLIP_GET }, text.len() as u32, 0)?;
        self.commands
            .write_all(text)
            .map_err(|e| format!("clipboard client command: {e}"))?;
        let mut header = [0; 16];
        read_n(&mut self.replies, &mut header)?;
        let status = i32::from_le_bytes(header[..4].try_into().unwrap());
        let n = word(&header, 4) as usize;
        if n > 16 * 1024 * 1024 {
            return Err("clipboard reply exceeds 16 MiB".into());
        }
        let mut text = vec![0; n];
        read_n(&mut self.replies, &mut text)?;
        Ok((status, quad(&header, 8), text))
    }
    pub fn info(&self) -> Result<abi::WinInfo, String> {
        listed()?
            .into_iter()
            .find(|w| w.id == self.id)
            .ok_or_else(|| format!("client window {} is absent from LIST", self.id))
    }
    pub fn await_event(
        &mut self,
        description: &str,
        test: impl Fn(&abi::WindowEvent) -> bool,
    ) -> Result<abi::WindowEvent, String> {
        let mut found = None;
        until(1000, description, || {
            self.snapshot()?;
            found = self.events.iter().find(|e| test(e)).copied();
            Ok(found.is_some())
        })
        .map_err(|e| format!("{description}: {e}"))?;
        let event = found.unwrap();
        if event.kind == abi::event_kind::POINTER_BUTTON {
            stall_command(&format!("click-observed {} {}", event.serial, self.id));
        }
        Ok(event)
    }
}

pub fn listed() -> Result<Vec<abi::WinInfo>, String> {
    let mut out = vec![abi::WinInfo::ZERO; 256];
    let n = window::list(&mut out).map_err(|e| format!("LIST: {e}"))?;
    if n > out.len() {
        return Err("registry exceeds observation capacity".into());
    }
    out.truncate(n);
    Ok(out)
}
pub fn quad(b: &[u8], p: usize) -> u64 {
    u64::from_le_bytes(b[p..p + 8].try_into().unwrap())
}

pub struct Screen {
    pub pixels: Vec<u8>,
    pub record: [u8; 1024],
    pub width: u32,
    pub height: u32,
}
impl Screen {
    pub fn read() -> Result<Self, String> {
        let mode = gpu::current_mode().map_err(|e| format!("CURRENT_MODE: {e}"))?;
        let mut s = Self {
            pixels: vec![0; (mode.width * mode.height * 4) as usize],
            record: [0; 1024],
            width: mode.width,
            height: mode.height,
        };
        let mut r = abi::Readback::ZERO;
        r.dst = s.pixels.as_mut_ptr() as u64;
        r.dst_len = s.pixels.len() as u64;
        r.record = s.record.as_mut_ptr() as u64;
        r.record_len = 1024;
        window::call(op::SCANOUT_READBACK, &mut r)
            .map_err(|e| format!("SCANOUT_READBACK: {e}, reason {}", r.reason))?;
        if !window::validate_scanout_observation(
            &s.record, s.width, s.height, window::ObservationKind::Composed,
        ) {
            return Err(format!("invalid scanout: backend={} flags={} context={} serial={} size={}x{} render={:x}/{:x} read={:x}/{:x} present={:x}",
                word(&s.record, 16), word(&s.record, 24), quad(&s.record, 64), s.serial(),
                word(&s.record, 40), word(&s.record, 44), word(&s.record, 112), word(&s.record, 116),
                word(&s.record, 128), word(&s.record, 132), word(&s.record, 192)));
        }
        Ok(s)
    }
    pub fn serial(&self) -> u64 {
        quad(&self.record, 176)
    }
    pub fn contains(&self, id: u64, commit: u64) -> bool {
        let n = word(&self.record, 196) as usize;
        n <= 16
            && (0..n).any(|i| {
                quad(&self.record, 200 + i * 32) == id && quad(&self.record, 208 + i * 32) >= commit
            })
    }
    pub fn pixel(&self, x: i32, y: i32) -> Result<u32, String> {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return Err(format!("screen coordinate outside mode: {x},{y}"));
        }
        let at = ((y as u32 * self.width + x as u32) * 4) as usize;
        Ok(word(&self.pixels, at) | 0xff00_0000)
    }
    pub fn client(&self, c: &Client) -> CaseResult {
        let w = c.info()?;
        check(
            self.contains(c.id, c.commit),
            &format!(
                "device observation does not list client {} commit {}",
                c.id, c.commit
            ),
        )?;
        for y in 0..c.height {
            for x in 0..c.width {
                let q = (if x >= c.width / 2 { 1 } else { 0 })
                    + (if y >= c.height / 2 { 2 } else { 0 });
                let expected = 0xff00_0000
                    | (((c.seed * 37 + q * 61) % 256) << 16)
                    | (((c.seed * 71 + q * 43) % 256) << 8)
                    | ((c.seed * 17 + q * 97) % 256);
                let got = self.pixel(w.x + x as i32, w.y + y as i32)?;
                if got != expected {
                    return Err(format!("bwm client {} at local {x},{y}: scanout {got:08x}, expected {expected:08x}", c.id).into());
                }
            }
        }
        Ok(())
    }
}

struct StartupTrace(std::path::PathBuf);
impl StartupTrace {
    fn new() -> Result<Self, String> {
        let path = std::path::PathBuf::from(format!("/tmp/wm-startup-{}", libuitest::startup::now()));
        std::fs::create_dir(&path).map_err(|e| format!("startup trace directory: {e}"))?;
        Ok(Self(path))
    }
    fn failure(&self) {
        let Ok(entries) = std::fs::read_dir(&self.0) else { return };
        for entry in entries.flatten() {
            let Ok(trace) = libuitest::startup::Trace::open(&entry.path()) else { continue };
            for event in trace.details() {
                failure_diagnostic("startup-detail", &format!("pid={} {event}", entry.file_name().to_string_lossy()));
            }
            let phases = trace.snapshot();
            let begin = phases.iter().filter(|(_, ns)| *ns != 0).map(|(_, ns)| *ns).min().unwrap_or(0);
            for (phase, ns) in phases.into_iter().filter(|(_, ns)| *ns != 0) {
                failure_diagnostic("startup", &format!("pid={} phase={phase} ns={ns} elapsed_us={}", entry.file_name().to_string_lossy(), ns.saturating_sub(begin) / 1000));
            }
        }
    }
}
impl Drop for StartupTrace {
    fn drop(&mut self) { self.failure(); let _ = std::fs::remove_dir_all(&self.0); }
}

pub struct Desktop {
    pub wm: Child,
    log: File,
    pub width: u32,
    pub height: u32,
    pub scale: i32,
    startup: StartupTrace,
}

// Retain the last observation without printing or formatting on a successful poll.
// A failed wait reports both readback refusal and the last completed binding.
#[derive(Default)]
struct WaitObservation {
    begin_ns: u64,
    end_ns: u64,
    error: Option<String>,
    record: Option<[u8; 1024]>,
    pixel: Option<u32>,
}

/// Failure records bypass the case verdict's short message field.
pub fn failure_diagnostic(label: &str, text: &str) {
    use std::io::Write;
    let case = breenix_suite::suite::running_case().unwrap_or("window-manager");
    breenix_suite::suite::case_budget("diagnostic-begin", case);
    if let Ok(mut evidence) = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence") {
        for line in text.lines() {
            let chars: Vec<_> = line.chars().collect();
            for chunk in chars.chunks(160) {
                let chunk: String = chunk.iter().collect();
                let _ = writeln!(evidence, "VALUE wm-{label} {chunk}");
            }
        }
    }
    breenix_suite::suite::case_budget("diagnostic-end", case);
}
impl Drop for WaitObservation {
    fn drop(&mut self) { self.export(); }
}
impl WaitObservation {
    fn export(&self) {
        if self.begin_ns == 0 { return; }
        failure_diagnostic("observer", &format!("begin_ns={} end_ns={} completed={}",
            self.begin_ns, self.end_ns, self.record.is_some()));
        if let Ok(trace) = std::fs::read_to_string("/proc/breenix/window-readback") {
            for line in trace.lines() {
                let begin = line.split_whitespace().find_map(|f| f.strip_prefix("begin_ns=")?.parse::<u64>().ok());
                if line.starts_with("retained=") {
                    failure_diagnostic("readback-buffer", line);
                }
                if begin.is_some_and(|ns| ns >= self.begin_ns && ns <= self.end_ns) {
                    failure_diagnostic("readback", line);
                }
            }
        }
        // Completed render/present/receipt rows have the same monotonic clock.
        if let Ok(trace) = std::fs::read_to_string("/proc/breenix/window-pipeline-full") {
            for line in trace.lines() {
                let row: Vec<u64> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                if row.len() == 5 && (10..=13).contains(&row[0]) && row[4] >= self.begin_ns && row[4] <= self.end_ns {
                    failure_diagnostic("completion", line);
                }
            }
        }
    }

    fn read(&mut self) -> Result<Screen, String> {
        if self.begin_ns == 0 { self.begin_ns = now(); }
        let result = Screen::read();
        self.end_ns = now();
        match result {
            Ok(s) => {
                self.record = Some(s.record);
                self.pixel = s.pixel(s.width as i32 / 2, 1).ok();
                Ok(s)
            }
            Err(e) => {
                self.error = Some(e.clone());
                Err(e)
            }
        }
    }
    fn failure(&self) -> String {
        let binding = self.record.as_ref().map(|r| {
            let clients: Vec<_> = (0..(word(r, 196) as usize).min(16))
                .map(|i| (quad(r, 200 + i * 32), quad(r, 208 + i * 32)))
                .collect();
            format!("context={} serial={} clients={clients:?} pixel={:?}",
                quad(r, 64), quad(r, 176), self.pixel)
        });
        format!("last readback error={:?}; last completed={binding:?}", self.error)
    }
}
impl Desktop {
    pub fn report_startup(&self) {
        self.startup.failure();
    }
    pub fn cpu_costs(&self) -> Result<[u64; 5], String> {
        Ok(libuitest::startup::Trace::open(&self.startup.0.join(self.wm.pid.to_string()))?.cpu_snapshot())
    }
    pub fn start() -> Result<Self, String> {
        Self::launch(false)
    }
    pub fn launch(supervised: bool) -> Result<Self, String> {
        let previous_context = Screen::read().map_or(0, |s| quad(&s.record, 64));
        graphics::take_over_display().map_err(|e| format!("session display: {e}"))?;
        let startup = StartupTrace::new()?;
        let (r, w) = io::pipe2(io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
        let previous_trace = std::env::var_os("BREENIX_STARTUP_TRACE_DIR");
        std::env::set_var("BREENIX_STARTUP_TRACE_DIR", &startup.0);
        let pid = if supervised {
            spawn(
                &[(w, Fd::STDOUT), (w, Fd::STDERR)],
                Run::Exec(b"/sbin/init\0", &[b"init\0", b"desktop\0"]),
            )
        } else {
            spawn(
                &[(w, Fd::STDOUT), (w, Fd::STDERR)],
                Run::Exec(b"/usr/bin/bwm\0", &[b"bwm\0"]),
            )
        };
        match previous_trace {
            Some(value) => std::env::set_var("BREENIX_STARTUP_TRACE_DIR", value),
            None => std::env::remove_var("BREENIX_STARTUP_TRACE_DIR"),
        }
        let _ = io::close(w);
        let pid = match pid {
            Ok(p) => p,
            Err(e) => {
                let _ = io::close(r);
                return Err(e.to_string());
            }
        };
        let mode = gpu::current_mode().map_err(|e| format!("display mode: {e}"))?;
        // SAFETY: the log's read end is owned only by this File.
        let log = unsafe { File::from_raw_fd(r.raw() as i32) };
        let mut d = Self {
            wm: pid.into(),
            log,
            width: mode.width,
            height: mode.height,
            scale: mode.scale as i32,
            startup,
        };
        let mut observation = WaitObservation::default();
        let ready = until(4000, "bwm first completed desktop", || {
            if let Some(status) = d.wm.status()? {
                return Err(format!(
                    "desktop process exited with wait status {status}; {}",
                    d.output()
                ));
            }
            Ok(observation.read().is_ok_and(|s| {
                s.serial() != 0
                    && quad(&s.record, 64) != previous_context
                    && s.pixel(d.width as i32 / 2, 1).ok() == Some(0xff14161e)
            }))
        });
        let ready_end_ns = now();
        failure_diagnostic("observer-wait", &format!("kind=desktop begin_ns={} end_ns={ready_end_ns} passed={}", observation.begin_ns, ready.is_ok()));
        ready.map_err(|e| {
            d.startup.failure();
            let detail = format!("previous context={previous_context}; {}; {}", observation.failure(), d.output());
            failure_diagnostic("ready", &format!("{e}; {detail}"));
            format!("{e}; {detail}")
        })?;
        if supervised {
            let mut observation = WaitObservation::default();
            let mut observed_windows = Vec::new();
            let begin_ns = now();
            let initial = until(
                2000,
                "init desktop's initial client reached scanout",
                || {
                    let windows = listed()?;
                    observed_windows = windows.iter().map(|w| (w.id, w.commit_id)).collect();
                    let s = match observation.read() {
                        Ok(s) => s,
                        Err(_) => return Ok(false),
                    };
                    Ok(windows
                        .iter()
                        .any(|w| w.commit_id != 0 && s.contains(w.id, w.commit_id)))
                },
            );
            let end_ns = now();
            failure_diagnostic("observer-wait", &format!("kind=initial-client begin_ns={begin_ns} end_ns={end_ns} passed={}", initial.is_ok()));
            initial.map_err(|e| {
                let detail = format!("windows={observed_windows:?}; {}; {}", observation.failure(), d.output());
                failure_diagnostic("initial-client", &detail);
                format!("{e}; {detail}")
            })?;
        }
        Ok(d)
    }
    pub fn output(&mut self) -> String {
        let mut out = Vec::new();
        let mut b = [0; 4096];
        while out.len() < 65536 {
            match self.log.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&b[..n]),
            }
        }
        String::from_utf8_lossy(&out).replace(['\n', '\r'], " ")
    }
    pub fn shown(&mut self, c: &Client) -> Result<Screen, String> {
        self.shown_after(c, c.commit)
    }
    pub fn shown_after(&mut self, c: &Client, commit: u64) -> Result<Screen, String> {
        let mut result = None;
        let mut observation = WaitObservation::default();
        let begin_ns = now();
        let observed = until(
            2000,
            &format!("bwm composite of client {} commit {}", c.id, commit),
            || {
                if let Some(status) = self.wm.status()? {
                    return Err(format!(
                        "desktop died, wait status {status}; {}",
                        self.output()
                    ));
                }
                match observation.read() {
                    Ok(s) if s.contains(c.id, commit) => {
                        result = Some(s);
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            },
        );
        let end_ns = now();
        failure_diagnostic("observer-wait", &format!("kind=client id={} commit={commit} begin_ns={begin_ns} end_ns={end_ns} passed={}", c.id, observed.is_ok()));
        observed.map_err(|e| {
            let detail = format!("{}; {}", observation.failure(), self.output());
            if let Ok(counters) = std::fs::read_to_string("/proc/trace/counters") {
                for line in counters.lines().filter(|line| line.contains("gpu_wait_") || line.contains("gpu_read_") || line.contains("gpu_irq_masked") || line.contains("GPU_MASKED")) {
                    failure_diagnostic("kernel-readback", line);
                }
            }
            self.startup.failure();
            failure_diagnostic("shown", &format!("{e}; {detail}"));
            format!("{e}; {detail}")
        })?;
        Ok(result.unwrap())
    }
    pub fn pointer(&self, x: i32, y: i32, buttons: u32, wheel: i32) -> Result<(), String> {
        check(
            x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32,
            "pointer outside screen",
        )
        .map_err(|e| format!("{e:?}"))?;
        breenix_suite::suite::hostpointer(
            x as u32,
            y as u32,
            self.width,
            self.height,
            buttons,
            wheel,
        )
        .map(|seq| { stall_command(&format!("click-request {seq} {buttons}")); })
    }
    pub fn click(&self, x: i32, y: i32) -> Result<(), String> {
        self.pointer(x, y, 1, 0)?;
        time::sleep_ms(100).map_err(|e| e.to_string())?;
        self.pointer(x, y, 0, 0)?;
        time::sleep_ms(100).map_err(|e| e.to_string())
    }
    pub fn focus(&mut self, c: &mut Client) -> Result<(), String> {
        until(2000, "bwm assigned client geometry", || {
            let w = c.info()?;
            Ok(w.x > 0 && w.y > 0)
        })?;
        c.clear()?;
        let w = c.info()?;
        self.click(w.x + 20, w.y + 20)?;
        let press = c.await_event("positive click delivery to target client", |e| {
            e.kind == abi::event_kind::POINTER_BUTTON && e.pressed == 1
        })?;
        c.await_event("focus click release delivery to target client", |e| {
            e.kind == abi::event_kind::POINTER_BUTTON
                && e.pressed == 0
                && e.button == press.button
                && e.serial > press.serial
        })?;
        Ok(())
    }
}

/// Arming is output-free; the parent samples even while this thread is blocked.
struct StallWait(u64);
impl StallWait {
    fn new(begin: u64, ms: u64, what: &str) -> Self {
        stall_command(&format!("arm {begin} {} {what}", ms * 1_000_000));
        Self(begin)
    }
}
impl Drop for StallWait {
    fn drop(&mut self) { stall_command(&format!("end {}", self.0)); }
}
fn stall_command(command: &str) {
    if let Ok(mut evidence) = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence") {
        let _ = evidence.write_all(format!("WM_STALL {command}\n").as_bytes());
    }
}
