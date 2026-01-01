//! Dup syscall test program
//!
//! Tests the dup() syscall which duplicates a file descriptor:
//! 1. Create a pipe with pipe()
//! 2. Call dup() on the read end
//! 3. Verify the new fd is different from the original
//! 4. Write data through the write end
//! 5. Read data from BOTH the original and dup'd read fds (to verify both work)
//! 6. Close the original read fd
//! 7. Verify the dup'd fd still works (read more data)
//! 8. Print "DUP_TEST_PASSED" on success

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::types::fd;

/// Print a number to stdout
unsafe fn print_number(num: u64) {
    let mut buffer: [u8; 32] = [0; 32];
    let mut n = num;
    let mut i = 0;

    if n == 0 {
        buffer[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            buffer[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = buffer[j];
            buffer[j] = buffer[i - j - 1];
            buffer[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &buffer[..i]);
}

/// Print signed number
unsafe fn print_signed(num: i64) {
    if num < 0 {
        io::print("-");
        print_number((-num) as u64);
    } else {
        print_number(num as u64);
    }
}

/// Helper to fail with message
fn fail(msg: &str) -> ! {
    io::print("  FAIL: ");
    io::print(msg);
    io::print("\n");
    io::print("DUP_TEST_FAILED\n");
    process::exit(1);
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Dup Syscall Test ===\n");

        // Phase 0: Test EBADF - dup on invalid fd should return -9
        io::print("\nPhase 0: Testing EBADF (dup on invalid fd)...\n");
        let bad_dup = io::dup(999);  // fd 999 doesn't exist
        if bad_dup != -9 {
            io::print("  dup(999) returned ");
            print_signed(bad_dup);
            io::print(", expected -9 (EBADF)\n");
            fail("dup() on invalid fd should return EBADF (-9)");
        }
        io::print("  PASS: dup(999) correctly returned -9 (EBADF)\n");

        // Phase 1: Create a pipe
        io::print("\nPhase 1: Creating pipe...\n");
        let mut pipefd: [i32; 2] = [0, 0];
        let ret = io::pipe(&mut pipefd);

        if ret < 0 {
            io::print("  pipe() returned error: ");
            print_signed(ret);
            io::print("\n");
            fail("pipe() failed");
        }

        let read_fd = pipefd[0] as u64;
        let write_fd = pipefd[1] as u64;

        io::print("  Read fd: ");
        print_number(read_fd);
        io::print("\n  Write fd: ");
        print_number(write_fd);
        io::print("\n");

        // Validate fd numbers
        if read_fd < 3 || write_fd < 3 {
            fail("Pipe fds should be >= 3");
        }
        if read_fd == write_fd {
            fail("Read and write fds should be different");
        }
        io::print("  Pipe created successfully\n");

        // Phase 2: Call dup() on the read end
        io::print("\nPhase 2: Duplicating read fd with dup()...\n");
        let dup_fd = io::dup(read_fd);

        if dup_fd < 0 {
            io::print("  dup() returned error: ");
            print_signed(dup_fd);
            io::print("\n");
            fail("dup() failed");
        }

        io::print("  Original read fd: ");
        print_number(read_fd);
        io::print("\n  Duplicated fd: ");
        print_number(dup_fd as u64);
        io::print("\n");

        // Phase 3: Verify the new fd is different from the original
        io::print("\nPhase 3: Verifying dup'd fd is different from original...\n");
        if dup_fd as u64 == read_fd {
            fail("dup() returned same fd as original");
        }
        io::print("  PASS: dup'd fd is different from original\n");

        // Phase 4: Write first chunk of data through the write end
        io::print("\nPhase 4: Writing first data chunk to pipe...\n");
        let data1 = b"Hello";
        let bytes_written = io::write(write_fd, data1);

        if bytes_written < 0 {
            io::print("  write() returned error: ");
            print_signed(bytes_written);
            io::print("\n");
            fail("write() failed");
        }

        io::print("  Wrote ");
        print_number(bytes_written as u64);
        io::print(" bytes\n");

        // Phase 5a: Read from original read fd
        io::print("\nPhase 5a: Reading from ORIGINAL read fd...\n");
        let mut read_buf: [u8; 32] = [0; 32];
        let bytes_read = io::read(read_fd, &mut read_buf);

        if bytes_read < 0 {
            io::print("  read() from original fd returned error: ");
            print_signed(bytes_read);
            io::print("\n");
            fail("read() from original fd failed");
        }

        io::print("  Read ");
        print_number(bytes_read as u64);
        io::print(" bytes from original fd: '");
        io::write(fd::STDOUT, &read_buf[..bytes_read as usize]);
        io::print("'\n");

        // Verify data
        if bytes_read != data1.len() as i64 {
            fail("Did not read expected number of bytes from original fd");
        }
        if &read_buf[..bytes_read as usize] != data1 {
            fail("Data from original fd does not match");
        }
        io::print("  PASS: Read correct data from original fd\n");

        // Phase 5b: Write second chunk and read from dup'd fd
        io::print("\nPhase 5b: Writing second data chunk and reading from DUP'd fd...\n");
        let data2 = b"World";
        let bytes_written2 = io::write(write_fd, data2);

        if bytes_written2 < 0 {
            io::print("  write() returned error: ");
            print_signed(bytes_written2);
            io::print("\n");
            fail("second write() failed");
        }

        let mut read_buf2: [u8; 32] = [0; 32];
        let bytes_read2 = io::read(dup_fd as u64, &mut read_buf2);

        if bytes_read2 < 0 {
            io::print("  read() from dup'd fd returned error: ");
            print_signed(bytes_read2);
            io::print("\n");
            fail("read() from dup'd fd failed");
        }

        io::print("  Read ");
        print_number(bytes_read2 as u64);
        io::print(" bytes from dup'd fd: '");
        io::write(fd::STDOUT, &read_buf2[..bytes_read2 as usize]);
        io::print("'\n");

        // Verify data
        if bytes_read2 != data2.len() as i64 {
            fail("Did not read expected number of bytes from dup'd fd");
        }
        if &read_buf2[..bytes_read2 as usize] != data2 {
            fail("Data from dup'd fd does not match");
        }
        io::print("  PASS: Read correct data from dup'd fd\n");

        // Phase 6: Close the original read fd
        io::print("\nPhase 6: Closing original read fd...\n");
        let close_ret = io::close(read_fd);

        if close_ret < 0 {
            io::print("  close() returned error: ");
            print_signed(close_ret);
            io::print("\n");
            fail("close() of original fd failed");
        }
        io::print("  Original read fd closed\n");

        // Phase 7: Verify the dup'd fd still works after original is closed
        io::print("\nPhase 7: Verifying dup'd fd still works after original closed...\n");
        let data3 = b"!";
        let bytes_written3 = io::write(write_fd, data3);

        if bytes_written3 < 0 {
            io::print("  write() returned error: ");
            print_signed(bytes_written3);
            io::print("\n");
            fail("third write() failed");
        }

        let mut read_buf3: [u8; 32] = [0; 32];
        let bytes_read3 = io::read(dup_fd as u64, &mut read_buf3);

        if bytes_read3 < 0 {
            io::print("  read() from dup'd fd after close returned error: ");
            print_signed(bytes_read3);
            io::print("\n");
            fail("read() from dup'd fd after original closed failed");
        }

        io::print("  Read ");
        print_number(bytes_read3 as u64);
        io::print(" bytes from dup'd fd after original closed: '");
        io::write(fd::STDOUT, &read_buf3[..bytes_read3 as usize]);
        io::print("'\n");

        // Verify data
        if bytes_read3 != data3.len() as i64 {
            fail("Did not read expected number of bytes from dup'd fd after close");
        }
        if &read_buf3[..bytes_read3 as usize] != data3 {
            fail("Data from dup'd fd after close does not match");
        }
        io::print("  PASS: Dup'd fd still works after original fd closed\n");

        // Phase 8: Cleanup and report success
        io::print("\nPhase 8: Cleanup...\n");
        let close_dup = io::close(dup_fd as u64);
        if close_dup < 0 {
            io::print("  Warning: close(dup_fd) returned ");
            print_signed(close_dup);
            io::print("\n");
        }

        let close_write = io::close(write_fd);
        if close_write < 0 {
            io::print("  Warning: close(write_fd) returned ");
            print_signed(close_write);
            io::print("\n");
        }

        io::print("  All fds closed\n");

        // All tests passed
        io::print("\n=== All dup() tests passed! ===\n");
        io::print("DUP_TEST_PASSED\n");
        process::exit(0);
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in dup test!\n");
    io::print("DUP_TEST_FAILED\n");
    process::exit(255);
}
