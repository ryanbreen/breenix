//! The socket object every socket descriptor refers to.
//!
//! A socket descriptor is `FdKind::Socket(Arc<Socket>)`. dup, fork and
//! SCM_RIGHTS clone the `Arc`; the protocol's `release` runs once, when the last
//! reference goes. Protocols never touch user memory, never raise signals and
//! never hold PROCESS_MANAGER: the syscall layer (syscall/socket.rs) copies
//! data, addresses and control messages, raises SIGPIPE and reports a pending
//! SO_ERROR.
//!
//! Lock order, outermost first: PROCESS_MANAGER (held only to resolve a
//! descriptor, never into a protocol call), the protocol registries, the
//! per-socket protocol state, `SocketCommon.opts` (a leaf), `wq`, the scheduler.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use super::addr::SockAddr;
use super::opts::SockOpts;
use super::tcp::TcpSock;
use super::udp::UdpSock;
use super::unix::UnixSock;
use crate::ipc::fd::{FdKind, FileDescriptor};
use crate::syscall::errno::{EBADF, ENOMEM, ENOPROTOOPT, ENOTSOCK, EOPNOTSUPP, ESRCH};
use crate::task::waitqueue::WaitQueueHead;

/// A socket: its identity, the state every protocol shares, and the protocol.
pub struct Socket {
    pub family: u16,
    pub kind: u16,
    pub protocol: u16,
    pub common: SocketCommon,
    pub proto: SocketProto,
}

/// The protocol half of a socket.
pub enum SocketProto {
    Udp(UdpSock),
    Tcp(TcpSock),
    Unix(UnixSock),
}

impl SocketProto {
    /// Move the protocol state out for a deferred release, leaving an inert
    /// value behind. Each adapter does this without allocating.
    fn detach(&mut self) -> SocketProto {
        match self {
            SocketProto::Udp(udp) => SocketProto::Udp(udp.detach()),
            SocketProto::Tcp(tcp) => SocketProto::Tcp(tcp.detach()),
            SocketProto::Unix(unix) => SocketProto::Unix(unix.detach()),
        }
    }
}

/// Sockets alive now. The release queue keeps capacity for each of them, so
/// a release deferred from Drop never allocates.
static LIVE_SOCKETS: AtomicUsize = AtomicUsize::new(0);

/// Sockets whose last reference went where a release may not run: under
/// PROCESS_MANAGER, in interrupt context or inside a scheduler scope.
static SOCKET_RELEASE_QUEUE: spin::Mutex<Vec<Socket>> = spin::Mutex::new(Vec::new());

/// A number for fstat's st_ino, unique per socket.
static NEXT_SOCKET_INO: AtomicU64 = AtomicU64::new(0x5000);

fn with_release_queue<R>(f: impl FnOnce(&mut Vec<Socket>) -> R) -> R {
    crate::arch_without_interrupts(|| f(&mut SOCKET_RELEASE_QUEUE.lock()))
}

impl Socket {
    /// A new socket. ENOMEM when the object or its release slot cannot be
    /// allocated; then no socket existed, so nothing was released, and the
    /// caller still owns whatever `proto` refers to.
    pub fn new(
        family: u16,
        kind: u16,
        protocol: u16,
        proto: SocketProto,
    ) -> Result<Arc<Socket>, i32> {
        let mut slot = Arc::<Socket>::try_new_uninit().map_err(|_| ENOMEM)?;
        with_release_queue(|queue| {
            let need = queue.len() + LIVE_SOCKETS.load(Ordering::Relaxed) + 1;
            if queue.capacity() < need {
                queue.try_reserve(need - queue.len()).map_err(|_| ENOMEM)?;
            }
            LIVE_SOCKETS.fetch_add(1, Ordering::Relaxed);
            Ok::<(), i32>(())
        })?;
        Arc::get_mut(&mut slot)
            .expect("a new Arc has no other reference")
            .write(Socket {
                family,
                kind,
                protocol,
                common: SocketCommon::new(),
                proto,
            });
        // SAFETY: the value was written just above.
        Ok(unsafe { slot.assume_init() })
    }

