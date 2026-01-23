//! Blocking recvfrom() test
//!
//! Verifies that a blocking UDP recvfrom() waits for data and wakes on packet.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::socket::{bind, recvfrom, socket, SockAddrIn, AF_INET, SOCK_DGRAM};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("BLOCKING_RECV_TEST: starting\n");

    let fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("BLOCKING_RECV_TEST: socket failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(1);
        }
    };

    let local_addr = SockAddrIn::new([0, 0, 0, 0], 55556);
    match bind(fd, &local_addr) {
        Ok(()) => {}
        Err(e) => {
            io::print("BLOCKING_RECV_TEST: bind failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(2);
        }
    }

    io::print("BLOCKING_RECV_TEST: waiting for packet...\n");

    let mut recv_buf = [0u8; 256];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    match recvfrom(fd, &mut recv_buf, Some(&mut src_addr)) {
        Ok(bytes) => {
            io::print("BLOCKING_RECV_TEST: received ");
            print_num(bytes as u64);
            io::print(" bytes from ");
            print_ip(src_addr.addr);
            io::print(":");
            print_num(src_addr.port_host() as u64);
            io::print("\n");

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
                io::print("BLOCKING_RECV_TEST: data verified\n");
            } else {
                io::print("BLOCKING_RECV_TEST: data mismatch\n");
                process::exit(4);
            }
        }
        Err(e) => {
            io::print("BLOCKING_RECV_TEST: recvfrom failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(3);
        }
    }

    io::print("BLOCKING_RECV_TEST: PASS\n");
    io::close(fd as u64);
    process::exit(0);
}

fn print_ip(addr: [u8; 4]) {
    print_num(addr[0] as u64);
    io::print(".");
    print_num(addr[1] as u64);
    io::print(".");
    print_num(addr[2] as u64);
    io::print(".");
    print_num(addr[3] as u64);
}

/// Simple number printing (no formatting)
fn print_num(mut n: u64) {
    if n == 0 {
        io::print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }

    while i > 0 {
        i -= 1;
        let ch = [buf[i]];
        if let Ok(s) = core::str::from_utf8(&ch) {
            io::print(s);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("BLOCKING_RECV_TEST: PANIC!\n");
    process::exit(99);
}
