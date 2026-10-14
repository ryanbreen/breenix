//! The TCP half of a socket: its place in the connection's life, and the
//! operations that move it along, over net/tcp.rs.
//!
//! A socket ties its connection or listener to itself (`tcp_set_owner`,
//! `tcp_listen`), so net/tcp.rs tells it of every change through
//! `common.notify()` and stores a reset's errno in its SO_ERROR. Every blocking
//! operation is a `wait_until` on the socket's wait queue.
//!
//! Where Linux departs from POSIX this follows Linux: listen on a socket that was
//! never bound binds it to an ephemeral port (POSIX allows EDESTADDRREQ), accept
//! on a socket that is not listening fails with EINVAL, send on a socket that
//! never connected fails with EPIPE (POSIX says ENOTCONN), a close with
//! SO_LINGER of 0 seconds, or with unread data, resets the connection, a socket
//! accept returns starts with the listener's options, connect to AF_UNSPEC
//! dissolves a connection, and MSG_TRUNC on a stream discards the bytes.

use alloc::sync::Arc;
use spin::Mutex;

use super::addr::SockAddr;
use super::object::{ProtoOps, RecvMsg, SendMsg, Socket, SocketProto};
use super::wait::{now_ns, wait_until};
use crate::net::tcp::{self, ConnectionId, ListenerId, RecvInto, RecvResult};
use crate::syscall::errno;

const POLLIN: u16 = crate::ipc::poll::events::POLLIN as u16;
const POLLOUT: u16 = crate::ipc::poll::events::POLLOUT as u16;
const POLLHUP: u16 = crate::ipc::poll::events::POLLHUP as u16;
const POLLRDHUP: u16 = crate::ipc::poll::events::POLLRDHUP as u16;

const MSG_OOB: u32 = 0x1;
const MSG_PEEK: u32 = 0x2;
const MSG_TRUNC: u32 = 0x20;
const MSG_WAITALL: u32 = 0x100;

const SHUT_RD: u8 = 0;
const SHUT_WR: u8 = 1;

/// How often a close lingering under SO_LINGER looks at its connection, which
/// no longer tells the closing socket of its changes.
const LINGER_POLL_NS: u64 = 10_000_000;

#[derive(Clone, Copy, Debug)]
pub enum TcpSockState {
    Unbound,
    Bound { addr: [u8; 4], port: u16 },
    Listening { addr: [u8; 4], port: u16, id: ListenerId },
    Connected(ConnectionId),
}

struct Inner {
    state: TcpSockState,
    /// The address and port bind() gave, which connect to AF_UNSPEC returns
    /// the socket to.
    bound: Option<([u8; 4], u16)>,
}

pub struct TcpSock {
    inner: Mutex<Inner>,
}

impl TcpSock {
    pub fn new() -> TcpSock {
        TcpSock {
            inner: Mutex::new(Inner {
                state: TcpSockState::Unbound,
                bound: None,
            }),
        }
    }

    pub fn connected(conn: ConnectionId) -> TcpSock {
        TcpSock {
            inner: Mutex::new(Inner {
                state: TcpSockState::Connected(conn),
                bound: None,
            }),
        }
    }

    pub fn state(&self) -> TcpSockState {
        self.inner.lock().state
    }

    /// The connection this socket carries, once connect or accept made one.
    pub fn connection(&self) -> Option<ConnectionId> {
        match self.state() {
            TcpSockState::Connected(conn) => Some(conn),
            _ => None,
        }
    }

    /// The state for a deferred release, leaving this one unbound.
    pub(super) fn detach(&mut self) -> TcpSock {
        let inner = self.inner.get_mut();
        let state = core::mem::replace(&mut inner.state, TcpSockState::Unbound);
        TcpSock {
            inner: Mutex::new(Inner {
                state,
                bound: inner.bound.take(),
            }),
        }
    }

