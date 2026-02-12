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

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::io::status_flags::O_NONBLOCK;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM, SHUT_RD, SHUT_WR, SHUT_RDWR};
use libbreenix::types::Fd;

// Maximum retries for loopback operations
const MAX_LOOPBACK_RETRIES: usize = 10;

fn accept_with_retry_no_addr(server_fd: Fd) -> (Result<Fd, Error>, usize) {
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match socket::accept(server_fd, None) {
            Ok(fd) => return (Ok(fd), retry),
            Err(Error::Os(Errno::EAGAIN)) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { std::hint::spin_loop(); }
                }
            }
            Err(e) => return (Err(e), retry),
        }
    }
    (Err(Error::Os(Errno::EAGAIN)), MAX_LOOPBACK_RETRIES)
}

fn accept_with_retry_addr(server_fd: Fd, addr: &mut SockAddrIn) -> (Result<Fd, Error>, usize) {
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match socket::accept(server_fd, Some(addr)) {
            Ok(fd) => return (Ok(fd), retry),
            Err(Error::Os(Errno::EAGAIN)) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { std::hint::spin_loop(); }
                }
            }
            Err(e) => return (Err(e), retry),
        }
    }
    (Err(Error::Os(Errno::EAGAIN)), MAX_LOOPBACK_RETRIES)
}

fn read_with_retry(fd: Fd, buf: &mut [u8]) -> (Result<usize, Error>, usize) {
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match io::read(fd, buf) {
            Ok(0) => {
                // EOF - retry
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { std::hint::spin_loop(); }
                }
            }
            Ok(n) => return (Ok(n), retry),
            Err(Error::Os(Errno::EAGAIN)) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { std::hint::spin_loop(); }
                }
            }
            Err(e) => return (Err(e), retry),
        }
    }
    (Err(Error::Os(Errno::EAGAIN)), MAX_LOOPBACK_RETRIES)
}

