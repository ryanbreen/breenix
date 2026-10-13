//! Unix domain socket implementation
//!
//! Provides AF_UNIX socket support for local inter-process communication.
//! Currently supports SOCK_STREAM, through socketpair() and through abstract
//! names that a listener binds.

use super::addr::{SockAddr, UnixName};
use super::object::{OptBuf, ProtoOps, RecvMsg, SendMsg, Socket, SocketProto, UCred};
use crate::syscall::errno;
use crate::task::thread::ThreadState;
use crate::task::waitqueue::WaitQueueHead;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};

const AF_UNIX: u16 = 1;
const SOCK_STREAM: u16 = 1;
const SOL_SOCKET: i32 = 1;
const SO_PEERCRED: i32 = 17;

const POLLIN: u16 = crate::ipc::poll::events::POLLIN as u16;
const POLLOUT: u16 = crate::ipc::poll::events::POLLOUT as u16;
const POLLHUP: u16 = crate::ipc::poll::events::POLLHUP as u16;

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
    /// Writers on each endpoint, independent of the legacy read waiters.
    write_waiters_a: Arc<WaitQueueHead>,
    write_waiters_b: Arc<WaitQueueHead>,
    /// Endpoint A closed
    closed_a: Mutex<bool>,
    /// Endpoint B closed
    closed_b: Mutex<bool>,
    /// Intrusive owned close work; no allocation or fixed-capacity wake buffer.
    pending_close: Mutex<PendingClose>,
}

