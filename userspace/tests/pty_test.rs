//! PTY (Pseudo-Terminal) Integration Test
//!
//! Tests the full PTY flow:
//! 1. posix_openpt() creates master fd
//! 2. grantpt() and unlockpt() prepare the slave
//! 3. ptsname() gets slave path
//! 4. open() on /dev/pts/N creates slave fd
//! 5. Data flows master -> slave -> master

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::{fs, io, process, pty};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::println("=== PTY Integration Test ===");

    // Test 0: Error path - grantpt on invalid fd should fail with ENOTTY/EBADF
    io::println("Test 0: grantpt(invalid fd) error handling...");
    match pty::grantpt(999) {
        // 999 is not a valid PTY master fd
        Ok(()) => {
            io::println("  FAIL: grantpt should reject invalid fd");
            process::exit(100);
        }
        Err(_e) => {
            io::println("  grantpt correctly rejected invalid fd");
        }
    }

    // Test 1: posix_openpt with O_RDWR | O_NOCTTY
    io::println("Test 1: posix_openpt(O_RDWR | O_NOCTTY)...");
    let master_fd_i32 = match pty::posix_openpt(pty::O_RDWR | pty::O_NOCTTY) {
        Ok(fd) => {
            io::println("  posix_openpt returned fd");
            fd
        }
        Err(_e) => {
            io::println("  FAIL: posix_openpt failed");
            process::exit(1);
        }
    };
    let master_fd = master_fd_i32 as u64;

    // Test 2: grantpt
    io::println("Test 2: grantpt()...");
    if let Err(_e) = pty::grantpt(master_fd_i32) {
        io::println("  FAIL: grantpt failed");
        process::exit(2);
    }
    io::println("  grantpt OK");

    // Test 3: unlockpt
    io::println("Test 3: unlockpt()...");
    if let Err(_e) = pty::unlockpt(master_fd_i32) {
        io::println("  FAIL: unlockpt failed");
        process::exit(3);
    }
    io::println("  unlockpt OK");

    // Test 4: ptsname
    io::println("Test 4: ptsname()...");
    let mut path_buf = [0u8; 32];
    match pty::ptsname(master_fd_i32, &mut path_buf) {
        Ok(_len) => {
            io::println("  ptsname returned path");
        }
        Err(_e) => {
            io::println("  FAIL: ptsname failed");
            process::exit(4);
        }
    }

    // Get the path as slice
    let slave_path = pty::slave_path_bytes(&path_buf);

    // Convert to str for open (including null terminator for syscall)
    // We need to include the null terminator for the syscall
    let path_len = slave_path.len();
    // Safe because slave_path is ASCII only (/dev/pts/N)
    let path_with_null = unsafe { core::str::from_utf8_unchecked(&path_buf[..=path_len]) };

    // Test 5: open slave device
    io::println("Test 5: open slave device...");
    let slave_fd = match fs::open(path_with_null, fs::O_RDWR) {
        Ok(fd) => {
            io::println("  open slave OK");
            fd
        }
        Err(_e) => {
            io::println("  FAIL: open slave failed");
            process::exit(5);
        }
    };

    // Test 6: Write to master, read from slave - with data verification
    io::println("Test 6: master -> slave data flow...");
    let test_msg = b"PTY test message\n";
    let write_len = match fs::write(master_fd, test_msg) {
        Ok(n) if n > 0 => n as usize,
        _ => {
            io::println("  FAIL: write to master failed");
            process::exit(6);
        }
    };

    // Read from slave and verify data matches
    let mut buf = [0u8; 64];
    let read_len = match fs::read(slave_fd, &mut buf) {
        Ok(n) if n > 0 => n as usize,
        _ => {
            io::println("  FAIL: read from slave failed");
            process::exit(7);
        }
    };

    // Verify data integrity: compare what we wrote with what we read
    if read_len != write_len {
        io::println("  FAIL: read length != write length");
        process::exit(10);
    }
    for i in 0..read_len {
        if buf[i] != test_msg[i] {
            io::println("  FAIL: data mismatch at byte");
            process::exit(11);
        }
    }
    io::println("  master -> slave OK (data verified)");

    // Test 7: Write to slave, read from master - with data verification
    io::println("Test 7: slave -> master data flow...");
    let test_msg2 = b"Response from slave\n";
    let write_len2 = match fs::write(slave_fd, test_msg2) {
        Ok(n) if n > 0 => n as usize,
        _ => {
            io::println("  FAIL: write to slave failed");
            process::exit(8);
        }
    };

    // Read from master and verify data matches
    let mut buf2 = [0u8; 64];
    let read_len2 = match fs::read(master_fd, &mut buf2) {
        Ok(n) if n > 0 => n as usize,
        _ => {
            io::println("  FAIL: read from master failed");
            process::exit(9);
        }
    };

    // Verify data integrity: compare what we wrote with what we read
    if read_len2 != write_len2 {
        io::println("  FAIL: read length != write length (slave->master)");
        process::exit(12);
    }
    for i in 0..read_len2 {
        if buf2[i] != test_msg2[i] {
            io::println("  FAIL: data mismatch (slave->master)");
            process::exit(13);
        }
    }
    io::println("  slave -> master OK (data verified)");

    // Close file descriptors
    let _ = io::close(slave_fd);
    let _ = io::close(master_fd);

    io::println("=== PTY Test PASSED ===");
    io::println("[PTY_TEST_PASSED]");
    process::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::println("PTY test panic!");
    process::exit(99);
}
