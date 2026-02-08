//! Dup syscall test program (std version)
//!
//! Tests the dup() syscall which duplicates a file descriptor.
//! Uses FFI for pipe/read/write/close/dup.

use std::process;

extern "C" {
    fn pipe(pipefd: *mut i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn dup(oldfd: i32) -> i32;
}

fn fail(msg: &str) -> ! {
    println!("  FAIL: {}", msg);
    println!("DUP_TEST_FAILED");
    process::exit(1);
}

fn main() {
    println!("=== Dup Syscall Test ===");

    // Phase 0: Test EBADF - dup on invalid fd should return -1 (with errno EBADF)
    println!("\nPhase 0: Testing EBADF (dup on invalid fd)...");
    let bad_dup = unsafe { dup(999) };
    if bad_dup != -1 {
        // libc dup returns -1 on error; the no_std version returns -9 (raw -EBADF)
        // Accept either convention
        if bad_dup != -9 {
            println!("  dup(999) returned {}, expected -1 or -9 (EBADF)", bad_dup);
            fail("dup() on invalid fd should return EBADF");
        }
    }
    println!("  PASS: dup(999) correctly returned error (EBADF)");

    // Phase 1: Create a pipe
    println!("\nPhase 1: Creating pipe...");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        println!("  pipe() returned error: {}", ret);
        fail("pipe() failed");
    }

    let read_fd = pipefd[0];
    let write_fd = pipefd[1];

    println!("  Read fd: {}", read_fd);
    println!("  Write fd: {}", write_fd);

    if read_fd < 3 || write_fd < 3 {
        fail("Pipe fds should be >= 3");
    }
    if read_fd == write_fd {
        fail("Read and write fds should be different");
    }
    println!("  Pipe created successfully");

    // Phase 2: Call dup() on the read end
    println!("\nPhase 2: Duplicating read fd with dup()...");
    let dup_fd = unsafe { dup(read_fd) };

    if dup_fd < 0 {
        println!("  dup() returned error: {}", dup_fd);
        fail("dup() failed");
    }

    println!("  Original read fd: {}", read_fd);
    println!("  Duplicated fd: {}", dup_fd);

    // Phase 3: Verify the new fd is different from the original
    println!("\nPhase 3: Verifying dup'd fd is different from original...");
    if dup_fd == read_fd {
        fail("dup() returned same fd as original");
    }
    println!("  PASS: dup'd fd is different from original");

    // Phase 4: Write first chunk of data through the write end
    println!("\nPhase 4: Writing first data chunk to pipe...");
    let data1 = b"Hello";
    let bytes_written = unsafe { write(write_fd, data1.as_ptr(), data1.len()) };

    if bytes_written < 0 {
        println!("  write() returned error: {}", bytes_written);
        fail("write() failed");
    }

    println!("  Wrote {} bytes", bytes_written);

    // Phase 5a: Read from original read fd
    println!("\nPhase 5a: Reading from ORIGINAL read fd...");
    let mut read_buf: [u8; 32] = [0; 32];
    let bytes_read = unsafe { read(read_fd, read_buf.as_mut_ptr(), read_buf.len()) };

    if bytes_read < 0 {
        println!("  read() from original fd returned error: {}", bytes_read);
        fail("read() from original fd failed");
    }

    println!("  Read {} bytes from original fd: '{}'",
             bytes_read, String::from_utf8_lossy(&read_buf[..bytes_read as usize]));

    if bytes_read != data1.len() as isize {
        fail("Did not read expected number of bytes from original fd");
    }
    if &read_buf[..bytes_read as usize] != data1 {
        fail("Data from original fd does not match");
    }
    println!("  PASS: Read correct data from original fd");

    // Phase 5b: Write second chunk and read from dup'd fd
    println!("\nPhase 5b: Writing second data chunk and reading from DUP'd fd...");
    let data2 = b"World";
    let bytes_written2 = unsafe { write(write_fd, data2.as_ptr(), data2.len()) };

    if bytes_written2 < 0 {
        println!("  write() returned error: {}", bytes_written2);
        fail("second write() failed");
    }

    let mut read_buf2: [u8; 32] = [0; 32];
    let bytes_read2 = unsafe { read(dup_fd, read_buf2.as_mut_ptr(), read_buf2.len()) };

    if bytes_read2 < 0 {
        println!("  read() from dup'd fd returned error: {}", bytes_read2);
        fail("read() from dup'd fd failed");
    }

    println!("  Read {} bytes from dup'd fd: '{}'",
             bytes_read2, String::from_utf8_lossy(&read_buf2[..bytes_read2 as usize]));

    if bytes_read2 != data2.len() as isize {
        fail("Did not read expected number of bytes from dup'd fd");
    }
    if &read_buf2[..bytes_read2 as usize] != data2 {
        fail("Data from dup'd fd does not match");
    }
    println!("  PASS: Read correct data from dup'd fd");

    // Phase 6: Close the original read fd
    println!("\nPhase 6: Closing original read fd...");
    let close_ret = unsafe { close(read_fd) };

    if close_ret < 0 {
        println!("  close() returned error: {}", close_ret);
        fail("close() of original fd failed");
    }
    println!("  Original read fd closed");

    // Phase 7: Verify the dup'd fd still works after original is closed
    println!("\nPhase 7: Verifying dup'd fd still works after original closed...");
    let data3 = b"!";
    let bytes_written3 = unsafe { write(write_fd, data3.as_ptr(), data3.len()) };

    if bytes_written3 < 0 {
        println!("  write() returned error: {}", bytes_written3);
        fail("third write() failed");
    }

    let mut read_buf3: [u8; 32] = [0; 32];
    let bytes_read3 = unsafe { read(dup_fd, read_buf3.as_mut_ptr(), read_buf3.len()) };

    if bytes_read3 < 0 {
        println!("  read() from dup'd fd after close returned error: {}", bytes_read3);
        fail("read() from dup'd fd after original closed failed");
    }

    println!("  Read {} bytes from dup'd fd after original closed: '{}'",
             bytes_read3, String::from_utf8_lossy(&read_buf3[..bytes_read3 as usize]));

    if bytes_read3 != data3.len() as isize {
        fail("Did not read expected number of bytes from dup'd fd after close");
    }
    if &read_buf3[..bytes_read3 as usize] != data3 {
        fail("Data from dup'd fd after close does not match");
    }
    println!("  PASS: Dup'd fd still works after original fd closed");

    // Phase 8: Cleanup and report success
    println!("\nPhase 8: Cleanup...");
    unsafe {
        close(dup_fd);
        close(write_fd);
    }
    println!("  All fds closed");

    // All tests passed
    println!("\n=== All dup() tests passed! ===");
    println!("DUP_TEST_PASSED");
    process::exit(0);
}
