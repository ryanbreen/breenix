//! Session control wire: u32 LE payload size, tag, then counted byte strings.
//! No native layouts, shell parsing or inherited environment enters this channel.

#[path = "ownership.rs"]
pub mod ownership;
#[path = "devices.rs"]
pub mod devices;
use devices::RawInput;

pub const MAX_FRAME: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Launch { argv: Vec<Vec<u8>>, env: Vec<Vec<u8>> },
    Configure { case_pid: u64, seed: u64, mode: String },
    Configured,
    LaunchApp { argv: Vec<Vec<u8>>, uid: u32 },
    StartCompositor { kiosk: bool },
    StartCompositorWithPointer(u32),
    ReadStderr(u64),
    Stderr { pid: u64, dropped: u64, bytes: Vec<u8>, done: bool },
    Devices([u32; 2]),
    Input(RawInput),
    Text(String),
    Motion { x: i32, y: i32 },
    InputDone,
    HoldWindow(u64),
    WindowHeld(u64),
    Teardown,
    Ready(u64),
    Launched(u64),
    Exited { pid: u64, status: i32 },
    Output { pid: u64, bytes: Vec<u8> },
    Error(String),
    Stopped,
}

fn valid_launch(argv: &[Vec<u8>], env: &[Vec<u8>]) -> bool {
    let size = argv.iter().chain(env).try_fold(9usize, |total, s| total.checked_add(4)?.checked_add(s.len()));
    size.is_some_and(|n| n <= MAX_FRAME) && !argv.is_empty() && !argv[0].is_empty() && argv.len() <= 256 && env.len() <= 256
        && argv.iter().chain(env).all(|s| !s.contains(&0))
        && env.iter().all(|s| s.iter().position(|&b| b == b'=').is_some_and(|at| at != 0))
}
fn bytes(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
}
fn strings(out: &mut Vec<u8>, values: &[Vec<u8>]) {
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for value in values { bytes(out, value); }
}

pub fn encode(message: &Message) -> Result<Vec<u8>, String> {
    match message {
        Message::Output { bytes, .. } if bytes.len() > MAX_FRAME - 13 => return Err("control frame exceeds limit".into()),
        Message::Stderr { bytes, .. } if bytes.len() > 4096 => return Err("stderr chunk exceeds limit".into()),
        Message::Error(error) if error.len() > MAX_FRAME - 5 => return Err("control frame exceeds limit".into()),
        _ => {},
    }
    let mut out = vec![0; 4];
    match message {
        Message::Launch { argv, env } => {
            if !valid_launch(argv, env) { return Err("invalid launch argv/env".into()); }
            out.push(1); strings(&mut out, argv); strings(&mut out, env);
        }
        Message::Configure { case_pid, seed, mode } => {
            SessionEnvironment::new(*case_pid, *seed, mode)?;
            out.push(9); out.extend_from_slice(&case_pid.to_le_bytes());
            out.extend_from_slice(&seed.to_le_bytes()); bytes(&mut out, mode.as_bytes());
        }
        Message::Configured => out.push(10),
        Message::LaunchApp { argv, uid } => {
            if !valid_launch(argv, &[]) || !matches!(uid, 0 | 1000) { return Err("invalid app launch".into()); }
            out.push(11); out.extend_from_slice(&uid.to_le_bytes()); strings(&mut out, argv);
        }
        Message::StartCompositor { kiosk } => { out.push(14); out.push(u8::from(*kiosk)); }
        Message::StartCompositorWithPointer(id) => {
            if *id == 0 { return Err("invalid auxiliary pointer".into()); }
            out.push(20); out.extend_from_slice(&id.to_le_bytes());
        }
        Message::ReadStderr(pid) => { out.push(12); out.extend_from_slice(&pid.to_le_bytes()); }
        Message::Stderr { pid, dropped, bytes: value, done } => {
            out.push(13); out.extend_from_slice(&pid.to_le_bytes()); out.extend_from_slice(&dropped.to_le_bytes());
            out.push(u8::from(*done)); bytes(&mut out, value);
        }
        Message::Devices(ids) => {
            if ids.contains(&0) || ids[0] == ids[1] { return Err("invalid session device ids".into()); }
            out.push(15); for id in ids { out.extend_from_slice(&id.to_le_bytes()); }
        }
        Message::Input(event) => {
            event.validate()?;
            out.push(16); out.extend_from_slice(&event.kind.to_le_bytes());
            out.extend_from_slice(&event.code.to_le_bytes()); out.extend_from_slice(&event.value.to_le_bytes());
        }
        Message::Motion {x,y} => {out.push(21);out.extend_from_slice(&x.to_le_bytes());out.extend_from_slice(&y.to_le_bytes());}
        Message::Text(text) => {
            if text.is_empty() || text.len() > 256 { return Err("invalid session text length".into()); }
            out.push(22); bytes(&mut out, text.as_bytes());
        }
        Message::InputDone => out.push(17),
        Message::HoldWindow(id) => { out.push(18); out.extend_from_slice(&id.to_le_bytes()); }
        Message::WindowHeld(id) => { out.push(19); out.extend_from_slice(&id.to_le_bytes()); }
        Message::Teardown => out.push(2),
        Message::Ready(pid) => { out.push(3); out.extend_from_slice(&pid.to_le_bytes()); }
        Message::Launched(pid) => { out.push(4); out.extend_from_slice(&pid.to_le_bytes()); }
        Message::Exited { pid, status } => { out.push(5); out.extend_from_slice(&pid.to_le_bytes()); out.extend_from_slice(&status.to_le_bytes()); }
        Message::Output { pid, bytes: value } => { out.push(6); out.extend_from_slice(&pid.to_le_bytes()); bytes(&mut out, value); }
        Message::Error(error) => { out.push(7); bytes(&mut out, error.as_bytes()); }
        Message::Stopped => out.push(8),
    }
    let len = out.len() - 4;
    if len > MAX_FRAME { return Err("control frame exceeds limit".into()); }
    out[..4].copy_from_slice(&(len as u32).to_le_bytes());
    Ok(out)
}

struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let value = self.0.get(..n).ok_or("truncated control message")?;
        self.0 = &self.0[n..]; Ok(value)
    }
    fn word(&mut self) -> Result<u32, String> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn pid(&mut self) -> Result<u64, String> { Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap())) }
    fn bytes(&mut self) -> Result<Vec<u8>, String> { let n = self.word()? as usize; Ok(self.take(n)?.to_vec()) }
    fn strings(&mut self) -> Result<Vec<Vec<u8>>, String> {
        let count = self.word()?;
        if count > 256 { return Err("too many launch strings".into()); }
        (0..count).map(|_| self.bytes()).collect()
    }
}
pub fn decode(payload: &[u8]) -> Result<Message, String> {
    if payload.len() > MAX_FRAME { return Err("control frame exceeds limit".into()); }
    let mut cur = Cursor(payload);
    let message = match cur.take(1)?[0] {
        1 => {
            let argv = cur.strings()?; let env = cur.strings()?;
            if !valid_launch(&argv, &env) { return Err("invalid launch argv/env".into()); }
            Message::Launch { argv, env }
        }
        2 => Message::Teardown,
        3 => Message::Ready(cur.pid()?),
        4 => Message::Launched(cur.pid()?),
        5 => Message::Exited { pid: cur.pid()?, status: cur.word()? as i32 },
        6 => Message::Output { pid: cur.pid()?, bytes: cur.bytes()? },
        7 => Message::Error(String::from_utf8(cur.bytes()?).map_err(|_| "invalid error text")?),
        8 => Message::Stopped,
        9 => {
            let case_pid = cur.pid()?; let seed = cur.pid()?;
            let mode = String::from_utf8(cur.bytes()?).map_err(|_| "invalid mode text")?;
            SessionEnvironment::new(case_pid, seed, &mode)?;
            Message::Configure { case_pid, seed, mode }
        }
        10 => Message::Configured,
        11 => {
            let uid = cur.word()?; let argv = cur.strings()?;
            if !valid_launch(&argv, &[]) || !matches!(uid, 0 | 1000) { return Err("invalid app launch".into()); }
            Message::LaunchApp { argv, uid }
        }
        22 => {
            let text = String::from_utf8(cur.bytes()?).map_err(|_| "invalid session UTF-8")?;
            if text.is_empty() || text.len() > 256 { return Err("invalid session text length".into()); }
            Message::Text(text)
        }
        21 => Message::Motion {x:cur.word()? as i32,y:cur.word()? as i32},
        20 => { let id=cur.word()?; if id==0 {return Err("invalid auxiliary pointer".into());} Message::StartCompositorWithPointer(id) },
        12 => Message::ReadStderr(cur.pid()?),
        13 => {
            let pid = cur.pid()?; let dropped = cur.pid()?;
            let done = match cur.take(1)?[0] { 0 => false, 1 => true, _ => return Err("invalid stderr end flag".into()) };
            let bytes = cur.bytes()?;
            if bytes.len() > 4096 { return Err("stderr chunk exceeds limit".into()); }
            Message::Stderr { pid, dropped, bytes, done }
        }
        14 => Message::StartCompositor { kiosk: match cur.take(1)?[0] {
            0 => false, 1 => true, _ => return Err("invalid compositor kind".into()),
        } },
        15 => {
            let ids = [cur.word()?, cur.word()?];
            if ids.contains(&0) || ids[0] == ids[1] { return Err("invalid session device ids".into()); }
            Message::Devices(ids)
        }
        16 => {
            let kind = u16::from_le_bytes(cur.take(2)?.try_into().unwrap());
            let code = u16::from_le_bytes(cur.take(2)?.try_into().unwrap());
            let event = RawInput { kind, code, value: cur.word()? as i32 };
            event.validate()?;
            Message::Input(event)
        }
        17 => Message::InputDone,
        18 => Message::HoldWindow(cur.pid()?),
        19 => Message::WindowHeld(cur.pid()?),
        _ => return Err("unknown control message".into()),
    };
    if !cur.0.is_empty() { return Err("trailing control bytes".into()); }
    Ok(message)
}

