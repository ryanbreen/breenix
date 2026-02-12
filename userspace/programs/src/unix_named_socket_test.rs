//! Named Unix domain socket test program (std version)
//!
//! Tests bind/listen/accept/connect for AF_UNIX sockets using abstract paths.
//! Must emit "UNIX_NAMED_SOCKET_TEST_PASSED" on success.

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::socket::{self, SockAddrUn, AF_UNIX, SOCK_NONBLOCK, SOCK_STREAM};
use libbreenix::types::Fd;

fn do_socket(domain: i32, sock_type: i32, protocol: i32) -> Result<Fd, Errno> {
    socket::socket(domain, sock_type, protocol).map_err(|e| match e {
        Error::Os(errno) => errno,
    })
}

fn do_bind(fd: Fd, addr: &SockAddrUn) -> Result<(), Errno> {
    socket::bind_unix(fd, addr).map_err(|e| match e {
        Error::Os(errno) => errno,
    })
}

fn do_listen(fd: Fd, backlog: i32) -> Result<(), Errno> {
    socket::listen(fd, backlog).map_err(|e| match e {
        Error::Os(errno) => errno,
    })
}

fn do_accept(fd: Fd) -> Result<Fd, Errno> {
    socket::accept(fd, None).map_err(|e| match e {
        Error::Os(errno) => errno,
    })
}

fn do_connect(fd: Fd, addr: &SockAddrUn) -> Result<(), Errno> {
    socket::connect_unix(fd, addr).map_err(|e| match e {
        Error::Os(errno) => errno,
    })
}

fn do_close(fd: Fd) {
    let _ = io::close(fd);
}

fn do_read(fd: Fd, buf: &mut [u8]) -> Result<usize, Errno> {
    io::read(fd, buf).map_err(|e| match e {
        Error::Os(errno) => errno,
    })
}

fn do_write(fd: Fd, data: &[u8]) -> Result<usize, Errno> {
    io::write(fd, data).map_err(|e| match e {
        Error::Os(errno) => errno,
    })
}

fn fail(msg: &str) -> ! {
    println!("UNIX_NAMED: FAIL - {}", msg);
    std::process::exit(1);
}

/// Phase 1: Basic server-client communication
fn test_basic_server_client() {
    println!("Phase 1: Basic server-client...");

    let server_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|e| {
        println!("  socket() failed: {:?}", e);
        fail("socket() for server failed");
    });
    println!("  Server socket created: fd={}", server_fd.raw());

    let addr = SockAddrUn::abstract_socket(b"test_server_1");

    do_bind(server_fd, &addr).unwrap_or_else(|e| {
        println!("  bind() failed: {:?}", e);
        do_close(server_fd);
        fail("bind() failed");
    });
    println!("  Server bound to abstract path");

    do_listen(server_fd, 5).unwrap_or_else(|e| {
        println!("  listen() failed: {:?}", e);
        do_close(server_fd);
        fail("listen() failed");
    });
    println!("  Server listening");

    let client_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|e| {
        println!("  client socket() failed: {:?}", e);
        do_close(server_fd);
        fail("socket() for client failed");
    });
    println!("  Client socket created: fd={}", client_fd.raw());

    do_connect(client_fd, &addr).unwrap_or_else(|e| {
        println!("  connect() failed: {:?}", e);
        do_close(client_fd);
        do_close(server_fd);
        fail("connect() failed");
    });
    println!("  Client connected to server");

    let accepted_fd = do_accept(server_fd).unwrap_or_else(|e| {
        println!("  accept() failed: {:?}", e);
        do_close(client_fd);
        do_close(server_fd);
        fail("accept() failed");
    });
    println!("  Server accepted connection: fd={}", accepted_fd.raw());

    // Test bidirectional I/O
    let test_data = b"Hello from client!";
    do_write(client_fd, test_data).unwrap_or_else(|e| {
        println!("  client write() failed: {:?}", e);
        fail("client write failed");
    });
    println!("  Client wrote {} bytes", test_data.len());

    let mut buf = [0u8; 64];
    let n = do_read(accepted_fd, &mut buf).unwrap_or_else(|e| {
        println!("  server read() failed: {:?}", e);
        fail("server read failed");
    });
    println!("  Server received {} bytes", n);
    if &buf[..n] != test_data {
        fail("Data mismatch: client -> server");
    }

    let reply_data = b"Hello from server!";
    do_write(accepted_fd, reply_data).unwrap_or_else(|e| {
        println!("  server write() failed: {:?}", e);
        fail("server write failed");
    });
    println!("  Server wrote {} bytes", reply_data.len());

    let mut buf2 = [0u8; 64];
    let n2 = do_read(client_fd, &mut buf2).unwrap_or_else(|e| {
        println!("  client read() failed: {:?}", e);
        fail("client read failed");
    });
    println!("  Client received {} bytes", n2);
    if &buf2[..n2] != reply_data {
        fail("Data mismatch: server -> client");
    }

    println!("  Bidirectional I/O works!");

    do_close(accepted_fd);
    do_close(client_fd);
    do_close(server_fd);
    println!("  PASSED");
}

