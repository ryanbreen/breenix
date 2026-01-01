//! O_NONBLOCK pipe test
//!
//! Tests non-blocking I/O behavior for pipes:
//! - pipe2(O_NONBLOCK) creates non-blocking pipes
//! - read() on empty non-blocking pipe returns -EAGAIN
//! - write() on full non-blocking pipe returns -EAGAIN
//! - fcntl(F_SETFL) can set O_NONBLOCK on existing pipe

#![no_std]
#![no_main]

use libbreenix::io::{
    close, fcntl_getfl, fcntl_setfl, pipe, pipe2, print, println, read, status_flags, write,
};
use libbreenix::process::exit;
use libbreenix::types::fd::STDOUT;

/// EAGAIN errno value
const EAGAIN: i64 = -11;

/// Pipe buffer size (matches kernel PIPE_BUF_SIZE)
const PIPE_BUF_SIZE: usize = 65536;

/// Print a signed number
fn print_num(n: i64) {
    if n < 0 {
        print("-");
        print_unum((-n) as u64);
    } else {
        print_unum(n as u64);
    }
}

/// Print an unsigned number
fn print_unum(mut n: u64) {
    if n == 0 {
        print("0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    // Reverse and print
    while i > 0 {
        i -= 1;
        let _ = write(STDOUT, &[buf[i]]);
    }
}

/// Print a hex number
fn print_hex(mut n: u64) {
    print("0x");
    if n == 0 {
        print("0");
        return;
    }
    let mut buf = [0u8; 16];
    let mut i = 0;
    while n > 0 {
        let digit = (n & 0xF) as u8;
        buf[i] = if digit < 10 {
            b'0' + digit
        } else {
            b'a' + digit - 10
        };
        n >>= 4;
        i += 1;
    }
    // Reverse and print
    while i > 0 {
        i -= 1;
        let _ = write(STDOUT, &[buf[i]]);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println("PANIC in nonblock_test!");
    exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== O_NONBLOCK pipe test ===");

    // Test 1: Read from empty pipe with O_NONBLOCK should return EAGAIN
    println("");
    println("Test 1: Read from empty O_NONBLOCK pipe");
    let mut pipefd = [0i32; 2];
    let ret = pipe2(&mut pipefd, status_flags::O_NONBLOCK);
    if ret < 0 {
        print("FAIL: pipe2(O_NONBLOCK) failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    let read_fd = pipefd[0] as u64;
    let write_fd = pipefd[1] as u64;
    print("Created O_NONBLOCK pipe: read_fd=");
    print_unum(read_fd);
    print(", write_fd=");
    print_unum(write_fd);
    println("");

    // Verify O_NONBLOCK is set
    let status = fcntl_getfl(read_fd);
    if (status as i32) & status_flags::O_NONBLOCK == 0 {
        print("FAIL: O_NONBLOCK not set on read_fd, status=");
        print_hex(status as u64);
        println("");
        exit(1);
    }
    println("O_NONBLOCK confirmed set on pipe");

    // Try to read from empty pipe - should return EAGAIN
    let mut read_buf = [0u8; 32];
    let read_ret = read(read_fd, &mut read_buf);
    if read_ret != EAGAIN {
        print("FAIL: Read from empty O_NONBLOCK pipe should return -11 (EAGAIN), got ");
        print_num(read_ret);
        println("");
        exit(1);
    }
    println("PASS: Read from empty O_NONBLOCK pipe returned EAGAIN");

    close(read_fd);
    close(write_fd);

    // Test 2: Write to full pipe with O_NONBLOCK should return EAGAIN
    println("");
    println("Test 2: Write to full O_NONBLOCK pipe");
    let mut pipefd = [0i32; 2];
    let ret = pipe2(&mut pipefd, status_flags::O_NONBLOCK);
    if ret < 0 {
        print("FAIL: pipe2(O_NONBLOCK) failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    let read_fd = pipefd[0] as u64;
    let write_fd = pipefd[1] as u64;

    // Fill the pipe buffer (64KB)
    println("Filling pipe buffer...");
    let fill_data = [0x41u8; 1024]; // 1KB of 'A's
    let mut total_written: usize = 0;
    let mut write_count = 0;

    // Write until we get EAGAIN (buffer full)
    loop {
        let ret = write(write_fd, &fill_data);
        if ret == EAGAIN {
            print("Got EAGAIN after writing ");
            print_unum(total_written as u64);
            println(" bytes");
            break;
        } else if ret < 0 {
            print("FAIL: Unexpected write error: ");
            print_num(ret);
            println("");
            exit(1);
        } else {
            total_written += ret as usize;
            write_count += 1;
            // Safety check to avoid infinite loop
            if total_written > PIPE_BUF_SIZE + 1024 {
                print("FAIL: Wrote more than PIPE_BUF_SIZE without EAGAIN: ");
                print_unum(total_written as u64);
                println("");
                exit(1);
            }
        }
    }

    if total_written < PIPE_BUF_SIZE - 1024 {
        print("WARN: Buffer filled at ");
        print_unum(total_written as u64);
        print(" bytes (expected ~");
        print_unum(PIPE_BUF_SIZE as u64);
        println(")");
    }
    print("PASS: Pipe buffer filled with ");
    print_unum(total_written as u64);
    print(" bytes in ");
    print_unum(write_count as u64);
    println(" writes, got EAGAIN on full buffer");

    close(read_fd);
    close(write_fd);

    // Test 3: Set O_NONBLOCK via fcntl(F_SETFL)
    println("");
    println("Test 3: Set O_NONBLOCK via fcntl(F_SETFL)");
    let mut pipefd = [0i32; 2];
    let ret = pipe(&mut pipefd); // Create blocking pipe
    if ret < 0 {
        print("FAIL: pipe() failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    let read_fd = pipefd[0] as u64;
    let write_fd = pipefd[1] as u64;

    // Verify O_NONBLOCK is NOT set initially
    let status = fcntl_getfl(read_fd);
    if (status as i32) & status_flags::O_NONBLOCK != 0 {
        print("FAIL: O_NONBLOCK should not be set initially, status=");
        print_hex(status as u64);
        println("");
        exit(1);
    }
    println("Confirmed O_NONBLOCK not set initially");

    // Set O_NONBLOCK via fcntl
    let ret = fcntl_setfl(read_fd, status_flags::O_NONBLOCK);
    if ret < 0 {
        print("FAIL: fcntl(F_SETFL, O_NONBLOCK) failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    println("Set O_NONBLOCK via fcntl(F_SETFL)");

    // Verify O_NONBLOCK is now set
    let status = fcntl_getfl(read_fd);
    if (status as i32) & status_flags::O_NONBLOCK == 0 {
        print("FAIL: O_NONBLOCK should be set now, status=");
        print_hex(status as u64);
        println("");
        exit(1);
    }
    println("PASS: O_NONBLOCK now set via fcntl");

    // Now read should return EAGAIN
    let mut read_buf = [0u8; 32];
    let read_ret = read(read_fd, &mut read_buf);
    if read_ret != EAGAIN {
        print("FAIL: Read from empty pipe (after fcntl) should return -11 (EAGAIN), got ");
        print_num(read_ret);
        println("");
        exit(1);
    }
    println("PASS: Read returns EAGAIN after setting O_NONBLOCK via fcntl");

    close(read_fd);
    close(write_fd);

    // Test 4: Read succeeds when data is available (even with O_NONBLOCK)
    println("");
    println("Test 4: Read with data available (O_NONBLOCK pipe)");
    let mut pipefd = [0i32; 2];
    let ret = pipe2(&mut pipefd, status_flags::O_NONBLOCK);
    if ret < 0 {
        print("FAIL: pipe2(O_NONBLOCK) failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    let read_fd = pipefd[0] as u64;
    let write_fd = pipefd[1] as u64;

    // Write some data
    let test_data = b"Hello!";
    let write_ret = write(write_fd, test_data);
    if write_ret != test_data.len() as i64 {
        print("FAIL: Write failed, expected ");
        print_unum(test_data.len() as u64);
        print(", got ");
        print_num(write_ret);
        println("");
        exit(1);
    }

    // Read should succeed (not EAGAIN) because data is available
    let mut read_buf = [0u8; 32];
    let read_ret = read(read_fd, &mut read_buf);
    if read_ret < 0 {
        print("FAIL: Read with data should succeed, got ");
        print_num(read_ret);
        println("");
        exit(1);
    }
    if read_ret != test_data.len() as i64 {
        print("FAIL: Read returned wrong count: expected ");
        print_unum(test_data.len() as u64);
        print(", got ");
        print_num(read_ret);
        println("");
        exit(1);
    }
    print("PASS: Read ");
    print_num(read_ret);
    println(" bytes when data available");

    // Now pipe is empty, read should return EAGAIN again
    let read_ret2 = read(read_fd, &mut read_buf);
    if read_ret2 != EAGAIN {
        print("FAIL: Second read should return EAGAIN, got ");
        print_num(read_ret2);
        println("");
        exit(1);
    }
    println("PASS: Second read (empty again) returns EAGAIN");

    close(read_fd);
    close(write_fd);

    println("");
    println("=== All O_NONBLOCK tests passed! ===");
    println("NONBLOCK_TEST_PASSED");
    exit(0);
}
