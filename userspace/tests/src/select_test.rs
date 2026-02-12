//! Select syscall test program (std version)
//!
//! Tests the select() syscall for monitoring file descriptors.

use libbreenix::io;
use libbreenix::io::{FdSet, fd_zero, fd_set_bit, fd_isset};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::types::Fd;
use std::process;

fn fail(msg: &str) -> ! {
    println!("USERSPACE SELECT: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== Select Test Program ===");

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

    // Phase 2: select on empty pipe for read (should not be ready)
    println!("Phase 2: Selecting on empty pipe for read...");
    let mut readfds: FdSet = 0;
    fd_zero(&mut readfds);
    fd_set_bit(pipe_read, &mut readfds);

    let nfds = pipe_read.raw() as i32 + 1;
    let select_ret = io::select(nfds, Some(&mut readfds), None, None, 0).unwrap();

    println!("  select() returned: {}, readfds={:#x}", select_ret, readfds);

    if fd_isset(pipe_read, &readfds) {
        fail("Empty pipe should not be ready for read");
    }
    println!("  OK: Empty pipe is not ready for read");

    // Phase 3: Write data to pipe, then select for read
    println!("Phase 3: Writing data and selecting for read...");
    let test_data = b"Test";
    let write_ret = io::write(pipe_write, test_data).unwrap();

    if write_ret != test_data.len() {
        println!("  write() returned: {}", write_ret);
        fail("write to pipe failed");
    }

    fd_zero(&mut readfds);
    fd_set_bit(pipe_read, &mut readfds);

    let select_ret = io::select(nfds, Some(&mut readfds), None, None, 0).unwrap();

    println!("  select() returned: {}, readfds={:#x}", select_ret, readfds);

    if select_ret != 1 {
        fail("select() should return 1 when pipe has data");
    }
    if !fd_isset(pipe_read, &readfds) {
        fail("Pipe with data should be ready for read");
    }
    println!("  OK: Pipe with data is ready for read");

    // Phase 4: Select on write end for write
    println!("Phase 4: Selecting on write end for write...");
    let mut writefds: FdSet = 0;
    fd_zero(&mut writefds);
    fd_set_bit(pipe_write, &mut writefds);

    let nfds_write = pipe_write.raw() as i32 + 1;
    let select_ret = io::select(nfds_write, None, Some(&mut writefds), None, 0).unwrap();

    println!("  select() returned: {}, writefds={:#x}", select_ret, writefds);

    if !fd_isset(pipe_write, &writefds) {
        fail("Write end should be ready for write");
    }
    println!("  OK: Write end is ready for write");

    // Phase 5: Select with multiple fd_sets
    println!("Phase 5: Selecting with multiple fd_sets...");
    fd_zero(&mut readfds);
    fd_set_bit(pipe_read, &mut readfds);
    fd_zero(&mut writefds);
    fd_set_bit(pipe_write, &mut writefds);

    let max_fd = if pipe_read.raw() > pipe_write.raw() { pipe_read.raw() } else { pipe_write.raw() };
    let nfds_multi = max_fd as i32 + 1;

    let select_ret = io::select(nfds_multi, Some(&mut readfds), Some(&mut writefds), None, 0).unwrap();

    println!("  select() returned: {}", select_ret);
    println!("  readfds={:#x}, writefds={:#x}", readfds, writefds);

    if select_ret < 2 {
        fail("Expected at least 2 ready fds");
    }
    println!("  OK: Multiple fd_sets work correctly");

    // Phase 6: Close write end and check for exception on read end
    println!("Phase 6: Closing write end and checking for exception...");

    // First drain the pipe
    let mut read_buf = [0u8; 32];
    let _ = io::read(pipe_read, &mut read_buf);

    // Close write end
    if let Err(e) = io::close(pipe_write) {
        println!("  close() returned: {:?}", e);
        fail("close() on write end failed");
    }

    let mut exceptfds: FdSet = 0;
    fd_zero(&mut readfds);
    fd_set_bit(pipe_read, &mut readfds);
    fd_zero(&mut exceptfds);
    fd_set_bit(pipe_read, &mut exceptfds);

    let select_ret = io::select(nfds, Some(&mut readfds), None, Some(&mut exceptfds), 0).unwrap();

    println!("  select() returned: {}", select_ret);
    println!("  readfds={:#x}, exceptfds={:#x}", readfds, exceptfds);

    println!("  OK: select() returns after write end closed");

    // Phase 7: Test stdout writability
    println!("Phase 7: Testing stdout writability...");
    fd_zero(&mut writefds);
    fd_set_bit(Fd::STDOUT, &mut writefds);

    let select_ret = io::select(2, None, Some(&mut writefds), None, 0).unwrap();

    println!("  select() returned: {}, writefds={:#x}", select_ret, writefds);

    if !fd_isset(Fd::STDOUT, &writefds) {
        fail("stdout should be writable");
    }
    println!("  OK: stdout is writable");

    // Phase 8: Select on TCP listener for first-call readiness
    println!("Phase 8: Selecting on TCP listener...");
    let server_fd = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();

    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9092);
    socket::bind_inet(server_fd, &server_addr).unwrap();
    socket::listen(server_fd, 16).unwrap();

    let client_fd = socket::socket(AF_INET, SOCK_STREAM, 0).unwrap();

    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 9092);
    socket::connect_inet(client_fd, &loopback_addr).unwrap();

    fd_zero(&mut readfds);
    fd_set_bit(server_fd, &mut readfds);
    let nfds_tcp = server_fd.raw() as i32 + 1;
    let select_ret = io::select(nfds_tcp, Some(&mut readfds), None, None, 0).unwrap();

    println!("  select() returned: {}, readfds={:#x}", select_ret, readfds);

    if select_ret != 1 {
        fail("select() should return 1 for listener readiness");
    }
    if !fd_isset(server_fd, &readfds) {
        fail("Listener should be ready for read on first select");
    }
    println!("  OK: Listener ready on first select");

    let _ = io::close(client_fd);
    let _ = io::close(server_fd);

    // Clean up
    println!("Phase 9: Cleanup...");
    let _ = io::close(pipe_read);
    println!("  Closed remaining fds");

    // All tests passed
    println!("USERSPACE SELECT: ALL TESTS PASSED");
    println!("SELECT_TEST_PASSED");
    process::exit(0);
}
