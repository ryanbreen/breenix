//! Select syscall test program (std version)
//!
//! Tests the select() syscall for monitoring file descriptors.

use std::process;

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(addr: [u8; 4], port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: addr,
            sin_zero: [0; 8],
        }
    }
}

extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn select(nfds: i32, readfds: *mut u64, writefds: *mut u64, exceptfds: *mut u64, timeout: *const u8) -> i32;
    fn socket(domain: i32, socket_type: i32, protocol: i32) -> i32;
    fn bind(sockfd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
    fn listen(sockfd: i32, backlog: i32) -> i32;
    fn connect(sockfd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
}

// fd_set helpers (using u64 bitmask like the kernel)
fn fd_zero(set: &mut u64) {
    *set = 0;
}

fn fd_set_bit(fd: i32, set: &mut u64) {
    if fd >= 0 && fd < 64 {
        *set |= 1u64 << fd;
    }
}

fn fd_isset(fd: i32, set: &u64) -> bool {
    if fd >= 0 && fd < 64 {
        (*set & (1u64 << fd)) != 0
    } else {
        false
    }
}

fn fail(msg: &str) -> ! {
    println!("USERSPACE SELECT: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== Select Test Program ===");

    // Phase 1: Create a pipe for testing
    println!("Phase 1: Creating pipe...");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        println!("  pipe() returned error: {}", ret);
        fail("pipe() failed");
    }

    println!("  Pipe created: read_fd={}, write_fd={}", pipefd[0], pipefd[1]);

    // Phase 2: select on empty pipe for read (should not be ready)
    println!("Phase 2: Selecting on empty pipe for read...");
    let mut readfds: u64 = 0;
    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);

    let nfds = pipefd[0] + 1;
    let select_ret = unsafe {
        select(nfds, &mut readfds, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
    };

    println!("  select() returned: {}, readfds={:#x}", select_ret, readfds);

    if select_ret < 0 {
        fail("select() on empty pipe failed");
    }

    if fd_isset(pipefd[0], &readfds) {
        fail("Empty pipe should not be ready for read");
    }
    println!("  OK: Empty pipe is not ready for read");

    // Phase 3: Write data to pipe, then select for read
    println!("Phase 3: Writing data and selecting for read...");
    let test_data = b"Test";
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };

    if write_ret != test_data.len() as isize {
        println!("  write() returned: {}", write_ret);
        fail("write to pipe failed");
    }

    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);

    let select_ret = unsafe {
        select(nfds, &mut readfds, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
    };

    println!("  select() returned: {}, readfds={:#x}", select_ret, readfds);

    if select_ret < 0 {
        fail("select() on pipe with data failed");
    }
    if select_ret != 1 {
        fail("select() should return 1 when pipe has data");
    }
    if !fd_isset(pipefd[0], &readfds) {
        fail("Pipe with data should be ready for read");
    }
    println!("  OK: Pipe with data is ready for read");

    // Phase 4: Select on write end for write
    println!("Phase 4: Selecting on write end for write...");
    let mut writefds: u64 = 0;
    fd_zero(&mut writefds);
    fd_set_bit(pipefd[1], &mut writefds);

    let nfds_write = pipefd[1] + 1;
    let select_ret = unsafe {
        select(nfds_write, std::ptr::null_mut(), &mut writefds, std::ptr::null_mut(), std::ptr::null())
    };

    println!("  select() returned: {}, writefds={:#x}", select_ret, writefds);

    if select_ret < 0 {
        fail("select() on write end failed");
    }
    if !fd_isset(pipefd[1], &writefds) {
        fail("Write end should be ready for write");
    }
    println!("  OK: Write end is ready for write");

    // Phase 5: Select with multiple fd_sets
    println!("Phase 5: Selecting with multiple fd_sets...");
    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);
    fd_zero(&mut writefds);
    fd_set_bit(pipefd[1], &mut writefds);

    let max_fd = if pipefd[0] > pipefd[1] { pipefd[0] } else { pipefd[1] };
    let nfds_multi = max_fd + 1;

    let select_ret = unsafe {
        select(nfds_multi, &mut readfds, &mut writefds, std::ptr::null_mut(), std::ptr::null())
    };

    println!("  select() returned: {}", select_ret);
    println!("  readfds={:#x}, writefds={:#x}", readfds, writefds);

    if select_ret < 0 {
        fail("select() with multiple fd_sets failed");
    }
    if select_ret < 2 {
        fail("Expected at least 2 ready fds");
    }
    println!("  OK: Multiple fd_sets work correctly");

    // Phase 6: Close write end and check for exception on read end
    println!("Phase 6: Closing write end and checking for exception...");

    // First drain the pipe
    let mut read_buf = [0u8; 32];
    unsafe {
        read(pipefd[0], read_buf.as_mut_ptr(), read_buf.len());
    }

    // Close write end
    let close_ret = unsafe { close(pipefd[1]) };
    if close_ret < 0 {
        println!("  close() returned: {}", close_ret);
        fail("close() on write end failed");
    }

    let mut exceptfds: u64 = 0;
    fd_zero(&mut readfds);
    fd_set_bit(pipefd[0], &mut readfds);
    fd_zero(&mut exceptfds);
    fd_set_bit(pipefd[0], &mut exceptfds);

    let select_ret = unsafe {
        select(nfds, &mut readfds, std::ptr::null_mut(), &mut exceptfds, std::ptr::null())
    };

    println!("  select() returned: {}", select_ret);
    println!("  readfds={:#x}, exceptfds={:#x}", readfds, exceptfds);

    if select_ret < 0 {
        fail("select() after closing write end failed");
    }
    println!("  OK: select() returns after write end closed");

    // Phase 7: Test stdout writability
    println!("Phase 7: Testing stdout writability...");
    fd_zero(&mut writefds);
    fd_set_bit(1, &mut writefds);

    let select_ret = unsafe {
        select(2, std::ptr::null_mut(), &mut writefds, std::ptr::null_mut(), std::ptr::null())
    };

    println!("  select() returned: {}, writefds={:#x}", select_ret, writefds);

    if select_ret < 0 {
        fail("select() on stdout failed");
    }
    if !fd_isset(1, &writefds) {
        fail("stdout should be writable");
    }
    println!("  OK: stdout is writable");

    // Phase 8: Select on TCP listener for first-call readiness
    println!("Phase 8: Selecting on TCP listener...");
    let server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server_fd < 0 {
        println!("  socket() returned: {}", server_fd);
        fail("socket() failed");
    }

    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9092);
    let bind_ret = unsafe {
        bind(server_fd, &server_addr, std::mem::size_of::<SockAddrIn>() as u32)
    };
    if bind_ret < 0 {
        println!("  bind() returned: {}", bind_ret);
        fail("bind() failed");
    }

    let listen_ret = unsafe { listen(server_fd, 16) };
    if listen_ret < 0 {
        println!("  listen() returned: {}", listen_ret);
        fail("listen() failed");
    }

    let client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if client_fd < 0 {
        println!("  client socket() returned: {}", client_fd);
        fail("client socket() failed");
    }

    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 9092);
    let connect_ret = unsafe {
        connect(client_fd, &loopback_addr, std::mem::size_of::<SockAddrIn>() as u32)
    };
    if connect_ret < 0 {
        println!("  connect() returned: {}", connect_ret);
        fail("connect() failed");
    }

    fd_zero(&mut readfds);
    fd_set_bit(server_fd, &mut readfds);
    let nfds_tcp = server_fd + 1;
    let select_ret = unsafe {
        select(nfds_tcp, &mut readfds, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
    };

    println!("  select() returned: {}, readfds={:#x}", select_ret, readfds);

    if select_ret < 0 {
        fail("select() on listener failed");
    }
    if select_ret != 1 {
        fail("select() should return 1 for listener readiness");
    }
    if !fd_isset(server_fd, &readfds) {
        fail("Listener should be ready for read on first select");
    }
    println!("  OK: Listener ready on first select");

    unsafe { close(client_fd); close(server_fd); }

    // Clean up
    println!("Phase 9: Cleanup...");
    unsafe { close(pipefd[0]); }
    println!("  Closed remaining fds");

    // All tests passed
    println!("USERSPACE SELECT: ALL TESTS PASSED");
    println!("SELECT_TEST_PASSED");
    process::exit(0);
}