impl UnixStreamPair {
    /// Create a new Unix stream socket pair
    pub fn new() -> Self {
        UnixStreamPair {
            buffer_a_to_b: Mutex::new(VecDeque::with_capacity(UNIX_SOCKET_BUFFER_SIZE)),
            buffer_b_to_a: Mutex::new(VecDeque::with_capacity(UNIX_SOCKET_BUFFER_SIZE)),
            waiters_a: Mutex::new(Vec::new()),
            waiters_b: Mutex::new(Vec::new()),
            write_waiters_a: Arc::new(WaitQueueHead::new()),
            write_waiters_b: Arc::new(WaitQueueHead::new()),
            closed_a: Mutex::new(false),
            closed_b: Mutex::new(false),
            pending_close: Mutex::new(PendingClose {
                queued: false,
                next: None,
            }),
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
}

impl UnixStreamSocket {
    /// Create a new pair of connected Unix stream sockets
    pub fn new_pair() -> Result<(Arc<Mutex<Self>>, Arc<Mutex<Self>>), i32> {
        let pair = Arc::try_new(UnixStreamPair::new()).map_err(|_| errno::ENOMEM)?;

        let socket_a = Arc::try_new(Mutex::new(UnixStreamSocket {
            pair: pair.clone(),
            endpoint: UnixEndpoint::A,
        }))
        .map_err(|_| errno::ENOMEM)?;

        let socket_b = Arc::try_new(Mutex::new(UnixStreamSocket {
            pair,
            endpoint: UnixEndpoint::B,
        }))
        .map_err(|_| errno::ENOMEM)?;

        Ok((socket_a, socket_b))
    }

    /// Snapshot the direction without retaining the endpoint mutex while asleep.
    pub(crate) fn writer(&self) -> UnixWriter {
        UnixWriter {
            pair: self.pair.clone(),
            endpoint: self.endpoint,
        }
    }

    /// Readiness and transfer share the guarded transmit-space definition.
    pub fn has_write_space(&self) -> bool {
        self.writer().state().has_write_space()
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

            // No data yet: the caller decides whether to wait.
            return Err(crate::syscall::errno::EAGAIN);
        }

        // Read available data
        let to_read = buf.len().min(rx_buf.len());
        for i in 0..to_read {
            buf[i] = rx_buf.pop_front().unwrap();
        }

        drop(rx_buf);
        if to_read > 0 {
            let writers = match self.endpoint {
                UnixEndpoint::A => &self.pair.write_waiters_b,
                UnixEndpoint::B => &self.pair.write_waiters_a,
            };
            writers.wake_up();
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
    pub fn close(&self) -> UnixCloseNotifications {
        // Mark ourselves as closed
        match self.endpoint {
            UnixEndpoint::A => *self.pair.closed_a.lock() = true,
            UnixEndpoint::B => *self.pair.closed_b.lock() = true,
        }

        UnixCloseNotifications {
            pair: self.pair.clone(),
        }
    }
}

struct PendingClose {
    queued: bool,
    next: Option<Arc<UnixStreamPair>>,
}

// Queue lock precedes the embedded link lock. Neither is held while delivering
// wakes, acquiring closed flags, or calling the scheduler. IRQ masking prevents
// a same-CPU fault/termination publisher from interrupting a queue operation.
static PENDING_CLOSES: Mutex<Option<Arc<UnixStreamPair>>> = Mutex::new(None);

/// Both peer-reader EOF and peer-writer EPIPE notifications own the pair until
/// delivery. Callers release the endpoint and PM guards first.
#[must_use = "deliver Unix close notifications after releasing object guards"]
pub struct UnixCloseNotifications {
    pair: Arc<UnixStreamPair>,
}

impl UnixCloseNotifications {
    pub fn deliver(self) {
        // FdTable destruction may run under PM or from fault cleanup.
        if crate::process::process_manager_held_on_current_cpu()
            || crate::per_cpu::in_interrupt()
            || crate::tracing::providers::teardown::scheduler_scope_active()
        {
            self.deliver_deferred();
            return;
        }
        let closed_a = *self.pair.closed_a.lock();
        let closed_b = *self.pair.closed_b.lock();
        for (closed, readers, writers) in [
            (closed_a, &self.pair.waiters_b, &self.pair.write_waiters_b),
            (closed_b, &self.pair.waiters_a, &self.pair.write_waiters_a),
        ] {
            if closed {
                let ids = core::mem::take(&mut *readers.lock());
                for tid in ids {
                    crate::task::scheduler::with_scheduler(|sched| {
                        sched.unblock(tid);
                    });
                }
                writers.wake_up();
            }
        }
    }

    /// Retain the pair in an intrusive queue for the existing thread-context
    /// reclamation boundary. Duplicate closes coalesce; no capacity can overflow.
    pub fn deliver_deferred(self) {
        crate::arch_without_interrupts(|| {
            let mut head = PENDING_CLOSES.lock();
            let mut pending = self.pair.pending_close.lock();
            if !pending.queued {
                pending.next = head.take();
                pending.queued = true;
                *head = Some(self.pair.clone());
            }
        });
    }
}

/// Called from the existing PM-free reclamation entry on both architectures.
/// A bounded pass leaves remaining pairs owned by the queue for the next pass.
pub(crate) fn drain_close_notifications() {
    if crate::process::process_manager_held_on_current_cpu()
        || crate::per_cpu::in_interrupt()
        || crate::tracing::providers::teardown::scheduler_scope_active()
    {
        return;
    }
    for _ in 0..64 {
        let pair = crate::arch_without_interrupts(|| {
            let mut head = PENDING_CLOSES.lock();
            let pair = head.take()?;
            {
                let mut pending = pair.pending_close.lock();
                *head = pending.next.take();
                pending.queued = false;
            }
            Some(pair)
        });
        let Some(pair) = pair else {
            break;
        };
        UnixCloseNotifications { pair }.deliver();
    }
}

/// Owned direction handle used after the descriptor and endpoint locks drop.
pub(crate) struct UnixWriter {
    pair: Arc<UnixStreamPair>,
    endpoint: UnixEndpoint,
}

/// Lock order: transmit buffer -> peer closed flag -> wait queue -> scheduler.
/// Keep both guards through publication so neither drain nor close loses a wake.
pub(crate) struct UnixWriteState<'a> {
    buffer: MutexGuard<'a, VecDeque<u8>>,
    peer_closed: MutexGuard<'a, bool>,
}

impl UnixWriteState<'_> {
    pub(crate) fn has_write_space(&self) -> bool {
        !*self.peer_closed && self.buffer.len() < UNIX_SOCKET_BUFFER_SIZE
    }

    pub(crate) fn peer_closed(&self) -> bool {
        *self.peer_closed
    }

    pub(crate) fn copy(&mut self, data: &[u8]) -> usize {
        if !self.has_write_space() {
            return 0;
        }
        let count = data.len().min(UNIX_SOCKET_BUFFER_SIZE - self.buffer.len());
        self.buffer.extend(&data[..count]);
        count
    }
}

impl UnixWriter {
    pub(crate) fn state(&self) -> UnixWriteState<'_> {
        let (buffer, closed) = match self.endpoint {
            UnixEndpoint::A => (&self.pair.buffer_a_to_b, &self.pair.closed_b),
            UnixEndpoint::B => (&self.pair.buffer_b_to_a, &self.pair.closed_a),
        };
        let buffer = buffer.lock();
        let peer_closed = closed.lock();
        UnixWriteState {
            buffer,
            peer_closed,
        }
    }

