//! Local collector protocol. Deadlines use the runner's monotonic clock.
use super::result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger { Fail, Crash, Timeout, SaveAll }
impl Trigger {
    pub fn name(self) -> &'static str {
        match self { Self::Fail => "fail", Self::Crash => "crash", Self::Timeout => "timeout", Self::SaveAll => "save-all" }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request { Discard, Collect(Trigger) }
impl Request {
    pub fn command(self) -> String {
        match self { Self::Discard => "end discard\n".into(), Self::Collect(trigger) => format!("end collect {}\n", trigger.name()) }
    }
    pub fn parse(command: &str) -> Result<Self, String> {
        match command {
            "end discard\n" => Ok(Self::Discard),
            "end collect fail\n" => Ok(Self::Collect(Trigger::Fail)),
            "end collect crash\n" => Ok(Self::Collect(Trigger::Crash)),
            "end collect timeout\n" => Ok(Self::Collect(Trigger::Timeout)),
            "end collect save-all\n" => Ok(Self::Collect(Trigger::SaveAll)),
            _ => Err("invalid collector request".into()),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum State { Waiting, Done, Timeout, Died }

pub struct Handshake<'a> {
    suite: &'a str,
    case: &'a str,
    request: Request,
    partial: Vec<u8>,
    collected: bool,
    done: bool,
}
impl<'a> Handshake<'a> {
    pub fn new(suite: &'a str, case: &'a str, request: Request) -> Self {
        Self { suite, case, request, partial: Vec::new(), collected: false, done: false }
    }
    pub fn request(&self) -> Request { self.request }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        let mut records = Vec::new();
        for &byte in bytes {
            if self.done { return Err("collector data after done".into()); }
            if byte != b'\n' {
                if self.partial.len() >= result::MAX_RECORD_BYTES - 1 { return Err("collector record exceeds 4096 bytes".into()); }
                self.partial.push(byte);
                continue;
            }
            let line = String::from_utf8(std::mem::take(&mut self.partial)).map_err(|_| "invalid collector UTF-8")?;
            if line == "done" { self.done = true; continue; }
            let Request::Collect(trigger) = self.request else { return Err("collection was not requested".into()); };
            if self.collected { return Err("duplicate collector record".into()); }
            if !line.starts_with(&format!("SUITE {} COLLECT {} ", self.suite, self.case))
                || result::Accounting::new(self.suite, self.case, &[], true)?.accept(&line).is_err()
                || line.split(' ').nth(4) != Some(format!("trigger={}", trigger.name()).as_str()) {
                return Err("malformed or other-case collector record".into());
            }
            self.collected = true;
            records.push(line);
        }
        Ok(records)
    }
    pub fn state(&self, now_ns: i128, deadline_ns: i128, supervisor_dead: bool) -> State {
        if now_ns >= deadline_ns { return State::Timeout; }
        if self.done {
            return if self.request == Request::Discard || self.collected { State::Done } else { State::Timeout };
        }
        if supervisor_dead { State::Died } else { State::Waiting }
    }
}
