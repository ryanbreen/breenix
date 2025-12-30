//! Concurrent pipe test program
//!
//! Tests concurrent access to the pipe buffer from multiple processes.
//! This stress-tests the Arc<Mutex<PipeBuffer>> under concurrent writes.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::types::fd;

/// Number of concurrent writer processes
const NUM_WRITERS: usize = 4;

/// Number of messages each writer sends
const MESSAGES_PER_WRITER: usize = 3;

/// Message size in bytes (including marker and newline)
const MESSAGE_SIZE: usize = 32;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 64] = [0; 64];

/// Convert number to string and write it
unsafe fn write_num(n: u64) {
    let mut num = n;
    let mut i = 0;

    if num == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while num > 0 {
            BUFFER[i] = b'0' + (num % 10) as u8;
            num /= 10;
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
}

/// Print a number with prefix
unsafe fn print_number(prefix: &str, num: u64) {
    io::print(prefix);
    write_num(num);
    io::print("\n");
}

/// Helper to fail with an error message
fn fail(msg: &str) -> ! {
    io::print("PIPE_CONCURRENT: FAIL - ");
    io::print(msg);
    io::print("\n");
    process::exit(1);
}

/// Helper to yield CPU multiple times
fn yield_cpu() {
    for _ in 0..10 {
        process::yield_now();
    }
}

/// Child writer process
unsafe fn child_writer(writer_id: u64, write_fd: i32) -> ! {
    io::print("  Writer ");
    write_num(writer_id);
    io::print(" started (PID ");
    write_num(process::getpid());
    io::print(")\n");

    // Each writer sends MESSAGES_PER_WRITER messages
    // Format: "W[id]M[msg]XXXXXXXXXXXXXXXX\n" (32 bytes total)
    for msg_num in 0..MESSAGES_PER_WRITER {
        // Build message: "W[id]M[msg]" followed by padding and newline
        let mut msg_buf = [b'X'; MESSAGE_SIZE];
        msg_buf[0] = b'W';
        msg_buf[1] = b'0' + (writer_id as u8);
        msg_buf[2] = b'M';
        msg_buf[3] = b'0' + (msg_num as u8);
        msg_buf[MESSAGE_SIZE - 1] = b'\n';

        // Write to pipe
        let ret = io::write(write_fd as u64, &msg_buf);
        if ret < 0 {
            io::print("  Writer ");
            write_num(writer_id);
            io::print(" FAILED to write message ");
            write_num(msg_num as u64);
            io::print("\n");
            process::exit(1);
        }

        if ret != MESSAGE_SIZE as i64 {
            io::print("  Writer ");
            write_num(writer_id);
            io::print(" wrote ");
            write_num(ret as u64);
            io::print(" bytes, expected ");
            write_num(MESSAGE_SIZE as u64);
            io::print("\n");
            process::exit(1);
        }

        // Small yield to encourage interleaving
        process::yield_now();
    }

    io::print("  Writer ");
    write_num(writer_id);
    io::print(" completed all messages\n");
    process::exit(0);
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("=== Concurrent Pipe Test Program ===\n");
    io::print("Testing concurrent writes from multiple processes\n");
    io::print("\n");

    unsafe {
        // Phase 1: Create pipe
        io::print("Phase 1: Creating pipe...\n");
        let mut pipefd: [i32; 2] = [0, 0];
        let ret = io::pipe(&mut pipefd);

        if ret < 0 {
            print_number("  pipe() returned error: ", (-ret) as u64);
            fail("pipe() failed");
        }

        print_number("  Read fd: ", pipefd[0] as u64);
        print_number("  Write fd: ", pipefd[1] as u64);

        // Validate fd numbers
        if pipefd[0] < 3 || pipefd[1] < 3 {
            fail("Pipe fds should be >= 3");
        }
        if pipefd[0] == pipefd[1] {
            fail("Read and write fds should be different");
        }

        io::print("  Pipe created successfully\n\n");

        // Phase 2: Fork multiple writer processes
        io::print("Phase 2: Forking ");
        write_num(NUM_WRITERS as u64);
        io::print(" writer processes...\n");

        let mut child_pids: [i64; NUM_WRITERS] = [0; NUM_WRITERS];

        for i in 0..NUM_WRITERS {
            let fork_result = process::fork();

            if fork_result < 0 {
                print_number("  fork() failed with error: ", (-fork_result) as u64);
                fail("fork() failed");
            }

            if fork_result == 0 {
                // Child process - close read end and become a writer
                let close_ret = io::close(pipefd[0] as u64);
                if close_ret < 0 {
                    fail("child failed to close read fd");
                }
                child_writer(i as u64, pipefd[1]);
                // Never returns
            }

            // Parent: record child PID
            child_pids[i] = fork_result;
            io::print("  Forked writer ");
            write_num(i as u64);
            io::print(" with PID ");
            write_num(fork_result as u64);
            io::print("\n");
        }

        io::print("  All writers forked\n\n");

        // Phase 3: Parent closes write end and reads all data
        io::print("Phase 3: Parent closing write end and reading data...\n");
        let close_ret = io::close(pipefd[1] as u64);
        if close_ret < 0 {
            fail("parent failed to close write fd");
        }
        io::print("  Parent closed write fd\n");

        // Read all messages
        // Total expected: NUM_WRITERS * MESSAGES_PER_WRITER messages of MESSAGE_SIZE bytes each
        let total_expected_bytes = (NUM_WRITERS * MESSAGES_PER_WRITER * MESSAGE_SIZE) as u64;
        let mut total_read: u64 = 0;
        let mut read_buf = [0u8; MESSAGE_SIZE];

        // Track which messages we received from each writer
        let mut writer_msg_counts = [0u32; NUM_WRITERS];

        io::print("  Reading messages (expecting ");
        write_num(total_expected_bytes);
        io::print(" bytes)...\n");

        // Maximum retries on EAGAIN before giving up
        const MAX_EAGAIN_RETRIES: u32 = 1000;
        let mut consecutive_eagain: u32 = 0;

        loop {
            let ret = io::read(pipefd[0] as u64, &mut read_buf);

            if ret == -11 {
                // EAGAIN - buffer empty but writers still exist
                // Yield and retry
                consecutive_eagain += 1;
                if consecutive_eagain >= MAX_EAGAIN_RETRIES {
                    io::print("  Too many EAGAIN retries, giving up\n");
                    print_number("  Total bytes read so far: ", total_read);
                    fail("read timed out waiting for data");
                }
                yield_cpu();
                continue;
            }

            // Reset counter on successful operation
            consecutive_eagain = 0;

            if ret < 0 {
                print_number("  read() failed with error: ", (-ret) as u64);
                fail("read from pipe failed");
            }

            if ret == 0 {
                // EOF - all writers have closed
                io::print("  EOF reached\n");
                break;
            }

            if ret != MESSAGE_SIZE as i64 {
                io::print("  WARNING: Read ");
                write_num(ret as u64);
                io::print(" bytes, expected ");
                write_num(MESSAGE_SIZE as u64);
                io::print(" bytes per message\n");
            }

            total_read += ret as u64;

            // Validate message format: "W[id]M[msg]..."
            if read_buf[0] != b'W' {
                io::print("  Invalid message format: first byte is not 'W'\n");
                io::print("  Got: ");
                io::write(fd::STDOUT, &read_buf[..ret as usize]);
                io::print("\n");
                fail("Invalid message format");
            }

            let writer_id = (read_buf[1] - b'0') as usize;
            if writer_id >= NUM_WRITERS {
                io::print("  Invalid writer ID: ");
                write_num(writer_id as u64);
                io::print("\n");
                fail("Writer ID out of range");
            }

            if read_buf[2] != b'M' {
                io::print("  Invalid message format: byte 2 is not 'M'\n");
                fail("Invalid message format");
            }

            // Track this message
            writer_msg_counts[writer_id] += 1;
        }

        io::print("\n");
        print_number("  Total bytes read: ", total_read);
        print_number("  Expected bytes: ", total_expected_bytes);

        if total_read != total_expected_bytes {
            fail("Did not read expected number of bytes");
        }

        io::print("  Byte count matches!\n\n");

        // Phase 4: Verify each writer sent the correct number of messages
        io::print("Phase 4: Verifying message distribution...\n");
        let mut all_correct = true;

        for i in 0..NUM_WRITERS {
            io::print("  Writer ");
            write_num(i as u64);
            io::print(": ");
            write_num(writer_msg_counts[i] as u64);
            io::print(" messages (expected ");
            write_num(MESSAGES_PER_WRITER as u64);
            io::print(")\n");

            if writer_msg_counts[i] != MESSAGES_PER_WRITER as u32 {
                all_correct = false;
            }
        }

        if !all_correct {
            fail("Message distribution incorrect");
        }

        io::print("  Message distribution verified!\n\n");

        // Phase 5: Close read end
        io::print("Phase 5: Closing read fd...\n");
        let close_ret = io::close(pipefd[0] as u64);
        if close_ret < 0 {
            fail("parent failed to close read fd");
        }
        io::print("  Read fd closed\n\n");

        // All tests passed
        io::print("===========================================\n");
        io::print("PIPE_CONCURRENT: ALL TESTS PASSED\n");
        io::print("===========================================\n");
        io::print("Successfully tested concurrent pipe writes from ");
        write_num(NUM_WRITERS as u64);
        io::print(" processes\n");
        io::print("Total messages: ");
        write_num((NUM_WRITERS * MESSAGES_PER_WRITER) as u64);
        io::print("\n");
        io::print("Total bytes: ");
        write_num(total_expected_bytes);
        io::print("\n");
        io::print("PIPE_CONCURRENT_TEST_PASSED\n");

        process::exit(0);
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in pipe concurrent test!\n");
    process::exit(255);
}