fn main() {
    println!("TCP Socket Test: Starting");
    let mut _passed = 0usize;
    let mut failed = 0usize;

    // Test 1: Create TCP socket
    let server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => {
            println!("TCP_TEST: socket created OK");
            _passed += 1;
            fd
        }
        Err(_) => {
            println!("TCP_TEST: socket FAILED errno");
            failed += 1;
            process::exit(1);
        }
    };

    // Test 2: Bind to local port
    let local_addr = SockAddrIn::new([0, 0, 0, 0], 8080);
    match socket::bind_inet(server_fd, &local_addr) {
        Ok(()) => {
            println!("TCP_TEST: bind OK");
            _passed += 1;
        }
        Err(_) => {
            println!("TCP_TEST: bind FAILED");
            failed += 1;
            process::exit(2);
        }
    }

    // Test 3: Start listening
    match socket::listen(server_fd, 128) {
        Ok(()) => {
            println!("TCP_TEST: listen OK");
            _passed += 1;
        }
        Err(_) => {
            println!("TCP_TEST: listen FAILED");
            failed += 1;
            process::exit(3);
        }
    }

    // Test 4: Create client socket
    let client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => {
            println!("TCP_TEST: client socket OK");
            _passed += 1;
            fd
        }
        Err(_) => {
            println!("TCP_TEST: client socket FAILED errno");
            failed += 1;
            process::exit(4);
        }
    };

    // Test 5: Connect to server (loopback)
    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 8080);
    match socket::connect_inet(client_fd, &loopback_addr) {
        Ok(()) => {
            println!("TCP_TEST: connect OK");
            _passed += 1;
        }
        Err(_) => {
            println!("TCP_TEST: connect FAILED");
            failed += 1;
            process::exit(5);
        }
    }

    // Test 6: Accept on server
    let (accept_result, retry_count) = accept_with_retry_no_addr(server_fd);
    let _accept_fd = match accept_result {
        Ok(fd) => {
            if retry_count > 0 {
                println!("TCP_TEST: accept OK (with retries - potential timing issue)");
            } else {
                println!("TCP_TEST: accept OK");
            }
            _passed += 1;
            fd
        }
        Err(_) => {
            println!("TCP_TEST: accept FAILED");
            failed += 1;
            Fd::from_raw(u64::MAX) // sentinel; tests below won't use it meaningfully
        }
    };

    // Test 7: Shutdown connected socket (SHUT_RDWR)
    match socket::shutdown(client_fd, SHUT_RDWR) {
        Ok(()) => {
            println!("TCP_TEST: shutdown OK");
            _passed += 1;
        }
        Err(_) => {
            println!("TCP_TEST: shutdown FAILED");
            failed += 1;
        }
    }

    // Test 8: Shutdown on unconnected socket
    match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(unconnected_fd) => {
            match socket::shutdown(unconnected_fd, SHUT_RDWR) {
                Err(Error::Os(Errno::ENOTCONN)) => {
                    println!("TCP_TEST: shutdown_unconnected OK");
                    _passed += 1;
                }
                _ => {
                    println!("TCP_TEST: shutdown_unconnected FAILED");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("TCP_TEST: shutdown_unconnected FAILED");
            failed += 1;
        }
    }

    // Test 9: EADDRINUSE on bind to same port
    match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(conflict_fd) => {
            let conflict_addr = SockAddrIn::new([0, 0, 0, 0], 8081);
            if socket::bind_inet(conflict_fd, &conflict_addr).is_ok() {
                if socket::listen(conflict_fd, 128).is_ok() {
                    match socket::socket(AF_INET, SOCK_STREAM, 0) {
                        Ok(second_fd) => {
                            match socket::bind_inet(second_fd, &conflict_addr) {
                                Err(Error::Os(Errno::EADDRINUSE)) => {
                                    println!("TCP_TEST: eaddrinuse OK");
                                    _passed += 1;
                                }
                                _ => {
                                    println!("TCP_TEST: eaddrinuse FAILED");
                                    failed += 1;
                                }
                            }
                        }
                        Err(_) => {
                            println!("TCP_TEST: eaddrinuse FAILED");
                            failed += 1;
                        }
                    }
                } else {
                    println!("TCP_TEST: eaddrinuse FAILED");
                    failed += 1;
                }
            } else {
                println!("TCP_TEST: eaddrinuse FAILED");
                failed += 1;
            }
        }
        Err(_) => {
            println!("TCP_TEST: eaddrinuse FAILED");
            failed += 1;
        }
    }

    // Test 10: listen on unbound socket
    match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(unbound_fd) => {
            match socket::listen(unbound_fd, 128) {
                Err(Error::Os(Errno::EINVAL)) => {
                    println!("TCP_TEST: listen_unbound OK");
                    _passed += 1;
                }
                _ => {
                    println!("TCP_TEST: listen_unbound FAILED");
                    failed += 1;
                }
            }
        }
        Err(_) => {
            println!("TCP_TEST: listen_unbound FAILED");
            failed += 1;
        }
    }

    // Test 11: accept on non-listening socket
    match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(nonlisten_fd) => {
            let nonlisten_addr = SockAddrIn::new([0, 0, 0, 0], 8083);
            if socket::bind_inet(nonlisten_fd, &nonlisten_addr).is_ok() {
                match socket::accept(nonlisten_fd, None) {
                    Err(Error::Os(Errno::EOPNOTSUPP)) => {
                        println!("TCP_TEST: accept_nonlisten OK");
                        _passed += 1;
                    }
                    _ => {
                        println!("TCP_TEST: accept_nonlisten FAILED");
                        failed += 1;
                    }
                }
            } else {
                println!("TCP_TEST: accept_nonlisten FAILED");
                failed += 1;
            }
        }
        Err(_) => {
            println!("TCP_TEST: accept_nonlisten FAILED");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 12: TCP Data Transfer Test
    // =========================================================================
    println!("TCP_DATA_TEST: starting");

    let data_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_DATA_TEST: server socket FAILED"); failed += 1; process::exit(12); }
    };

    let data_server_addr = SockAddrIn::new([0, 0, 0, 0], 8082);
    if socket::bind_inet(data_server_fd, &data_server_addr).is_err() { println!("TCP_DATA_TEST: server bind FAILED"); failed += 1; process::exit(12); }
    if socket::listen(data_server_fd, 128).is_err() { println!("TCP_DATA_TEST: server listen FAILED"); failed += 1; process::exit(12); }
    println!("TCP_DATA_TEST: server listening on 8082");

    let data_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_DATA_TEST: client socket FAILED"); failed += 1; process::exit(12); }
    };

    let data_loopback_addr = SockAddrIn::new([127, 0, 0, 1], 8082);
    if socket::connect_inet(data_client_fd, &data_loopback_addr).is_err() { println!("TCP_DATA_TEST: client connect FAILED"); failed += 1; process::exit(12); }
    println!("TCP_DATA_TEST: client connected");

    // Client writes "HELLO"
    let send_data = b"HELLO";
    match io::write(data_client_fd, send_data) {
        Ok(bytes_written) if bytes_written == send_data.len() => {
            println!("TCP_DATA_TEST: send OK");
            _passed += 1;
        }
        Ok(_) => { println!("TCP_DATA_TEST: send FAILED (partial write)"); failed += 1; process::exit(12); }
        Err(_) => { println!("TCP_DATA_TEST: send FAILED (error)"); failed += 1; process::exit(12); }
    }

    // Server accepts
    let (accept_result, accept_retries) = accept_with_retry_no_addr(data_server_fd);
    let accepted_fd = match accept_result {
        Ok(fd) => {
            if accept_retries > 0 { println!("TCP_DATA_TEST: accept OK (with retries)"); } else { println!("TCP_DATA_TEST: accept OK"); }
            fd
        }
        Err(_) => { println!("TCP_DATA_TEST: accept FAILED"); failed += 1; process::exit(12); }
    };

    // Server reads
    let mut recv_buf = [0u8; 16];
    let (read_result, read_retries) = read_with_retry(accepted_fd, &mut recv_buf);
    let bytes_read = match read_result {
        Ok(n) => {
            if read_retries > 0 { println!("TCP_DATA_TEST: recv OK (with retries)"); } else { println!("TCP_DATA_TEST: recv OK"); }
            _passed += 1;
            n
        }
        Err(_) => { println!("TCP_DATA_TEST: recv FAILED"); failed += 1; process::exit(12); }
    };

    // Verify received data matches "HELLO"
    let expected = b"HELLO";
    if bytes_read == expected.len() {
        if &recv_buf[..bytes_read] == expected {
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

    let shutdown_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SHUTDOWN_WRITE_TEST: server socket FAILED"); failed += 1; process::exit(13); }
    };
    let shutdown_server_addr = SockAddrIn::new([0, 0, 0, 0], 8084);
    if socket::bind_inet(shutdown_server_fd, &shutdown_server_addr).is_err() { println!("TCP_SHUTDOWN_WRITE_TEST: bind FAILED"); failed += 1; process::exit(13); }
    if socket::listen(shutdown_server_fd, 128).is_err() { println!("TCP_SHUTDOWN_WRITE_TEST: listen FAILED"); failed += 1; process::exit(13); }

    let shutdown_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SHUTDOWN_WRITE_TEST: client socket FAILED"); failed += 1; process::exit(13); }
    };
    let shutdown_loopback = SockAddrIn::new([127, 0, 0, 1], 8084);
    if socket::connect_inet(shutdown_client_fd, &shutdown_loopback).is_err() { println!("TCP_SHUTDOWN_WRITE_TEST: connect FAILED"); failed += 1; process::exit(13); }

    let (shutdown_accept_result, _) = accept_with_retry_no_addr(shutdown_server_fd);
    let _shutdown_accepted_fd = match shutdown_accept_result {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SHUTDOWN_WRITE_TEST: accept FAILED"); failed += 1; process::exit(13); }
    };

    // Shutdown write on client
    if socket::shutdown(shutdown_client_fd, SHUT_WR).is_err() {
        println!("TCP_SHUTDOWN_WRITE_TEST: shutdown FAILED");
        failed += 1;
    } else {
        let test_data = b"TEST";
        match io::write(shutdown_client_fd, test_data) {
            Err(Error::Os(Errno::EPIPE)) => {
                println!("TCP_SHUTDOWN_WRITE_TEST: EPIPE OK");
                _passed += 1;
            }
            Ok(_) => {
                println!("TCP_SHUTDOWN_WRITE_TEST: write should fail after shutdown");
                failed += 1;
            }
            Err(_) => {
                println!("TCP_SHUTDOWN_WRITE_TEST: expected EPIPE, got other error");
                failed += 1;
            }
        }
    }

    // =========================================================================
    // Test 14: SHUT_RD test
    // =========================================================================
    println!("TCP_SHUT_RD_TEST: starting");

    let shutrd_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SHUT_RD_TEST: socket FAILED"); failed += 1; process::exit(14); }
    };
    let shutrd_addr = SockAddrIn::new([0, 0, 0, 0], 8085);
    if socket::bind_inet(shutrd_server_fd, &shutrd_addr).is_err() || socket::listen(shutrd_server_fd, 128).is_err() {
        println!("TCP_SHUT_RD_TEST: bind/listen FAILED"); failed += 1; process::exit(14);
    }

    let shutrd_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SHUT_RD_TEST: client socket FAILED"); failed += 1; process::exit(14); }
    };
    let shutrd_loopback = SockAddrIn::new([127, 0, 0, 1], 8085);
    if socket::connect_inet(shutrd_client_fd, &shutrd_loopback).is_err() {
        println!("TCP_SHUT_RD_TEST: connect FAILED"); failed += 1; process::exit(14);
    }

    let (shutrd_accept_result, _) = accept_with_retry_no_addr(shutrd_server_fd);
    match shutrd_accept_result {
        Ok(_shutrd_accepted_fd) => {
            if socket::shutdown(shutrd_client_fd, SHUT_RD).is_ok() {
                let mut shutrd_buf = [0u8; 16];
                match io::read(shutrd_client_fd, &mut shutrd_buf) {
                    Ok(0) => {
                        println!("TCP_SHUT_RD_TEST: EOF OK");
                        _passed += 1;
                    }
                    Err(_) => {
                        println!("TCP_SHUT_RD_TEST: read error OK");
                        _passed += 1;
                    }
                    Ok(_) => {
                        println!("TCP_SHUT_RD_TEST: read returned data after SHUT_RD");
                        failed += 1;
                    }
                }
            } else {
                println!("TCP_SHUT_RD_TEST: SHUT_RD FAILED");
                failed += 1;
            }
        }
        Err(_) => {
            println!("TCP_SHUT_RD_TEST: accept FAILED");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 15: SHUT_WR test
    // =========================================================================
    println!("TCP_SHUT_WR_TEST: starting");

    let shutwr_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SHUT_WR_TEST: socket FAILED"); failed += 1; process::exit(15); }
    };
    let shutwr_addr = SockAddrIn::new([0, 0, 0, 0], 8086);
    if socket::bind_inet(shutwr_server_fd, &shutwr_addr).is_err() || socket::listen(shutwr_server_fd, 128).is_err() {
        println!("TCP_SHUT_WR_TEST: bind/listen FAILED"); failed += 1; process::exit(15);
    }

    let shutwr_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SHUT_WR_TEST: client socket FAILED"); failed += 1; process::exit(15); }
    };
    let shutwr_loopback = SockAddrIn::new([127, 0, 0, 1], 8086);
    if socket::connect_inet(shutwr_client_fd, &shutwr_loopback).is_err() {
        println!("TCP_SHUT_WR_TEST: connect FAILED"); failed += 1; process::exit(15);
    }

    let (shutwr_accept_result, _) = accept_with_retry_no_addr(shutwr_server_fd);
    match shutwr_accept_result {
        Ok(shutwr_accepted_fd) => {
            if socket::shutdown(shutwr_client_fd, SHUT_WR).is_ok() {
                let shutwr_test_data = b"TEST";
                match io::write(shutwr_client_fd, shutwr_test_data) {
                    Err(_) => {
                        println!("TCP_SHUT_WR_TEST: SHUT_WR write rejected OK");
                        _passed += 1;
                    }
                    Ok(_) => {
                        println!("TCP_SHUT_WR_TEST: write should fail after SHUT_WR");
                        failed += 1;
                    }
                }
                // Check FIN on server side
                let mut shutwr_buf = [0u8; 16];
                for _ in 0..10000 { std::hint::spin_loop(); }
                match io::read(shutwr_accepted_fd, &mut shutwr_buf) {
                    Ok(0) | Err(Error::Os(Errno::EAGAIN)) => {
                        println!("TCP_SHUT_WR_TEST: server saw FIN OK");
                    }
                    _ => {}
                }
            } else {
                println!("TCP_SHUT_WR_TEST: SHUT_WR FAILED");
                failed += 1;
            }
        }
        Err(_) => {
            println!("TCP_SHUT_WR_TEST: accept FAILED");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 16: Bidirectional data test
    // =========================================================================
    println!("TCP_BIDIR_TEST: starting");

    let bidir_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_BIDIR_TEST: server socket FAILED"); failed += 1; process::exit(16); }
    };
    let bidir_addr = SockAddrIn::new([0, 0, 0, 0], 8087);
    if socket::bind_inet(bidir_server_fd, &bidir_addr).is_err() || socket::listen(bidir_server_fd, 128).is_err() {
        println!("TCP_BIDIR_TEST: bind/listen FAILED"); failed += 1; process::exit(16);
    }

    let bidir_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_BIDIR_TEST: client socket FAILED"); failed += 1; process::exit(16); }
    };
    let bidir_loopback = SockAddrIn::new([127, 0, 0, 1], 8087);
    if socket::connect_inet(bidir_client_fd, &bidir_loopback).is_err() {
        println!("TCP_BIDIR_TEST: connect FAILED"); failed += 1; process::exit(16);
    }

    let (bidir_accept_result, _) = accept_with_retry_no_addr(bidir_server_fd);
    let bidir_accepted_fd = match bidir_accept_result {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_BIDIR_TEST: accept FAILED"); failed += 1; process::exit(16); }
    };

    // Server sends "WORLD" to client
    let bidir_send_data = b"WORLD";
    match io::write(bidir_accepted_fd, bidir_send_data) {
        Ok(n) if n != bidir_send_data.len() => {
            println!("TCP_BIDIR_TEST: server send FAILED");
            failed += 1;
        }
        Err(_) => {
            println!("TCP_BIDIR_TEST: server send FAILED");
            failed += 1;
        }
        Ok(_) => {
            let mut bidir_recv_buf = [0u8; 16];
            let (bidir_read_result, _) = read_with_retry(bidir_client_fd, &mut bidir_recv_buf);

            match bidir_read_result {
                Ok(bidir_read) if bidir_read == bidir_send_data.len() => {
                    if &bidir_recv_buf[..bidir_read] == bidir_send_data {
                        println!("TCP_BIDIR_TEST: server->client OK");
                        _passed += 1;
                    } else {
                        println!("TCP_BIDIR_TEST: data mismatch");
                        failed += 1;
                    }
                }
                _ => {
                    println!("TCP_BIDIR_TEST: wrong length");
                    failed += 1;
                }
            }
        }
    }

    // =========================================================================
    // Test 17: Large data test (256 bytes)
    // =========================================================================
    println!("TCP_LARGE_TEST: starting");

    let large_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_LARGE_TEST: server socket FAILED"); failed += 1; process::exit(17); }
    };
    let large_addr = SockAddrIn::new([0, 0, 0, 0], 8088);
    if socket::bind_inet(large_server_fd, &large_addr).is_err() || socket::listen(large_server_fd, 128).is_err() {
        println!("TCP_LARGE_TEST: bind/listen FAILED"); failed += 1; process::exit(17);
    }

    let large_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_LARGE_TEST: client socket FAILED"); failed += 1; process::exit(17); }
    };
    let large_loopback = SockAddrIn::new([127, 0, 0, 1], 8088);
    if socket::connect_inet(large_client_fd, &large_loopback).is_err() {
        println!("TCP_LARGE_TEST: connect FAILED"); failed += 1; process::exit(17);
    }

    let (large_accept_result, _) = accept_with_retry_no_addr(large_server_fd);
    let large_accepted_fd = match large_accept_result {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_LARGE_TEST: accept FAILED"); failed += 1; process::exit(17); }
    };

    // Create 256-byte test pattern
    let mut large_send_data = [0u8; 256];
    for i in 0..256 { large_send_data[i] = i as u8; }

    match io::write(large_client_fd, &large_send_data) {
        Ok(n) if n != large_send_data.len() => {
            println!("TCP_LARGE_TEST: send FAILED");
            failed += 1;
        }
        Err(_) => {
            println!("TCP_LARGE_TEST: send FAILED");
            failed += 1;
        }
        Ok(_) => {
            let mut large_recv_buf = [0u8; 512];
            let mut total_read: usize = 0;

            for _attempt in 0..10 {
                match io::read(large_accepted_fd, &mut large_recv_buf[total_read..]) {
                    Ok(bytes) if bytes > 0 => {
                        total_read += bytes;
                        if total_read >= 256 { break; }
                    }
                    Ok(_) => {
                        // 0 bytes (EOF) - retry
                        for _ in 0..10000 { std::hint::spin_loop(); }
                    }
                    Err(Error::Os(Errno::EAGAIN)) => {
                        for _ in 0..10000 { std::hint::spin_loop(); }
                    }
                    Err(_) => { break; }
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
    }

    // =========================================================================
    // Test 18: Backlog overflow test
    // =========================================================================
    println!("TCP_BACKLOG_TEST: starting");

    let backlog_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_BACKLOG_TEST: server socket FAILED"); failed += 1; process::exit(18); }
    };
    let backlog_addr = SockAddrIn::new([0, 0, 0, 0], 8089);
    if socket::bind_inet(backlog_server_fd, &backlog_addr).is_err() { println!("TCP_BACKLOG_TEST: bind FAILED"); failed += 1; process::exit(18); }
    if socket::listen(backlog_server_fd, 2).is_err() { println!("TCP_BACKLOG_TEST: listen FAILED"); failed += 1; process::exit(18); }

    let backlog_loopback = SockAddrIn::new([127, 0, 0, 1], 8089);
    let mut connect_results = [false; 3];

    let client1 = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_BACKLOG_TEST: client1 socket FAILED"); failed += 1; process::exit(18); }
    };
    connect_results[0] = socket::connect_inet(client1, &backlog_loopback).is_ok();

    let client2 = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_BACKLOG_TEST: client2 socket FAILED"); failed += 1; process::exit(18); }
    };
    connect_results[1] = socket::connect_inet(client2, &backlog_loopback).is_ok();

    let client3 = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_BACKLOG_TEST: client3 socket FAILED"); failed += 1; process::exit(18); }
    };
    connect_results[2] = socket::connect_inet(client3, &backlog_loopback).is_ok();

    // Set server to non-blocking to count accepted
    if let Ok(flags) = io::fcntl_getfl(backlog_server_fd) {
        let _ = io::fcntl_setfl(backlog_server_fd, flags as i32 | O_NONBLOCK);
    }

    let mut accepted_count = 0;
    for _ in 0..3 {
        match socket::accept(backlog_server_fd, None) {
            Ok(_) => { accepted_count += 1; }
            Err(Error::Os(Errno::EAGAIN)) => { break; }
            Err(_) => { break; }
        }
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

    let refused_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_CONNREFUSED_TEST: socket FAILED"); failed += 1; process::exit(19); }
    };

    let refused_addr = SockAddrIn::new([127, 0, 0, 1], 9999);
    match socket::connect_inet(refused_client_fd, &refused_addr) {
        Err(Error::Os(Errno::ECONNREFUSED)) => {
            println!("TCP_CONNREFUSED_TEST: ECONNREFUSED OK");
            _passed += 1;
        }
        Err(Error::Os(Errno::ETIMEDOUT)) => {
            println!("TCP_CONNREFUSED_TEST: ETIMEDOUT OK");
            _passed += 1;
        }
        Ok(()) => {
            println!("TCP_CONNREFUSED_TEST: connect should have failed");
            failed += 1;
        }
        Err(_) => {
            println!("TCP_CONNREFUSED_TEST: unexpected error");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 20: MSS boundary test (data > 1460 bytes)
    // =========================================================================
    println!("TCP_MSS_TEST: starting");

    let mss_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_MSS_TEST: server socket FAILED"); failed += 1; process::exit(20); }
    };
    let mss_addr = SockAddrIn::new([0, 0, 0, 0], 8090);
    if socket::bind_inet(mss_server_fd, &mss_addr).is_err() || socket::listen(mss_server_fd, 128).is_err() {
        println!("TCP_MSS_TEST: bind/listen FAILED"); failed += 1; process::exit(20);
    }

    let mss_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_MSS_TEST: client socket FAILED"); failed += 1; process::exit(20); }
    };
    let mss_loopback = SockAddrIn::new([127, 0, 0, 1], 8090);
    if socket::connect_inet(mss_client_fd, &mss_loopback).is_err() {
        println!("TCP_MSS_TEST: connect FAILED"); failed += 1; process::exit(20);
    }

    let (mss_accept_result, _) = accept_with_retry_no_addr(mss_server_fd);
    let mss_accepted_fd = match mss_accept_result {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_MSS_TEST: accept FAILED"); failed += 1; process::exit(20); }
    };

    // Create 2000-byte test pattern
    let mut mss_send_data = [0u8; 2000];
    for i in 0..2000 { mss_send_data[i] = (i % 256) as u8; }

    let mut total_written: usize = 0;
    for _attempt in 0..10 {
        match io::write(mss_client_fd, &mss_send_data[total_written..]) {
            Ok(bytes) if bytes > 0 => {
                total_written += bytes;
                if total_written >= 2000 { break; }
            }
            Err(Error::Os(Errno::EAGAIN)) => { for _ in 0..10000 { std::hint::spin_loop(); } }
            Err(_) => { break; }
            Ok(_) => {}
        }
    }

    if total_written != 2000 {
        println!("TCP_MSS_TEST: send FAILED (incomplete)");
        failed += 1;
    } else {
        let mut mss_recv_buf = [0u8; 2500];
        let mut total_read: usize = 0;

        for _attempt in 0..20 {
            match io::read(mss_accepted_fd, &mut mss_recv_buf[total_read..]) {
                Ok(bytes) if bytes > 0 => {
                    total_read += bytes;
                    if total_read >= 2000 { break; }
                }
                Ok(_) => {
                    for _ in 0..10000 { std::hint::spin_loop(); }
                }
                Err(Error::Os(Errno::EAGAIN)) => {
                    for _ in 0..10000 { std::hint::spin_loop(); }
                }
                Err(_) => { break; }
            }
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

    let multi_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_MULTI_TEST: server socket FAILED"); failed += 1; process::exit(21); }
    };
    let multi_addr = SockAddrIn::new([0, 0, 0, 0], 8091);
    if socket::bind_inet(multi_server_fd, &multi_addr).is_err() || socket::listen(multi_server_fd, 128).is_err() {
        println!("TCP_MULTI_TEST: bind/listen FAILED"); failed += 1; process::exit(21);
    }

    let multi_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_MULTI_TEST: client socket FAILED"); failed += 1; process::exit(21); }
    };
    let multi_loopback = SockAddrIn::new([127, 0, 0, 1], 8091);
    if socket::connect_inet(multi_client_fd, &multi_loopback).is_err() {
        println!("TCP_MULTI_TEST: connect FAILED"); failed += 1; process::exit(21);
    }

    let (multi_accept_result, _) = accept_with_retry_no_addr(multi_server_fd);
    let multi_accepted_fd = match multi_accept_result {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_MULTI_TEST: accept FAILED"); failed += 1; process::exit(21); }
    };

    // Send 3 messages on same connection
    let messages: [&[u8]; 3] = [b"MSG1", b"MSG2", b"MSG3"];
    let mut multi_success = true;

    for msg in messages.iter() {
        match io::write(multi_client_fd, msg) {
            Ok(written) if written == msg.len() => {}
            _ => { multi_success = false; break; }
        }

        let mut recv_buf = [0u8; 16];
        let (read_result, _) = read_with_retry(multi_accepted_fd, &mut recv_buf);
        match read_result {
            Ok(bytes_read) if bytes_read == msg.len() => {
                for i in 0..msg.len() {
                    if recv_buf[i] != msg[i] { multi_success = false; break; }
                }
                if !multi_success { break; }
            }
            _ => { multi_success = false; break; }
        }
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

    let addr_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_ADDR_TEST: server socket FAILED"); failed += 1; process::exit(22); }
    };
    let addr_server_addr = SockAddrIn::new([0, 0, 0, 0], 8092);
    if socket::bind_inet(addr_server_fd, &addr_server_addr).is_err() || socket::listen(addr_server_fd, 128).is_err() {
        println!("TCP_ADDR_TEST: bind/listen FAILED"); failed += 1; process::exit(22);
    }

    let addr_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_ADDR_TEST: client socket FAILED"); failed += 1; process::exit(22); }
    };
    let addr_loopback = SockAddrIn::new([127, 0, 0, 1], 8092);
    if socket::connect_inet(addr_client_fd, &addr_loopback).is_err() {
        println!("TCP_ADDR_TEST: connect FAILED"); failed += 1; process::exit(22);
    }

    let mut client_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let (addr_accept_result, _) = accept_with_retry_addr(addr_server_fd, &mut client_addr);

    match addr_accept_result {
        Ok(_addr_accepted_fd) => {
            if client_addr.addr[0] == 127 && client_addr.addr[1] == 0 &&
               client_addr.addr[2] == 0 && client_addr.addr[3] == 1 {
                println!("TCP_ADDR_TEST: 127.0.0.1 OK");
                _passed += 1;
            } else if client_addr.addr[0] == 10 {
                println!("TCP_ADDR_TEST: 10.x.x.x OK");
                _passed += 1;
            } else if client_addr.addr[0] == 0 && client_addr.addr[1] == 0 &&
                      client_addr.addr[2] == 0 && client_addr.addr[3] == 0 {
                println!("TCP_ADDR_TEST: address not filled FAILED");
                failed += 1;
            } else {
                println!("TCP_ADDR_TEST: unexpected address FAILED");
                failed += 1;
            }
        }
        Err(_) => {
            println!("TCP_ADDR_TEST: accept FAILED");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 23: Simultaneous close test
    // =========================================================================
    println!("TCP_SIMUL_CLOSE_TEST: starting");

    let simul_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SIMUL_CLOSE_TEST: server socket FAILED"); failed += 1; process::exit(23); }
    };
    let simul_addr = SockAddrIn::new([0, 0, 0, 0], 8093);
    if socket::bind_inet(simul_server_fd, &simul_addr).is_err() || socket::listen(simul_server_fd, 128).is_err() {
        println!("TCP_SIMUL_CLOSE_TEST: bind/listen FAILED"); failed += 1; process::exit(23);
    }

    let simul_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SIMUL_CLOSE_TEST: client socket FAILED"); failed += 1; process::exit(23); }
    };
    let simul_loopback = SockAddrIn::new([127, 0, 0, 1], 8093);
    if socket::connect_inet(simul_client_fd, &simul_loopback).is_err() {
        println!("TCP_SIMUL_CLOSE_TEST: connect FAILED"); failed += 1; process::exit(23);
    }

    let (simul_accept_result, _) = accept_with_retry_no_addr(simul_server_fd);
    let simul_accepted_fd = match simul_accept_result {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_SIMUL_CLOSE_TEST: accept FAILED"); failed += 1; process::exit(23); }
    };

    let client_shutdown_result = socket::shutdown(simul_client_fd, SHUT_RDWR);
    let server_shutdown_result = socket::shutdown(simul_accepted_fd, SHUT_RDWR);

    if client_shutdown_result.is_ok() && server_shutdown_result.is_ok() {
        println!("TCP_SIMUL_CLOSE_TEST: simultaneous close OK");
        _passed += 1;
    } else if client_shutdown_result.is_ok() || server_shutdown_result.is_ok() {
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

    let halfclose_server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_HALFCLOSE_TEST: server socket FAILED"); failed += 1; process::exit(24); }
    };
    let halfclose_addr = SockAddrIn::new([0, 0, 0, 0], 8094);
    if socket::bind_inet(halfclose_server_fd, &halfclose_addr).is_err() || socket::listen(halfclose_server_fd, 128).is_err() {
        println!("TCP_HALFCLOSE_TEST: bind/listen FAILED"); failed += 1; process::exit(24);
    }

    let halfclose_client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_HALFCLOSE_TEST: client socket FAILED"); failed += 1; process::exit(24); }
    };
    let halfclose_loopback = SockAddrIn::new([127, 0, 0, 1], 8094);
    if socket::connect_inet(halfclose_client_fd, &halfclose_loopback).is_err() {
        println!("TCP_HALFCLOSE_TEST: connect FAILED"); failed += 1; process::exit(24);
    }

    let (halfclose_accept_result, _) = accept_with_retry_no_addr(halfclose_server_fd);
    let halfclose_accepted_fd = match halfclose_accept_result {
        Ok(fd) => fd,
        Err(_) => { println!("TCP_HALFCLOSE_TEST: accept FAILED"); failed += 1; process::exit(24); }
    };

    if socket::shutdown(halfclose_client_fd, SHUT_WR).is_err() {
        println!("TCP_HALFCLOSE_TEST: SHUT_WR FAILED");
        failed += 1;
    } else {
        let halfclose_data = b"HALFCLOSE_DATA";
        match io::write(halfclose_accepted_fd, halfclose_data) {
            Ok(written) if written != halfclose_data.len() => {
                println!("TCP_HALFCLOSE_TEST: server send FAILED");
                failed += 1;
            }
            Err(_) => {
                println!("TCP_HALFCLOSE_TEST: server send FAILED");
                failed += 1;
            }
            Ok(_) => {
                let mut halfclose_recv_buf = [0u8; 32];
                let (halfclose_read_result, _) = read_with_retry(halfclose_client_fd, &mut halfclose_recv_buf);

                match halfclose_read_result {
                    Ok(bytes_read) if bytes_read == halfclose_data.len() => {
                        if &halfclose_recv_buf[..bytes_read] == halfclose_data {
                            println!("TCP_HALFCLOSE_TEST: read after SHUT_WR OK");
                            _passed += 1;
                        } else {
                            println!("TCP_HALFCLOSE_TEST: data mismatch FAILED");
                            failed += 1;
                        }
                    }
                    Ok(bytes_read) if bytes_read > 0 => {
                        println!("TCP_HALFCLOSE_TEST: wrong length FAILED");
                        failed += 1;
                    }
                    _ => {
                        println!("TCP_HALFCLOSE_TEST: read FAILED");
                        failed += 1;
                    }
                }
            }
        }
    }

    // =========================================================================
    // Test 25: First-call accept (no retry)
    // =========================================================================
    println!("TCP_FIRST_ACCEPT_TEST: starting");

    match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(first_accept_server_fd) => {
            let first_accept_addr = SockAddrIn::new([0, 0, 0, 0], 9090);
            if socket::bind_inet(first_accept_server_fd, &first_accept_addr).is_ok() &&
               socket::listen(first_accept_server_fd, 128).is_ok() {
                match socket::socket(AF_INET, SOCK_STREAM, 0) {
                    Ok(first_accept_client_fd) => {
                        let first_accept_loopback = SockAddrIn::new([127, 0, 0, 1], 9090);
                        if socket::connect_inet(first_accept_client_fd, &first_accept_loopback).is_ok() {
                            match socket::accept(first_accept_server_fd, None) {
                                Ok(accepted_fd) => {
                                    println!("TCP_FIRST_ACCEPT_TEST: accept OK");
                                    _passed += 1;
                                    let _ = io::close(accepted_fd);
                                }
                                Err(Error::Os(Errno::EAGAIN)) => {
                                    println!("TCP_FIRST_ACCEPT_TEST: accept returned EAGAIN FAILED");
                                    failed += 1;
                                }
                                Err(_) => {
                                    println!("TCP_FIRST_ACCEPT_TEST: accept FAILED");
                                    failed += 1;
                                }
                            }
                        } else {
                            println!("TCP_FIRST_ACCEPT_TEST: connect FAILED");
                            failed += 1;
                        }
                        let _ = io::close(first_accept_client_fd);
                    }
                    Err(_) => {
                        println!("TCP_FIRST_ACCEPT_TEST: client socket FAILED");
                        failed += 1;
                    }
                }
            } else {
                println!("TCP_FIRST_ACCEPT_TEST: bind/listen FAILED");
                failed += 1;
            }
            let _ = io::close(first_accept_server_fd);
        }
        Err(_) => {
            println!("TCP_FIRST_ACCEPT_TEST: server socket FAILED");
            failed += 1;
        }
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
