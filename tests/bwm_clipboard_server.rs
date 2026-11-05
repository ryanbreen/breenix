#[path = "../userspace/programs/src/bwm/clipboard.rs"]
mod clipboard;
#[path = "../userspace/programs/src/bwm/clipboard_server.rs"]
mod clipboard_server;

use clipboard::Store;
use clipboard_server::Connection;
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
