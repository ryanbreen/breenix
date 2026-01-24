//! Unix domain socket implementation
//!
//! Provides AF_UNIX socket support for local inter-process communication.
//! Currently supports SOCK_STREAM via socketpair().

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// Default buffer size for Unix stream sockets (64 KB)
const UNIX_SOCKET_BUFFER_SIZE: usize = 65536;

/// Shared state for a Unix stream socket pair
///
/// This structure is shared between both endpoints of a socketpair.
/// Each endpoint writes to one buffer and reads from the other.
pub struct UnixStreamPair {
    /// Buffer A→B (endpoint A writes here, endpoint B reads from here)
    buffer_a_to_b: Mutex<VecDeque<u8>>,
    /// Buffer B→A (endpoint B writes here, endpoint A reads from here)
    buffer_b_to_a: Mutex<VecDeque<u8>>,
    /// Threads waiting to read on endpoint A (waiting for data in buffer_b_to_a)
    waiters_a: Mutex<Vec<u64>>,
    /// Threads waiting to read on endpoint B (waiting for data in buffer_a_to_b)
    waiters_b: Mutex<Vec<u64>>,
    /// Endpoint A closed
    closed_a: Mutex<bool>,
    /// Endpoint B closed
    closed_b: Mutex<bool>,
}

impl UnixStreamPair {
    /// Create a new Unix stream socket pair
    pub fn new() -> Self {
        UnixStreamPair {
            buffer_a_to_b: Mutex::new(VecDeque::with_capacity(UNIX_SOCKET_BUFFER_SIZE)),
            buffer_b_to_a: Mutex::new(VecDeque::with_capacity(UNIX_SOCKET_BUFFER_SIZE)),
            waiters_a: Mutex::new(Vec::new()),
            waiters_b: Mutex::new(Vec::new()),
            closed_a: Mutex::new(false),
            closed_b: Mutex::new(false),
        }
    }
}

impl Default for UnixStreamPair {
    fn default() -> Self {
        Self::new()
    }
}

/// Which endpoint of the socket pair this is
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnixEndpoint {
    A,
    B,
}

/// A Unix stream socket endpoint
///
/// This is a file descriptor wrapper around one end of a UnixStreamPair.
pub struct UnixStreamSocket {
    /// The shared socket pair state
    pub pair: Arc<UnixStreamPair>,
    /// Which endpoint this socket represents
    pub endpoint: UnixEndpoint,
    /// Non-blocking mode
    pub nonblocking: bool,
}

impl UnixStreamSocket {
    /// Create a new pair of connected Unix stream sockets
    pub fn new_pair(nonblocking: bool) -> (Arc<Mutex<Self>>, Arc<Mutex<Self>>) {
        let pair = Arc::new(UnixStreamPair::new());

        let socket_a = Arc::new(Mutex::new(UnixStreamSocket {
            pair: pair.clone(),
            endpoint: UnixEndpoint::A,
            nonblocking,
        }));

        let socket_b = Arc::new(Mutex::new(UnixStreamSocket {
            pair,
            endpoint: UnixEndpoint::B,
            nonblocking,
        }));

        (socket_a, socket_b)
    }

