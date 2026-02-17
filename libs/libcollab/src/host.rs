//! Host-side session logic: listen, accept, relay, canvas sync.

use libbreenix::error::Error;
use libbreenix::io::{self, PollFd, poll_events};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM, SOCK_NONBLOCK};
use libbreenix::types::Fd;

use crate::event::{CollabEvent, DrawOp};
use crate::wire::{self, MessageType, StreamDecoder, HEADER_SIZE, MAX_NAME_LEN, PROTOCOL_VERSION};

/// Maximum number of connected clients
const MAX_CLIENTS: usize = 15;

/// Sync chunk size (60KB)
const SYNC_CHUNK_SIZE: usize = 60 * 1024;

/// Per-client state managed by the host
struct ClientSlot {
    fd: Fd,
    peer_id: u8,
    decoder: StreamDecoder,
    name: [u8; 32],
    name_len: u8,
    /// True once Hello has been received and Welcome sent
    ready: bool,
    /// True while canvas sync is in progress for this client
    syncing: bool,
}

/// Host session state
pub struct HostState {
    listen_fd: Fd,
    clients: Vec<Option<ClientSlot>>,
    next_seqno: u32,
    canvas_w: u16,
    canvas_h: u16,
    event_queue: Vec<CollabEvent>,
}

impl HostState {
    /// Create a new host, binding to the given port.
    pub fn new(port: u16, _name: &[u8], canvas_w: u16, canvas_h: u16) -> Result<Self, Error> {
        let listen_fd = socket::socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0)?;
        let addr = SockAddrIn::new([0, 0, 0, 0], port);
        socket::bind_inet(listen_fd, &addr)?;
        socket::listen(listen_fd, 16)?;

        let mut clients = Vec::with_capacity(MAX_CLIENTS);
        for _ in 0..MAX_CLIENTS {
            clients.push(None);
        }