    pub(crate) fn queue(&self) -> Arc<WaitQueueHead> {
        match self.endpoint {
            UnixEndpoint::A => self.pair.write_waiters_a.clone(),
            UnixEndpoint::B => self.pair.write_waiters_b.clone(),
        }
    }

    #[cfg(feature = "boot_tests")]
    pub(crate) fn witness(&self, tid: u64) -> (u64, u64, u64) {
        let state = self.state();
        (
            self.queue().contains_waiter(tid) as u64,
            state.buffer.len() as u64,
            UNIX_SOCKET_BUFFER_SIZE as u64,
        )
    }

    pub(crate) fn wake_readers(&self) {
        let waiters = match self.endpoint {
            UnixEndpoint::A => &self.pair.waiters_b,
            UnixEndpoint::B => &self.pair.waiters_a,
        };
        let ids = waiters.lock().clone();
        for tid in ids {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(tid);
            });
        }
    }
}

impl core::fmt::Debug for UnixStreamSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UnixStreamSocket")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

// ============================================================================
// Named Unix Domain Socket Support
// ============================================================================

/// A listening Unix domain socket
///
/// This represents a socket that has called listen() and is ready
/// to accept incoming connections.
///
/// Note: Non-blocking mode is tracked via status_flags on the FileDescriptor,
/// not on this struct. Use fd_entry.status_flags() & O_NONBLOCK to check.
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

/// Stream writes return a positive partial count immediately; 4096 is not an
/// atomicity threshold. No guard survives entry to the prepared-wait helper.
/// A closed peer is EPIPE; the syscall layer raises SIGPIPE.
pub(crate) fn write_unix(
    writer: UnixWriter,
    data: &[u8],
    is_nonblocking: bool,
) -> Result<usize, i32> {
    let queue = writer.queue();
    loop {
        let outcome = {
            let mut state = writer.state();
            if state.peer_closed() {
                return Err(errno::EPIPE);
            }
            let count = state.copy(data);
            if count > 0 || data.is_empty() {
                drop(state);
                writer.wake_readers();
                // A later close cannot replace already copied progress with EPIPE.
                return Ok(count);
            }
            if is_nonblocking {
                return Err(errno::EAGAIN);
            }
            queue.prepare_to_wait_checked(ThreadState::BlockedOnIO, None, || {
                !state.peer_closed() && !state.has_write_space()
            })
        };
        crate::syscall::blocking_io::wait_prepared(&queue, outcome)?;
    }
}

// ============================================================================
// The AF_UNIX half of a socket
// ============================================================================

/// Where a Unix socket is in its life. A bound name is the registry key: a
/// NUL and then the abstract name.
enum UnixState {
    Unbound,
    Bound(Vec<u8>),
    Listening(Arc<Mutex<UnixListener>>),
    Connected(Arc<Mutex<UnixStreamSocket>>),
}

pub struct UnixSock {
    kind: u16,
    state: Mutex<UnixState>,
    /// The peer's identity, when it is known (socketpair).
    peer_cred: Option<UCred>,
}

/// The registry key for an abstract name.
fn abstract_key(name: &[u8]) -> Result<Vec<u8>, i32> {
    let mut key = Vec::new();
    key.try_reserve_exact(name.len() + 1)
        .map_err(|_| errno::ENOMEM)?;
    key.push(0);
    key.extend_from_slice(name);
    Ok(key)
}

fn name_of_key(key: &[u8]) -> SockAddr {
    let mut name = Vec::new();
    if name.try_reserve_exact(key.len().saturating_sub(1)).is_err() {
        return SockAddr::Unix(UnixName::Unnamed);
    }
    name.extend_from_slice(key.get(1..).unwrap_or(&[]));
    SockAddr::Unix(UnixName::Abstract(name))
}

impl UnixSock {
    /// Whether AF_UNIX sockets of this type can be created: socket() and
    /// socketpair() refuse the rest with EINVAL.
    pub fn supports_kind(kind: u16) -> bool {
        kind == SOCK_STREAM
    }

    pub fn new(kind: u16) -> UnixSock {
        UnixSock {
            kind,
            state: Mutex::new(UnixState::Unbound),
            peer_cred: None,
        }
    }

    fn connected(
        kind: u16,
        stream: Arc<Mutex<UnixStreamSocket>>,
        peer_cred: Option<UCred>,
    ) -> UnixSock {
        UnixSock {
            kind,
            state: Mutex::new(UnixState::Connected(stream)),
            peer_cred,
        }
    }

