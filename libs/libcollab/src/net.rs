//! Nonblocking loopback TCP transport for collaboration sessions.
use crate::queue::OutputQueues;
pub use crate::queue::{SendError, Transport};
use alloc::{collections::BTreeMap, vec::Vec};
use libbreenix::{
    errno::Errno,
    error::Error,
    io::{self, poll_events, PollFd},
    socket::{self, SockAddrIn},
    types::{Fd, OwnedFd},
};

pub struct TcpTransport {
    listener: OwnedFd,
    connections: BTreeMap<u32, OwnedFd>,
    next: u32,
}
impl TcpTransport {
    pub fn bind(port: u16) -> Result<Self, Error> {
        let listener = OwnedFd::new(socket::socket(
            socket::AF_INET,
            socket::SOCK_STREAM | socket::SOCK_NONBLOCK,
            0,
        )?);
        socket::bind_inet(listener.fd(), &SockAddrIn::new([127, 0, 0, 1], port))?;
        socket::listen(listener.fd(), 16)?;
        Ok(Self {
            listener,
            connections: BTreeMap::new(),
            next: 1,
        })
    }
    pub fn listener(&self) -> Fd {
        self.listener.fd()
    }
    pub fn connect(&mut self, port: u16) -> Result<u32, Error> {
        let fd = OwnedFd::new(socket::socket(
            socket::AF_INET,
            socket::SOCK_STREAM | socket::SOCK_NONBLOCK,
            0,
        )?);
        match socket::connect_inet(fd.fd(), &SockAddrIn::new([127, 0, 0, 1], port)) {
            Ok(()) | Err(Error::Os(Errno::EINPROGRESS)) => {}
            Err(error) => return Err(error),
        }
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(Error::Os(Errno::EMFILE))?;
        self.connections.insert(id, fd);
        Ok(id)
    }
    pub fn accept(&mut self) -> Result<u32, Error> {
        let fd = OwnedFd::new(socket::accept(self.listener.fd(), None)?);
        let flags = io::fcntl_getfl(fd.fd())?;
        io::fcntl_setfl(fd.fd(), flags as i32 | io::status_flags::O_NONBLOCK)?;
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(Error::Os(Errno::EMFILE))?;
        self.connections.insert(id, fd);
        Ok(id)
    }
    pub fn close(&mut self, conn: u32) {
        self.connections.remove(&conn);
    }
    pub fn recv(&self, conn: u32, bytes: &mut [u8]) -> Result<usize, Error> {
        socket::recv(
            self.connections
                .get(&conn)
                .ok_or(Error::Os(Errno::EBADF))?
                .fd(),
            bytes,
        )
    }
    pub fn poll_fds(&self, queues: &OutputQueues) -> Vec<(Option<u32>, PollFd)> {
        let mut fds = Vec::new();
        fds.push((None, PollFd::new(self.listener.fd(), poll_events::POLLIN)));
        for (&id, fd) in &self.connections {
            let mut events = poll_events::POLLIN;
            if queues.wants_write(id) {
                events |= poll_events::POLLOUT;
            }
            fds.push((Some(id), PollFd::new(fd.fd(), events)));
        }
        fds
    }
}
impl Transport for TcpTransport {
    type Error = Error;
    fn send(&mut self, conn: u32, bytes: &[u8]) -> Result<usize, SendError<Error>> {
        let fd = self
            .connections
            .get(&conn)
            .ok_or(SendError::Other(Error::Os(Errno::EBADF)))?;
        socket::send(fd.fd(), bytes).map_err(|e| match e {
            Error::Os(Errno::EAGAIN) => SendError::WouldBlock,
            e => SendError::Other(e),
        })
    }
}
