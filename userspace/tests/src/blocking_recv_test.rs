//! Blocking recvfrom() test (std version)
//!
//! Verifies that a blocking UDP recvfrom() waits for data and wakes on packet.

use std::process;

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: addr,
            sin_zero: [0; 8],
        }
    }

    fn port_host(&self) -> u16 {
        u16::from_be(self.sin_port)
    }
}

extern "C" {
    fn socket(domain: i32, sock_type: i32, protocol: i32) -> i32;
    fn bind(sockfd: i32, addr: *const u8, addrlen: u32) -> i32;
    fn recvfrom(
        sockfd: i32,
        buf: *mut u8,
        len: usize,
        flags: i32,
        src_addr: *mut u8,
        addrlen: *mut u32,
    ) -> isize;
    fn close(fd: i32) -> i32;
    static mut ERRNO: i32;
}

fn get_errno() -> i32 {
    unsafe { ERRNO }
}

fn main() {
    print!("BLOCKING_RECV_TEST: starting\n");

    let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        let e = get_errno();
        print!("BLOCKING_RECV_TEST: socket failed, errno={}\n", e);
        process::exit(1);
    }

    let local_addr = SockAddrIn::new([0, 0, 0, 0], 55556);
    let ret = unsafe {
        bind(
            fd,
            &local_addr as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>() as u32,
        )
    };
    if ret < 0 {
        let e = get_errno();
        print!("BLOCKING_RECV_TEST: bind failed, errno={}\n", e);
        process::exit(2);
    }

    print!("BLOCKING_RECV_TEST: waiting for packet...\n");

    let mut recv_buf = [0u8; 256];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    let ret = unsafe {
        recvfrom(
            fd,
            recv_buf.as_mut_ptr(),
            recv_buf.len(),
            0,
            &mut src_addr as *mut SockAddrIn as *mut u8,
            &mut addrlen,
        )
    };

    if ret < 0 {
        let e = get_errno();
        print!("BLOCKING_RECV_TEST: recvfrom failed, errno={}\n", e);
        process::exit(3);
    }

    let bytes = ret as usize;
    print!(
        "BLOCKING_RECV_TEST: received {} bytes from {}.{}.{}.{}:{}\n",
        bytes,
        src_addr.sin_addr[0],
        src_addr.sin_addr[1],
        src_addr.sin_addr[2],
        src_addr.sin_addr[3],
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

    print!("BLOCKING_RECV_TEST: PASS\n");
    unsafe { close(fd); }
    process::exit(0);
}