/// Phase 2: ECONNREFUSED on non-existent path
fn test_econnrefused() {
    println!("Phase 2: ECONNREFUSED on non-existent path...");

    let client_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"nonexistent_path_xyz");

    match do_connect(client_fd, &addr) {
        Ok(_) => {
            do_close(client_fd);
            fail("connect() should have failed with ECONNREFUSED");
        }
        Err(e) => {
            println!("  connect() returned: {:?}", e);
            if e != Errno::ECONNREFUSED {
                do_close(client_fd);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    do_close(client_fd);
    println!("  PASSED");
}

/// Phase 3: EADDRINUSE on duplicate bind
fn test_eaddrinuse() {
    println!("Phase 3: EADDRINUSE on duplicate bind...");

    let fd1 = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_addrinuse");

    do_bind(fd1, &addr).unwrap_or_else(|_| { do_close(fd1); fail("First bind() failed"); });
    do_listen(fd1, 5).unwrap_or_else(|_| { do_close(fd1); fail("listen() failed"); });

    let fd2 = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| { do_close(fd1); fail("socket() failed"); });

    match do_bind(fd2, &addr) {
        Ok(_) => {
            do_close(fd1);
            do_close(fd2);
            fail("Second bind() should have failed with EADDRINUSE");
        }
        Err(e) => {
            println!("  Second bind() returned: {:?}", e);
            if e != Errno::EADDRINUSE {
                do_close(fd1);
                do_close(fd2);
                fail("Expected EADDRINUSE");
            }
        }
    }

    do_close(fd1);
    do_close(fd2);
    println!("  PASSED");
}

/// Phase 4: Non-blocking accept returns EAGAIN
fn test_nonblock_accept() {
    println!("Phase 4: Non-blocking accept (EAGAIN)...");

    let server_fd = do_socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_nonblock");

    do_bind(server_fd, &addr).unwrap_or_else(|_| { do_close(server_fd); fail("bind() failed"); });
    do_listen(server_fd, 5).unwrap_or_else(|_| { do_close(server_fd); fail("listen() failed"); });

    match do_accept(server_fd) {
        Ok(_) => {
            do_close(server_fd);
            fail("accept() should have returned EAGAIN");
        }
        Err(e) => {
            println!("  accept() returned: {:?}", e);
            if e != Errno::EAGAIN {
                do_close(server_fd);
                fail("Expected EAGAIN");
            }
        }
    }

    do_close(server_fd);
    println!("  PASSED");
}

/// Phase 5: EINVAL on listen for unbound socket
fn test_listen_unbound() {
    println!("Phase 5: EINVAL on listen for unbound socket...");

    let fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));

    match do_listen(fd, 5) {
        Ok(_) => {
            do_close(fd);
            fail("listen() should have failed with EINVAL on unbound socket");
        }
        Err(e) => {
            println!("  listen() on unbound returned: {:?}", e);
            if e != Errno::EINVAL {
                do_close(fd);
                fail("Expected EINVAL");
            }
        }
    }

    do_close(fd);
    println!("  PASSED");
}

