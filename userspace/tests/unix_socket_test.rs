//! Unix domain socket test program
//!
//! Tests the socketpair() syscall for AF_UNIX sockets using libbreenix.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::io::{self, close, fcntl_getfd, fd_flags};
use libbreenix::process;
use libbreenix::socket::{
    socketpair, socket, listen, accept, bind_unix, connect_unix,
    SockAddrUn, AF_UNIX, AF_INET, SOCK_STREAM, SOCK_DGRAM, SOCK_NONBLOCK, SOCK_CLOEXEC,
};
use libbreenix::syscall::{nr, raw};

// Buffer size (must match kernel's UNIX_SOCKET_BUFFER_SIZE)
const UNIX_SOCKET_BUFFER_SIZE: usize = 65536;

// Error codes for named socket tests
const EAGAIN_RAW: i32 = 11;
const EOPNOTSUPP: i32 = 95;
const EADDRINUSE: i32 = 98;
const EISCONN: i32 = 106;
const ECONNREFUSED: i32 = 111;

/// Helper to write a file descriptor using raw syscall (to test sockets directly)
fn write_fd(fd: i32, data: &[u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(nr::WRITE, fd as u64, data.as_ptr() as u64, data.len() as u64)
    } as i64;

    if ret < 0 {
        Err(Errno::from_raw(-ret))
    } else {
        Ok(ret as usize)
    }
}

