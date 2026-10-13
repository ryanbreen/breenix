//! The TCP half of a socket: its place in the connection's life, and the
//! operations that move it along, over net/tcp.rs.
//!
//! net/tcp.rs still wakes readers, connectors and acceptors by thread id from
//! `waiting_threads`, so each blocking check below registers the calling
//! thread there before it looks, and unregisters once the wait is over;
//! `wait_until`'s second check covers a wake that lands before the thread is
//! published as blocked.

use alloc::sync::Arc;
use spin::Mutex;

use super::addr::SockAddr;
use super::object::{ProtoOps, RecvMsg, SendMsg, Socket, SocketProto};
use super::wait::{now_ns, wait_until};
use crate::net::tcp::{self, ConnectionId, TcpState};
use crate::syscall::errno;

const AF_INET: u16 = 2;
const SOCK_STREAM: u16 = 1;
const IPPROTO_TCP: u16 = 6;

const POLLIN: u16 = crate::ipc::poll::events::POLLIN as u16;
const POLLOUT: u16 = crate::ipc::poll::events::POLLOUT as u16;
const POLLERR: u16 = crate::ipc::poll::events::POLLERR as u16;
const POLLHUP: u16 = crate::ipc::poll::events::POLLHUP as u16;

/// How long a blocking connect waits for the handshake.
const CONNECT_TIMEOUT_NS: u64 = 10_000_000_000;

/// The first port an unbound connect takes.
const EPHEMERAL_PORT_START: u16 = 49152;

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

fn current_tid() -> Result<u64, i32> {
    crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH)
}

fn send_error(error: &str) -> i32 {
    if error.contains("shutdown") {
        errno::EPIPE
    } else if error.contains("not found") {
        errno::EBADF
    } else if error.contains("not established") {
        // RST received (Closed) or FIN received (CloseWait).
        errno::ENOTCONN
    } else {
        errno::EIO
    }
}

impl ProtoOps for TcpSock {
    fn bind(&self, _sk: &Arc<Socket>, addr: &SockAddr) -> Result<(), i32> {
        let SockAddr::Inet { addr, port } = *addr else {
            return Err(errno::EAFNOSUPPORT);
        };
        let mut state = self.state.lock();
        if !matches!(*state, TcpSockState::Unbound) {
            return Err(errno::EINVAL);
        }
        if tcp::with_tcp_listeners(|listeners| listeners.contains_key(&port)) {
            return Err(errno::EADDRINUSE);
        }
        *state = TcpSockState::Bound { addr, port };
        Ok(())
    }

    fn listen(&self, _sk: &Arc<Socket>, backlog: i32) -> Result<(), i32> {
        let mut state = self.state.lock();
        let TcpSockState::Bound { addr, port } = *state else {
            return Err(errno::EINVAL);
        };
        if port == 0 {
            return Err(errno::EINVAL);
        }
        tcp::tcp_listen(port, backlog.max(0) as usize, super::object::caller_pid())
            .map_err(|_| errno::EADDRINUSE)?;
        *state = TcpSockState::Listening { addr, port };
        Ok(())
    }

