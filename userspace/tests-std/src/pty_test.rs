//! PTY (Pseudo-Terminal) Integration Test (std version)
//!
//! Tests the full PTY flow:
//! 1. posix_openpt() creates master fd
//! 2. grantpt() and unlockpt() prepare the slave
//! 3. ptsname_r() gets slave path
//! 4. open() on /dev/pts/N creates slave fd
//! 5. Data flows master -> slave -> master

// O_RDWR | O_NOCTTY for posix_openpt
const O_RDWR: i32 = 0x02;
const O_NOCTTY: i32 = 0x100;

extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname_r(fd: i32, buf: *mut u8, buflen: usize) -> i32;
    fn open(path: *const u8, flags: i32, mode: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

fn main() {
    println!("=== PTY Integration Test ===");

    // Test 0: Error path - grantpt on invalid fd should fail with ENOTTY/EBADF
    println!("Test 0: grantpt(invalid fd) error handling...");
    let ret = unsafe { grantpt(999) };
    if ret == 0 {
        println!("  FAIL: grantpt should reject invalid fd");
        std::process::exit(100);
    } else {
        println!("  grantpt correctly rejected invalid fd");
    }

    // Test 1: posix_openpt with O_RDWR | O_NOCTTY
    println!("Test 1: posix_openpt(O_RDWR | O_NOCTTY)...");
    let master_fd = unsafe { posix_openpt(O_RDWR | O_NOCTTY) };
    if master_fd < 0 {
        println!("  FAIL: posix_openpt failed");
        std::process::exit(1);
    }
    println!("  posix_openpt returned fd");

    // Test 2: grantpt
    println!("Test 2: grantpt()...");
    let ret = unsafe { grantpt(master_fd) };
    if ret != 0 {
        println!("  FAIL: grantpt failed");
        std::process::exit(2);
    }
    println!("  grantpt OK");

    // Test 3: unlockpt
    println!("Test 3: unlockpt()...");
    let ret = unsafe { unlockpt(master_fd) };
    if ret != 0 {
        println!("  FAIL: unlockpt failed");
        std::process::exit(3);
    }
    println!("  unlockpt OK");

    // Test 4: ptsname_r
    println!("Test 4: ptsname()...");
    let mut path_buf = [0u8; 32];
    let ret = unsafe { ptsname_r(master_fd, path_buf.as_mut_ptr(), path_buf.len()) };
    if ret != 0 {
        println!("  FAIL: ptsname failed");
        std::process::exit(4);
    }
    println!("  ptsname returned path");

    // Find the null terminator to get path length
    let _path_len = path_buf.iter().position(|&b| b == 0).unwrap_or(path_buf.len());

    // Test 5: open slave device
    println!("Test 5: open slave device...");
    // path_buf already contains null-terminated path from ptsname_r
    let slave_fd = unsafe { open(path_buf.as_ptr(), O_RDWR, 0) };
    if slave_fd < 0 {
        println!("  FAIL: open slave failed");
        std::process::exit(5);
    }
    println!("  open slave OK");

    // Test 6: Write to master, read from slave - with data verification
    println!("Test 6: master -> slave data flow...");
    let test_msg = b"PTY test message\n";
    let write_len = unsafe { write(master_fd, test_msg.as_ptr(), test_msg.len()) };
    if write_len <= 0 {
        println!("  FAIL: write to master failed");
        std::process::exit(6);
    }
    let write_len = write_len as usize;

    // Read from slave and verify data matches
    let mut buf = [0u8; 64];
    let read_len = unsafe { read(slave_fd, buf.as_mut_ptr(), buf.len()) };
    if read_len <= 0 {
        println!("  FAIL: read from slave failed");
        std::process::exit(7);
    }
    let read_len = read_len as usize;

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
    let write_len2 = unsafe { write(slave_fd, test_msg2.as_ptr(), test_msg2.len()) };
    if write_len2 <= 0 {
        println!("  FAIL: write to slave failed");
        std::process::exit(8);
    }
    let write_len2 = write_len2 as usize;

    // Read from master and verify data matches
    let mut buf2 = [0u8; 64];
    let read_len2 = unsafe { read(master_fd, buf2.as_mut_ptr(), buf2.len()) };
    if read_len2 <= 0 {
        println!("  FAIL: read from master failed");
        std::process::exit(9);
    }
    let read_len2 = read_len2 as usize;

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
    unsafe {
        close(slave_fd);
        close(master_fd);
    }

    println!("=== PTY Test PASSED ===");
    println!("[PTY_TEST_PASSED]");
    std::process::exit(0);
}
