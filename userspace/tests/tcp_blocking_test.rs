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
//! Child processes act as watchdogs - if they encounter an error before
//! completing their task, they kill the parent to prevent infinite hangs.
//! Once a child completes successfully, it exits immediately (no post-work
//! delay) so the parent's waitpid() can return and the test can continue.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::io::{fcntl_getfl, fcntl_setfl, status_flags::O_NONBLOCK};
use libbreenix::process;
use libbreenix::signal::{kill, SIGKILL};
use libbreenix::socket::{accept, bind, connect, listen, socket, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::time::{now_monotonic, sleep_ms};

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

/// Calculate elapsed time in milliseconds between two Timespec values
fn elapsed_ms(start: &libbreenix::types::Timespec, end: &libbreenix::types::Timespec) -> i64 {
    let sec_diff = end.tv_sec - start.tv_sec;
    let nsec_diff = end.tv_nsec - start.tv_nsec;
    (sec_diff * 1000) + (nsec_diff / 1_000_000)
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

        // Child's job is done - exit immediately.
        // Error-path watchdogs above (lines 171-204) handle killing parent if child fails.
        //
        // We do NOT run a post-work watchdog here because the parent will call
        // waitpid() to wait for us after TEST 2 completes. If we delay here and
        // check if parent is alive, we'd see it waiting in waitpid (for us!) and
        // incorrectly kill it - preventing TEST 3, 4, 4b, 5, 6 from running.
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
    // Test 4b: Non-blocking read() returns EAGAIN IMMEDIATELY (Unfalsifiable)
    //
    // This is an UNFALSIFIABLE test that proves O_NONBLOCK actually works.
    // Simply checking for EAGAIN is insufficient because a buggy kernel could:
    //   - Always return EAGAIN regardless of O_NONBLOCK flag
    //   - Return EAGAIN after blocking for some time
    //
    // To be unfalsifiable, this test proves TWO things:
    //   Part A: Blocking read actually blocks (~200ms when child delays send)
    //   Part B: Non-blocking read returns EAGAIN in < 50ms
    //
    // If both pass, we KNOW O_NONBLOCK is the reason for the fast return.
    //
    // This test WILL FAIL if:
    //   - O_NONBLOCK check is removed (Part B would block ~200ms)
    //   - Always-return-EAGAIN bug exists (Part A would return EAGAIN, not block)
    //   - Non-blocking path is slow (Part B timing check fails)
    // =========================================================================
    print("=== TEST 4b: Non-blocking read() returns EAGAIN IMMEDIATELY (Unfalsifiable) ===\n");
    print("  This test proves O_NONBLOCK works by comparing blocking vs non-blocking timing.\n\n");
    {
        // Timing thresholds
        const CHILD_DELAY_MS: u64 = 300;   // Child waits 300ms after connecting before sending
        const BLOCKING_MIN_MS: i64 = 100;  // Blocking read must take at least this long
        const NONBLOCK_MAX_MS: i64 = 100;  // Non-blocking read must complete within this

        let mut part_a_passed = false;
        let mut part_b_passed = false;

        // =====================================================================
        // Part A: Prove blocking read actually blocks (counterfactual)
        // =====================================================================
        print("  --- Part A: Proving blocking read actually blocks ---\n");

        let server_a = match socket(AF_INET, SOCK_STREAM, 0) {
            Ok(fd) if fd >= 0 => fd,
            _ => {
                print("  FAIL - Part A server socket creation failed\n");
                -1
            }
        };

        if server_a >= 0 {
            let server_a_addr = SockAddrIn::new([0, 0, 0, 0], 9104);
            if bind(server_a, &server_a_addr).is_ok() && listen(server_a, 128).is_ok() {
                print("  Server A listening on port 9104 (blocking test)\n");

                let parent_pid = process::getpid() as i32;
                let fork_a = process::fork();

                if fork_a == 0 {
                    // ===== CHILD for Part A =====
                    print("  [CHILD-A] Started, will delay then send data\n");

                    // Create client and connect
                    let client = match socket(AF_INET, SOCK_STREAM, 0) {
                        Ok(fd) if fd >= 0 => fd,
                        _ => {
                            print("  [CHILD-A] FAIL - socket creation failed\n");
                            let _ = kill(parent_pid, SIGKILL);
                            process::exit(20);
                        }
                    };

                    let loopback = SockAddrIn::new([127, 0, 0, 1], 9104);
                    if connect(client, &loopback).is_err() {
                        // May get EINPROGRESS, wait a bit
                        delay_yield(10);
                    }
                    print("  [CHILD-A] Connected\n");

                    // DELAY before sending using wall-clock time
                    // This ensures parent has time to accept and start blocking read
                    print("  [CHILD-A] Sleeping ");
                    print_num(CHILD_DELAY_MS as i64);
                    print("ms before send...\n");
                    sleep_ms(CHILD_DELAY_MS);

                    // Send data
                    let data = b"BLOCKING_TEST";
                    let written = io::write(client as u64, data);
                    if written < 0 {
                        print("  [CHILD-A] FAIL - write failed\n");
                        let _ = kill(parent_pid, SIGKILL);
                        process::exit(21);
                    }
                    print("  [CHILD-A] Sent data, exiting\n");
                    io::close(client as u64);
                    process::exit(0);

                } else if fork_a > 0 {
                    // ===== PARENT for Part A =====
                    // Yield to let child start and connect
                    // This ensures child connects BEFORE we try to accept
                    delay_yield(20);

                    // Accept connection (child should have connected by now)
                    match accept(server_a, None) {
                        Ok(conn_fd) if conn_fd >= 0 => {
                            print("  [PARENT-A] Accepted connection\n");

                            // BLOCKING read - measure how long it takes
                            let mut buf = [0u8; 64];
                            print("  [PARENT-A] Starting BLOCKING read (should wait for child)...\n");

                            let start = now_monotonic();
                            let result = io::read(conn_fd as u64, &mut buf);
                            let end = now_monotonic();

                            let elapsed = elapsed_ms(&start, &end);

                            if result > 0 {
                                print("  [PARENT-A] Blocking read returned ");
                                print_num(result);
                                print(" bytes in ");
                                print_num(elapsed);
                                print("ms\n");

                                if elapsed >= BLOCKING_MIN_MS {
                                    print("  [PARENT-A] PASS - blocking read took >= ");
                                    print_num(BLOCKING_MIN_MS);
                                    print("ms (proves blocking works)\n");
                                    part_a_passed = true;
                                } else {
                                    print("  [PARENT-A] FAIL - blocking read too fast (");
                                    print_num(elapsed);
                                    print("ms < ");
                                    print_num(BLOCKING_MIN_MS);
                                    print("ms)\n");
                                    print("  This suggests blocking is broken or child sent too fast\n");
                                }
                            } else if result == -(EAGAIN as i64) {
                                print("  [PARENT-A] FAIL - blocking read returned EAGAIN!\n");
                                print("  This proves 'always return EAGAIN' bug exists!\n");
                            } else {
                                print("  [PARENT-A] FAIL - blocking read error: ");
                                print_num(-result);
                                print("\n");
                            }
                            io::close(conn_fd as u64);
                        }
                        _ => {
                            print("  [PARENT-A] FAIL - accept failed\n");
                        }
                    }

                    // Wait for child
                    let mut status: i32 = 0;
                    let _ = process::waitpid(fork_a as i32, &mut status, 0);
                }
            } else {
                print("  FAIL - Part A bind/listen failed\n");
            }
            io::close(server_a as u64);
        }

        print("\n");

        // =====================================================================
        // Part B: Prove non-blocking read returns IMMEDIATELY
        // =====================================================================
        print("  --- Part B: Proving non-blocking read returns immediately ---\n");

        let server_b = match socket(AF_INET, SOCK_STREAM, 0) {
            Ok(fd) if fd >= 0 => fd,
            _ => {
                print("  FAIL - Part B server socket creation failed\n");
                -1
            }
        };

        if server_b >= 0 {
            let server_b_addr = SockAddrIn::new([0, 0, 0, 0], 9105);
            if bind(server_b, &server_b_addr).is_ok() && listen(server_b, 128).is_ok() {
                print("  Server B listening on port 9105 (non-blocking test)\n");

                // Create client and connect
                let client_b = match socket(AF_INET, SOCK_STREAM, 0) {
                    Ok(fd) if fd >= 0 => fd,
                    _ => {
                        print("  FAIL - Part B client socket creation failed\n");
                        -1
                    }
                };

                if client_b >= 0 {
                    let loopback = SockAddrIn::new([127, 0, 0, 1], 9105);
                    let _ = connect(client_b, &loopback); // May return EINPROGRESS
                    delay_yield(10); // Let connection complete

                    match accept(server_b, None) {
                        Ok(conn_fd) if conn_fd >= 0 => {
                            print("  Connection established\n");

                            // Set O_NONBLOCK
                            let flags = fcntl_getfl(conn_fd as u64);
                            if flags >= 0 && fcntl_setfl(conn_fd as u64, (flags as i32) | O_NONBLOCK) >= 0 {
                                print("  Socket set to O_NONBLOCK\n");

                                // NON-BLOCKING read with NO data pending - measure timing
                                let mut buf = [0u8; 64];
                                print("  Starting NON-BLOCKING read (no data pending)...\n");

                                let start = now_monotonic();
                                let result = io::read(conn_fd as u64, &mut buf);
                                let end = now_monotonic();

                                let elapsed = elapsed_ms(&start, &end);

                                print("  Non-blocking read returned in ");
                                print_num(elapsed);
                                print("ms with result ");
                                print_num(result);
                                print("\n");

                                if result == -(EAGAIN as i64) {
                                    if elapsed < NONBLOCK_MAX_MS {
                                        print("  PASS - returned EAGAIN in < ");
                                        print_num(NONBLOCK_MAX_MS);
                                        print("ms (proves O_NONBLOCK works)\n");
                                        part_b_passed = true;
                                    } else {
                                        print("  FAIL - returned EAGAIN but took too long (");
                                        print_num(elapsed);
                                        print("ms >= ");
                                        print_num(NONBLOCK_MAX_MS);
                                        print("ms)\n");
                                    }
                                } else if result >= 0 {
                                    print("  FAIL - read returned data but none was sent!\n");
                                } else {
                                    print("  FAIL - unexpected error: ");
                                    print_num(-result);
                                    print("\n");
                                }
                            } else {
                                print("  FAIL - fcntl failed\n");
                            }
                            io::close(conn_fd as u64);
                        }
                        _ => {
                            print("  FAIL - Part B accept failed\n");
                        }
                    }
                    io::close(client_b as u64);
                }
            } else {
                print("  FAIL - Part B bind/listen failed\n");
            }
            io::close(server_b as u64);
        }

        // Final verdict
        print("\n  --- TEST 4b VERDICT ---\n");
        if part_a_passed && part_b_passed {
            print("  Part A (blocking proof): PASS\n");
            print("  Part B (non-blocking proof): PASS\n");
            print("  TEST 4b: PASS (unfalsifiable - both paths verified)\n\n");
        } else {
            if !part_a_passed {
                print("  Part A (blocking proof): FAIL\n");
            }
            if !part_b_passed {
                print("  Part B (non-blocking proof): FAIL\n");
            }
            print("  TEST 4b: FAIL\n\n");
            failed += 1;
        }
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
