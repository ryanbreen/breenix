#[path = "../userspace/programs/src/bwm/clipboard.rs"]
mod clipboard;
#[path = "../userspace/programs/src/bwm/clipboard_server.rs"]
mod clipboard_server;

use clipboard::Store;
use clipboard_server::{admit, Connection, IDLE_NS, MAX_CONNECTIONS};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }
unsafe extern "C" { fn poll(fds: *mut PollFd, n: usize, timeout: i32) -> i32; }

fn wire(op: u32, window: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = b"BCLIP001".to_vec();
    out.extend(op.to_le_bytes()); out.extend((bytes.len() as u32).to_le_bytes());
    out.extend(window.to_le_bytes()); out.extend(bytes); out
}
fn pair() -> (UnixStream, Connection<UnixStream>) {
    let (client, server) = UnixStream::pair().unwrap();
    client.set_nonblocking(true).unwrap(); server.set_nonblocking(true).unwrap();
    (client, Connection::new(server, 42))
}
fn drive(client: &mut UnixStream, conn: &mut Connection<UnixStream>, store: &mut Store, request: &[u8]) -> Vec<u8> {
    let mut sent = 0; let mut reply = Vec::new();
    for _ in 0..10000 {
        let mut fds = [PollFd { fd: client.as_raw_fd(), events: if sent < request.len() { 5 } else { 1 }, revents: 0 },
            PollFd { fd: conn.stream().as_raw_fd(), events: if conn.wants_write() { 4 } else { 1 }, revents: 0 }];
        assert!(unsafe { poll(fds.as_mut_ptr(), fds.len(), 100) } > 0);
        if fds[0].revents & 4 != 0 { match client.write(&request[sent..]) { Ok(n) => sent += n, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (), Err(e) => panic!("{e}") } }
        if fds[1].revents & 5 != 0 { conn.advance(fds[1].revents & 1 != 0, fds[1].revents & 4 != 0, store, |pid, id| pid == 42 && id == 7).unwrap(); }
        if fds[0].revents & 1 != 0 { let mut buf = [0; 8192]; match client.read(&mut buf) { Ok(0) => (), Ok(n) => reply.extend_from_slice(&buf[..n]), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (), Err(e) => panic!("{e}") } }
        if reply.len() >= 16 && reply.len() == 16 + u32::from_le_bytes(reply[4..8].try_into().unwrap()) as usize { return reply; }
    }
    panic!("exchange stalled");
}
#[test]
fn large_transfer_stalled_peer_and_ownership() {
    let mut store = Store::new();
    let (mut stalled, mut stalled_server) = pair();
    stalled.write_all(&wire(1, 7, b"partial")[..25]).unwrap();
    stalled_server.advance(true, false, &mut store, |_, _| true).unwrap();
    assert!(!stalled_server.wants_write());
    assert!(!stalled_server.done());
    let data = vec![0x5a; 1024 * 1024];
    let (mut client, mut server) = pair();
    let set = drive(&mut client, &mut server, &mut store, &wire(1, 7, &data));
    assert_eq!(i32::from_le_bytes(set[..4].try_into().unwrap()), 0);
    drop(client); drop(server);
    let (mut client, mut server) = pair();
    let got = drive(&mut client, &mut server, &mut store, &wire(2, 7, &[]));
    assert_eq!(i32::from_le_bytes(got[..4].try_into().unwrap()), 0);
    assert_eq!(&got[16..], data);
    let (mut client, mut server) = pair();
    let denied = drive(&mut client, &mut server, &mut store, &wire(2, 8, &[]));
    assert_eq!(i32::from_le_bytes(denied[..4].try_into().unwrap()), -1);
    assert_eq!(denied.len(), 16);
    stalled_server.close();
    assert!(stalled_server.done());
}

/// An in-memory peer: bwm reads `input` in its own chunk sizes and writes into `output`.
struct Peer { input: Vec<u8>, read: usize, output: Vec<u8> }
impl Read for Peer {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.input.len() - self.read);
        buf[..n].copy_from_slice(&self.input[self.read..self.read + n]);
        self.read += n;
        Ok(n)
    }
}
impl Write for Peer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { self.output.extend_from_slice(buf); Ok(buf.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
fn owner(pid: u32, id: u64) -> bool { pid == 42 && id == 7 }

#[test]
fn non_owner_is_refused_at_its_header_before_bwm_buffers_the_payload() {
    let mut store = Store::new();
    let mut request = wire(1, 8, &vec![0x41; 16 * 1024 * 1024]);
    request.truncate(24 + 100);
    let mut conn = Connection::new(Peer { input: request, read: 0, output: Vec::new() }, 42);
    conn.advance(true, false, &mut store, owner).unwrap();
    assert!(conn.wants_write(), "the refusal must not wait for the declared 16 MiB");
    conn.advance(false, true, &mut store, owner).unwrap();
    let reply = &conn.stream().output;
    assert_eq!(reply.len(), 16);
    assert_eq!(i32::from_le_bytes(reply[..4].try_into().unwrap()), -1);
    assert!(conn.done());
    assert_eq!(store.serial(), 1);
}

#[test]
fn idle_connection_expires_and_progress_extends_it() {
    let (_client, server) = UnixStream::pair().unwrap();
    let mut conn = Connection::accepted(server, 42, 1_000);
    assert_eq!(conn.peer(), 42);
    assert_eq!(conn.deadline(), 1_000 + IDLE_NS);
    conn.idle(1_000 + IDLE_NS - 1, false);
    assert!(!conn.done());
    conn.idle(2_000, true);
    assert_eq!(conn.deadline(), 2_000 + IDLE_NS);
    conn.idle(1_000 + IDLE_NS, false);
    assert!(!conn.done());
    conn.idle(2_000 + IDLE_NS, false);
    assert!(conn.done());
}

#[test]
fn admission_requires_a_window_and_bounds_each_peer() {
    assert!(admit([].into_iter(), 42, true));
    assert!(!admit([].into_iter(), 43, false));
    assert!(admit([42].into_iter(), 42, true));
    assert!(!admit([42, 42].into_iter(), 42, true));
    assert!(admit([42, 42].into_iter(), 43, true));
    assert!(!admit((0..MAX_CONNECTIONS as u32).map(|p| p + 100), 42, true));
}

#[test]
fn hangup_after_a_large_set_keeps_the_queued_bytes() {
    let mut store = Store::new();
    let data = vec![0x33; 64 * 1024];
    let mut conn = Connection::new(Peer { input: wire(1, 7, &data), read: 0, output: Vec::new() }, 42);
    // Breenix reports POLLIN|POLLHUP once the setter closed (kernel/src/socket/unix.rs:820-829).
    while !conn.wants_write() {
        assert!(conn.retained(true, false), "hangup dropped a SET with bytes still queued");
        conn.advance(true, false, &mut store, owner).unwrap();
    }
    assert!(!conn.retained(true, false), "a reply nobody can read is dropped on hangup");
    assert_eq!(store.serial(), 2);
    let got = store.execute(clipboard::Request { window: 7, op: 2, payload: Vec::new() });
    assert_eq!(got.payload, data);
}
