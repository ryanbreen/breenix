//! TCP Blocking I/O Validation Test
//!
//! This test validates that TCP blocking operations actually BLOCK and do NOT return EAGAIN.
//! Unlike tcp_socket_test.rs which uses retry loops (masking whether blocking works),
//! this test uses fork() to create concurrent processes and expects single blocking calls
//! to succeed without EAGAIN handling.
//!
//! Test structure:
//! 1. Test blocking accept(): Parent blocks on accept(), child connects after delay
//! 2. Test blocking recv(): After accept, parent blocks on read(), child sends after delay
//! 3. Test blocking connect(): Validates connect() completes the TCP handshake
//!
//! If blocking I/O is broken (sockets returning EAGAIN in blocking mode), this test
//! will FAIL clearly rather than silently working through retries.
//!
//! Timeout Watchdog Mechanism:
//! The child process acts as a watchdog - after completing its work, it waits
//! an additional delay and then sends SIGKILL to the parent if still alive.
//! This prevents infinite hangs if blocking is broken.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::io::{fcntl_getfl, fcntl_setfl, status_flags::O_NONBLOCK};
use libbreenix::process;
use libbreenix::signal::{kill, SIGKILL};
use libbreenix::socket::{accept, bind, connect, listen, socket, SockAddrIn, AF_INET, SOCK_STREAM};

/// Timeout delay iterations for the watchdog
/// This should be significantly longer than the normal test delays
const WATCHDOG_TIMEOUT_ITERATIONS: usize = 500;

// Errno constants
const EAGAIN: i32 = 11;

/// Print a string
fn print(s: &str) {
    io::print(s);
}