    /// The protocol's operations.
    pub fn ops(&self) -> &dyn ProtoOps {
        match &self.proto {
            SocketProto::Udp(udp) => udp,
            SocketProto::Tcp(tcp) => tcp,
            SocketProto::Unix(unix) => unix,
        }
    }

    /// Whether the protocol reports every state change through
    /// `common.notify()`. TCP (T2) and Unix (X2) still wake waiters by thread
    /// id; poll and select keep a short rescan for them until they move over.
    pub(crate) fn notifies(&self) -> bool {
        matches!(self.proto, SocketProto::Udp(_))
    }
}

fn release_may_run_here() -> bool {
    !(crate::per_cpu::in_interrupt()
        || crate::process::process_manager_held_on_current_cpu()
        || crate::tracing::providers::teardown::scheduler_scope_active())
}

impl Drop for Socket {
    fn drop(&mut self) {
        if self.common.released.load(Ordering::Relaxed) {
            return;
        }
        if release_may_run_here() {
            LIVE_SOCKETS.fetch_sub(1, Ordering::Relaxed);
            self.ops().release(self);
            return;
        }
        let carcass = Socket {
            family: self.family,
            kind: self.kind,
            protocol: self.protocol,
            common: SocketCommon::new(),
            proto: self.proto.detach(),
        };
        // Capacity for this push was reserved when the socket was created.
        // The count moves to the queue under its lock, so a concurrent
        // reservation never sees the socket in neither place.
        with_release_queue(|queue| {
            LIVE_SOCKETS.fetch_sub(1, Ordering::Relaxed);
            queue.push(carcass);
        });
    }
}

/// Release sockets whose last reference went under PROCESS_MANAGER or in
/// interrupt context. Called from the PM-free reclamation boundary; a bounded
/// pass leaves the rest for the next one.
pub(crate) fn drain_release_queue() {
    if !release_may_run_here() {
        return;
    }
    for _ in 0..64 {
        let Some(carcass) = with_release_queue(|queue| queue.pop()) else {
            break;
        };
        carcass.ops().release(&carcass);
        carcass.common.released.store(true, Ordering::Relaxed);
    }
}

/// The calling process's identity, for SO_PEERCRED and SCM credentials.
/// Takes PROCESS_MANAGER briefly: call it before any protocol lock.
pub fn caller_ucred() -> UCred {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return UCred::default();
    };
    let guard = crate::process::manager();
    guard
        .as_ref()
        .and_then(|manager| manager.find_process_by_thread(tid))
        .map(|(pid, process)| UCred {
            pid: process.thread_group_id.unwrap_or(pid.as_u64()) as u32,
            uid: process.cred.euid,
            gid: process.cred.egid,
        })
        .unwrap_or_default()
}

/// The calling process's id, for the protocol tables that record an owner.
pub(crate) fn caller_pid() -> crate::process::ProcessId {
    crate::process::ProcessId::new(u64::from(caller_ucred().pid))
}

/// Resolve `fd` to its socket under PROCESS_MANAGER and return with PM
/// released: EBADF for a closed descriptor, ENOTSOCK for one that is not a
/// socket. The second value is the descriptor's status flags (O_NONBLOCK).
pub fn socket_from_fd(fd: i32) -> Result<(Arc<Socket>, u32), i32> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let guard = crate::process::manager();
    let manager = guard.as_ref().ok_or(ESRCH)?;
    let (_, process) = manager.find_process_by_thread(tid).ok_or(ESRCH)?;
    let entry = process.fd_table.get(fd).ok_or(EBADF)?;
    match &entry.kind {
        FdKind::Socket(socket) => Ok((socket.clone(), entry.status_flags())),
        _ => Err(ENOTSOCK),
    }
}

