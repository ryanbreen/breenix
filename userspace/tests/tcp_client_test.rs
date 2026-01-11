//! TCP Client interactive test
//!
//! Connects to an external host and sends a message.
//!
//! Usage:
//! 1. On host machine: nc -l 18888
//! 2. In Breenix shell: tcpclient
//! 3. See "Hello from Breenix!" appear in netcat
//!
//! Network: Uses QEMU SLIRP, host is reachable at 10.0.2.2

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::socket::{socket, connect, SockAddrIn, AF_INET, SOCK_STREAM};

const MESSAGE: &[u8] = b"Hello from Breenix!\n";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("TCP Client: Starting\n");

    // Target: host machine via QEMU SLIRP gateway
    // In SLIRP mode, host is accessible at 10.0.2.2
    let dest = SockAddrIn::new([10, 0, 2, 2], 18888);

    // Create TCP socket
    let fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => {
            io::print("TCP Client: Socket created\n");
            fd
        }
        Ok(_) => {
            io::print("TCP Client: Socket returned invalid fd\n");
            process::exit(1);
        }
        Err(e) => {
            io::print("TCP Client: Socket failed with errno ");
            print_errno(e);
            io::print("\n");
            process::exit(1);
        }
    };

    // Connect to host
    match connect(fd, &dest) {
        Ok(()) => {
            io::print("TCP Client: Connected to 10.0.2.2:18888\n");
        }
        Err(e) => {
            io::print("TCP Client: Connect failed with errno ");
            print_errno(e);
            io::print("\n");
            io::print("TCP Client: Make sure 'nc -l 18888' is running on host\n");
            process::exit(2);
        }
    }

    // Send message using write() syscall
    let written = io::write(fd as u64, MESSAGE);
    if written > 0 {
        io::print("TCP Client: Message sent (");
        print_num(written as u64);
        io::print(" bytes)\n");
        io::print("TCP Client: SUCCESS\n");
        process::exit(0);
    } else {
        io::print("TCP Client: Write failed with errno ");
        print_errno((-written) as i32);
        io::print("\n");
        process::exit(3);
    }
}

fn print_num(n: u64) {
    if n == 0 {
        io::print("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut val = n;
    while val > 0 {
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        io::print(unsafe { core::str::from_utf8_unchecked(&buf[i..i+1]) });
    }
}

fn print_errno(e: i32) {
    print_num(e as u64);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("TCP Client: PANIC!\n");
    process::exit(99);
}
