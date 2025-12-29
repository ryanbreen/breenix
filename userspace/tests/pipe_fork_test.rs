//! Fork + Pipe concurrency test program
//!
//! Tests that pipes work correctly across fork boundaries:
//! - Pipe created before fork is shared between parent and child
//! - Parent can write, child can read (and vice versa)
//! - EOF detection when writer closes
//! - Data integrity across process boundaries

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::types::fd;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    io::print(prefix);

    let mut n = num;
    let mut i = 0;

    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &BUFFER[..i]);
    io::print("\n");
}

/// Helper to exit with error message
fn fail(msg: &str) -> ! {
    io::print("PIPE_FORK_TEST: FAIL - ");
    io::print(msg);
    io::print("\n");
    process::exit(1);
}

/// Helper to yield CPU
fn yield_cpu() {
    for _ in 0..10 {
        process::yield_now();
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Pipe + Fork Concurrency Test ===\n");

        // Phase 1: Create pipe before fork
        io::print("Phase 1: Creating pipe...\n");
        let mut pipefd: [i32; 2] = [0, 0];
        let ret = io::pipe(&mut pipefd);

        if ret < 0 {
            print_number("  pipe() failed with error: ", (-ret) as u64);
            fail("pipe creation failed");
        }

        let read_fd = pipefd[0] as u64;
        let write_fd = pipefd[1] as u64;

        print_number("  Read fd: ", read_fd);
        print_number("  Write fd: ", write_fd);

        // Validate FD numbers
        if read_fd < 3 || write_fd < 3 {
            fail("Pipe fds should be >= 3");
        }
        if read_fd == write_fd {
            fail("Read and write fds should be different");
        }

        io::print("  Pipe created successfully\n");

        // Phase 2: Fork to create parent/child
        io::print("Phase 2: Forking process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            print_number("  fork() failed with error: ", (-fork_result) as u64);
            fail("fork failed");
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("\n[CHILD] Process started\n");
            print_number("[CHILD] PID: ", process::getpid());

            // Phase 3a: Child closes write end (won't be writing)
            io::print("[CHILD] Phase 3a: Closing write end of pipe...\n");
            let ret = io::close(write_fd);
            if ret < 0 {
                print_number("[CHILD] close(write_fd) failed: ", (-ret) as u64);
                fail("child close write_fd failed");
            }
            io::print("[CHILD] Write end closed\n");

            // Phase 4a: Child reads message from parent
            io::print("[CHILD] Phase 4a: Reading message from parent...\n");
            let mut read_buf = [0u8; 64];

            // Read with retry on EAGAIN (error 11 = would block)
            // The pipe is non-blocking, so we need to poll
            let mut bytes_read: i64 = -11; // Start with EAGAIN
            let mut retries = 0;
            while bytes_read == -11 && retries < 100 {
                bytes_read = io::read(read_fd, &mut read_buf);
                if bytes_read == -11 {
                    // EAGAIN - yield and retry
                    yield_cpu();
                    retries += 1;
                }
            }

            if bytes_read < 0 {
                print_number("[CHILD] read() failed after retries: ", (-bytes_read) as u64);
                fail("child read failed");
            }

            print_number("[CHILD] Read bytes: ", bytes_read as u64);

            // Verify message content
            let expected = b"Hello from parent!";
            if bytes_read != expected.len() as i64 {
                print_number("[CHILD] Expected bytes: ", expected.len() as u64);
                print_number("[CHILD] Got bytes: ", bytes_read as u64);
                fail("child read wrong number of bytes");
            }

            let read_slice = &read_buf[..bytes_read as usize];
            if read_slice != expected {
                io::print("[CHILD] Data mismatch!\n");
                io::print("[CHILD] Expected: ");
                io::write(fd::STDOUT, expected);
                io::print("\n[CHILD] Got: ");
                io::write(fd::STDOUT, read_slice);
                io::print("\n");
                fail("child data verification failed");
            }

            io::print("[CHILD] Received: '");
            io::write(fd::STDOUT, read_slice);
            io::print("'\n");

            // Phase 5a: Test EOF detection - read again should get 0 (EOF)
            // The parent needs time to close its write end. Since yield() only sets a flag
            // and doesn't guarantee an immediate context switch (the actual switch happens
            // on timer interrupt), we need to retry on EAGAIN until we get EOF.
            io::print("[CHILD] Phase 5a: Testing EOF detection...\n");

            let mut eof_read: i64 = -11; // Start with EAGAIN
            let mut eof_retries = 0;
            while eof_read == -11 && eof_retries < 100 {
                eof_read = io::read(read_fd, &mut read_buf);
                if eof_read == -11 {
                    // EAGAIN - parent hasn't closed write end yet, yield and retry
                    yield_cpu();
                    eof_retries += 1;
                }
            }

            if eof_read != 0 {
                print_number("[CHILD] Expected EOF (0), got: ", eof_read as u64);
                print_number("[CHILD] Retries: ", eof_retries as u64);
                fail("child EOF detection failed");
            }
            io::print("[CHILD] EOF detected correctly (read returned 0)\n");
            print_number("[CHILD] EOF detected after retries: ", eof_retries as u64);

            // Phase 6a: Close read end
            io::print("[CHILD] Phase 6a: Closing read end...\n");
            let ret = io::close(read_fd);
            if ret < 0 {
                print_number("[CHILD] close(read_fd) failed: ", (-ret) as u64);
                fail("child close read_fd failed");
            }

            io::print("[CHILD] All tests passed!\n");
            io::print("[CHILD] Exiting with code 0\n");
            process::exit(0);

        } else {
            // ========== PARENT PROCESS ==========
            io::print("\n[PARENT] Process continuing\n");
            print_number("[PARENT] PID: ", process::getpid());
            print_number("[PARENT] Child PID: ", fork_result as u64);

            // Phase 3b: Parent closes read end (won't be reading)
            io::print("[PARENT] Phase 3b: Closing read end of pipe...\n");
            let ret = io::close(read_fd);
            if ret < 0 {
                print_number("[PARENT] close(read_fd) failed: ", (-ret) as u64);
                fail("parent close read_fd failed");
            }
            io::print("[PARENT] Read end closed\n");

            // Phase 4b: Parent writes message to child
            io::print("[PARENT] Phase 4b: Writing message to child...\n");
            let message = b"Hello from parent!";
            let bytes_written = io::write(write_fd, message);

            if bytes_written < 0 {
                print_number("[PARENT] write() failed: ", (-bytes_written) as u64);
                fail("parent write failed");
            }

            print_number("[PARENT] Wrote bytes: ", bytes_written as u64);

            if bytes_written != message.len() as i64 {
                print_number("[PARENT] Expected to write: ", message.len() as u64);
                print_number("[PARENT] Actually wrote: ", bytes_written as u64);
                fail("parent write incomplete");
            }

            io::print("[PARENT] Message sent successfully\n");

            // Phase 5b: Close write end to signal EOF to child
            io::print("[PARENT] Phase 5b: Closing write end to signal EOF...\n");
            let ret = io::close(write_fd);
            if ret < 0 {
                print_number("[PARENT] close(write_fd) failed: ", (-ret) as u64);
                fail("parent close write_fd failed");
            }
            io::print("[PARENT] Write end closed (EOF sent to child)\n");

            // Phase 6b: Wait a bit for child to complete
            io::print("[PARENT] Phase 6b: Waiting for child to complete...\n");
            for i in 0..10 {
                yield_cpu();
                if i % 3 == 0 {
                    io::print("[PARENT] .\n");
                }
            }

            io::print("\n[PARENT] All tests completed successfully!\n");

            // Emit boot stage markers
            io::print("PIPE_FORK_TEST: ALL TESTS PASSED\n");
            io::print("PIPE_FORK_TEST_PASSED\n");

            io::print("[PARENT] Exiting with code 0\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in pipe_fork_test!\n");
    process::exit(255);
}
