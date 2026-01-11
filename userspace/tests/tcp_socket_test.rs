//! TCP Socket userspace test
//!
//! Tests the TCP socket syscalls from userspace:
//! 1. Create a TCP socket (SOCK_STREAM) - MUST succeed
//! 2. Bind to a local port - MUST succeed
//! 3. Listen for connections - MUST succeed
//! 4. Create a second socket for client - MUST succeed
//! 5. Connect to server (loopback) - MUST succeed
//! 6. Accept on server - OK or EAGAIN (no pending connections)
//! 7. Shutdown connected socket (SHUT_RDWR) - MUST succeed
//! 8. Shutdown unconnected socket - MUST return ENOTCONN
//! 9. Bind same port - MUST return EADDRINUSE
//! 10. Listen on unbound socket - MUST return EINVAL
//! 11. Accept on non-listening socket - MUST return EOPNOTSUPP
//!
//! This validates the TCP syscall path from userspace to kernel.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::socket::{accept, bind, connect, listen, shutdown, socket, SockAddrIn, AF_INET, SHUT_RDWR, SOCK_STREAM};

// Expected errno values
const EAGAIN: i32 = 11;
const EADDRINUSE: i32 = 98;
const EINVAL: i32 = 22;
const EOPNOTSUPP: i32 = 95;
const ENOTCONN: i32 = 107;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("TCP Socket Test: Starting\n");
    let mut passed = 0;
    let mut failed = 0;

    // Test 1: Create TCP socket
    let server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => {
            io::print("TCP_TEST: socket created OK\n");
            passed += 1;
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
            passed += 1;
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
            passed += 1;
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
            passed += 1;
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
            passed += 1;
        }
        Err(_) => {
            io::print("TCP_TEST: connect FAILED\n");
            failed += 1;
            // Exit early - Tests 6-9 depend on a connected socket
            process::exit(5);
        }
    }

    // Test 6: Accept on server (should fail with EAGAIN)
    match accept(server_fd, None) {
        Ok(_) => {
            io::print("TCP_TEST: accept OK\n");
            passed += 1;
        }
        Err(EAGAIN) => {
            io::print("TCP_TEST: accept OK no_pending\n");
            passed += 1;
        }
        Err(_) => {
            io::print("TCP_TEST: accept FAILED\n");
            failed += 1;
        }
    }

    // Test 7: Shutdown connected socket (SHUT_RDWR) - MUST succeed
    match shutdown(client_fd, SHUT_RDWR) {
        Ok(()) => {
            io::print("TCP_TEST: shutdown OK\n");
            passed += 1;
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
                passed += 1;
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
                        passed += 1;
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
                passed += 1;
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
                    passed += 1;
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

    // Final result
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