#[derive(Default)]
pub struct Decoder { buffer: Vec<u8> }
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.buffer.len().saturating_add(bytes.len()) > 2 * MAX_FRAME + 8 { return Err("control buffer exceeds limit".into()); }
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() >= 4 {
            let n = u32::from_le_bytes(self.buffer[..4].try_into().unwrap()) as usize;
            if n == 0 || n > MAX_FRAME { return Err("invalid control frame length".into()); }
        }
        Ok(())
    }
    pub fn next(&mut self) -> Result<Option<Message>, String> {
        if self.buffer.len() < 4 { return Ok(None); }
        let n = u32::from_le_bytes(self.buffer[..4].try_into().unwrap()) as usize;
        if n == 0 || n > MAX_FRAME { return Err("invalid control frame length".into()); }
        if self.buffer.len() < n + 4 { return Ok(None); }
        let message = decode(&self.buffer[4..n+4])?;
        self.buffer.drain(..n+4);
        Ok(Some(message))
    }
}

pub fn signal_name(signal: i32) -> &'static str {
    const NAMES: [&str; 32] = ["UNKNOWN", "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGBUS", "SIGFPE", "SIGKILL", "SIGUSR1", "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM", "SIGSTKFLT", "SIGCHLD", "SIGCONT", "SIGSTOP", "SIGTSTP", "SIGTTIN", "SIGTTOU", "SIGURG", "SIGXCPU", "SIGXFSZ", "SIGVTALRM", "SIGPROF", "SIGWINCH", "SIGIO", "SIGPWR", "SIGSYS"];
    NAMES.get(signal as usize).copied().unwrap_or("UNKNOWN")
}

#[cfg(target_os = "breenix")]
#[path = "supervisor_runtime.rs"]
mod runtime;
#[cfg(target_os = "breenix")]
pub use runtime::serve;


pub const STDERR_CAP: usize = 64 * 1024;

/// Retain the newest bytes without allowing a writer to grow its evidence buffer.
#[derive(Default)]
pub struct StderrRing {
    bytes: std::collections::VecDeque<u8>,
    dropped: u64,
}
impl StderrRing {
    pub fn push(&mut self, input: &[u8]) {
        let overflow = self.bytes.len().saturating_add(input.len()).saturating_sub(STDERR_CAP);
        self.dropped = self.dropped.saturating_add(overflow as u64);
        if input.len() >= STDERR_CAP {
            self.bytes.clear();
            self.bytes.extend(&input[input.len() - STDERR_CAP..]);
        } else {
            self.bytes.drain(..overflow);
            self.bytes.extend(input);
        }
    }
    pub fn bytes(&self) -> Vec<u8> { self.bytes.iter().copied().collect() }
    pub fn dropped(&self) -> u64 { self.dropped }
}

#[derive(Clone, Debug)]
pub struct SessionEnvironment {
    root: String,
    seed: u64,
    mode: String,
}
impl SessionEnvironment {
    pub fn new(case_pid: u64, seed: u64, mode: &str) -> Result<Self, String> {
        if case_pid <= 1 || mode.is_empty() || mode.len() > 64 || !mode.bytes().all(|b| b.is_ascii_graphic() && b != b'=') {
            return Err("invalid session configuration".into());
        }
        Ok(Self { root: format!("/tmp/uitest/{case_pid}/"), seed, mode: mode.into() })
    }
    pub fn config_root(&self) -> &str { &self.root }
    /// Fonts are selected by the pinned etc/fonts.conf under this config root.
    pub fn environment(&self) -> Vec<Vec<u8>> {
        ["TZ=UTC".into(), "LANG=C.UTF-8".into(),
            format!("BREENIX_UI_CLOCK={}ui-clock", self.root),
            format!("BREENIX_UI_SEED={}", self.seed), format!("BREENIX_UI_MODE={}", self.mode),
            format!("BREENIX_CONFIG_ROOT={}", self.root)]
            .into_iter().map(String::into_bytes).collect()
    }
}
