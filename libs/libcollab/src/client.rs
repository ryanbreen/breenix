//! Client-side session logic: connect, handshake, receive ops.

use libbreenix::error::Error;
use libbreenix::io::{self, PollFd, poll_events};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::types::Fd;

use crate::event::{CollabEvent, DrawOp};
use crate::peer::PeerInfo;
use crate::wire::{self, MessageType, StreamDecoder, HEADER_SIZE, PROTOCOL_VERSION};

/// Client session state
pub struct ClientState {
    fd: Fd,
    decoder: StreamDecoder,
    canvas_w: u16,
    canvas_h: u16,
    next_seqno: u32,
    connected: bool,
    event_queue: Vec<CollabEvent>,
    peers: Vec<PeerInfo>,
}

impl ClientState {
    /// Connect to a host and send Hello.
    pub fn new(addr: &SockAddrIn, name: &[u8]) -> Result<Self, Error> {
        let fd = socket::socket(AF_INET, SOCK_STREAM, 0)?;
        if let Err(e) = socket::connect_inet(fd, addr) {
            let _ = io::close(fd);
            return Err(e);
        }

        // Set non-blocking after connect succeeds
        let _ = io::fcntl_getfl(fd).and_then(|flags| {
            io::fcntl_setfl(fd, flags as i32 | io::status_flags::O_NONBLOCK)
        });

        let len = name.len().min(32);

        let mut state = Self {
            fd,
            decoder: StreamDecoder::new(),
            canvas_w: 0,
            canvas_h: 0,
            next_seqno: 0,
            connected: true,
            event_queue: Vec::new(),
            peers: Vec::new(),
        };

        // Send Hello: version(u16) + name_len(u8) + name(N bytes)
        let mut payload = [0u8; 35];
        let mut off = 0;
        off = wire::put_u16(&mut payload, off, PROTOCOL_VERSION);
        off = wire::put_u8(&mut payload, off, len as u8);
        payload[off..off + len].copy_from_slice(&name[..len]);
        let plen = off + len;
        state.send_msg(MessageType::Hello, &payload[..plen]);

        Ok(state)
    }

    /// Fill poll FDs for the client's socket.
    pub fn poll_fds(&self, out: &mut [PollFd]) -> usize {
        if !self.connected || out.is_empty() {
            return 0;
        }
        out[0] = PollFd::new(self.fd, poll_events::POLLIN);
        1
    }

    /// Process I/O after poll returns.
    pub fn process_io(&mut self, poll_results: &[PollFd]) {
        if !self.connected {
            return;
        }

        for pfd in poll_results {
            if pfd.fd == self.fd.raw() as i32 {
                if (pfd.revents & (poll_events::POLLHUP | poll_events::POLLERR)) != 0 {
                    self.connected = false;
                    self.event_queue.push(CollabEvent::SessionEnded);
                    return;
                }
                if (pfd.revents & poll_events::POLLIN) != 0 {
                    self.read_data();
                }
                break;
            }
        }
    }

    /// Drain the next event from the queue.
    pub fn next_event(&mut self) -> Option<CollabEvent> {
        if self.event_queue.is_empty() {
            None
        } else {
            Some(self.event_queue.remove(0))
        }
    }

    /// Send a draw op to the host.
    pub fn send_draw_op(&mut self, op: &DrawOp) {
        let mut payload = [0u8; 64];
        let (msg_type, plen) = op.encode(&mut payload);
        self.send_msg(msg_type, &payload[..plen]);
    }

    /// Send cursor update to the host.
    pub fn send_cursor(&mut self, x: i16, y: i16, visible: bool) {
        // Client sends without peer_id prefix (host adds it)
        let mut payload = [0u8; 5];
        let mut off = 0;
        off = wire::put_i16(&mut payload, off, x);
        off = wire::put_i16(&mut payload, off, y);
        wire::put_u8(&mut payload, off, if visible { 1 } else { 0 });
        self.send_msg(MessageType::Cursor, &payload[..5]);
    }

    /// Send tool change to the host.
    pub fn send_tool_change(&mut self, tool: u8, size: u8, r: u8, g: u8, b: u8) {
        let mut payload = [0u8; 5];
        let mut off = 0;
        off = wire::put_u8(&mut payload, off, tool);
        off = wire::put_u8(&mut payload, off, size);
        off = wire::put_u8(&mut payload, off, r);
        off = wire::put_u8(&mut payload, off, g);
        wire::put_u8(&mut payload, off, b);
        self.send_msg(MessageType::ToolChange, &payload[..5]);
    }

    /// Get canvas dimensions from Welcome.
    pub fn canvas_dims(&self) -> (u16, u16) {
        (self.canvas_w, self.canvas_h)
    }

    /// Get peers list.
    pub fn peers(&self) -> &[PeerInfo] {
        &self.peers
    }