    /// Write data to the socket (sends to peer)
    ///
    /// Returns the number of bytes written, or an error code.
    pub fn write(&self, data: &[u8]) -> Result<usize, i32> {
        // Check if peer is closed
        let peer_closed = match self.endpoint {
            UnixEndpoint::A => *self.pair.closed_b.lock(),
            UnixEndpoint::B => *self.pair.closed_a.lock(),
        };

        if peer_closed {
            return Err(crate::syscall::errno::EPIPE);
        }

        // Get the buffer to write to (peer's read buffer)
        let buffer = match self.endpoint {
            UnixEndpoint::A => &self.pair.buffer_a_to_b,
            UnixEndpoint::B => &self.pair.buffer_b_to_a,
        };

        // Write data to buffer
        let mut buf = buffer.lock();

        // Check available space
        let available = UNIX_SOCKET_BUFFER_SIZE.saturating_sub(buf.len());
        if available == 0 {
            if self.nonblocking {
                return Err(crate::syscall::errno::EAGAIN);
            }
            // For blocking mode, we'd need to block here
            // For now, just write nothing and return EAGAIN
            return Err(crate::syscall::errno::EAGAIN);
        }

        let to_write = data.len().min(available);
        for &byte in &data[..to_write] {
            buf.push_back(byte);
        }

        drop(buf);

        // Wake waiting readers on the peer endpoint
        let waiters = match self.endpoint {
            UnixEndpoint::A => &self.pair.waiters_b,
            UnixEndpoint::B => &self.pair.waiters_a,
        };

        let waiter_ids: Vec<u64> = waiters.lock().clone();
        for thread_id in waiter_ids {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(thread_id);
            });
        }

        Ok(to_write)
    }

    /// Read data from the socket (receives from peer)
    ///
    /// Returns the number of bytes read, or an error code.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        // Get the buffer to read from
        let buffer = match self.endpoint {
            UnixEndpoint::A => &self.pair.buffer_b_to_a,
            UnixEndpoint::B => &self.pair.buffer_a_to_b,
        };

        let mut rx_buf = buffer.lock();

        if rx_buf.is_empty() {
            // Check if peer is closed
            let peer_closed = match self.endpoint {
                UnixEndpoint::A => *self.pair.closed_b.lock(),
                UnixEndpoint::B => *self.pair.closed_a.lock(),
            };

            if peer_closed {
                // EOF - peer closed and no more data
                return Ok(0);
            }

            if self.nonblocking {
                return Err(crate::syscall::errno::EAGAIN);
            }

            // For blocking mode, indicate no data available
            // The caller (sys_read) handles the blocking logic
            return Err(crate::syscall::errno::EAGAIN);
        }

        // Read available data
        let to_read = buf.len().min(rx_buf.len());
        for i in 0..to_read {
            buf[i] = rx_buf.pop_front().unwrap();
        }

        Ok(to_read)
    }

    /// Check if data is available for reading
    pub fn has_data(&self) -> bool {
        let buffer = match self.endpoint {
            UnixEndpoint::A => &self.pair.buffer_b_to_a,
            UnixEndpoint::B => &self.pair.buffer_a_to_b,
        };
        !buffer.lock().is_empty()
    }

    /// Check if peer has closed
    pub fn peer_closed(&self) -> bool {
        match self.endpoint {
            UnixEndpoint::A => *self.pair.closed_b.lock(),
            UnixEndpoint::B => *self.pair.closed_a.lock(),
        }
    }

    /// Register a thread as waiting for data
    pub fn register_waiter(&self, thread_id: u64) {
        let waiters = match self.endpoint {
            UnixEndpoint::A => &self.pair.waiters_a,
            UnixEndpoint::B => &self.pair.waiters_b,
        };
        let mut w = waiters.lock();
        if !w.contains(&thread_id) {
            w.push(thread_id);
        }
    }

    /// Unregister a thread from waiting
    pub fn unregister_waiter(&self, thread_id: u64) {
        let waiters = match self.endpoint {
            UnixEndpoint::A => &self.pair.waiters_a,
            UnixEndpoint::B => &self.pair.waiters_b,
        };
        waiters.lock().retain(|&id| id != thread_id);
    }

    /// Mark this endpoint as closed and wake any waiters on peer
    pub fn close(&self) {
        // Mark ourselves as closed
        match self.endpoint {
            UnixEndpoint::A => *self.pair.closed_a.lock() = true,
            UnixEndpoint::B => *self.pair.closed_b.lock() = true,
        }

        // Wake peer's waiters (they'll see EOF)
        let waiters = match self.endpoint {
            UnixEndpoint::A => &self.pair.waiters_b,
            UnixEndpoint::B => &self.pair.waiters_a,
        };

        let waiter_ids: Vec<u64> = waiters.lock().clone();
        for thread_id in waiter_ids {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(thread_id);
            });
        }
    }
}

impl core::fmt::Debug for UnixStreamSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UnixStreamSocket")
            .field("endpoint", &self.endpoint)
            .field("nonblocking", &self.nonblocking)
            .finish()
    }
}

// ============================================================================
// Named Unix Domain Socket Support
// ============================================================================

