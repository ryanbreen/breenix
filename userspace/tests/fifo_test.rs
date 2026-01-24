//! FIFO (Named Pipe) Test
//!
//! Tests the FIFO implementation including:
//! - mkfifo() syscall
//! - Opening FIFOs for read/write
//! - Reading and writing through FIFOs
//! - O_NONBLOCK behavior
//! - unlink() for FIFOs
//! - Error conditions

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{mkfifo, open, unlink, O_RDONLY, O_WRONLY, O_NONBLOCK};
use libbreenix::io::{self, close, read, write};
use libbreenix::process;

// Error codes (as i64 for comparison with Errno cast)
const ENOENT: i64 = 2;
const EEXIST: i64 = 17;
const EAGAIN: i64 = 11;
const ENXIO: i64 = 6;

/// Print a number to stdout
fn print_num(n: i64) {
    let mut buf = [0u8; 21];
    let mut i = 20;
    let negative = n < 0;
    let mut n = if negative { (-n) as u64 } else { n as u64 };

    if n == 0 {
        io::print("0");
        return;
    }

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }

    if negative {
        buf[i] = b'-';
        i -= 1;
    }

    if let Ok(s) = core::str::from_utf8(&buf[i + 1..]) {
        io::print(s);
    }
}

/// Get error code from Errno enum
fn errno_code(e: Errno) -> i64 {
    e as i64
}

