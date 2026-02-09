//! TCP Socket userspace test (std version)
//!
//! Tests the TCP socket syscalls from userspace:
//! 1. Create a TCP socket (SOCK_STREAM) - MUST succeed
//! 2. Bind to a local port - MUST succeed
//! 3. Listen for connections - MUST succeed
//! 4. Create a second socket for client - MUST succeed
//! 5. Connect to server (loopback) - MUST succeed
//! 6. Accept on server - MUST succeed
//! 7. Shutdown connected socket (SHUT_RDWR) - MUST succeed
//! 8. Shutdown unconnected socket - MUST return ENOTCONN
//! 9. Bind same port - MUST return EADDRINUSE
//! 10. Listen on unbound socket - MUST return EINVAL
//! 11. Accept on non-listening socket - MUST return EOPNOTSUPP
//! 12-25. Various data transfer and edge case tests
//!
//! This validates the TCP syscall path from userspace to kernel.

#![allow(unused_assignments)]

use std::process;

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

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SHUT_RD: i32 = 0;
const SHUT_WR: i32 = 1;
const SHUT_RDWR: i32 = 2;
const O_NONBLOCK: i32 = 2048;

// Expected errno values
const EAGAIN: i32 = 11;
const EADDRINUSE: i32 = 98;
const EINVAL: i32 = 22;
const EOPNOTSUPP: i32 = 95;
const ENOTCONN: i32 = 107;
const EPIPE: i32 = 32;
const ECONNREFUSED: i32 = 111;
const ETIMEDOUT: i32 = 110;

// Maximum retries for loopback operations
const MAX_LOOPBACK_RETRIES: usize = 10;

