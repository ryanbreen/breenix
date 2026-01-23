//! Non-blocking recvfrom() EAGAIN test
//!
//! Verifies that a non-blocking UDP recvfrom() returns EAGAIN when no data.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::socket::{
    bind, recvfrom, socket, SockAddrIn, AF_INET, SOCK_DGRAM, SOCK_NONBLOCK,
};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("NONBLOCK_EAGAIN_TEST: starting\n");

    let fd = match socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("NONBLOCK_EAGAIN_TEST: socket failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(1);
        }
    };

    let local_addr = SockAddrIn::new([0, 0, 0, 0], 55557);
    match bind(fd, &local_addr) {
        Ok(()) => {}
        Err(e) => {
            io::print("NONBLOCK_EAGAIN_TEST: bind failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(2);
        }
    }

    io::print("NONBLOCK_EAGAIN_TEST: recvfrom with empty queue...\n");

    let mut recv_buf = [0u8; 256];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    match recvfrom(fd, &mut recv_buf, Some(&mut src_addr)) {
        Ok(bytes) => {
            io::print("NONBLOCK_EAGAIN_TEST: unexpected data, bytes=");
            print_num(bytes as u64);
            io::print("\n");
            process::exit(3);
        }
        Err(e) => {
            if e == 11 {
                io::print("NONBLOCK_EAGAIN_TEST: got EAGAIN\n");
            } else {
                io::print("NONBLOCK_EAGAIN_TEST: expected EAGAIN, errno=");
                print_num(e as u64);
                io::print("\n");
                process::exit(4);
            }
        }
    }

    io::print("NONBLOCK_EAGAIN_TEST: PASS\n");
    io::close(fd as u64);
    process::exit(0);
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
    io::print("NONBLOCK_EAGAIN_TEST: PANIC!\n");
    process::exit(99);
}
