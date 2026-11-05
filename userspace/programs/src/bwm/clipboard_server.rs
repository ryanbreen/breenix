//! Incremental, readiness-driven BCLIP001 connection.
use std::io::{self, Read, Write};
use super::clipboard::{Decoder, Reply, Store};

pub struct Connection<S> {
    stream: S,
    peer: u32,
    decoder: Decoder,
    reply: Option<Vec<u8>>,
    written: usize,
    done: bool,
}
impl<S: Read + Write> Connection<S> {
    pub fn new(stream: S, peer: u32) -> Self {
        Self { stream, peer, decoder: Decoder::new(), reply: None, written: 0, done: false }
    }
    pub fn stream(&self) -> &S { &self.stream }
    pub fn wants_write(&self) -> bool { self.reply.is_some() }
    pub fn done(&self) -> bool { self.done }
    pub fn close(&mut self) { self.done = true; }
    pub fn advance(&mut self, readable: bool, writable: bool, store: &mut Store, owns: impl Fn(u32, u64) -> bool) -> io::Result<()> {
        if readable && self.reply.is_none() {
            let mut buf = [0u8; 8192];
            match self.stream.read(&mut buf) {
                Ok(0) => self.done = true,
                Ok(n) => {
                    let (_, result) = self.decoder.feed(&buf[..n]);
                    if let Some(result) = result {
                        let reply = match result {
                            Ok(request) if owns(self.peer, request.window) => store.execute(request),
                            Ok(_) => Reply::error(-1, store.serial()),
                            Err(status) => Reply::error(status, store.serial()),
                        };
                        self.reply = Some(reply.encode());
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
                    Ok(n) => { self.written += n; self.done = self.written == reply.len(); }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }
}
