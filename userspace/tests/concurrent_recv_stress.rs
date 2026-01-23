//! Concurrent blocking recvfrom() stress test
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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process::{self, wexitstatus, wifexited};
use libbreenix::socket::{bind, recvfrom, sendto, socket, SockAddrIn, AF_INET, SOCK_DGRAM};

/// Number of concurrent child processes to spawn
const NUM_CHILDREN: usize = 4;

/// Base port number (children use BASE_PORT + child_index)
const BASE_PORT: u16 = 56000;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("CONCURRENT_RECV_STRESS: starting with ");
    print_num(NUM_CHILDREN as u64);
    io::print(" children\n");

    // Fork child processes
    let mut child_pids = [0i64; NUM_CHILDREN];

    for i in 0..NUM_CHILDREN {
        let result = process::fork();
        if result == 0 {
            // Child process - run the blocking receiver
            child_receiver(i);
            // child_receiver exits, never returns
        } else if result > 0 {
            // Parent - save child PID
            child_pids[i] = result;
            io::print("CONCURRENT_RECV_STRESS: forked child ");
            print_num(i as u64);
            io::print(" with pid ");
            print_num(result as u64);
            io::print("\n");
        } else {
            // Error
            io::print("CONCURRENT_RECV_STRESS: fork failed, errno=");
            print_num((-result) as u64);
            io::print("\n");
            process::exit(1);
        }
    }

    // Parent: give children time to bind and start blocking
    // In a real OS we'd use nanosleep, here we just yield many times
    for _ in 0..1000 {
        process::yield_now();
    }

    io::print("CONCURRENT_RECV_STRESS: parent sending packets to all children\n");

    // Create sender socket
    let sender_fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("CONCURRENT_RECV_STRESS: sender socket failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(2);
        }
    };

    // Send a packet to each child
    for i in 0..NUM_CHILDREN {
        let port = BASE_PORT + i as u16;
        let dest_addr = SockAddrIn::new([127, 0, 0, 1], port);

        // Create unique payload for each child
        let payload: [u8; 8] = [
            b'C', b'H', b'I', b'L', b'D',
            b'0' + (i as u8),
            b'\n', 0
        ];

        match sendto(sender_fd, &payload[..7], &dest_addr) {
            Ok(sent) => {
                io::print("CONCURRENT_RECV_STRESS: sent ");
                print_num(sent as u64);
                io::print(" bytes to port ");
                print_num(port as u64);
                io::print("\n");
            }
            Err(e) => {
                io::print("CONCURRENT_RECV_STRESS: sendto failed for child ");
                print_num(i as u64);
                io::print(", errno=");
                print_num(e as u64);
                io::print("\n");
                process::exit(3);
            }
        }

        // Small delay between sends
        for _ in 0..100 {
            process::yield_now();
        }
    }

    io::close(sender_fd as u64);

    // Wait for all children to complete
    io::print("CONCURRENT_RECV_STRESS: waiting for all children\n");

    let mut all_success = true;
    for i in 0..NUM_CHILDREN {
        let pid = child_pids[i] as i32;
        let mut status: i32 = 0;
        let result = process::waitpid(pid, &mut status as *mut i32, 0);

        if result > 0 {
            if wifexited(status) && wexitstatus(status) == 0 {
                io::print("CONCURRENT_RECV_STRESS: child ");
                print_num(i as u64);
                io::print(" (pid ");
                print_num(pid as u64);
                io::print(") succeeded\n");
            } else {
                io::print("CONCURRENT_RECV_STRESS: child ");
                print_num(i as u64);
                io::print(" (pid ");
                print_num(pid as u64);
                io::print(") failed with status ");
                print_num(wexitstatus(status) as u64);
                io::print("\n");
                all_success = false;
            }
        } else {
            io::print("CONCURRENT_RECV_STRESS: waitpid failed for child ");
            print_num(i as u64);
            io::print(", errno=");
            print_num((-result) as u64);
            io::print("\n");
            all_success = false;
        }
    }

    if all_success {
        io::print("CONCURRENT_RECV_STRESS: PASS - all ");
        print_num(NUM_CHILDREN as u64);
        io::print(" children received data correctly\n");
        process::exit(0);
    } else {
        io::print("CONCURRENT_RECV_STRESS: FAIL - some children failed\n");
        process::exit(4);
    }
}

/// Child process: bind to port and wait for packet
fn child_receiver(child_index: usize) -> ! {
    let port = BASE_PORT + child_index as u16;

    io::print("CHILD");
    print_num(child_index as u64);
    io::print(": binding to port ");
    print_num(port as u64);
    io::print("\n");

    let fd = match socket(AF_INET, SOCK_DGRAM, 0) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("CHILD");
            print_num(child_index as u64);
            io::print(": socket failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(10);
        }
    };

    let local_addr = SockAddrIn::new([0, 0, 0, 0], port);
    match bind(fd, &local_addr) {
        Ok(()) => {}
        Err(e) => {
            io::print("CHILD");
            print_num(child_index as u64);
            io::print(": bind failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(11);
        }
    }

    io::print("CHILD");
    print_num(child_index as u64);
    io::print(": waiting for packet (blocking)...\n");

    let mut recv_buf = [0u8; 64];
    let mut src_addr = SockAddrIn::new([0, 0, 0, 0], 0);
    match recvfrom(fd, &mut recv_buf, Some(&mut src_addr)) {
        Ok(bytes) => {
            io::print("CHILD");
            print_num(child_index as u64);
            io::print(": received ");
            print_num(bytes as u64);
            io::print(" bytes\n");

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
                io::print("CHILD");
                print_num(child_index as u64);
                io::print(": payload verified\n");
            } else {
                io::print("CHILD");
                print_num(child_index as u64);
                io::print(": payload mismatch!\n");
                process::exit(13);
            }
        }
        Err(e) => {
            io::print("CHILD");
            print_num(child_index as u64);
            io::print(": recvfrom failed, errno=");
            print_num(e as u64);
            io::print("\n");
            process::exit(12);
        }
    }

    io::close(fd as u64);
    process::exit(0);
}

/// Simple number printing (no formatting)
fn print_num(mut n: u64) {
    if n == 0 {
        io::print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }

    while i > 0 {
        i -= 1;
        let ch = [buf[i]];
        if let Ok(s) = core::str::from_utf8(&ch) {
            io::print(s);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("CONCURRENT_RECV_STRESS: PANIC!\n");
    process::exit(99);
}