/// Helper to read from a file descriptor using raw syscall
fn read_fd(fd: i32, buf: &mut [u8]) -> Result<usize, Errno> {
    let ret = unsafe {
        raw::syscall3(nr::READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;

    if ret < 0 {
        Err(Errno::from_raw(-ret))
    } else {
        Ok(ret as usize)
    }
}

/// Helper to call socketpair with a raw pointer (for testing EFAULT)
fn socketpair_raw(domain: i32, sock_type: i32, protocol: i32, sv_ptr: u64) -> Result<(), Errno> {
    let ret = unsafe {
        raw::syscall4(nr::SOCKETPAIR, domain as u64, sock_type as u64, protocol as u64, sv_ptr)
    } as i64;

    if ret < 0 {
        Err(Errno::from_raw(-ret))
    } else {
        Ok(())
    }
}

fn print_num(n: i64) {
    let mut buf = [0u8; 21];
    let mut i = 20;
    let negative = n < 0;
    let mut n = if negative { (-n) as u64 } else { n as u64 };

    if n == 0 {
        io::print("0");
        return;
    }

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }

    if negative {
        buf[i] = b'-';
        i -= 1;
    }

    if let Ok(s) = core::str::from_utf8(&buf[i + 1..]) {
        io::print(s);
    }
}

fn fail(msg: &str) -> ! {
    io::print("UNIX_SOCKET: FAIL - ");
    io::print(msg);
    io::print("\n");
    process::exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("=== Unix Socket Test ===\n");

    // Phase 1: Create socket pair
    io::print("Phase 1: Creating socket pair with socketpair()...\n");
    let (sv0, sv1) = match socketpair(AF_UNIX, SOCK_STREAM, 0) {
        Ok(pair) => pair,
        Err(e) => {
            io::print("  socketpair() returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("socketpair() failed");
        }
    };

    io::print("  Socket pair created successfully\n");
    io::print("  sv[0] = ");
    print_num(sv0 as i64);
    io::print("\n  sv[1] = ");
    print_num(sv1 as i64);
    io::print("\n");

    // Validate fd numbers are reasonable (should be >= 3 after stdin/stdout/stderr)
    if sv0 < 3 || sv1 < 3 {
        fail("Socket fds should be >= 3 (after stdin/stdout/stderr)");
    }
    if sv0 == sv1 {
        fail("Socket fds should be different");
    }
    io::print("  FD numbers are valid\n");

    // Phase 2: Write from sv[0], read from sv[1]
    io::print("Phase 2: Writing from sv[0], reading from sv[1]...\n");
    let test_data = b"Hello from sv[0]!";
    let write_ret = match write_fd(sv0, test_data) {
        Ok(n) => n,
        Err(e) => {
            io::print("  write(sv[0]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("write to sv[0] failed");
        }
    };

    io::print("  Wrote ");
    print_num(write_ret as i64);
    io::print(" bytes to sv[0]\n");

    if write_ret != test_data.len() {
        fail("Did not write expected number of bytes");
    }

    // Read from sv[1]
    let mut read_buf = [0u8; 32];
    let read_ret = match read_fd(sv1, &mut read_buf) {
        Ok(n) => n,
        Err(e) => {
            io::print("  read(sv[1]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("read from sv[1] failed");
        }
    };

    io::print("  Read ");
    print_num(read_ret as i64);
    io::print(" bytes from sv[1]\n");

    if read_ret != test_data.len() {
        fail("Did not read expected number of bytes");
    }

    // Verify data matches
    let read_slice = &read_buf[..read_ret];
    if read_slice != test_data {
        fail("Data verification failed (sv[0] -> sv[1])");
    }
    io::print("  Data verified: '");
    if let Ok(s) = core::str::from_utf8(read_slice) {
        io::print(s);
    }
    io::print("'\n");

    // Phase 3: Write from sv[1], read from sv[0] (reverse direction)
    io::print("Phase 3: Writing from sv[1], reading from sv[0]...\n");
    let test_data2 = b"Reply from sv[1]!";
    let write_ret2 = match write_fd(sv1, test_data2) {
        Ok(n) => n,
        Err(e) => {
            io::print("  write(sv[1]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("write to sv[1] failed");
        }
    };

    io::print("  Wrote ");
    print_num(write_ret2 as i64);
    io::print(" bytes to sv[1]\n");

    // Read from sv[0]
    let mut read_buf2 = [0u8; 32];
    let read_ret2 = match read_fd(sv0, &mut read_buf2) {
        Ok(n) => n,
        Err(e) => {
            io::print("  read(sv[0]) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("read from sv[0] failed");
        }
    };

    io::print("  Read ");
    print_num(read_ret2 as i64);
    io::print(" bytes from sv[0]\n");

    let read_slice2 = &read_buf2[..read_ret2];
    if read_slice2 != test_data2 {
        fail("Data verification failed (sv[1] -> sv[0])");
    }
    io::print("  Bidirectional communication works!\n");

    // Phase 4: Close sv[0], verify sv[1] sees EOF
    io::print("Phase 4: Testing EOF on peer close...\n");
    let close_ret = close(sv0 as u64);
    if close_ret < 0 {
        io::print("  close(sv[0]) returned error: ");
        print_num(close_ret);
        io::print("\n");
        fail("close(sv[0]) failed");
    }
    io::print("  Closed sv[0]\n");

    // Read from sv[1] should return 0 (EOF)
    let mut eof_buf = [0u8; 8];
    let eof_ret = match read_fd(sv1, &mut eof_buf) {
        Ok(n) => n as i64,
        Err(e) => -(e as i64),
    };

    io::print("  Read from sv[1] returned: ");
    print_num(eof_ret);
    io::print("\n");

    if eof_ret != 0 {
        fail("Expected EOF (0) after peer close");
    }
    io::print("  EOF on peer close works!\n");

    // Phase 5: Close sv[1]
    io::print("Phase 5: Closing sv[1]...\n");
    let close_ret2 = close(sv1 as u64);
    if close_ret2 < 0 {
        io::print("  close(sv[1]) returned error: ");
        print_num(close_ret2);
        io::print("\n");
        fail("close(sv[1]) failed");
    }
    io::print("  Closed sv[1]\n");

    // Phase 6: Test SOCK_NONBLOCK - read should return EAGAIN when no data
    io::print("Phase 6: Testing SOCK_NONBLOCK (EAGAIN on empty read)...\n");
    let (sv_nb0, sv_nb1) = match socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(pair) => pair,
        Err(e) => {
            io::print("  socketpair(SOCK_NONBLOCK) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("socketpair(SOCK_NONBLOCK) failed");
        }
    };
    io::print("  Created non-blocking socket pair\n");
    io::print("  sv_nb[0] = ");
    print_num(sv_nb0 as i64);
    io::print(", sv_nb[1] = ");
    print_num(sv_nb1 as i64);
    io::print("\n");

    // Try to read from empty socket - should return EAGAIN
    let mut nb_buf = [0u8; 8];
    match read_fd(sv_nb1, &mut nb_buf) {
        Ok(n) => {
            io::print("  Read returned ");
            print_num(n as i64);
            io::print(" instead of EAGAIN\n");
            fail("Non-blocking read should return EAGAIN when no data available");
        }
        Err(e) => {
            io::print("  Read from empty non-blocking socket returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EAGAIN {
                io::print("  Expected EAGAIN, got different error\n");
                fail("Non-blocking read should return EAGAIN when no data available");
            }
        }
    }
    io::print("  SOCK_NONBLOCK works correctly!\n");

    // Clean up non-blocking sockets
    close(sv_nb0 as u64);
    close(sv_nb1 as u64);

    // Phase 7: Test EPIPE - write to socket after peer closed
    io::print("Phase 7: Testing EPIPE (write to closed peer)...\n");
    let (sv_pipe0, sv_pipe1) = match socketpair(AF_UNIX, SOCK_STREAM, 0) {
        Ok(pair) => pair,
        Err(_) => fail("socketpair() for EPIPE test failed"),
    };
    io::print("  Created socket pair for EPIPE test\n");

    // Close the reader end
    close(sv_pipe1 as u64);
    io::print("  Closed sv_pipe[1] (reader)\n");

    // Try to write to the socket whose peer is closed
    let pipe_data = b"This should fail";
    match write_fd(sv_pipe0, pipe_data) {
        Ok(n) => {
            io::print("  Write returned ");
            print_num(n as i64);
            io::print(" instead of EPIPE\n");
            fail("Write to closed peer should return EPIPE");
        }
        Err(e) => {
            io::print("  Write to socket with closed peer returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EPIPE {
                io::print("  Expected EPIPE, got different error\n");
                fail("Write to closed peer should return EPIPE");
            }
        }
    }
    io::print("  EPIPE works correctly!\n");

    // Clean up
    close(sv_pipe0 as u64);

    // Phase 8: Test error handling - wrong domain and type
    io::print("Phase 8: Testing error handling (invalid domain/type)...\n");

    // Test 8a: AF_INET should return EAFNOSUPPORT
    match socketpair(AF_INET, SOCK_STREAM, 0) {
        Ok(_) => fail("socketpair(AF_INET) should fail"),
        Err(e) => {
            io::print("  socketpair(AF_INET) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            // socketpair returns raw errno as i32
            if e != 97 {
                // EAFNOSUPPORT = 97
                io::print("  Expected EAFNOSUPPORT (97)\n");
                fail("socketpair(AF_INET) should return EAFNOSUPPORT");
            }
        }
    }
    io::print("  AF_INET correctly rejected with EAFNOSUPPORT\n");

    // Test 8b: SOCK_DGRAM should return EINVAL (not yet implemented)
    match socketpair(AF_UNIX, SOCK_DGRAM, 0) {
        Ok(_) => fail("socketpair(SOCK_DGRAM) should fail"),
        Err(e) => {
            io::print("  socketpair(SOCK_DGRAM) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            // socketpair returns raw errno as i32
            if e != 22 {
                // EINVAL = 22
                io::print("  Expected EINVAL (22)\n");
                fail("socketpair(SOCK_DGRAM) should return EINVAL");
            }
        }
    }
    io::print("  SOCK_DGRAM correctly rejected with EINVAL\n");

    io::print("  Error handling works correctly!\n");

    // Phase 9: Test buffer-full scenario (EAGAIN on write when buffer is full)
    io::print("Phase 9: Testing buffer-full (EAGAIN on non-blocking write)...\n");
    let (sv_buf0, sv_buf1) = match socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(pair) => pair,
        Err(_) => fail("socketpair() for buffer-full test failed"),
    };
    io::print("  Created non-blocking socket pair for buffer test\n");

    // Fill the buffer by writing chunks until EAGAIN
    let chunk = [0x42u8; 4096]; // 4KB chunks
    let mut total_written: usize = 0;
    let mut eagain_received = false;

    // Write until we get EAGAIN (buffer full)
    while total_written < UNIX_SOCKET_BUFFER_SIZE + 4096 {
        match write_fd(sv_buf0, &chunk) {
            Ok(n) => {
                total_written += n;
            }
            Err(e) => {
                if e == Errno::EAGAIN {
                    eagain_received = true;
                    io::print("  Got EAGAIN after writing ");
                    print_num(total_written as i64);
                    io::print(" bytes\n");
                    break;
                } else {
                    io::print("  Unexpected error during buffer fill: ");
                    print_num(-(e as i64));
                    io::print("\n");
                    fail("Unexpected error while filling buffer");
                }
            }
        }
    }

    if !eagain_received {
        io::print("  Wrote ");
        print_num(total_written as i64);
        io::print(" bytes without EAGAIN\n");
        fail("Expected EAGAIN when buffer is full");
    }

    // Verify we wrote at least UNIX_SOCKET_BUFFER_SIZE bytes before EAGAIN
    if total_written < UNIX_SOCKET_BUFFER_SIZE {
        io::print("  Only wrote ");
        print_num(total_written as i64);
        io::print(" bytes, expected at least ");
        print_num(UNIX_SOCKET_BUFFER_SIZE as i64);
        io::print("\n");
        fail("Buffer should hold at least UNIX_SOCKET_BUFFER_SIZE bytes");
    }
    io::print("  Buffer-full test passed!\n");

    // Clean up
    close(sv_buf0 as u64);
    close(sv_buf1 as u64);

    // Phase 10: Test NULL sv_ptr (should return EFAULT)
    io::print("Phase 10: Testing NULL sv_ptr (EFAULT)...\n");
    match socketpair_raw(AF_UNIX, SOCK_STREAM, 0, 0) {
        Ok(_) => fail("socketpair(NULL) should fail"),
        Err(e) => {
            io::print("  socketpair(NULL) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EFAULT {
                io::print("  Expected EFAULT\n");
                fail("socketpair(NULL) should return EFAULT");
            }
        }
    }
    io::print("  NULL sv_ptr correctly rejected with EFAULT\n");

    // Phase 11: Test non-zero protocol (should return EINVAL)
    io::print("Phase 11: Testing non-zero protocol (EINVAL)...\n");
    let mut sv_proto: [i32; 2] = [0, 0];
    match socketpair_raw(AF_UNIX, SOCK_STREAM, 1, sv_proto.as_mut_ptr() as u64) {
        Ok(_) => fail("socketpair(protocol=1) should fail"),
        Err(e) => {
            io::print("  socketpair(protocol=1) returned: ");
            print_num(-(e as i64));
            io::print("\n");
            if e != Errno::EINVAL {
                io::print("  Expected EINVAL\n");
                fail("socketpair(protocol!=0) should return EINVAL");
            }
        }
    }
    io::print("  Non-zero protocol correctly rejected with EINVAL\n");

    // Phase 12: Test SOCK_CLOEXEC flag
    io::print("Phase 12: Testing SOCK_CLOEXEC flag...\n");
    let (sv_cloexec0, sv_cloexec1) = match socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0) {
        Ok(pair) => pair,
        Err(e) => {
            io::print("  socketpair(SOCK_CLOEXEC) returned error: ");
            print_num(e as i64);
            io::print("\n");
            fail("socketpair(SOCK_CLOEXEC) failed");
        }
    };
    io::print("  Created socket pair with SOCK_CLOEXEC\n");

    // Verify FD_CLOEXEC is set on both fds using fcntl(F_GETFD)
    let flags0 = fcntl_getfd(sv_cloexec0 as u64);
    let flags1 = fcntl_getfd(sv_cloexec1 as u64);

    io::print("  sv_cloexec[0] flags: ");
    print_num(flags0);
    io::print(", sv_cloexec[1] flags: ");
    print_num(flags1);
    io::print("\n");

    if flags0 < 0 || flags1 < 0 {
        io::print("  fcntl(F_GETFD) failed\n");
        fail("fcntl(F_GETFD) failed on SOCK_CLOEXEC socket");
    }

    if (flags0 & fd_flags::FD_CLOEXEC as i64) == 0 {
        fail("sv_cloexec[0] should have FD_CLOEXEC set");
    }
    if (flags1 & fd_flags::FD_CLOEXEC as i64) == 0 {
        fail("sv_cloexec[1] should have FD_CLOEXEC set");
    }
    io::print("  FD_CLOEXEC correctly set on both sockets\n");

    // Clean up
    close(sv_cloexec0 as u64);
    close(sv_cloexec1 as u64);

    // ============================================================
    // Named Unix Socket Tests (bind/listen/accept/connect)
    // These tests verify the full socket lifecycle beyond socketpair
    // ============================================================

    io::print("\n=== Named Unix Socket Tests ===\n");

    // Phase 13: Basic server-client communication with bind/listen/accept/connect
    io::print("Phase 13: Basic server-client (bind/listen/accept/connect)...\n");
    test_named_basic_server_client();
    io::print("  Phase 13 PASSED\n");

    // Phase 14: Test ECONNREFUSED on non-existent path
    io::print("Phase 14: ECONNREFUSED on non-existent path...\n");
    test_named_econnrefused();
    io::print("  Phase 14 PASSED\n");

    // Phase 15: Test EADDRINUSE on duplicate bind
    io::print("Phase 15: EADDRINUSE on duplicate bind...\n");
    test_named_eaddrinuse();
    io::print("  Phase 15 PASSED\n");

    // Phase 16: Test non-blocking accept returns EAGAIN
    io::print("Phase 16: Non-blocking accept (EAGAIN)...\n");
    test_named_nonblock_accept();
    io::print("  Phase 16 PASSED\n");

    // Phase 17: Test EISCONN on already-connected socket
    io::print("Phase 17: EISCONN on already-connected socket...\n");
    test_named_eisconn();
    io::print("  Phase 17 PASSED\n");

    // Phase 18: Test accept on non-listener socket
    io::print("Phase 18: Accept on non-listener socket...\n");
    test_named_accept_non_listener();
    io::print("  Phase 18 PASSED\n");

    // All tests passed
    io::print("=== Unix Socket Test PASSED ===\n");
    io::print("UNIX_SOCKET_TEST_PASSED\n");
    process::exit(0);
}

// ============================================================
// Named Unix Socket Test Functions
// ============================================================

/// Test basic server-client communication with bind/listen/accept/connect
fn test_named_basic_server_client() {
    // Create server socket
    let server_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  socket() failed: ");
            print_num(e as i64);
            io::print("\n");
            fail("socket() for server failed");
        }
    };

    // Create abstract socket address
    let addr = SockAddrUn::abstract_socket(b"combined_test_1");

    // Bind server socket
    if let Err(e) = bind_unix(server_fd, &addr) {
        io::print("  bind() failed: ");
        print_num(e as i64);
        io::print("\n");
        close(server_fd as u64);
        fail("bind() failed");
    }

    // Listen for connections
    if let Err(e) = listen(server_fd, 5) {
        io::print("  listen() failed: ");
        print_num(e as i64);
        io::print("\n");
        close(server_fd as u64);
        fail("listen() failed");
    }

    // Create client socket and connect
    let client_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            close(server_fd as u64);
            fail("socket() for client failed");
        }
    };

    if let Err(e) = connect_unix(client_fd, &addr) {
        io::print("  connect() failed: ");
        print_num(e as i64);
        io::print("\n");
        close(client_fd as u64);
        close(server_fd as u64);
        fail("connect() failed");
    }

    // Accept connection on server
    let accepted_fd = match accept(server_fd, None) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  accept() failed: ");
            print_num(e as i64);
            io::print("\n");
            close(client_fd as u64);
            close(server_fd as u64);
            fail("accept() failed");
        }
    };

    // Test bidirectional I/O - client to server
    let test_data = b"Hello from client!";
    if let Err(e) = write_fd(client_fd, test_data) {
        io::print("  client write() failed: ");
        print_num(-(e as i64));
        io::print("\n");
        fail("client write failed");
    }

    let mut buf = [0u8; 64];
    match read_fd(accepted_fd, &mut buf) {
        Ok(n) => {
            if &buf[..n] != test_data {
                fail("Data mismatch: client -> server");
            }
        }
        Err(e) => {
            io::print("  server read() failed: ");
            print_num(-(e as i64));
            io::print("\n");
            fail("server read failed");
        }
    }

    // Server to client
    let reply_data = b"Hello from server!";
    if let Err(e) = write_fd(accepted_fd, reply_data) {
        io::print("  server write() failed: ");
        print_num(-(e as i64));
        io::print("\n");
        fail("server write failed");
    }

    let mut buf2 = [0u8; 64];
    match read_fd(client_fd, &mut buf2) {
        Ok(n) => {
            if &buf2[..n] != reply_data {
                fail("Data mismatch: server -> client");
            }
        }
        Err(e) => {
            io::print("  client read() failed: ");
            print_num(-(e as i64));
            io::print("\n");
            fail("client read failed");
        }
    }

    io::print("  Bidirectional I/O works!\n");

    // Clean up
    close(accepted_fd as u64);
    close(client_fd as u64);
    close(server_fd as u64);
}

