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
    /// The socket accept makes from the listener `parent`: as on Linux, it
    /// starts with the listener's options (buffer sizes, timeouts, linger,
    /// keepalive, TCP_NODELAY and the rest).
    pub fn new_child(parent: &Socket, proto: SocketProto) -> Result<Arc<Socket>, i32> {
        let socket = Socket::new(parent.family, parent.kind, parent.protocol, proto)?;
        let opts = parent.common.with_opts(|o| o.clone());
        socket.common.with_opts(|o| *o = opts);
        Ok(socket)
    }

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
    /// `common.notify()`; poll and select keep a short rescan for one that
    /// does not.
    pub(crate) fn notifies(&self) -> bool {
        self.ops().notifies()
    }

    /// The protocol's readiness, with POLLIN while a receive left stream
    /// bytes in the stash, and POLLERR while an error is pending.
    pub fn poll(&self) -> u16 {
        let mut bits = self.ops().poll(self);
        if self.common.has_stash() {
            bits |= POLLIN;
        }
        if self.common.peek_error() != 0 {
            bits |= POLLERR;
        }
        bits
    }
}

const POLLIN: u16 = 0x001;
const POLLERR: u16 = 0x008;
const SOCK_STREAM: u16 = 1;
const MSG_PEEK: u32 = 0x2;

fn release_may_run_here() -> bool {
    !(crate::per_cpu::in_interrupt()
        || crate::process::process_manager_held_on_current_cpu()
        || crate::tracing::providers::teardown::scheduler_scope_active()
        || DEFER_RELEASE_DEPTH[defer_slot()].load(Ordering::Relaxed) != 0)
}

const DEFER_RELEASE_CPUS: usize = crate::tracing::MAX_CPUS;

/// Per CPU: open `DeferRelease` scopes.
static DEFER_RELEASE_DEPTH: [AtomicUsize; DEFER_RELEASE_CPUS] =
    [const { AtomicUsize::new(0) }; DEFER_RELEASE_CPUS];

fn defer_slot() -> usize {
    crate::task::softirqd::current_cpu().min(DEFER_RELEASE_CPUS - 1)
}

/// A region in which a socket's last reference goes to the release queue
/// rather than releasing inline: a waiter that has published its blocked
/// state and rechecks its condition with preemption disabled
/// (`wait::wait_until`). A release there could block or take the scheduler
/// as a blocked thread. Open and closed on the same CPU.
pub(crate) struct DeferRelease {
    slot: usize,
}

impl DeferRelease {
    pub(crate) fn enter() -> Self {
        let slot = defer_slot();
        DEFER_RELEASE_DEPTH[slot].fetch_add(1, Ordering::Relaxed);
        DeferRelease { slot }
    }
}

impl Drop for DeferRelease {
    fn drop(&mut self) {
        DEFER_RELEASE_DEPTH[self.slot].fetch_sub(1, Ordering::Relaxed);
    }
}

