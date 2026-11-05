//! Clipboard listener and its bounded set of nonblocking clients.
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use libbreenix::io::{PollFd, poll_events};
use super::{clipboard, clipboard_server::Connection, session};

const PATH: &str = "/run/bwm/clipboard";
const MAX_CONNECTIONS: usize = 16;
const SOL_SOCKET: i32 = 1;
const SO_PEERCRED: i32 = 17;
unsafe extern "C" { fn getsockopt(fd: i32, level: i32, name: i32, value: *mut u8, length: *mut u32) -> i32; }

pub struct Server { listener: UnixListener, clients: Vec<Connection<UnixStream>>, store: clipboard::Store }
impl Server {
    pub fn open() -> io::Result<Self> {
        match std::fs::create_dir("/run/bwm") { Ok(()) => (), Err(e) if e.kind() == ErrorKind::AlreadyExists => (), Err(e) => return Err(e) }
        std::fs::set_permissions("/run/bwm", std::fs::Permissions::from_mode(0o755))?;
        match std::fs::remove_file(PATH) { Ok(()) => (), Err(e) if e.kind() == ErrorKind::NotFound => (), Err(e) => return Err(e) }
        let listener = UnixListener::bind(PATH)?;
        std::fs::set_permissions(PATH, std::fs::Permissions::from_mode(0o666))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, clients: Vec::new(), store: clipboard::Store::new() })
    }
    pub fn append_poll(&self, fds: &mut Vec<PollFd>) {
        if self.clients.len() < MAX_CONNECTIONS { fds.push(PollFd { fd: self.listener.as_raw_fd(), events: poll_events::POLLIN, revents: 0 }); }
        for client in &self.clients {
            fds.push(PollFd { fd: client.stream().as_raw_fd(), events: if client.wants_write() { poll_events::POLLOUT } else { poll_events::POLLIN }, revents: 0 });
        }
    }
    pub fn service(&mut self, fds: &[PollFd]) {
        let find = |fd| fds.iter().find(|p| p.fd == fd).map_or(0, |p| p.revents);
        for client in &mut self.clients {
            let events = find(client.stream().as_raw_fd());
            if events & (poll_events::POLLIN | poll_events::POLLOUT) != 0 {
                let result = client.advance(events & poll_events::POLLIN != 0, events & poll_events::POLLOUT != 0, &mut self.store, |pid, id| {
                    let mut list = Vec::new();
                    session::list_windows(&mut list).is_ok() && list.iter().any(|w| w.id == id && w.owner_pid == pid)
                });
                if result.is_err() { client.close(); }
            }
        }
        self.clients.retain(|c| !c.done() && find(c.stream().as_raw_fd()) & (poll_events::POLLERR | poll_events::POLLHUP | poll_events::POLLNVAL) == 0);
        if find(self.listener.as_raw_fd()) & poll_events::POLLIN != 0 && self.clients.len() < MAX_CONNECTIONS {
            if let Ok((stream, _)) = self.listener.accept() {
                let mut credentials = [0u32; 3]; let mut len = 12u32;
                let ok = unsafe { getsockopt(stream.as_raw_fd(), SOL_SOCKET, SO_PEERCRED, credentials.as_mut_ptr().cast(), &mut len) } == 0 && len == 12;
                if ok && stream.set_nonblocking(true).is_ok() { self.clients.push(Connection::new(stream, credentials[0])); }
            }
        }
    }
}
