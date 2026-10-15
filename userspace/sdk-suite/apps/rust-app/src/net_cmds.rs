use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
use std::os::unix::net::UnixStream;
use std::thread;

use crate::CmdResult;

pub fn tcp() -> CmdResult {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind 127.0.0.1:0: {e}"))?;
    let addr = listener.local_addr().map_err(|e| format!("local_addr: {e}"))?;
    let client = thread::spawn(move || -> Result<String, String> {
        let mut stream = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
        stream.write_all(b"hello tcp").map_err(|e| format!("client write: {e}"))?;
        stream.shutdown(Shutdown::Write).map_err(|e| format!("shutdown: {e}"))?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply).map_err(|e| format!("client read: {e}"))?;
        Ok(reply)
    });
    let (mut conn, _) = listener.accept().map_err(|e| format!("accept: {e}"))?;
    let mut request = String::new();
    conn.read_to_string(&mut request).map_err(|e| format!("server read: {e}"))?;
    conn.write_all(format!("ack:{request}").as_bytes()).map_err(|e| format!("server write: {e}"))?;
    drop(conn);
    let reply = client.join().map_err(|_| "the client thread panicked")??;
    println!("tcp server got: {request}");
    println!("tcp client got: {reply}");
    Ok(())
}

pub fn udp() -> CmdResult {
    let a = UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("bind a: {e}"))?;
    let b = UdpSocket::bind("127.0.0.1:0").map_err(|e| format!("bind b: {e}"))?;
    let b_addr = b.local_addr().map_err(|e| format!("local_addr: {e}"))?;
    a.send_to(b"datagram", b_addr).map_err(|e| format!("send_to: {e}"))?;
    let mut buf = [0u8; 32];
    let (n, from) = b.recv_from(&mut buf).map_err(|e| format!("recv_from: {e}"))?;
    let a_addr = a.local_addr().map_err(|e| format!("local_addr: {e}"))?;
    println!("udp: {} from-matches={}", String::from_utf8_lossy(&buf[..n]), from == a_addr);
    Ok(())
}

pub fn unix() -> CmdResult {
    let (mut left, mut right) = UnixStream::pair().map_err(|e| format!("UnixStream::pair: {e}"))?;
    let writer = thread::spawn(move || left.write_all(b"unix ping"));
    writer.join().map_err(|_| "the writer panicked")?.map_err(|e| format!("write: {e}"))?;
    let mut buf = [0u8; 16];
    let n = right.read(&mut buf).map_err(|e| format!("read: {e}"))?;
    println!("unix: {}", String::from_utf8_lossy(&buf[..n]));
    Ok(())
}