impl Socket {
    /// The protocol's release, then the descriptors a failed receive left in
    /// the stash. Thread context, PROCESS_MANAGER not held.
    fn finish(&self) {
        self.ops().release(self);
        self.common.discard_stash();
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if self.common.released.load(Ordering::Relaxed) {
            return;
        }
        if release_may_run_here() {
            LIVE_SOCKETS.fetch_sub(1, Ordering::Relaxed);
            self.finish();
            return;
        }
        // The carcass takes the socket's own common state, so the release
        // sees its linger, shutdown directions and pending error.
        let carcass = Socket {
            family: self.family,
            kind: self.kind,
            protocol: self.protocol,
            common: core::mem::replace(&mut self.common, SocketCommon::inert()),
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
        carcass.finish();
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
    /// A receive that hands the message to `deliver`, which copies it to the
    /// caller. A protocol that can leave the message queued until `deliver`
    /// succeeds overrides this. The default receives, and when `deliver`
    /// fails on a stream it puts the bytes and descriptors back in the
    /// socket's stash, ahead of the queue, as Linux leaves stream bytes it
    /// could not copy queued. A datagram that could not be copied is gone,
    /// as on Linux.
    fn recvmsg_with(
        &self,
        sk: &Arc<Socket>,
        msg: &mut RecvMsg,
        deliver: &mut dyn FnMut(&mut RecvMsg) -> Result<(), i32>,
    ) -> Result<usize, i32> {
        let received = self.recvmsg(sk, msg)?;
        if let Err(errno) = deliver(msg) {
            if sk.kind == SOCK_STREAM && msg.flags & MSG_PEEK == 0 {
                let data = core::mem::take(&mut msg.data);
                let rights = core::mem::take(&mut msg.rights);
                if let Err((data, rights)) = sk.common.put_back(data, rights) {
                    msg.data = data;
                    msg.rights = rights;
                }
            }
            return Err(errno);
        }
        Ok(received)
    }
    fn shutdown(&self, sk: &Arc<Socket>, how: u8) -> Result<(), i32>;
    fn local_name(&self, sk: &Socket) -> SockAddr;
    /// `local_name` for a protocol whose name may need memory: ENOMEM when
    /// it cannot be copied.
    fn try_local_name(&self, sk: &Socket) -> Result<SockAddr, i32> {
        Ok(self.local_name(sk))
    }
    /// ENOTCONN when there is no peer.
    fn peer_name(&self, sk: &Socket) -> Result<SockAddr, i32>;
    /// POLLIN, POLLOUT, POLLERR, POLLHUP and POLLRDHUP bits.
    fn poll(&self, sk: &Socket) -> u16;
    fn is_listening(&self, _sk: &Socket) -> bool {
        false
    }
    /// True when every change `poll` can see is published through
    /// `sk.common.notify()`, so a poll or select may sleep on `sk.common.wq`
    /// until its deadline. A protocol that still wakes waiters by thread id
    /// leaves this false and is rescanned every millisecond.
    fn notifies(&self) -> bool {
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
    /// A shared option (`socket::opts`) changed: the protocol applies it to
    /// state it already has (a connection's buffers, keepalive, Nagle).
    fn option_set(&self, _sk: &Socket, _level: i32, _name: i32) {}
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

/// Stream bytes a receive took from the protocol but could not copy to the
/// caller, and the descriptors that rode with them. The next receive takes
/// them before the protocol's queue, as Linux leaves bytes it could not copy
/// on the queue.
#[derive(Default)]
struct Stash {
    data: Vec<u8>,
    rights: Vec<FileDescriptor>,
}

/// The state every protocol shares: options, the pending error, the shutdown
/// directions and the wait queue that `notify` wakes.
pub struct SocketCommon {
    opts: spin::Mutex<SockOpts>,
    so_error: AtomicI32,
    shut: AtomicU8,
    seq: AtomicU64,
    rx_seq: AtomicU64,
    tx_seq: AtomicU64,
    ino: u64,
    released: AtomicBool,
    stash: spin::Mutex<Stash>,
    pub wq: WaitQueueHead,
}

impl SocketCommon {
    fn new() -> Self {
        SocketCommon {
            ino: NEXT_SOCKET_INO.fetch_add(1, Ordering::Relaxed),
            ..SocketCommon::inert()
        }
    }

    /// State left in a socket whose own moved to its release carcass.
    fn inert() -> Self {
        SocketCommon {
            opts: spin::Mutex::new(SockOpts::new()),
            so_error: AtomicI32::new(0),
            shut: AtomicU8::new(0),
            seq: AtomicU64::new(0),
            rx_seq: AtomicU64::new(0),
            tx_seq: AtomicU64::new(0),
            ino: 0,
            released: AtomicBool::new(false),
            stash: spin::Mutex::new(Stash {
                data: Vec::new(),
                rights: Vec::new(),
            }),
            wq: WaitQueueHead::new(),
        }
    }

    fn with_stash<R>(&self, f: impl FnOnce(&mut Stash) -> R) -> R {
        crate::arch_without_interrupts(|| f(&mut self.stash.lock()))
    }

    /// Whether a receive left bytes for the next one.
    pub(crate) fn has_stash(&self) -> bool {
        self.with_stash(|stash| !stash.data.is_empty())
    }

    /// Up to `cap` stashed bytes, and with them the stashed descriptors;
    /// `peek` leaves both in place and returns no descriptors. None when
    /// nothing is stashed; ENOMEM when a part cannot be split off.
    pub(crate) fn take_stash(
        &self,
        cap: usize,
        peek: bool,
    ) -> Option<Result<(Vec<u8>, Vec<FileDescriptor>), i32>> {
        self.with_stash(|stash| {
            if stash.data.is_empty() {
                return None;
            }
            if !peek && cap >= stash.data.len() {
                return Some(Ok((
                    core::mem::take(&mut stash.data),
                    core::mem::take(&mut stash.rights),
                )));
            }
            let n = cap.min(stash.data.len());
            let mut part = Vec::new();
            if part.try_reserve_exact(n).is_err() {
                return Some(Err(ENOMEM));
            }
            part.extend_from_slice(&stash.data[..n]);
            if peek {
                return Some(Ok((part, Vec::new())));
            }
            stash.data.drain(..n);
            Some(Ok((part, core::mem::take(&mut stash.rights))))
        })
    }

    /// Put bytes a receive could not copy back at the front of the stream,
    /// before anything stashed since. Gives them back when the two cannot be
    /// joined.
    pub(crate) fn put_back(
        &self,
        mut data: Vec<u8>,
        mut rights: Vec<FileDescriptor>,
    ) -> Result<(), (Vec<u8>, Vec<FileDescriptor>)> {
        let result = self.with_stash(|stash| {
            if data.try_reserve_exact(stash.data.len()).is_err()
                || rights.try_reserve_exact(stash.rights.len()).is_err()
            {
                return Err((data, rights));
            }
            data.append(&mut stash.data);
            rights.append(&mut stash.rights);
            stash.data = data;
            stash.rights = rights;
            Ok(())
        });
        if result.is_ok() {
            self.notify();
        }
        result
    }

    /// Close the descriptors a stash still holds. Thread context, no PM.
    fn discard_stash(&self) {
        let stash = self.with_stash(core::mem::take);
        for right in stash.rights {
            crate::task::process_task::close_extracted_fd(right);
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
    /// context, with no protocol lock and no PROCESS_MANAGER held.
    pub fn notify(&self) {
        self.rx_seq.fetch_add(1, Ordering::AcqRel);
        self.tx_seq.fetch_add(1, Ordering::AcqRel);
        self.seq.fetch_add(1, Ordering::AcqRel);
        self.wq.wake_up();
    }

    /// `notify` for a change on the receive side only (data, a FIN, a queued
    /// connection): an edge-triggered epoll watching only for output does
    /// not report it.
    pub fn notify_readable(&self) {
        self.rx_seq.fetch_add(1, Ordering::AcqRel);
        self.seq.fetch_add(1, Ordering::AcqRel);
        self.wq.wake_up();
    }

    /// `notify` for send space freed: an edge-triggered epoll watching only
    /// for input does not report it.
    pub fn notify_writable(&self) {
        self.tx_seq.fetch_add(1, Ordering::AcqRel);
        self.seq.fetch_add(1, Ordering::AcqRel);
        self.wq.wake_up();
    }

    /// The receive-side and send-side change counts, for edge-triggered
    /// epoll.
    pub(crate) fn edges(&self) -> (u64, u64) {
        (
            self.rx_seq.load(Ordering::Acquire),
            self.tx_seq.load(Ordering::Acquire),
        )
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

    /// IP_TTL: the time to live of the IPv4 packets the socket sends.
    pub fn ttl(&self) -> u8 {
        self.with_opts(|o| o.ttl)
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
