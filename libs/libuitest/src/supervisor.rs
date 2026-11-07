//! Session control wire: u32 LE payload size, tag, then counted byte strings.
//! No native layouts, shell parsing or inherited environment enters this channel.

pub const MAX_FRAME: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Launch { argv: Vec<Vec<u8>>, env: Vec<Vec<u8>> },
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
        Message::Error(error) if error.len() > MAX_FRAME - 5 => return Err("control frame exceeds limit".into()),
        _ => {},
    }
    let mut out = vec![0; 4];
    match message {
        Message::Launch { argv, env } => {
            if !valid_launch(argv, env) { return Err("invalid launch argv/env".into()); }
            out.push(1); strings(&mut out, argv); strings(&mut out, env);
        }
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