    /// A connected pair, as socketpair makes it. Each end's peer is the
    /// creating process.
    pub fn new_pair(kind: u16, cred: UCred) -> Result<(Arc<Socket>, Arc<Socket>), i32> {
        let (a, b) = UnixStreamSocket::new_pair()?;
        let first = Socket::new(
            AF_UNIX,
            kind,
            0,
            SocketProto::Unix(UnixSock::connected(kind, a, Some(cred))),
        )?;
        let second = Socket::new(
            AF_UNIX,
            kind,
            0,
            SocketProto::Unix(UnixSock::connected(kind, b, Some(cred))),
        )?;
        Ok((first, second))
    }

    /// The connected stream endpoint, for the blocking I/O oracle.
    #[cfg(feature = "boot_tests")]
    pub(crate) fn stream(&self) -> Option<Arc<Mutex<UnixStreamSocket>>> {
        match &*self.state.lock() {
            UnixState::Connected(stream) => Some(stream.clone()),
            _ => None,
        }
    }

    /// The state for a deferred release, leaving this one unbound.
    pub(super) fn detach(&mut self) -> UnixSock {
        UnixSock {
            kind: self.kind,
            state: Mutex::new(core::mem::replace(self.state.get_mut(), UnixState::Unbound)),
            peer_cred: self.peer_cred,
        }
    }

    fn stream_of(&self) -> Option<Arc<Mutex<UnixStreamSocket>>> {
        match &*self.state.lock() {
            UnixState::Connected(stream) => Some(stream.clone()),
            _ => None,
        }
    }
}

impl ProtoOps for UnixSock {
    fn bind(&self, _sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32> {
        let key = match addr {
            SockAddr::Unix(UnixName::Abstract(name)) => abstract_key(name)?,
            _ => return Err(errno::EINVAL),
        };
        let mut state = self.state.lock();
        if !matches!(*state, UnixState::Unbound) {
            return Err(errno::EINVAL);
        }
        if super::UNIX_SOCKET_REGISTRY.is_bound(&key) {
            return Err(errno::EADDRINUSE);
        }
        *state = UnixState::Bound(key);
        Ok(())
    }

    fn listen(&self, _sk: &Arc<Socket>, backlog: i32) -> Result<(), i32> {
        let mut state = self.state.lock();
        let UnixState::Bound(key) = &*state else {
            return Err(errno::EINVAL);
        };
        let listener = Arc::try_new(Mutex::new(UnixListener::new(
            key.clone(),
            backlog.max(0) as usize,
        )))
        .map_err(|_| errno::ENOMEM)?;
        super::UNIX_SOCKET_REGISTRY.bind(key.clone(), listener.clone())?;
        *state = UnixState::Listening(listener);
        Ok(())
    }

    fn accept(&self, sk: &Arc<Socket>, nonblock: bool) -> Result<(Arc<Socket>, SockAddr), i32> {
        let listener = match &*self.state.lock() {
            UnixState::Listening(listener) => listener.clone(),
            _ => return Err(errno::EOPNOTSUPP),
        };
        let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH)?;
        let accepted = super::wait::wait_until(sk, nonblock, None, || {
            let mut listener = listener.lock();
            listener.register_waiter(tid);
            listener.pop_pending().map(Ok)
        });
        listener.lock().unregister_waiter(tid);
        let stream = accepted?;
        match Socket::new(
            AF_UNIX,
            self.kind,
            0,
            SocketProto::Unix(UnixSock::connected(self.kind, stream.clone(), None)),
        ) {
            Ok(socket) => Ok((socket, SockAddr::Unix(UnixName::Unnamed))),
            Err(error) => {
                let notifications = stream.lock().close();
                notifications.deliver();
                Err(error)
            }
        }
    }

    fn connect(&self, _sk: &Arc<Socket>, addr: &SockAddr, _nonblock: bool) -> Result<(), i32> {
        let key = match addr {
            SockAddr::Unix(UnixName::Abstract(name)) => abstract_key(name)?,
            SockAddr::Unix(UnixName::Path(_)) => return Err(errno::ENOENT),
            _ => return Err(errno::EINVAL),
        };
        let mut state = self.state.lock();
        match *state {
            UnixState::Connected(_) => return Err(errno::EISCONN),
            UnixState::Listening(_) => return Err(errno::EOPNOTSUPP),
            UnixState::Unbound | UnixState::Bound(_) => {}
        }
        let listener = super::UNIX_SOCKET_REGISTRY
            .lookup(&key)
            .ok_or(errno::ECONNREFUSED)?;
        let (client, server) = UnixStreamSocket::new_pair()?;
        {
            let mut listener = listener.lock();
            listener.push_pending(server)?;
            listener.wake_waiters();
        }
        *state = UnixState::Connected(client);
        Ok(())
    }

