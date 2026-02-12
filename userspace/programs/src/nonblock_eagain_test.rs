//! Non-blocking recvfrom() EAGAIN test (std version)
//!
//! Verifies that a non-blocking UDP recvfrom() returns EAGAIN when no data.

use libbreenix::io;
use libbreenix::error::Error;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK};
use libbreenix::Errno;
use std::process;

fn main() {
    print!("NONBLOCK_EAGAIN_TEST: starting\n");

    let fd = match socket::socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0) {
        Ok(fd) => fd,
        Err(e) => {
            print!("NONBLOCK_EAGAIN_TEST: socket failed, error={:?}\n", e);
            process::exit(1);
        }
    };

    let local_addr = SockAddrIn::new([0, 0, 0, 0], 55557);
    if let Err(e) = socket::bind_inet(fd, &local_addr) {
        print!("NONBLOCK_EAGAIN_TEST: bind failed, error={:?}\n", e);
        process::exit(2);
    }

    print!("NONBLOCK_EAGAIN_TEST: recvfrom with empty queue...\n");

    let mut recv_buf = [0u8; 256];
    match socket::recvfrom(fd, &mut recv_buf, None) {
        Ok(n) => {
            print!("NONBLOCK_EAGAIN_TEST: unexpected data, bytes={}\n", n);
            process::exit(3);
        }
        Err(Error::Os(Errno::EAGAIN)) => {
            print!("NONBLOCK_EAGAIN_TEST: got EAGAIN\n");
        }
        Err(e) => {
            print!("NONBLOCK_EAGAIN_TEST: expected EAGAIN, error={:?}\n", e);
            process::exit(4);
        }
    }

    print!("NONBLOCK_EAGAIN_TEST: PASS\n");
    let _ = io::close(fd);
    process::exit(0);
}
