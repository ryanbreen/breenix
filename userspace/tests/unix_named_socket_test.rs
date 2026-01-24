//! Named Unix domain socket test program
//!
//! Tests bind/listen/accept/connect for AF_UNIX sockets using abstract paths.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{self, close};
use libbreenix::process;
use libbreenix::socket::{
    socket, listen, accept, bind_unix, connect_unix,
    SockAddrUn, AF_UNIX, SOCK_STREAM, SOCK_NONBLOCK,
};
use libbreenix::syscall::{nr, raw};

/// Helper to write a file descriptor using raw syscall
fn write_fd(fd: i32, data: &[u8]) -> Result<usize, i32> {
    let ret = unsafe {
        raw::syscall3(nr::WRITE, fd as u64, data.as_ptr() as u64, data.len() as u64)
    } as i64;

    if ret < 0 {
        Err(-ret as i32)
    } else {
        Ok(ret as usize)
    }
}

/// Helper to read from a file descriptor using raw syscall
fn read_fd(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let ret = unsafe {
        raw::syscall3(nr::READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    } as i64;

    if ret < 0 {
        Err(-ret as i32)
    } else {
        Ok(ret as usize)
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
    io::print("UNIX_NAMED: FAIL - ");
    io::print(msg);
    io::print("\n");
    process::exit(1);
}

// Error codes
const EAGAIN: i32 = 11;
const EINVAL: i32 = 22;
const EOPNOTSUPP: i32 = 95;
const EADDRINUSE: i32 = 98;
const EISCONN: i32 = 106;
const ECONNREFUSED: i32 = 111;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("=== Named Unix Socket Test ===\n");

    // Phase 1: Basic server-client communication
    io::print("Phase 1: Basic server-client...\n");
    test_basic_server_client();
    io::print("  PASSED\n");

    // Phase 2: Test ECONNREFUSED on non-existent path
    io::print("Phase 2: ECONNREFUSED on non-existent path...\n");
    test_econnrefused();
    io::print("  PASSED\n");

    // Phase 3: Test EADDRINUSE on duplicate bind
    io::print("Phase 3: EADDRINUSE on duplicate bind...\n");
    test_eaddrinuse();
    io::print("  PASSED\n");

    // Phase 4: Test non-blocking accept
    io::print("Phase 4: Non-blocking accept (EAGAIN)...\n");
    test_nonblock_accept();
    io::print("  PASSED\n");

    // Phase 5: Test EINVAL on listen for unbound socket
    io::print("Phase 5: EINVAL on listen for unbound socket...\n");
    test_listen_unbound();
    io::print("  PASSED\n");

    // Phase 6: Test backlog full returns ECONNREFUSED
    io::print("Phase 6: Backlog full returns ECONNREFUSED...\n");
    test_backlog_full();
    io::print("  PASSED\n");

    // Phase 7: Test EISCONN on already-connected socket
    io::print("Phase 7: EISCONN on already-connected socket...\n");
    test_eisconn();
    io::print("  PASSED\n");

    // Phase 8: Test accept on non-listener socket
    io::print("Phase 8: Accept on non-listener socket...\n");
    test_accept_non_listener();
    io::print("  PASSED\n");

    // All tests passed
    io::print("=== All Named Unix Socket Tests PASSED ===\n");
    io::print("UNIX_NAMED_SOCKET_TEST_PASSED\n");
    process::exit(0);
}

/// Test basic server-client communication
fn test_basic_server_client() {
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
    io::print("  Server socket created: fd=");
    print_num(server_fd as i64);
    io::print("\n");

    // Create abstract socket address
    let addr = SockAddrUn::abstract_socket(b"test_server_1");

    // Bind server socket
    if let Err(e) = bind_unix(server_fd, &addr) {
        io::print("  bind() failed: ");
        print_num(e as i64);
        io::print("\n");
        close(server_fd as u64);
        fail("bind() failed");
    }
    io::print("  Server bound to abstract path\n");

    // Listen for connections
    if let Err(e) = listen(server_fd, 5) {
        io::print("  listen() failed: ");
        print_num(e as i64);
        io::print("\n");
        close(server_fd as u64);
        fail("listen() failed");
    }
    io::print("  Server listening\n");

    // Create client socket
    let client_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  client socket() failed: ");
            print_num(e as i64);
            io::print("\n");
            close(server_fd as u64);
            fail("socket() for client failed");
        }
    };
    io::print("  Client socket created: fd=");
    print_num(client_fd as i64);
    io::print("\n");

    // Connect to server
    if let Err(e) = connect_unix(client_fd, &addr) {
        io::print("  connect() failed: ");
        print_num(e as i64);
        io::print("\n");
        close(client_fd as u64);
        close(server_fd as u64);
        fail("connect() failed");
    }
    io::print("  Client connected to server\n");

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
    io::print("  Server accepted connection: fd=");
    print_num(accepted_fd as i64);
    io::print("\n");

    // Test bidirectional I/O
    // Client sends to server
    let test_data = b"Hello from client!";
    match write_fd(client_fd, test_data) {
        Ok(n) => {
            io::print("  Client wrote ");
            print_num(n as i64);
            io::print(" bytes\n");
        }
        Err(e) => {
            io::print("  client write() failed: ");
            print_num(e as i64);
            io::print("\n");
            fail("client write failed");
        }
    }

    // Server receives from client
    let mut buf = [0u8; 64];
    match read_fd(accepted_fd, &mut buf) {
        Ok(n) => {
            io::print("  Server received ");
            print_num(n as i64);
            io::print(" bytes\n");
            if &buf[..n] != test_data {
                fail("Data mismatch: client -> server");
            }
        }
        Err(e) => {
            io::print("  server read() failed: ");
            print_num(e as i64);
            io::print("\n");
            fail("server read failed");
        }
    }

    // Server sends to client
    let reply_data = b"Hello from server!";
    match write_fd(accepted_fd, reply_data) {
        Ok(n) => {
            io::print("  Server wrote ");
            print_num(n as i64);
            io::print(" bytes\n");
        }
        Err(e) => {
            io::print("  server write() failed: ");
            print_num(e as i64);
            io::print("\n");
            fail("server write failed");
        }
    }

    // Client receives from server
    let mut buf2 = [0u8; 64];
    match read_fd(client_fd, &mut buf2) {
        Ok(n) => {
            io::print("  Client received ");
            print_num(n as i64);
            io::print(" bytes\n");
            if &buf2[..n] != reply_data {
                fail("Data mismatch: server -> client");
            }
        }
        Err(e) => {
            io::print("  client read() failed: ");
            print_num(e as i64);
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
fn test_econnrefused() {
    let client_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    // Try to connect to a path that doesn't exist
    let addr = SockAddrUn::abstract_socket(b"nonexistent_path_xyz");

    match connect_unix(client_fd, &addr) {
        Ok(_) => {
            close(client_fd as u64);
            fail("connect() should have failed with ECONNREFUSED");
        }
        Err(e) => {
            io::print("  connect() returned: ");
            print_num(e as i64);
            io::print("\n");
            if e != ECONNREFUSED {
                close(client_fd as u64);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    close(client_fd as u64);
}

/// Test EADDRINUSE when binding same path twice
fn test_eaddrinuse() {
    // Create and bind first socket
    let fd1 = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"test_addrinuse");

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

    // This bind should fail with EADDRINUSE
    match bind_unix(fd2, &addr) {
        Ok(_) => {
            close(fd1 as u64);
            close(fd2 as u64);
            fail("Second bind() should have failed with EADDRINUSE");
        }
        Err(e) => {
            io::print("  Second bind() returned: ");
            print_num(e as i64);
            io::print("\n");
            if e != EADDRINUSE {
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
fn test_nonblock_accept() {
    // Create server socket with SOCK_NONBLOCK
    let server_fd = match socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"test_nonblock");

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
            io::print("  accept() returned: ");
            print_num(e as i64);
            io::print("\n");
            if e != EAGAIN {
                close(server_fd as u64);
                fail("Expected EAGAIN");
            }
        }
    }

    close(server_fd as u64);
}

/// Test EINVAL when calling listen on unbound socket
fn test_listen_unbound() {
    // Create socket but don't bind it
    let fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    // Try to listen without binding - should return EINVAL
    match listen(fd, 5) {
        Ok(_) => {
            close(fd as u64);
            fail("listen() should have failed with EINVAL on unbound socket");
        }
        Err(e) => {
            io::print("  listen() on unbound returned: ");
            print_num(e as i64);
            io::print("\n");
            if e != EINVAL {
                close(fd as u64);
                fail("Expected EINVAL");
            }
        }
    }

    close(fd as u64);
}

/// Test that backlog full returns ECONNREFUSED
fn test_backlog_full() {
    // Create server socket with very small backlog (1)
    let server_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"test_backlog_full");

    if let Err(_) = bind_unix(server_fd, &addr) {
        close(server_fd as u64);
        fail("bind() failed");
    }

    // Listen with backlog of 1
    if let Err(_) = listen(server_fd, 1) {
        close(server_fd as u64);
        fail("listen() failed");
    }

    // Create first client and connect (this fills the backlog)
    let client1_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            close(server_fd as u64);
            fail("socket() for client1 failed");
        }
    };

    if let Err(e) = connect_unix(client1_fd, &addr) {
        io::print("  First connect failed: ");
        print_num(e as i64);
        io::print("\n");
        close(client1_fd as u64);
        close(server_fd as u64);
        fail("First connect should succeed");
    }
    io::print("  First client connected (filled backlog)\n");

    // Create second client and try to connect - should fail with ECONNREFUSED
    let client2_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            close(client1_fd as u64);
            close(server_fd as u64);
            fail("socket() for client2 failed");
        }
    };

    match connect_unix(client2_fd, &addr) {
        Ok(_) => {
            close(client2_fd as u64);
            close(client1_fd as u64);
            close(server_fd as u64);
            fail("Second connect() should have failed with ECONNREFUSED (backlog full)");
        }
        Err(e) => {
            io::print("  Second connect() returned: ");
            print_num(e as i64);
            io::print("\n");
            if e != ECONNREFUSED {
                close(client2_fd as u64);
                close(client1_fd as u64);
                close(server_fd as u64);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    close(client2_fd as u64);
    close(client1_fd as u64);
    close(server_fd as u64);
}

/// Test EISCONN when connecting an already-connected socket
fn test_eisconn() {
    // Create server socket
    let server_fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() for server failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"test_eisconn");

    if let Err(_) = bind_unix(server_fd, &addr) {
        close(server_fd as u64);
        fail("bind() failed");
    }

    if let Err(_) = listen(server_fd, 5) {
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
        io::print("  First connect failed: ");
        print_num(e as i64);
        io::print("\n");
        close(client_fd as u64);
        close(server_fd as u64);
        fail("First connect should succeed");
    }
    io::print("  Client connected to server\n");

    // Try to connect again - should fail with EISCONN
    match connect_unix(client_fd, &addr) {
        Ok(_) => {
            close(client_fd as u64);
            close(server_fd as u64);
            fail("Second connect() should have failed with EISCONN");
        }
        Err(e) => {
            io::print("  Second connect() returned: ");
            print_num(e as i64);
            io::print("\n");
            if e != EISCONN {
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
fn test_accept_non_listener() {
    // Create an unbound socket (not listening)
    let fd = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    // Try to accept on a socket that's not listening
    // Should return EOPNOTSUPP (operation not supported) since it's not a listener
    match accept(fd, None) {
        Ok(_) => {
            close(fd as u64);
            fail("accept() on non-listener should have failed");
        }
        Err(e) => {
            io::print("  accept() on non-listener returned: ");
            print_num(e as i64);
            io::print("\n");
            // EOPNOTSUPP is returned because the socket is not a listener type
            if e != EOPNOTSUPP {
                close(fd as u64);
                fail("Expected EOPNOTSUPP");
            }
        }
    }

    // Also test on a bound but non-listening socket
    let fd2 = match socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            close(fd as u64);
            fail("socket() failed");
        }
    };

    let addr = SockAddrUn::abstract_socket(b"test_accept_nonlistener");

    if let Err(_) = bind_unix(fd2, &addr) {
        close(fd as u64);
        close(fd2 as u64);
        fail("bind() failed");
    }

    // Socket is bound but not listening - accept should fail
    match accept(fd2, None) {
        Ok(_) => {
            close(fd as u64);
            close(fd2 as u64);
            fail("accept() on bound-but-not-listening should have failed");
        }
        Err(e) => {
            io::print("  accept() on bound socket returned: ");
            print_num(e as i64);
            io::print("\n");
            // Should also be EOPNOTSUPP since it's still not a listener
            if e != EOPNOTSUPP {
                close(fd as u64);
                close(fd2 as u64);
                fail("Expected EOPNOTSUPP");
            }
        }
    }

    close(fd as u64);
    close(fd2 as u64);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in unix named socket test!\n");
    process::exit(1);
}
