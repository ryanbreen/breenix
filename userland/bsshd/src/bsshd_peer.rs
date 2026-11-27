use std::net::Ipv4Addr;
use std::os::fd::OwnedFd;

/// Refuse the peer before its descriptor can reach the SSH transport.
pub fn admit(
    client: OwnedFd,
    peer: Ipv4Addr,
    allowed: Ipv4Addr,
    refused: &mut u64,
) -> Option<OwnedFd> {
    if peer == allowed {
        Some(client)
    } else {
        *refused += 1;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    #[test]
    fn refuses_and_closes_the_peer_before_ssh() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut refused = 0;
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (server, peer) = listener.accept().unwrap();
        let actual = peer.ip().to_string().parse().unwrap();
        assert!(admit(
            server.into(),
            actual,
            Ipv4Addr::new(192, 0, 2, 1),
            &mut refused
        )
        .is_none());
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
        assert_eq!(refused, 1);

        let _client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        assert!(admit(server.into(), actual, Ipv4Addr::LOCALHOST, &mut refused).is_some());
        assert_eq!(refused, 1);
    }
}
