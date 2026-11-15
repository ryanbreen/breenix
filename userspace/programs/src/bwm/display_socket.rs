//! Authenticated, bounded nonblocking display requests.
use std::io::{self, Read, Write, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::{fs::PermissionsExt, net::{UnixListener, UnixStream}};
use libbreenix::io::{PollFd, poll_events};
use super::display_protocol::{self, Command, Geometry};

const PATH: &str = "/run/bwm/display";
const MAX_CLIENTS: usize = 8;
const TIMEOUT: u64 = 5_000_000_000;
unsafe extern "C" {
    fn getsockopt(fd: i32, level: i32, name: i32, value: *mut u8, length: *mut u32) -> i32;
    fn geteuid() -> u32;
}
struct Client { stream: UnixStream, input: Vec<u8>, output: Vec<u8>, written: usize, deadline: u64 }
pub struct Server { listener: UnixListener, clients: Vec<Client> }
impl Server {
    pub fn open() -> io::Result<Self> {
        match std::fs::create_dir("/run/bwm") { Ok(()) => (), Err(e) if e.kind() == ErrorKind::AlreadyExists => (), Err(e) => return Err(e) }
        match std::fs::remove_file(PATH) { Ok(()) => (), Err(e) if e.kind() == ErrorKind::NotFound => (), Err(e) => return Err(e) }
        let listener = UnixListener::bind(PATH)?;
        std::fs::set_permissions(PATH, std::fs::Permissions::from_mode(0o666))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, clients: Vec::new() })
    }
    pub fn deadline(&self) -> Option<u64> { self.clients.iter().map(|c| c.deadline).min() }
    pub fn append_poll(&self, fds: &mut Vec<PollFd>) {
        if self.clients.len() < MAX_CLIENTS { fds.push(PollFd { fd: self.listener.as_raw_fd(), events: poll_events::POLLIN, revents: 0 }); }
        for c in &self.clients { fds.push(PollFd { fd: c.stream.as_raw_fd(), events: if c.output.is_empty() { poll_events::POLLIN } else { poll_events::POLLOUT }, revents: 0 }); }
    }
    pub fn service(&mut self, fds: &[PollFd], now: u64, mut execute: impl FnMut(Command) -> Result<Geometry, String>) {
        let events = |fd| fds.iter().find(|p| p.fd == fd).map_or(0, |p| p.revents);
        self.clients.retain_mut(|c| {
            let ev = events(c.stream.as_raw_fd());
            if now >= c.deadline || ev & (poll_events::POLLERR | poll_events::POLLNVAL) != 0 { return false; }
            if c.output.is_empty() && ev & (poll_events::POLLIN | poll_events::POLLHUP) != 0 {
                let mut buf = [0; 128];
                match c.stream.read(&mut buf) {
                    Ok(0) => return false,
                    Ok(n) => {
                        c.input.extend_from_slice(&buf[..n]);
                        if c.input.len() > 128 { c.output = display_protocol::reply(Err("request too long")).into_bytes(); }
                        else if c.input.contains(&b'\n') {
                            let response = match std::str::from_utf8(&c.input).map_err(|_| "invalid request").and_then(display_protocol::parse) {
                                Ok(cmd) => execute(cmd), Err(e) => Err(e.to_string()),
                            };
                            c.output = match response { Ok(g) => display_protocol::reply(Ok(g)), Err(e) => display_protocol::reply(Err(&e)) }.into_bytes();
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => (), Err(_) => return false,
                }
            }
            if !c.output.is_empty() && ev & poll_events::POLLOUT != 0 {
                match c.stream.write(&c.output[c.written..]) { Ok(0) => return false, Ok(n) => c.written += n, Err(e) if e.kind() == ErrorKind::WouldBlock => (), Err(_) => return false }
                if c.written == c.output.len() { return false; }
            }
            true
        });
        if events(self.listener.as_raw_fd()) & poll_events::POLLIN != 0 && self.clients.len() < MAX_CLIENTS {
            if let Ok((stream, _)) = self.listener.accept() {
                let mut cred = [0u32; 3]; let mut len = 12u32;
                let ok = unsafe { getsockopt(stream.as_raw_fd(), 1, 17, cred.as_mut_ptr().cast(), &mut len) } == 0 && len == 12;
                if ok && (cred[1] == 0 || cred[1] == unsafe { geteuid() }) && stream.set_nonblocking(true).is_ok() {
                    self.clients.push(Client { stream, input: Vec::new(), output: Vec::new(), written: 0, deadline: now.saturating_add(TIMEOUT) });
                }
            }
        }
    }
}