    /// Check if still connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Disconnect gracefully.
    pub fn disconnect(&mut self) {
        if self.connected {
            self.send_msg(MessageType::Bye, &[]);
            self.connected = false;
            let _ = io::close(self.fd);
        }
    }

    // ---- Internal helpers ----

    fn next_seq(&mut self) -> u32 {
        let s = self.next_seqno;
        self.next_seqno = self.next_seqno.wrapping_add(1);
        s
    }

    fn send_msg(&mut self, msg_type: MessageType, payload: &[u8]) {
        if !self.connected {
            return;
        }
        let mut buf = vec![0u8; HEADER_SIZE + payload.len()];
        let seq = self.next_seq();
        if let Some(total) = wire::encode(&mut buf, msg_type, seq, payload) {
            let _ = socket::send(self.fd, &buf[..total]);
        }
    }

    fn read_data(&mut self) {
        let mut buf = [0u8; 4096];
        match socket::recv(self.fd, &mut buf) {
            Ok(0) => {
                self.connected = false;
                self.event_queue.push(CollabEvent::SessionEnded);
                return;
            }
            Ok(n) => {
                self.decoder.feed(&buf[..n]);
            }
            Err(_) => return,
        }

        // Process complete messages
        while let Some((header, payload)) = self.decoder.next_message() {
            self.handle_msg(header.msg_type, &payload);
        }
    }

    fn handle_msg(&mut self, msg_type: MessageType, payload: &[u8]) {
        match msg_type {
            MessageType::Welcome => {
                if payload.len() >= 6 {
                    // peer_id at offset 0 (informational)
                    self.canvas_w = wire::get_u16(payload, 1);
                    self.canvas_h = wire::get_u16(payload, 3);
                    // peer_count at offset 5 (informational)
                }
            }

            MessageType::PeerJoined => {
                if payload.len() >= 2 {
                    let peer_id = wire::get_u8(payload, 0);
                    let name_len = wire::get_u8(payload, 1) as usize;
                    let name_len = name_len.min(32).min(payload.len() - 2);
                    let mut name = [0u8; 32];
                    name[..name_len].copy_from_slice(&payload[2..2 + name_len]);

                    self.peers.push(PeerInfo::new(peer_id, &name[..name_len]));
                    self.event_queue.push(CollabEvent::PeerJoined {
                        peer_id,
                        name,
                        name_len: name_len as u8,
                    });
                }
            }

            MessageType::PeerLeft => {
                if payload.len() >= 1 {
                    let peer_id = wire::get_u8(payload, 0);
                    self.peers.retain(|p| p.peer_id != peer_id);
                    self.event_queue.push(CollabEvent::PeerLeft { peer_id });
                }
            }

            MessageType::Bye => {
                self.connected = false;
                self.event_queue.push(CollabEvent::SessionEnded);
            }

            MessageType::SyncMeta => {
                // Start sync: canvas_w(u16), canvas_h(u16), total_chunks(u16), total_bytes(u32)
                if payload.len() >= 10 {
                    self.canvas_w = wire::get_u16(payload, 0);
                    self.canvas_h = wire::get_u16(payload, 2);
                }
            }

            MessageType::SyncChunk => {
                if payload.len() >= 6 {
                    // chunk_index(u16) + byte_offset(u32) + data
                    let byte_offset = wire::get_u32(payload, 2);
                    let data = payload[6..].to_vec();
                    self.event_queue.push(CollabEvent::SyncChunk {
                        offset: byte_offset,
                        data,
                    });
                }
            }

            MessageType::SyncEnd => {
                self.event_queue.push(CollabEvent::SyncComplete);
            }

            MessageType::Cursor => {
                if payload.len() >= 6 {
                    self.event_queue.push(CollabEvent::CursorMoved {
                        peer_id: wire::get_u8(payload, 0),
                        x: wire::get_i16(payload, 1),
                        y: wire::get_i16(payload, 3),
                        visible: wire::get_u8(payload, 5) != 0,
                    });
                }
            }

            MessageType::ToolChange => {
                if payload.len() >= 6 {
                    self.event_queue.push(CollabEvent::ToolChanged {
                        peer_id: wire::get_u8(payload, 0),
                        tool: wire::get_u8(payload, 1),
                        brush_size: wire::get_u8(payload, 2),
                        r: wire::get_u8(payload, 3),
                        g: wire::get_u8(payload, 4),
                        b: wire::get_u8(payload, 5),
                    });
                }
            }

            MessageType::Ping => {
                self.send_msg(MessageType::Pong, &[]);
            }

            MessageType::Pong => {} // Ignore

            _ if DrawOp::is_draw_op(msg_type) => {
                if let Some(op) = DrawOp::decode(msg_type, payload) {
                    self.event_queue.push(CollabEvent::DrawOp(op));
                }
            }

            _ => {} // Ignore unknown
        }
    }
}