    /// connect to AF_UNSPEC (Linux's tcp_disconnect): abort a connection, with
    /// a reset where the peer needs one, or stop listening, and return to the
    /// address bind() gave, or to unbound.
    fn disconnect(&self, sk: &Socket) -> Result<(), i32> {
        let reuse = sk.common.reuseaddr();
        let mut inner = self.inner.lock();
        match inner.state {
            TcpSockState::Unbound | TcpSockState::Bound { .. } => return Ok(()),
            TcpSockState::Listening { id, .. } => tcp::tcp_unlisten(id),
            TcpSockState::Connected(conn) => {
                let _ = tcp::tcp_close_connection(&conn, true, reuse);
            }
        }
        inner.state = match inner.bound {
            Some((addr, port)) if tcp::tcp_bind_port(addr, port, reuse).is_ok() => {
                TcpSockState::Bound { addr, port }
            }
            _ => {
                inner.bound = None;
                TcpSockState::Unbound
            }
        };
        drop(inner);
        crate::net::drain_loopback_queue();
        Ok(())
    }
}

impl Default for TcpSock {
    fn default() -> Self {
        Self::new()
    }
}

/// True for an address a socket on this host may bind: INADDR_ANY, the
/// loopback network or the interface's address.
fn is_local_addr(addr: [u8; 4]) -> bool {
    addr == [0; 4] || addr[0] == 127 || addr == crate::net::config().ip_addr
}

/// Bring the connection up to date with the socket's SO_RCVBUF, SO_SNDBUF,
/// SO_KEEPALIVE and SO_REUSEADDR: when it is made, and whenever one of them
/// is set (`option_set`).
fn apply_options(sk: &Socket, conn: &ConnectionId) {
    let options = sk.common.with_opts(|o| tcp::ConnOptions {
        rcvbuf: o.rcvbuf,
        sndbuf: o.sndbuf,
        keepalive: o.keepalive,
        reuse: o.reuseaddr,
        ttl: o.ttl,
    });
    tcp::tcp_set_options(conn, options);
}

/// SO_LINGER with a positive time: wait, at most that long, for the peer to
/// acknowledge the FIN, as Linux's tcp_close does. A signal ends the wait.
fn linger_close(sk: &Socket, conn: &ConnectionId, seconds: u32) {
    // Only a process's own close lingers: a kernel thread that dropped the
    // last reference (ktcptimerd delivering a notice) must not stall on it.
    let user_thread = crate::task::scheduler::with_scheduler(|sched| {
        sched
            .current_thread_mut()
            .is_some_and(|thread| thread.privilege == crate::task::thread::ThreadPrivilege::User)
    })
    .unwrap_or(false);
    if !user_thread {
        return;
    }
    let deadline = now_ns().saturating_add(u64::from(seconds).saturating_mul(1_000_000_000));
    while tcp::tcp_close_pending(conn) {
        let now = now_ns();
        if now >= deadline {
            return;
        }
        let step = deadline.min(now.saturating_add(LINGER_POLL_NS));
        if wait_until(sk, false, Some(step), || None::<Result<(), i32>>) != Err(errno::EAGAIN) {
            return;
        }
        crate::net::drain_loopback_queue();
    }
}

