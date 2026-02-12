//! Poll syscall test program (std version)
//!
//! Tests the poll() syscall for monitoring file descriptors.

use libbreenix::io;
use libbreenix::io::poll_events::*;
use libbreenix::io::PollFd;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::types::Fd;
use std::process;

fn fail(msg: &str) -> ! {
    println!("USERSPACE POLL: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== Poll Test Program ===");

    // Phase 1: Create a pipe for testing
    println!("Phase 1: Creating pipe...");
    let (pipe_read, pipe_write) = match io::pipe() {
        Ok(fds) => fds,
        Err(e) => {
            println!("  pipe() returned error: {:?}", e);
            fail("pipe() failed");
        }
    };

    println!("  Pipe created: read_fd={}, write_fd={}", pipe_read.raw() as i32, pipe_write.raw() as i32);

    // Phase 2: Poll empty pipe for POLLIN (should not be ready)
    println!("Phase 2: Polling empty pipe for POLLIN...");
    let mut fds = [PollFd::new(pipe_read, POLLIN)];

    let poll_ret = io::poll(&mut fds, 0).unwrap();

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if fds[0].revents & POLLIN != 0 {
        fail("Empty pipe should not have POLLIN set");
    }
    println!("  OK: Empty pipe has no POLLIN");

    // Phase 3: Write data to pipe, then poll for POLLIN
    println!("Phase 3: Writing data and polling for POLLIN...");
    let test_data = b"Test";
    let write_ret = io::write(pipe_write, test_data).unwrap();

    if write_ret != test_data.len() {
        println!("  write() returned: {}", write_ret);
        fail("write to pipe failed");
    }

    fds[0].revents = 0;
    let poll_ret = io::poll(&mut fds, 0).unwrap();

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if poll_ret != 1 {
        fail("poll() should return 1 when pipe has data");
    }
    if fds[0].revents & POLLIN == 0 {
        fail("Pipe with data should have POLLIN set");
    }
    println!("  OK: Pipe with data has POLLIN");

    // Phase 4: Poll write end for POLLOUT
    println!("Phase 4: Polling write end for POLLOUT...");
    fds[0] = PollFd::new(pipe_write, POLLOUT);

    let poll_ret = io::poll(&mut fds, 0).unwrap();

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

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

    let poll_ret = io::poll(&mut fds, 0).unwrap();

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

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
    let _ = io::read(pipe_read, &mut read_buf);

    // Close write end
    if let Err(e) = io::close(pipe_write) {
        println!("  close() returned: {:?}", e);
        fail("close() on write end failed");
    }

    fds[0] = PollFd::new(pipe_read, POLLIN);

    let poll_ret = io::poll(&mut fds, 0).unwrap();

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, fds[0].revents);

    if fds[0].revents & POLLHUP == 0 {
        fail("Read end should have POLLHUP after write end closed");
    }
    println!("  OK: Read end has POLLHUP after write end closed");

    // Phase 7: Poll multiple fds
    println!("Phase 7: Polling multiple fds...");
    let mut multi_fds = [
        PollFd::new(Fd::STDIN, POLLIN),    // stdin
        PollFd::new(Fd::STDOUT, POLLOUT),   // stdout
        PollFd { fd: 999, events: POLLIN, revents: 0 },  // invalid
    ];

    let poll_ret = io::poll(&mut multi_fds, 0).unwrap();

    println!("  poll() returned: {}", poll_ret);
    println!("  stdin revents={:#06x}, stdout revents={:#06x}, invalid revents={:#06x}",
             multi_fds[0].revents, multi_fds[1].revents, multi_fds[2].revents);

    if multi_fds[1].revents & POLLOUT == 0 {
        fail("stdout should have POLLOUT");
    }
    if multi_fds[2].revents & POLLNVAL == 0 {
        fail("invalid fd should have POLLNVAL");
    }
    println!("  OK: Multiple fds poll works correctly");

    // Phase 8: Poll TCP listener for first-call readiness
    println!("Phase 8: Polling TCP listener for POLLIN...");
    let server_fd = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();

    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9091);
    socket::bind_inet(server_fd, &server_addr).unwrap();
    socket::listen(server_fd, 16).unwrap();

    let client_fd = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();

    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 9091);
    socket::connect_inet(client_fd, &loopback_addr).unwrap();

    let mut listen_fds = [PollFd::new(server_fd, POLLIN)];
    let poll_ret = io::poll(&mut listen_fds, 0).unwrap();

    println!("  poll() returned: {}, revents={:#06x}", poll_ret, listen_fds[0].revents);

    if poll_ret != 1 {
        fail("poll() should return 1 for listener readiness");
    }
    if listen_fds[0].revents & POLLIN == 0 {
        fail("Listener should have POLLIN set on first poll");
    }
    println!("  OK: Listener POLLIN set on first poll");

    let _ = io::close(client_fd);
    let _ = io::close(server_fd);

    // Clean up
    println!("Phase 9: Cleanup...");
    let _ = io::close(pipe_read);
    println!("  Closed remaining fds");

    // All tests passed
    println!("USERSPACE POLL: ALL TESTS PASSED");
    println!("POLL_TEST_PASSED");
    process::exit(0);
}
