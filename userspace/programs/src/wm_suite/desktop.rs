//! The case controls processes and observes scanout; bwm alone places, routes and composes.
use super::event_log::{Entry, EventLog};
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
    mut test: impl FnMut() -> Result<bool, String>,
) -> Result<(), String> {
    let end = now() + ms * 1_000_000;
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
        self.signal(signal::SIGKILL)?;
        let mut result = None;
        until(500, "reap owned process", || {
            result = self.status()?;
            Ok(result.is_some())
        })?;
        let status = result.unwrap();
        if status & 127 != signal::SIGKILL {
            return Err(format!(
                "owned process did not die from requested SIGKILL: wait status {status}"
            ));
        }
        Ok(status)
    }
}
impl From<Pid> for Child {
    fn from(pid: Pid) -> Self {
        Self {
            pid: pid.raw() as i32,
            reaped: false,
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
    log: EventLog,
    pub logged: Vec<Entry>,
    pub second: u64,
    /// 0 disables the counter; another value is the displayed frame plus one.
    pub frame: u64,
    pub id: u64,
    pub commit: u64,
    /// Commit that introduced the current pixels; metadata-only commits retain it.
    pub frame_commit: u64,
    pub commit_ns: u64,
    pub handling_commit: u64,
    pub width: u32,
    pub height: u32,
    pub seed: u32,
    pub events: Vec<abi::WindowEvent>,
}
fn read_n(file: &mut File, bytes: &mut [u8]) -> Result<(), String> {
    let end = now() + 2_000_000_000;
    let mut done = 0;
    while done < bytes.len() {
        if now() >= end || case_ms_left() < 200 {
            return Err(
                "wm-client reply deadline (client exited, hung or failed its GPU operation)".into(),
            );
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
        Self::start_mode(seed, flags::IGNORE_CLOSE)
    }
    pub fn unregistered(seed: u32) -> Result<Self, String> {
        Self::start_mode(seed, flags::UNREGISTERED | flags::IGNORE_CLOSE)
    }
    pub fn start_mode(seed: u32, flags: u32) -> Result<Self, String> {
        let (log, log_file) = EventLog::create()?;
        let (cr, cw) = io::pipe().map_err(|e| e.to_string())?;
        let (rr, rw) = io::pipe().map_err(|e| e.to_string())?;
        let pid = spawn(
            &[
                (cr, Fd::from_raw(3)),
                (rw, Fd::from_raw(4)),
                (Fd::from_raw(log_file.as_raw_fd() as u64), Fd::from_raw(5)),
            ],
            Run::Exec(
                b"/usr/libexec/breenix/wm-client_test\0",
                &[b"wm-client_test\0", b"--control\0"],
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
            log,
            logged: Vec::new(),
            second: 0,
            frame: 0,
            id: 0,
            commit: 0,
            frame_commit: 0,
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
        self.receive()
    }
    pub(super) fn receive(&mut self) -> Result<(), String> {
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
        self.frame_commit = quad(&h, 64);
        self.commit_ns = quad(&h, 32);
        self.handling_commit = quad(&h, 40);
        self.second = quad(&h, 48);
        self.frame = quad(&h, 56);
        self.width = word(&h, 16);
        self.height = word(&h, 20);
        self.seed = word(&h, 24);
        self.collect_through(word(&h, 28) as u64)
    }
    pub fn collect_events(&mut self) -> Result<(), String> {
        self.collect_through(self.log.published())
    }
    fn collect_through(&mut self, published: u64) -> Result<(), String> {
        for entry in self.log.drain_through(published)? {
            if self.logged.len() >= 8192 {
                return Err("retained client event log overflow".into());
            }
            if entry.window == self.id {
                self.events.push(entry.event);
            }
            self.logged.push(entry);
        }
        Ok(())
    }
    pub fn select(&mut self, index: u32) -> Result<(), String> {
        self.command(SELECT_WINDOW, index, 0)?;
        self.events = self
            .logged
            .iter()
            .filter(|e| e.window == self.id)
            .map(|e| e.event)
            .collect();
        Ok(())
    }
    pub fn field(&mut self, text: &str, select: bool) -> Result<(), String> {
        self.send(FIELD_SET, text.len() as u32, u32::from(select))?;
        self.commands
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
        self.receive()
    }
    pub fn field_text(&mut self) -> Result<Vec<u8>, String> {
        self.send(FIELD_GET, 0, 0)?;
        let (code, _, text) = self.clipboard_reply()?;
        if code != 0 {
            return Err(format!("text field query errno {}", -code));
        }
        Ok(text)
    }
    pub fn title(&mut self, title: &[u8]) -> Result<(), String> {
        self.send(SET_TITLE, title.len() as u32, 0)?;
        self.commands.write_all(title).map_err(|e| e.to_string())?;
        self.receive()
    }
    pub fn create_window(
        &mut self,
        seed: u32,
        width: u32,
        height: u32,
        buffers: u32,
    ) -> Result<(), String> {
        if width > 65535 || height > 65535 {
            return Err("fixture dimensions exceed command encoding".into());
        }
        let mut bytes = [0; COMMAND_SIZE];
        for (at, v) in [
            (0, CREATE_WINDOW),
            (4, seed),
            (8, buffers),
            (12, width | (height << 16)),
        ] {
            bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.commands.write_all(&bytes).map_err(|e| e.to_string())?;
        self.receive()
    }
    pub fn finish_command(&mut self) -> Result<(), String> {
        self.receive()
    }
    pub fn response_ready(&self) -> Result<bool, String> {
        let mut f = [io::PollFd {
            fd: self.replies.as_raw_fd(),
            events: 1,
            revents: 0,
        }];
        io::poll(&mut f, 0).map_err(|e| e.to_string())?;
        Ok(f[0].revents & 1 != 0)
    }
    pub fn require_alive(&mut self) -> Result<(), String> {
        match self.child.status()? {
            None => Ok(()),
            Some(status) => Err(format!(
                "unexpected client exit: window {} wait status {status}",
                self.id
            )),
        }
    }
    pub fn snapshot(&mut self) -> Result<(), String> {
        self.command(SNAPSHOT, 0, 0)
    }
    pub fn clear(&mut self) -> Result<(), String> {
        self.command(CLEAR_EVENTS, 0, 0)?;
        self.events.clear();
        self.logged.clear();
        Ok(())
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
        Ok(quad(&h, 8))
    }
    pub fn clipboard(&mut self, set: bool, text: &[u8]) -> Result<(i32, u64, Vec<u8>), String> {
        self.clipboard_mime(set, text, 1)
    }
    pub fn clipboard_mime(
        &mut self,
        set: bool,
        text: &[u8],
        mime: u32,
    ) -> Result<(i32, u64, Vec<u8>), String> {
        self.send(
            if set { CLIP_SET } else { CLIP_GET },
            text.len() as u32,
            mime,
        )?;
        self.commands
            .write_all(text)
            .map_err(|e| format!("clipboard client command: {e}"))?;
        self.clipboard_reply()
    }
    pub fn clipboard_begin(&mut self, text: &[u8]) -> Result<(), String> {
        self.send(CLIP_SET, text.len() as u32, 1)?;
        self.commands.write_all(text).map_err(|e| e.to_string())
    }
    pub fn clipboard_reply(&mut self) -> Result<(i32, u64, Vec<u8>), String> {
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
        Ok(found.unwrap())
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
        let success = if word(&s.record, 16) == 4 { 0x1100 } else { 1 };
        if word(&s.record, 0) != 2
            || word(&s.record, 4) != 1024
            || word(&s.record, 20) != 4
            || word(&s.record, 40) != s.width
            || word(&s.record, 44) != s.height
            || word(&s.record, 56) != s.width * 4
            || word(&s.record, 164) != 0
            || quad(&s.record, 80) == 0
            || quad(&s.record, 120) == 0
            || quad(&s.record, 184) == 0
            || word(&s.record, 128) != success
            || word(&s.record, 192) != success
        {
            return Err("scanout lacks its complete v2 dimensions, binding, native readback and present completion".into());
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
                quad(&self.record, 200 + i * 32) == id
                    && quad(&self.record, 208 + i * 32) >= commit
                    && (commit == 0
                        || (quad(&self.record, 216 + i * 32) != 0
                            && quad(&self.record, 224 + i * 32) != 0))
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
            c.frame_commit != 0 && self.contains(c.id, c.frame_commit),
            &format!(
                "device observation does not list client {} commit {}",
                c.id, c.frame_commit
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
                let expected = if c.frame != 0 && counter_ink(c.frame - 1, x, y) {
                    0xffff_ffff
                } else {
                    expected
                };
                let got = self.pixel(w.x + x as i32, w.y + y as i32)?;
                if got != expected {
                    return Err(format!("bwm client {} at local {x},{y}: scanout {got:08x}, expected {expected:08x}", c.id).into());
                }
            }
        }
        Ok(())
    }
}

fn counter_ink(frame: u64, x: u32, y: u32) -> bool {
    const MASKS: [u16; 10] = [
        0b111_101_101_101_111,
        0b010_110_010_010_111,
        0b111_001_111_100_111,
        0b111_001_111_001_111,
        0b101_101_111_001_001,
        0b111_100_111_001_111,
        0b111_100_111_101_111,
        0b111_001_001_001_001,
        0b111_101_111_101_111,
        0b111_101_111_001_111,
    ];
    if x < 2 || y < 2 || y >= 7 {
        return false;
    }
    let text = frame.to_string();
    let column = (x - 2) as usize / 4;
    let local = (x - 2) % 4;
    if column >= text.len() || local == 3 {
        return false;
    }
    let digit = (text.as_bytes()[column] - b'0') as usize;
    MASKS[digit] & (1 << (14 - (y - 2) * 3 - local)) != 0
}

pub struct Desktop {
    pub wm: Child,
    log: File,
    output: String,
    pub width: u32,
    pub height: u32,
    pub scale: i32,
}
impl Desktop {
    pub fn start() -> Result<Self, String> {
        Self::launch(false)
    }
    pub fn launch(supervised: bool) -> Result<Self, String> {
        let previous_context = Screen::read().map_or(0, |s| quad(&s.record, 64));
        graphics::take_over_display().map_err(|e| format!("session display: {e}"))?;
        let (r, w) = io::pipe2(io::status_flags::O_NONBLOCK).map_err(|e| e.to_string())?;
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
            output: String::new(),
            width: mode.width,
            height: mode.height,
            scale: mode.scale as i32,
        };
        let ready = until(4000, "bwm first completed desktop", || {
            if let Some(status) = d.wm.status()? {
                return Err(format!(
                    "desktop process exited with wait status {status}; {}",
                    d.output()
                ));
            }
            // The first present precedes bwm's full startup validation. Wait for
            // that producer to finish before measuring the client's settle.
            // init's SPAWN children use the console rather than this pipe. Its
            // initial client's native presentation is the stronger readiness
            // handshake for that path, under the same startup deadline.
            if !supervised && !d.output().contains("[bwm] desktop fills the scanout:") {
                return Ok(false);
            }
            let initial_clients = if supervised { listed()? } else { Vec::new() };
            Ok(Screen::read().is_ok_and(|s| {
                s.serial() != 0
                    && quad(&s.record, 64) != previous_context
                    && s.pixel(d.width as i32 / 2, 1).ok() == Some(0xff14161e)
                    && (!supervised
                        || initial_clients
                            .iter()
                            .any(|w| w.commit_id != 0 && s.contains(w.id, w.commit_id)))
            }))
        });
        ready.map_err(|e| format!("{e}; {}", d.output()))?;
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
        if self.output.len() < 65536 {
            self.output
                .push_str(&String::from_utf8_lossy(&out).replace(['\n', '\r'], " "));
        }
        self.output.clone()
    }
    pub fn shown(&mut self, c: &Client) -> Result<Screen, String> {
        self.shown_after(c, c.frame_commit)
    }
    /// A newly discovered client's first-frame validation runs synchronously in
    /// bwm. Do not start an input scenario until that startup work is finished.
    /// The marker gates readiness; the completed device record must also name
    /// the client's commit, within the existing settle bound.
    pub fn client_ready(&mut self, c: &Client) -> Result<(), String> {
        let marker = format!("[bwm] composited client frame: window {} on screen,", c.id);
        until(2000, "bwm completed client startup validation", || {
            if !self.output().contains(&marker) {
                return Ok(false);
            }
            Ok(Screen::read().is_ok_and(|s| s.contains(c.id, c.frame_commit)))
        })
    }
    pub fn shown_after(&mut self, c: &Client, commit: u64) -> Result<Screen, String> {
        let mut result = None;
        let mut observed = String::from("no readback attempted");
        until(
            2000,
            &format!("bwm composite of client {} commit {}", c.id, commit),
            || {
                if let Some(status) = self.wm.status()? {
                    return Err(format!(
                        "desktop died, wait status {status}; {}",
                        self.output()
                    ));
                }
                match Screen::read() {
                    Ok(s) if s.contains(c.id, commit) => {
                        result = Some(s);
                        Ok(true)
                    }
                    Ok(s) => {
                        let n = word(&s.record, 196) as usize;
                        let entries: Vec<_> = (0..n.min(16))
                            .map(|i| (quad(&s.record, 200 + i * 32), quad(&s.record, 208 + i * 32)))
                            .collect();
                        observed = format!("composite {} entries {:?}", s.serial(), entries);
                        Ok(false)
                    }
                    Err(e) => {
                        observed = e;
                        Ok(false)
                    }
                }
            },
        )
        .map_err(|e| {
            let log = self.output();
            let tail = log.char_indices().rev().nth(90).map_or(0, |(at, _)| at);
            format!("{e}; last {observed}; {}", &log[tail..])
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
        .map(|_| ())
    }
    pub fn click(&self, x: i32, y: i32) -> Result<(), String> {
        self.pointer(x, y, 1, 0)?;
        time::sleep_ms(100).map_err(|e| e.to_string())?;
        self.pointer(x, y, 0, 0)?;
        time::sleep_ms(100).map_err(|e| e.to_string())
    }
    pub fn focus(&mut self, c: &mut Client) -> Result<(), String> {
        let marker = format!("[bwm] composited client frame: window {} on screen,", c.id);
        if !self.output().contains(&marker) {
            self.client_ready(c)?;
        }
        until(2000, "bwm assigned client geometry", || {
            let w = c.info()?;
            Ok(w.x > 0 && w.y > 0)
        })?;
        c.clear()?;
        let w = c.info()?;
        self.pointer(w.x + 20, w.y + 20, 1, 0)?;
        let pressed = c.await_event("positive click delivery to target client", |e| {
            e.kind == abi::event_kind::POINTER_BUTTON && e.pressed == 1
        });
        // Release even when the prerequisite failed, so the next case cannot
        // inherit a held hardware button. Fast clicks have their own case.
        let released = self.pointer(w.x + 20, w.y + 20, 0, 0);
        pressed?;
        released?;
        Ok(())
    }
}
