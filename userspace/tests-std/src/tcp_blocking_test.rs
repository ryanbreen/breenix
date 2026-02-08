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

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const O_NONBLOCK: i32 = 2048;
const EAGAIN: i32 = 11;
const SIGKILL: i32 = 9;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const CLOCK_MONOTONIC: i32 = 1;

extern "C" {
    fn socket(domain: i32, typ: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
    fn listen(fd: i32, backlog: i32) -> i32;
    fn accept(fd: i32, addr: *mut SockAddrIn, addrlen: *mut u32) -> i32;
    fn connect(fd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;

    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn fcntl(fd: i32, cmd: i32, arg: i64) -> i32;
    fn fork() -> i32;
    fn getpid() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn sched_yield() -> i32;
    fn nanosleep(req: *const Timespec, rem: *mut Timespec) -> i32;
    fn clock_gettime(clockid: i32, tp: *mut Timespec) -> i32;
}

fn sock_size() -> u32 {
    std::mem::size_of::<SockAddrIn>() as u32
}

/// Yield CPU multiple times to give other process time to run
fn delay_yield(iterations: usize) {
    for _ in 0..iterations {
        unsafe { sched_yield(); }
        for _ in 0..1000 { std::hint::spin_loop(); }
    }
}

fn sleep_ms(ms: u64) {
    let req = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    unsafe { nanosleep(&req, std::ptr::null_mut()); }
}

fn now_monotonic() -> Timespec {
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts); }
    ts
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

    let server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server_fd < 0 { println!("TCP_BLOCKING_TEST: FAIL - server socket failed"); process::exit(1); }
    println!("  Server socket created: fd={}", server_fd);

    let server_addr = SockAddrIn::new([0, 0, 0, 0], 9100);
    if unsafe { bind(server_fd, &server_addr, sock_size()) } != 0 { println!("TCP_BLOCKING_TEST: FAIL - bind failed"); process::exit(2); }
    println!("  Server bound to port 9100");

    if unsafe { listen(server_fd, 128) } != 0 { println!("TCP_BLOCKING_TEST: FAIL - listen failed"); process::exit(3); }
    println!("  Server listening");

    let parent_pid = unsafe { getpid() };
    let fork_result = unsafe { fork() };

    if fork_result < 0 { println!("TCP_BLOCKING_TEST: FAIL - fork failed"); process::exit(4); }

    if fork_result == 0 {
        // ===== CHILD PROCESS =====
        println!("  [CHILD] Started, will delay then connect...");
        println!("  [CHILD] Parent PID={} (watchdog target)", parent_pid);

        delay_yield(50);

        let client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
        if client_fd < 0 {
            println!("  [CHILD] FAIL - client socket creation failed");
            println!("  [CHILD] WATCHDOG: Killing parent to prevent hang");
            unsafe { kill(parent_pid, SIGKILL); }
            process::exit(10);
        }

        let loopback = SockAddrIn::new([127, 0, 0, 1], 9100);
        if unsafe { connect(client_fd, &loopback, sock_size()) } != 0 {
            println!("  [CHILD] FAIL - connect failed");
            println!("  [CHILD] WATCHDOG: Killing parent to prevent hang");
            unsafe { kill(parent_pid, SIGKILL); }
            process::exit(11);
        }
        println!("  [CHILD] Connected to server");

        delay_yield(30);

        // Send test data
        let test_data = b"BLOCKING_TEST_DATA";
        let written = unsafe { write(client_fd, test_data.as_ptr(), test_data.len()) };
        if written < 0 {
            println!("  [CHILD] FAIL - write failed");
            println!("  [CHILD] WATCHDOG: Killing parent to prevent hang");
            unsafe { kill(parent_pid, SIGKILL); }
            process::exit(12);
        }
        println!("  [CHILD] Sent {} bytes", written);

        unsafe { close(client_fd); }
        println!("  [CHILD] Exiting successfully");
        process::exit(0);
    } else {
        // ===== PARENT PROCESS =====
        println!("  [PARENT] Forked child pid={}", fork_result);
        println!("  [PARENT] Calling accept() - should block until child connects...");

        let accepted_fd = unsafe { accept(server_fd, std::ptr::null_mut(), std::ptr::null_mut()) };

        if accepted_fd >= 0 {
            println!("  [PARENT] accept() returned fd={} - BLOCKING WORKED!", accepted_fd);
            println!("  TEST 1 (blocking accept): PASS\n");

            // Test 2: Blocking read()
            println!("=== TEST 2: Blocking recv() ===");
            println!("  [PARENT] Calling read() - should block until child sends data...");

            let mut recv_buf = [0u8; 64];
            let bytes_read = unsafe { read(accepted_fd, recv_buf.as_mut_ptr(), recv_buf.len()) };

            if bytes_read == -(EAGAIN as isize) {
                println!("  [PARENT] FAIL - read() returned EAGAIN (-11)");
                println!("  TEST 2 (blocking recv): FAIL\n");
                failed += 1;
            } else if bytes_read < 0 {
                println!("  [PARENT] FAIL - read() failed, errno={}", -bytes_read);
                println!("  TEST 2 (blocking recv): FAIL\n");
                failed += 1;
            } else if bytes_read == 0 {
                println!("  [PARENT] FAIL - read() returned 0 (EOF) - expected data");
                println!("  TEST 2 (blocking recv): FAIL\n");
                failed += 1;
            } else {
                println!("  [PARENT] read() returned {} bytes - BLOCKING WORKED!", bytes_read);

                let expected = b"BLOCKING_TEST_DATA";
                if bytes_read as usize == expected.len() && &recv_buf[..bytes_read as usize] == expected {
                    println!("  [PARENT] Data verified correctly");
                    println!("  TEST 2 (blocking recv): PASS\n");
                } else {
                    println!("  [PARENT] FAIL - data mismatch or wrong byte count");
                    println!("  TEST 2 (blocking recv): FAIL\n");
                    failed += 1;
                }
            }

            unsafe { close(accepted_fd); }
        } else if -accepted_fd == EAGAIN {
            println!("  [PARENT] FAIL - accept() returned EAGAIN (-11)");
            println!("  TEST 1 (blocking accept): FAIL\n");
            failed += 1;
        } else {
            println!("  [PARENT] FAIL - accept() failed, errno={}", -accepted_fd);
            println!("  TEST 1 (blocking accept): FAIL\n");
            failed += 1;
        }

        let mut status: i32 = 0;
        unsafe { waitpid(fork_result, &mut status, 0); }
    }

    // =========================================================================
    // Test 3: Non-blocking connect() returns EINPROGRESS
    // =========================================================================
    println!("=== TEST 3: Non-blocking connect() returns EINPROGRESS ===");

    let server2_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server2_fd < 0 {
        println!("  FAIL - server2 socket creation failed");
        println!("  TEST 3 (non-blocking connect): FAIL\n");
        failed += 1;
    } else {
        let server2_addr = SockAddrIn::new([0, 0, 0, 0], 9101);
        if unsafe { bind(server2_fd, &server2_addr, sock_size()) } != 0 || unsafe { listen(server2_fd, 128) } != 0 {
            println!("  FAIL - server2 bind/listen failed");
            println!("  TEST 3 (non-blocking connect): FAIL\n");
            failed += 1;
        } else {
            println!("  Server listening on port 9101");

            let client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
            if client_fd >= 0 {
                let current_flags = unsafe { fcntl(client_fd, F_GETFL, 0) };
                if current_flags >= 0 {
                    let set_result = unsafe { fcntl(client_fd, F_SETFL, (current_flags | O_NONBLOCK) as i64) };
                    if set_result >= 0 {
                        println!("  Client socket set to non-blocking mode");

                        let loopback = SockAddrIn::new([127, 0, 0, 1], 9101);
                        println!("  Calling connect() in non-blocking mode...");

                        let ret = unsafe { connect(client_fd, &loopback, sock_size()) };
                        if -ret == 115 {
                            println!("  connect() returned EINPROGRESS (-115) as expected!");
                            println!("  TEST 3 (non-blocking connect): PASS\n");
                        } else if ret == 0 {
                            println!("  FAIL - connect() returned success (0) instead of EINPROGRESS");
                            println!("  TEST 3 (non-blocking connect): FAIL\n");
                            failed += 1;
                        } else {
                            println!("  FAIL - connect() returned unexpected error, errno={}", -ret);
                            println!("  TEST 3 (non-blocking connect): FAIL\n");
                            failed += 1;
                        }
                    } else {
                        println!("  FAIL - fcntl(F_SETFL, O_NONBLOCK) failed");
                        println!("  TEST 3 (non-blocking connect): FAIL\n");
                        failed += 1;
                    }
                } else {
                    println!("  FAIL - fcntl(F_GETFL) failed");
                    println!("  TEST 3 (non-blocking connect): FAIL\n");
                    failed += 1;
                }
                unsafe { close(client_fd); }
            } else {
                println!("  FAIL - client socket creation failed");
                println!("  TEST 3 (non-blocking connect): FAIL\n");
                failed += 1;
            }
        }
        unsafe { close(server2_fd); }
    }

    // =========================================================================
    // Test 4: Non-blocking accept() returns EAGAIN
    // =========================================================================
    println!("=== TEST 4: Non-blocking accept() returns EAGAIN ===");

    let server3_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server3_fd >= 0 {
        let current_flags = unsafe { fcntl(server3_fd, F_GETFL, 0) };
        if current_flags >= 0 && unsafe { fcntl(server3_fd, F_SETFL, (current_flags | O_NONBLOCK) as i64) } >= 0 {
            println!("  Socket set to non-blocking mode (O_NONBLOCK)");

            let server3_addr = SockAddrIn::new([0, 0, 0, 0], 9103);
            if unsafe { bind(server3_fd, &server3_addr, sock_size()) } == 0 &&
               unsafe { listen(server3_fd, 128) } == 0 {
                println!("  Server listening");
                println!("  Calling accept() with NO pending connections...");

                let accept_result = unsafe { accept(server3_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
                if -accept_result == EAGAIN {
                    println!("  accept() returned EAGAIN (-11) as expected!");
                    println!("  TEST 4 (non-blocking EAGAIN): PASS\n");
                } else if accept_result >= 0 {
                    println!("  FAIL - accept() returned fd={} but no client connected!", accept_result);
                    println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
                    failed += 1;
                    unsafe { close(accept_result); }
                } else {
                    println!("  FAIL - accept() returned unexpected error, errno={}", -accept_result);
                    println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
                    failed += 1;
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
        unsafe { close(server3_fd); }
    } else {
        println!("  FAIL - server3 socket creation failed");
        println!("  TEST 4 (non-blocking EAGAIN): FAIL\n");
        failed += 1;
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

    let server_a = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server_a >= 0 {
        let server_a_addr = SockAddrIn::new([0, 0, 0, 0], 9104);
        if unsafe { bind(server_a, &server_a_addr, sock_size()) } == 0 && unsafe { listen(server_a, 128) } == 0 {
            println!("  Server A listening on port 9104 (blocking test)");

            let parent_pid = unsafe { getpid() };
            let fork_a = unsafe { fork() };

            if fork_a == 0 {
                // Child for Part A
                println!("  [CHILD-A] Started, will delay then send data");

                let client = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
                if client < 0 {
                    println!("  [CHILD-A] FAIL - socket creation failed");
                    unsafe { kill(parent_pid, SIGKILL); }
                    process::exit(20);
                }

                let loopback = SockAddrIn::new([127, 0, 0, 1], 9104);
                let _ = unsafe { connect(client, &loopback, sock_size()) };
                delay_yield(10);
                println!("  [CHILD-A] Connected");

                println!("  [CHILD-A] Sleeping {}ms before send...", child_delay_ms);
                sleep_ms(child_delay_ms);

                let data = b"BLOCKING_TEST";
                let written = unsafe { write(client, data.as_ptr(), data.len()) };
                if written < 0 {
                    println!("  [CHILD-A] FAIL - write failed");
                    unsafe { kill(parent_pid, SIGKILL); }
                    process::exit(21);
                }
                println!("  [CHILD-A] Sent data, exiting");
                unsafe { close(client); }
                process::exit(0);
            } else if fork_a > 0 {
                // Parent for Part A
                delay_yield(20);

                let conn_fd = unsafe { accept(server_a, std::ptr::null_mut(), std::ptr::null_mut()) };
                if conn_fd >= 0 {
                    println!("  [PARENT-A] Accepted connection");

                    let mut buf = [0u8; 64];
                    println!("  [PARENT-A] Starting BLOCKING read (should wait for child)...");

                    let start = now_monotonic();
                    let result = unsafe { read(conn_fd, buf.as_mut_ptr(), buf.len()) };
                    let end = now_monotonic();

                    let elapsed = elapsed_ms(&start, &end);

                    if result > 0 {
                        println!("  [PARENT-A] Blocking read returned {} bytes in {}ms", result, elapsed);
                        if elapsed >= blocking_min_ms {
                            println!("  [PARENT-A] PASS - blocking read took >= {}ms", blocking_min_ms);
                            part_a_passed = true;
                        } else {
                            println!("  [PARENT-A] FAIL - blocking read too fast ({}ms < {}ms)", elapsed, blocking_min_ms);
                        }
                    } else if result == -(EAGAIN as isize) {
                        println!("  [PARENT-A] FAIL - blocking read returned EAGAIN!");
                    } else {
                        println!("  [PARENT-A] FAIL - blocking read error: {}", -result);
                    }
                    unsafe { close(conn_fd); }
                } else {
                    println!("  [PARENT-A] FAIL - accept failed");
                }

                let mut status: i32 = 0;
                unsafe { waitpid(fork_a, &mut status, 0); }
            }
        } else {
            println!("  FAIL - Part A bind/listen failed");
        }
        unsafe { close(server_a); }
    }

    println!();

    // Part B: Prove non-blocking read returns IMMEDIATELY
    println!("  --- Part B: Proving non-blocking read returns immediately ---");

    let server_b = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if server_b >= 0 {
        let server_b_addr = SockAddrIn::new([0, 0, 0, 0], 9105);
        if unsafe { bind(server_b, &server_b_addr, sock_size()) } == 0 && unsafe { listen(server_b, 128) } == 0 {
            println!("  Server B listening on port 9105 (non-blocking test)");

            let client_b = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
            if client_b >= 0 {
                let loopback = SockAddrIn::new([127, 0, 0, 1], 9105);
                let _ = unsafe { connect(client_b, &loopback, sock_size()) };
                delay_yield(10);

                let conn_fd = unsafe { accept(server_b, std::ptr::null_mut(), std::ptr::null_mut()) };
                if conn_fd >= 0 {
                    println!("  Connection established");

                    let flags = unsafe { fcntl(conn_fd, F_GETFL, 0) };
                    if flags >= 0 && unsafe { fcntl(conn_fd, F_SETFL, (flags | O_NONBLOCK) as i64) } >= 0 {
                        println!("  Socket set to O_NONBLOCK");

                        let mut buf = [0u8; 64];
                        println!("  Starting NON-BLOCKING read (no data pending)...");

                        let start = now_monotonic();
                        let result = unsafe { read(conn_fd, buf.as_mut_ptr(), buf.len()) };
                        let end = now_monotonic();

                        let elapsed = elapsed_ms(&start, &end);

                        println!("  Non-blocking read returned in {}ms with result {}", elapsed, result);

                        if result == -(EAGAIN as isize) {
                            if elapsed < nonblock_max_ms {
                                println!("  PASS - returned EAGAIN in < {}ms", nonblock_max_ms);
                                part_b_passed = true;
                            } else {
                                println!("  FAIL - returned EAGAIN but took too long ({}ms >= {}ms)", elapsed, nonblock_max_ms);
                            }
                        } else if result >= 0 {
                            println!("  FAIL - read returned data but none was sent!");
                        } else {
                            println!("  FAIL - unexpected error: {}", -result);
                        }
                    } else {
                        println!("  FAIL - fcntl failed");
                    }
                    unsafe { close(conn_fd); }
                } else {
                    println!("  FAIL - Part B accept failed");
                }
                unsafe { close(client_b); }
            } else {
                println!("  FAIL - Part B client socket creation failed");
            }
        } else {
            println!("  FAIL - Part B bind/listen failed");
        }
        unsafe { close(server_b); }
    }

    // Final verdict for Test 4b
    println!("\n  --- TEST 4b VERDICT ---");
    if part_a_passed && part_b_passed {
        println!("  Part A (blocking proof): PASS");
        println!("  Part B (non-blocking proof): PASS");
        println!("  TEST 4b: PASS (unfalsifiable - both paths verified)\n");
    } else {
        if !part_a_passed { println!("  Part A (blocking proof): FAIL"); }
        if !part_b_passed { println!("  Part B (non-blocking proof): FAIL"); }
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

        let ret = unsafe { connect(9999, &addr, sock_size()) };
        if -ret == 9 {
            println!("  connect() returned EBADF (-9) as expected!");
            println!("  TEST 5 (EBADF): PASS\n");
        } else if ret == 0 {
            println!("  FAIL - connect() succeeded with invalid fd!");
            println!("  TEST 5 (EBADF): FAIL\n");
            failed += 1;
        } else {
            println!("  FAIL - connect() returned unexpected error, errno={}", -ret);
            println!("  TEST 5 (EBADF): FAIL\n");
            failed += 1;
        }
    }

    // =========================================================================
    // Test 6: connect() on connected socket returns EISCONN
    // =========================================================================
    println!("=== TEST 6: connect() on connected socket returns EISCONN ===");
    {
        let server_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
        if server_fd >= 0 {
            let server_addr = SockAddrIn::new([0, 0, 0, 0], 9106);
            if unsafe { bind(server_fd, &server_addr, sock_size()) } == 0 && unsafe { listen(server_fd, 128) } == 0 {
                let client_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
                if client_fd >= 0 {
                    let loopback = SockAddrIn::new([127, 0, 0, 1], 9106);

                    println!("  First connect() call...");
                    let ret = unsafe { connect(client_fd, &loopback, sock_size()) };
                    if ret == 0 {
                        println!("  First connect() succeeded");
                        println!("  Second connect() call on same socket...");
                        let ret2 = unsafe { connect(client_fd, &loopback, sock_size()) };
                        if -ret2 == 106 {
                            println!("  connect() returned EISCONN (-106) as expected!");
                            println!("  TEST 6 (EISCONN): PASS\n");
                        } else if ret2 == 0 {
                            println!("  FAIL - second connect() succeeded!");
                            println!("  TEST 6 (EISCONN): FAIL\n");
                            failed += 1;
                        } else {
                            println!("  FAIL - connect() returned unexpected error, errno={}", -ret2);
                            println!("  TEST 6 (EISCONN): FAIL\n");
                            failed += 1;
                        }
                    } else if -ret == 115 {
                        // EINPROGRESS
                        println!("  First connect() returned EINPROGRESS, waiting...");
                        delay_yield(50);
                        println!("  Second connect() call on same socket...");
                        let ret2 = unsafe { connect(client_fd, &loopback, sock_size()) };
                        if -ret2 == 106 {
                            println!("  connect() returned EISCONN (-106) as expected!");
                            println!("  TEST 6 (EISCONN): PASS\n");
                        } else {
                            println!("  FAIL - second connect() returned errno={}", -ret2);
                            println!("  TEST 6 (EISCONN): FAIL\n");
                            failed += 1;
                        }
                    } else {
                        println!("  FAIL - first connect() failed, errno={}", -ret);
                        println!("  TEST 6 (EISCONN): FAIL\n");
                        failed += 1;
                    }
                    unsafe { close(client_fd); }
                } else {
                    println!("  FAIL - client socket creation failed");
                    println!("  TEST 6 (EISCONN): FAIL\n");
                    failed += 1;
                }
            } else {
                println!("  FAIL - server bind/listen failed");
                println!("  TEST 6 (EISCONN): FAIL\n");
                failed += 1;
            }
            unsafe { close(server_fd); }
        } else {
            println!("  FAIL - server socket creation failed");
            println!("  TEST 6 (EISCONN): FAIL\n");
            failed += 1;
        }
    }

    // Final results
    println!("=== FINAL RESULTS ===");
    if failed == 0 {
        println!("TCP_BLOCKING_TEST: ALL TESTS PASSED");
        println!("TCP_BLOCKING_TEST: PASS");
        process::exit(0);
    } else {
        println!("TCP_BLOCKING_TEST: {} tests FAILED", failed);
        println!("TCP_BLOCKING_TEST: FAIL");
        process::exit(1);
    }
}