/// Phase 6: Backlog full returns ECONNREFUSED
fn test_backlog_full() {
    println!("Phase 6: Backlog full returns ECONNREFUSED...");

    let server_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_backlog_full");

    do_bind(server_fd, &addr).unwrap_or_else(|_| { do_close(server_fd); fail("bind() failed"); });
    do_listen(server_fd, 1).unwrap_or_else(|_| { do_close(server_fd); fail("listen() failed"); });

    // First client fills the backlog
    let client1_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| { do_close(server_fd); fail("socket() failed"); });
    do_connect(client1_fd, &addr).unwrap_or_else(|e| {
        println!("  First connect failed: {:?}", e);
        do_close(client1_fd);
        do_close(server_fd);
        fail("First connect should succeed");
    });
    println!("  First client connected (filled backlog)");

    // Second client should be refused
    let client2_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| {
        do_close(client1_fd);
        do_close(server_fd);
        fail("socket() failed");
    });

    match do_connect(client2_fd, &addr) {
        Ok(_) => {
            do_close(client2_fd);
            do_close(client1_fd);
            do_close(server_fd);
            fail("Second connect() should have failed with ECONNREFUSED (backlog full)");
        }
        Err(e) => {
            println!("  Second connect() returned: {:?}", e);
            if e != Errno::ECONNREFUSED {
                do_close(client2_fd);
                do_close(client1_fd);
                do_close(server_fd);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    do_close(client2_fd);
    do_close(client1_fd);
    do_close(server_fd);
    println!("  PASSED");
}

/// Phase 7: EISCONN on already-connected socket
fn test_eisconn() {
    println!("Phase 7: EISCONN on already-connected socket...");

    let server_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_eisconn");

    do_bind(server_fd, &addr).unwrap_or_else(|_| { do_close(server_fd); fail("bind() failed"); });
    do_listen(server_fd, 5).unwrap_or_else(|_| { do_close(server_fd); fail("listen() failed"); });

    let client_fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| { do_close(server_fd); fail("socket() failed"); });

    do_connect(client_fd, &addr).unwrap_or_else(|e| {
        println!("  First connect failed: {:?}", e);
        do_close(client_fd);
        do_close(server_fd);
        fail("First connect should succeed");
    });
    println!("  Client connected to server");

    // Try to connect again - should fail with EISCONN
    match do_connect(client_fd, &addr) {
        Ok(_) => {
            do_close(client_fd);
            do_close(server_fd);
            fail("Second connect() should have failed with EISCONN");
        }
        Err(e) => {
            println!("  Second connect() returned: {:?}", e);
            if e != Errno::EISCONN {
                do_close(client_fd);
                do_close(server_fd);
                fail("Expected EISCONN");
            }
        }
    }

    do_close(client_fd);
    do_close(server_fd);
    println!("  PASSED");
}

/// Phase 8: Accept on non-listener socket
fn test_accept_non_listener() {
    println!("Phase 8: Accept on non-listener socket...");

    let fd = do_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));

    match do_accept(fd) {
        Ok(_) => {
            do_close(fd);
            fail("accept() on non-listener should have failed");
        }
        Err(e) => {
            println!("  accept() on non-listener returned: {:?}", e);
            if e != Errno::EOPNOTSUPP {
                do_close(fd);
                fail("Expected EOPNOTSUPP");
            }
        }
    }

    do_close(fd);
    println!("  PASSED");
}

fn main() {
    println!("=== Named Unix Socket Test ===");

    test_basic_server_client();
    test_econnrefused();
    test_eaddrinuse();
    test_nonblock_accept();
    test_listen_unbound();
    test_backlog_full();
    test_eisconn();
    test_accept_non_listener();

    println!("\n=== All Named Unix Socket Tests PASSED ===");
    println!("UNIX_NAMED_SOCKET_TEST_PASSED");
    std::process::exit(0);
}
