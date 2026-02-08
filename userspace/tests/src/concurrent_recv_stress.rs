//! Concurrent blocking recvfrom() stress test (std version)
//!
//! Tests multiple processes blocking on recvfrom() simultaneously to verify:
//! 1. Multiple processes can block on different sockets concurrently
//! 2. All processes wake correctly when packets arrive
//! 3. No deadlocks or race conditions under concurrent load
//!
//! Test scenario:
//! - Parent forks N child processes
//! - Each child binds to a unique port and calls blocking recvfrom()
//! - Parent sends packets to each child's port
//! - Each child verifies received data and exits with status 0
//! - Parent waits for all children and verifies all succeeded

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
const SOCK_DGRAM: i32 = 2;

/// Number of concurrent child processes to spawn
const NUM_CHILDREN: usize = 4;

/// Base port number (children use BASE_PORT + child_index)
const BASE_PORT: u16 = 56000;

extern "C" {
    fn socket(domain: i32, typ: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const SockAddrIn, addrlen: u32) -> i32;
    fn sendto(fd: i32, buf: *const u8, len: usize, flags: i32,
              dest_addr: *const SockAddrIn, addrlen: u32) -> isize;
    fn recvfrom(fd: i32, buf: *mut u8, len: usize, flags: i32,
                src_addr: *mut SockAddrIn, addrlen: *mut u32) -> isize;
    fn close(fd: i32) -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn sched_yield() -> i32;
}

fn sock_size() -> u32 {
    std::mem::size_of::<SockAddrIn>() as u32
}

/// Check WIFEXITED: (status & 0x7f) == 0
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// Get WEXITSTATUS: (status >> 8) & 0xff
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Child process: bind to port and wait for packet
fn child_receiver(child_index: usize) -> ! {
    let port = BASE_PORT + child_index as u16;

    println!("CHILD{}: binding to port {}", child_index, port);

    let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        println!("CHILD{}: socket failed, errno={}", child_index, -fd);
        process::exit(10);
    }

    let local_addr = SockAddrIn::new([0, 0, 0, 0], port);
    let ret = unsafe { bind(fd, &local_addr, sock_size()) };
    if ret != 0 {
        println!("CHILD{}: bind failed, errno={}", child_index, -ret);
        process::exit(11);
    }

    println!("CHILD{}: waiting for packet (blocking)...", child_index);

    let mut recv_buf = [0u8; 64];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    let mut addrlen: u32 = sock_size();
    let bytes = unsafe {
        recvfrom(fd, recv_buf.as_mut_ptr(), recv_buf.len(), 0,
                 &mut src_addr, &mut addrlen)
    };

    if bytes < 0 {
        println!("CHILD{}: recvfrom failed, errno={}", child_index, -bytes);
        process::exit(12);
    }

    println!("CHILD{}: received {} bytes", child_index, bytes);

    // Verify payload contains our child index
    let expected_char = b'0' + (child_index as u8);
    let mut found = false;
    for j in 0..bytes as usize {
        if recv_buf[j] == expected_char {
            found = true;
            break;
        }
    }

    if found {
        println!("CHILD{}: payload verified", child_index);
    } else {
        println!("CHILD{}: payload mismatch!", child_index);
        process::exit(13);
    }

    unsafe { close(fd); }
    process::exit(0);
}

fn main() {
    println!("CONCURRENT_RECV_STRESS: starting with {} children", NUM_CHILDREN);

    // Fork child processes
    let mut child_pids = [0i32; NUM_CHILDREN];

    for i in 0..NUM_CHILDREN {
        let result = unsafe { fork() };
        if result == 0 {
            // Child process - run the blocking receiver
            child_receiver(i);
            // child_receiver exits, never returns
        } else if result > 0 {
            // Parent - save child PID
            child_pids[i] = result;
            println!("CONCURRENT_RECV_STRESS: forked child {} with pid {}", i, result);
        } else {
            // Error
            println!("CONCURRENT_RECV_STRESS: fork failed, errno={}", -result);
            process::exit(1);
        }
    }

    // Parent: give children time to bind and start blocking
    for _ in 0..1000 {
        unsafe { sched_yield(); }
    }

    println!("CONCURRENT_RECV_STRESS: parent sending packets to all children");

    // Create sender socket
    let sender_fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if sender_fd < 0 {
        println!("CONCURRENT_RECV_STRESS: sender socket failed, errno={}", -sender_fd);
        process::exit(2);
    }

    // Send a packet to each child
    for i in 0..NUM_CHILDREN {
        let port = BASE_PORT + i as u16;
        let dest_addr = SockAddrIn::new([127, 0, 0, 1], port);

        // Create unique payload for each child
        let payload: [u8; 7] = [
            b'C', b'H', b'I', b'L', b'D',
            b'0' + (i as u8),
            b'\n',
        ];

        let sent = unsafe {
            sendto(sender_fd, payload.as_ptr(), payload.len(), 0,
                   &dest_addr, sock_size())
        };
        if sent < 0 {
            println!("CONCURRENT_RECV_STRESS: sendto failed for child {}, errno={}", i, -sent);
            process::exit(3);
        }
        println!("CONCURRENT_RECV_STRESS: sent {} bytes to port {}", sent, port);

        // Small delay between sends
        for _ in 0..100 {
            unsafe { sched_yield(); }
        }
    }

    unsafe { close(sender_fd); }

    // Wait for all children to complete
    println!("CONCURRENT_RECV_STRESS: waiting for all children");

    let mut all_success = true;
    for i in 0..NUM_CHILDREN {
        let pid = child_pids[i];
        let mut status: i32 = 0;
        let result = unsafe { waitpid(pid, &mut status, 0) };

        if result > 0 {
            if wifexited(status) && wexitstatus(status) == 0 {
                println!("CONCURRENT_RECV_STRESS: child {} (pid {}) succeeded", i, pid);
            } else {
                println!("CONCURRENT_RECV_STRESS: child {} (pid {}) failed with status {}",
                         i, pid, wexitstatus(status));
                all_success = false;
            }
        } else {
            println!("CONCURRENT_RECV_STRESS: waitpid failed for child {}, errno={}", i, -result);
            all_success = false;
        }
    }

    if all_success {
        println!("CONCURRENT_RECV_STRESS: PASS - all {} children received data correctly", NUM_CHILDREN);
        process::exit(0);
    } else {
        println!("CONCURRENT_RECV_STRESS: FAIL - some children failed");
        process::exit(4);
    }
}
