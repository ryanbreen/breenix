//! Named Unix domain socket test program (std version)
//!
//! Tests bind/listen/accept/connect for AF_UNIX sockets using abstract paths.
//! Must emit "UNIX_NAMED_SOCKET_TEST_PASSED" on success.

const AF_UNIX: i32 = 1;
const SOCK_STREAM: i32 = 1;
const SOCK_NONBLOCK: i32 = 0x800;

const EAGAIN: i32 = 11;
const EINVAL: i32 = 22;
const EOPNOTSUPP: i32 = 95;
const EADDRINUSE: i32 = 98;
const EISCONN: i32 = 106;
const ECONNREFUSED: i32 = 111;

#[repr(C)]
struct SockAddrUn {
    sun_family: u16,
    sun_path: [u8; 108],
}

impl SockAddrUn {
    fn abstract_socket(name: &[u8]) -> Self {
        let mut addr = SockAddrUn {
            sun_family: AF_UNIX as u16,
            sun_path: [0u8; 108],
        };
        // Abstract socket: sun_path[0] = 0, then the name
        if name.len() < 107 {
            addr.sun_path[1..1 + name.len()].copy_from_slice(name);
        }
        addr
    }

    fn addrlen(&self) -> u32 {
        // family (2) + null byte (1) + name length
        let name_len = self.sun_path[1..].iter().position(|&b| b == 0).unwrap_or(107);
        (2 + 1 + name_len) as u32
    }
}

