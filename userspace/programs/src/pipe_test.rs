//! Pipe syscall test program (std version)
//!
//! Tests the pipe() and close() syscalls for IPC.
//! Uses libbreenix for pipe/read/write/close.

use libbreenix::syscall::{nr, raw::syscall3};
use libbreenix::{io, time};
use std::process;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// `ioctl` request that holds the caller for some milliseconds the next time
/// a pipe read wakes it, between the wake and the read's retry, with
/// preemption disabled (testing kernels, kernel/src/syscall/handlers.rs).
const PIPE_WAKE_HOLD: u64 = 0xB813_0005;
const SCHED_FIFO: u64 = 1;
#[cfg(target_arch = "aarch64")]
const SCHED_SETAFFINITY: u64 = 122;
#[cfg(target_arch = "x86_64")]
const SCHED_SETAFFINITY: u64 = 203;
const EINVAL: i64 = 22;
/// The reader's processor, and the writer's.
const READER_CPU: u32 = 1;
const WRITER_CPU: u32 = 0;

static BOOSTED: AtomicBool = AtomicBool::new(false);
static BOOSTED_TID: AtomicU64 = AtomicU64::new(0);
static BOOSTED_RAN_AT: AtomicU64 = AtomicU64::new(0);

fn fail(msg: &str) -> ! {
    println!("USERSPACE PIPE: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== Pipe Test Program ===");

    // Phase 1: Create a pipe
    println!("Phase 1: Creating pipe with pipe()...");
    let (read_fd, write_fd) = match io::pipe() {
        Ok(fds) => fds,
        Err(e) => {
            println!("  pipe() returned error: {:?}", e);
            fail("pipe() failed");
        }
    };

    println!("  Pipe created successfully");
    println!("  Read fd: {}", read_fd.raw() as i32);
    println!("  Write fd: {}", write_fd.raw() as i32);

    // Validate fd numbers are reasonable (should be >= 3 after stdin/stdout/stderr)
    if (read_fd.raw() as i32) < 3 || (write_fd.raw() as i32) < 3 {
        fail("Pipe fds should be >= 3 (after stdin/stdout/stderr)");
    }
    if read_fd == write_fd {
        fail("Read and write fds should be different");
    }
    println!("  FD numbers are valid");

    // Phase 2: Write data to pipe
    println!("Phase 2: Writing data to pipe...");
    let test_data = b"Hello, Pipe!";
    let write_ret = match io::write(write_fd, test_data) {
        Ok(n) => n as isize,
        Err(e) => {
            println!("  write() returned error: {:?}", e);
            fail("write to pipe failed");
        }
    };

    println!("  Wrote {} bytes to pipe", write_ret);

    if write_ret != test_data.len() as isize {
        fail("Did not write expected number of bytes");
    }

    // Phase 3: Read data from pipe
    println!("Phase 3: Reading data from pipe...");
    let mut read_buf = [0u8; 32];
    let read_ret = match io::read(read_fd, &mut read_buf) {
        Ok(n) => n as isize,
        Err(e) => {
            println!("  read() returned error: {:?}", e);
            fail("read from pipe failed");
        }
    };

    println!("  Read {} bytes from pipe", read_ret);

    if read_ret != test_data.len() as isize {
        fail("Did not read expected number of bytes");
    }

    // Phase 4: Verify data matches
    println!("Phase 4: Verifying data...");
    let read_slice = &read_buf[..read_ret as usize];

    if read_slice != test_data {
        println!("  Data mismatch!");
        println!("  Expected: {}", String::from_utf8_lossy(test_data));
        println!("  Got: {}", String::from_utf8_lossy(read_slice));
        fail("Data verification failed");
    }

    println!("  Data verified: '{}'", String::from_utf8_lossy(read_slice));

    // Phase 5: Close the pipe ends
    println!("Phase 5: Closing pipe file descriptors...");

    if let Err(e) = io::close(read_fd) {
        println!("  close(read_fd) returned error: {:?}", e);
        fail("close(read_fd) failed");
    }
    println!("  Closed read fd");

    if let Err(e) = io::close(write_fd) {
        println!("  close(write_fd) returned error: {:?}", e);
        fail("close(write_fd) failed");
    }
    println!("  Closed write fd");

    // Phase 6: a reschedule requested as a pipe wait ends
    println!("Phase 6: Rescheduling after a pipe wait...");
    match reschedule_after_pipe_wait() {
        Ok(true) => println!("  The SCHED_FIFO thread ran before the read returned"),
        Ok(false) => println!("  Skipped: one processor"),
        Err(message) => {
            println!("  {}", message);
            fail("a reschedule requested as a pipe wait ended was dropped");
        }
    }

    // All tests passed - emit boot stage markers
    println!("USERSPACE PIPE: ALL TESTS PASSED");
    println!("PIPE_TEST_PASSED");
    process::exit(0);
}

