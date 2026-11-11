//! Bounded per-connection output, independent of the socket implementation.
use crate::wire::{self, Message};
use alloc::{
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};

pub const QUEUE_LIMIT: usize = 1024 * 1024;
pub const SYNC_LOW_WATER: usize = 256 * 1024;
const CHUNK: usize = 65531;

#[derive(Debug, PartialEq, Eq)]
pub enum SendError<E> {
    WouldBlock,
    Other(E),
}

pub trait Transport {
    type Error;
    fn send(&mut self, conn: u32, bytes: &[u8]) -> Result<usize, SendError<Self::Error>>;
}

#[derive(Default)]
struct Connection {
    frames: VecDeque<Vec<u8>>,
    offset: usize,
    len: usize,
    closed: bool,
    sync: Option<(Vec<u8>, usize)>,
}

#[derive(Default)]
pub struct OutputQueues {
    connections: BTreeMap<u32, Connection>,
}
impl OutputQueues {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn connect(&mut self, conn: u32) {
        self.connections.entry(conn).or_default();
    }
    pub fn remove(&mut self, conn: u32) {
        self.connections.remove(&conn);
    }
    pub fn queued(&self, conn: u32) -> usize {
        self.connections.get(&conn).map_or(0, |c| c.len)
    }
    pub fn wants_write(&self, conn: u32) -> bool {
        self.queued(conn) != 0
    }
    pub fn close_reason(&self, conn: u32) -> Option<&'static [u8]> {
        self.connections
            .get(&conn)
            .filter(|c| c.closed)
            .map(|_| &b"queue-full"[..])
    }
    pub fn enqueue(&mut self, conn: u32, frame: &[u8]) -> Result<(), &'static [u8]> {
        let c = self
            .connections
            .get_mut(&conn)
            .ok_or(&b"disconnected"[..])?;
        if c.closed {
            return Err(b"queue-full");
        }
        if frame.len() > QUEUE_LIMIT - c.len {
            c.closed = true;
            c.frames.clear();
            c.len = 0;
            c.offset = 0;
            c.sync = None;
            return Err(b"queue-full");
        }
        if !frame.is_empty() {
            c.frames.push_back(frame.to_vec());
            c.len += frame.len();
        }
        Ok(())
    }
    pub fn start_sync(&mut self, conn: u32, canvas: Vec<u8>) -> Result<(), &'static [u8]> {
        if canvas.len() > u32::MAX as usize {
            return Err(b"canvas-too-large");
        }
        let c = self
            .connections
            .get_mut(&conn)
            .ok_or(&b"disconnected"[..])?;
        if c.closed {
            return Err(b"queue-full");
        }
        if c.sync.is_some() {
            return Err(b"sync-in-progress");
        }
        c.sync = Some((canvas, 0));
        Ok(())
    }
    pub fn syncing(&self, conn: u32) -> bool {
        self.connections
            .get(&conn)
            .is_some_and(|c| c.sync.is_some())
    }
    pub fn fill_sync(&mut self, conn: u32) -> Result<(), &'static [u8]> {
        while self.queued(conn) < SYNC_LOW_WATER {
            let c = self
                .connections
                .get_mut(&conn)
                .ok_or(&b"disconnected"[..])?;
            if c.closed {
                return Err(b"queue-full");
            }
            let Some((canvas, offset)) = c.sync.as_mut() else {
                break;
            };
            let message = if *offset == canvas.len() {
                Message::SyncEnd
            } else {
                let end = (*offset + CHUNK).min(canvas.len());
                let msg = Message::SyncChunk {
                    offset: *offset as u32,
                    data: canvas[*offset..end].to_vec(),
                };
                *offset = end;
                msg
            };
            if matches!(message, Message::SyncEnd) {
                c.sync = None;
            }
            let frame = wire::encode(&message).expect("sync chunk fits");
            self.enqueue(conn, &frame)?;
        }
        Ok(())
    }
    pub fn flush<T: Transport>(
        &mut self,
        conn: u32,
        transport: &mut T,
    ) -> Result<(), SendError<T::Error>> {
        let Some(c) = self.connections.get_mut(&conn) else {
            return Ok(());
        };
        while let Some(frame) = c.frames.front() {
            match transport.send(conn, &frame[c.offset..]) {
                Ok(0) | Err(SendError::WouldBlock) => break,
                Ok(n) => {
                    assert!(n <= frame.len() - c.offset, "transport sent beyond buffer");
                    c.offset += n;
                    c.len -= n;
                    if c.offset == frame.len() {
                        c.frames.pop_front();
                        c.offset = 0;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}
