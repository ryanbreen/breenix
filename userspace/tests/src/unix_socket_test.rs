//! Unix domain socket test program (std version)
//!
//! Tests socketpair(), named sockets (bind/listen/accept/connect),
//! EOF, EPIPE, EAGAIN, SOCK_NONBLOCK, SOCK_CLOEXEC, and error handling.

// Socket constants
const AF_UNIX: i32 = 1;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0x800;
const SOCK_CLOEXEC: i32 = 0x80000;

// Error codes
const EAGAIN: i32 = 11;
const EINVAL: i32 = 22;
const EPIPE: i32 = 32;
const EAFNOSUPPORT: i32 = 97;
const EADDRINUSE: i32 = 98;
const EISCONN: i32 = 106;
const ECONNREFUSED: i32 = 111;
const EOPNOTSUPP: i32 = 95;
const EFAULT: i32 = 14;

// fcntl constants
const F_GETFD: i32 = 1;
const FD_CLOEXEC: i32 = 1;

// Buffer size (must match kernel's UNIX_SOCKET_BUFFER_SIZE)
const UNIX_SOCKET_BUFFER_SIZE: usize = 65536;

// SockAddrUn structure matching the kernel layout
#[repr(C)]
struct SockAddrUn {
    family: u16,
    path: [u8; 108],
}

impl SockAddrUn {
    fn abstract_socket(name: &[u8]) -> Self {
        let mut addr = SockAddrUn {
            family: AF_UNIX as u16,
            path: [0u8; 108],
        };
        // Abstract socket: path[0] = 0, name follows
        let copy_len = name.len().min(107);
        addr.path[1..1 + copy_len].copy_from_slice(&name[..copy_len]);
        addr
    }

    fn len(&self) -> u32 {
        // family (2) + null byte (1) + name length
        let mut name_len = 0;
        for i in 1..108 {
            if self.path[i] == 0 { break; }
            name_len += 1;
        }
        (2 + 1 + name_len) as u32
    }
}

extern "C" {
    fn socket(domain: i32, sock_type: i32, protocol: i32) -> i32;
    fn socketpair(domain: i32, sock_type: i32, protocol: i32, sv: *mut i32) -> i32;
    fn bind(sockfd: i32, addr: *const u8, addrlen: u32) -> i32;
    fn listen(sockfd: i32, backlog: i32) -> i32;
    fn accept(sockfd: i32, addr: *mut u8, addrlen: *mut u32) -> i32;
    fn connect(sockfd: i32, addr: *const u8, addrlen: u32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn fcntl(fd: i32, cmd: i32, arg: u64) -> i32;
}

// Wrapper to get errno from libbreenix-libc
// The libc functions set a global ERRNO variable and return -1 on error.
// For raw fd operations we use extern "C" directly.
fn write_fd(fd: i32, data: &[u8]) -> Result<usize, i32> {
    let ret = unsafe { write(fd, data.as_ptr(), data.len()) };
    if ret < 0 {
        // The libbreenix-libc returns -1 and sets ERRNO, but we need the actual errno.
        // For now we'll use a raw syscall approach.
        Err(get_errno())
    } else {
        Ok(ret as usize)
    }
}

fn read_fd(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let ret = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(ret as usize)
    }
}

// Get errno from the libbreenix-libc global
fn get_errno() -> i32 {
    // The libbreenix-libc stores errno in a global. We access it via __errno_location
    // or directly. For simplicity, use a raw syscall approach instead.
    // Actually, let's use the libc's errno mechanism.
    extern "C" {
        static mut ERRNO: i32;
    }
    unsafe { ERRNO }
}

fn fail(msg: &str) -> ! {
    println!("UNIX_SOCKET: FAIL - {}", msg);
    std::process::exit(1);
}

fn do_socketpair(domain: i32, sock_type: i32, protocol: i32) -> Result<(i32, i32), i32> {
    let mut sv = [0i32; 2];
    let ret = unsafe { socketpair(domain, sock_type, protocol, sv.as_mut_ptr()) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok((sv[0], sv[1]))
    }
}