fn now_ns() -> u64 {
    let ts = time::now_monotonic().unwrap_or_else(|_| fail("clock_gettime failed"));
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Pin the calling thread to `cpu`; the negative errno on failure.
fn pin(cpu: u32) -> Result<(), i64> {
    let mask = 1u64 << cpu;
    let r = unsafe { syscall3(SCHED_SETAFFINITY, 0, 8, &mask as *const u64 as u64) } as i64;
    if r == 0 { Ok(()) } else { Err(r) }
}

/// A reschedule requested while a pipe reader finishes its wait must run
/// before the reader is back in user mode. The kernel holds the reader after
/// its wake (`PIPE_WAKE_HOLD`). Meanwhile a thread on the reader's processor
/// is made runnable and raised to SCHED_FIFO, which asks that processor to
/// reschedule; it must run before the read returns. Ok(false) when there is
/// no second processor to write from.
fn reschedule_after_pipe_wait() -> Result<bool, String> {
    match pin(READER_CPU) {
        Ok(()) => {}
        Err(e) if e == -EINVAL => return Ok(false),
        Err(e) => return Err(format!("sched_setaffinity to processor {READER_CPU} returned {e}")),
    }
    let (read_fd, write_fd) = io::pipe().map_err(|e| format!("pipe() failed: {e:?}"))?;
    let (wake_read, wake_write) = io::pipe().map_err(|e| format!("pipe() failed: {e:?}"))?;

    let boosted = std::thread::spawn(move || {
        let tid = libbreenix::process::gettid().map_or(0, |tid| tid.raw());
        if pin(READER_CPU).is_ok() {
            BOOSTED_TID.store(tid, Ordering::Release);
        }
        let mut byte = [0u8; 1];
        let _ = io::read(wake_read, &mut byte);
        while !BOOSTED.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        BOOSTED_RAN_AT.store(now_ns(), Ordering::Release);
    });
    // The writer writes both bytes and raises the other thread even after a
    // failed step, so neither the reader nor that thread is left waiting; it
    // reports the first failure.
    let writer = std::thread::spawn(move || -> Result<(), String> {
        let mut result = pin(WRITER_CPU)
            .map_err(|e| format!("sched_setaffinity to processor {WRITER_CPU} returned {e}"));
        // The reader blocks in read(), is woken by this write and is then held.
        let _ = time::sleep_ms(50);
        if let Err(e) = io::write(write_fd, b"R") {
            result = result.and(Err(format!("write() failed: {e:?}")));
        }
        // While it is held, the other thread becomes runnable on its processor
        // and then outranks it.
        let _ = time::sleep_ms(30);
        if let Err(e) = io::write(wake_write, b"W") {
            result = result.and(Err(format!("write() failed: {e:?}")));
        }
        let _ = time::sleep_ms(30);
        let tid = BOOSTED_TID.load(Ordering::Acquire);
        BOOSTED.store(true, Ordering::Release);
        let priority: i32 = 10;
        let r = unsafe {
            syscall3(nr::SCHED_SETSCHEDULER, tid, SCHED_FIFO, &priority as *const i32 as u64)
        } as i64;
        if tid == 0 || r != 0 {
            result = result.and(Err(format!("sched_setscheduler(SCHED_FIFO) for tid {tid} returned {r}")));
        }
        result
    });

    let held = unsafe { syscall3(nr::IOCTL, read_fd.raw(), PIPE_WAKE_HOLD, 300) } as i64;
    if held != 0 {
        return Err(format!("ioctl(PIPE_WAKE_HOLD) returned {held}"));
    }
    let mut byte = [0u8; 1];
    io::read(read_fd, &mut byte).map_err(|e| format!("read() failed: {e:?}"))?;
    let returned_at = now_ns();
    let ran_at = BOOSTED_RAN_AT.load(Ordering::Acquire);
    writer.join().map_err(|_| String::from("the writer thread panicked"))??;
    boosted.join().map_err(|_| String::from("the boosted thread panicked"))?;
    for fd in [read_fd, write_fd, wake_read, wake_write] {
        let _ = io::close(fd);
    }
    if ran_at == 0 || ran_at > returned_at {
        return Err(format!(
            "the SCHED_FIFO thread on the reader's processor ran at {ran_at} ns, after the read returned at {returned_at} ns"
        ));
    }
    Ok(true)
}