extern "C" {
    fn socket(domain: i32, sock_type: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const SockAddrUn, addrlen: u32) -> i32;
    fn listen(fd: i32, backlog: i32) -> i32;
    fn accept(fd: i32, addr: *mut SockAddrUn, addrlen: *mut u32) -> i32;
    fn connect(fd: i32, addr: *const SockAddrUn, addrlen: u32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

fn c_socket(domain: i32, sock_type: i32, protocol: i32) -> Result<i32, i32> {
    let ret = unsafe { socket(domain, sock_type, protocol) };
    if ret < 0 { Err(-ret) } else { Ok(ret) }
}

fn c_bind(fd: i32, addr: &SockAddrUn) -> Result<(), i32> {
    let ret = unsafe { bind(fd, addr, addr.addrlen()) };
    if ret < 0 { Err(-ret) } else { Ok(()) }
}

fn c_listen(fd: i32, backlog: i32) -> Result<(), i32> {
    let ret = unsafe { listen(fd, backlog) };
    if ret < 0 { Err(-ret) } else { Ok(()) }
}

fn c_accept(fd: i32) -> Result<i32, i32> {
    let ret = unsafe { accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ret < 0 { Err(-ret) } else { Ok(ret) }
}

fn c_connect(fd: i32, addr: &SockAddrUn) -> Result<(), i32> {
    let ret = unsafe { connect(fd, addr, addr.addrlen()) };
    if ret < 0 { Err(-ret) } else { Ok(()) }
}

fn c_close(fd: i32) {
    unsafe { close(fd); }
}

fn c_read(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let ret = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
    if ret < 0 { Err(-ret as i32) } else { Ok(ret as usize) }
}

fn c_write(fd: i32, data: &[u8]) -> Result<usize, i32> {
    let ret = unsafe { write(fd, data.as_ptr(), data.len()) };
    if ret < 0 { Err(-ret as i32) } else { Ok(ret as usize) }
}

fn fail(msg: &str) -> ! {
    println!("UNIX_NAMED: FAIL - {}", msg);
    std::process::exit(1);
}

/// Phase 1: Basic server-client communication
fn test_basic_server_client() {
    println!("Phase 1: Basic server-client...");

    let server_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|e| {
        println!("  socket() failed: {}", e);
        fail("socket() for server failed");
    });
    println!("  Server socket created: fd={}", server_fd);

    let addr = SockAddrUn::abstract_socket(b"test_server_1");

    c_bind(server_fd, &addr).unwrap_or_else(|e| {
        println!("  bind() failed: {}", e);
        c_close(server_fd);
        fail("bind() failed");
    });
    println!("  Server bound to abstract path");

    c_listen(server_fd, 5).unwrap_or_else(|e| {
        println!("  listen() failed: {}", e);
        c_close(server_fd);
        fail("listen() failed");
    });
    println!("  Server listening");

    let client_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|e| {
        println!("  client socket() failed: {}", e);
        c_close(server_fd);
        fail("socket() for client failed");
    });
    println!("  Client socket created: fd={}", client_fd);

    c_connect(client_fd, &addr).unwrap_or_else(|e| {
        println!("  connect() failed: {}", e);
        c_close(client_fd);
        c_close(server_fd);
        fail("connect() failed");
    });
    println!("  Client connected to server");

    let accepted_fd = c_accept(server_fd).unwrap_or_else(|e| {
        println!("  accept() failed: {}", e);
        c_close(client_fd);
        c_close(server_fd);
        fail("accept() failed");
    });
    println!("  Server accepted connection: fd={}", accepted_fd);

    // Test bidirectional I/O
    let test_data = b"Hello from client!";
    c_write(client_fd, test_data).unwrap_or_else(|e| {
        println!("  client write() failed: {}", e);
        fail("client write failed");
    });
    println!("  Client wrote {} bytes", test_data.len());

    let mut buf = [0u8; 64];
    let n = c_read(accepted_fd, &mut buf).unwrap_or_else(|e| {
        println!("  server read() failed: {}", e);
        fail("server read failed");
    });
    println!("  Server received {} bytes", n);
    if &buf[..n] != test_data {
        fail("Data mismatch: client -> server");
    }

    let reply_data = b"Hello from server!";
    c_write(accepted_fd, reply_data).unwrap_or_else(|e| {
        println!("  server write() failed: {}", e);
        fail("server write failed");
    });
    println!("  Server wrote {} bytes", reply_data.len());

    let mut buf2 = [0u8; 64];
    let n2 = c_read(client_fd, &mut buf2).unwrap_or_else(|e| {
        println!("  client read() failed: {}", e);
        fail("client read failed");
    });
    println!("  Client received {} bytes", n2);
    if &buf2[..n2] != reply_data {
        fail("Data mismatch: server -> client");
    }

    println!("  Bidirectional I/O works!");

    c_close(accepted_fd);
    c_close(client_fd);
    c_close(server_fd);
    println!("  PASSED");
}

/// Phase 2: ECONNREFUSED on non-existent path
fn test_econnrefused() {
    println!("Phase 2: ECONNREFUSED on non-existent path...");

    let client_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"nonexistent_path_xyz");

    match c_connect(client_fd, &addr) {
        Ok(_) => {
            c_close(client_fd);
            fail("connect() should have failed with ECONNREFUSED");
        }
        Err(e) => {
            println!("  connect() returned: {}", e);
            if e != ECONNREFUSED {
                c_close(client_fd);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    c_close(client_fd);
    println!("  PASSED");
}

/// Phase 3: EADDRINUSE on duplicate bind
fn test_eaddrinuse() {
    println!("Phase 3: EADDRINUSE on duplicate bind...");

    let fd1 = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_addrinuse");

    c_bind(fd1, &addr).unwrap_or_else(|_| { c_close(fd1); fail("First bind() failed"); });
    c_listen(fd1, 5).unwrap_or_else(|_| { c_close(fd1); fail("listen() failed"); });

    let fd2 = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| { c_close(fd1); fail("socket() failed"); });

    match c_bind(fd2, &addr) {
        Ok(_) => {
            c_close(fd1);
            c_close(fd2);
            fail("Second bind() should have failed with EADDRINUSE");
        }
        Err(e) => {
            println!("  Second bind() returned: {}", e);
            if e != EADDRINUSE {
                c_close(fd1);
                c_close(fd2);
                fail("Expected EADDRINUSE");
            }
        }
    }

    c_close(fd1);
    c_close(fd2);
    println!("  PASSED");
}

/// Phase 4: Non-blocking accept returns EAGAIN
fn test_nonblock_accept() {
    println!("Phase 4: Non-blocking accept (EAGAIN)...");

    let server_fd = c_socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_nonblock");

    c_bind(server_fd, &addr).unwrap_or_else(|_| { c_close(server_fd); fail("bind() failed"); });
    c_listen(server_fd, 5).unwrap_or_else(|_| { c_close(server_fd); fail("listen() failed"); });

    match c_accept(server_fd) {
        Ok(_) => {
            c_close(server_fd);
            fail("accept() should have returned EAGAIN");
        }
        Err(e) => {
            println!("  accept() returned: {}", e);
            if e != EAGAIN {
                c_close(server_fd);
                fail("Expected EAGAIN");
            }
        }
    }

    c_close(server_fd);
    println!("  PASSED");
}

/// Phase 5: EINVAL on listen for unbound socket
fn test_listen_unbound() {
    println!("Phase 5: EINVAL on listen for unbound socket...");

    let fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));

    match c_listen(fd, 5) {
        Ok(_) => {
            c_close(fd);
            fail("listen() should have failed with EINVAL on unbound socket");
        }
        Err(e) => {
            println!("  listen() on unbound returned: {}", e);
            if e != EINVAL {
                c_close(fd);
                fail("Expected EINVAL");
            }
        }
    }

    c_close(fd);
    println!("  PASSED");
}

