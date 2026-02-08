//! Non-blocking recvfrom() EAGAIN test (std version)
//!
//! Verifies that a non-blocking UDP recvfrom() returns EAGAIN when no data.

use std::process;

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0x800;

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
    print!("NONBLOCK_EAGAIN_TEST: starting\n");

    let fd = unsafe { socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0) };
    if fd < 0 {
        let e = get_errno();
        print!("NONBLOCK_EAGAIN_TEST: socket failed, errno={}\n", e);
        process::exit(1);
    }

    let local_addr = SockAddrIn::new([0, 0, 0, 0], 55557);
    let ret = unsafe {
        bind(
            fd,
            &local_addr as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>() as u32,
        )
    };
    if ret < 0 {
        let e = get_errno();
        print!("NONBLOCK_EAGAIN_TEST: bind failed, errno={}\n", e);
        process::exit(2);
    }

    print!("NONBLOCK_EAGAIN_TEST: recvfrom with empty queue...\n");

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

    if ret >= 0 {
        print!("NONBLOCK_EAGAIN_TEST: unexpected data, bytes={}\n", ret);
        process::exit(3);
    } else {
        let e = get_errno();
        if e == 11 {
            print!("NONBLOCK_EAGAIN_TEST: got EAGAIN\n");
        } else {
            print!("NONBLOCK_EAGAIN_TEST: expected EAGAIN, errno={}\n", e);
            process::exit(4);
        }
    }

    print!("NONBLOCK_EAGAIN_TEST: PASS\n");
    unsafe { close(fd); }
    process::exit(0);
}
