//! Unified CollabSession API that abstracts over host and client roles.

use libbreenix::error::Error;
use libbreenix::io::PollFd;
use libbreenix::socket::SockAddrIn;

use crate::client::ClientState;
use crate::event::{CollabEvent, DrawOp};
use crate::host::HostState;

enum Role {
    Host(HostState),
    Client(ClientState),
}

/// A collaboration session, either hosting or joined.
///
/// Integrates with the application's poll-based event loop:
/// 1. Call `poll_fds()` to get FDs to add to your poll set
/// 2. After poll returns, call `process_io()` with the results
/// 3. Drain events with `next_event()`
pub struct CollabSession {
    role: Role,
}

impl CollabSession {
    /// Create a host session, listening on the given port.
    ///
    /// `canvas_w` and `canvas_h` are sent to joining clients in the Welcome message.
    pub fn host(
        port: u16,
        name: &[u8],
        canvas_w: u16,
        canvas_h: u16,
    ) -> Result<Self, Error> {
        let state = HostState::new(port, name, canvas_w, canvas_h)?;
        Ok(Self {
            role: Role::Host(state),
        })
    }

    /// Join an existing session at the given address.
    pub fn join(addr: &SockAddrIn, name: &[u8]) -> Result<Self, Error> {
        let state = ClientState::new(addr, name)?;
        Ok(Self {
            role: Role::Client(state),
        })
    }

    /// Get file descriptors to add to the application's poll set.
    /// Returns the number of FDs written to `out`.
    pub fn poll_fds(&self, out: &mut [PollFd]) -> usize {
        match &self.role {
            Role::Host(h) => h.poll_fds(out),
            Role::Client(c) => c.poll_fds(out),
        }
    }

    /// Process I/O events after poll returns.
    /// Pass the poll results that include this session's FDs.
    pub fn process_io(&mut self, poll_results: &[PollFd]) {
        match &mut self.role {
            Role::Host(h) => h.process_io(poll_results),
            Role::Client(c) => c.process_io(poll_results),
        }
    }

    /// Get the next event from the session, if any.
    pub fn next_event(&mut self) -> Option<CollabEvent> {
        match &mut self.role {
            Role::Host(h) => h.next_event(),
            Role::Client(c) => c.next_event(),
        }
    }

    /// Send a draw operation to all peers.
    pub fn send_op(&mut self, op: &DrawOp) {
        match &mut self.role {
            Role::Host(h) => h.broadcast_draw_op(op),
            Role::Client(c) => c.send_draw_op(op),
        }
    }

    /// Send cursor position update to peers.
    pub fn send_cursor(&mut self, x: i16, y: i16, visible: bool) {
        match &mut self.role {
            Role::Host(h) => h.broadcast_cursor(x, y, visible),
            Role::Client(c) => c.send_cursor(x, y, visible),
        }
    }

    /// Send tool/color change to peers.
    pub fn send_tool_change(&mut self, tool: u8, size: u8, r: u8, g: u8, b: u8) {
        match &mut self.role {
            Role::Host(h) => h.broadcast_tool_change(tool, size, r, g, b),
            Role::Client(c) => c.send_tool_change(tool, size, r, g, b),
        }
    }

    /// Host-only: send canvas sync to a newly joined peer.
    /// Does nothing if this is a client session.
    pub fn send_canvas_sync(
        &mut self,
        peer_id: u8,
        canvas: &[u8],
        canvas_w: u16,
        canvas_h: u16,
    ) {
        if let Role::Host(host) = &mut self.role {
            host.send_canvas_sync(peer_id, canvas, canvas_w, canvas_h);
        }
    }

    /// Get the number of connected peers (excluding self).
    pub fn peer_count(&self) -> usize {
        match &self.role {
            Role::Host(h) => h.peer_count(),
            Role::Client(c) => c.peers().len(),
        }
    }

    /// Check if the session is still active.
    pub fn is_active(&self) -> bool {
        match &self.role {
            Role::Host(_) => true, // Host is always active
            Role::Client(c) => c.is_connected(),
        }
    }

    /// Whether this is the host role.
    pub fn is_host(&self) -> bool {
        matches!(self.role, Role::Host(_))
    }

    /// Get canvas dimensions (from Welcome, for clients).
    pub fn canvas_dims(&self) -> (u16, u16) {
        match &self.role {
            Role::Host(_) => (0, 0), // Host knows its own canvas
            Role::Client(c) => c.canvas_dims(),
        }
    }

    /// Disconnect and clean up.
    pub fn disconnect(&mut self) {
        match &mut self.role {
            Role::Host(h) => h.shutdown(),
            Role::Client(c) => c.disconnect(),
        }
    }
}