/// Phase 1: Basic FIFO create and open
/// Uses O_NONBLOCK on reader so we don't block waiting for writer
fn test_basic_fifo() -> bool {
    io::print("Phase 1: Basic FIFO create/open/close\n");

    // Create a FIFO
    let path = "/tmp/test_fifo1\0";
    match mkfifo(path, 0o644) {
        Ok(()) => io::print("  Created FIFO at /tmp/test_fifo1\n"),
        Err(e) => {
            io::print("  ERROR: mkfifo failed: ");
            print_num(errno_code(e));
            io::print("\n");
            return false;
        }
    }

    // Open read end with O_NONBLOCK (won't block waiting for writer)
    let read_fd = match open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => {
            io::print("  Opened FIFO for reading: fd=");
            print_num(fd as i64);
            io::print("\n");
            fd
        }
        Err(e) => {
            io::print("  ERROR: open for read failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = unlink(path);
            return false;
        }
    };

    // Open write end (will succeed now that reader exists)
    let write_fd = match open(path, O_WRONLY) {
        Ok(fd) => {
            io::print("  Opened FIFO for writing: fd=");
            print_num(fd as i64);
            io::print("\n");
            fd
        }
        Err(e) => {
            io::print("  ERROR: open for write failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = close(read_fd);
            let _ = unlink(path);
            return false;
        }
    };

    // Write data
    let data = b"Hello FIFO!";
    let write_ret = write(write_fd, data);
    if write_ret < 0 {
        io::print("  ERROR: write failed: ");
        print_num(-write_ret);
        io::print("\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        let _ = unlink(path);
        return false;
    }
    io::print("  Wrote ");
    print_num(write_ret);
    io::print(" bytes to FIFO\n");

    // Read data back
    let mut buf = [0u8; 32];
    let read_ret = read(read_fd, &mut buf);
    if read_ret < 0 {
        io::print("  ERROR: read failed: ");
        print_num(-read_ret);
        io::print("\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        let _ = unlink(path);
        return false;
    }
    io::print("  Read ");
    print_num(read_ret);
    io::print(" bytes from FIFO\n");
    if &buf[..read_ret as usize] == data {
        io::print("  Data matches!\n");
    } else {
        io::print("  ERROR: Data mismatch!\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        let _ = unlink(path);
        return false;
    }

    // Cleanup
    let _ = close(read_fd);
    let _ = close(write_fd);
    let _ = unlink(path);

    io::print("Phase 1: PASSED\n");
    true
}

/// Phase 2: EEXIST - mkfifo on existing path
fn test_eexist() -> bool {
    io::print("Phase 2: EEXIST test\n");

    let path = "/tmp/test_fifo2\0";

    // Create first FIFO
    match mkfifo(path, 0o644) {
        Ok(()) => io::print("  Created first FIFO\n"),
        Err(e) => {
            io::print("  ERROR: first mkfifo failed: ");
            print_num(errno_code(e));
            io::print("\n");
            return false;
        }
    }

    // Try to create again - should fail with EEXIST
    match mkfifo(path, 0o644) {
        Ok(()) => {
            io::print("  ERROR: second mkfifo should have failed\n");
            let _ = unlink(path);
            return false;
        }
        Err(e) => {
            if errno_code(e) == EEXIST {
                io::print("  Got expected EEXIST error\n");
            } else {
                io::print("  ERROR: expected EEXIST, got ");
                print_num(errno_code(e));
                io::print("\n");
                let _ = unlink(path);
                return false;
            }
        }
    }

    let _ = unlink(path);
    io::print("Phase 2: PASSED\n");
    true
}

/// Phase 3: ENOENT - open non-existent FIFO
fn test_enoent() -> bool {
    io::print("Phase 3: ENOENT test\n");

    let path = "/tmp/nonexistent_fifo\0";

    // Try to open non-existent FIFO
    match open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => {
            io::print("  ERROR: open should have failed, got fd=");
            print_num(fd as i64);
            io::print("\n");
            let _ = close(fd);
            return false;
        }
        Err(e) => {
            if errno_code(e) == ENOENT {
                io::print("  Got expected ENOENT error\n");
            } else {
                io::print("  ERROR: expected ENOENT, got ");
                print_num(errno_code(e));
                io::print("\n");
                return false;
            }
        }
    }

    io::print("Phase 3: PASSED\n");
    true
}

/// Phase 4: O_NONBLOCK write without reader returns ENXIO
fn test_nonblock_write_no_reader() -> bool {
    io::print("Phase 4: O_NONBLOCK write without reader\n");

    let path = "/tmp/test_fifo4\0";

    // Create FIFO
    match mkfifo(path, 0o644) {
        Ok(()) => io::print("  Created FIFO\n"),
        Err(e) => {
            io::print("  ERROR: mkfifo failed: ");
            print_num(errno_code(e));
            io::print("\n");
            return false;
        }
    }

    // Try to open for writing with O_NONBLOCK (no reader yet)
    // POSIX says this should return ENXIO
    match open(path, O_WRONLY | O_NONBLOCK) {
        Ok(fd) => {
            io::print("  ERROR: open should have returned ENXIO, got fd=");
            print_num(fd as i64);
            io::print("\n");
            let _ = close(fd);
            let _ = unlink(path);
            return false;
        }
        Err(e) => {
            if errno_code(e) == ENXIO {
                io::print("  Got expected ENXIO error\n");
            } else {
                io::print("  ERROR: expected ENXIO, got ");
                print_num(errno_code(e));
                io::print("\n");
                let _ = unlink(path);
                return false;
            }
        }
    }

    let _ = unlink(path);
    io::print("Phase 4: PASSED\n");
    true
}

/// Phase 5: Read from empty FIFO with O_NONBLOCK returns EAGAIN
fn test_nonblock_read_empty() -> bool {
    io::print("Phase 5: O_NONBLOCK read from empty FIFO\n");

    let path = "/tmp/test_fifo5\0";

    // Create FIFO
    match mkfifo(path, 0o644) {
        Ok(()) => io::print("  Created FIFO\n"),
        Err(e) => {
            io::print("  ERROR: mkfifo failed: ");
            print_num(errno_code(e));
            io::print("\n");
            return false;
        }
    }

    // Open read end with O_NONBLOCK
    let read_fd = match open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  ERROR: open for read failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = unlink(path);
            return false;
        }
    };

    // Open write end so we have a complete FIFO
    let write_fd = match open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  ERROR: open for write failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = close(read_fd);
            let _ = unlink(path);
            return false;
        }
    };

    // Try to read from empty FIFO - should return EAGAIN (negative)
    let mut buf = [0u8; 32];
    let read_ret = read(read_fd, &mut buf);
    if read_ret >= 0 {
        io::print("  ERROR: read should have returned EAGAIN, got ");
        print_num(read_ret);
        io::print(" bytes\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        let _ = unlink(path);
        return false;
    }
    if -read_ret == EAGAIN {
        io::print("  Got expected EAGAIN error\n");
    } else {
        io::print("  ERROR: expected EAGAIN, got ");
        print_num(-read_ret);
        io::print("\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        let _ = unlink(path);
        return false;
    }

    let _ = close(read_fd);
    let _ = close(write_fd);
    let _ = unlink(path);
    io::print("Phase 5: PASSED\n");
    true
}

/// Phase 6: Multiple writes and reads
fn test_multiple_io() -> bool {
    io::print("Phase 6: Multiple writes and reads\n");

    let path = "/tmp/test_fifo6\0";

    // Create FIFO
    match mkfifo(path, 0o644) {
        Ok(()) => (),
        Err(e) => {
            io::print("  ERROR: mkfifo failed: ");
            print_num(errno_code(e));
            io::print("\n");
            return false;
        }
    }

    // Open both ends
    let read_fd = match open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  ERROR: open for read failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = unlink(path);
            return false;
        }
    };

    let write_fd = match open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  ERROR: open for write failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = close(read_fd);
            let _ = unlink(path);
            return false;
        }
    };

    // Write multiple times - all same size (8 bytes) for easy counting
    let data1 = b"First\0\0\0";
    let data2 = b"Second\0\0";
    let data3 = b"Third\0\0\0";

    for (i, data) in [(1i64, data1 as &[u8]), (2i64, data2), (3i64, data3)].iter() {
        let write_ret = write(write_fd, *data);
        if write_ret < 0 {
            io::print("  ERROR: write failed: ");
            print_num(-write_ret);
            io::print("\n");
            let _ = close(read_fd);
            let _ = close(write_fd);
            let _ = unlink(path);
            return false;
        }
        io::print("  Write ");
        print_num(*i);
        io::print(": ");
        print_num(write_ret);
        io::print(" bytes\n");
    }

    // Read all data
    let mut buf = [0u8; 64];
    let mut total = 0usize;
    loop {
        let read_ret = read(read_fd, &mut buf[total..]);
        if read_ret == 0 {
            break; // EOF
        } else if read_ret < 0 {
            if -read_ret == EAGAIN {
                // No more data
                break;
            }
            io::print("  ERROR: read failed: ");
            print_num(-read_ret);
            io::print("\n");
            let _ = close(read_fd);
            let _ = close(write_fd);
            let _ = unlink(path);
            return false;
        }
        total += read_ret as usize;
        io::print("  Read ");
        print_num(read_ret);
        io::print(" bytes (total: ");
        print_num(total as i64);
        io::print(")\n");
    }

    io::print("  Total read: ");
    print_num(total as i64);
    io::print(" bytes\n");
    if total != 24 {
        // 8 + 8 + 8 = 24
        io::print("  ERROR: expected 24 bytes, got ");
        print_num(total as i64);
        io::print("\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        let _ = unlink(path);
        return false;
    }

    let _ = close(read_fd);
    let _ = close(write_fd);
    let _ = unlink(path);
    io::print("Phase 6: PASSED\n");
    true
}