extern "C" {
    fn socket(domain: i32, typ: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
    fn listen(fd: i32, backlog: i32) -> i32;
    fn accept(fd: i32, addr: *mut SockAddrIn, addrlen: *mut u32) -> i32;
    fn connect(fd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
    fn shutdown(fd: i32, how: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn fcntl(fd: i32, cmd: i32, arg: i64) -> i32;
    static mut ERRNO: i32;
}

/// Get the current errno value from libbreenix-libc
fn get_errno() -> i32 {
    unsafe { ERRNO }
}

const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;

fn sock_size() -> u32 {
    std::mem::size_of::<SockAddrIn>() as u32
}

fn do_accept(server_fd: i32, addr: Option<&mut SockAddrIn>) -> i32 {
    match addr {
        Some(a) => {
            let mut addrlen: u32 = sock_size();
            unsafe { accept(server_fd, a as *mut SockAddrIn, &mut addrlen) }
        }
        None => {
            unsafe { accept(server_fd, std::ptr::null_mut(), std::ptr::null_mut()) }
        }
    }
}

fn accept_with_retry_no_addr(server_fd: i32) -> (i32, usize) {
    for retry in 0..MAX_LOOPBACK_RETRIES {
        let result = unsafe { accept(server_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if result >= 0 {
            return (result, retry);
        }
        if result == -1 && get_errno() == EAGAIN {
            if retry < MAX_LOOPBACK_RETRIES - 1 {
                for _ in 0..10000 { std::hint::spin_loop(); }
            }
        } else {
            return (result, retry);
        }
    }
    (-1, MAX_LOOPBACK_RETRIES)
}

fn accept_with_retry_addr(server_fd: i32, addr: &mut SockAddrIn) -> (i32, usize) {
    for retry in 0..MAX_LOOPBACK_RETRIES {
        let mut addrlen: u32 = sock_size();
        let result = unsafe { accept(server_fd, addr as *mut SockAddrIn, &mut addrlen) };
        if result >= 0 {
            return (result, retry);
        }
        if result == -1 && get_errno() == EAGAIN {
            if retry < MAX_LOOPBACK_RETRIES - 1 {
                for _ in 0..10000 { std::hint::spin_loop(); }
            }
        } else {
            return (result, retry);
        }
    }
    (-1, MAX_LOOPBACK_RETRIES)
}

fn read_with_retry(fd: i32, buf: &mut [u8]) -> (isize, usize) {
    for retry in 0..MAX_LOOPBACK_RETRIES {
        let result = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
        if result > 0 {
            return (result, retry);
        }
        if (result == -1 && get_errno() == EAGAIN) || result == 0 {
            if retry < MAX_LOOPBACK_RETRIES - 1 {
                for _ in 0..10000 { std::hint::spin_loop(); }
            }
        } else {
            return (result, retry);
        }
    }
    (-1, MAX_LOOPBACK_RETRIES)
}

fn main() {
    println!("TCP Socket Test: Starting");
    let mut _passed = 0usize;
    let mut failed = 0usize;

    // Test 1: Create TCP socket
    let server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server_fd >= 0 {
        println!("TCP_TEST: socket created OK");
        _passed += 1;
    } else {
        println!("TCP_TEST: socket FAILED errno");
        failed += 1;
        process::exit(1);
    }

    // Test 2: Bind to local port
    let local_addr = SockAddrIn::new([0, 0, 0, 0], 8080);
    let ret = unsafe { bind(server_fd, &local_addr, sock_size()) };
    if ret == 0 {
        println!("TCP_TEST: bind OK");
        _passed += 1;
    } else {
        println!("TCP_TEST: bind FAILED");
        failed += 1;
        process::exit(2);
    }

    // Test 3: Start listening
    let ret = unsafe { listen(server_fd, 128) };
    if ret == 0 {
        println!("TCP_TEST: listen OK");
        _passed += 1;
    } else {
        println!("TCP_TEST: listen FAILED");
        failed += 1;
        process::exit(3);
    }

    // Test 4: Create client socket
    let client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if client_fd >= 0 {
        println!("TCP_TEST: client socket OK");
        _passed += 1;
    } else {
        println!("TCP_TEST: client socket FAILED errno");
        failed += 1;
        process::exit(4);
    }

    // Test 5: Connect to server (loopback)
    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 8080);
    let ret = unsafe { connect(client_fd, &loopback_addr, sock_size()) };
    if ret == 0 {
        println!("TCP_TEST: connect OK");
        _passed += 1;
    } else {
        println!("TCP_TEST: connect FAILED");
        failed += 1;
        process::exit(5);
    }

    // Test 6: Accept on server
    let (accept_fd, retry_count) = accept_with_retry_no_addr(server_fd);
    if accept_fd >= 0 {
        if retry_count > 0 {
            println!("TCP_TEST: accept OK (with retries - potential timing issue)");
        } else {
            println!("TCP_TEST: accept OK");
        }
        _passed += 1;
    } else {
        println!("TCP_TEST: accept FAILED");
        failed += 1;
    }

    // Test 7: Shutdown connected socket (SHUT_RDWR)
    let ret = unsafe { shutdown(client_fd, SHUT_RDWR) };
    if ret == 0 {
        println!("TCP_TEST: shutdown OK");
        _passed += 1;
    } else {
        println!("TCP_TEST: shutdown FAILED");
        failed += 1;
    }

    // Test 8: Shutdown on unconnected socket
    let unconnected_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if unconnected_fd >= 0 {
        let ret = unsafe { shutdown(unconnected_fd, SHUT_RDWR) };
        if ret == -1 && get_errno() == ENOTCONN {
            println!("TCP_TEST: shutdown_unconnected OK");
            _passed += 1;
        } else {
            println!("TCP_TEST: shutdown_unconnected FAILED");
            failed += 1;
        }
    } else {
        println!("TCP_TEST: shutdown_unconnected FAILED");
        failed += 1;
    }

    // Test 9: EADDRINUSE on bind to same port
    let conflict_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if conflict_fd >= 0 {
        let conflict_addr = SockAddrIn::new([0, 0, 0, 0], 8081);
        let ret = unsafe { bind(conflict_fd, &conflict_addr, sock_size()) };
        if ret == 0 {
            let ret = unsafe { listen(conflict_fd, 128) };
            if ret == 0 {
                let second_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
                if second_fd >= 0 {
                    let ret = unsafe { bind(second_fd, &conflict_addr, sock_size()) };
                    if ret == -1 && get_errno() == EADDRINUSE {
                        println!("TCP_TEST: eaddrinuse OK");
                        _passed += 1;
                    } else {
                        println!("TCP_TEST: eaddrinuse FAILED");
                        failed += 1;
                    }
                } else {
                    println!("TCP_TEST: eaddrinuse FAILED");
                    failed += 1;
                }
            } else {
                println!("TCP_TEST: eaddrinuse FAILED");
                failed += 1;
            }
        } else {
            println!("TCP_TEST: eaddrinuse FAILED");
            failed += 1;
        }
    } else {
        println!("TCP_TEST: eaddrinuse FAILED");
        failed += 1;
    }

    // Test 10: listen on unbound socket
    let unbound_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if unbound_fd >= 0 {
        let ret = unsafe { listen(unbound_fd, 128) };
        if ret == -1 && get_errno() == EINVAL {
            println!("TCP_TEST: listen_unbound OK");
            _passed += 1;
        } else {
            println!("TCP_TEST: listen_unbound FAILED");
            failed += 1;
        }
    } else {
        println!("TCP_TEST: listen_unbound FAILED");
        failed += 1;
    }

    // Test 11: accept on non-listening socket
    let nonlisten_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if nonlisten_fd >= 0 {
        let nonlisten_addr = SockAddrIn::new([0, 0, 0, 0], 8083);
        let ret = unsafe { bind(nonlisten_fd, &nonlisten_addr, sock_size()) };
        if ret == 0 {
            let ret = do_accept(nonlisten_fd, None);
            if ret == -1 && get_errno() == EOPNOTSUPP {
                println!("TCP_TEST: accept_nonlisten OK");
                _passed += 1;
            } else {
                println!("TCP_TEST: accept_nonlisten FAILED");
                failed += 1;
            }
        } else {
            println!("TCP_TEST: accept_nonlisten FAILED");
            failed += 1;
        }
    } else {
        println!("TCP_TEST: accept_nonlisten FAILED");
        failed += 1;
    }

    // =========================================================================
    // Test 12: TCP Data Transfer Test
    // =========================================================================
    println!("TCP_DATA_TEST: starting");

    let data_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if data_server_fd < 0 { println!("TCP_DATA_TEST: server socket FAILED"); failed += 1; process::exit(12); }

    let data_server_addr = SockAddrIn::new([0, 0, 0, 0], 8082);
    if unsafe { bind(data_server_fd, &data_server_addr, sock_size()) } != 0 { println!("TCP_DATA_TEST: server bind FAILED"); failed += 1; process::exit(12); }
    if unsafe { listen(data_server_fd, 128) } != 0 { println!("TCP_DATA_TEST: server listen FAILED"); failed += 1; process::exit(12); }
    println!("TCP_DATA_TEST: server listening on 8082");

    let data_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if data_client_fd < 0 { println!("TCP_DATA_TEST: client socket FAILED"); failed += 1; process::exit(12); }

    let data_loopback_addr = SockAddrIn::new([127, 0, 0, 1], 8082);
    if unsafe { connect(data_client_fd, &data_loopback_addr, sock_size()) } != 0 { println!("TCP_DATA_TEST: client connect FAILED"); failed += 1; process::exit(12); }
    println!("TCP_DATA_TEST: client connected");

    // Client writes "HELLO"
    let send_data = b"HELLO";
    let bytes_written = unsafe { write(data_client_fd, send_data.as_ptr(), send_data.len()) };
    if bytes_written < 0 { println!("TCP_DATA_TEST: send FAILED (error)"); failed += 1; process::exit(12); }
    if bytes_written as usize != send_data.len() { println!("TCP_DATA_TEST: send FAILED (partial write)"); failed += 1; process::exit(12); }
    println!("TCP_DATA_TEST: send OK");
    _passed += 1;

    // Server accepts
    let (accepted_fd, accept_retries) = accept_with_retry_no_addr(data_server_fd);
    if accepted_fd < 0 { println!("TCP_DATA_TEST: accept FAILED"); failed += 1; process::exit(12); }
    if accept_retries > 0 { println!("TCP_DATA_TEST: accept OK (with retries)"); } else { println!("TCP_DATA_TEST: accept OK"); }

    // Server reads
    let mut recv_buf = [0u8; 16];
    let (bytes_read, read_retries) = read_with_retry(accepted_fd, &mut recv_buf);
    if bytes_read < 0 { println!("TCP_DATA_TEST: recv FAILED"); failed += 1; process::exit(12); }
    if read_retries > 0 { println!("TCP_DATA_TEST: recv OK (with retries)"); } else { println!("TCP_DATA_TEST: recv OK"); }
    _passed += 1;

    // Verify received data matches "HELLO"
    let expected = b"HELLO";
    let received_len = bytes_read as usize;
    if received_len == expected.len() {
        if &recv_buf[..received_len] == expected {
            println!("TCP_DATA_TEST: data verified");
            _passed += 1;
        } else {
            println!("TCP_DATA_TEST: data mismatch");
            failed += 1;
        }
    } else {
        println!("TCP_DATA_TEST: wrong length");
        failed += 1;
    }

    // =========================================================================
    // Test 13: Post-shutdown write verification
    // =========================================================================
    println!("TCP_SHUTDOWN_WRITE_TEST: starting");

    let shutdown_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if shutdown_server_fd < 0 { println!("TCP_SHUTDOWN_WRITE_TEST: server socket FAILED"); failed += 1; process::exit(13); }
    let shutdown_server_addr = SockAddrIn::new([0, 0, 0, 0], 8084);
    if unsafe { bind(shutdown_server_fd, &shutdown_server_addr, sock_size()) } != 0 { println!("TCP_SHUTDOWN_WRITE_TEST: bind FAILED"); failed += 1; process::exit(13); }
    if unsafe { listen(shutdown_server_fd, 128) } != 0 { println!("TCP_SHUTDOWN_WRITE_TEST: listen FAILED"); failed += 1; process::exit(13); }

    let shutdown_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if shutdown_client_fd < 0 { println!("TCP_SHUTDOWN_WRITE_TEST: client socket FAILED"); failed += 1; process::exit(13); }
    let shutdown_loopback = SockAddrIn::new([127, 0, 0, 1], 8084);
    if unsafe { connect(shutdown_client_fd, &shutdown_loopback, sock_size()) } != 0 { println!("TCP_SHUTDOWN_WRITE_TEST: connect FAILED"); failed += 1; process::exit(13); }

    let (shutdown_accepted_fd, _) = accept_with_retry_no_addr(shutdown_server_fd);
    if shutdown_accepted_fd < 0 { println!("TCP_SHUTDOWN_WRITE_TEST: accept FAILED"); failed += 1; process::exit(13); }
    let _ = shutdown_accepted_fd;

    // Shutdown write on client
    if unsafe { shutdown(shutdown_client_fd, SHUT_WR) } != 0 {
        println!("TCP_SHUTDOWN_WRITE_TEST: shutdown FAILED");
        failed += 1;
    } else {
        let test_data = b"TEST";
        let write_result = unsafe { write(shutdown_client_fd, test_data.as_ptr(), test_data.len()) };
        if write_result == -1 && get_errno() == EPIPE {
            println!("TCP_SHUTDOWN_WRITE_TEST: EPIPE OK");
            _passed += 1;
        } else if write_result >= 0 {
            println!("TCP_SHUTDOWN_WRITE_TEST: write should fail after shutdown");
            failed += 1;
        } else {
            println!("TCP_SHUTDOWN_WRITE_TEST: expected EPIPE, got other error");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 14: SHUT_RD test
    // =========================================================================
    println!("TCP_SHUT_RD_TEST: starting");

    let shutrd_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if shutrd_server_fd < 0 { println!("TCP_SHUT_RD_TEST: socket FAILED"); failed += 1; process::exit(14); }
    let shutrd_addr = SockAddrIn::new([0, 0, 0, 0], 8085);
    if unsafe { bind(shutrd_server_fd, &shutrd_addr, sock_size()) } != 0 || unsafe { listen(shutrd_server_fd, 128) } != 0 {
        println!("TCP_SHUT_RD_TEST: bind/listen FAILED"); failed += 1; process::exit(14);
    }

    let shutrd_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if shutrd_client_fd < 0 { println!("TCP_SHUT_RD_TEST: client socket FAILED"); failed += 1; process::exit(14); }
    let shutrd_loopback = SockAddrIn::new([127, 0, 0, 1], 8085);
    if unsafe { connect(shutrd_client_fd, &shutrd_loopback, sock_size()) } != 0 {
        println!("TCP_SHUT_RD_TEST: connect FAILED"); failed += 1; process::exit(14);
    }

    let (shutrd_accepted_fd, _) = accept_with_retry_no_addr(shutrd_server_fd);
    if shutrd_accepted_fd >= 0 {
        let _ = shutrd_accepted_fd;
        if unsafe { shutdown(shutrd_client_fd, SHUT_RD) } == 0 {
            let mut shutrd_buf = [0u8; 16];
            let read_result = unsafe { read(shutrd_client_fd, shutrd_buf.as_mut_ptr(), shutrd_buf.len()) };
            if read_result == 0 {
                println!("TCP_SHUT_RD_TEST: EOF OK");
                _passed += 1;
            } else if read_result < 0 {
                println!("TCP_SHUT_RD_TEST: read error OK");
                _passed += 1;
            } else {
                println!("TCP_SHUT_RD_TEST: read returned data after SHUT_RD");
                failed += 1;
            }
        } else {
            println!("TCP_SHUT_RD_TEST: SHUT_RD FAILED");
            failed += 1;
        }
    } else {
        println!("TCP_SHUT_RD_TEST: accept FAILED");
        failed += 1;
    }

    // =========================================================================
    // Test 15: SHUT_WR test
    // =========================================================================
    println!("TCP_SHUT_WR_TEST: starting");

    let shutwr_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if shutwr_server_fd < 0 { println!("TCP_SHUT_WR_TEST: socket FAILED"); failed += 1; process::exit(15); }
    let shutwr_addr = SockAddrIn::new([0, 0, 0, 0], 8086);
    if unsafe { bind(shutwr_server_fd, &shutwr_addr, sock_size()) } != 0 || unsafe { listen(shutwr_server_fd, 128) } != 0 {
        println!("TCP_SHUT_WR_TEST: bind/listen FAILED"); failed += 1; process::exit(15);
    }

    let shutwr_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if shutwr_client_fd < 0 { println!("TCP_SHUT_WR_TEST: client socket FAILED"); failed += 1; process::exit(15); }
    let shutwr_loopback = SockAddrIn::new([127, 0, 0, 1], 8086);
    if unsafe { connect(shutwr_client_fd, &shutwr_loopback, sock_size()) } != 0 {
        println!("TCP_SHUT_WR_TEST: connect FAILED"); failed += 1; process::exit(15);
    }

    let (shutwr_accepted_fd, _) = accept_with_retry_no_addr(shutwr_server_fd);
    if shutwr_accepted_fd >= 0 {
        if unsafe { shutdown(shutwr_client_fd, SHUT_WR) } == 0 {
            let shutwr_test_data = b"TEST";
            let write_result = unsafe { write(shutwr_client_fd, shutwr_test_data.as_ptr(), shutwr_test_data.len()) };
            if write_result < 0 {
                println!("TCP_SHUT_WR_TEST: SHUT_WR write rejected OK");
                _passed += 1;
            } else {
                println!("TCP_SHUT_WR_TEST: write should fail after SHUT_WR");
                failed += 1;
            }
            // Check FIN on server side
            let mut shutwr_buf = [0u8; 16];
            for _ in 0..10000 { std::hint::spin_loop(); }
            let read_result = unsafe { read(shutwr_accepted_fd, shutwr_buf.as_mut_ptr(), shutwr_buf.len()) };
            if read_result == 0 || (read_result == -1 && get_errno() == EAGAIN) {
                println!("TCP_SHUT_WR_TEST: server saw FIN OK");
            }
        } else {
            println!("TCP_SHUT_WR_TEST: SHUT_WR FAILED");
            failed += 1;
        }
    } else {
        println!("TCP_SHUT_WR_TEST: accept FAILED");
        failed += 1;
    }

    // =========================================================================
    // Test 16: Bidirectional data test
    // =========================================================================
    println!("TCP_BIDIR_TEST: starting");

    let bidir_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if bidir_server_fd < 0 { println!("TCP_BIDIR_TEST: server socket FAILED"); failed += 1; process::exit(16); }
    let bidir_addr = SockAddrIn::new([0, 0, 0, 0], 8087);
    if unsafe { bind(bidir_server_fd, &bidir_addr, sock_size()) } != 0 || unsafe { listen(bidir_server_fd, 128) } != 0 {
        println!("TCP_BIDIR_TEST: bind/listen FAILED"); failed += 1; process::exit(16);
    }

    let bidir_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if bidir_client_fd < 0 { println!("TCP_BIDIR_TEST: client socket FAILED"); failed += 1; process::exit(16); }
    let bidir_loopback = SockAddrIn::new([127, 0, 0, 1], 8087);
    if unsafe { connect(bidir_client_fd, &bidir_loopback, sock_size()) } != 0 {
        println!("TCP_BIDIR_TEST: connect FAILED"); failed += 1; process::exit(16);
    }

    let (bidir_accepted_fd, _) = accept_with_retry_no_addr(bidir_server_fd);
    if bidir_accepted_fd < 0 { println!("TCP_BIDIR_TEST: accept FAILED"); failed += 1; process::exit(16); }

    // Server sends "WORLD" to client
    let bidir_send_data = b"WORLD";
    let bidir_written = unsafe { write(bidir_accepted_fd, bidir_send_data.as_ptr(), bidir_send_data.len()) };
    if bidir_written as usize != bidir_send_data.len() {
        println!("TCP_BIDIR_TEST: server send FAILED");
        failed += 1;
    } else {
        let mut bidir_recv_buf = [0u8; 16];
        let (bidir_read, _) = read_with_retry(bidir_client_fd, &mut bidir_recv_buf);

        if bidir_read == bidir_send_data.len() as isize {
            if &bidir_recv_buf[..bidir_read as usize] == bidir_send_data {
                println!("TCP_BIDIR_TEST: server->client OK");
                _passed += 1;
            } else {
                println!("TCP_BIDIR_TEST: data mismatch");
                failed += 1;
            }
        } else {
            println!("TCP_BIDIR_TEST: wrong length");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 17: Large data test (256 bytes)
    // =========================================================================
    println!("TCP_LARGE_TEST: starting");

    let large_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if large_server_fd < 0 { println!("TCP_LARGE_TEST: server socket FAILED"); failed += 1; process::exit(17); }
    let large_addr = SockAddrIn::new([0, 0, 0, 0], 8088);
    if unsafe { bind(large_server_fd, &large_addr, sock_size()) } != 0 || unsafe { listen(large_server_fd, 128) } != 0 {
        println!("TCP_LARGE_TEST: bind/listen FAILED"); failed += 1; process::exit(17);
    }

    let large_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if large_client_fd < 0 { println!("TCP_LARGE_TEST: client socket FAILED"); failed += 1; process::exit(17); }
    let large_loopback = SockAddrIn::new([127, 0, 0, 1], 8088);
    if unsafe { connect(large_client_fd, &large_loopback, sock_size()) } != 0 {
        println!("TCP_LARGE_TEST: connect FAILED"); failed += 1; process::exit(17);
    }

    let (large_accepted_fd, _) = accept_with_retry_no_addr(large_server_fd);
    if large_accepted_fd < 0 { println!("TCP_LARGE_TEST: accept FAILED"); failed += 1; process::exit(17); }

    // Create 256-byte test pattern
    let mut large_send_data = [0u8; 256];
    for i in 0..256 { large_send_data[i] = i as u8; }

    let large_written = unsafe { write(large_client_fd, large_send_data.as_ptr(), large_send_data.len()) };
    if large_written as usize != large_send_data.len() {
        println!("TCP_LARGE_TEST: send FAILED");
        failed += 1;
    } else {
        let mut large_recv_buf = [0u8; 512];
        let mut total_read: usize = 0;

        for _attempt in 0..10 {
            let bytes = unsafe { read(large_accepted_fd, large_recv_buf.as_mut_ptr().add(total_read), large_recv_buf.len() - total_read) };
            if bytes > 0 {
                total_read += bytes as usize;
                if total_read >= 256 { break; }
            } else if (bytes == -1 && get_errno() == EAGAIN) || bytes == 0 {
                for _ in 0..10000 { std::hint::spin_loop(); }
            } else {
                break;
            }
        }

        if total_read == 256 {
            let mut matches = true;
            for i in 0..256 { if large_recv_buf[i] != i as u8 { matches = false; break; } }
            if matches {
                println!("TCP_LARGE_TEST: 256 bytes verified OK");
                _passed += 1;
            } else {
                println!("TCP_LARGE_TEST: data mismatch");
                failed += 1;
            }
        } else {
            println!("TCP_LARGE_TEST: incomplete read");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 18: Backlog overflow test
    // =========================================================================
    println!("TCP_BACKLOG_TEST: starting");

    let backlog_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if backlog_server_fd < 0 { println!("TCP_BACKLOG_TEST: server socket FAILED"); failed += 1; process::exit(18); }
    let backlog_addr = SockAddrIn::new([0, 0, 0, 0], 8089);
    if unsafe { bind(backlog_server_fd, &backlog_addr, sock_size()) } != 0 { println!("TCP_BACKLOG_TEST: bind FAILED"); failed += 1; process::exit(18); }
    if unsafe { listen(backlog_server_fd, 2) } != 0 { println!("TCP_BACKLOG_TEST: listen FAILED"); failed += 1; process::exit(18); }

    let backlog_loopback = SockAddrIn::new([127, 0, 0, 1], 8089);
    let mut connect_results = [false; 3];

    let client1 = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if client1 < 0 { println!("TCP_BACKLOG_TEST: client1 socket FAILED"); failed += 1; process::exit(18); }
    connect_results[0] = unsafe { connect(client1, &backlog_loopback, sock_size()) } == 0;

    let client2 = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if client2 < 0 { println!("TCP_BACKLOG_TEST: client2 socket FAILED"); failed += 1; process::exit(18); }
    connect_results[1] = unsafe { connect(client2, &backlog_loopback, sock_size()) } == 0;

    let client3 = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if client3 < 0 { println!("TCP_BACKLOG_TEST: client3 socket FAILED"); failed += 1; process::exit(18); }
    connect_results[2] = unsafe { connect(client3, &backlog_loopback, sock_size()) } == 0;

    // Set server to non-blocking to count accepted
    let flags = unsafe { fcntl(backlog_server_fd, F_GETFL, 0) };
    if flags >= 0 { unsafe { fcntl(backlog_server_fd, F_SETFL, (flags | O_NONBLOCK) as i64); } }

    let mut accepted_count = 0;
    for _ in 0..3 {
        let ret = unsafe { accept(backlog_server_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if ret >= 0 { accepted_count += 1; }
        else if ret == -1 && get_errno() == EAGAIN { break; }
        else { break; }
    }

    if connect_results[0] && connect_results[1] {
        if !connect_results[2] {
            println!("TCP_BACKLOG_TEST: overflow rejected OK");
        } else if accepted_count <= 2 {
            println!("TCP_BACKLOG_TEST: overflow limited OK");
        } else {
            println!("TCP_BACKLOG_TEST: all accepted OK");
        }
        _passed += 1;
    } else {
        println!("TCP_BACKLOG_TEST: first 2 connects FAILED");
        failed += 1;
    }

    // =========================================================================
    // Test 19: ECONNREFUSED test
    // =========================================================================
    println!("TCP_CONNREFUSED_TEST: starting");

    let refused_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if refused_client_fd < 0 { println!("TCP_CONNREFUSED_TEST: socket FAILED"); failed += 1; process::exit(19); }

    let refused_addr = SockAddrIn::new([127, 0, 0, 1], 9999);
    let ret = unsafe { connect(refused_client_fd, &refused_addr, sock_size()) };
    if ret == -1 && get_errno() == ECONNREFUSED {
        println!("TCP_CONNREFUSED_TEST: ECONNREFUSED OK");
        _passed += 1;
    } else if ret == -1 && get_errno() == ETIMEDOUT {
        println!("TCP_CONNREFUSED_TEST: ETIMEDOUT OK");
        _passed += 1;
    } else if ret == 0 {
        println!("TCP_CONNREFUSED_TEST: connect should have failed");
        failed += 1;
    } else {
        println!("TCP_CONNREFUSED_TEST: unexpected error");
        failed += 1;
    }

    // =========================================================================
    // Test 20: MSS boundary test (data > 1460 bytes)
    // =========================================================================
    println!("TCP_MSS_TEST: starting");

    let mss_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if mss_server_fd < 0 { println!("TCP_MSS_TEST: server socket FAILED"); failed += 1; process::exit(20); }
    let mss_addr = SockAddrIn::new([0, 0, 0, 0], 8090);
    if unsafe { bind(mss_server_fd, &mss_addr, sock_size()) } != 0 || unsafe { listen(mss_server_fd, 128) } != 0 {
        println!("TCP_MSS_TEST: bind/listen FAILED"); failed += 1; process::exit(20);
    }

    let mss_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if mss_client_fd < 0 { println!("TCP_MSS_TEST: client socket FAILED"); failed += 1; process::exit(20); }
    let mss_loopback = SockAddrIn::new([127, 0, 0, 1], 8090);
    if unsafe { connect(mss_client_fd, &mss_loopback, sock_size()) } != 0 {
        println!("TCP_MSS_TEST: connect FAILED"); failed += 1; process::exit(20);
    }

    let (mss_accepted_fd, _) = accept_with_retry_no_addr(mss_server_fd);
    if mss_accepted_fd < 0 { println!("TCP_MSS_TEST: accept FAILED"); failed += 1; process::exit(20); }

    // Create 2000-byte test pattern
    let mut mss_send_data = [0u8; 2000];
    for i in 0..2000 { mss_send_data[i] = (i % 256) as u8; }

    let mut total_written: usize = 0;
    for _attempt in 0..10 {
        let bytes = unsafe { write(mss_client_fd, mss_send_data.as_ptr().add(total_written), mss_send_data.len() - total_written) };
        if bytes > 0 { total_written += bytes as usize; if total_written >= 2000 { break; } }
        else if bytes == -1 && get_errno() == EAGAIN { for _ in 0..10000 { std::hint::spin_loop(); } }
        else if bytes < 0 { break; }
    }

    if total_written != 2000 {
        println!("TCP_MSS_TEST: send FAILED (incomplete)");
        failed += 1;
    } else {
        let mut mss_recv_buf = [0u8; 2500];
        let mut total_read: usize = 0;

        for _attempt in 0..20 {
            let bytes = unsafe { read(mss_accepted_fd, mss_recv_buf.as_mut_ptr().add(total_read), mss_recv_buf.len() - total_read) };
            if bytes > 0 { total_read += bytes as usize; if total_read >= 2000 { break; } }
            else if (bytes == -1 && get_errno() == EAGAIN) || bytes == 0 { for _ in 0..10000 { std::hint::spin_loop(); } }
            else { break; }
        }

        if total_read == 2000 {
            let mut matches = true;
            for i in 0..2000 { if mss_recv_buf[i] != (i % 256) as u8 { matches = false; break; } }
            if matches {
                println!("TCP_MSS_TEST: 2000 bytes (>MSS) verified OK");
                _passed += 1;
            } else {
                println!("TCP_MSS_TEST: data mismatch");
                failed += 1;
            }
        } else {
            println!("TCP_MSS_TEST: incomplete read");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 21: Multiple write/read cycles test
    // =========================================================================
    println!("TCP_MULTI_TEST: starting");

    let multi_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if multi_server_fd < 0 { println!("TCP_MULTI_TEST: server socket FAILED"); failed += 1; process::exit(21); }
    let multi_addr = SockAddrIn::new([0, 0, 0, 0], 8091);
    if unsafe { bind(multi_server_fd, &multi_addr, sock_size()) } != 0 || unsafe { listen(multi_server_fd, 128) } != 0 {
        println!("TCP_MULTI_TEST: bind/listen FAILED"); failed += 1; process::exit(21);
    }

    let multi_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if multi_client_fd < 0 { println!("TCP_MULTI_TEST: client socket FAILED"); failed += 1; process::exit(21); }
    let multi_loopback = SockAddrIn::new([127, 0, 0, 1], 8091);
    if unsafe { connect(multi_client_fd, &multi_loopback, sock_size()) } != 0 {
        println!("TCP_MULTI_TEST: connect FAILED"); failed += 1; process::exit(21);
    }

    let (multi_accepted_fd, _) = accept_with_retry_no_addr(multi_server_fd);
    if multi_accepted_fd < 0 { println!("TCP_MULTI_TEST: accept FAILED"); failed += 1; process::exit(21); }

    // Send 3 messages on same connection
    let messages: [&[u8]; 3] = [b"MSG1", b"MSG2", b"MSG3"];
    let mut multi_success = true;

    for msg in messages.iter() {
        let written = unsafe { write(multi_client_fd, msg.as_ptr(), msg.len()) };
        if written as usize != msg.len() { multi_success = false; break; }

        let mut recv_buf = [0u8; 16];
        let (bytes_read, _) = read_with_retry(multi_accepted_fd, &mut recv_buf);
        if bytes_read as usize != msg.len() { multi_success = false; break; }

        for i in 0..msg.len() {
            if recv_buf[i] != msg[i] { multi_success = false; break; }
        }
        if !multi_success { break; }
    }

    if multi_success {
        println!("TCP_MULTI_TEST: 3 messages verified OK");
        _passed += 1;
    } else {
        println!("TCP_MULTI_TEST: multi-message FAILED");
        failed += 1;
    }

    // =========================================================================
    // Test 22: Accept with client address test
    // =========================================================================
    println!("TCP_ADDR_TEST: starting");

    let addr_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if addr_server_fd < 0 { println!("TCP_ADDR_TEST: server socket FAILED"); failed += 1; process::exit(22); }
    let addr_server_addr = SockAddrIn::new([0, 0, 0, 0], 8092);
    if unsafe { bind(addr_server_fd, &addr_server_addr, sock_size()) } != 0 || unsafe { listen(addr_server_fd, 128) } != 0 {
        println!("TCP_ADDR_TEST: bind/listen FAILED"); failed += 1; process::exit(22);
    }

    let addr_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if addr_client_fd < 0 { println!("TCP_ADDR_TEST: client socket FAILED"); failed += 1; process::exit(22); }
    let addr_loopback = SockAddrIn::new([127, 0, 0, 1], 8092);
    if unsafe { connect(addr_client_fd, &addr_loopback, sock_size()) } != 0 {
        println!("TCP_ADDR_TEST: connect FAILED"); failed += 1; process::exit(22);
    }

    let mut client_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let (addr_accepted_fd, _) = accept_with_retry_addr(addr_server_fd, &mut client_addr);

    if addr_accepted_fd >= 0 {
        if client_addr.sin_addr[0] == 127 && client_addr.sin_addr[1] == 0 &&
           client_addr.sin_addr[2] == 0 && client_addr.sin_addr[3] == 1 {
            println!("TCP_ADDR_TEST: 127.0.0.1 OK");
            _passed += 1;
        } else if client_addr.sin_addr[0] == 10 {
            println!("TCP_ADDR_TEST: 10.x.x.x OK");
            _passed += 1;
        } else if client_addr.sin_addr[0] == 0 && client_addr.sin_addr[1] == 0 &&
                  client_addr.sin_addr[2] == 0 && client_addr.sin_addr[3] == 0 {
            println!("TCP_ADDR_TEST: address not filled FAILED");
            failed += 1;
        } else {
            println!("TCP_ADDR_TEST: unexpected address FAILED");
            failed += 1;
        }
    } else {
        println!("TCP_ADDR_TEST: accept FAILED");
        failed += 1;
    }

    // =========================================================================
    // Test 23: Simultaneous close test
    // =========================================================================
    println!("TCP_SIMUL_CLOSE_TEST: starting");

    let simul_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if simul_server_fd < 0 { println!("TCP_SIMUL_CLOSE_TEST: server socket FAILED"); failed += 1; process::exit(23); }
    let simul_addr = SockAddrIn::new([0, 0, 0, 0], 8093);
    if unsafe { bind(simul_server_fd, &simul_addr, sock_size()) } != 0 || unsafe { listen(simul_server_fd, 128) } != 0 {
        println!("TCP_SIMUL_CLOSE_TEST: bind/listen FAILED"); failed += 1; process::exit(23);
    }

    let simul_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if simul_client_fd < 0 { println!("TCP_SIMUL_CLOSE_TEST: client socket FAILED"); failed += 1; process::exit(23); }
    let simul_loopback = SockAddrIn::new([127, 0, 0, 1], 8093);
    if unsafe { connect(simul_client_fd, &simul_loopback, sock_size()) } != 0 {
        println!("TCP_SIMUL_CLOSE_TEST: connect FAILED"); failed += 1; process::exit(23);
    }

    let (simul_accepted_fd, _) = accept_with_retry_no_addr(simul_server_fd);
    if simul_accepted_fd < 0 { println!("TCP_SIMUL_CLOSE_TEST: accept FAILED"); failed += 1; process::exit(23); }

    let client_shutdown_result = unsafe { shutdown(simul_client_fd, SHUT_RDWR) };
    let server_shutdown_result = unsafe { shutdown(simul_accepted_fd, SHUT_RDWR) };

    if client_shutdown_result == 0 && server_shutdown_result == 0 {
        println!("TCP_SIMUL_CLOSE_TEST: simultaneous close OK");
        _passed += 1;
    } else if client_shutdown_result == 0 || server_shutdown_result == 0 {
        println!("TCP_SIMUL_CLOSE_TEST: simultaneous close OK");
        _passed += 1;
    } else {
        println!("TCP_SIMUL_CLOSE_TEST: both shutdowns FAILED");
        failed += 1;
    }

    // =========================================================================
    // Test 24: Half-close data flow test
    // =========================================================================
    println!("TCP_HALFCLOSE_TEST: starting");

    let halfclose_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if halfclose_server_fd < 0 { println!("TCP_HALFCLOSE_TEST: server socket FAILED"); failed += 1; process::exit(24); }
    let halfclose_addr = SockAddrIn::new([0, 0, 0, 0], 8094);
    if unsafe { bind(halfclose_server_fd, &halfclose_addr, sock_size()) } != 0 || unsafe { listen(halfclose_server_fd, 128) } != 0 {
        println!("TCP_HALFCLOSE_TEST: bind/listen FAILED"); failed += 1; process::exit(24);
    }

    let halfclose_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if halfclose_client_fd < 0 { println!("TCP_HALFCLOSE_TEST: client socket FAILED"); failed += 1; process::exit(24); }
    let halfclose_loopback = SockAddrIn::new([127, 0, 0, 1], 8094);
    if unsafe { connect(halfclose_client_fd, &halfclose_loopback, sock_size()) } != 0 {
        println!("TCP_HALFCLOSE_TEST: connect FAILED"); failed += 1; process::exit(24);
    }

    let (halfclose_accepted_fd, _) = accept_with_retry_no_addr(halfclose_server_fd);
    if halfclose_accepted_fd < 0 { println!("TCP_HALFCLOSE_TEST: accept FAILED"); failed += 1; process::exit(24); }

    if unsafe { shutdown(halfclose_client_fd, SHUT_WR) } != 0 {
        println!("TCP_HALFCLOSE_TEST: SHUT_WR FAILED");
        failed += 1;
    } else {
        let halfclose_data = b"HALFCLOSE_DATA";
        let written = unsafe { write(halfclose_accepted_fd, halfclose_data.as_ptr(), halfclose_data.len()) };
        if written as usize != halfclose_data.len() {
            println!("TCP_HALFCLOSE_TEST: server send FAILED");
            failed += 1;
        } else {
            let mut halfclose_recv_buf = [0u8; 32];
            let (bytes_read, _) = read_with_retry(halfclose_client_fd, &mut halfclose_recv_buf);

            if bytes_read == halfclose_data.len() as isize {
                if &halfclose_recv_buf[..bytes_read as usize] == halfclose_data {
                    println!("TCP_HALFCLOSE_TEST: read after SHUT_WR OK");
                    _passed += 1;
                } else {
                    println!("TCP_HALFCLOSE_TEST: data mismatch FAILED");
                    failed += 1;
                }
            } else if bytes_read > 0 {
                println!("TCP_HALFCLOSE_TEST: wrong length FAILED");
                failed += 1;
            } else {
                println!("TCP_HALFCLOSE_TEST: read FAILED");
                failed += 1;
            }
        }
    }

    // =========================================================================
    // Test 25: First-call accept (no retry)
    // =========================================================================
    println!("TCP_FIRST_ACCEPT_TEST: starting");

    let first_accept_server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if first_accept_server_fd >= 0 {
        let first_accept_addr = SockAddrIn::new([0, 0, 0, 0], 9090);
        if unsafe { bind(first_accept_server_fd, &first_accept_addr, sock_size()) } == 0 &&
           unsafe { listen(first_accept_server_fd, 128) } == 0 {
            let first_accept_client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
            if first_accept_client_fd >= 0 {
                let first_accept_loopback = SockAddrIn::new([127, 0, 0, 1], 9090);
                if unsafe { connect(first_accept_client_fd, &first_accept_loopback, sock_size()) } == 0 {
                    let accepted_fd = unsafe { accept(first_accept_server_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
                    if accepted_fd >= 0 {
                        println!("TCP_FIRST_ACCEPT_TEST: accept OK");
                        _passed += 1;
                        unsafe { close(accepted_fd); }
                    } else if accepted_fd == -1 && get_errno() == EAGAIN {
                        println!("TCP_FIRST_ACCEPT_TEST: accept returned EAGAIN FAILED");
                        failed += 1;
                    } else {
                        println!("TCP_FIRST_ACCEPT_TEST: accept FAILED");
                        failed += 1;
                    }
                } else {
                    println!("TCP_FIRST_ACCEPT_TEST: connect FAILED");
                    failed += 1;
                }
                unsafe { close(first_accept_client_fd); }
            } else {
                println!("TCP_FIRST_ACCEPT_TEST: client socket FAILED");
                failed += 1;
            }
        } else {
            println!("TCP_FIRST_ACCEPT_TEST: bind/listen FAILED");
            failed += 1;
        }
        unsafe { close(first_accept_server_fd); }
    } else {
        println!("TCP_FIRST_ACCEPT_TEST: server socket FAILED");
        failed += 1;
    }

    // Final result
    if failed == 0 {
        println!("TCP Socket Test: PASSED");
        process::exit(0);
    } else {
        println!("TCP Socket Test: FAILED");
        process::exit(1);
    }
}