fn do_socketpair_raw(domain: i32, sock_type: i32, protocol: i32, sv_ptr: u64) -> Result<(), i32> {
    let ret = unsafe { socketpair(domain, sock_type, protocol, sv_ptr as *mut i32) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(())
    }
}

fn do_socket(domain: i32, sock_type: i32, protocol: i32) -> Result<i32, i32> {
    let ret = unsafe { socket(domain, sock_type, protocol) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(ret)
    }
}

fn do_bind(sockfd: i32, addr: &SockAddrUn) -> Result<(), i32> {
    let ret = unsafe { bind(sockfd, addr as *const SockAddrUn as *const u8, addr.len()) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(())
    }
}

fn do_listen(sockfd: i32, backlog: i32) -> Result<(), i32> {
    let ret = unsafe { listen(sockfd, backlog) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(())
    }
}

fn do_accept(sockfd: i32) -> Result<i32, i32> {
    let ret = unsafe { accept(sockfd, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(ret)
    }
}

fn do_connect(sockfd: i32, addr: &SockAddrUn) -> Result<(), i32> {
    let ret = unsafe { connect(sockfd, addr as *const SockAddrUn as *const u8, addr.len()) };
    if ret < 0 {
        Err(get_errno())
    } else {
        Ok(())
    }
}

fn do_close(fd: i32) {
    unsafe { close(fd); }
}

fn do_fcntl_getfd(fd: i32) -> i32 {
    unsafe { fcntl(fd, F_GETFD, 0) }
}

// Named socket test functions

fn test_named_basic_server_client() {
    let server_fd = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(e) => { println!("  socket() failed: {}", e); fail("socket() for server failed"); }
    };

    let addr = SockAddrUn::abstract_socket(b"combined_test_1_std");

    if let Err(e) = do_bind(server_fd, &addr) {
        println!("  bind() failed: {}", e);
        do_close(server_fd);
        fail("bind() failed");
    }

    if let Err(e) = do_listen(server_fd, 5) {
        println!("  listen() failed: {}", e);
        do_close(server_fd);
        fail("listen() failed");
    }

    let client_fd = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { do_close(server_fd); fail("socket() for client failed"); }
    };

    if let Err(e) = do_connect(client_fd, &addr) {
        println!("  connect() failed: {}", e);
        do_close(client_fd);
        do_close(server_fd);
        fail("connect() failed");
    }

    let accepted_fd = match do_accept(server_fd) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  accept() failed: {}", e);
            do_close(client_fd);
            do_close(server_fd);
            fail("accept() failed");
        }
    };

    // Test bidirectional I/O
    let test_data = b"Hello from client!";
    if let Err(e) = write_fd(client_fd, test_data) {
        println!("  client write() failed: {}", e);
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
            println!("  server read() failed: {}", e);
            fail("server read failed");
        }
    }

    let reply_data = b"Hello from server!";
    if let Err(e) = write_fd(accepted_fd, reply_data) {
        println!("  server write() failed: {}", e);
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
            println!("  client read() failed: {}", e);
            fail("client read failed");
        }
    }

    println!("  Bidirectional I/O works!");

    do_close(accepted_fd);
    do_close(client_fd);
    do_close(server_fd);
}

fn test_named_econnrefused() {
    let client_fd = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"nonexistent_combined_std");

    match do_connect(client_fd, &addr) {
        Ok(_) => {
            do_close(client_fd);
            fail("connect() should have failed with ECONNREFUSED");
        }
        Err(e) => {
            if e != ECONNREFUSED {
                println!("  Expected ECONNREFUSED, got: {}", e);
                do_close(client_fd);
                fail("Expected ECONNREFUSED");
            }
        }
    }

    do_close(client_fd);
}