/// Phase 7: Unlink FIFO while open
fn test_unlink_while_open() -> bool {
    io::print("Phase 7: Unlink FIFO while open\n");

    let path = "/tmp/test_fifo7\0";

    // Create FIFO
    match mkfifo(path, 0o644) {
        Ok(()) => (),
        Err(e) => {
            io::print("  ERROR: mkfifo failed: ");
            print_num(errno_code(e));
            io::print("\n");
            return false;
        }
    }

    // Open both ends
    let read_fd = match open(path, O_RDONLY | O_NONBLOCK) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  ERROR: open for read failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = unlink(path);
            return false;
        }
    };

    let write_fd = match open(path, O_WRONLY) {
        Ok(fd) => fd,
        Err(e) => {
            io::print("  ERROR: open for write failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = close(read_fd);
            let _ = unlink(path);
            return false;
        }
    };

    // Unlink while both ends are open
    match unlink(path) {
        Ok(()) => io::print("  Unlinked FIFO while open\n"),
        Err(e) => {
            io::print("  ERROR: unlink failed: ");
            print_num(errno_code(e));
            io::print("\n");
            let _ = close(read_fd);
            let _ = close(write_fd);
            return false;
        }
    }

    // I/O should still work on open fds
    let data = b"After unlink";
    let write_ret = write(write_fd, data);
    if write_ret < 0 {
        io::print("  ERROR: write after unlink failed: ");
        print_num(-write_ret);
        io::print("\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        return false;
    }
    io::print("  Wrote ");
    print_num(write_ret);
    io::print(" bytes after unlink\n");

    let mut buf = [0u8; 32];
    let read_ret = read(read_fd, &mut buf);
    if read_ret < 0 {
        io::print("  ERROR: read after unlink failed: ");
        print_num(-read_ret);
        io::print("\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        return false;
    }
    io::print("  Read ");
    print_num(read_ret);
    io::print(" bytes after unlink\n");
    if &buf[..read_ret as usize] != data {
        io::print("  ERROR: data mismatch\n");
        let _ = close(read_fd);
        let _ = close(write_fd);
        return false;
    }

    let _ = close(read_fd);
    let _ = close(write_fd);
    io::print("Phase 7: PASSED\n");
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    io::print("=== FIFO (Named Pipe) Test ===\n");

    let mut all_passed = true;

    // Phase 1: Basic create/open/read/write/close
    if !test_basic_fifo() {
        all_passed = false;
    }

    // Phase 2: EEXIST
    if !test_eexist() {
        all_passed = false;
    }

    // Phase 3: ENOENT
    if !test_enoent() {
        all_passed = false;
    }

    // Phase 4: ENXIO on write without reader
    if !test_nonblock_write_no_reader() {
        all_passed = false;
    }

    // Phase 5: EAGAIN on empty read
    if !test_nonblock_read_empty() {
        all_passed = false;
    }

    // Phase 6: Multiple I/O
    if !test_multiple_io() {
        all_passed = false;
    }

    // Phase 7: Unlink while open
    if !test_unlink_while_open() {
        all_passed = false;
    }

    if all_passed {
        io::print("=== All FIFO Tests PASSED ===\n");
        io::print("FIFO_TEST_PASSED\n");
    } else {
        io::print("=== Some FIFO Tests FAILED ===\n");
    }

    process::exit(if all_passed { 0 } else { 1 });
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in FIFO test!\n");
    process::exit(1);
}
