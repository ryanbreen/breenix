//! TCP Client interactive test (std version)
//!
//! Connects to an external host and sends a message.
//!
//! Usage:
//! 1. On host machine: nc -l 18888
//! 2. In Breenix shell: tcpclient
//! 3. See "Hello from Breenix!" appear in netcat
//!
//! Network: Uses QEMU SLIRP, host is reachable at 10.0.2.2

use libbreenix::io;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use std::process;

const MESSAGE: &[u8] = b"Hello from Breenix!\n";

fn main() {
    print!("TCP Client: Starting\n");

    // Target: host machine via QEMU SLIRP gateway
    // In SLIRP mode, host is accessible at 10.0.2.2
    let dest = SockAddrIn::new([10, 0, 2, 2], 18888);

    // Create TCP socket
    let fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            print!("TCP Client: Socket failed with errno {:?}\n", e);
            process::exit(1);
        }
    };
    print!("TCP Client: Socket created\n");

    // Connect to host
    if let Err(e) = socket::connect_inet(fd, &dest) {
        print!("TCP Client: Connect failed with errno {:?}\n", e);
        print!("TCP Client: Make sure 'nc -l 18888' is running on host\n");
        process::exit(2);
    }
    print!("TCP Client: Connected to 10.0.2.2:18888\n");

    // Send message using write() syscall
    match io::write(fd, MESSAGE) {
        Ok(written) => {
            print!("TCP Client: Message sent ({} bytes)\n", written);
            print!("TCP Client: SUCCESS\n");
            let _ = io::close(fd);
            process::exit(0);
        }
        Err(e) => {
            print!("TCP Client: Write failed with errno {:?}\n", e);
            let _ = io::close(fd);
            process::exit(3);
        }
    }
}