fn test_named_eaddrinuse() {
    let fd1 = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"combined_addrinuse_std");

    if let Err(_) = do_bind(fd1, &addr) {
        do_close(fd1);
        fail("First bind() failed");
    }

    if let Err(_) = do_listen(fd1, 5) {
        do_close(fd1);
        fail("listen() failed");
    }

    let fd2 = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { do_close(fd1); fail("socket() failed"); }
    };

    match do_bind(fd2, &addr) {
        Ok(_) => {
            do_close(fd1);
            do_close(fd2);
            fail("Second bind() should have failed with EADDRINUSE");
        }
        Err(e) => {
            if e != EADDRINUSE {
                println!("  Expected EADDRINUSE, got: {}", e);
                do_close(fd1);
                do_close(fd2);
                fail("Expected EADDRINUSE");
            }
        }
    }

    do_close(fd1);
    do_close(fd2);
}

fn test_named_nonblock_accept() {
    let server_fd = match do_socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"combined_nonblock_std");

    if let Err(_) = do_bind(server_fd, &addr) {
        do_close(server_fd);
        fail("bind() failed");
    }

    if let Err(_) = do_listen(server_fd, 5) {
        do_close(server_fd);
        fail("listen() failed");
    }

    match do_accept(server_fd) {
        Ok(_) => {
            do_close(server_fd);
            fail("accept() should have returned EAGAIN");
        }
        Err(e) => {
            if e != EAGAIN {
                println!("  Expected EAGAIN, got: {}", e);
                do_close(server_fd);
                fail("Expected EAGAIN");
            }
        }
    }

    do_close(server_fd);
}

fn test_named_eisconn() {
    let server_fd = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() for server failed"),
    };

    let addr = SockAddrUn::abstract_socket(b"combined_eisconn_std");

    if let Err(_) = do_bind(server_fd, &addr) {
        do_close(server_fd);
        fail("bind() failed");
    }

    if let Err(_) = do_listen(server_fd, 5) {
        do_close(server_fd);
        fail("listen() failed");
    }

    let client_fd = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { do_close(server_fd); fail("socket() for client failed"); }
    };

    if let Err(e) = do_connect(client_fd, &addr) {
        println!("  First connect failed: {}", e);
        do_close(client_fd);
        do_close(server_fd);
        fail("First connect should succeed");
    }

    match do_connect(client_fd, &addr) {
        Ok(_) => {
            do_close(client_fd);
            do_close(server_fd);
            fail("Second connect() should have failed with EISCONN");
        }
        Err(e) => {
            if e != EISCONN {
                println!("  Expected EISCONN, got: {}", e);
                do_close(client_fd);
                do_close(server_fd);
                fail("Expected EISCONN");
            }
        }
    }

    do_close(client_fd);
    do_close(server_fd);
}

fn test_named_accept_non_listener() {
    let fd = match do_socket(AF_UNIX, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => fail("socket() failed"),
    };

    match do_accept(fd) {
        Ok(_) => {
            do_close(fd);
            fail("accept() on non-listener should have failed");
        }
        Err(e) => {
            if e != EOPNOTSUPP {
                println!("  Expected EOPNOTSUPP, got: {}", e);
                do_close(fd);
                fail("Expected EOPNOTSUPP");
            }
        }
    }

    do_close(fd);
}