/// Print a number
fn print_num(n: i64) {
    if n < 0 {
        print("-");
        print_num(-n);
        return;
    }
    if n == 0 {
        print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut val = n as u64;

    while val > 0 {
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
        i += 1;
    }

    while i > 0 {
        i -= 1;
        let ch = [buf[i]];
        if let Ok(s) = core::str::from_utf8(&ch) {
            print(s);
        }
    }
}

/// Yield CPU multiple times to give other process time to run
fn delay_yield(iterations: usize) {
    for _ in 0..iterations {
        process::yield_now();
        // Spin a bit too to consume time
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    print("TCP_BLOCKING_TEST: Starting\n");
    print("TCP_BLOCKING_TEST: This test validates blocking I/O - NO EAGAIN retry loops!\n\n");

    let mut failed = 0;

    // =========================================================================
    // Test 1: Blocking accept()
    //
    // Setup: Create server socket, bind, listen
    // Parent: Calls accept() which should BLOCK until child connects
    // Child: Waits briefly, then connects to server
    // Expectation: accept() returns a valid fd, NOT EAGAIN
    // =========================================================================
    print("=== TEST 1: Blocking accept() ===\n");

    // Create server socket
    let server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => {
            print("  Server socket created: fd=");
            print_num(fd as i64);
            print("\n");
            fd
        }
        Ok(fd) => {
            print("TCP_BLOCKING_TEST: FAIL - server socket invalid fd=");
            print_num(fd as i64);
            print("\n");
            process::exit(1);
        }
        Err(e) => {
            print("TCP_BLOCKING_TEST: FAIL - server socket failed, errno=");
            print_num(e as i64);
            print("\n");
            process::exit(1);
        }
    };

    // Bind to port 9100
    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9100);
    if let Err(e) = bind(server_fd, &server_addr) {
        print("TCP_BLOCKING_TEST: FAIL - bind failed, errno=");
        print_num(e as i64);
        print("\n");
        process::exit(2);
    }
    print("  Server bound to port 9100\n");

    // Listen
    if let Err(e) = listen(server_fd, 128) {
        print("TCP_BLOCKING_TEST: FAIL - listen failed, errno=");
        print_num(e as i64);
        print("\n");
        process::exit(3);
    }
    print("  Server listening\n");

    // Get parent PID before forking so child can use it for watchdog
    let parent_pid = process::getpid() as i32;

    // Fork
    let fork_result = process::fork();

    if fork_result < 0 {
        print("TCP_BLOCKING_TEST: FAIL - fork failed, errno=");
        print_num(-fork_result);
        print("\n");
        process::exit(4);
    }

    if fork_result == 0 {
        // ===== CHILD PROCESS =====
        print("  [CHILD] Started, will delay then connect...\n");
        print("  [CHILD] Parent PID=");
        print_num(parent_pid as i64);
        print(" (watchdog target)\n");

        // Give parent time to enter blocking accept()
        delay_yield(50);

        // Create client socket
        let client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
            Ok(fd) if fd >= 0 => fd,
            _ => {
                print("  [CHILD] FAIL - client socket creation failed\n");
                // Watchdog: kill parent before exiting on error
                print("  [CHILD] WATCHDOG: Killing parent to prevent hang\n");
                let _ = kill(parent_pid, SIGKILL);
                process::exit(10);
            }
        };

        // Connect to server
        let loopback = SockAddrIn::new([127, 0, 0, 1], 9100);
        if let Err(e) = connect(client_fd, &loopback) {
            print("  [CHILD] FAIL - connect failed, errno=");
            print_num(e as i64);
            print("\n");
            // Watchdog: kill parent before exiting on error
            print("  [CHILD] WATCHDOG: Killing parent to prevent hang\n");
            let _ = kill(parent_pid, SIGKILL);
            process::exit(11);
        }
        print("  [CHILD] Connected to server\n");

        // Give parent time to call read()
        delay_yield(30);

        // Send test data
        let test_data = b"BLOCKING_TEST_DATA";
        let written = io::write(client_fd as u64, test_data);
        if written < 0 {
            print("  [CHILD] FAIL - write failed, errno=");
            print_num(-written);
            print("\n");
            // Watchdog: kill parent before exiting on error
            print("  [CHILD] WATCHDOG: Killing parent to prevent hang\n");
            let _ = kill(parent_pid, SIGKILL);
            process::exit(12);
        }
        print("  [CHILD] Sent ");
        print_num(written);
        print(" bytes\n");

        // Close socket
        io::close(client_fd as u64);

        // =========================================================================
        // WATCHDOG: Wait a bit longer and kill parent if still alive
        // If blocking I/O is working, the parent should have already completed
        // and exited (or at least be past the blocking calls). If the parent
        // is stuck in a blocking call that never returns, we kill it.
        // =========================================================================
        print("  [CHILD] Starting watchdog timer...\n");
        delay_yield(WATCHDOG_TIMEOUT_ITERATIONS);

        // Check if parent is still alive and kill it
        // kill(pid, 0) checks if process exists without sending a signal
        if kill(parent_pid, 0).is_ok() {
            print("  [CHILD] WATCHDOG TIMEOUT: Parent still alive after ");
            print_num(WATCHDOG_TIMEOUT_ITERATIONS as i64);
            print(" iterations!\n");
            print("  [CHILD] WATCHDOG: Blocking call did not complete in time - killing parent\n");
            print("TCP_BLOCKING_TEST: TIMEOUT - blocking I/O appears to be broken!\n");
            print("TCP_BLOCKING_TEST: FAIL\n");
            let _ = kill(parent_pid, SIGKILL);
            process::exit(99); // Special exit code for watchdog timeout
        } else {
            print("  [CHILD] Watchdog: Parent completed normally\n");
        }

        print("  [CHILD] Exiting successfully\n");
        process::exit(0);

    } else {
        // ===== PARENT PROCESS =====
        print("  [PARENT] Forked child pid=");
        print_num(fork_result);
        print("\n");

        // Call accept() - this should BLOCK until child connects
        // NO RETRY LOOP - single call that expects success
        print("  [PARENT] Calling accept() - should block until child connects...\n");

        let accepted_result = accept(server_fd, None);

        match accepted_result {
            Ok(accepted_fd) if accepted_fd >= 0 => {
                print("  [PARENT] accept() returned fd=");
                print_num(accepted_fd as i64);
                print(" - BLOCKING WORKED!\n");
                print("  TEST 1 (blocking accept): PASS\n\n");

                // =========================================================================
                // Test 2: Blocking read()
                //
                // Now that we have a connected socket, test that read() blocks
                // Parent: Calls read() which should BLOCK until child sends data
                // Expectation: read() returns data, NOT EAGAIN
                // =========================================================================
                print("=== TEST 2: Blocking recv() ===\n");
                print("  [PARENT] Calling read() - should block until child sends data...\n");

                let mut recv_buf = [0u8; 64];

                // Single blocking read - NO retry loop
                let bytes_read = io::read(accepted_fd as u64, &mut recv_buf);

                if bytes_read == -(EAGAIN as i64) {
                    print("  [PARENT] FAIL - read() returned EAGAIN (-11)\n");
                    print("  This means blocking read is NOT working!\n");
                    print("  TEST 2 (blocking recv): FAIL\n\n");
                    failed += 1;
                } else if bytes_read < 0 {
                    print("  [PARENT] FAIL - read() failed, errno=");
                    print_num(-bytes_read);
                    print("\n");
                    print("  TEST 2 (blocking recv): FAIL\n\n");
                    failed += 1;
                } else if bytes_read == 0 {
                    print("  [PARENT] FAIL - read() returned 0 (EOF) - expected data\n");
                    print("  TEST 2 (blocking recv): FAIL\n\n");
                    failed += 1;
                } else {
                    print("  [PARENT] read() returned ");
                    print_num(bytes_read);
                    print(" bytes - BLOCKING WORKED!\n");

                    // Verify the data
                    let expected = b"BLOCKING_TEST_DATA";
                    let received = &recv_buf[..bytes_read as usize];

                    if bytes_read as usize == expected.len() {
                        let mut data_matches = true;
                        for i in 0..expected.len() {
                            if received[i] != expected[i] {
                                data_matches = false;
                                break;
                            }
                        }

                        if data_matches {
                            print("  [PARENT] Data verified correctly\n");
                            print("  TEST 2 (blocking recv): PASS\n\n");
                        } else {
                            print("  [PARENT] FAIL - data mismatch\n");
                            print("  TEST 2 (blocking recv): FAIL\n\n");
                            failed += 1;
                        }
                    } else {
                        print("  [PARENT] FAIL - wrong byte count, expected ");
                        print_num(expected.len() as i64);
                        print("\n");
                        print("  TEST 2 (blocking recv): FAIL\n\n");
                        failed += 1;
                    }
                }

                io::close(accepted_fd as u64);
            }
            Ok(fd) => {
                print("  [PARENT] FAIL - accept() returned invalid fd=");
                print_num(fd as i64);
                print("\n");
                print("  TEST 1 (blocking accept): FAIL\n\n");
                failed += 1;
            }
            Err(EAGAIN) => {
                print("  [PARENT] FAIL - accept() returned EAGAIN (-11)\n");
                print("  This means blocking accept is NOT working!\n");
                print("  The socket should block until a connection arrives.\n");
                print("  TEST 1 (blocking accept): FAIL\n\n");
                failed += 1;
            }
            Err(e) => {
                print("  [PARENT] FAIL - accept() failed, errno=");
                print_num(e as i64);
                print("\n");
                print("  TEST 1 (blocking accept): FAIL\n\n");
                failed += 1;
            }
        }

        // Wait for child to exit
        let mut status: i32 = 0;
        let _ = process::waitpid(fork_result as i32, &mut status, 0);
    }

    // =========================================================================
    // Test 3: Non-blocking connect() returns EINPROGRESS
    //
    // This tests that non-blocking connect behaves correctly:
    // - MUST return EINPROGRESS (-115) immediately when connection is in progress
    // - This exercises the O_NONBLOCK check in sys_connect
    //
    // The kernel returns EINPROGRESS BEFORE calling drain_loopback_queue(),
    // so even on loopback we expect EINPROGRESS (not immediate success).
    // =========================================================================
    print("=== TEST 3: Non-blocking connect() returns EINPROGRESS ===\n");

    // Create a server first so there's something to connect to
    let server2_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            print("  FAIL - server2 socket creation failed\n");
            print("  TEST 3 (non-blocking connect): FAIL\n\n");
            failed += 1;

            // Skip to final results
            if failed == 0 {
                print("TCP_BLOCKING_TEST: ALL TESTS PASSED\n");
                print("TCP_BLOCKING_TEST: PASS\n");
                process::exit(0);
            } else {
                print("TCP_BLOCKING_TEST: ");
                print_num(failed as i64);
                print(" tests FAILED\n");
                print("TCP_BLOCKING_TEST: FAIL\n");
                process::exit(1);
            }
        }
    };

    let server2_addr = SockAddrIn::new([0, 0, 0, 0], 9101);
    if bind(server2_fd, &server2_addr).is_err() || listen(server2_fd, 128).is_err() {
        print("  FAIL - server2 bind/listen failed\n");
        print("  TEST 3 (non-blocking connect): FAIL\n\n");
        failed += 1;
    } else {
        print("  Server listening on port 9101\n");

        // Create client socket
        let client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
            Ok(fd) if fd >= 0 => fd,
            _ => {
                print("  FAIL - client socket creation failed\n");
                print("  TEST 3 (non-blocking connect): FAIL\n\n");
                failed += 1;
                -1
            }
        };

        if client_fd >= 0 {
            // Set socket to non-blocking mode
            let current_flags = fcntl_getfl(client_fd as u64);
            if current_flags < 0 {
                print("  FAIL - fcntl(F_GETFL) failed\n");
                print("  TEST 3 (non-blocking connect): FAIL\n\n");
                failed += 1;
            } else {
                let new_flags = (current_flags as i32) | O_NONBLOCK;
                let set_result = fcntl_setfl(client_fd as u64, new_flags);
                if set_result < 0 {
                    print("  FAIL - fcntl(F_SETFL, O_NONBLOCK) failed\n");
                    print("  TEST 3 (non-blocking connect): FAIL\n\n");
                    failed += 1;
                } else {
                    print("  Client socket set to non-blocking mode\n");

                    // Call connect() in non-blocking mode
                    // MUST return EINPROGRESS immediately
                    let loopback = SockAddrIn::new([127, 0, 0, 1], 9101);
                    print("  Calling connect() in non-blocking mode...\n");

                    match connect(client_fd, &loopback) {
                        Err(115) => {
                            // EINPROGRESS - this is the ONLY acceptable result
                            print("  connect() returned EINPROGRESS (-115) as expected!\n");
                            print("  TEST 3 (non-blocking connect): PASS\n\n");
                        }
                        Ok(()) => {
                            // Immediate success should NOT happen - we return EINPROGRESS
                            // before drain_loopback_queue() is called
                            print("  FAIL - connect() returned success (0) instead of EINPROGRESS\n");
                            print("  Non-blocking connect should always return EINPROGRESS\n");
                            print("  TEST 3 (non-blocking connect): FAIL\n\n");
                            failed += 1;
                        }
                        Err(e) => {
                            print("  FAIL - connect() returned unexpected error, errno=");
                            print_num(e as i64);
                            print("\n");
                            print("  Expected EINPROGRESS (-115) for non-blocking connect\n");
                            print("  TEST 3 (non-blocking connect): FAIL\n\n");
                            failed += 1;
                        }
                    }
                }
            }

            io::close(client_fd as u64);
        }
    }

    io::close(server2_fd as u64);

    // =========================================================================
    // Test 4: Non-blocking accept() returns EAGAIN
    //
    // This negative test proves that blocking vs non-blocking paths are different.
    // Setup: Create server socket, set O_NONBLOCK, bind, listen
    // Call accept() BEFORE any client connects
    // Expectation: accept() returns EAGAIN (-11) immediately
    // =========================================================================
    print("=== TEST 4: Non-blocking accept() returns EAGAIN ===\n");

    // Create server socket for non-blocking test
    let server3_fd = match socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) if fd >= 0 => {
            print("  Server socket created: fd=");
            print_num(fd as i64);
            print("\n");
            fd
        }
        Ok(fd) => {
            print("  FAIL - server3 socket invalid fd=");
            print_num(fd as i64);
            print("\n");
            print("  TEST 4 (non-blocking EAGAIN): FAIL\n\n");
            failed += 1;
            -1
        }
        Err(e) => {
            print("  FAIL - server3 socket creation failed, errno=");
            print_num(e as i64);
            print("\n");
            print("  TEST 4 (non-blocking EAGAIN): FAIL\n\n");
            failed += 1;
            -1
        }
    };

    if server3_fd >= 0 {
        // Set socket to non-blocking mode using fcntl
        let current_flags = fcntl_getfl(server3_fd as u64);
        if current_flags < 0 {
            print("  FAIL - fcntl(F_GETFL) failed, errno=");
            print_num(-current_flags);
            print("\n");
            print("  TEST 4 (non-blocking EAGAIN): FAIL\n\n");
            failed += 1;
        } else {
            let new_flags = (current_flags as i32) | O_NONBLOCK;
            let set_result = fcntl_setfl(server3_fd as u64, new_flags);
            if set_result < 0 {
                print("  FAIL - fcntl(F_SETFL, O_NONBLOCK) failed, errno=");
                print_num(-set_result);
                print("\n");
                print("  TEST 4 (non-blocking EAGAIN): FAIL\n\n");
                failed += 1;
            } else {
                print("  Socket set to non-blocking mode (O_NONBLOCK)\n");

                // Bind to port 9103
                let server3_addr = SockAddrIn::new([0, 0, 0, 0], 9103);
                if let Err(e) = bind(server3_fd, &server3_addr) {
                    print("  FAIL - bind failed, errno=");
                    print_num(e as i64);
                    print("\n");
                    print("  TEST 4 (non-blocking EAGAIN): FAIL\n\n");
                    failed += 1;
                } else {
                    print("  Server bound to port 9103\n");

                    // Listen
                    if let Err(e) = listen(server3_fd, 128) {
                        print("  FAIL - listen failed, errno=");
                        print_num(e as i64);
                        print("\n");
                        print("  TEST 4 (non-blocking EAGAIN): FAIL\n\n");
                        failed += 1;
                    } else {
                        print("  Server listening\n");

                        // Call accept() BEFORE any client connects
                        // With non-blocking mode, this should immediately return EAGAIN
                        print("  Calling accept() with NO pending connections...\n");

                        let accept_result = accept(server3_fd, None);

                        match accept_result {
                            Ok(fd) => {
                                // This would be unexpected - no client connected!
                                print("  FAIL - accept() returned fd=");
                                print_num(fd as i64);
                                print(" but no client connected!\n");
                                print("  TEST 4 (non-blocking EAGAIN): FAIL - expected EAGAIN but got fd\n\n");
                                failed += 1;
                                io::close(fd as u64);
                            }
                            Err(EAGAIN) => {
                                // This is the expected result for non-blocking with no connections
                                print("  accept() returned EAGAIN (-11) as expected!\n");
                                print("  TEST 4 (non-blocking EAGAIN): PASS\n\n");
                            }
                            Err(e) => {
                                print("  FAIL - accept() returned unexpected error, errno=");
                                print_num(e as i64);
                                print("\n");
                                print("  TEST 4 (non-blocking EAGAIN): FAIL - expected EAGAIN but got ");
                                print_num(e as i64);
                                print("\n\n");
                                failed += 1;
                            }
                        }
                    }
                }
            }
        }

        io::close(server3_fd as u64);
    }

    // =========================================================================
    // Test 5: connect() with invalid fd returns EBADF
    //
    // Validates error handling for bad file descriptors.
    // =========================================================================
    print("=== TEST 5: connect() with invalid fd returns EBADF ===\n");
    {
        let invalid_fd = 9999; // Unlikely to be a valid fd
        let addr = SockAddrIn::new([127, 0, 0, 1], 9999);
        print("  Calling connect() with invalid fd=9999...\n");

        match connect(invalid_fd, &addr) {
            Err(9) => {
                // EBADF - expected
                print("  connect() returned EBADF (-9) as expected!\n");
                print("  TEST 5 (EBADF): PASS\n\n");
            }
            Ok(()) => {
                print("  FAIL - connect() succeeded with invalid fd!\n");
                print("  TEST 5 (EBADF): FAIL\n\n");
                failed += 1;
            }
            Err(e) => {
                print("  FAIL - connect() returned unexpected error, errno=");
                print_num(e as i64);
                print("\n");
                print("  Expected EBADF (-9)\n");
                print("  TEST 5 (EBADF): FAIL\n\n");
                failed += 1;
            }
        }
    }

    // =========================================================================
    // Test 6: connect() on already-connected socket returns EISCONN
    //
    // Validates that calling connect() twice on the same socket fails properly.
    // =========================================================================
    print("=== TEST 6: connect() on connected socket returns EISCONN ===\n");
    {
        // Create server
        let server_fd = match socket(AF_INET, SOCK_STREAM, 0) {
            Ok(fd) if fd >= 0 => fd,
            _ => {
                print("  FAIL - server socket creation failed\n");
                print("  TEST 6 (EISCONN): FAIL\n\n");
                failed += 1;
                -1
            }
        };

        if server_fd >= 0 {
            let server_addr = SockAddrIn::new([0, 0, 0, 0], 9106);
            if bind(server_fd, &server_addr).is_err() || listen(server_fd, 128).is_err() {
                print("  FAIL - server bind/listen failed\n");
                print("  TEST 6 (EISCONN): FAIL\n\n");
                failed += 1;
            } else {
                // Create client and connect first time
                let client_fd = match socket(AF_INET, SOCK_STREAM, 0) {
                    Ok(fd) if fd >= 0 => fd,
                    _ => {
                        print("  FAIL - client socket creation failed\n");
                        print("  TEST 6 (EISCONN): FAIL\n\n");
                        failed += 1;
                        -1
                    }
                };

                if client_fd >= 0 {
                    let loopback = SockAddrIn::new([127, 0, 0, 1], 9106);

                    // First connect - should succeed (blocking)
                    print("  First connect() call...\n");
                    match connect(client_fd, &loopback) {
                        Ok(()) => {
                            print("  First connect() succeeded\n");

                            // Second connect on same socket - should fail with EISCONN
                            print("  Second connect() call on same socket...\n");
                            match connect(client_fd, &loopback) {
                                Err(106) => {
                                    // EISCONN - expected
                                    print("  connect() returned EISCONN (-106) as expected!\n");
                                    print("  TEST 6 (EISCONN): PASS\n\n");
                                }
                                Ok(()) => {
                                    print("  FAIL - second connect() succeeded!\n");
                                    print("  TEST 6 (EISCONN): FAIL\n\n");
                                    failed += 1;
                                }
                                Err(e) => {
                                    print("  FAIL - connect() returned unexpected error, errno=");
                                    print_num(e as i64);
                                    print("\n");
                                    print("  Expected EISCONN (-106)\n");
                                    print("  TEST 6 (EISCONN): FAIL\n\n");
                                    failed += 1;
                                }
                            }
                        }
                        Err(115) => {
                            // EINPROGRESS - wait a bit for connection to complete, then retry
                            print("  First connect() returned EINPROGRESS, waiting...\n");
                            delay_yield(50);
                            // Connection should be established now, try second connect
                            print("  Second connect() call on same socket...\n");
                            match connect(client_fd, &loopback) {
                                Err(106) => {
                                    print("  connect() returned EISCONN (-106) as expected!\n");
                                    print("  TEST 6 (EISCONN): PASS\n\n");
                                }
                                Ok(()) => {
                                    print("  FAIL - second connect() succeeded!\n");
                                    print("  TEST 6 (EISCONN): FAIL\n\n");
                                    failed += 1;
                                }
                                Err(e) => {
                                    print("  FAIL - connect() returned unexpected error, errno=");
                                    print_num(e as i64);
                                    print("\n");
                                    print("  Expected EISCONN (-106)\n");
                                    print("  TEST 6 (EISCONN): FAIL\n\n");
                                    failed += 1;
                                }
                            }
                        }
                        Err(e) => {
                            print("  FAIL - first connect() failed, errno=");
                            print_num(e as i64);
                            print("\n");
                            print("  TEST 6 (EISCONN): FAIL\n\n");
                            failed += 1;
                        }
                    }

                    io::close(client_fd as u64);
                }
            }

            io::close(server_fd as u64);
        }
    }

    // Final results
    print("=== FINAL RESULTS ===\n");
    if failed == 0 {
        print("TCP_BLOCKING_TEST: ALL TESTS PASSED\n");
        print("TCP_BLOCKING_TEST: PASS\n");
        process::exit(0);
    } else {
        print("TCP_BLOCKING_TEST: ");
        print_num(failed as i64);
        print(" tests FAILED\n");
        print("TCP_BLOCKING_TEST: FAIL\n");
        process::exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print("TCP_BLOCKING_TEST: PANIC!\n");
    process::exit(99);
}
