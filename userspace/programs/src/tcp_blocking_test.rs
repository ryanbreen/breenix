//! TCP Blocking I/O Validation Test (std version)
//!
//! This test validates that TCP blocking operations actually BLOCK and do NOT return EAGAIN.
//! Uses fork() to create concurrent processes and expects single blocking calls
//! to succeed without EAGAIN handling.
//!
//! Test structure:
//! 1. Test blocking accept(): Parent blocks on accept(), child connects after delay
//! 2. Test blocking recv(): After accept, parent blocks on read(), child sends after delay
//! 3. Test non-blocking connect() returns EINPROGRESS
//! 4. Test non-blocking accept() returns EAGAIN
//! 4b. Unfalsifiable non-blocking read() test
//! 5. connect() with invalid fd returns EBADF
//! 6. connect() on connected socket returns EISCONN

use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::io::fcntl_cmd::{F_GETFL, F_SETFL};
use libbreenix::io::status_flags::O_NONBLOCK;
use libbreenix::process::{self, ForkResult};
use libbreenix::signal;
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_STREAM};
use libbreenix::time;
use libbreenix::types::{Fd, Timespec};
use libbreenix::Errno;
use std::process as std_process;

/// Yield CPU multiple times to give other process time to run
fn delay_yield(iterations: usize) {
    for _ in 0..iterations {
        let _ = process::yield_now();
        for _ in 0..1000 {
            std::hint::spin_loop();
        }
    }
}

fn sleep_ms(ms: u64) {
    let req = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    let _ = time::nanosleep(&req);
}

fn now_monotonic() -> Timespec {
    time::now_monotonic().unwrap_or(Timespec { tv_sec: 0, tv_nsec: 0 })
}

fn elapsed_ms(start: &Timespec, end: &Timespec) -> i64 {
    let sec_diff = end.tv_sec - start.tv_sec;
    let nsec_diff = end.tv_nsec - start.tv_nsec;
    (sec_diff * 1000) + (nsec_diff / 1_000_000)
}