/// Test ECONNREFUSED when connecting to non-existent path
fn test_named_econnrefused() {
    let client_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"nonexistent_combined");

    match connect_unix(client_fd, &addr) {
        Ok(_) => {
            close(client_fd as u64);
            fail("connect() should have failed with ECONNREFUSED");
        }
        Err(e) => {
            if e != ECONNREFUSED {
                io::print("  Expected ECONNREFUSED, got: ");
                print_num(e as i64);
                io::print("\n");
                close(client_fd as u64);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    close(client_fd as u64);
}

/// Test EADDRINUSE when binding same path twice
fn test_named_eaddrinuse() {
    let fd1 = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"combined_addrinuse");

    if let Err(_) = bind_unix(fd1, &addr) {
        close(fd1 as u64);
        fail("First bind() failed");
    }

    if let Err(_) = listen(fd1, 5) {
        close(fd1 as u64);
        fail("listen() failed");
    }

    // Create second socket and try to bind to same path
    let fd2 = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            close(fd1 as u64);
            fail("socket() failed");
        }
    };

    match bind_unix(fd2, &addr) {
        Ok(_) => {
            close(fd1 as u64);
            close(fd2 as u64);
            fail("Second bind() should have failed with EADDRINUSE");
        }
        Err(e) => {
            if e != EADDRINUSE {
                io::print("  Expected EADDRINUSE, got: ");
                print_num(e as i64);
                io::print("\n");
                close(fd1 as u64);
                close(fd2 as u64);
                fail("Expected EADDRINUSE");
            }
        }
    }

    close(fd1 as u64);
    close(fd2 as u64);
}

