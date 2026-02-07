//! Poll syscall test program (std version)
//!
//! Tests the poll() syscall for monitoring file descriptors.

use std::process;

const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
const POLLHUP: i16 = 0x0010;
const POLLNVAL: i16 = 0x0020;

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

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
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
    fn socket(domain: i32, socket_type: i32, protocol: i32) -> i32;
    fn bind(sockfd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
    fn listen(sockfd: i32, backlog: i32) -> i32;
    fn connect(sockfd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
}

fn fail(msg: &str) -> ! {
    println!("USERSPACE POLL: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== Poll Test Program ===");

    // Phase 1: Create a pipe for testing
    println!("Phase 1: Creating pipe...");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        println!("  pipe() returned error: {}", ret);
        fail("pipe() failed");
    }

    println!("  Pipe created: read_fd={}, write_fd={}", pipefd[0], pipefd[1]);

    // Phase 2: Poll empty pipe for POLLIN (should not be ready)
    println!("Phase 2: Polling empty pipe for POLLIN...");
    let mut fds = [PollFd {
        fd: pipefd[0],
        events: POLLIN,
        revents: 0,
    }];

    let poll_ret = unsafe { poll(fds.as_mut_ptr(), 1, 0) };

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if poll_ret < 0 {
        fail("poll() on empty pipe failed");
    }

    if fds[0].revents & POLLIN != 0 {
        fail("Empty pipe should not have POLLIN set");
    }
    println!("  OK: Empty pipe has no POLLIN");

    // Phase 3: Write data to pipe, then poll for POLLIN
    println!("Phase 3: Writing data and polling for POLLIN...");
    let test_data = b"Test";
    let write_ret = unsafe {
        write(pipefd[1], test_data.as_ptr(), test_data.len())
    };

    if write_ret != test_data.len() as isize {
        println!("  write() returned: {}", write_ret);
        fail("write to pipe failed");
    }

    fds[0].revents = 0;
    let poll_ret = unsafe { poll(fds.as_mut_ptr(), 1, 0) };

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if poll_ret < 0 {
        fail("poll() on pipe with data failed");
    }
    if poll_ret != 1 {
        fail("poll() should return 1 when pipe has data");
    }
    if fds[0].revents & POLLIN == 0 {
        fail("Pipe with data should have POLLIN set");
    }
    println!("  OK: Pipe with data has POLLIN");

    // Phase 4: Poll write end for POLLOUT
    println!("Phase 4: Polling write end for POLLOUT...");
    fds[0] = PollFd {
        fd: pipefd[1],
        events: POLLOUT,
        revents: 0,
    };

    let poll_ret = unsafe { poll(fds.as_mut_ptr(), 1, 0) };

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if poll_ret < 0 {
        fail("poll() on write end failed");
    }
    if fds[0].revents & POLLOUT == 0 {
        fail("Write end should have POLLOUT set");
    }
    println!("  OK: Write end has POLLOUT");

    // Phase 5: Poll invalid fd
    println!("Phase 5: Polling invalid fd...");
    fds[0] = PollFd {
        fd: 999,
        events: POLLIN,
        revents: 0,
    };

    let poll_ret = unsafe { poll(fds.as_mut_ptr(), 1, 0) };

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if poll_ret < 0 {
        fail("poll() on invalid fd should not return error");
    }
    if poll_ret != 1 {
        fail("poll() should return 1 when fd has POLLNVAL");
    }
    if fds[0].revents & POLLNVAL == 0 {
        fail("Invalid fd should have POLLNVAL set");
    }
    println!("  OK: Invalid fd has POLLNVAL");

    // Phase 6: Close write end and check for POLLHUP on read end
    println!("Phase 6: Closing write end and checking for POLLHUP...");

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

    fds[0] = PollFd {
        fd: pipefd[0],
        events: POLLIN,
        revents: 0,
    };

    let poll_ret = unsafe { poll(fds.as_mut_ptr(), 1, 0) };

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if poll_ret < 0 {
        fail("poll() after closing write end failed");
    }
    if fds[0].revents & POLLHUP == 0 {
        fail("Read end should have POLLHUP after write end closed");
    }
    println!("  OK: Read end has POLLHUP after write end closed");

    // Phase 7: Poll multiple fds
    println!("Phase 7: Polling multiple fds...");
    let mut multi_fds = [
        PollFd { fd: 0, events: POLLIN, revents: 0 },   // stdin
        PollFd { fd: 1, events: POLLOUT, revents: 0 },   // stdout
        PollFd { fd: 999, events: POLLIN, revents: 0 },  // invalid
    ];

    let poll_ret = unsafe { poll(multi_fds.as_mut_ptr(), 3, 0) };

    println!("  poll() returned: {}", poll_ret);
    println!("  stdin revents={:#06x}, stdout revents={:#06x}, invalid revents={:#06x}",
             multi_fds[0].revents, multi_fds[1].revents, multi_fds[2].revents);

    if poll_ret < 0 {
        fail("poll() on multiple fds failed");
    }
    if multi_fds[1].revents & POLLOUT == 0 {
        fail("stdout should have POLLOUT");
    }
    if multi_fds[2].revents & POLLNVAL == 0 {
        fail("invalid fd should have POLLNVAL");
    }
    println!("  OK: Multiple fds poll works correctly");

    // Phase 8: Poll TCP listener for first-call readiness
    println!("Phase 8: Polling TCP listener for POLLIN...");
    let server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server_fd < 0 {
        println!("  socket() returned: {}", server_fd);
        fail("socket() failed");
    }

    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9091);
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

    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 9091);
    let connect_ret = unsafe {
        connect(client_fd, &loopback_addr, std::mem::size_of::<SockAddrIn>() as u32)
    };
    if connect_ret < 0 {
        println!("  connect() returned: {}", connect_ret);
        fail("connect() failed");
    }

    let mut listen_fds = [PollFd {
        fd: server_fd,
        events: POLLIN,
        revents: 0,
    }];
    let poll_ret = unsafe { poll(listen_fds.as_mut_ptr(), 1, 0) };

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, listen_fds[0].revents);

    if poll_ret < 0 {
        fail("poll() on listener failed");
    }
    if poll_ret != 1 {
        fail("poll() should return 1 for listener readiness");
    }
    if listen_fds[0].revents & POLLIN == 0 {
        fail("Listener should have POLLIN set on first poll");
    }
    println!("  OK: Listener POLLIN set on first poll");

    unsafe { close(client_fd); close(server_fd); }

    // Clean up
    println!("Phase 9: Cleanup...");
    unsafe { close(pipefd[0]); }
    println!("  Closed remaining fds");

    // All tests passed
    println!("USERSPACE POLL: ALL TESTS PASSED");
    println!("POLL_TEST_PASSED");
    process::exit(0);
}