/// Phase 6: Backlog full returns ECONNREFUSED
fn test_backlog_full() {
    println!("Phase 6: Backlog full returns ECONNREFUSED...");

    let server_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_backlog_full");

    c_bind(server_fd, &addr).unwrap_or_else(|_| { c_close(server_fd); fail("bind() failed"); });
    c_listen(server_fd, 1).unwrap_or_else(|_| { c_close(server_fd); fail("listen() failed"); });

    // First client fills the backlog
    let client1_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| { c_close(server_fd); fail("socket() failed"); });
    c_connect(client1_fd, &addr).unwrap_or_else(|e| {
        println!("  First connect failed: {}", e);
        c_close(client1_fd);
        c_close(server_fd);
        fail("First connect should succeed");
    });
    println!("  First client connected (filled backlog)");

    // Second client should be refused
    let client2_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| {
        c_close(client1_fd);
        c_close(server_fd);
        fail("socket() failed");
    });

    match c_connect(client2_fd, &addr) {
        Ok(_) => {
            c_close(client2_fd);
            c_close(client1_fd);
            c_close(server_fd);
            fail("Second connect() should have failed with ECONNREFUSED (backlog full)");
        }
        Err(e) => {
            println!("  Second connect() returned: {}", e);
            if e != ECONNREFUSED {
                c_close(client2_fd);
                c_close(client1_fd);
                c_close(server_fd);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    c_close(client2_fd);
    c_close(client1_fd);
    c_close(server_fd);
    println!("  PASSED");
}

/// Phase 7: EISCONN on already-connected socket
fn test_eisconn() {
    println!("Phase 7: EISCONN on already-connected socket...");

    let server_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));
    let addr = SockAddrUn::abstract_socket(b"test_eisconn");

    c_bind(server_fd, &addr).unwrap_or_else(|_| { c_close(server_fd); fail("bind() failed"); });
    c_listen(server_fd, 5).unwrap_or_else(|_| { c_close(server_fd); fail("listen() failed"); });

    let client_fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| { c_close(server_fd); fail("socket() failed"); });

    c_connect(client_fd, &addr).unwrap_or_else(|e| {
        println!("  First connect failed: {}", e);
        c_close(client_fd);
        c_close(server_fd);
        fail("First connect should succeed");
    });
    println!("  Client connected to server");

    // Try to connect again - should fail with EISCONN
    match c_connect(client_fd, &addr) {
        Ok(_) => {
            c_close(client_fd);
            c_close(server_fd);
            fail("Second connect() should have failed with EISCONN");
        }
        Err(e) => {
            println!("  Second connect() returned: {}", e);
            if e != EISCONN {
                c_close(client_fd);
                c_close(server_fd);
                fail("Expected EISCONN");
            }
        }
    }

    c_close(client_fd);
    c_close(server_fd);
    println!("  PASSED");
}

/// Phase 8: Accept on non-listener socket
fn test_accept_non_listener() {
    println!("Phase 8: Accept on non-listener socket...");

    let fd = c_socket(AF_UNIX, SOCK_STREAM, 0).unwrap_or_else(|_| fail("socket() failed"));

    match c_accept(fd) {
        Ok(_) => {
            c_close(fd);
            fail("accept() on non-listener should have failed");
        }
        Err(e) => {
            println!("  accept() on non-listener returned: {}", e);
            if e != EOPNOTSUPP {
                c_close(fd);
                fail("Expected EOPNOTSUPP");
            }
        }
    }

    c_close(fd);
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
