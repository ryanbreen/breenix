//! The TCP half of a socket: its place in the connection's life, and the
//! operations that move it along, over net/tcp.rs.
//!
//! A socket ties its connection or listener to itself (`tcp_set_owner`,
//! `tcp_set_listener_owner`), so net/tcp.rs tells it of every change through
//! `common.notify()` and hands a reset's errno to its SO_ERROR. Every blocking
//! operation is a `wait_until` on the socket's wait queue.
//!
//! Where Linux departs from POSIX this follows Linux: listen on a socket that was
//! never bound binds it to an ephemeral port (POSIX allows EDESTADDRREQ), accept
//! on a socket that is not listening fails with EINVAL, send on a socket that
//! never connected fails with EPIPE (POSIX says ENOTCONN), and a close with
//! SO_LINGER of 0 seconds, or with unread data, resets the connection.

use alloc::sync::Arc;
use spin::Mutex;

use super::addr::SockAddr;
use super::object::{ProtoOps, RecvMsg, SendMsg, Socket, SocketProto};
use super::wait::wait_until;
use crate::net::tcp::{self, ConnectionId};
use crate::syscall::errno;

const AF_INET: u16 = 2;
const SOCK_STREAM: u16 = 1;
const IPPROTO_TCP: u16 = 6;

const POLLIN: u16 = crate::ipc::poll::events::POLLIN as u16;
const POLLOUT: u16 = crate::ipc::poll::events::POLLOUT as u16;
const POLLHUP: u16 = crate::ipc::poll::events::POLLHUP as u16;
const POLLRDHUP: u16 = crate::ipc::poll::events::POLLRDHUP as u16;

const MSG_PEEK: u32 = 0x2;
const MSG_WAITALL: u32 = 0x100;

const SHUT_RD: u8 = 0;
const SHUT_WR: u8 = 1;

#[derive(Clone, Copy, Debug)]
pub enum TcpSockState {
    Unbound,
    Bound { addr: [u8; 4], port: u16 },
    Listening { addr: [u8; 4], port: u16 },
    Connected(ConnectionId),
}

pub struct TcpSock {
    state: Mutex<TcpSockState>,
}

impl TcpSock {
    pub fn new() -> TcpSock {
        TcpSock {
            state: Mutex::new(TcpSockState::Unbound),
        }
    }

    pub fn connected(conn: ConnectionId) -> TcpSock {
        TcpSock {
            state: Mutex::new(TcpSockState::Connected(conn)),
        }
    }

