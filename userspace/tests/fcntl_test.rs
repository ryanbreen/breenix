//! fcntl() syscall test
//!
//! Tests file descriptor control operations:
//! - F_GETFD/F_SETFD for FD_CLOEXEC
//! - F_GETFL/F_SETFL for O_NONBLOCK
//! - F_DUPFD for duplicating with minimum fd

#![no_std]
#![no_main]

use libbreenix::io::{
    close, fcntl, fcntl_cmd, fcntl_getfd, fcntl_getfl, fcntl_setfd, fcntl_setfl, fd_flags, pipe,
    print, println, status_flags, write,
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
    println("PANIC in fcntl_test!");
    exit(1);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== fcntl() syscall test ===");

    // Create a pipe for testing
    let mut pipefd = [0i32; 2];
    let ret = pipe(&mut pipefd);
    if ret < 0 {
        print("FAIL: pipe() failed with ");
        print_num(ret as i64);
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

    // Test 1: F_GETFD - should return 0 initially (no FD_CLOEXEC)
    println("");
    println("Test 1: F_GETFD (initial)");
    let flags = fcntl_getfd(read_fd);
    if flags < 0 {
        print("FAIL: F_GETFD failed with ");
        print_num(flags);
        println("");
        exit(1);
    }
    if flags != 0 {
        print("FAIL: Expected fd_flags=0, got ");
        print_num(flags);
        println("");
        exit(1);
    }
    print("PASS: Initial fd_flags = ");
    print_num(flags);
    println(" (no FD_CLOEXEC)");

    // Test 2: F_SETFD - set FD_CLOEXEC
    println("");
    println("Test 2: F_SETFD (set FD_CLOEXEC)");
    let ret = fcntl_setfd(read_fd, fd_flags::FD_CLOEXEC);
    if ret < 0 {
        print("FAIL: F_SETFD failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    print("PASS: F_SETFD returned ");
    print_num(ret);
    println("");

    // Test 3: F_GETFD - should now have FD_CLOEXEC
    println("");
    println("Test 3: F_GETFD (after set)");
    let flags = fcntl_getfd(read_fd);
    if flags < 0 {
        print("FAIL: F_GETFD failed with ");
        print_num(flags);
        println("");
        exit(1);
    }
    if flags != fd_flags::FD_CLOEXEC as i64 {
        print("FAIL: Expected FD_CLOEXEC=");
        print_num(fd_flags::FD_CLOEXEC as i64);
        print(", got ");
        print_num(flags);
        println("");
        exit(1);
    }
    print("PASS: fd_flags = ");
    print_num(flags);
    println(" (FD_CLOEXEC set)");

    // Test 4: F_GETFL - should return 0 initially
    println("");
    println("Test 4: F_GETFL (initial)");
    let flags = fcntl_getfl(write_fd);
    if flags < 0 {
        print("FAIL: F_GETFL failed with ");
        print_num(flags);
        println("");
        exit(1);
    }
    print("PASS: Initial status_flags = ");
    print_hex(flags as u64);
    println("");

    // Test 5: F_SETFL - set O_NONBLOCK
    println("");
    println("Test 5: F_SETFL (set O_NONBLOCK)");
    let ret = fcntl_setfl(write_fd, status_flags::O_NONBLOCK);
    if ret < 0 {
        print("FAIL: F_SETFL failed with ");
        print_num(ret);
        println("");
        exit(1);
    }
    print("PASS: F_SETFL returned ");
    print_num(ret);
    println("");

    // Test 6: F_GETFL - should now have O_NONBLOCK
    println("");
    println("Test 6: F_GETFL (after set)");
    let flags = fcntl_getfl(write_fd);
    if flags < 0 {
        print("FAIL: F_GETFL failed with ");
        print_num(flags);
        println("");
        exit(1);
    }
    if (flags as i32) & status_flags::O_NONBLOCK != status_flags::O_NONBLOCK {
        print("FAIL: Expected O_NONBLOCK=");
        print_hex(status_flags::O_NONBLOCK as u64);
        print(" in flags ");
        print_hex(flags as u64);
        println("");
        exit(1);
    }
    print("PASS: status_flags = ");
    print_hex(flags as u64);
    println(" (O_NONBLOCK set)");

    // Test 7: F_DUPFD - duplicate to minimum fd
    println("");
    println("Test 7: F_DUPFD (dup to >= 10)");
    let new_fd = fcntl(read_fd, fcntl_cmd::F_DUPFD, 10);
    if new_fd < 0 {
        print("FAIL: F_DUPFD failed with ");
        print_num(new_fd);
        println("");
        exit(1);
    }
    if new_fd < 10 {
        print("FAIL: Expected new_fd >= 10, got ");
        print_num(new_fd);
        println("");
        exit(1);
    }
    print("PASS: F_DUPFD returned ");
    print_num(new_fd);
    println(" (>= 10)");

    // Test 8: F_DUPFD result should NOT have FD_CLOEXEC (even though source did)
    println("");
    println("Test 8: F_DUPFD clears FD_CLOEXEC");
    let flags = fcntl_getfd(new_fd as u64);
    if flags < 0 {
        print("FAIL: F_GETFD on dup'd fd failed with ");
        print_num(flags);
        println("");
        exit(1);
    }
    if flags != 0 {
        print("FAIL: F_DUPFD should clear FD_CLOEXEC, got flags=");
        print_num(flags);
        println("");
        exit(1);
    }
    print("PASS: F_DUPFD cleared FD_CLOEXEC (flags=");
    print_num(flags);
    println(")");

    // Test 9: F_DUPFD_CLOEXEC - duplicate with cloexec set
    println("");
    println("Test 9: F_DUPFD_CLOEXEC");
    let new_fd2 = fcntl(write_fd, fcntl_cmd::F_DUPFD_CLOEXEC, 20);
    if new_fd2 < 0 {
        print("FAIL: F_DUPFD_CLOEXEC failed with ");
        print_num(new_fd2);
        println("");
        exit(1);
    }
    if new_fd2 < 20 {
        print("FAIL: Expected new_fd >= 20, got ");
        print_num(new_fd2);
        println("");
        exit(1);
    }
    let flags = fcntl_getfd(new_fd2 as u64);
    if flags != fd_flags::FD_CLOEXEC as i64 {
        print("FAIL: F_DUPFD_CLOEXEC should set FD_CLOEXEC, got flags=");
        print_num(flags);
        println("");
        exit(1);
    }
    print("PASS: F_DUPFD_CLOEXEC returned ");
    print_num(new_fd2);
    println(" with FD_CLOEXEC set");

    // Test 10: fcntl on invalid fd should fail
    println("");
    println("Test 10: fcntl on invalid fd");
    let ret = fcntl_getfd(999);
    if ret >= 0 {
        print("FAIL: fcntl on invalid fd should fail, got ");
        print_num(ret);
        println("");
        exit(1);
    }
    print("PASS: fcntl on invalid fd returned ");
    print_num(ret);
    println(" (EBADF)");

    // Cleanup
    close(read_fd);
    close(write_fd);
    close(new_fd as u64);
    close(new_fd2 as u64);

    println("");
    println("=== All fcntl tests passed! ===");
    println("FCNTL_TEST_PASSED");
    exit(0);
}