/// A protocol's operations. `sk` is the socket the operation is on; the
/// syscall layer has already copied every user argument.
pub trait ProtoOps: Send + Sync {
    fn bind(&self, sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32>;
    fn listen(&self, _sk: &Arc<Socket>, _backlog: i32) -> Result<(), i32> {
        Err(EOPNOTSUPP)
    }
    fn connect(&self, sk: &Arc<Socket>, addr: &SockAddr, nonblock: bool) -> Result<(), i32>;
    fn accept(&self, _sk: &Arc<Socket>, _nonblock: bool) -> Result<(Arc<Socket>, SockAddr), i32> {
        Err(EOPNOTSUPP)
    }
    fn sendmsg(&self, sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32>;
    /// Bytes received; a datagram's whole length under MSG_TRUNC.
    fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32>;
    fn shutdown(&self, sk: &Arc<Socket>, how: u8) -> Result<(), i32>;
    fn local_name(&self, sk: &Socket) -> SockAddr;
    /// ENOTCONN when there is no peer.
    fn peer_name(&self, sk: &Socket) -> Result<SockAddr, i32>;
    /// POLLIN, POLLOUT, POLLERR, POLLHUP and POLLRDHUP bits.
    fn poll(&self, sk: &Socket) -> u16;
    fn is_listening(&self, _sk: &Socket) -> bool {
        false
    }
    fn getsockopt(
        &self,
        _sk: &Socket,
        _level: i32,
        _name: i32,
        _out: &mut OptBuf,
    ) -> Result<(), i32> {
        Err(ENOPROTOOPT)
    }
    fn setsockopt(&self, _sk: &Socket, _level: i32, _name: i32, _val: &[u8]) -> Result<(), i32> {
        Err(ENOPROTOOPT)
    }
    /// Thread context, PROCESS_MANAGER not held. Runs once per socket.
    fn release(&self, sk: &Socket);
}

/// One send: the bytes, the destination when the caller named one, MSG_*
/// flags, and the descriptors and credentials an AF_UNIX message carries.
/// A protocol that queues `rights` takes them out of the vector; the syscall
/// layer closes whatever is left.
pub struct SendMsg<'a> {
    pub data: &'a [u8],
    pub dest: Option<SockAddr>,
    pub flags: u32,
    pub nonblock: bool,
    pub rights: Vec<FileDescriptor>,
    pub cred: UCred,
}

/// One receive: room for `cap` bytes, which the protocol puts in `data`.
pub struct RecvMsg {
    pub cap: usize,
    pub data: Vec<u8>,
    pub flags: u32,
    pub nonblock: bool,
    pub out_flags: u32,
    pub from: Option<SockAddr>,
    pub rights: Vec<FileDescriptor>,
}

/// A getsockopt value of up to 16 bytes.
pub struct OptBuf {
    pub bytes: [u8; 16],
    pub len: usize,
}

impl OptBuf {
    pub const fn new() -> Self {
        OptBuf {
            bytes: [0; 16],
            len: 0,
        }
    }

    pub fn put_int(&mut self, value: i32) {
        self.bytes[..4].copy_from_slice(&value.to_ne_bytes());
        self.len = 4;
    }

    pub fn put(&mut self, value: &[u8]) {
        let n = value.len().min(self.bytes.len());
        self.bytes[..n].copy_from_slice(&value[..n]);
        self.len = n;
    }
}

impl Default for OptBuf {
    fn default() -> Self {
        Self::new()
    }
}

/// struct ucred: a process id and its effective user and group ids.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UCred {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

const SHUT_RD_BIT: u8 = 1;
const SHUT_WR_BIT: u8 = 2;

/// The state every protocol shares: options, the pending error, the shutdown
/// directions and the wait queue that `notify` wakes.
pub struct SocketCommon {
    opts: spin::Mutex<SockOpts>,
    so_error: AtomicI32,
    shut: AtomicU8,
    seq: AtomicU64,
    ino: u64,
    released: AtomicBool,
    pub wq: WaitQueueHead,
}

impl SocketCommon {
    fn new() -> Self {
        SocketCommon {
            opts: spin::Mutex::new(SockOpts::new()),
            so_error: AtomicI32::new(0),
            shut: AtomicU8::new(0),
            seq: AtomicU64::new(0),
            ino: NEXT_SOCKET_INO.fetch_add(1, Ordering::Relaxed),
            released: AtomicBool::new(false),
            wq: WaitQueueHead::new(),
        }
    }