/// Test non-blocking accept returns EAGAIN
fn test_named_nonblock_accept() {
    let server_fd = match socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"combined_nonblock");

    if let Err(_) = bind_unix(server_fd, &addr) {
        close(server_fd as u64);
        fail("bind() failed");
    }

    if let Err(_) = listen(server_fd, 5) {
        close(server_fd as u64);
        fail("listen() failed");
    }

    // Try to accept without any pending connections
    match accept(server_fd, None) {
        Ok(_) => {
            close(server_fd as u64);
            fail("accept() should have returned EAGAIN");
        }
        Err(e) => {
            if e != EAGAIN_RAW {
                io::print("  Expected EAGAIN, got: ");
                print_num(e as i64);
                io::print("\n");
                close(server_fd as u64);
                fail("Expected EAGAIN");
            }
        }
    }

    close(server_fd as u64);
}

/// Test EISCONN when connecting an already-connected socket
fn test_named_eisconn() {
    let server_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() for server failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"combined_eisconn");

    if let Err(_) = bind_unix(server_fd, &addr) {
        close(server_fd as u64);
        fail("bind() failed");
    }

    if let Err(_) = listen(server_fd, 5) {
        close(server_fd as u64);
        fail("listen() failed");
    }

    let client_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            close(server_fd as u64);
            fail("socket() for client failed");
        }
    };

    // First connect should succeed
    if let Err(e) = connect_unix(client_fd, &addr) {
        io::print("  First connect failed: ");
        print_num(e as i64);
        io::print("\n");
        close(client_fd as u64);
        close(server_fd as u64);
        fail("First connect should succeed");
    }

    // Second connect on same socket should fail with EISCONN
    match connect_unix(client_fd, &addr) {
        Ok(_) => {
            close(client_fd as u64);
            close(server_fd as u64);
            fail("Second connect() should have failed with EISCONN");
        }
        Err(e) => {
            if e != EISCONN {
                io::print("  Expected EISCONN, got: ");
                print_num(e as i64);
                io::print("\n");
                close(client_fd as u64);
                close(server_fd as u64);
                fail("Expected EISCONN");
            }
        }
    }

    close(client_fd as u64);
    close(server_fd as u64);
}

/// Test accept on a non-listener socket returns error
fn test_named_accept_non_listener() {
    // Create an unbound socket (not listening)
    let fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    // Try to accept on a socket that's not listening - should return EOPNOTSUPP
    match accept(fd, None) {
        Ok(_) => {
            close(fd as u64);
            fail("accept() on non-listener should have failed");
        }
        Err(e) => {
            if e != EOPNOTSUPP {
                io::print("  Expected EOPNOTSUPP, got: ");
                print_num(e as i64);
                io::print("\n");
                close(fd as u64);
                fail("Expected EOPNOTSUPP");
            }
        }
    }

    close(fd as u64);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in unix socket test!\n");
    process::exit(1);
}