    fn sendmsg(&self, _sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32> {
        let stream = self.stream_of().ok_or(errno::ENOTCONN)?;
        let writer = stream.lock().writer();
        write_unix(writer, msg.data, msg.nonblock)
    }

    fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32> {
        let stream = self.stream_of().ok_or(errno::ENOTCONN)?;
        if msg.data.try_reserve_exact(msg.cap).is_err() {
            return Err(errno::ENOMEM);
        }
        msg.data.resize(msg.cap, 0);
        let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH)?;
        let data = &mut msg.data;
        let result = super::wait::wait_until(sk, msg.nonblock, sk.common.rcv_deadline(), || {
            let socket = stream.lock();
            socket.register_waiter(tid);
            match socket.read(data) {
                Ok(n) => Some(Ok(n)),
                Err(errno::EAGAIN) => None,
                Err(error) => Some(Err(error)),
            }
        });
        stream.lock().unregister_waiter(tid);
        let received = result?;
        msg.data.truncate(received);
        Ok(received)
    }

    fn shutdown(&self, _sk: &Arc<Socket>, _how: u8) -> Result<(), i32> {
        Err(errno::EOPNOTSUPP)
    }

    fn local_name(&self, _sk: &Socket) -> SockAddr {
        match &*self.state.lock() {
            UnixState::Bound(key) => name_of_key(key),
            UnixState::Listening(listener) => name_of_key(&listener.lock().path),
            UnixState::Unbound | UnixState::Connected(_) => SockAddr::Unix(UnixName::Unnamed),
        }
    }

    fn peer_name(&self, _sk: &Socket) -> Result<SockAddr, i32> {
        match &*self.state.lock() {
            UnixState::Connected(_) => Ok(SockAddr::Unix(UnixName::Unnamed)),
            _ => Err(errno::ENOTCONN),
        }
    }

    fn poll(&self, _sk: &Socket) -> u16 {
        match &*self.state.lock() {
            UnixState::Connected(stream) => {
                let socket = stream.lock();
                let mut bits = 0;
                if socket.has_data() {
                    bits |= POLLIN;
                }
                if socket.has_write_space() {
                    bits |= POLLOUT;
                }
                if socket.peer_closed() && !socket.has_data() {
                    bits |= POLLHUP;
                }
                bits
            }
            UnixState::Listening(listener) => {
                if listener.lock().has_pending() {
                    POLLIN
                } else {
                    0
                }
            }
            // An unconnected socket can start a connect at once.
            UnixState::Unbound | UnixState::Bound(_) => POLLOUT,
        }
    }

    fn is_listening(&self, _sk: &Socket) -> bool {
        matches!(*self.state.lock(), UnixState::Listening(_))
    }

    fn getsockopt(&self, _sk: &Socket, level: i32, name: i32, out: &mut OptBuf) -> Result<(), i32> {
        match (level, name, self.peer_cred) {
            (SOL_SOCKET, SO_PEERCRED, Some(cred)) => {
                let mut bytes = [0u8; 12];
                bytes[..4].copy_from_slice(&cred.pid.to_ne_bytes());
                bytes[4..8].copy_from_slice(&cred.uid.to_ne_bytes());
                bytes[8..].copy_from_slice(&cred.gid.to_ne_bytes());
                out.put(&bytes);
                Ok(())
            }
            _ => Err(errno::ENOPROTOOPT),
        }
    }

    fn release(&self, _sk: &Socket) {
        let state = core::mem::replace(&mut *self.state.lock(), UnixState::Unbound);
        match state {
            UnixState::Connected(stream) => {
                let notifications = stream.lock().close();
                notifications.deliver();
            }
            UnixState::Listening(listener) => {
                super::UNIX_SOCKET_REGISTRY.unbind(&listener.lock().path);
                // Connections nobody accepted see their peer close.
                loop {
                    let Some(pending) = listener.lock().pop_pending() else {
                        break;
                    };
                    let notifications = pending.lock().close();
                    notifications.deliver();
                }
                listener.lock().wake_waiters();
            }
            UnixState::Unbound | UnixState::Bound(_) => {}
        }
    }
}
