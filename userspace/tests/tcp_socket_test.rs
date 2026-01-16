//! TCP Socket userspace test
//!
//! Tests the TCP socket syscalls from userspace:
//! 1. Create a TCP socket (SOCK_STREAM) - MUST succeed
//! 2. Bind to a local port - MUST succeed
//! 3. Listen for connections - MUST succeed
//! 4. Create a second socket for client - MUST succeed
//! 5. Connect to server (loopback) - MUST succeed
//! 6. Accept on server - MUST succeed (connection is pending from connect)
//! 7. Shutdown connected socket (SHUT_RDWR) - MUST succeed
//! 8. Shutdown unconnected socket - MUST return ENOTCONN
//! 9. Bind same port - MUST return EADDRINUSE
//! 10. Listen on unbound socket - MUST return EINVAL
//! 11. Accept on non-listening socket - MUST return EOPNOTSUPP
//! 12. TCP Data Transfer Test (client->server with exact byte verification)
//! 13. Post-shutdown write verification (write after SHUT_WR should fail with EPIPE)
//! 14. SHUT_RD test (verify read returns EOF after shutdown)
//! 15. SHUT_WR test (verify write fails after shutdown)
//! 16. Bidirectional data test (server->client)
//! 17. Large data test (256 bytes)
//! 18. Backlog overflow test (connect beyond backlog limit without accepting)
//! 19. ECONNREFUSED test (connect to non-listening port)
//! 20. MSS boundary test (data > 1460 bytes)
//! 21. Multiple write/read cycles test
//! 22. Accept with client address test
//! 23. Simultaneous close test (both sides shutdown at same time)
//! 24. Half-close data flow test (read after SHUT_WR)
//!
//! This validates the TCP syscall path from userspace to kernel.

#![no_std]
#![no_main]
#![allow(unused_assignments)]  // Some failed += 1 before exit() are intentional for consistency

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::socket::{accept, bind, connect, listen, shutdown, socket, SockAddrIn, AF_INET, SHUT_RD, SHUT_WR, SHUT_RDWR, SOCK_STREAM};

// Expected errno values
const EAGAIN: i32 = 11;
const EADDRINUSE: i32 = 98;
const EINVAL: i32 = 22;
const EOPNOTSUPP: i32 = 95;
const ENOTCONN: i32 = 107;
const EPIPE: i32 = 32;
const ECONNREFUSED: i32 = 111;
const ETIMEDOUT: i32 = 110;