fn main() {
    println!("=== Unix Socket Test ===");

    // Phase 1: Create socket pair
    println!("Phase 1: Creating socket pair with socketpair()...");
    let (sv0, sv1) = match do_socketpair(AF_UNIX, SOCK_STREAM, 0) {
        Ok(pair) => pair,
        Err(e) => {
            println!("  socketpair() returned error: {}", e);
            fail("socketpair() failed");
        }
    };

    println!("  Socket pair created successfully");
    println!("  sv[0] = {}", sv0);
    println!("  sv[1] = {}", sv1);

    if sv0 < 3 || sv1 < 3 {
        fail("Socket fds should be >= 3 (after stdin/stdout/stderr)");
    }
    if sv0 == sv1 {
        fail("Socket fds should be different");
    }
    println!("  FD numbers are valid");

    // Phase 2: Write from sv[0], read from sv[1]
    println!("Phase 2: Writing from sv[0], reading from sv[1]...");
    let test_data = b"Hello from sv[0]!";
    let write_ret = match write_fd(sv0, test_data) {
        Ok(n) => n,
        Err(e) => {
            println!("  write(sv[0]) returned error: {}", e);
            fail("write to sv[0] failed");
        }
    };

    println!("  Wrote {} bytes to sv[0]", write_ret);
    if write_ret != test_data.len() {
        fail("Did not write expected number of bytes");
    }

    let mut read_buf = [0u8; 32];
    let read_ret = match read_fd(sv1, &mut read_buf) {
        Ok(n) => n,
        Err(e) => {
            println!("  read(sv[1]) returned error: {}", e);
            fail("read from sv[1] failed");
        }
    };

    println!("  Read {} bytes from sv[1]", read_ret);
    if read_ret != test_data.len() {
        fail("Did not read expected number of bytes");
    }

    if &read_buf[..read_ret] != test_data {
        fail("Data verification failed (sv[0] -> sv[1])");
    }
    let data_str = std::str::from_utf8(&read_buf[..read_ret]).unwrap_or("<invalid utf8>");
    println!("  Data verified: '{}'", data_str);

    // Phase 3: Write from sv[1], read from sv[0] (reverse direction)
    println!("Phase 3: Writing from sv[1], reading from sv[0]...");
    let test_data2 = b"Reply from sv[1]!";
    let write_ret2 = match write_fd(sv1, test_data2) {
        Ok(n) => n,
        Err(e) => {
            println!("  write(sv[1]) returned error: {}", e);
            fail("write to sv[1] failed");
        }
    };

    println!("  Wrote {} bytes to sv[1]", write_ret2);

    let mut read_buf2 = [0u8; 32];
    let read_ret2 = match read_fd(sv0, &mut read_buf2) {
        Ok(n) => n,
        Err(e) => {
            println!("  read(sv[0]) returned error: {}", e);
            fail("read from sv[0] failed");
        }
    };

    println!("  Read {} bytes from sv[0]", read_ret2);
    if &read_buf2[..read_ret2] != test_data2 {
        fail("Data verification failed (sv[1] -> sv[0])");
    }
    println!("  Bidirectional communication works!");

    // Phase 4: Close sv[0], verify sv[1] sees EOF
    println!("Phase 4: Testing EOF on peer close...");
    do_close(sv0);
    println!("  Closed sv[0]");

    let mut eof_buf = [0u8; 8];
    let eof_ret = match read_fd(sv1, &mut eof_buf) {
        Ok(n) => n as i64,
        Err(e) => -(e as i64),
    };

    println!("  Read from sv[1] returned: {}", eof_ret);
    if eof_ret != 0 {
        fail("Expected EOF (0) after peer close");
    }
    println!("  EOF on peer close works!");

    // Phase 5: Close sv[1]
    println!("Phase 5: Closing sv[1]...");
    do_close(sv1);
    println!("  Closed sv[1]");

    // Phase 6: Test SOCK_NONBLOCK
    println!("Phase 6: Testing SOCK_NONBLOCK (EAGAIN on empty read)...");
    let (sv_nb0, sv_nb1) = match do_socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(pair) => pair,
        Err(e) => {
            println!("  socketpair(SOCK_NONBLOCK) returned error: {}", e);
            fail("socketpair(SOCK_NONBLOCK) failed");
        }
    };
    println!("  Created non-blocking socket pair");
    println!("  sv_nb[0] = {}, sv_nb[1] = {}", sv_nb0, sv_nb1);

    let mut nb_buf = [0u8; 8];
    match read_fd(sv_nb1, &mut nb_buf) {
        Ok(n) => {
            println!("  Read returned {} instead of EAGAIN", n);
            fail("Non-blocking read should return EAGAIN when no data available");
        }
        Err(e) => {
            println!("  Read from empty non-blocking socket returned: -{}", e);
            if e != EAGAIN {
                println!("  Expected EAGAIN, got different error");
                fail("Non-blocking read should return EAGAIN when no data available");
            }
        }
    }
    println!("  SOCK_NONBLOCK works correctly!");

    do_close(sv_nb0);
    do_close(sv_nb1);

    // Phase 7: Test EPIPE
    println!("Phase 7: Testing EPIPE (write to closed peer)...");
    let (sv_pipe0, sv_pipe1) = match do_socketpair(AF_UNIX, SOCK_STREAM, 0) {
        Ok(pair) => pair,
        Err(_) => fail("socketpair() for EPIPE test failed"),
    };
    println!("  Created socket pair for EPIPE test");

    do_close(sv_pipe1);
    println!("  Closed sv_pipe[1] (reader)");

    let pipe_data = b"This should fail";
    match write_fd(sv_pipe0, pipe_data) {
        Ok(n) => {
            println!("  Write returned {} instead of EPIPE", n);
            fail("Write to closed peer should return EPIPE");
        }
        Err(e) => {
            println!("  Write to socket with closed peer returned: -{}", e);
            if e != EPIPE {
                println!("  Expected EPIPE, got different error");
                fail("Write to closed peer should return EPIPE");
            }
        }
    }
    println!("  EPIPE works correctly!");

    do_close(sv_pipe0);

    // Phase 8: Test error handling
    println!("Phase 8: Testing error handling (invalid domain/type)...");

    match do_socketpair(AF_INET, SOCK_STREAM, 0) {
        Ok(_) => fail("socketpair(AF_INET) should fail"),
        Err(e) => {
            println!("  socketpair(AF_INET) returned: -{}", e);
            if e != EAFNOSUPPORT {
                println!("  Expected EAFNOSUPPORT (97)");
                fail("socketpair(AF_INET) should return EAFNOSUPPORT");
            }
        }
    }
    println!("  AF_INET correctly rejected with EAFNOSUPPORT");

    match do_socketpair(AF_UNIX, SOCK_DGRAM, 0) {
        Ok(_) => fail("socketpair(SOCK_DGRAM) should fail"),
        Err(e) => {
            println!("  socketpair(SOCK_DGRAM) returned: -{}", e);
            if e != EINVAL {
                println!("  Expected EINVAL (22)");
                fail("socketpair(SOCK_DGRAM) should return EINVAL");
            }
        }
    }
    println!("  SOCK_DGRAM correctly rejected with EINVAL");

    println!("  Error handling works correctly!");

    // Phase 9: Test buffer-full scenario
    println!("Phase 9: Testing buffer-full (EAGAIN on non-blocking write)...");
    let (sv_buf0, sv_buf1) = match do_socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0) {
        Ok(pair) => pair,
        Err(_) => fail("socketpair() for buffer-full test failed"),
    };
    println!("  Created non-blocking socket pair for buffer test");

    let chunk = [0x42u8; 4096];
    let mut total_written: usize = 0;
    let mut eagain_received = false;

    while total_written < UNIX_SOCKET_BUFFER_SIZE + 4096 {
        match write_fd(sv_buf0, &chunk) {
            Ok(n) => {
                total_written += n;
            }
            Err(e) => {
                if e == EAGAIN {
                    eagain_received = true;
                    println!("  Got EAGAIN after writing {} bytes", total_written);
                    break;
                } else {
                    println!("  Unexpected error during buffer fill: -{}", e);
                    fail("Unexpected error while filling buffer");
                }
            }
        }
    }

    if !eagain_received {
        println!("  Wrote {} bytes without EAGAIN", total_written);
        fail("Expected EAGAIN when buffer is full");
    }

    if total_written < UNIX_SOCKET_BUFFER_SIZE {
        println!("  Only wrote {} bytes, expected at least {}", total_written, UNIX_SOCKET_BUFFER_SIZE);
        fail("Buffer should hold at least UNIX_SOCKET_BUFFER_SIZE bytes");
    }
    println!("  Buffer-full test passed!");

    do_close(sv_buf0);
    do_close(sv_buf1);

    // Phase 10: Test NULL sv_ptr (EFAULT)
    println!("Phase 10: Testing NULL sv_ptr (EFAULT)...");
    match do_socketpair_raw(AF_UNIX, SOCK_STREAM, 0, 0) {
        Ok(_) => fail("socketpair(NULL) should fail"),
        Err(e) => {
            println!("  socketpair(NULL) returned: -{}", e);
            if e != EFAULT {
                println!("  Expected EFAULT");
                fail("socketpair(NULL) should return EFAULT");
            }
        }
    }
    println!("  NULL sv_ptr correctly rejected with EFAULT");

    // Phase 11: Test non-zero protocol (EINVAL)
    println!("Phase 11: Testing non-zero protocol (EINVAL)...");
    let mut sv_proto = [0i32; 2];
    match do_socketpair_raw(AF_UNIX, SOCK_STREAM, 1, sv_proto.as_mut_ptr() as u64) {
        Ok(_) => fail("socketpair(protocol=1) should fail"),
        Err(e) => {
            println!("  socketpair(protocol=1) returned: -{}", e);
            if e != EINVAL {
                println!("  Expected EINVAL");
                fail("socketpair(protocol!=0) should return EINVAL");
            }
        }
    }
    println!("  Non-zero protocol correctly rejected with EINVAL");

    // Phase 12: Test SOCK_CLOEXEC flag
    println!("Phase 12: Testing SOCK_CLOEXEC flag...");
    let (sv_cloexec0, sv_cloexec1) = match do_socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0) {
        Ok(pair) => pair,
        Err(e) => {
            println!("  socketpair(SOCK_CLOEXEC) returned error: {}", e);
            fail("socketpair(SOCK_CLOEXEC) failed");
        }
    };
    println!("  Created socket pair with SOCK_CLOEXEC");

    let flags0 = do_fcntl_getfd(sv_cloexec0);
    let flags1 = do_fcntl_getfd(sv_cloexec1);

    println!("  sv_cloexec[0] flags: {}, sv_cloexec[1] flags: {}", flags0, flags1);

    if flags0 < 0 || flags1 < 0 {
        println!("  fcntl(F_GETFD) failed");
        fail("fcntl(F_GETFD) failed on SOCK_CLOEXEC socket");
    }

    if (flags0 & FD_CLOEXEC) == 0 {
        fail("sv_cloexec[0] should have FD_CLOEXEC set");
    }
    if (flags1 & FD_CLOEXEC) == 0 {
        fail("sv_cloexec[1] should have FD_CLOEXEC set");
    }
    println!("  FD_CLOEXEC correctly set on both sockets");

    do_close(sv_cloexec0);
    do_close(sv_cloexec1);

    // Named Unix Socket Tests
    println!();
    println!("=== Named Unix Socket Tests ===");

    println!("Phase 13: Basic server-client (bind/listen/accept/connect)...");
    test_named_basic_server_client();
    println!("  Phase 13 PASSED");

    println!("Phase 14: ECONNREFUSED on non-existent path...");
    test_named_econnrefused();
    println!("  Phase 14 PASSED");

    println!("Phase 15: EADDRINUSE on duplicate bind...");
    test_named_eaddrinuse();
    println!("  Phase 15 PASSED");

    println!("Phase 16: Non-blocking accept (EAGAIN)...");
    test_named_nonblock_accept();
    println!("  Phase 16 PASSED");

    println!("Phase 17: EISCONN on already-connected socket...");
    test_named_eisconn();
    println!("  Phase 17 PASSED");

    println!("Phase 18: Accept on non-listener socket...");
    test_named_accept_non_listener();
    println!("  Phase 18 PASSED");

    // All tests passed
    println!("=== Unix Socket Test PASSED ===");
    println!("UNIX_SOCKET_TEST_PASSED");
    std::process::exit(0);
}