    /// Run `f` on the options. The lock is a leaf and is held masked, since
    /// the receive path may read an option from the NetRx softirq.
    pub(crate) fn with_opts<R>(&self, f: impl FnOnce(&mut SockOpts) -> R) -> R {
        crate::arch_without_interrupts(|| f(&mut self.opts.lock()))
    }

    pub fn set_error(&self, errno: i32) {
        self.so_error.store(errno, Ordering::Release);
    }

    /// The pending error, cleared by reading it (SO_ERROR).
    pub fn take_error(&self) -> i32 {
        self.so_error.swap(0, Ordering::AcqRel)
    }

    pub(crate) fn peek_error(&self) -> i32 {
        self.so_error.load(Ordering::Acquire)
    }

    /// Record shutdown(`how`): SHUT_RD 0, SHUT_WR 1, SHUT_RDWR 2.
    pub fn set_shut(&self, how: u8) {
        let bits = match how {
            0 => SHUT_RD_BIT,
            1 => SHUT_WR_BIT,
            _ => SHUT_RD_BIT | SHUT_WR_BIT,
        };
        self.shut.fetch_or(bits, Ordering::AcqRel);
    }

    pub fn shut_rd(&self) -> bool {
        self.shut.load(Ordering::Acquire) & SHUT_RD_BIT != 0
    }

    pub fn shut_wr(&self) -> bool {
        self.shut.load(Ordering::Acquire) & SHUT_WR_BIT != 0
    }

    /// Publish a state change and wake every waiter. Thread or softirq
    /// context, with no protocol lock held.
    pub fn notify(&self) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        self.wq.wake_up();
    }

    /// `notify` for a caller under PROCESS_MANAGER: the wakes go through the
    /// scheduler's lock-free deferred path.
    pub fn notify_deferred(&self) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        self.wq.wake_up_deferred();
    }

    /// The number of state changes published so far.
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    pub fn reuseaddr(&self) -> bool {
        self.with_opts(|o| o.reuseaddr)
    }

    pub fn broadcast(&self) -> bool {
        self.with_opts(|o| o.broadcast)
    }

    pub fn keepalive(&self) -> bool {
        self.with_opts(|o| o.keepalive)
    }

    pub fn nodelay(&self) -> bool {
        self.with_opts(|o| o.nodelay)
    }

    /// Some(seconds) when SO_LINGER is on.
    pub fn linger(&self) -> Option<u32> {
        self.with_opts(|o| o.linger)
    }

    /// The receive buffer size, as SO_RCVBUF reports it (Linux's doubled value).
    pub fn rcvbuf(&self) -> usize {
        self.with_opts(|o| o.rcvbuf)
    }

    /// The send buffer size, as SO_SNDBUF reports it (Linux's doubled value).
    pub fn sndbuf(&self) -> usize {
        self.with_opts(|o| o.sndbuf)
    }

    /// When a receive started now gives up under SO_RCVTIMEO, in monotonic
    /// nanoseconds; None without a timeout.
    pub fn rcv_deadline(&self) -> Option<u64> {
        self.with_opts(|o| o.rcvtimeo_ns)
            .map(|timeout| super::wait::now_ns().saturating_add(timeout))
    }

    /// When a send started now gives up under SO_SNDTIMEO.
    pub fn snd_deadline(&self) -> Option<u64> {
        self.with_opts(|o| o.sndtimeo_ns)
            .map(|timeout| super::wait::now_ns().saturating_add(timeout))
    }

    /// fstat's st_ino for this socket.
    pub(crate) fn ino(&self) -> u64 {
        self.ino
    }
}
