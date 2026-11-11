//! Transport-independent host sequencing for collaboration sessions.
extern crate alloc;
use crate::wire::{self, DrawOp, Message};
use alloc::{collections::BTreeMap, vec::Vec};

pub const VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Apply(Message),
    PeerLeft { peer: u8, reason: Vec<u8> },
    Refused { local: u32, reason: Vec<u8> },
    Close { conn: u32, reason: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Frame { conn: u32, data: Vec<u8> },
    Event(Event),
}

struct Connection {
    peer: Option<u8>,
    next_local: u64,
    input: Vec<u8>,
}

/// Owns protocol state only; callers send frames and perform requested closes.
pub struct HostCore {
    width: u16,
    height: u16,
    connections: BTreeMap<u32, Connection>,
    global: u64,
    host_local: u32,
}

impl HostCore {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            connections: BTreeMap::new(),
            global: 0,
            host_local: 0,
        }
    }

    pub fn connect(&mut self, conn: u32) -> Vec<Output> {
        self.connections.entry(conn).or_insert_with(|| Connection {
            peer: None,
            next_local: 1,
            input: Vec::new(),
        });
        Vec::new()
    }

    pub fn bytes(&mut self, conn: u32, data: &[u8]) -> Vec<Output> {
        let mut out = Vec::new();
        let Some(c) = self.connections.get_mut(&conn) else {
            return out;
        };
        c.input.extend_from_slice(data);
        loop {
            let decoded = match self.connections.get(&conn) {
                Some(c) => wire::decode(&c.input),
                None => break,
            };
            match decoded {
                Ok(Some((message, used))) => {
                    self.connections.get_mut(&conn).unwrap().input.drain(..used);
                    self.message(conn, message, &mut out);
                }
                Ok(None) => break,
                Err(_) => {
                    self.disconnect(conn, b"decode", true, &mut out);
                    break;
                }
            }
        }
        out
    }

    pub fn close(&mut self, conn: u32) -> Vec<Output> {
        let mut out = Vec::new();
        self.disconnect(conn, b"disconnected", false, &mut out);
        out
    }

    pub fn submit(&mut self, op: DrawOp) -> Vec<Output> {
        let mut out = Vec::new();
        self.host_local = self
            .host_local
            .checked_add(1)
            .expect("host local sequence exhausted");
        if self.valid(&op) {
            self.admit(0, self.host_local, op, &mut out);
        } else {
            out.push(Output::Event(Event::Refused {
                local: self.host_local,
                reason: b"invalid-coords".to_vec(),
            }));
        }
        out
    }

    fn message(&mut self, conn: u32, message: Message, out: &mut Vec<Output>) {
        let peer = self.connections[&conn].peer;
        match (peer, message) {
            (None, Message::Hello { version, name }) => {
                let reason = if version != VERSION {
                    Some(&b"version"[..])
                } else if self
                    .connections
                    .values()
                    .filter(|c| c.peer.is_some())
                    .count()
                    >= 15
                {
                    Some(&b"full"[..])
                } else {
                    None
                };
                if let Some(reason) = reason {
                    Self::frame(
                        conn,
                        Message::JoinRefused {
                            reason: reason.to_vec(),
                        },
                        out,
                    );
                    self.disconnect(conn, reason, true, out);
                    return;
                }
                let peer = (1..=15)
                    .find(|id| !self.connections.values().any(|c| c.peer == Some(*id)))
                    .unwrap();
                self.connections.get_mut(&conn).unwrap().peer = Some(peer);
                Self::frame(
                    conn,
                    Message::Welcome {
                        peer,
                        width: self.width,
                        height: self.height,
                    },
                    out,
                );
                self.broadcast_except(conn, Message::PeerJoined { peer, name }, out);
            }
            (Some(peer), Message::Op { local, op }) => {
                let c = self.connections.get_mut(&conn).unwrap();
                if u64::from(local) != c.next_local {
                    self.disconnect(conn, b"sequence", true, out);
                } else {
                    c.next_local += 1;
                    if !self.valid(&op) {
                        Self::frame(
                            conn,
                            Message::Refused {
                                local,
                                reason: b"invalid-coords".to_vec(),
                            },
                            out,
                        );
                    } else {
                        let global = self
                            .global
                            .checked_add(1)
                            .expect("global sequence exhausted");
                        Self::frame(conn, Message::Ack { local, global }, out);
                        self.admit(peer, local, op, out);
                    }
                }
            }
            (Some(_), Message::Bye) => self.disconnect(conn, b"disconnected", true, out),
            _ => self.disconnect(conn, b"protocol", true, out),
        }
    }

    fn admit(&mut self, peer: u8, local: u32, op: DrawOp, out: &mut Vec<Output>) {
        self.global = self
            .global
            .checked_add(1)
            .expect("global sequence exhausted");
        let message = Message::Apply {
            global: self.global,
            peer,
            local,
            op,
        };
        for (&conn, c) in &self.connections {
            if c.peer.is_some() {
                Self::frame(conn, message.clone(), out);
            }
        }
        out.push(Output::Event(Event::Apply(message)));
    }

    fn disconnect(&mut self, conn: u32, reason: &[u8], request_close: bool, out: &mut Vec<Output>) {
        if let Some(c) = self.connections.remove(&conn) {
            if request_close {
                out.push(Output::Event(Event::Close {
                    conn,
                    reason: reason.to_vec(),
                }));
            }
            if let Some(peer) = c.peer {
                out.push(Output::Event(Event::PeerLeft {
                    peer,
                    reason: reason.to_vec(),
                }));
                self.broadcast_except(
                    conn,
                    Message::PeerLeft {
                        peer,
                        reason: reason.to_vec(),
                    },
                    out,
                );
            }
        }
    }

    fn broadcast_except(&self, except: u32, message: Message, out: &mut Vec<Output>) {
        for (&conn, c) in &self.connections {
            if conn != except && c.peer.is_some() {
                Self::frame(conn, message.clone(), out);
            }
        }
    }

    fn frame(conn: u32, message: Message, out: &mut Vec<Output>) {
        out.push(Output::Frame {
            conn,
            data: wire::encode(&message).expect("host message fits frame"),
        });
    }

    fn valid(&self, op: &DrawOp) -> bool {
        let point = |x: i32, y: i32| {
            x >= 0 && y >= 0 && x < i32::from(self.width) && y < i32::from(self.height)
        };
        match *op {
            DrawOp::Pencil { x0, y0, x1, y1, .. }
            | DrawOp::Brush { x0, y0, x1, y1, .. }
            | DrawOp::Eraser { x0, y0, x1, y1, .. }
            | DrawOp::Line { x0, y0, x1, y1, .. } => {
                point(x0.into(), y0.into()) && point(x1.into(), y1.into())
            }
            DrawOp::Rectangle { x, y, .. } | DrawOp::Circle { x, y, .. } => {
                point(x.into(), y.into())
            }
            DrawOp::Fill { x, y, .. } => point(x.into(), y.into()),
            DrawOp::Clear | DrawOp::Undo => true,
        }
    }
}