        Ok(Self {
            listen_fd,
            clients,
            next_seqno: 0,
            canvas_w,
            canvas_h,
            event_queue: Vec::new(),
        })
    }

    /// Fill poll FDs for the host's sockets.
    /// Returns the number of FDs written.
    pub fn poll_fds(&self, out: &mut [PollFd]) -> usize {
        let mut n = 0;
        if n < out.len() {
            out[n] = PollFd::new(self.listen_fd, poll_events::POLLIN);
            n += 1;
        }
        for slot in &self.clients {
            if let Some(c) = slot {
                if n < out.len() {
                    out[n] = PollFd::new(c.fd, poll_events::POLLIN);
                    n += 1;
                }
            }
        }
        n
    }

    /// Process I/O after poll returns.
    pub fn process_io(&mut self, poll_results: &[PollFd]) {
        // Check listen socket for new connections
        for pfd in poll_results {
            if pfd.fd == self.listen_fd.raw() as i32
                && (pfd.revents & poll_events::POLLIN) != 0
            {
                self.try_accept();
            }
        }

        // Process each client's data
        // Collect client FDs first to avoid borrow issues
        let client_fds: Vec<(usize, i32)> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|c| (i, c.fd.raw() as i32)))
            .collect();

        for (idx, fd_raw) in client_fds {
            for pfd in poll_results {
                if pfd.fd == fd_raw {
                    if (pfd.revents & (poll_events::POLLHUP | poll_events::POLLERR)) != 0 {
                        self.remove_client(idx);
                    } else if (pfd.revents & poll_events::POLLIN) != 0 {
                        self.read_client(idx);
                    }
                    break;
                }
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

    /// Send a draw op to all connected clients.
    pub fn broadcast_draw_op(&mut self, op: &DrawOp) {
        let mut payload = [0u8; 64];
        let (msg_type, plen) = op.encode(&mut payload);
        self.broadcast_msg(msg_type, &payload[..plen], None);
    }

    /// Send cursor update to all clients.
    pub fn broadcast_cursor(&mut self, x: i16, y: i16, visible: bool) {
        let mut payload = [0u8; 6];
        let mut off = 0;
        off = wire::put_u8(&mut payload, off, 0); // peer_id 0 = host
        off = wire::put_i16(&mut payload, off, x);
        off = wire::put_i16(&mut payload, off, y);
        wire::put_u8(&mut payload, off, if visible { 1 } else { 0 });
        self.broadcast_msg(MessageType::Cursor, &payload[..6], None);
    }

    /// Send tool change to all clients.
    pub fn broadcast_tool_change(&mut self, tool: u8, size: u8, r: u8, g: u8, b: u8) {
        let mut payload = [0u8; 6];
        let mut off = 0;
        off = wire::put_u8(&mut payload, off, 0); // peer_id 0 = host
        off = wire::put_u8(&mut payload, off, tool);
        off = wire::put_u8(&mut payload, off, size);
        off = wire::put_u8(&mut payload, off, r);
        off = wire::put_u8(&mut payload, off, g);
        wire::put_u8(&mut payload, off, b);
        self.broadcast_msg(MessageType::ToolChange, &payload[..6], None);
    }

    /// Send canvas sync to a specific peer (called when a new client joins).
    pub fn send_canvas_sync(&mut self, peer_id: u8, canvas: &[u8], w: u16, h: u16) {
        let slot_idx = (peer_id - 1) as usize;
        if slot_idx >= self.clients.len() {
            return;
        }
        let fd = match &self.clients[slot_idx] {
            Some(c) => c.fd,
            None => return,
        };

        // SyncMeta: canvas_w(u16), canvas_h(u16), total_chunks(u16), total_bytes(u32)
        let total_bytes = canvas.len();
        let total_chunks =
            (total_bytes + SYNC_CHUNK_SIZE - 1) / SYNC_CHUNK_SIZE;
        let mut meta = [0u8; 10];
        let mut off = 0;
        off = wire::put_u16(&mut meta, off, w);
        off = wire::put_u16(&mut meta, off, h);
        off = wire::put_u16(&mut meta, off, total_chunks as u16);
        wire::put_u32(&mut meta, off, total_bytes as u32);
        self.send_to_fd(fd, MessageType::SyncMeta, &meta[..10]);

        // SyncChunks
        for (chunk_idx, chunk) in canvas.chunks(SYNC_CHUNK_SIZE).enumerate() {
            let byte_offset = chunk_idx * SYNC_CHUNK_SIZE;
            // chunk header: chunk_index(u16) + byte_offset(u32)
            let mut hdr = [0u8; 6];
            let mut off = 0;
            off = wire::put_u16(&mut hdr, off, chunk_idx as u16);
            wire::put_u32(&mut hdr, off, byte_offset as u32);
            // Combine header + chunk data
            let mut payload = Vec::with_capacity(6 + chunk.len());
            payload.extend_from_slice(&hdr[..6]);
            payload.extend_from_slice(chunk);
            self.send_to_fd(fd, MessageType::SyncChunk, &payload);
        }

        // SyncEnd
        self.send_to_fd(fd, MessageType::SyncEnd, &[]);

        if let Some(slot) = &mut self.clients[slot_idx] {
            slot.syncing = false;
        }
    }

    /// Get number of connected (ready) clients.
    pub fn peer_count(&self) -> usize {
        self.clients.iter().filter(|s| s.as_ref().is_some_and(|c| c.ready)).count()
    }

    /// Disconnect all clients and close the listen socket.
    pub fn shutdown(&mut self) {
        // Send Bye to all clients
        self.broadcast_msg(MessageType::Bye, &[], None);
        // Close all client FDs
        for slot in &mut self.clients {
            if let Some(c) = slot.take() {
                let _ = io::close(c.fd);
            }
        }
        let _ = io::close(self.listen_fd);
    }

    // ---- Internal helpers ----

    fn next_seq(&mut self) -> u32 {
        let s = self.next_seqno;
        self.next_seqno = self.next_seqno.wrapping_add(1);
        s
    }

    fn try_accept(&mut self) {
        // Find a free slot
        let free_idx = self.clients.iter().position(|s| s.is_none());
        let free_idx = match free_idx {
            Some(i) => i,
            None => return, // Full
        };

        match socket::accept(self.listen_fd, None) {
            Ok(client_fd) => {
                // Set non-blocking
                let _ = io::fcntl_getfl(client_fd).and_then(|flags| {
                    io::fcntl_setfl(client_fd, flags as i32 | io::status_flags::O_NONBLOCK)
                });

                let peer_id = (free_idx + 1) as u8;
                self.clients[free_idx] = Some(ClientSlot {
                    fd: client_fd,
                    peer_id,
                    decoder: StreamDecoder::new(),
                    name: [0; 32],
                    name_len: 0,
                    ready: false,
                    syncing: false,
                });
            }
            Err(_) => {} // EAGAIN or error
        }
    }

    fn remove_client(&mut self, idx: usize) {
        if let Some(c) = self.clients[idx].take() {
            let _ = io::close(c.fd);
            if c.ready {
                // Broadcast PeerLeft
                let payload = [c.peer_id];
                self.broadcast_msg(MessageType::PeerLeft, &payload, None);
                self.event_queue.push(CollabEvent::PeerLeft {
                    peer_id: c.peer_id,
                });
            }
        }
    }

    fn read_client(&mut self, idx: usize) {
        let mut buf = [0u8; 4096];
        let fd = match &self.clients[idx] {
            Some(c) => c.fd,
            None => return,
        };

        match socket::recv(fd, &mut buf) {
            Ok(0) => {
                self.remove_client(idx);
                return;
            }
            Ok(n) => {
                if let Some(slot) = &mut self.clients[idx] {
                    slot.decoder.feed(&buf[..n]);
                }
            }
            Err(_) => return,
        }

        // Process complete messages
        loop {
            let msg = {
                match &mut self.clients[idx] {
                    Some(slot) => slot.decoder.next_message(),
                    None => break,
                }
            };

            match msg {
                Some((header, payload)) => {
                    self.handle_client_msg(idx, header.msg_type, header.seqno, &payload);
                }
                None => break,
            }
        }
    }

    fn handle_client_msg(
        &mut self,
        idx: usize,
        msg_type: MessageType,
        _seqno: u32,
        payload: &[u8],
    ) {
        let peer_id = match &self.clients[idx] {
            Some(c) => c.peer_id,
            None => return,
        };

        match msg_type {
            MessageType::Hello => {
                if payload.len() < 3 {
                    return;
                }
                let version = wire::get_u16(payload, 0);
                if version != PROTOCOL_VERSION {
                    self.remove_client(idx);
                    return;
                }
                let name_len = wire::get_u8(payload, 2) as usize;
                let name_len = name_len.min(MAX_NAME_LEN).min(payload.len() - 3);
                let mut name = [0u8; 32];
                name[..name_len].copy_from_slice(&payload[3..3 + name_len]);

                if let Some(slot) = &mut self.clients[idx] {
                    slot.name = name;
                    slot.name_len = name_len as u8;
                    slot.ready = true;
                    slot.syncing = true;
                }

                // Send Welcome: peer_id(u8), canvas_w(u16), canvas_h(u16), peer_count(u8)
                let mut welcome = [0u8; 6];
                let mut off = 0;
                off = wire::put_u8(&mut welcome, off, peer_id);
                off = wire::put_u16(&mut welcome, off, self.canvas_w);
                off = wire::put_u16(&mut welcome, off, self.canvas_h);
                wire::put_u8(&mut welcome, off, self.peer_count() as u8);
                self.send_to_idx(idx, MessageType::Welcome, &welcome[..6]);

                // Broadcast PeerJoined to all OTHER clients
                let mut joined_payload = [0u8; 34];
                let mut off = 0;
                off = wire::put_u8(&mut joined_payload, off, peer_id);
                off = wire::put_u8(&mut joined_payload, off, name_len as u8);
                joined_payload[off..off + name_len].copy_from_slice(&name[..name_len]);
                let plen = off + name_len;
                self.broadcast_msg(
                    MessageType::PeerJoined,
                    &joined_payload[..plen],
                    Some(idx),
                );

                self.event_queue.push(CollabEvent::PeerJoined {
                    peer_id,
                    name,
                    name_len: name_len as u8,
                });
            }

            MessageType::Bye => {
                self.remove_client(idx);
            }

            MessageType::Cursor => {
                if payload.len() >= 5 {
                    // Rewrite peer_id and relay
                    let mut relayed = [0u8; 6];
                    relayed[0] = peer_id;
                    relayed[1..].copy_from_slice(&payload[..5]);
                    self.broadcast_msg(MessageType::Cursor, &relayed[..6], Some(idx));

                    let x = wire::get_i16(payload, 0);
                    let y = wire::get_i16(payload, 2);
                    let visible = wire::get_u8(payload, 4) != 0;
                    self.event_queue.push(CollabEvent::CursorMoved {
                        peer_id,
                        x,
                        y,
                        visible,
                    });
                }
            }

            MessageType::ToolChange => {
                if payload.len() >= 5 {
                    let mut relayed = [0u8; 6];
                    relayed[0] = peer_id;
                    relayed[1..6].copy_from_slice(&payload[..5]);
                    self.broadcast_msg(MessageType::ToolChange, &relayed[..6], Some(idx));

                    self.event_queue.push(CollabEvent::ToolChanged {
                        peer_id,
                        tool: wire::get_u8(payload, 0),
                        brush_size: wire::get_u8(payload, 1),
                        r: wire::get_u8(payload, 2),
                        g: wire::get_u8(payload, 3),
                        b: wire::get_u8(payload, 4),
                    });
                }
            }

            MessageType::Ping => {
                self.send_to_idx(idx, MessageType::Pong, &[]);
            }

            _ if DrawOp::is_draw_op(msg_type) => {
                // Relay draw op to all other clients
                self.broadcast_msg(msg_type, payload, Some(idx));
                // Emit event for the host's canvas
                if let Some(op) = DrawOp::decode(msg_type, payload) {
                    self.event_queue.push(CollabEvent::DrawOp(op));
                }
            }

            _ => {} // Ignore unknown messages
        }
    }

    fn send_to_fd(&mut self, fd: Fd, msg_type: MessageType, payload: &[u8]) {
        let mut buf = vec![0u8; HEADER_SIZE + payload.len()];
        let seq = self.next_seq();
        if let Some(total) = wire::encode(&mut buf, msg_type, seq, payload) {
            let _ = socket::send(fd, &buf[..total]);
        }
    }

    fn send_to_idx(&mut self, idx: usize, msg_type: MessageType, payload: &[u8]) {
        let fd = match &self.clients[idx] {
            Some(c) => c.fd,
            None => return,
        };
        self.send_to_fd(fd, msg_type, payload);
    }

    /// Broadcast a message to all connected (ready) clients, optionally excluding one.
    fn broadcast_msg(
        &mut self,
        msg_type: MessageType,
        payload: &[u8],
        exclude_idx: Option<usize>,
    ) {
        let mut buf = vec![0u8; HEADER_SIZE + payload.len()];
        let seq = self.next_seq();
        let total = match wire::encode(&mut buf, msg_type, seq, payload) {
            Some(t) => t,
            None => return,
        };

        for (i, slot) in self.clients.iter().enumerate() {
            if let Some(exclude) = exclude_idx {
                if i == exclude {
                    continue;
                }
            }
            if let Some(c) = slot {
                if c.ready {
                    let _ = socket::send(c.fd, &buf[..total]);
                }
            }
        }
    }
}
