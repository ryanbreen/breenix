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

use std::process;

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;

const MESSAGE: &[u8] = b"Hello from Breenix!\n";

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
}

extern "C" {
    fn socket(domain: i32, sock_type: i32, protocol: i32) -> i32;
    fn connect(sockfd: i32, addr: *const u8, addrlen: u32) -> i32;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    static mut ERRNO: i32;
}

fn get_errno() -> i32 {
    unsafe { ERRNO }
}

fn main() {
    print!("TCP Client: Starting\n");

    // Target: host machine via QEMU SLIRP gateway
    // In SLIRP mode, host is accessible at 10.0.2.2
    let dest = SockAddrIn::new([10, 0, 2, 2], 18888);

    // Create TCP socket
    let fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if fd < 0 {
        let e = get_errno();
        print!("TCP Client: Socket failed with errno {}\n", e);
        process::exit(1);
    }
    if fd < 0 {
        print!("TCP Client: Socket returned invalid fd\n");
        process::exit(1);
    }
    print!("TCP Client: Socket created\n");

    // Connect to host
    let ret = unsafe {
        connect(
            fd,
            &dest as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>() as u32,
        )
    };
    if ret < 0 {
        let e = get_errno();
        print!("TCP Client: Connect failed with errno {}\n", e);
        print!("TCP Client: Make sure 'nc -l 18888' is running on host\n");
        process::exit(2);
    }
    print!("TCP Client: Connected to 10.0.2.2:18888\n");

    // Send message using write() syscall
    let written = unsafe { write(fd, MESSAGE.as_ptr(), MESSAGE.len()) };
    if written > 0 {
        print!("TCP Client: Message sent ({} bytes)\n", written);
        print!("TCP Client: SUCCESS\n");
        unsafe { close(fd); }
        process::exit(0);
    } else {
        let e = get_errno();
        print!("TCP Client: Write failed with errno {}\n", e);
        unsafe { close(fd); }
        process::exit(3);
    }
}