    pub fn state(&self) -> TcpSockState {
        *self.state.lock()
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
        let state = core::mem::replace(self.state.get_mut(), TcpSockState::Unbound);
        TcpSock {
            state: Mutex::new(state),
        }
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

/// Size the connection's buffers from the socket's SO_RCVBUF and SO_SNDBUF.
fn apply_buffers(sk: &Socket, conn: &ConnectionId) {
    tcp::tcp_set_buffer_sizes(conn, sk.common.rcvbuf(), sk.common.sndbuf());
}

impl ProtoOps for TcpSock {
    fn bind(&self, sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32> {
        let SockAddr::Inet { addr, port } = *addr else {
            return Err(errno::EAFNOSUPPORT);
        };
        let mut state = self.state.lock();
        if !matches!(*state, TcpSockState::Unbound) {
            return Err(errno::EINVAL);
        }
        if !is_local_addr(addr) {
            return Err(errno::EADDRNOTAVAIL);
        }
        let port = if port == 0 {
            tcp::tcp_bind_ephemeral()?
        } else {
            tcp::tcp_bind_port(port, sk.common.reuseaddr())?;
            port
        };
        *state = TcpSockState::Bound { addr, port };
        Ok(())
    }

    fn listen(&self, sk: &Arc<Socket>, backlog: i32) -> Result<(), i32> {
        let mut state = self.state.lock();
        let (addr, port) = match *state {
            TcpSockState::Bound { addr, port } => (addr, port),
            // Linux binds a socket that was never bound to an ephemeral port.
            TcpSockState::Unbound => ([0; 4], tcp::tcp_bind_ephemeral()?),
            // A second listen only changes the backlog, which Linux allows.
            TcpSockState::Listening { .. } => return Ok(()),
            TcpSockState::Connected(_) => return Err(errno::EINVAL),
        };
        let listening = tcp::tcp_listen(port, backlog.max(0) as usize, super::object::caller_pid());
        tcp::tcp_unbind_port(port);
        if listening.is_err() {
            *state = TcpSockState::Unbound;
            return Err(errno::EADDRINUSE);
        }
        tcp::tcp_set_listener_owner(port, Arc::downgrade(sk));
        *state = TcpSockState::Listening { addr, port };
        Ok(())
    }

    fn accept(&self, sk: &Arc<Socket>, nonblock: bool) -> Result<(Arc<Socket>, SockAddr), i32> {
        let TcpSockState::Listening { port, .. } = self.state() else {
            return Err(errno::EINVAL);
        };
        let conn = wait_until(sk, nonblock, None, || tcp::tcp_accept(port).map(Ok))?;
        let peer = SockAddr::Inet {
            addr: conn.remote_ip,
            port: conn.remote_port,
        };
        match Socket::new(
            AF_INET,
            SOCK_STREAM,
            IPPROTO_TCP,
            SocketProto::Tcp(TcpSock::connected(conn)),
        ) {
            Ok(socket) => {
                tcp::tcp_set_owner(&conn, Arc::downgrade(&socket));
                apply_buffers(&socket, &conn);
                Ok((socket, peer))
            }
            Err(error) => {
                let _ = tcp::tcp_close_connection(&conn, true);
                crate::net::drain_loopback_queue();
                Err(error)
            }
        }
    }

    fn connect(&self, sk: &Arc<Socket>, addr: &SockAddr, nonblock: bool) -> Result<(), i32> {
        let SockAddr::Inet {
            addr: remote_ip,
            port: remote_port,
        } = *addr
        else {
            return Err(errno::EAFNOSUPPORT);
        };
        let conn = {
            let mut state = self.state.lock();
            let local_port = match *state {
                TcpSockState::Unbound => tcp::tcp_bind_ephemeral()?,
                TcpSockState::Bound { port, .. } => port,
                TcpSockState::Connected(conn) => {
                    return Err(if tcp::tcp_get_state(&conn) == Some(tcp::TcpState::SynSent) {
                        errno::EALREADY
                    } else {
                        errno::EISCONN
                    });
                }
                TcpSockState::Listening { .. } => return Err(errno::EINVAL),
            };
            let connecting = tcp::tcp_connect(
                local_port,
                remote_ip,
                remote_port,
                super::object::caller_pid(),
            );
            // The connection holds the port from here, or the socket gives it up.
            tcp::tcp_unbind_port(local_port);
            let Ok(conn) = connecting else {
                *state = TcpSockState::Unbound;
                return Err(errno::EADDRINUSE);
            };
            tcp::tcp_set_owner(&conn, Arc::downgrade(sk));
            apply_buffers(sk, &conn);
            *state = TcpSockState::Connected(conn);
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
                Some(tcp::TcpState::SynSent) => None,
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
        let deadline = sk.common.snd_deadline();
        let mut sent = 0;
        loop {
            let data = &msg.data[sent..];
            let result = wait_until(sk, msg.nonblock, deadline, || {
                match tcp::tcp_sendmsg(&conn, data) {
                    Ok(n) => Some(Ok(n)),
                    // The send buffer is full, or the handshake has not finished.
                    Err(errno::EAGAIN) | Err(errno::ENOTCONN) => None,
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
        if msg.data.try_reserve_exact(msg.cap).is_err() {
            return Err(errno::ENOMEM);
        }
        msg.data.resize(msg.cap, 0);
        if msg.cap == 0 {
            return Ok(0);
        }
        let peek = msg.flags & MSG_PEEK != 0;
        let waitall = msg.flags & MSG_WAITALL != 0 && !peek;
        let deadline = sk.common.rcv_deadline();
        let mut received = 0;
        loop {
            let room = &mut msg.data[received..];
            let result = wait_until(sk, msg.nonblock, deadline, || {
                match tcp::tcp_recvmsg(&conn, room, peek) {
                    Err(errno::EAGAIN) => None,
                    other => Some(other),
                }
            });
            match result {
                Ok(0) => break,
                Ok(n) => {
                    received += n;
                    if !waitall || received == msg.cap {
                        break;
                    }
                }
                Err(_) if received > 0 => break,
                Err(error) => return Err(error),
            }
        }
        // A window update the read made goes out now.
        crate::net::drain_loopback_queue();
        msg.data.truncate(received);
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
            TcpSockState::Bound { addr, port } | TcpSockState::Listening { addr, port } => {
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
            TcpSockState::Listening { port, .. } => {
                if tcp::tcp_has_pending(port) {
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

    fn release(&self, sk: &Socket) {
        match self.state() {
            TcpSockState::Listening { port, .. } => {
                tcp::tcp_listener_ref_dec(port);
            }
            TcpSockState::Connected(conn) => {
                let abort = sk.common.linger() == Some(0);
                let _ = tcp::tcp_close_connection(&conn, abort);
            }
            TcpSockState::Bound { port, .. } => tcp::tcp_unbind_port(port),
            TcpSockState::Unbound => {}
        }
        // The FIN or RST goes out now.
        crate::net::drain_loopback_queue();
    }
}
