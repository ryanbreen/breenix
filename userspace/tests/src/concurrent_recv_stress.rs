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

use libbreenix::io;
use libbreenix::process::{self, ForkResult, wifexited, wexitstatus};
use libbreenix::socket::{self, SockAddrIn, AF_INET, SOCK_DGRAM};
use std::process as std_process;

/// Number of concurrent child processes to spawn
const NUM_CHILDREN: usize = 4;

/// Base port number (children use BASE_PORT + child_index)
const BASE_PORT: u16 = 56000;

fn sock_addr_len() -> u32 {
    core::mem::size_of::<SockAddrIn>() as u32
}

/// Child process: bind to port and wait for packet
fn child_receiver(child_index: usize) -> ! {
    let port = BASE_PORT + child_index as u16;

    println!("CHILD{}: binding to port {}", child_index, port);

    let fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("CHILD{}: socket failed, errno={:?}", child_index, e);
            std_process::exit(10);
        }
    };

    let local_addr = SockAddrIn::new([0, 0, 0, 0], port);
    if let Err(e) = socket::bind_inet(fd, &local_addr) {
        println!("CHILD{}: bind failed, errno={:?}", child_index, e);
        std_process::exit(11);
    }

    println!("CHILD{}: waiting for packet (blocking)...", child_index);

    let mut recv_buf = [0u8; 64];
    match socket::recvfrom(fd, &mut recv_buf, None) {
        Ok(bytes) => {
            println!("CHILD{}: received {} bytes", child_index, bytes);

            // Verify payload contains our child index
            let expected_char = b'0' + (child_index as u8);
            let mut found = false;
            for j in 0..bytes {
                if recv_buf[j] == expected_char {
                    found = true;
                    break;
                }
            }

            if found {
                println!("CHILD{}: payload verified", child_index);
            } else {
                println!("CHILD{}: payload mismatch!", child_index);
                std_process::exit(13);
            }
        }
        Err(e) => {
            println!("CHILD{}: recvfrom failed, errno={:?}", child_index, e);
            std_process::exit(12);
        }
    }

    let _ = io::close(fd);
    std_process::exit(0);
}

fn main() {
    println!("CONCURRENT_RECV_STRESS: starting with {} children", NUM_CHILDREN);

    // Fork child processes
    let mut child_pids = [0u64; NUM_CHILDREN];

    for i in 0..NUM_CHILDREN {
        match process::fork() {
            Ok(ForkResult::Child) => {
                // Child process - run the blocking receiver
                child_receiver(i);
                // child_receiver exits, never returns
            }
            Ok(ForkResult::Parent(pid)) => {
                // Parent - save child PID
                child_pids[i] = pid.raw();
                println!("CONCURRENT_RECV_STRESS: forked child {} with pid {}", i, pid.raw());
            }
            Err(e) => {
                println!("CONCURRENT_RECV_STRESS: fork failed, errno={:?}", e);
                std_process::exit(1);
            }
        }
    }

    // Parent: give children time to bind and start blocking
    for _ in 0..1000 {
        let _ = process::yield_now();
    }

    println!("CONCURRENT_RECV_STRESS: parent sending packets to all children");

    // Create sender socket
    let sender_fd = match socket::socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            println!("CONCURRENT_RECV_STRESS: sender socket failed, errno={:?}", e);
            std_process::exit(2);
        }
    };

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

        match socket::sendto(sender_fd, &payload, &dest_addr) {
            Ok(sent) => {
                println!("CONCURRENT_RECV_STRESS: sent {} bytes to port {}", sent, port);
            }
            Err(e) => {
                println!("CONCURRENT_RECV_STRESS: sendto failed for child {}, errno={:?}", i, e);
                std_process::exit(3);
            }
        }

        // Small delay between sends
        for _ in 0..100 {
            let _ = process::yield_now();
        }
    }

    let _ = io::close(sender_fd);

    // Wait for all children to complete
    println!("CONCURRENT_RECV_STRESS: waiting for all children");

    let mut all_success = true;
    for i in 0..NUM_CHILDREN {
        let pid = child_pids[i] as i32;
        let mut status: i32 = 0;
        match process::waitpid(pid, &mut status, 0) {
            Ok(result_pid) => {
                if wifexited(status) && wexitstatus(status) == 0 {
                    println!("CONCURRENT_RECV_STRESS: child {} (pid {}) succeeded", i, pid);
                } else {
                    println!("CONCURRENT_RECV_STRESS: child {} (pid {}) failed with status {}",
                             i, pid, wexitstatus(status));
                    all_success = false;
                }
            }
            Err(e) => {
                println!("CONCURRENT_RECV_STRESS: waitpid failed for child {}, errno={:?}", i, e);
                all_success = false;
            }
        }
    }

    if all_success {
        println!("CONCURRENT_RECV_STRESS: PASS - all {} children received data correctly", NUM_CHILDREN);
        std_process::exit(0);
    } else {
        println!("CONCURRENT_RECV_STRESS: FAIL - some children failed");
        std_process::exit(4);
    }
}
