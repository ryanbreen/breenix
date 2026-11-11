use libsmb::{
    auth::ntlm::Credentials,
    client::{Client, DirectTcp, Error},
    DialectCap,
};
use std::{
    io::ErrorKind,
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};

#[test]
fn expired_deadline_bounds_connect_and_every_frame() {
    let deadline = Instant::now() - Duration::from_secs(1);
    let address = "127.0.0.1:1".parse().unwrap();
    let creds = Credentials {
        user: "",
        domain: "",
        password: "",
        workstation: "",
    };
    assert!(
        matches!(Client::connect_at("localhost", address, "share", &creds,
        DialectCap::Default, deadline), Err(Error::Io(e)) if e.kind() == ErrorKind::TimedOut)
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut tcp = DirectTcp::from_stream(stream, Duration::from_secs(15)).unwrap();
    tcp.set_deadline(Some(deadline));
    assert!(matches!(tcp.send(&[0; 64]), Err(Error::Io(e)) if e.kind() == ErrorKind::TimedOut));
    assert!(matches!(tcp.receive(), Err(Error::Io(e)) if e.kind() == ErrorKind::TimedOut));
}
