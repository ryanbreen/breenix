//! Blocking recvfrom() test (std version)
//!
//! Verifies that a blocking UDP recvfrom() waits for data and wakes on packet.

use libbreenix::io;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_DGRAM};
use std::process;

fn main() {
    print!("BLOCKING_RECV_TEST: starting\n");

    let fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            print!("BLOCKING_RECV_TEST: socket failed, errno={:?}\n", e);
            process::exit(1);
        }
    };

    let local_addr = SockAddrIn::new([0, 0, 0, 0], 55556);
    if let Err(e) = socket::bind_inet(fd, &local_addr) {
        print!("BLOCKING_RECV_TEST: bind failed, errno={:?}\n", e);
        process::exit(2);
    }

    print!("BLOCKING_RECV_TEST: waiting for packet...\n");

    let mut recv_buf = [0u8; 256];
    let mut src_addr = SockAddrIn::default();
    match socket::recvfrom(fd, &mut recv_buf, Some(&mut src_addr)) {
        Ok(bytes) => {
            print!(
                "BLOCKING_RECV_TEST: received {} bytes from {}.{}.{}.{}:{}\n",
                bytes,
                src_addr.addr[0],
                src_addr.addr[1],
                src_addr.addr[2],
                src_addr.addr[3],
                src_addr.port_host()
            );

            let expected = b"wakeup";
            let mut matches = bytes >= expected.len();
            if matches {
                for i in 0..expected.len() {
                    if recv_buf[i] != expected[i] {
                        matches = false;
                        break;
                    }
                }
            }

            if matches {
                print!("BLOCKING_RECV_TEST: data verified\n");
            } else {
                print!("BLOCKING_RECV_TEST: data mismatch\n");
                process::exit(4);
            }
        }
        Err(e) => {
            print!("BLOCKING_RECV_TEST: recvfrom failed, errno={:?}\n", e);
            process::exit(3);
        }
    }

    print!("BLOCKING_RECV_TEST: PASS\n");
    let _ = io::close(fd);
    process::exit(0);
}