fn main() {
    println!("TCP_BLOCKING_TEST: Starting");
    println!("TCP_BLOCKING_TEST: This test validates blocking I/O - NO EAGAIN retry loops!\n");

    let mut failed = 0;

    // =========================================================================
    // Test 1: Blocking accept()
    // =========================================================================
    println!("=== TEST 1: Blocking accept() ===");

    let server_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            println!("TCP_BLOCKING_TEST: FAIL - server socket failed");
            std_process::exit(1);
        }
    };
    println!("  Server socket created: fd={}", server_fd.raw());

    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9100);
    if socket::bind_inet(server_fd, &server_addr).is_err() {
        println!("TCP_BLOCKING_TEST: FAIL - bind failed");
        std_process::exit(2);
    }
    println!("  Server bound to port 9100");

    if socket::listen(server_fd, 128).is_err() {
        println!("TCP_BLOCKING_TEST: FAIL - listen failed");
        std_process::exit(3);
    }
    println!("  Server listening");

    let parent_pid = process::getpid().unwrap();
    let fork_result = process::fork();

    match fork_result {
        Err(_) => {
            println!("TCP_BLOCKING_TEST: FAIL - fork failed");
            std_process::exit(4);
        }
        Ok(ForkResult::Child) => {
            // ===== CHILD PROCESS =====
            println!("  [CHILD] Started, will delay then connect...");
            println!("  [CHILD] Parent PID={} (watchdog target)", parent_pid.raw());

            delay_yield(50);

            let client_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
                Ok(fd) => fd,
                Err(_) => {
                    println!("  [CHILD] FAIL - client socket creation failed");
                    println!("  [CHILD] WATCHDOG: Killing parent to prevent hang");
                    let _ = signal::kill(parent_pid.raw() as i32, signal::SIGKILL);
                    std_process::exit(10);
                }
            };

            let loopback = SockAddrIn::new([127, 0, 0, 1], 9100);
            if socket::connect_inet(client_fd, &loopback).is_err() {
                println!("  [CHILD] FAIL - connect failed");
                println!("  [CHILD] WATCHDOG: Killing parent to prevent hang");
                let _ = signal::kill(parent_pid.raw() as i32, signal::SIGKILL);
                std_process::exit(11);
            }
            println!("  [CHILD] Connected to server");

            delay_yield(30);

            // Send test data
            let test_data = b"BLOCKING_TEST_DATA";
            match io::write(client_fd, test_data) {
                Ok(written) => {
                    println!("  [CHILD] Sent {} bytes", written);
                }
                Err(_) => {
                    println!("  [CHILD] FAIL - write failed");
                    println!("  [CHILD] WATCHDOG: Killing parent to prevent hang");
                    let _ = signal::kill(parent_pid.raw() as i32, signal::SIGKILL);
                    std_process::exit(12);
                }
            }

            let _ = io::close(client_fd);
            println!("  [CHILD] Exiting successfully");
            std_process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ===== PARENT PROCESS =====
            println!("  [PARENT] Forked child pid={}", child_pid.raw());
            println!("  [PARENT] Calling accept() - should block until child connects...");

            match socket::accept(server_fd, None) {
                Ok(accepted_fd) => {
                    println!("  [PARENT] accept() returned fd={} - BLOCKING WORKED!", accepted_fd.raw());
                    println!("  TEST 1 (blocking accept): PASS\n");

                    // Test 2: Blocking read()
                    println!("=== TEST 2: Blocking recv() ===");
                    println!("  [PARENT] Calling read() - should block until child sends data...");

                    let mut recv_buf = [0u8; 64];
                    match io::read(accepted_fd, &mut recv_buf) {
                        Err(Error::Os(Errno::EAGAIN)) => {
                            println!("  [PARENT] FAIL - read() returned EAGAIN (-11)");
                            println!("  TEST 2 (blocking recv): FAIL\n");
                            failed += 1;
                        }
                        Err(e) => {
                            println!("  [PARENT] FAIL - read() failed, error={:?}", e);
                            println!("  TEST 2 (blocking recv): FAIL\n");
                            failed += 1;
                        }
                        Ok(0) => {
                            println!("  [PARENT] FAIL - read() returned 0 (EOF) - expected data");
                            println!("  TEST 2 (blocking recv): FAIL\n");
                            failed += 1;
                        }
                        Ok(bytes_read) => {
                            println!("  [PARENT] read() returned {} bytes - BLOCKING WORKED!", bytes_read);

                            let expected = b"BLOCKING_TEST_DATA";
                            if bytes_read == expected.len() && &recv_buf[..bytes_read] == expected {
                                println!("  [PARENT] Data verified correctly");
                                println!("  TEST 2 (blocking recv): PASS\n");
                            } else {
                                println!("  [PARENT] FAIL - data mismatch or wrong byte count");
                                println!("  TEST 2 (blocking recv): FAIL\n");
                                failed += 1;
                            }
                        }
                    }

                    let _ = io::close(accepted_fd);
                }
                Err(Error::Os(Errno::EAGAIN)) => {
                    println!("  [PARENT] FAIL - accept() returned EAGAIN (-11)");
                    println!("  TEST 1 (blocking accept): FAIL\n");
                    failed += 1;
                }
                Err(e) => {
                    println!("  [PARENT] FAIL - accept() failed, error={:?}", e);
                    println!("  TEST 1 (blocking accept): FAIL\n");
                    failed += 1;
                }
            }

            let mut status: i32 = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);
        }
    }

    // =========================================================================
    // Test 3: Non-blocking connect() returns EINPROGRESS
    // =========================================================================
    println!("=== TEST 3: Non-blocking connect() returns EINPROGRESS ===");

    let server2_fd = match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(fd) => fd,
        Err(_) => {
            println!("  FAIL - server2 socket creation failed");
            println!("  TEST 3 (non-blocking connect): FAIL\n");
            failed += 1;
            Fd::from_raw(u64::MAX) // sentinel, won't be used
        }
    };
    if server2_fd.raw() != u64::MAX {
        let server2_addr = SockAddrIn::new([0, 0, 0, 0], 9101);
        if socket::bind_inet(server2_fd, &server2_addr).is_err() || socket::listen(server2_fd, 128).is_err() {
            println!("  FAIL - server2 bind/listen failed");
            println!("  TEST 3 (non-blocking connect): FAIL\n");
            failed += 1;
        } else {
            println!("  Server listening on port 9101");

            match socket::socket(AF_INET, SOCK_STREAM, 0) {
                Ok(client_fd) => {
                    match io::fcntl(client_fd, F_GETFL, 0) {
                        Ok(current_flags) => {
                            match io::fcntl(client_fd, F_SETFL, (current_flags as i32 | O_NONBLOCK) as i64) {
                                Ok(_) => {
                                    println!("  Client socket set to non-blocking mode");

                                    let loopback = SockAddrIn::new([127, 0, 0, 1], 9101);
                                    println!("  Calling connect() in non-blocking mode...");

                                    match socket::connect_inet(client_fd, &loopback) {
                                        Err(Error::Os(Errno::EINPROGRESS)) => {
                                            println!("  connect() returned EINPROGRESS (-115) as expected!");
                                            println!("  TEST 3 (non-blocking connect): PASS\n");
                                        }
                                        Ok(()) => {
                                            println!("  FAIL - connect() returned success (0) instead of EINPROGRESS");
                                            println!("  TEST 3 (non-blocking connect): FAIL\n");
                                            failed += 1;
                                        }
                                        Err(e) => {
                                            println!("  FAIL - connect() returned unexpected error, error={:?}", e);
                                            println!("  TEST 3 (non-blocking connect): FAIL\n");
                                            failed += 1;
                                        }
                                    }
                                }
                                Err(_) => {
                                    println!("  FAIL - fcntl(F_SETFL, O_NONBLOCK) failed");
                                    println!("  TEST 3 (non-blocking connect): FAIL\n");
                                    failed += 1;
                                }
                            }
                        }
                        Err(_) => {
                            println!("  FAIL - fcntl(F_GETFL) failed");
                            println!("  TEST 3 (non-blocking connect): FAIL\n");
                            failed += 1;
                        }
                    }
                    let _ = io::close(client_fd);
                }
                Err(_) => {
                    println!("  FAIL - client socket creation failed");
                    println!("  TEST 3 (non-blocking connect): FAIL\n");
                    failed += 1;
                }
            }
        }
        let _ = io::close(server2_fd);
    }

    // =========================================================================
    // Test 4: Non-blocking accept() returns EAGAIN
    // =========================================================================
    println!("=== TEST 4: Non-blocking accept() returns EAGAIN ===");

    match socket::socket(AF_INET, SOCK_STREAM, 0) {
        Ok(server3_fd) => {
            let fcntl_ok = match io::fcntl(server3_fd, F_GETFL, 0) {
                Ok(current_flags) => {
                    io::fcntl(server3_fd, F_SETFL, (current_flags as i32 | O_NONBLOCK) as i64).is_ok()
                }
                Err(_) => false,
            };
            if fcntl_ok {
                println!("  Socket set to non-blocking mode (O_NONBLOCK)");

                let server3_addr = SockAddrIn::new([0, 0, 0, 0], 9103);
                if socket::bind_inet(server3_fd, &server3_addr).is_ok()
                    && socket::listen(server3_fd, 128).is_ok()
                {
                    println!("  Server listening");
                    println!("  Calling accept() with NO pending connections...");

                    match socket::accept(server3_fd, None) {
                        Err(Error::Os(Errno::EAGAIN)) => {
                            println!("  accept() returned EAGAIN (-11) as expected!");
                            println!("  TEST 4 (non-blocking EAGAIN): PASS\n");
                        }
                        Ok(accepted_fd) => {
                            println!("  FAIL - accept() returned fd={} but no client connected!", accepted_fd.raw());
                            println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
                            failed += 1;
                            let _ = io::close(accepted_fd);
                        }
                        Err(e) => {
                            println!("  FAIL - accept() returned unexpected error, error={:?}", e);
                            println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
                            failed += 1;
                        }
                    }
                } else {
                    println!("  FAIL - bind/listen failed");
                    println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
                    failed += 1;
                }
            } else {
                println!("  FAIL - fcntl failed");
                println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
                failed += 1;
            }
            let _ = io::close(server3_fd);
        }
        Err(_) => {
            println!("  FAIL - server3 socket creation failed");
            println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 4b: Non-blocking read() returns EAGAIN IMMEDIATELY (Unfalsifiable)
    // =========================================================================
    println!("=== TEST 4b: Non-blocking read() returns EAGAIN IMMEDIATELY (Unfalsifiable) ===");
    println!("  This test proves O_NONBLOCK works by comparing blocking vs non-blocking timing.\n");

    let child_delay_ms: u64 = 300;
    let blocking_min_ms: i64 = 100;
    let nonblock_max_ms: i64 = 100;

    let mut part_a_passed = false;
    let mut part_b_passed = false;

    // Part A: Prove blocking read actually blocks
    println!("  --- Part A: Proving blocking read actually blocks ---");

    if let Ok(server_a) = socket::socket(AF_INET, SOCK_STREAM, 0) {
        let server_a_addr = SockAddrIn::new([0, 0, 0, 0], 9104);
        if socket::bind_inet(server_a, &server_a_addr).is_ok() && socket::listen(server_a, 128).is_ok() {
            println!("  Server A listening on port 9104 (blocking test)");

            let parent_pid = process::getpid().unwrap();
            let fork_a = process::fork();

            match fork_a {
                Ok(ForkResult::Child) => {
                    // Child for Part A
                    println!("  [CHILD-A] Started, will delay then send data");

                    let client = match socket::socket(AF_INET, SOCK_STREAM, 0) {
                        Ok(fd) => fd,
                        Err(_) => {
                            println!("  [CHILD-A] FAIL - socket creation failed");
                            let _ = signal::kill(parent_pid.raw() as i32, signal::SIGKILL);
                            std_process::exit(20);
                        }
                    };

                    let loopback = SockAddrIn::new([127, 0, 0, 1], 9104);
                    let _ = socket::connect_inet(client, &loopback);
                    delay_yield(10);
                    println!("  [CHILD-A] Connected");

                    println!("  [CHILD-A] Sleeping {}ms before send...", child_delay_ms);
                    sleep_ms(child_delay_ms);

                    let data = b"BLOCKING_TEST";
                    match io::write(client, data) {
                        Ok(_) => {}
                        Err(_) => {
                            println!("  [CHILD-A] FAIL - write failed");
                            let _ = signal::kill(parent_pid.raw() as i32, signal::SIGKILL);
                            std_process::exit(21);
                        }
                    }
                    println!("  [CHILD-A] Sent data, exiting");
                    let _ = io::close(client);
                    std_process::exit(0);
                }
                Ok(ForkResult::Parent(child_a_pid)) => {
                    // Parent for Part A
                    delay_yield(20);

                    if let Ok(conn_fd) = socket::accept(server_a, None) {
                        println!("  [PARENT-A] Accepted connection");

                        let mut buf = [0u8; 64];
                        println!("  [PARENT-A] Starting BLOCKING read (should wait for child)...");

                        let start = now_monotonic();
                        let result = io::read(conn_fd, &mut buf);
                        let end = now_monotonic();

                        let elapsed = elapsed_ms(&start, &end);

                        match result {
                            Ok(n) if n > 0 => {
                                println!("  [PARENT-A] Blocking read returned {} bytes in {}ms", n, elapsed);
                                if elapsed >= blocking_min_ms {
                                    println!("  [PARENT-A] PASS - blocking read took >= {}ms", blocking_min_ms);
                                    part_a_passed = true;
                                } else {
                                    println!(
                                        "  [PARENT-A] FAIL - blocking read too fast ({}ms < {}ms)",
                                        elapsed, blocking_min_ms
                                    );
                                }
                            }
                            Err(Error::Os(Errno::EAGAIN)) => {
                                println!("  [PARENT-A] FAIL - blocking read returned EAGAIN!");
                            }
                            Ok(_) => {
                                println!("  [PARENT-A] FAIL - blocking read returned 0 bytes");
                            }
                            Err(e) => {
                                println!("  [PARENT-A] FAIL - blocking read error: {:?}", e);
                            }
                        }
                        let _ = io::close(conn_fd);
                    } else {
                        println!("  [PARENT-A] FAIL - accept failed");
                    }

                    let mut status: i32 = 0;
                    let _ = process::waitpid(child_a_pid.raw() as i32, &mut status, 0);
                }
                Err(_) => {
                    println!("  FAIL - Part A fork failed");
                }
            }
        } else {
            println!("  FAIL - Part A bind/listen failed");
        }
        let _ = io::close(server_a);
    }

    println!();

    // Part B: Prove non-blocking read returns IMMEDIATELY
    println!("  --- Part B: Proving non-blocking read returns immediately ---");

    if let Ok(server_b) = socket::socket(AF_INET, SOCK_STREAM, 0) {
        let server_b_addr = SockAddrIn::new([0, 0, 0, 0], 9105);
        if socket::bind_inet(server_b, &server_b_addr).is_ok() && socket::listen(server_b, 128).is_ok() {
            println!("  Server B listening on port 9105 (non-blocking test)");

            if let Ok(client_b) = socket::socket(AF_INET, SOCK_STREAM, 0) {
                let loopback = SockAddrIn::new([127, 0, 0, 1], 9105);
                let _ = socket::connect_inet(client_b, &loopback);
                delay_yield(10);

                if let Ok(conn_fd) = socket::accept(server_b, None) {
                    println!("  Connection established");

                    let flags_ok = match io::fcntl(conn_fd, F_GETFL, 0) {
                        Ok(flags) => {
                            io::fcntl(conn_fd, F_SETFL, (flags as i32 | O_NONBLOCK) as i64).is_ok()
                        }
                        Err(_) => false,
                    };
                    if flags_ok {
                        println!("  Socket set to O_NONBLOCK");

                        let mut buf = [0u8; 64];
                        println!("  Starting NON-BLOCKING read (no data pending)...");

                        let start = now_monotonic();
                        let result = io::read(conn_fd, &mut buf);
                        let end = now_monotonic();

                        let elapsed = elapsed_ms(&start, &end);

                        match result {
                            Err(Error::Os(Errno::EAGAIN)) => {
                                println!("  Non-blocking read returned in {}ms with EAGAIN", elapsed);
                                if elapsed < nonblock_max_ms {
                                    println!("  PASS - returned EAGAIN in < {}ms", nonblock_max_ms);
                                    part_b_passed = true;
                                } else {
                                    println!(
                                        "  FAIL - returned EAGAIN but took too long ({}ms >= {}ms)",
                                        elapsed, nonblock_max_ms
                                    );
                                }
                            }
                            Ok(n) => {
                                println!(
                                    "  Non-blocking read returned in {}ms with result {}",
                                    elapsed, n
                                );
                                println!("  FAIL - read returned data but none was sent!");
                            }
                            Err(e) => {
                                println!("  Non-blocking read returned in {}ms with error {:?}", elapsed, e);
                                println!("  FAIL - unexpected error");
                            }
                        }
                    } else {
                        println!("  FAIL - fcntl failed");
                    }
                    let _ = io::close(conn_fd);
                } else {
                    println!("  FAIL - Part B accept failed");
                }
                let _ = io::close(client_b);
            } else {
                println!("  FAIL - Part B client socket creation failed");
            }
        } else {
            println!("  FAIL - Part B bind/listen failed");
        }
        let _ = io::close(server_b);
    }

    // Final verdict for Test 4b
    println!("\n  --- TEST 4b VERDICT ---");
    if part_a_passed && part_b_passed {
        println!("  Part A (blocking proof): PASS");
        println!("  Part B (non-blocking proof): PASS");
        println!("  TEST 4b: PASS (unfalsifiable - both paths verified)\n");
    } else {
        if !part_a_passed {
            println!("  Part A (blocking proof): FAIL");
        }
        if !part_b_passed {
            println!("  Part B (non-blocking proof): FAIL");
        }
        println!("  TEST 4b: FAIL\n");
        failed += 1;
    }

    // =========================================================================
    // Test 5: connect() with invalid fd returns EBADF
    // =========================================================================
    println!("=== TEST 5: connect() with invalid fd returns EBADF ===");
    {
        let addr = SockAddrIn::new([127, 0, 0, 1], 9999);
        println!("  Calling connect() with invalid fd=9999...");

        match socket::connect_inet(Fd::from_raw(9999), &addr) {
            Err(Error::Os(Errno::EBADF)) => {
                println!("  connect() returned EBADF (-9) as expected!");
                println!("  TEST 5 (EBADF): PASS\n");
            }
            Ok(()) => {
                println!("  FAIL - connect() succeeded with invalid fd!");
                println!("  TEST 5 (EBADF): FAIL\n");
                failed += 1;
            }
            Err(e) => {
                println!("  FAIL - connect() returned unexpected error, error={:?}", e);
                println!("  TEST 5 (EBADF): FAIL\n");
                failed += 1;
            }
        }
    }

    // =========================================================================
    // Test 6: connect() on connected socket returns EISCONN
    // =========================================================================
    println!("=== TEST 6: connect() on connected socket returns EISCONN ===");
    {
        match socket::socket(AF_INET, SOCK_STREAM, 0) {
            Ok(server_fd) => {
                let server_addr = SockAddrIn::new([0, 0, 0, 0], 9106);
                if socket::bind_inet(server_fd, &server_addr).is_ok()
                    && socket::listen(server_fd, 128).is_ok()
                {
                    match socket::socket(AF_INET, SOCK_STREAM, 0) {
                        Ok(client_fd) => {
                            let loopback = SockAddrIn::new([127, 0, 0, 1], 9106);

                            println!("  First connect() call...");
                            match socket::connect_inet(client_fd, &loopback) {
                                Ok(()) => {
                                    println!("  First connect() succeeded");
                                    println!("  Second connect() call on same socket...");
                                    match socket::connect_inet(client_fd, &loopback) {
                                        Err(Error::Os(Errno::EISCONN)) => {
                                            println!(
                                                "  connect() returned EISCONN (-106) as expected!"
                                            );
                                            println!("  TEST 6 (EISCONN): PASS\n");
                                        }
                                        Ok(()) => {
                                            println!("  FAIL - second connect() succeeded!");
                                            println!("  TEST 6 (EISCONN): FAIL\n");
                                            failed += 1;
                                        }
                                        Err(e) => {
                                            println!(
                                                "  FAIL - connect() returned unexpected error, error={:?}",
                                                e
                                            );
                                            println!("  TEST 6 (EISCONN): FAIL\n");
                                            failed += 1;
                                        }
                                    }
                                }
                                Err(Error::Os(Errno::EINPROGRESS)) => {
                                    // EINPROGRESS
                                    println!("  First connect() returned EINPROGRESS, waiting...");
                                    delay_yield(50);
                                    println!("  Second connect() call on same socket...");
                                    match socket::connect_inet(client_fd, &loopback) {
                                        Err(Error::Os(Errno::EISCONN)) => {
                                            println!(
                                                "  connect() returned EISCONN (-106) as expected!"
                                            );
                                            println!("  TEST 6 (EISCONN): PASS\n");
                                        }
                                        _ => {
                                            println!("  FAIL - second connect() did not return EISCONN");
                                            println!("  TEST 6 (EISCONN): FAIL\n");
                                            failed += 1;
                                        }
                                    }
                                }
                                Err(e) => {
                                    println!(
                                        "  FAIL - first connect() failed, error={:?}",
                                        e
                                    );
                                    println!("  TEST 6 (EISCONN): FAIL\n");
                                    failed += 1;
                                }
                            }
                            let _ = io::close(client_fd);
                        }
                        Err(_) => {
                            println!("  FAIL - client socket creation failed");
                            println!("  TEST 6 (EISCONN): FAIL\n");
                            failed += 1;
                        }
                    }
                } else {
                    println!("  FAIL - server bind/listen failed");
                    println!("  TEST 6 (EISCONN): FAIL\n");
                    failed += 1;
                }
                let _ = io::close(server_fd);
            }
            Err(_) => {
                println!("  FAIL - server socket creation failed");
                println!("  TEST 6 (EISCONN): FAIL\n");
                failed += 1;
            }
        }
    }

    // Final results
    println!("=== FINAL RESULTS ===");
    if failed == 0 {
        println!("TCP_BLOCKING_TEST: ALL TESTS PASSED");
        println!("TCP_BLOCKING_TEST: PASS");
        std_process::exit(0);
    } else {
        println!("TCP_BLOCKING_TEST: {} tests FAILED", failed);
        println!("TCP_BLOCKING_TEST: FAIL");
        std_process::exit(1);
    }
}