impl ProtoOps for TcpSock {
    fn bind(&self, sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32> {
        let SockAddr::Inet { addr, port } = *addr else {
            return Err(errno::EAFNOSUPPORT);
        };
        let reuse = sk.common.reuseaddr();
        let mut inner = self.inner.lock();
        if !matches!(inner.state, TcpSockState::Unbound) {
            return Err(errno::EINVAL);
        }
        if !is_local_addr(addr) {
            return Err(errno::EADDRNOTAVAIL);
        }
        let port = if port == 0 {
            tcp::tcp_bind_ephemeral(addr)?
        } else {
            tcp::tcp_bind_port(addr, port, reuse)?;
            port
        };
        inner.state = TcpSockState::Bound { addr, port };
        inner.bound = Some((addr, port));
        Ok(())
    }

    fn listen(&self, sk: &Arc<Socket>, backlog: i32) -> Result<(), i32> {
        let backlog = backlog.max(0) as usize;
        let pid = super::object::caller_pid();
        let reuse = sk.common.reuseaddr();
        let ttl = sk.common.ttl();
        let mut inner = self.inner.lock();
        let (addr, port, autobound) = match inner.state {
            TcpSockState::Bound { addr, port } => (addr, port, false),
            // Linux binds a socket that was never bound to an ephemeral port.
            TcpSockState::Unbound => ([0; 4], tcp::tcp_bind_ephemeral([0; 4])?, true),
            // A second listen changes only the backlog.
            TcpSockState::Listening { id, .. } => {
                tcp::tcp_set_backlog(id, backlog);
                return Ok(());
            }
            TcpSockState::Connected(_) => return Err(errno::EINVAL),
        };
        match tcp::tcp_listen(
            addr,
            port,
            backlog,
            pid,
            reuse,
            ttl,
            Some(Arc::downgrade(sk)),
        ) {
            Ok(id) => {
                inner.state = TcpSockState::Listening { addr, port, id };
                Ok(())
            }
            // A socket that bound keeps its binding; an ephemeral one gives
            // the port back.
            Err(error) => {
                if autobound {
                    tcp::tcp_unbind_port(addr, port);
                }
                Err(error)
            }
        }
    }

    fn accept(&self, sk: &Arc<Socket>, nonblock: bool) -> Result<(Arc<Socket>, SockAddr), i32> {
        let TcpSockState::Listening { id, .. } = self.state() else {
            return Err(errno::EINVAL);
        };
        // SO_RCVTIMEO bounds accept as it bounds a receive, with EAGAIN.
        let deadline = sk.common.rcv_deadline();
        let conn = wait_until(sk, nonblock, deadline, || tcp::tcp_accept(id).map(Ok))?;
        let peer = SockAddr::Inet {
            addr: conn.remote_ip,
            port: conn.remote_port,
        };
        // The socket starts with the listener's options, as on Linux.
        match Socket::new_child(sk, SocketProto::Tcp(TcpSock::connected(conn))) {
            Ok(socket) => {
                tcp::tcp_set_owner(&conn, Arc::downgrade(&socket));
                apply_options(&socket, &conn);
                Ok((socket, peer))
            }
            Err(error) => {
                let _ = tcp::tcp_close_connection(&conn, true, false);
                crate::net::drain_loopback_queue();
                Err(error)
            }
        }
    }

    fn connect(&self, sk: &Arc<Socket>, addr: &SockAddr, nonblock: bool) -> Result<(), i32> {
        let (remote_ip, remote_port) = match *addr {
            SockAddr::Inet { addr, port } => (addr, port),
            SockAddr::Unspec => return self.disconnect(sk),
            _ => return Err(errno::EAFNOSUPPORT),
        };
        let pid = super::object::caller_pid();
        let ttl = sk.common.ttl();
        let conn = {
            let mut inner = self.inner.lock();
            let (bind_addr, local_port, autobound) = match inner.state {
                // An exhausted ephemeral range is EADDRNOTAVAIL for connect.
                TcpSockState::Unbound => match tcp::tcp_bind_ephemeral([0; 4]) {
                    Ok(port) => ([0; 4], port, true),
                    Err(errno::EADDRINUSE) => return Err(errno::EADDRNOTAVAIL),
                    Err(error) => return Err(error),
                },
                TcpSockState::Bound { addr, port } => (addr, port, false),
                TcpSockState::Connected(conn) => {
                    return Err(match tcp::tcp_get_state(&conn) {
                        Some(tcp::TcpState::SynSent | tcp::TcpState::SynReceived) => {
                            errno::EALREADY
                        }
                        _ => errno::EISCONN,
                    });
                }
                TcpSockState::Listening { .. } => return Err(errno::EISCONN),
            };
            // The connection takes over the binding, or the socket keeps it.
            let conn =
                match tcp::tcp_connect(bind_addr, local_port, remote_ip, remote_port, pid, ttl) {
                    Ok(conn) => conn,
                    Err(error) => {
                        if autobound {
                            tcp::tcp_unbind_port(bind_addr, local_port);
                        }
                        return Err(error);
                    }
                };
            tcp::tcp_set_owner(&conn, Arc::downgrade(sk));
            apply_options(sk, &conn);
            inner.state = TcpSockState::Connected(conn);
            conn
        };
        // The loopback SYN, SYN+ACK and ACK go through now, with no lock held.
        crate::net::drain_loopback_queue();
        if nonblock {
            // Linux reports EINPROGRESS even when a loopback handshake has
            // already finished; the outcome is read with poll and SO_ERROR.
            return Err(errno::EINPROGRESS);
        }
        // A refused or timed-out handshake leaves its errno in SO_ERROR; the
        // retransmission timer gives up with ETIMEDOUT. SO_SNDTIMEO bounds the
        // wait, as on Linux, after which the connect continues in the background.
        let result = wait_until(sk, false, sk.common.snd_deadline(), || {
            match tcp::tcp_get_state(&conn) {
                Some(tcp::TcpState::SynSent | tcp::TcpState::SynReceived) => None,
                Some(tcp::TcpState::Closed) | None => {
                    let error = sk.common.take_error();
                    Some(Err(if error != 0 { error } else { errno::ECONNREFUSED }))
                }
                Some(_) => Some(Ok(())),
            }
        });
        match result {
            Err(errno::EAGAIN) => Err(errno::EINPROGRESS),
            other => other,
        }
    }

    fn sendmsg(&self, sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32> {
        let Some(conn) = self.connection() else {
            return Err(errno::EPIPE);
        };
        let oob = msg.flags & MSG_OOB != 0;
        let deadline = sk.common.snd_deadline();
        let mut sent = 0;
        loop {
            let data = &msg.data[sent..];
            let result = wait_until(sk, msg.nonblock, deadline, || {
                match tcp::tcp_sendmsg(&conn, data, oob) {
                    Ok(n) => Some(Ok(n)),
                    // The send buffer is full, or the handshake has not finished.
                    Err(errno::EAGAIN) | Err(errno::ENOTCONN) => None,
                    // A reset while waiting is reported once, as its errno.
                    Err(errno::EPIPE) => {
                        let error = sk.common.take_error();
                        Some(Err(if error != 0 { error } else { errno::EPIPE }))
                    }
                    Err(error) => Some(Err(error)),
                }
            });
            crate::net::drain_loopback_queue();
            match result {
                Ok(n) => {
                    sent += n;
                    if sent == msg.data.len() || msg.nonblock {
                        return Ok(sent);
                    }
                }
                // What went before an interruption, a timeout or a reset counts.
                Err(_) if sent > 0 => return Ok(sent),
                Err(error) => return Err(error),
            }
        }
    }

    fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32> {
        let Some(conn) = self.connection() else {
            return Err(errno::ENOTCONN);
        };
        let peek = msg.flags & MSG_PEEK != 0;
        if msg.flags & MSG_OOB != 0 {
            return recv_oob(&conn, msg, peek);
        }
        // MSG_TRUNC on a stream takes the bytes without copying them.
        let discard = msg.flags & MSG_TRUNC != 0;
        if !discard {
            if msg.data.try_reserve_exact(msg.cap).is_err() {
                return Err(errno::ENOMEM);
            }
            msg.data.resize(msg.cap, 0);
        }
        if msg.cap == 0 {
            return Ok(0);
        }
        let waitall = msg.flags & MSG_WAITALL != 0;
        // MSG_WAITALL with MSG_PEEK waits until the whole request is queued.
        let min = if waitall && peek { msg.cap } else { 0 };
        let deadline = sk.common.rcv_deadline();
        let mut received = 0;
        loop {
            let result = wait_until(sk, msg.nonblock, deadline, || {
                let into = if discard {
                    RecvInto::Discard(msg.cap - received)
                } else {
                    RecvInto::Copy(&mut msg.data[received..])
                };
                match tcp::tcp_recvmsg(&conn, into, peek, min) {
                    Ok(RecvResult::Data(n)) => Some(Ok(n)),
                    Ok(RecvResult::Eof) => Some(Ok(0)),
                    // A reset or a timeout closed the connection: its errno is
                    // reported once, after the bytes before it, and the stream
                    // reads as ended after that.
                    Ok(RecvResult::Closed) if received > 0 => Some(Ok(0)),
                    Ok(RecvResult::Closed) => {
                        let error = sk.common.take_error();
                        Some(if error != 0 { Err(error) } else { Ok(0) })
                    }
                    Ok(RecvResult::Empty) => None,
                    Err(error) => Some(Err(error)),
                }
            });
            match result {
                Ok(0) => break,
                Ok(n) => {
                    received += n;
                    if !waitall || peek || received == msg.cap {
                        break;
                    }
                }
                Err(_) if received > 0 => break,
                Err(error) => return Err(error),
            }
        }
        // A window update the read made goes out now.
        crate::net::drain_loopback_queue();
        if !discard {
            msg.data.truncate(received);
        }
        Ok(received)
    }

    fn shutdown(&self, _sk: &Arc<Socket>, how: u8) -> Result<(), i32> {
        let Some(conn) = self.connection() else {
            return Err(errno::ENOTCONN);
        };
        tcp::tcp_shutdown(&conn, how != SHUT_WR, how != SHUT_RD);
        crate::net::drain_loopback_queue();
        Ok(())
    }

    fn local_name(&self, _sk: &Socket) -> SockAddr {
        match self.state() {
            TcpSockState::Unbound => SockAddr::Inet {
                addr: [0; 4],
                port: 0,
            },
            TcpSockState::Bound { addr, port } | TcpSockState::Listening { addr, port, .. } => {
                SockAddr::Inet { addr, port }
            }
            TcpSockState::Connected(conn) => SockAddr::Inet {
                addr: conn.local_ip,
                port: conn.local_port,
            },
        }
    }

    fn peer_name(&self, _sk: &Socket) -> Result<SockAddr, i32> {
        match self.state() {
            TcpSockState::Connected(conn) if tcp::tcp_has_peer(&conn) => Ok(SockAddr::Inet {
                addr: conn.remote_ip,
                port: conn.remote_port,
            }),
            _ => Err(errno::ENOTCONN),
        }
    }

    fn poll(&self, _sk: &Socket) -> u16 {
        match self.state() {
            // Linux: a socket in TCP_CLOSE with nothing shut is writable and hung up.
            TcpSockState::Unbound | TcpSockState::Bound { .. } => POLLOUT | POLLHUP,
            TcpSockState::Listening { id, .. } => {
                if tcp::tcp_has_pending(id) {
                    POLLIN
                } else {
                    0
                }
            }
            // A connection the table has let go of is closed both ways.
            TcpSockState::Connected(conn) => tcp::tcp_poll_events(&conn)
                .unwrap_or(POLLIN | POLLOUT | POLLHUP | POLLRDHUP),
        }
    }

    fn is_listening(&self, _sk: &Socket) -> bool {
        matches!(self.state(), TcpSockState::Listening { .. })
    }

    fn notifies(&self) -> bool {
        true
    }

    fn option_set(&self, sk: &Socket, _level: i32, _name: i32) {
        match self.state() {
            TcpSockState::Connected(conn) => apply_options(sk, &conn),
            TcpSockState::Listening { id, .. } => tcp::tcp_set_listener_ttl(id, sk.common.ttl()),
            _ => {}
        }
    }

    fn release(&self, sk: &Socket) {
        match self.state() {
            TcpSockState::Listening { id, .. } => tcp::tcp_unlisten(id),
            TcpSockState::Connected(conn) => {
                let linger = sk.common.linger();
                let _ =
                    tcp::tcp_close_connection(&conn, linger == Some(0), sk.common.reuseaddr());
                crate::net::drain_loopback_queue();
                if let Some(seconds) = linger.filter(|&seconds| seconds > 0) {
                    linger_close(sk, &conn, seconds);
                }
            }
            TcpSockState::Bound { addr, port } => tcp::tcp_unbind_port(addr, port),
            TcpSockState::Unbound => {}
        }
        // The FIN or RST goes out now.
        crate::net::drain_loopback_queue();
    }
}

/// recv(MSG_OOB): the urgent byte, flagged MSG_OOB, or MSG_TRUNC when the
/// buffer has no room for it (it is read all the same, as on Linux).
fn recv_oob(conn: &ConnectionId, msg: &mut RecvMsg, peek: bool) -> Result<usize, i32> {
    let copy = msg.cap > 0 && msg.flags & MSG_TRUNC == 0;
    if copy && msg.data.try_reserve_exact(1).is_err() {
        return Err(errno::ENOMEM);
    }
    let Some(byte) = tcp::tcp_recv_oob(conn, peek)? else {
        return Ok(0);
    };
    msg.out_flags |= MSG_OOB;
    if msg.cap == 0 {
        msg.out_flags |= MSG_TRUNC;
        return Ok(0);
    }
    if copy {
        msg.data.push(byte);
    }
    Ok(1)
}
