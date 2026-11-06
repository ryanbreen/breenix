//! Incremental, readiness-driven BCLIP001 connection.
use std::io::{self, Read, Write};
use super::clipboard::{Decoder, Reply, Store};

/// Slots bwm serves at once, and how many of them one peer process may hold.
pub const MAX_CONNECTIONS: usize = 16;
pub const MAX_PER_PEER: usize = 2;
/// A connection that moves no bytes for this long on the UI clock is closed.
pub const IDLE_NS: u64 = 5_000_000_000;

/// Admit a peer only while it owns a desktop window and holds fewer than its share of slots.
pub fn admit(peers: impl Iterator<Item = u32>, peer: u32, owns_window: bool) -> bool {
    let (mut total, mut held) = (0, 0);
    for p in peers {
        total += 1;
        held += usize::from(p == peer);
    }
    owns_window && total < MAX_CONNECTIONS && held < MAX_PER_PEER
}

pub struct Connection<S> {
    stream: S,
    peer: u32,
    decoder: Decoder,
    authorized: bool,
    reply: Option<Vec<u8>>,
    written: usize,
    done: bool,
    deadline: u64,
}
impl<S: Read + Write> Connection<S> {
    pub fn new(stream: S, peer: u32) -> Self {
        Self { stream, peer, decoder: Decoder::new(), authorized: false, reply: None, written: 0, done: false, deadline: u64::MAX }
    }
    pub fn accepted(stream: S, peer: u32, now: u64) -> Self {
        let mut connection = Self::new(stream, peer);
        connection.deadline = now.saturating_add(IDLE_NS);
        connection
    }
    pub fn stream(&self) -> &S { &self.stream }
    pub fn peer(&self) -> u32 { self.peer }
    pub fn wants_write(&self) -> bool { self.reply.is_some() }
    pub fn done(&self) -> bool { self.done }
    pub fn close(&mut self) { self.done = true; }
    pub fn deadline(&self) -> u64 { self.deadline }
    /// Extend the deadline after progress; close a connection that let it pass.
    pub fn idle(&mut self, now: u64, progressed: bool) {
        if progressed && self.deadline != u64::MAX { self.deadline = now.saturating_add(IDLE_NS); }
        else if now >= self.deadline { self.done = true; }
    }
    /// After a hangup the queued request bytes are still read; only an unreadable reply is dropped.
    pub fn retained(&self, hangup: bool, error: bool) -> bool {
        !self.done && !error && !(hangup && self.wants_write())
    }
    /// Returns whether any bytes moved.
    pub fn advance(&mut self, readable: bool, writable: bool, store: &mut Store, owns: impl Fn(u32, u64) -> bool) -> io::Result<bool> {
        let mut progressed = false;
        if readable && self.reply.is_none() {
            let mut buf = [0u8; 8192];
            match self.stream.read(&mut buf) {
                Ok(0) => self.done = true,
                Ok(n) => {
                    progressed = true;
                    let (_, result) = self.decoder.feed(&buf[..n]);
                    if let Some(result) = result {
                        let reply = match result {
                            Ok(request) if owns(self.peer, request.window) => store.execute(request),
                            Ok(_) => Reply::error(-1, store.serial()),
                            Err(status) => Reply::error(status, store.serial()),
                        };
                        self.reply = Some(reply.encode());
                    } else if let Some(window) = self.decoder.window().filter(|_| !self.authorized) {
                        // Refuse a non-owner before buffering its payload.
                        if owns(self.peer, window) { self.authorized = true; }
                        else { self.reply = Some(Reply::error(-1, store.serial()).encode()); }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
                Err(e) => return Err(e),
            }
        }
        if writable {
            if let Some(reply) = &self.reply {
                let end = (self.written + 8192).min(reply.len());
                match self.stream.write(&reply[self.written..end]) {
                    Ok(0) => self.done = true,
                    Ok(n) => { progressed = true; self.written += n; self.done = self.written == reply.len(); }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(progressed)
    }
}