/// State of an unbound Unix socket (before it becomes a connected stream)
///
/// Note: Listening and Connected states don't exist here because the socket
/// transforms into a different type (UnixListener or UnixStream) when
/// those states are reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnixSocketState {
    /// Socket created but not bound to any path
    Unbound,
    /// Socket bound to a path but not listening
    Bound,
}

/// An unbound Unix domain socket
///
/// This represents a socket created with socket(AF_UNIX, SOCK_STREAM, 0)
/// before it has been connected or converted to a listener.
pub struct UnixSocket {
    /// Current state of the socket
    pub state: UnixSocketState,
    /// Non-blocking mode
    pub nonblocking: bool,
    /// Path this socket is bound to (None if unbound)
    pub bound_path: Option<Vec<u8>>,
}

impl UnixSocket {
    /// Create a new unbound Unix socket
    pub fn new(nonblocking: bool) -> Self {
        UnixSocket {
            state: UnixSocketState::Unbound,
            nonblocking,
            bound_path: None,
        }
    }

    /// Bind this socket to a path
    pub fn bind(&mut self, path: Vec<u8>) -> Result<(), i32> {
        if self.state != UnixSocketState::Unbound {
            return Err(crate::syscall::errno::EINVAL);
        }
        self.bound_path = Some(path);
        self.state = UnixSocketState::Bound;
        Ok(())
    }
}

impl core::fmt::Debug for UnixSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UnixSocket")
            .field("state", &self.state)
            .field("nonblocking", &self.nonblocking)
            .field("bound_path", &self.bound_path.as_ref().map(|p| p.len()))
            .finish()
    }
}

/// A listening Unix domain socket
///
/// This represents a socket that has called listen() and is ready
/// to accept incoming connections.
///
/// Note: Non-blocking mode is tracked via status_flags on the FileDescriptor,
/// not on this struct. Use fd_entry.status_flags & O_NONBLOCK to check.
pub struct UnixListener {
    /// Path this listener is bound to
    pub path: Vec<u8>,
    /// Maximum pending connections
    pub backlog: usize,
    /// Queue of pending connections (server-side of connected pairs)
    pub pending: VecDeque<Arc<Mutex<UnixStreamSocket>>>,
    /// Threads waiting in accept()
    pub waiting_threads: Mutex<Vec<u64>>,
}

impl UnixListener {
    /// Create a new listener from a bound socket
    pub fn new(path: Vec<u8>, backlog: usize) -> Self {
        UnixListener {
            path,
            backlog,
            pending: VecDeque::with_capacity(backlog.min(128)),
            waiting_threads: Mutex::new(Vec::new()),
        }
    }

    /// Check if there are pending connections
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Get the number of pending connections
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Push a new pending connection (server-side socket)
    /// Returns Err if backlog is full
    pub fn push_pending(&mut self, socket: Arc<Mutex<UnixStreamSocket>>) -> Result<(), i32> {
        if self.pending.len() >= self.backlog {
            return Err(crate::syscall::errno::ECONNREFUSED);
        }
        self.pending.push_back(socket);
        Ok(())
    }

    /// Pop a pending connection
    pub fn pop_pending(&mut self) -> Option<Arc<Mutex<UnixStreamSocket>>> {
        self.pending.pop_front()
    }

    /// Register a thread as waiting for a connection
    pub fn register_waiter(&self, thread_id: u64) {
        let mut waiters = self.waiting_threads.lock();
        if !waiters.contains(&thread_id) {
            waiters.push(thread_id);
        }
    }

    /// Unregister a thread from waiting
    pub fn unregister_waiter(&self, thread_id: u64) {
        self.waiting_threads.lock().retain(|&id| id != thread_id);
    }

    /// Wake all threads waiting for connections
    pub fn wake_waiters(&self) {
        let waiter_ids: Vec<u64> = self.waiting_threads.lock().clone();
        for thread_id in waiter_ids {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(thread_id);
            });
        }
    }
}

impl core::fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UnixListener")
            .field("path_len", &self.path.len())
            .field("backlog", &self.backlog)
            .field("pending", &self.pending.len())
            .finish()
    }
}