    fn accept(&self, sk: &Arc<Socket>, nonblock: bool) -> Result<(Arc<Socket>, SockAddr), i32> {
        let TcpSockState::Listening { port, .. } = self.state() else {
            return Err(errno::EOPNOTSUPP);
        };
        let tid = current_tid()?;
        let accepted = wait_until(sk, nonblock, None, || {
            tcp::tcp_register_accept_waiter(port, tid);
            tcp::tcp_accept(port).map(Ok)
        });
        tcp::tcp_unregister_accept_waiter(port, tid);
        let conn = accepted?;
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
            Ok(socket) => Ok((socket, peer)),
            Err(error) => {
                let _ = tcp::tcp_close(&conn);
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
                TcpSockState::Unbound | TcpSockState::Bound { port: 0, .. } => {
                    static EPHEMERAL_PORT: core::sync::atomic::AtomicU16 =
                        core::sync::atomic::AtomicU16::new(EPHEMERAL_PORT_START);
                    EPHEMERAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                }
                TcpSockState::Bound { port, .. } => port,
                TcpSockState::Connected(_) => return Err(errno::EISCONN),
                TcpSockState::Listening { .. } => return Err(errno::EOPNOTSUPP),
            };
            let conn = tcp::tcp_connect(
                local_port,
                remote_ip,
                remote_port,
                super::object::caller_pid(),
            )
            .map_err(|_| errno::ECONNREFUSED)?;
            *state = TcpSockState::Connected(conn);
            conn
        };
        // The loopback SYN, SYN+ACK and ACK go through now, with no lock held.
        crate::net::drain_loopback_queue();
        if nonblock {
            return Err(errno::EINPROGRESS);
        }
        let tid = current_tid()?;
        let deadline = now_ns().saturating_add(CONNECT_TIMEOUT_NS);
        let result = wait_until(sk, false, Some(deadline), || {
            tcp::tcp_register_recv_waiter(&conn, tid);
            if tcp::tcp_is_established(&conn) {
                Some(Ok(()))
            } else if tcp::tcp_is_failed(&conn) {
                Some(Err(errno::ECONNREFUSED))
            } else {
                None
            }
        });
        tcp::tcp_unregister_recv_waiter(&conn, tid);
        match result {
            Err(errno::EAGAIN) => Err(errno::ETIMEDOUT),
            other => other,
        }
    }

    fn sendmsg(&self, _sk: &Arc<Socket>, msg: &mut SendMsg) -> Result<usize, i32> {
        let Some(conn) = self.connection() else {
            return Err(errno::ENOTCONN);
        };
        let sent = tcp::tcp_send(&conn, msg.data).map_err(send_error)?;
        crate::net::drain_loopback_queue();
        Ok(sent)
    }

    fn recvmsg(&self, sk: &Arc<Socket>, msg: &mut RecvMsg) -> Result<usize, i32> {
        let Some(conn) = self.connection() else {
            return Err(errno::ENOTCONN);
        };
        if msg.data.try_reserve_exact(msg.cap).is_err() {
            return Err(errno::ENOMEM);
        }
        msg.data.resize(msg.cap, 0);
        let tid = current_tid()?;
        let data = &mut msg.data;
        let result = wait_until(sk, msg.nonblock, sk.common.rcv_deadline(), || {
            tcp::tcp_register_recv_waiter(&conn, tid);
            match tcp::tcp_recv(&conn, data) {
                Ok(n) => Some(Ok(n)),
                Err(_) if sk.common.shut_rd() => Some(Ok(0)),
                Err(_) => None,
            }
        });
        tcp::tcp_unregister_recv_waiter(&conn, tid);
        let received = result?;
        msg.data.truncate(received);
        Ok(received)
    }

    fn shutdown(&self, _sk: &Arc<Socket>, how: u8) -> Result<(), i32> {
        let Some(conn) = self.connection() else {
            return Err(errno::ENOTCONN);
        };
        tcp::tcp_shutdown(&conn, how != 1, how != 0);
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
            TcpSockState::Connected(conn) => Ok(SockAddr::Inet {
                addr: conn.remote_ip,
                port: conn.remote_port,
            }),
            _ => Err(errno::ENOTCONN),
        }
    }

    fn poll(&self, _sk: &Socket) -> u16 {
        match self.state() {
            // An unconnected socket can start a connect at once.
            TcpSockState::Unbound | TcpSockState::Bound { .. } => POLLOUT,
            TcpSockState::Listening { port, .. } => {
                if tcp::tcp_has_pending(port) {
                    POLLIN
                } else {
                    0
                }
            }
            TcpSockState::Connected(conn) => tcp::with_tcp_connections(|connections| {
                let Some(connection) = connections.get(&conn) else {
                    return POLLERR;
                };
                let mut bits = 0;
                if !connection.rx_is_empty() {
                    bits |= POLLIN;
                }
                if connection.state == TcpState::Established {
                    bits |= POLLOUT;
                }
                if matches!(connection.state, TcpState::Closed | TcpState::CloseWait) {
                    bits |= POLLHUP;
                }
                bits
            }),
        }
    }

    fn is_listening(&self, _sk: &Socket) -> bool {
        matches!(self.state(), TcpSockState::Listening { .. })
    }

    fn release(&self, _sk: &Socket) {
        match self.state() {
            TcpSockState::Listening { port, .. } => {
                tcp::tcp_listener_ref_dec(port);
            }
            TcpSockState::Connected(conn) => {
                let _ = tcp::tcp_close(&conn);
            }
            TcpSockState::Unbound | TcpSockState::Bound { .. } => {}
        }
    }
}
