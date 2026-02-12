//! PTY (Pseudo-Terminal) Integration Test (std version)
//!
//! Tests the full PTY flow:
//! 1. posix_openpt() creates master fd
//! 2. grantpt() and unlockpt() prepare the slave
//! 3. ptsname() gets slave path
//! 4. open() on /dev/pts/N creates slave fd
//! 5. Data flows master -> slave -> master

use libbreenix::io;
use libbreenix::pty::{self, O_RDWR, O_NOCTTY};
use libbreenix::types::Fd;

fn main() {
    println!("=== PTY Integration Test ===");

    // Test 0: Error path - grantpt on invalid fd should fail with ENOTTY/EBADF
    println!("Test 0: grantpt(invalid fd) error handling...");
    let ret = pty::grantpt(Fd::from_raw(999));
    if ret.is_ok() {
        println!("  FAIL: grantpt should reject invalid fd");
        std::process::exit(100);
    } else {
        println!("  grantpt correctly rejected invalid fd");
    }

    // Test 1: posix_openpt with O_RDWR | O_NOCTTY
    println!("Test 1: posix_openpt(O_RDWR | O_NOCTTY)...");
    let master_fd = match pty::posix_openpt(O_RDWR | O_NOCTTY) {
        Ok(fd) => fd,
        Err(_) => {
            println!("  FAIL: posix_openpt failed");
            std::process::exit(1);
        }
    };
    println!("  posix_openpt returned fd");

    // Test 2: grantpt
    println!("Test 2: grantpt()...");
    if pty::grantpt(master_fd).is_err() {
        println!("  FAIL: grantpt failed");
        std::process::exit(2);
    }
    println!("  grantpt OK");

    // Test 3: unlockpt
    println!("Test 3: unlockpt()...");
    if pty::unlockpt(master_fd).is_err() {
        println!("  FAIL: unlockpt failed");
        std::process::exit(3);
    }
    println!("  unlockpt OK");

    // Test 4: ptsname
    println!("Test 4: ptsname()...");
    let mut path_buf = [0u8; 32];
    if pty::ptsname(master_fd, &mut path_buf).is_err() {
        println!("  FAIL: ptsname failed");
        std::process::exit(4);
    }
    println!("  ptsname returned path");

    // Test 5: open slave device
    println!("Test 5: open slave device...");
    // path_buf contains null-terminated path from ptsname
    // Convert to &str for libbreenix::fs::open (needs null-terminated string)
    let path_len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());
    let slave_path = unsafe { core::str::from_utf8_unchecked(&path_buf[..path_len + 1]) };
    let slave_fd = match libbreenix::fs::open(slave_path, libbreenix::fs::O_RDWR) {
        Ok(fd) => fd,
        Err(_) => {
            println!("  FAIL: open slave failed");
            std::process::exit(5);
        }
    };
    println!("  open slave OK");

    // Test 6: Write to master, read from slave - with data verification
    println!("Test 6: master -> slave data flow...");
    let test_msg = b"PTY test message\n";
    let write_len = match io::write(master_fd, test_msg) {
        Ok(n) if n > 0 => n,
        _ => {
            println!("  FAIL: write to master failed");
            std::process::exit(6);
        }
    };

    // Read from slave and verify data matches
    let mut buf = [0u8; 64];
    let read_len = match io::read(slave_fd, &mut buf) {
        Ok(n) if n > 0 => n,
        _ => {
            println!("  FAIL: read from slave failed");
            std::process::exit(7);
        }
    };

    // Verify data integrity: compare what we wrote with what we read
    if read_len != write_len {
        println!("  FAIL: read length != write length");
        std::process::exit(10);
    }
    for i in 0..read_len {
        if buf[i] != test_msg[i] {
            println!("  FAIL: data mismatch at byte");
            std::process::exit(11);
        }
    }
    println!("  master -> slave OK (data verified)");

    // Test 7: Write to slave, read from master - with data verification
    println!("Test 7: slave -> master data flow...");
    let test_msg2 = b"Response from slave\n";
    let write_len2 = match io::write(slave_fd, test_msg2) {
        Ok(n) if n > 0 => n,
        _ => {
            println!("  FAIL: write to slave failed");
            std::process::exit(8);
        }
    };

    // Read from master and verify data matches
    let mut buf2 = [0u8; 64];
    let read_len2 = match io::read(master_fd, &mut buf2) {
        Ok(n) if n > 0 => n,
        _ => {
            println!("  FAIL: read from master failed");
            std::process::exit(9);
        }
    };

    // Verify data integrity: compare what we wrote with what we read
    if read_len2 != write_len2 {
        println!("  FAIL: read length != write length (slave->master)");
        std::process::exit(12);
    }
    for i in 0..read_len2 {
        if buf2[i] != test_msg2[i] {
            println!("  FAIL: data mismatch (slave->master)");
            std::process::exit(13);
        }
    }
    println!("  slave -> master OK (data verified)");

    // Close file descriptors
    let _ = io::close(slave_fd);
    let _ = io::close(master_fd);

    println!("=== PTY Test PASSED ===");
    println!("[PTY_TEST_PASSED]");
    std::process::exit(0);
}
