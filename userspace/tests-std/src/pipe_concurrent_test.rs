//! Concurrent pipe test program (std version)
//!
//! Tests concurrent access to the pipe buffer from multiple processes.
//! This stress-tests the Arc<Mutex<PipeBuffer>> under concurrent writes.

extern "C" {
    fn fork() -> i32;
    fn getpid() -> i32;
    fn pipe(pipefd: *mut i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn sched_yield() -> i32;
}

/// Number of concurrent writer processes
const NUM_WRITERS: usize = 4;

/// Number of messages each writer sends
const MESSAGES_PER_WRITER: usize = 3;

/// Message size in bytes (including marker and newline)
const MESSAGE_SIZE: usize = 32;

/// Helper to fail with an error message
fn fail(msg: &str) -> ! {
    println!("PIPE_CONCURRENT: FAIL - {}", msg);
    std::process::exit(1);
}

/// Helper to yield CPU multiple times
fn yield_cpu() {
    for _ in 0..10 {
        unsafe { sched_yield(); }
    }
}

/// Child writer process
fn child_writer(writer_id: usize, write_fd: i32) -> ! {
    let pid = unsafe { getpid() };
    println!("  Writer {} started (PID {})", writer_id, pid);

    // Each writer sends MESSAGES_PER_WRITER messages
    // Format: "W[id]M[msg]XXXXXXXXXXXXXXXX\n" (32 bytes total)
    for msg_num in 0..MESSAGES_PER_WRITER {
        let mut msg_buf = [b'X'; MESSAGE_SIZE];
        msg_buf[0] = b'W';
        msg_buf[1] = b'0' + (writer_id as u8);
        msg_buf[2] = b'M';
        msg_buf[3] = b'0' + (msg_num as u8);
        msg_buf[MESSAGE_SIZE - 1] = b'\n';

        let ret = unsafe { write(write_fd, msg_buf.as_ptr(), msg_buf.len()) };
        if ret < 0 {
            println!("  Writer {} FAILED to write message {}", writer_id, msg_num);
            std::process::exit(1);
        }

        if ret != MESSAGE_SIZE as isize {
            println!("  Writer {} wrote {} bytes, expected {}", writer_id, ret, MESSAGE_SIZE);
            std::process::exit(1);
        }

        // Small yield to encourage interleaving
        unsafe { sched_yield(); }
    }

    println!("  Writer {} completed all messages", writer_id);
    std::process::exit(0);
}

fn main() {
    println!("=== Concurrent Pipe Test Program ===");
    println!("Testing concurrent writes from multiple processes");
    println!();

    // Phase 1: Create pipe
    println!("Phase 1: Creating pipe...");
    let mut pipefd: [i32; 2] = [0, 0];
    let ret = unsafe { pipe(pipefd.as_mut_ptr()) };

    if ret < 0 {
        println!("  pipe() returned error: {}", -ret);
        fail("pipe() failed");
    }

    println!("  Read fd: {}", pipefd[0]);
    println!("  Write fd: {}", pipefd[1]);

    // Validate fd numbers
    if pipefd[0] < 3 || pipefd[1] < 3 {
        fail("Pipe fds should be >= 3");
    }
    if pipefd[0] == pipefd[1] {
        fail("Read and write fds should be different");
    }

    println!("  Pipe created successfully\n");

    // Phase 2: Fork multiple writer processes
    println!("Phase 2: Forking {} writer processes...", NUM_WRITERS);

    let mut child_pids = [0i32; NUM_WRITERS];

    for i in 0..NUM_WRITERS {
        let fork_result = unsafe { fork() };

        if fork_result < 0 {
            println!("  fork() failed with error: {}", -fork_result);
            fail("fork() failed");
        }

        if fork_result == 0 {
            // Child process - close read end and become a writer
            let close_ret = unsafe { close(pipefd[0]) };
            if close_ret < 0 {
                fail("child failed to close read fd");
            }
            child_writer(i, pipefd[1]);
            // Never returns
        }

        // Parent: record child PID
        child_pids[i] = fork_result;
        println!("  Forked writer {} with PID {}", i, fork_result);
    }

    println!("  All writers forked\n");

    // Phase 3: Parent closes write end and reads all data
    println!("Phase 3: Parent closing write end and reading data...");
    let close_ret = unsafe { close(pipefd[1]) };
    if close_ret < 0 {
        fail("parent failed to close write fd");
    }
    println!("  Parent closed write fd");

    // Read all messages
    let total_expected_bytes = (NUM_WRITERS * MESSAGES_PER_WRITER * MESSAGE_SIZE) as u64;
    let mut total_read: u64 = 0;
    let mut read_buf = [0u8; MESSAGE_SIZE];

    // Track which messages we received from each writer
    let mut writer_msg_counts = [0u32; NUM_WRITERS];

    println!("  Reading messages (expecting {} bytes)...", total_expected_bytes);

    const MAX_EAGAIN_RETRIES: u32 = 1000;
    let mut consecutive_eagain: u32 = 0;

    loop {
        let ret = unsafe { read(pipefd[0], read_buf.as_mut_ptr(), read_buf.len()) };

        if ret == -11 {
            // EAGAIN - buffer empty but writers still exist
            consecutive_eagain += 1;
            if consecutive_eagain >= MAX_EAGAIN_RETRIES {
                println!("  Too many EAGAIN retries, giving up");
                println!("  Total bytes read so far: {}", total_read);
                fail("read timed out waiting for data");
            }
            yield_cpu();
            continue;
        }

        // Reset counter on successful operation
        consecutive_eagain = 0;

        if ret < 0 {
            println!("  read() failed with error: {}", -ret);
            fail("read from pipe failed");
        }

        if ret == 0 {
            // EOF - all writers have closed
            println!("  EOF reached");
            break;
        }

        if ret != MESSAGE_SIZE as isize {
            println!("  WARNING: Read {} bytes, expected {} bytes per message",
                ret, MESSAGE_SIZE);
        }

        total_read += ret as u64;

        // Validate message format: "W[id]M[msg]..."
        if read_buf[0] != b'W' {
            println!("  Invalid message format: first byte is not 'W'");
            fail("Invalid message format");
        }

        let writer_id = (read_buf[1] - b'0') as usize;
        if writer_id >= NUM_WRITERS {
            println!("  Invalid writer ID: {}", writer_id);
            fail("Writer ID out of range");
        }

        if read_buf[2] != b'M' {
            println!("  Invalid message format: byte 2 is not 'M'");
            fail("Invalid message format");
        }

        // Track this message
        writer_msg_counts[writer_id] += 1;
    }

    println!();
    println!("  Total bytes read: {}", total_read);
    println!("  Expected bytes: {}", total_expected_bytes);

    if total_read != total_expected_bytes {
        fail("Did not read expected number of bytes");
    }

    println!("  Byte count matches!\n");

    // Phase 4: Verify each writer sent the correct number of messages
    println!("Phase 4: Verifying message distribution...");
    let mut all_correct = true;

    for i in 0..NUM_WRITERS {
        println!("  Writer {}: {} messages (expected {})",
            i, writer_msg_counts[i], MESSAGES_PER_WRITER);

        if writer_msg_counts[i] != MESSAGES_PER_WRITER as u32 {
            all_correct = false;
        }
    }

    if !all_correct {
        fail("Message distribution incorrect");
    }

    println!("  Message distribution verified!\n");

    // Phase 5: Close read end
    println!("Phase 5: Closing read fd...");
    let close_ret = unsafe { close(pipefd[0]) };
    if close_ret < 0 {
        fail("parent failed to close read fd");
    }
    println!("  Read fd closed\n");

    // All tests passed
    println!("===========================================");
    println!("PIPE_CONCURRENT: ALL TESTS PASSED");
    println!("===========================================");
    println!("Successfully tested concurrent pipe writes from {} processes", NUM_WRITERS);
    println!("Total messages: {}", NUM_WRITERS * MESSAGES_PER_WRITER);
    println!("Total bytes: {}", total_expected_bytes);
    println!("PIPE_CONCURRENT_TEST_PASSED");

    std::process::exit(0);
}