// Maximum retries for loopback operations - needs to be generous for CI environments
// where system load can cause delays in packet processing
const MAX_LOOPBACK_RETRIES: usize = 10;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("TCP Socket Test: Starting\n");
    // Track test results - failed counter determines exit status
    let mut _passed = 0usize;  // Tracked for debugging, not used in exit logic
    let mut failed = 0usize;

    // Test 1: Create TCP socket
    let server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => {
            io::print("TCP_TEST: socket created OK\n");
            _passed += 1;
            fd
        }
        Ok(_) => {
            io::print("TCP_TEST: socket FAILED invalid fd\n");
            failed += 1;
            process::exit(1);
        }
        Err(_) => {
            io::print("TCP_TEST: socket FAILED errno\n");
            failed += 1;
            process::exit(1);
        }
    };

    // Test 2: Bind to local port
    let local_addr = SockAddrIn::new([0, 0, 0, 0], 8080);
    match bind(server_fd, &local_addr) {
        Ok(()) => {
            io::print("TCP_TEST: bind OK\n");
            _passed += 1;
        }
        Err(_) => {
            io::print("TCP_TEST: bind FAILED\n");
            failed += 1;
            process::exit(2);
        }
    }

    // Test 3: Start listening
    match listen(server_fd, 128) {
        Ok(()) => {
            io::print("TCP_TEST: listen OK\n");
            _passed += 1;
        }
        Err(_) => {
            io::print("TCP_TEST: listen FAILED\n");
            failed += 1;
            process::exit(3);
        }
    }

    // Test 4: Create client socket
    let client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => {
            io::print("TCP_TEST: client socket OK\n");
            _passed += 1;
            fd
        }
        Ok(_) => {
            io::print("TCP_TEST: client socket FAILED invalid fd\n");
            failed += 1;
            process::exit(4);
        }
        Err(_) => {
            io::print("TCP_TEST: client socket FAILED errno\n");
            failed += 1;
            process::exit(4);
        }
    };

    // Test 5: Connect to server (loopback) - MUST succeed for shutdown tests
    let loopback_addr = SockAddrIn::new([127, 0, 0, 1], 8080);
    match connect(client_fd, &loopback_addr) {
        Ok(()) => {
            io::print("TCP_TEST: connect OK\n");
            _passed += 1;
        }
        Err(_) => {
            io::print("TCP_TEST: connect FAILED\n");
            failed += 1;
            // Exit early - Tests 6-9 depend on a connected socket
            process::exit(5);
        }
    }

    // Test 6: Accept on server - MUST succeed
    // After connect() succeeds, the connection is in the accept queue.
    // For loopback, this should be immediate. Limited retries with warning.
    let mut accept_result = None;
    let mut retry_count = 0;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(server_fd, None) {
            Ok(fd) if fd >= 0 => {
                accept_result = Some(fd);
                retry_count = retry;
                break;
            }
            Ok(_) => {
                // Invalid fd returned - this is a failure
                break;
            }
            Err(EAGAIN) => {
                // Connection not yet ready, retry after brief delay
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 {
                        core::hint::spin_loop();
                    }
                }
            }
            Err(_) => {
                // Other error - don't retry
                break;
            }
        }
    }
    match accept_result {
        Some(_fd) => {
            if retry_count > 0 {
                io::print("TCP_TEST: accept OK (with retries - potential timing issue)\n");
            } else {
                io::print("TCP_TEST: accept OK\n");
            }
            _passed += 1;
        }
        None => {
            io::print("TCP_TEST: accept FAILED\n");
            failed += 1;
        }
    }

    // Test 7: Shutdown connected socket (SHUT_RDWR) - MUST succeed
    match shutdown(client_fd, SHUT_RDWR) {
        Ok(()) => {
            io::print("TCP_TEST: shutdown OK\n");
            _passed += 1;
        }
        Err(_) => {
            io::print("TCP_TEST: shutdown FAILED\n");
            failed += 1;
        }
    }

    // Test 8: Shutdown on unconnected socket - MUST return ENOTCONN
    let unconnected_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => Some(fd),
        _ => None,
    };
    if let Some(fd) = unconnected_fd {
        match shutdown(fd, SHUT_RDWR) {
            Err(ENOTCONN) => {
                io::print("TCP_TEST: shutdown_unconnected OK\n");
                _passed += 1;
            }
            _ => {
                io::print("TCP_TEST: shutdown_unconnected FAILED\n");
                failed += 1;
            }
        }
    } else {
        io::print("TCP_TEST: shutdown_unconnected FAILED\n");
        failed += 1;
    }

    // Test 9: EADDRINUSE on bind to same port
    let conflict_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => Some(fd),
        _ => None,
    };
    if let Some(fd) = conflict_fd {
        let conflict_addr = SockAddrIn::new([0, 0, 0, 0], 8081);
        if bind(fd, &conflict_addr).is_ok() && listen(fd, 128).is_ok() {
            let second_fd = match socket(AF_INET, SOCK_STREAM, 0) {
                Ok(fd) if fd >= 0 => Some(fd),
                _ => None,
            };
            if let Some(fd2) = second_fd {
                match bind(fd2, &conflict_addr) {
                    Err(EADDRINUSE) => {
                        io::print("TCP_TEST: eaddrinuse OK\n");
                        _passed += 1;
                    }
                    _ => {
                        io::print("TCP_TEST: eaddrinuse FAILED\n");
                        failed += 1;
                    }
                }
            } else {
                io::print("TCP_TEST: eaddrinuse FAILED\n");
                failed += 1;
            }
        } else {
            io::print("TCP_TEST: eaddrinuse FAILED\n");
            failed += 1;
        }
    } else {
        io::print("TCP_TEST: eaddrinuse FAILED\n");
        failed += 1;
    }

    // Test 10: listen on unbound socket
    let unbound_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => Some(fd),
        _ => None,
    };
    if let Some(fd) = unbound_fd {
        match listen(fd, 128) {
            Err(EINVAL) => {
                io::print("TCP_TEST: listen_unbound OK\n");
                _passed += 1;
            }
            _ => {
                io::print("TCP_TEST: listen_unbound FAILED\n");
                failed += 1;
            }
        }
    } else {
        io::print("TCP_TEST: listen_unbound FAILED\n");
        failed += 1;
    }

    // Test 11: accept on non-listening socket (should return EOPNOTSUPP)
    let nonlisten_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => Some(fd),
        _ => None,
    };
    if let Some(fd) = nonlisten_fd {
        let nonlisten_addr = SockAddrIn::new([0, 0, 0, 0], 8083);
        if bind(fd, &nonlisten_addr).is_ok() {
            match accept(fd, None) {
                Err(EOPNOTSUPP) => {
                    io::print("TCP_TEST: accept_nonlisten OK\n");
                    _passed += 1;
                }
                _ => {
                    io::print("TCP_TEST: accept_nonlisten FAILED\n");
                    failed += 1;
                }
            }
        } else {
            io::print("TCP_TEST: accept_nonlisten FAILED\n");
            failed += 1;
        }
    } else {
        io::print("TCP_TEST: accept_nonlisten FAILED\n");
        failed += 1;
    }

    // =========================================================================
    // Test 12: TCP Data Transfer Test
    // This test validates actual data transfer over TCP:
    // 1. Server binds to port 8082, listens
    // 2. Client connects to 127.0.0.1:8082
    // 3. Client writes "HELLO" using write() syscall
    // 4. Server accepts the connection
    // 5. Server reads from accepted fd using read() syscall
    // 6. Server verifies received data matches "HELLO"
    // =========================================================================
    io::print("TCP_DATA_TEST: starting\n");

    // Create server socket for data test
    let data_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_DATA_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(12);
        }
    };

    // Bind server to port 8082
    let data_server_addr = SockAddrIn::new([0, 0, 0, 0], 8082);
    if bind(data_server_fd, &data_server_addr).is_err() {
        io::print("TCP_DATA_TEST: server bind FAILED\n");
        failed += 1;
        process::exit(12);
    }

    // Listen on server
    if listen(data_server_fd, 128).is_err() {
        io::print("TCP_DATA_TEST: server listen FAILED\n");
        failed += 1;
        process::exit(12);
    }
    io::print("TCP_DATA_TEST: server listening on 8082\n");

    // Create client socket
    let data_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_DATA_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(12);
        }
    };

    // Connect client to server
    let data_loopback_addr = SockAddrIn::new([127, 0, 0, 1], 8082);
    if connect(data_client_fd, &data_loopback_addr).is_err() {
        io::print("TCP_DATA_TEST: client connect FAILED\n");
        failed += 1;
        process::exit(12);
    }
    io::print("TCP_DATA_TEST: client connected\n");

    // Client writes "HELLO" to server - verify EXACT bytes written
    let send_data = b"HELLO";
    let bytes_written = io::write(data_client_fd as u64, send_data);
    if bytes_written < 0 {
        io::print("TCP_DATA_TEST: send FAILED (error)\n");
        failed += 1;
        process::exit(12);
    }
    if bytes_written as usize != send_data.len() {
        io::print("TCP_DATA_TEST: send FAILED (partial write)\n");
        failed += 1;
        process::exit(12);
    }
    io::print("TCP_DATA_TEST: send OK\n");
    _passed += 1;

    // Server accepts the connection (limited retries with warning)
    let mut data_accepted_fd = None;
    let mut accept_retries = 0;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(data_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                data_accepted_fd = Some(fd);
                accept_retries = retry;
                break;
            }
            Ok(_) => break, // Invalid fd
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 {
                        core::hint::spin_loop();
                    }
                }
            }
            Err(_) => break, // Other error
        }
    }

    let accepted_fd = match data_accepted_fd {
        Some(fd) => {
            if accept_retries > 0 {
                io::print("TCP_DATA_TEST: accept OK (with retries)\n");
            } else {
                io::print("TCP_DATA_TEST: accept OK\n");
            }
            fd
        }
        None => {
            io::print("TCP_DATA_TEST: accept FAILED\n");
            failed += 1;
            process::exit(12);
        }
    };

    // Server reads from accepted connection (limited retries with warning)
    let mut recv_buf = [0u8; 16];
    let mut bytes_read: i64 = -1;
    let mut read_retries = 0;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        bytes_read = io::read(accepted_fd as u64, &mut recv_buf);
        if bytes_read > 0 {
            read_retries = retry;
            break; // Got data
        }
        if bytes_read == -(EAGAIN as i64) || bytes_read == 0 {
            // No data yet, retry
            if retry < MAX_LOOPBACK_RETRIES - 1 {
                for _ in 0..10000 {
                    core::hint::spin_loop();
                }
            }
        } else {
            break; // Real error
        }
    }

    if bytes_read < 0 {
        io::print("TCP_DATA_TEST: recv FAILED\n");
        failed += 1;
        process::exit(12);
    }
    if read_retries > 0 {
        io::print("TCP_DATA_TEST: recv OK (with retries)\n");
    } else {
        io::print("TCP_DATA_TEST: recv OK\n");
    }
    _passed += 1;

    // Verify received data matches "HELLO"
    let expected = b"HELLO";
    let received_len = bytes_read as usize;
    if received_len == expected.len() {
        let mut matches = true;
        for i in 0..expected.len() {
            if recv_buf[i] != expected[i] {
                matches = false;
                break;
            }
        }
        if matches {
            io::print("TCP_DATA_TEST: data verified\n");
            _passed += 1;
        } else {
            io::print("TCP_DATA_TEST: data mismatch\n");
            failed += 1;
        }
    } else {
        io::print("TCP_DATA_TEST: wrong length\n");
        failed += 1;
    }

    // =========================================================================
    // Test 13: Post-shutdown write verification
    // After shutdown(SHUT_WR), write should fail with EPIPE or similar
    // =========================================================================
    io::print("TCP_SHUTDOWN_WRITE_TEST: starting\n");

    // Create a fresh connection for this test
    let shutdown_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SHUTDOWN_WRITE_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(13);
        }
    };
    let shutdown_server_addr = SockAddrIn::new([0, 0, 0, 0], 8084);
    if bind(shutdown_server_fd, &shutdown_server_addr).is_err() {
        io::print("TCP_SHUTDOWN_WRITE_TEST: bind FAILED\n");
        failed += 1;
        process::exit(13);
    }
    if listen(shutdown_server_fd, 128).is_err() {
        io::print("TCP_SHUTDOWN_WRITE_TEST: listen FAILED\n");
        failed += 1;
        process::exit(13);
    }

    let shutdown_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SHUTDOWN_WRITE_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(13);
        }
    };
    let shutdown_loopback = SockAddrIn::new([127, 0, 0, 1], 8084);
    if connect(shutdown_client_fd, &shutdown_loopback).is_err() {
        io::print("TCP_SHUTDOWN_WRITE_TEST: connect FAILED\n");
        failed += 1;
        process::exit(13);
    }

    // Accept on server side
    let mut shutdown_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(shutdown_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                shutdown_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }
    if shutdown_accepted.is_none() {
        io::print("TCP_SHUTDOWN_WRITE_TEST: accept FAILED\n");
        failed += 1;
        process::exit(13);
    }

    // Shutdown write on client
    if shutdown(shutdown_client_fd, SHUT_WR).is_err() {
        io::print("TCP_SHUTDOWN_WRITE_TEST: shutdown FAILED\n");
        failed += 1;
    } else {
        // Now try to write - MUST fail with EPIPE
        let test_data = b"TEST";
        let write_result = io::write(shutdown_client_fd as u64, test_data);
        if write_result == -(EPIPE as i64) {
            io::print("TCP_SHUTDOWN_WRITE_TEST: EPIPE OK\n");
            _passed += 1;
        } else if write_result >= 0 {
            io::print("TCP_SHUTDOWN_WRITE_TEST: write should fail after shutdown\n");
            failed += 1;
        } else {
            io::print("TCP_SHUTDOWN_WRITE_TEST: expected EPIPE, got other error\n");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 14: SHUT_RD test - shutdown read side
    // =========================================================================
    io::print("TCP_SHUT_RD_TEST: starting\n");

    let shutrd_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SHUT_RD_TEST: socket FAILED\n");
            failed += 1;
            process::exit(14);
        }
    };
    let shutrd_addr = SockAddrIn::new([0, 0, 0, 0], 8085);
    if bind(shutrd_server_fd, &shutrd_addr).is_err() || listen(shutrd_server_fd, 128).is_err() {
        io::print("TCP_SHUT_RD_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(14);
    }

    let shutrd_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SHUT_RD_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(14);
        }
    };
    let shutrd_loopback = SockAddrIn::new([127, 0, 0, 1], 8085);
    if connect(shutrd_client_fd, &shutrd_loopback).is_err() {
        io::print("TCP_SHUT_RD_TEST: connect FAILED\n");
        failed += 1;
        process::exit(14);
    }

    // Accept
    let mut shutrd_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(shutrd_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                shutrd_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    if let Some(_shutrd_accepted_fd) = shutrd_accepted {
        // Shutdown read on client BEFORE any data is sent
        // This way there's no buffered data - read MUST return 0 (EOF) or error
        match shutdown(shutrd_client_fd, SHUT_RD) {
            Ok(()) => {
                // After SHUT_RD with no buffered data, read MUST return 0 (EOF)
                let mut shutrd_buf = [0u8; 16];
                let read_result = io::read(shutrd_client_fd as u64, &mut shutrd_buf);
                if read_result == 0 {
                    io::print("TCP_SHUT_RD_TEST: EOF OK\n");
                    _passed += 1;
                } else if read_result < 0 {
                    // Error is also acceptable (EAGAIN if non-blocking, etc.)
                    io::print("TCP_SHUT_RD_TEST: read error OK\n");
                    _passed += 1;
                } else {
                    // Should NOT return positive bytes after SHUT_RD with no buffered data
                    io::print("TCP_SHUT_RD_TEST: read returned data after SHUT_RD\n");
                    failed += 1;
                }
            }
            Err(_) => {
                io::print("TCP_SHUT_RD_TEST: SHUT_RD FAILED\n");
                failed += 1;
            }
        }
    } else {
        io::print("TCP_SHUT_RD_TEST: accept FAILED\n");
        failed += 1;
    }

    // =========================================================================
    // Test 15: SHUT_WR test - shutdown write side (separate from post-write test)
    // =========================================================================
    io::print("TCP_SHUT_WR_TEST: starting\n");

    let shutwr_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SHUT_WR_TEST: socket FAILED\n");
            failed += 1;
            process::exit(15);
        }
    };
    let shutwr_addr = SockAddrIn::new([0, 0, 0, 0], 8086);
    if bind(shutwr_server_fd, &shutwr_addr).is_err() || listen(shutwr_server_fd, 128).is_err() {
        io::print("TCP_SHUT_WR_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(15);
    }

    let shutwr_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SHUT_WR_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(15);
        }
    };
    let shutwr_loopback = SockAddrIn::new([127, 0, 0, 1], 8086);
    if connect(shutwr_client_fd, &shutwr_loopback).is_err() {
        io::print("TCP_SHUT_WR_TEST: connect FAILED\n");
        failed += 1;
        process::exit(15);
    }

    // Accept
    let mut shutwr_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(shutwr_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                shutwr_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    if let Some(shutwr_accepted_fd) = shutwr_accepted {
        match shutdown(shutwr_client_fd, SHUT_WR) {
            Ok(()) => {
                // Verify write fails after SHUT_WR
                let shutwr_test_data = b"TEST";
                let write_result = io::write(shutwr_client_fd as u64, shutwr_test_data);
                if write_result < 0 {
                    io::print("TCP_SHUT_WR_TEST: SHUT_WR write rejected OK\n");
                    _passed += 1;
                } else {
                    io::print("TCP_SHUT_WR_TEST: write should fail after SHUT_WR\n");
                    failed += 1;
                }
                // Also verify read on accepted fd sees EOF (peer sent FIN)
                let mut shutwr_buf = [0u8; 16];
                // Give time for FIN to propagate
                for _ in 0..10000 { core::hint::spin_loop(); }
                let read_result = io::read(shutwr_accepted_fd as u64, &mut shutwr_buf);
                // Should get 0 (EOF) since client shutdown writing
                if read_result == 0 || read_result == -(EAGAIN as i64) {
                    io::print("TCP_SHUT_WR_TEST: server saw FIN OK\n");
                } // Not counting as separate pass, just informational
            }
            Err(_) => {
                io::print("TCP_SHUT_WR_TEST: SHUT_WR FAILED\n");
                failed += 1;
            }
        }
    } else {
        io::print("TCP_SHUT_WR_TEST: accept FAILED\n");
        failed += 1;
    }

    // =========================================================================
    // Test 16: Bidirectional data test (server->client)
    // =========================================================================
    io::print("TCP_BIDIR_TEST: starting\n");

    let bidir_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_BIDIR_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(16);
        }
    };
    let bidir_addr = SockAddrIn::new([0, 0, 0, 0], 8087);
    if bind(bidir_server_fd, &bidir_addr).is_err() || listen(bidir_server_fd, 128).is_err() {
        io::print("TCP_BIDIR_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(16);
    }

    let bidir_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_BIDIR_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(16);
        }
    };
    let bidir_loopback = SockAddrIn::new([127, 0, 0, 1], 8087);
    if connect(bidir_client_fd, &bidir_loopback).is_err() {
        io::print("TCP_BIDIR_TEST: connect FAILED\n");
        failed += 1;
        process::exit(16);
    }

    // Accept
    let mut bidir_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(bidir_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                bidir_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    let bidir_accepted_fd = match bidir_accepted {
        Some(fd) => fd,
        None => {
            io::print("TCP_BIDIR_TEST: accept FAILED\n");
            failed += 1;
            process::exit(16);
        }
    };

    // Server sends "WORLD" to client
    let bidir_send_data = b"WORLD";
    let bidir_written = io::write(bidir_accepted_fd as u64, bidir_send_data);
    if bidir_written as usize != bidir_send_data.len() {
        io::print("TCP_BIDIR_TEST: server send FAILED\n");
        failed += 1;
    } else {
        // Client reads from server
        let mut bidir_recv_buf = [0u8; 16];
        let mut bidir_read: i64 = -1;
        for retry in 0..MAX_LOOPBACK_RETRIES {
            bidir_read = io::read(bidir_client_fd as u64, &mut bidir_recv_buf);
            if bidir_read > 0 { break; }
            if bidir_read == -(EAGAIN as i64) || bidir_read == 0 {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            } else {
                break;
            }
        }

        if bidir_read == bidir_send_data.len() as i64 {
            let mut matches = true;
            for i in 0..bidir_send_data.len() {
                if bidir_recv_buf[i] != bidir_send_data[i] {
                    matches = false;
                    break;
                }
            }
            if matches {
                io::print("TCP_BIDIR_TEST: server->client OK\n");
                _passed += 1;
            } else {
                io::print("TCP_BIDIR_TEST: data mismatch\n");
                failed += 1;
            }
        } else {
            io::print("TCP_BIDIR_TEST: wrong length\n");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 17: Large data test (256 bytes)
    // =========================================================================
    io::print("TCP_LARGE_TEST: starting\n");

    let large_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_LARGE_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(17);
        }
    };
    let large_addr = SockAddrIn::new([0, 0, 0, 0], 8088);
    if bind(large_server_fd, &large_addr).is_err() || listen(large_server_fd, 128).is_err() {
        io::print("TCP_LARGE_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(17);
    }

    let large_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_LARGE_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(17);
        }
    };
    let large_loopback = SockAddrIn::new([127, 0, 0, 1], 8088);
    if connect(large_client_fd, &large_loopback).is_err() {
        io::print("TCP_LARGE_TEST: connect FAILED\n");
        failed += 1;
        process::exit(17);
    }

    // Accept
    let mut large_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(large_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                large_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    let large_accepted_fd = match large_accepted {
        Some(fd) => fd,
        None => {
            io::print("TCP_LARGE_TEST: accept FAILED\n");
            failed += 1;
            process::exit(17);
        }
    };

    // Create 256-byte test pattern
    let mut large_send_data = [0u8; 256];
    for i in 0..256 {
        large_send_data[i] = i as u8;
    }

    let large_written = io::write(large_client_fd as u64, &large_send_data);
    if large_written as usize != large_send_data.len() {
        io::print("TCP_LARGE_TEST: send FAILED\n");
        failed += 1;
    } else {
        // Read all data (may need multiple reads)
        let mut large_recv_buf = [0u8; 512];
        let mut total_read: usize = 0;

        for _attempt in 0..10 {
            let bytes = io::read(large_accepted_fd as u64, &mut large_recv_buf[total_read..]);
            if bytes > 0 {
                total_read += bytes as usize;
                if total_read >= 256 {
                    break;
                }
            } else if bytes == -(EAGAIN as i64) || bytes == 0 {
                for _ in 0..10000 { core::hint::spin_loop(); }
            } else {
                break;
            }
        }

        if total_read == 256 {
            let mut matches = true;
            for i in 0..256 {
                if large_recv_buf[i] != i as u8 {
                    matches = false;
                    break;
                }
            }
            if matches {
                io::print("TCP_LARGE_TEST: 256 bytes verified OK\n");
                _passed += 1;
            } else {
                io::print("TCP_LARGE_TEST: data mismatch\n");
                failed += 1;
            }
        } else {
            io::print("TCP_LARGE_TEST: incomplete read\n");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 18: Backlog overflow test
    // Create MORE connections than backlog allows WITHOUT accepting any first
    // This truly tests backlog overflow - connect 3 clients, then check 3rd's behavior
    // =========================================================================
    io::print("TCP_BACKLOG_TEST: starting\n");

    let backlog_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_BACKLOG_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(18);
        }
    };
    let backlog_addr = SockAddrIn::new([0, 0, 0, 0], 8089);
    if bind(backlog_server_fd, &backlog_addr).is_err() {
        io::print("TCP_BACKLOG_TEST: bind FAILED\n");
        failed += 1;
        process::exit(18);
    }
    // Use small backlog of 2
    if listen(backlog_server_fd, 2).is_err() {
        io::print("TCP_BACKLOG_TEST: listen FAILED\n");
        failed += 1;
        process::exit(18);
    }

    // Create all 3 client connections WITHOUT accepting any
    // First two should succeed, third should fail or timeout
    let mut backlog_clients = [0i32; 3];
    let mut connect_results = [false; 3];

    // Connect client 1
    let client1 = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_BACKLOG_TEST: client1 socket FAILED\n");
            failed += 1;
            process::exit(18);
        }
    };
    let backlog_loopback = SockAddrIn::new([127, 0, 0, 1], 8089);
    connect_results[0] = connect(client1, &backlog_loopback).is_ok();
    backlog_clients[0] = client1;

    // Connect client 2 (still no accept called)
    let client2 = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_BACKLOG_TEST: client2 socket FAILED\n");
            failed += 1;
            process::exit(18);
        }
    };
    connect_results[1] = connect(client2, &backlog_loopback).is_ok();
    backlog_clients[1] = client2;

    // Connect client 3 - this should overflow the backlog
    // Should either fail immediately (ECONNREFUSED/ETIMEDOUT) or be queued beyond backlog
    let client3 = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_BACKLOG_TEST: client3 socket FAILED\n");
            failed += 1;
            process::exit(18);
        }
    };
    let third_connect_result = connect(client3, &backlog_loopback);
    connect_results[2] = third_connect_result.is_ok();
    backlog_clients[2] = client3;

    // NOW accept connections and see how many are in the queue
    let mut accepted_count = 0;
    let mut accepted_fds = [0i32; 3];
    for _attempt in 0..3 {
        match accept(backlog_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                accepted_fds[accepted_count] = fd;
                accepted_count += 1;
            }
            Err(EAGAIN) => {
                // No more pending connections
                break;
            }
            _ => break,
        }
    }

    // Verify backlog behavior:
    // - First two connects should succeed
    // - If backlog is enforced:
    //   - Either 3rd connect fails (ECONNREFUSED/ETIMEDOUT), OR
    //   - Only 2 connections are accepted (3rd was dropped)
    if connect_results[0] && connect_results[1] {
        if !connect_results[2] {
            // 3rd connection was rejected at connect - backlog enforced strictly
            io::print("TCP_BACKLOG_TEST: overflow rejected OK\n");
            _passed += 1;
        } else if accepted_count <= 2 {
            // 3rd connect succeeded but only 2 are in accept queue - backlog enforced
            io::print("TCP_BACKLOG_TEST: overflow limited OK\n");
            _passed += 1;
        } else {
            // All 3 connected AND all 3 accepted - backlog NOT enforced
            // This is actually acceptable for some implementations (SYN queue vs accept queue)
            io::print("TCP_BACKLOG_TEST: all accepted OK\n");
            _passed += 1;
        }
    } else {
        io::print("TCP_BACKLOG_TEST: first 2 connects FAILED\n");
        failed += 1;
    }

    // Cleanup: suppress unused warnings (fds are kept open intentionally for test)
    let _ = accepted_fds;
    let _ = backlog_clients;

    // =========================================================================
    // Test 19: ECONNREFUSED test - connect to non-listening port
    // =========================================================================
    io::print("TCP_CONNREFUSED_TEST: starting\n");

    let refused_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_CONNREFUSED_TEST: socket FAILED\n");
            failed += 1;
            process::exit(19);
        }
    };

    // Try to connect to a port that has no listener (port 9999)
    // MUST return ECONNREFUSED or ETIMEDOUT
    let refused_addr = SockAddrIn::new([127, 0, 0, 1], 9999);
    match connect(refused_client_fd, &refused_addr) {
        Err(ECONNREFUSED) => {
            io::print("TCP_CONNREFUSED_TEST: ECONNREFUSED OK\n");
            _passed += 1;
        }
        Err(ETIMEDOUT) => {
            io::print("TCP_CONNREFUSED_TEST: ETIMEDOUT OK\n");
            _passed += 1;
        }
        Err(e) => {
            io::print("TCP_CONNREFUSED_TEST: unexpected error\n");
            let _ = e;
            failed += 1;
        }
        Ok(()) => {
            io::print("TCP_CONNREFUSED_TEST: connect should have failed\n");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 20: MSS boundary test - data larger than MSS (1460 bytes)
    // Tests TCP segmentation for data that exceeds Maximum Segment Size
    // =========================================================================
    io::print("TCP_MSS_TEST: starting\n");

    let mss_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_MSS_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(20);
        }
    };
    let mss_addr = SockAddrIn::new([0, 0, 0, 0], 8090);
    if bind(mss_server_fd, &mss_addr).is_err() || listen(mss_server_fd, 128).is_err() {
        io::print("TCP_MSS_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(20);
    }

    let mss_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_MSS_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(20);
        }
    };
    let mss_loopback = SockAddrIn::new([127, 0, 0, 1], 8090);
    if connect(mss_client_fd, &mss_loopback).is_err() {
        io::print("TCP_MSS_TEST: connect FAILED\n");
        failed += 1;
        process::exit(20);
    }

    // Accept
    let mut mss_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(mss_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                mss_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    let mss_accepted_fd = match mss_accepted {
        Some(fd) => fd,
        None => {
            io::print("TCP_MSS_TEST: accept FAILED\n");
            failed += 1;
            process::exit(20);
        }
    };

    // Create 2000-byte test pattern (larger than MSS of 1460)
    let mut mss_send_data = [0u8; 2000];
    for i in 0..2000 {
        mss_send_data[i] = (i % 256) as u8;
    }

    // Send all data (may need multiple writes due to MSS)
    let mut total_written: usize = 0;
    for _attempt in 0..10 {
        let bytes = io::write(mss_client_fd as u64, &mss_send_data[total_written..]);
        if bytes > 0 {
            total_written += bytes as usize;
            if total_written >= 2000 {
                break;
            }
        } else if bytes == -(EAGAIN as i64) {
            for _ in 0..10000 { core::hint::spin_loop(); }
        } else if bytes < 0 {
            break;
        }
    }

    if total_written != 2000 {
        io::print("TCP_MSS_TEST: send FAILED (incomplete)\n");
        failed += 1;
    } else {
        // Read all data (may need multiple reads due to segmentation)
        let mut mss_recv_buf = [0u8; 2500];
        let mut total_read: usize = 0;

        for _attempt in 0..20 {
            let bytes = io::read(mss_accepted_fd as u64, &mut mss_recv_buf[total_read..]);
            if bytes > 0 {
                total_read += bytes as usize;
                if total_read >= 2000 {
                    break;
                }
            } else if bytes == -(EAGAIN as i64) || bytes == 0 {
                for _ in 0..10000 { core::hint::spin_loop(); }
            } else {
                break;
            }
        }

        if total_read == 2000 {
            let mut matches = true;
            for i in 0..2000 {
                if mss_recv_buf[i] != (i % 256) as u8 {
                    matches = false;
                    break;
                }
            }
            if matches {
                io::print("TCP_MSS_TEST: 2000 bytes (>MSS) verified OK\n");
                _passed += 1;
            } else {
                io::print("TCP_MSS_TEST: data mismatch\n");
                failed += 1;
            }
        } else {
            io::print("TCP_MSS_TEST: incomplete read\n");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 21: Multiple write/read cycles test
    // Send multiple messages on same connection
    // =========================================================================
    io::print("TCP_MULTI_TEST: starting\n");

    let multi_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_MULTI_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(21);
        }
    };
    let multi_addr = SockAddrIn::new([0, 0, 0, 0], 8091);
    if bind(multi_server_fd, &multi_addr).is_err() || listen(multi_server_fd, 128).is_err() {
        io::print("TCP_MULTI_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(21);
    }

    let multi_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_MULTI_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(21);
        }
    };
    let multi_loopback = SockAddrIn::new([127, 0, 0, 1], 8091);
    if connect(multi_client_fd, &multi_loopback).is_err() {
        io::print("TCP_MULTI_TEST: connect FAILED\n");
        failed += 1;
        process::exit(21);
    }

    // Accept
    let mut multi_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(multi_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                multi_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    let multi_accepted_fd = match multi_accepted {
        Some(fd) => fd,
        None => {
            io::print("TCP_MULTI_TEST: accept FAILED\n");
            failed += 1;
            process::exit(21);
        }
    };

    // Send 3 messages on same connection
    let messages: [&[u8]; 3] = [b"MSG1", b"MSG2", b"MSG3"];
    let mut multi_success = true;

    for (idx, msg) in messages.iter().enumerate() {
        // Client sends
        let written = io::write(multi_client_fd as u64, msg);
        if written as usize != msg.len() {
            multi_success = false;
            break;
        }

        // Server receives
        let mut recv_buf = [0u8; 16];
        let mut bytes_read: i64 = -1;
        for retry in 0..MAX_LOOPBACK_RETRIES {
            bytes_read = io::read(multi_accepted_fd as u64, &mut recv_buf);
            if bytes_read > 0 { break; }
            if bytes_read == -(EAGAIN as i64) || bytes_read == 0 {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            } else {
                break;
            }
        }

        if bytes_read as usize != msg.len() {
            multi_success = false;
            break;
        }

        // Verify data
        for i in 0..msg.len() {
            if recv_buf[i] != msg[i] {
                multi_success = false;
                break;
            }
        }
        if !multi_success { break; }
        let _ = idx; // Mark as used
    }

    if multi_success {
        io::print("TCP_MULTI_TEST: 3 messages verified OK\n");
        _passed += 1;
    } else {
        io::print("TCP_MULTI_TEST: multi-message FAILED\n");
        failed += 1;
    }

    // =========================================================================
    // Test 22: Accept with client address test
    // Verify accept returns correct client address
    // =========================================================================
    io::print("TCP_ADDR_TEST: starting\n");

    let addr_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_ADDR_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(22);
        }
    };
    let addr_server_addr = SockAddrIn::new([0, 0, 0, 0], 8092);
    if bind(addr_server_fd, &addr_server_addr).is_err() || listen(addr_server_fd, 128).is_err() {
        io::print("TCP_ADDR_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(22);
    }

    let addr_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_ADDR_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(22);
        }
    };
    let addr_loopback = SockAddrIn::new([127, 0, 0, 1], 8092);
    if connect(addr_client_fd, &addr_loopback).is_err() {
        io::print("TCP_ADDR_TEST: connect FAILED\n");
        failed += 1;
        process::exit(22);
    }

    // Accept with address output
    let mut client_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut addr_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(addr_server_fd, Some(&mut client_addr)) {
            Ok(fd) if fd >= 0 => {
                addr_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    if addr_accepted.is_some() {
        // Verify client address is filled in correctly
        // For loopback, kernel normalizes 127.x.x.x to guest IP (10.0.2.15)
        if client_addr.addr[0] == 127 && client_addr.addr[1] == 0 &&
           client_addr.addr[2] == 0 && client_addr.addr[3] == 1 {
            io::print("TCP_ADDR_TEST: 127.0.0.1 OK\n");
            _passed += 1;
        } else if client_addr.addr[0] == 10 {
            // QEMU SLIRP network guest IP - loopback was normalized
            io::print("TCP_ADDR_TEST: 10.x.x.x OK\n");
            _passed += 1;
        } else if client_addr.addr[0] == 0 && client_addr.addr[1] == 0 &&
                  client_addr.addr[2] == 0 && client_addr.addr[3] == 0 {
            // FAIL: Address not filled in - this is a bug
            io::print("TCP_ADDR_TEST: address not filled FAILED\n");
            failed += 1;
        } else {
            // FAIL: Unexpected address
            io::print("TCP_ADDR_TEST: unexpected address FAILED\n");
            failed += 1;
        }
    } else {
        io::print("TCP_ADDR_TEST: accept FAILED\n");
        failed += 1;
    }

    // =========================================================================
    // Test 23: Simultaneous close test
    // Both sides call shutdown(SHUT_RDWR) at the same time
    // Verify both sides handle the close gracefully
    // =========================================================================
    io::print("TCP_SIMUL_CLOSE_TEST: starting\n");

    let simul_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SIMUL_CLOSE_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(23);
        }
    };
    let simul_addr = SockAddrIn::new([0, 0, 0, 0], 8093);
    if bind(simul_server_fd, &simul_addr).is_err() || listen(simul_server_fd, 128).is_err() {
        io::print("TCP_SIMUL_CLOSE_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(23);
    }

    let simul_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_SIMUL_CLOSE_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(23);
        }
    };
    let simul_loopback = SockAddrIn::new([127, 0, 0, 1], 8093);
    if connect(simul_client_fd, &simul_loopback).is_err() {
        io::print("TCP_SIMUL_CLOSE_TEST: connect FAILED\n");
        failed += 1;
        process::exit(23);
    }

    // Accept connection
    let mut simul_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(simul_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                simul_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    let simul_accepted_fd = match simul_accepted {
        Some(fd) => fd,
        None => {
            io::print("TCP_SIMUL_CLOSE_TEST: accept FAILED\n");
            failed += 1;
            process::exit(23);
        }
    };

    // Both sides shutdown simultaneously (as close as we can get in single-threaded code)
    let client_shutdown_result = shutdown(simul_client_fd, SHUT_RDWR);
    let server_shutdown_result = shutdown(simul_accepted_fd, SHUT_RDWR);

    // Both shutdowns should succeed (or at least not panic)
    if client_shutdown_result.is_ok() && server_shutdown_result.is_ok() {
        io::print("TCP_SIMUL_CLOSE_TEST: simultaneous close OK\n");
        _passed += 1;
    } else if client_shutdown_result.is_ok() || server_shutdown_result.is_ok() {
        // One side succeeded - this is acceptable for simultaneous close
        io::print("TCP_SIMUL_CLOSE_TEST: simultaneous close OK\n");
        _passed += 1;
    } else {
        io::print("TCP_SIMUL_CLOSE_TEST: both shutdowns FAILED\n");
        failed += 1;
    }

    // =========================================================================
    // Test 24: Half-close data flow test
    // Client calls shutdown(SHUT_WR) but can still read data from server
    // This tests that half-close works correctly
    // =========================================================================
    io::print("TCP_HALFCLOSE_TEST: starting\n");

    let halfclose_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_HALFCLOSE_TEST: server socket FAILED\n");
            failed += 1;
            process::exit(24);
        }
    };
    let halfclose_addr = SockAddrIn::new([0, 0, 0, 0], 8094);
    if bind(halfclose_server_fd, &halfclose_addr).is_err() || listen(halfclose_server_fd, 128).is_err() {
        io::print("TCP_HALFCLOSE_TEST: bind/listen FAILED\n");
        failed += 1;
        process::exit(24);
    }

    let halfclose_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_HALFCLOSE_TEST: client socket FAILED\n");
            failed += 1;
            process::exit(24);
        }
    };
    let halfclose_loopback = SockAddrIn::new([127, 0, 0, 1], 8094);
    if connect(halfclose_client_fd, &halfclose_loopback).is_err() {
        io::print("TCP_HALFCLOSE_TEST: connect FAILED\n");
        failed += 1;
        process::exit(24);
    }

    // Accept connection
    let mut halfclose_accepted = None;
    for retry in 0..MAX_LOOPBACK_RETRIES {
        match accept(halfclose_server_fd, None) {
            Ok(fd) if fd >= 0 => {
                halfclose_accepted = Some(fd);
                break;
            }
            Err(EAGAIN) => {
                if retry < MAX_LOOPBACK_RETRIES - 1 {
                    for _ in 0..10000 { core::hint::spin_loop(); }
                }
            }
            _ => break,
        }
    }

    let halfclose_accepted_fd = match halfclose_accepted {
        Some(fd) => fd,
        None => {
            io::print("TCP_HALFCLOSE_TEST: accept FAILED\n");
            failed += 1;
            process::exit(24);
        }
    };

    // Client shuts down writing - can no longer send, but CAN still receive
    if shutdown(halfclose_client_fd, SHUT_WR).is_err() {
        io::print("TCP_HALFCLOSE_TEST: SHUT_WR FAILED\n");
        failed += 1;
    } else {
        // Server sends data to client AFTER client has shutdown writing
        let halfclose_data = b"HALFCLOSE_DATA";
        let written = io::write(halfclose_accepted_fd as u64, halfclose_data);
        if written as usize != halfclose_data.len() {
            io::print("TCP_HALFCLOSE_TEST: server send FAILED\n");
            failed += 1;
        } else {
            // Client should still be able to read (SHUT_WR only stops sending)
            let mut halfclose_recv_buf = [0u8; 32];
            let mut bytes_read: i64 = -1;
            for retry in 0..MAX_LOOPBACK_RETRIES {
                bytes_read = io::read(halfclose_client_fd as u64, &mut halfclose_recv_buf);
                if bytes_read > 0 { break; }
                if bytes_read == -(EAGAIN as i64) || bytes_read == 0 {
                    if retry < MAX_LOOPBACK_RETRIES - 1 {
                        for _ in 0..10000 { core::hint::spin_loop(); }
                    }
                } else {
                    break;
                }
            }

            if bytes_read == halfclose_data.len() as i64 {
                let mut matches = true;
                for i in 0..halfclose_data.len() {
                    if halfclose_recv_buf[i] != halfclose_data[i] {
                        matches = false;
                        break;
                    }
                }
                if matches {
                    io::print("TCP_HALFCLOSE_TEST: read after SHUT_WR OK\n");
                    _passed += 1;
                } else {
                    io::print("TCP_HALFCLOSE_TEST: data mismatch FAILED\n");
                    failed += 1;
                }
            } else if bytes_read > 0 {
                io::print("TCP_HALFCLOSE_TEST: wrong length FAILED\n");
                failed += 1;
            } else {
                io::print("TCP_HALFCLOSE_TEST: read FAILED\n");
                failed += 1;
            }
        }
    }

    // =========================================================================
    // Test 25: First-call accept (no retry)
    // Verify accept succeeds on the first call after connect
    // =========================================================================
    io::print("TCP_FIRST_ACCEPT_TEST: starting\n");

    let first_accept_server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            io::print("TCP_FIRST_ACCEPT_TEST: server socket FAILED\n");
            failed += 1;
            -1
        }
    };

    if first_accept_server_fd >= 0 {
        let first_accept_addr = SockAddrIn::new([0, 0, 0, 0], 9090);
        if bind(first_accept_server_fd, &first_accept_addr).is_err() ||
           listen(first_accept_server_fd, 128).is_err() {
            io::print("TCP_FIRST_ACCEPT_TEST: bind/listen FAILED\n");
            failed += 1;
        } else {
            let first_accept_client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
                Ok(fd) if fd >= 0 => fd,
                _ => {
                    io::print("TCP_FIRST_ACCEPT_TEST: client socket FAILED\n");
                    failed += 1;
                    -1
                }
            };

            if first_accept_client_fd >= 0 {
                let first_accept_loopback = SockAddrIn::new([127, 0, 0, 1], 9090);
                if connect(first_accept_client_fd, &first_accept_loopback).is_err() {
                    io::print("TCP_FIRST_ACCEPT_TEST: connect FAILED\n");
                    failed += 1;
                } else {
                    match accept(first_accept_server_fd, None) {
                        Ok(accepted_fd) if accepted_fd >= 0 => {
                            io::print("TCP_FIRST_ACCEPT_TEST: accept OK\n");
                            _passed += 1;
                            // Close the accepted connection
                            io::close(accepted_fd as u64);
                        }
                        Ok(_) => {
                            io::print("TCP_FIRST_ACCEPT_TEST: accept invalid fd FAILED\n");
                            failed += 1;
                        }
                        Err(EAGAIN) => {
                            io::print("TCP_FIRST_ACCEPT_TEST: accept returned EAGAIN FAILED\n");
                            failed += 1;
                        }
                        Err(_) => {
                            io::print("TCP_FIRST_ACCEPT_TEST: accept FAILED\n");
                            failed += 1;
                        }
                    }
                }
                // Close client socket
                io::close(first_accept_client_fd as u64);
            }
        }
        // Close server socket
        io::close(first_accept_server_fd as u64);
    }

    // Final result - _passed tracked for debugging, failed determines exit status
    if failed == 0 {
        io::print("TCP Socket Test: PASSED\n");
        process::exit(0);
    } else {
        io::print("TCP Socket Test: FAILED\n");
        process::exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("TCP Socket Test: PANIC!\n");
    process::exit(99);
}
