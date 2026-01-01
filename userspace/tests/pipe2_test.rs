//! pipe2() syscall test
//!
//! Tests pipe2() with various flags:
//! - No flags (should work like pipe)
//! - O_CLOEXEC - verify FD_CLOEXEC is set
//! - O_NONBLOCK - verify O_NONBLOCK is set
//! - Both flags combined

#![no_std]
#![no_main]

use libbreenix::io::{
    close, fcntl_getfd, fcntl_getfl, fd_flags, pipe, pipe2, print, println, status_flags, write,
};
use libbreenix::process::exit;
use libbreenix::types::fd::STDOUT;

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
    println("PANIC in pipe2_test!");
    exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== pipe2() syscall test ===");

    // Test 1: pipe2 with no flags (should work like pipe)
    println("");
    println("Test 1: pipe2 with no flags");
    let mut pipefd = [0i32; 2];
    let ret = pipe2(&mut pipefd, 0);
    if ret < 0 {
        print("FAIL: pipe2(0) failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    let read_fd = pipefd[0] as u64;
    let write_fd = pipefd[1] as u64;
    print("PASS: Created pipe with read_fd=");
    print_unum(read_fd);
    print(", write_fd=");
    print_unum(write_fd);
    println("");

    // Verify no FD_CLOEXEC
    let fd_flags_val = fcntl_getfd(read_fd);
    if fd_flags_val < 0 {
        print("FAIL: F_GETFD failed with ");
        print_num(fd_flags_val);
        println("");
        exit(1);
    }
    if fd_flags_val != 0 {
        print("FAIL: Expected fd_flags=0, got ");
        print_num(fd_flags_val);
        println("");
        exit(1);
    }
    println("PASS: No FD_CLOEXEC set (as expected)");

    // Verify no O_NONBLOCK
    let status_val = fcntl_getfl(read_fd);
    if status_val < 0 {
        print("FAIL: F_GETFL failed with ");
        print_num(status_val);
        println("");
        exit(1);
    }
    if status_val != 0 {
        print("FAIL: Expected status_flags=0, got ");
        print_hex(status_val as u64);
        println("");
        exit(1);
    }
    println("PASS: No O_NONBLOCK set (as expected)");

    close(read_fd);
    close(write_fd);

    // Test 2: pipe2 with O_CLOEXEC
    println("");
    println("Test 2: pipe2 with O_CLOEXEC");
    let mut pipefd = [0i32; 2];
    let ret = pipe2(&mut pipefd, status_flags::O_CLOEXEC);
    if ret < 0 {
        print("FAIL: pipe2(O_CLOEXEC) failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    let read_fd = pipefd[0] as u64;
    let write_fd = pipefd[1] as u64;
    print("Created pipe: read_fd=");
    print_unum(read_fd);
    print(", write_fd=");
    print_unum(write_fd);
    println("");

    // Verify FD_CLOEXEC is set on both fds
    let fd_flags_read = fcntl_getfd(read_fd);
    if fd_flags_read != fd_flags::FD_CLOEXEC as i64 {
        print("FAIL: read_fd should have FD_CLOEXEC, got ");
        print_num(fd_flags_read);
        println("");
        exit(1);
    }
    print("PASS: read_fd has FD_CLOEXEC=");
    print_num(fd_flags_read);
    println("");

    let fd_flags_write = fcntl_getfd(write_fd);
    if fd_flags_write != fd_flags::FD_CLOEXEC as i64 {
        print("FAIL: write_fd should have FD_CLOEXEC, got ");
        print_num(fd_flags_write);
        println("");
        exit(1);
    }
    print("PASS: write_fd has FD_CLOEXEC=");
    print_num(fd_flags_write);
    println("");

    close(read_fd);
    close(write_fd);

    // Test 3: pipe2 with O_NONBLOCK
    println("");
    println("Test 3: pipe2 with O_NONBLOCK");
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
    print("Created pipe: read_fd=");
    print_unum(read_fd);
    print(", write_fd=");
    print_unum(write_fd);
    println("");

    // Verify O_NONBLOCK is set on both fds
    let status_read = fcntl_getfl(read_fd);
    if (status_read as i32) & status_flags::O_NONBLOCK != status_flags::O_NONBLOCK {
        print("FAIL: read_fd should have O_NONBLOCK, got ");
        print_hex(status_read as u64);
        println("");
        exit(1);
    }
    print("PASS: read_fd has O_NONBLOCK (status_flags=");
    print_hex(status_read as u64);
    println(")");

    let status_write = fcntl_getfl(write_fd);
    if (status_write as i32) & status_flags::O_NONBLOCK != status_flags::O_NONBLOCK {
        print("FAIL: write_fd should have O_NONBLOCK, got ");
        print_hex(status_write as u64);
        println("");
        exit(1);
    }
    print("PASS: write_fd has O_NONBLOCK (status_flags=");
    print_hex(status_write as u64);
    println(")");

    // Verify no FD_CLOEXEC
    let fd_flags_val = fcntl_getfd(read_fd);
    if fd_flags_val != 0 {
        print("FAIL: read_fd should NOT have FD_CLOEXEC, got ");
        print_num(fd_flags_val);
        println("");
        exit(1);
    }
    println("PASS: No FD_CLOEXEC set (as expected)");

    close(read_fd);
    close(write_fd);

    // Test 4: pipe2 with both O_CLOEXEC and O_NONBLOCK
    println("");
    println("Test 4: pipe2 with O_CLOEXEC | O_NONBLOCK");
    let mut pipefd = [0i32; 2];
    let flags = status_flags::O_CLOEXEC | status_flags::O_NONBLOCK;
    let ret = pipe2(&mut pipefd, flags);
    if ret < 0 {
        print("FAIL: pipe2(O_CLOEXEC|O_NONBLOCK) failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    let read_fd = pipefd[0] as u64;
    let write_fd = pipefd[1] as u64;
    print("Created pipe: read_fd=");
    print_unum(read_fd);
    print(", write_fd=");
    print_unum(write_fd);
    println("");

    // Verify both flags
    let fd_flags_val = fcntl_getfd(read_fd);
    if fd_flags_val != fd_flags::FD_CLOEXEC as i64 {
        print("FAIL: read_fd should have FD_CLOEXEC, got ");
        print_num(fd_flags_val);
        println("");
        exit(1);
    }
    println("PASS: read_fd has FD_CLOEXEC");

    let status_val = fcntl_getfl(read_fd);
    if (status_val as i32) & status_flags::O_NONBLOCK != status_flags::O_NONBLOCK {
        print("FAIL: read_fd should have O_NONBLOCK, got ");
        print_hex(status_val as u64);
        println("");
        exit(1);
    }
    println("PASS: read_fd has O_NONBLOCK");

    close(read_fd);
    close(write_fd);

    // Test 5: pipe2 with invalid flags should fail with EINVAL
    println("");
    println("Test 5: pipe2 with invalid flags");
    let mut pipefd = [0i32; 2];
    let invalid_flags = 0x1; // Some invalid flag
    let ret = pipe2(&mut pipefd, invalid_flags);
    if ret >= 0 {
        print("FAIL: pipe2 with invalid flags should fail, but returned ");
        print_num(ret);
        println("");
        exit(1);
    }
    // EINVAL = 22
    if ret != -22 {
        print("WARN: Expected -22 (EINVAL), got ");
        print_num(ret);
        println(" (still a failure, which is correct)");
    } else {
        print("PASS: pipe2 with invalid flags returned ");
        print_num(ret);
        println(" (EINVAL)");
    }

    // Test 6: Verify pipe2(flags=0) works the same as pipe()
    println("");
    println("Test 6: Compare pipe2(0) with pipe()");
    let mut pipefd1 = [0i32; 2];
    let mut pipefd2 = [0i32; 2];

    let ret1 = pipe(&mut pipefd1);
    let ret2 = pipe2(&mut pipefd2, 0);

    if ret1 < 0 || ret2 < 0 {
        println("FAIL: pipe() or pipe2(0) failed");
        exit(1);
    }

    // Both should have same flags (0)
    let flags1 = fcntl_getfd(pipefd1[0] as u64);
    let flags2 = fcntl_getfd(pipefd2[0] as u64);

    if flags1 != flags2 {
        print("FAIL: pipe() fd_flags=");
        print_num(flags1);
        print(", pipe2(0) fd_flags=");
        print_num(flags2);
        println("");
        exit(1);
    }
    println("PASS: pipe() and pipe2(0) produce same fd_flags");

    close(pipefd1[0] as u64);
    close(pipefd1[1] as u64);
    close(pipefd2[0] as u64);
    close(pipefd2[1] as u64);

    println("");
    println("=== All pipe2 tests passed! ===");
    println("PIPE2_TEST_PASSED");
    exit(0);
}
