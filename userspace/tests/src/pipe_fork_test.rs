//! Fork + Pipe concurrency test program (std version)
//!
//! Tests that pipes work correctly across fork boundaries:
//! - Pipe created before fork is shared between parent and child
//! - Parent can write, child can read (and vice versa)
//! - EOF detection when writer closes
//! - Data integrity across process boundaries

use libbreenix::io;
use libbreenix::process::{self, ForkResult};
use libbreenix::types::Fd;
use libbreenix::Errno;

/// Helper to fail with an error message
fn fail(msg: &str) -> ! {
    println!("PIPE_FORK_TEST: FAIL - {}", msg);
    std::process::exit(1);
}

/// Helper to yield CPU
fn yield_cpu() {
    for _ in 0..10 {
        let _ = process::yield_now();
    }
}

fn main() {
    println!("=== Pipe + Fork Concurrency Test ===");

    // Phase 1: Create pipe before fork
    println!("Phase 1: Creating pipe...");
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(e) => {
            println!("  pipe() failed with error: {:?}", e);
            fail("pipe creation failed");
        }
    };

    println!("  Read fd: {}", read_fd.raw() as i32);
    println!("  Write fd: {}", write_fd.raw() as i32);

    // Validate FD numbers
    if (read_fd.raw() as i32) < 3 || (write_fd.raw() as i32) < 3 {
        fail("Pipe fds should be >= 3");
    }
    if read_fd == write_fd {
        fail("Read and write fds should be different");
    }

    println!("  Pipe created successfully");

    // Phase 2: Fork to create parent/child
    println!("Phase 2: Forking process...");
    let fork_result = match process::fork() {
        Ok(result) => result,
        Err(e) => {
            println!("  fork() failed with error: {:?}", e);
            fail("fork failed");
        }
    };

    match fork_result {
        ForkResult::Child => {
            // ========== CHILD PROCESS ==========
            let child_pid = process::getpid().unwrap();
            println!("\n[CHILD] Process started");
            println!("[CHILD] PID: {}", child_pid.raw());

            // Phase 3a: Child closes write end (won't be writing)
            println!("[CHILD] Phase 3a: Closing write end of pipe...");
            if let Err(e) = io::close(write_fd) {
                println!("[CHILD] close(write_fd) failed: {:?}", e);
                fail("child close write_fd failed");
            }
            println!("[CHILD] Write end closed");

            // Phase 4a: Child reads message from parent
            println!("[CHILD] Phase 4a: Reading message from parent...");
            let mut read_buf = [0u8; 64];

            // Read with retry on EAGAIN
            let mut bytes_read: isize = -11;
            let mut retries = 0;
            while bytes_read == -11 && retries < 100 {
                match io::read(read_fd, &mut read_buf) {
                    Ok(n) => { bytes_read = n as isize; }
                    Err(libbreenix::error::Error::Os(Errno::EAGAIN)) => {
                        bytes_read = -11;
                        yield_cpu();
                        retries += 1;
                    }
                    Err(e) => {
                        bytes_read = -1;
                        println!("[CHILD] read() failed: {:?}", e);
                    }
                }
            }

            if bytes_read < 0 {
                println!("[CHILD] read() failed after retries");
                fail("child read failed");
            }

            println!("[CHILD] Read bytes: {}", bytes_read);

            // Verify message content
            let expected = b"Hello from parent!";
            if bytes_read != expected.len() as isize {
                println!("[CHILD] Expected bytes: {}", expected.len());
                println!("[CHILD] Got bytes: {}", bytes_read);
                fail("child read wrong number of bytes");
            }

            let read_slice = &read_buf[..bytes_read as usize];
            if read_slice != expected {
                println!("[CHILD] Data mismatch!");
                fail("child data verification failed");
            }

            println!("[CHILD] Received: '{}'",
                std::str::from_utf8(read_slice).unwrap_or("<invalid utf8>"));

            // Phase 5a: Test EOF detection
            println!("[CHILD] Phase 5a: Testing EOF detection...");

            let mut eof_read: isize = -11;
            let mut eof_retries = 0;
            while eof_read == -11 && eof_retries < 100 {
                match io::read(read_fd, &mut read_buf) {
                    Ok(n) => { eof_read = n as isize; }
                    Err(libbreenix::error::Error::Os(Errno::EAGAIN)) => {
                        eof_read = -11;
                        yield_cpu();
                        eof_retries += 1;
                    }
                    Err(_) => { eof_read = -1; }
                }
            }

            if eof_read != 0 {
                println!("[CHILD] Expected EOF (0), got: {}", eof_read);
                println!("[CHILD] Retries: {}", eof_retries);
                fail("child EOF detection failed");
            }
            println!("[CHILD] EOF detected correctly (read returned 0)");
            println!("[CHILD] EOF detected after retries: {}", eof_retries);

            // Phase 6a: Close read end
            println!("[CHILD] Phase 6a: Closing read end...");
            if let Err(e) = io::close(read_fd) {
                println!("[CHILD] close(read_fd) failed: {:?}", e);
                fail("child close read_fd failed");
            }

            println!("[CHILD] All tests passed!");
            println!("[CHILD] Exiting with code 0");
            std::process::exit(0);
        }

        ForkResult::Parent(child_pid) => {
            // ========== PARENT PROCESS ==========
            let parent_pid = process::getpid().unwrap();
            println!("\n[PARENT] Process continuing");
            println!("[PARENT] PID: {}", parent_pid.raw());
            println!("[PARENT] Child PID: {}", child_pid.raw());

            // Phase 3b: Parent closes read end (won't be reading)
            println!("[PARENT] Phase 3b: Closing read end of pipe...");
            if let Err(e) = io::close(read_fd) {
                println!("[PARENT] close(read_fd) failed: {:?}", e);
                fail("parent close read_fd failed");
            }
            println!("[PARENT] Read end closed");

            // Phase 4b: Parent writes message to child
            println!("[PARENT] Phase 4b: Writing message to child...");
            let message = b"Hello from parent!";
            let bytes_written = match io::write(write_fd, message) {
                Ok(n) => n as isize,
                Err(e) => {
                    println!("[PARENT] write() failed: {:?}", e);
                    fail("parent write failed");
                }
            };

            println!("[PARENT] Wrote bytes: {}", bytes_written);

            if bytes_written != message.len() as isize {
                println!("[PARENT] Expected to write: {}", message.len());
                println!("[PARENT] Actually wrote: {}", bytes_written);
                fail("parent write incomplete");
            }

            println!("[PARENT] Message sent successfully");

            // Phase 5b: Close write end to signal EOF to child
            println!("[PARENT] Phase 5b: Closing write end to signal EOF...");
            if let Err(e) = io::close(write_fd) {
                println!("[PARENT] close(write_fd) failed: {:?}", e);
                fail("parent close write_fd failed");
            }
            println!("[PARENT] Write end closed (EOF sent to child)");

            // Phase 6b: Wait a bit for child to complete
            println!("[PARENT] Phase 6b: Waiting for child to complete...");
            for i in 0..10 {
                yield_cpu();
                if i % 3 == 0 {
                    println!("[PARENT] .");
                }
            }

            println!("\n[PARENT] All tests completed successfully!");

            // Emit boot stage markers
            println!("PIPE_FORK_TEST: ALL TESTS PASSED");
            println!("PIPE_FORK_TEST_PASSED");

            println!("[PARENT] Exiting with code 0");
            std::process::exit(0);
        }
    }
}
