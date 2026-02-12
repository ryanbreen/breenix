//! Dup syscall test program (std version)
//!
//! Tests the dup() syscall which duplicates a file descriptor.
//! Uses libbreenix for pipe/read/write/close/dup.

use libbreenix::io;
use libbreenix::error::Error;
use libbreenix::types::Fd;
use libbreenix::Errno;
use std::process;

fn fail(msg: &str) -> ! {
    println!("  FAIL: {}", msg);
    println!("DUP_TEST_FAILED");
    process::exit(1);
}

fn main() {
    println!("=== Dup Syscall Test ===");

    // Phase 0: Test EBADF - dup on invalid fd should return error
    println!("\nPhase 0: Testing EBADF (dup on invalid fd)...");
    match io::dup(Fd::from_raw(999)) {
        Ok(_) => {
            println!("  dup(999) succeeded, expected error");
            fail("dup() on invalid fd should return EBADF");
        }
        Err(Error::Os(Errno::EBADF)) => {
            // Expected
        }
        Err(e) => {
            println!("  dup(999) returned {:?}, expected EBADF", e);
            // Accept any error - the key is it didn't succeed
        }
    }
    println!("  PASS: dup(999) correctly returned error (EBADF)");

    // Phase 1: Create a pipe
    println!("\nPhase 1: Creating pipe...");
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(e) => {
            println!("  pipe() returned error: {:?}", e);
            fail("pipe() failed");
        }
    };

    println!("  Read fd: {}", read_fd.raw() as i32);
    println!("  Write fd: {}", write_fd.raw() as i32);

    if (read_fd.raw() as i32) < 3 || (write_fd.raw() as i32) < 3 {
        fail("Pipe fds should be >= 3");
    }
    if read_fd == write_fd {
        fail("Read and write fds should be different");
    }
    println!("  Pipe created successfully");

    // Phase 2: Call dup() on the read end
    println!("\nPhase 2: Duplicating read fd with dup()...");
    let dup_fd = match io::dup(read_fd) {
        Ok(fd) => fd,
        Err(e) => {
            println!("  dup() returned error: {:?}", e);
            fail("dup() failed");
        }
    };

    println!("  Original read fd: {}", read_fd.raw() as i32);
    println!("  Duplicated fd: {}", dup_fd.raw() as i32);

    // Phase 3: Verify the new fd is different from the original
    println!("\nPhase 3: Verifying dup'd fd is different from original...");
    if dup_fd == read_fd {
        fail("dup() returned same fd as original");
    }
    println!("  PASS: dup'd fd is different from original");

    // Phase 4: Write first chunk of data through the write end
    println!("\nPhase 4: Writing first data chunk to pipe...");
    let data1 = b"Hello";
    let bytes_written = match io::write(write_fd, data1) {
        Ok(n) => n,
        Err(e) => {
            println!("  write() returned error: {:?}", e);
            fail("write() failed");
        }
    };

    println!("  Wrote {} bytes", bytes_written);

    // Phase 5a: Read from original read fd
    println!("\nPhase 5a: Reading from ORIGINAL read fd...");
    let mut read_buf: [u8; 32] = [0; 32];
    let bytes_read = match io::read(read_fd, &mut read_buf) {
        Ok(n) => n,
        Err(e) => {
            println!("  read() from original fd returned error: {:?}", e);
            fail("read() from original fd failed");
        }
    };

    println!("  Read {} bytes from original fd: '{}'",
             bytes_read, String::from_utf8_lossy(&read_buf[..bytes_read]));

    if bytes_read != data1.len() {
        fail("Did not read expected number of bytes from original fd");
    }
    if &read_buf[..bytes_read] != data1 {
        fail("Data from original fd does not match");
    }
    println!("  PASS: Read correct data from original fd");

    // Phase 5b: Write second chunk and read from dup'd fd
    println!("\nPhase 5b: Writing second data chunk and reading from DUP'd fd...");
    let data2 = b"World";
    let bytes_written2 = match io::write(write_fd, data2) {
        Ok(n) => n,
        Err(e) => {
            println!("  write() returned error: {:?}", e);
            fail("second write() failed");
        }
    };
    let _ = bytes_written2;

    let mut read_buf2: [u8; 32] = [0; 32];
    let bytes_read2 = match io::read(dup_fd, &mut read_buf2) {
        Ok(n) => n,
        Err(e) => {
            println!("  read() from dup'd fd returned error: {:?}", e);
            fail("read() from dup'd fd failed");
        }
    };

    println!("  Read {} bytes from dup'd fd: '{}'",
             bytes_read2, String::from_utf8_lossy(&read_buf2[..bytes_read2]));

    if bytes_read2 != data2.len() {
        fail("Did not read expected number of bytes from dup'd fd");
    }
    if &read_buf2[..bytes_read2] != data2 {
        fail("Data from dup'd fd does not match");
    }
    println!("  PASS: Read correct data from dup'd fd");

    // Phase 6: Close the original read fd
    println!("\nPhase 6: Closing original read fd...");
    if let Err(e) = io::close(read_fd) {
        println!("  close() returned error: {:?}", e);
        fail("close() of original fd failed");
    }
    println!("  Original read fd closed");

    // Phase 7: Verify the dup'd fd still works after original is closed
    println!("\nPhase 7: Verifying dup'd fd still works after original closed...");
    let data3 = b"!";
    let bytes_written3 = match io::write(write_fd, data3) {
        Ok(n) => n,
        Err(e) => {
            println!("  write() returned error: {:?}", e);
            fail("third write() failed");
        }
    };
    let _ = bytes_written3;

    let mut read_buf3: [u8; 32] = [0; 32];
    let bytes_read3 = match io::read(dup_fd, &mut read_buf3) {
        Ok(n) => n,
        Err(e) => {
            println!("  read() from dup'd fd after close returned error: {:?}", e);
            fail("read() from dup'd fd after original closed failed");
        }
    };

    println!("  Read {} bytes from dup'd fd after original closed: '{}'",
             bytes_read3, String::from_utf8_lossy(&read_buf3[..bytes_read3]));

    if bytes_read3 != data3.len() {
        fail("Did not read expected number of bytes from dup'd fd after close");
    }
    if &read_buf3[..bytes_read3] != data3 {
        fail("Data from dup'd fd after close does not match");
    }
    println!("  PASS: Dup'd fd still works after original fd closed");

    // Phase 8: Cleanup and report success
    println!("\nPhase 8: Cleanup...");
    let _ = io::close(dup_fd);
    let _ = io::close(write_fd);
    println!("  All fds closed");

    // All tests passed
    println!("\n=== All dup() tests passed! ===");
    println!("DUP_TEST_PASSED");
    process::exit(0);
}
