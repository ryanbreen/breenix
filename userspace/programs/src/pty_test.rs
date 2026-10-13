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

fn ioctl_u32(fd: Fd, request: u64, value: &mut u32) -> i64 {
    unsafe { libbreenix::syscall::raw::syscall3(
        libbreenix::syscall::nr::IOCTL, fd.raw(), request, value as *mut u32 as u64,
    ) as i64 }
}

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

    // Positive ioctl checks use the caller's mapped userspace stack.
    let mut number = u32::MAX;
    let mut locked = u32::MAX;
    if ioctl_u32(master_fd, 0x80045430, &mut number) != 0 || number >= 256
        || ioctl_u32(master_fd, 0x80045439, &mut locked) != 0 || locked != 1 {
        println!("FAIL: PTY number or initial lock ioctl");
        std::process::exit(101);
    }
    let mut unlock = 0;
    if ioctl_u32(master_fd, 0x40045431, &mut unlock) != 0
        || ioctl_u32(master_fd, 0x80045439, &mut locked) != 0 || locked != 0 {
        println!("FAIL: PTY unlock ioctl did not change lock state");
        std::process::exit(102);
    }
    let mut lock = 1;
    if ioctl_u32(master_fd, 0x40045431, &mut lock) != 0
        || ioctl_u32(master_fd, 0x80045439, &mut locked) != 0 || locked != 1 {
        println!("FAIL: PTY relock ioctl did not change lock state");
        std::process::exit(103);
    }
    for request in [0x80045430, 0x80045439, 0x40045431, 0x5413, 0x5414] {
        let ret = unsafe { libbreenix::syscall::raw::syscall3(
            libbreenix::syscall::nr::IOCTL, master_fd.raw(), request, u64::MAX,
        ) as i64 };
        if ret != -14 {
            println!("FAIL: PTY ioctl accepted an invalid user pointer");
            std::process::exit(104);
        }
    }

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

    for request in [0x80045430, 0x80045439, 0x40045431] {
        let mut value = u32::MAX;
        if ioctl_u32(slave_fd, request, &mut value) != -25 || value != u32::MAX {
            println!("FAIL: master-only PTY ioctl accepted on slave");
            std::process::exit(105);
        }
    }

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

    // A read to an address outside user space takes no input (Linux checks
    // the range before the terminal is touched), including for NULL.
    for bad in [0, u64::MAX] {
        let ret = unsafe { libbreenix::syscall::raw::syscall3(
            libbreenix::syscall::nr::READ, slave_fd.raw(), bad, 64,
        ) as i64 };
        if ret != -14 {
            println!("FAIL: bad slave read did not return EFAULT");
            std::process::exit(106);
        }
    }
    // A huge requested count must return available bytes using bounded storage.
    let mut buf = [0u8; 64];
    let ret = unsafe { libbreenix::syscall::raw::syscall3(
        libbreenix::syscall::nr::READ, slave_fd.raw(), buf.as_mut_ptr() as u64, u64::MAX,
    ) as i64 };
    let read_len = if ret > 0 && ret <= buf.len() as i64 { ret as usize } else {
        println!("FAIL: SIZE_MAX slave read did not return available input");
        std::process::exit(107);
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

    // Default ECHO and OPOST|ONLCR produce this output from the input above.
    let mut echo = [0u8; 64];
    let echoed = io::read(master_fd, &mut echo).unwrap_or(0);
    if &echo[..echoed] != b"PTY test message\r\n" {
        println!("FAIL: default echo was not CRLF-translated");
        std::process::exit(108);
    }

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

    // The default output mode translates the slave's newline to CRLF.
    let expected = b"Response from slave\r\n";
    let mut buf2 = [0u8; 64];
    for bad in [0, u64::MAX] {
        let ret = unsafe { libbreenix::syscall::raw::syscall3(
            libbreenix::syscall::nr::READ, master_fd.raw(), bad, 64,
        ) as i64 };
        if ret != -14 {
            println!("FAIL: bad master read did not return EFAULT");
            std::process::exit(109);
        }
    }
    let ret = unsafe { libbreenix::syscall::raw::syscall3(
        libbreenix::syscall::nr::READ, master_fd.raw(), buf2.as_mut_ptr() as u64, u64::MAX,
    ) as i64 };
    if write_len2 != test_msg2.len() || ret != expected.len() as i64
        || &buf2[..expected.len()] != expected {
        println!("FAIL: slave output lost data or did not apply ONLCR");
        std::process::exit(12);
    }
    println!("  slave -> master OK (data verified)");

    // Close file descriptors
    let _ = io::close(slave_fd);
    let _ = io::close(master_fd);

    println!("=== PTY Test PASSED ===");
    println!("[PTY_TEST_PASSED]");
    std::process::exit(0);
}
