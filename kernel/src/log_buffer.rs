//! Kernel log ring buffer for /proc/kmsg
//!
//! Captures serial output bytes into a lock-free ring buffer so that userspace
//! programs can read kernel logs via /proc/kmsg.
//!
//! Both architectures feed it from their serial log writers.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// Size of the log ring buffer (32 KB).
const LOG_BUFFER_SIZE: usize = 32 * 1024;

/// The log ring buffer. Bytes are atomics so readers on other CPUs (/proc/kmsg,
/// the panic diagnostics screen) never race a writer; relaxed byte loads and
/// stores compile to plain ones.
static LOG_BUFFER: [AtomicU8; LOG_BUFFER_SIZE] = [const { AtomicU8::new(0) }; LOG_BUFFER_SIZE];

/// Read index (consumer).
static LOG_HEAD: AtomicUsize = AtomicUsize::new(0);

/// Write index (producer).
static LOG_TAIL: AtomicUsize = AtomicUsize::new(0);

// Even values count completed bytes twice; odd means a byte is being changed.
// The existing serial writers serialize capture_byte, so this needs no lock.
static LOG_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// Whether the buffer is initialized.
static LOG_READY: AtomicBool = AtomicBool::new(false);

/// Initialize the log buffer.
pub fn init() {
    LOG_READY.store(true, Ordering::SeqCst);
}

/// Check if the log buffer is ready.
#[inline]
pub fn is_ready() -> bool {
    LOG_READY.load(Ordering::Relaxed)
}

/// Capture a single byte into the log ring buffer.
///
/// Called from the serial write path. Must be safe with interrupts disabled.
#[inline]
pub fn capture_byte(byte: u8) {
    if !is_ready() {
        return;
    }

    let sequence = LOG_SEQUENCE.load(Ordering::Relaxed);
    LOG_SEQUENCE.store(sequence.wrapping_add(1), Ordering::SeqCst);
    let tail = LOG_TAIL.load(Ordering::Relaxed);
    let next_tail = (tail + 1) % LOG_BUFFER_SIZE;
    let head = LOG_HEAD.load(Ordering::Acquire);

    if next_tail == head {
        // Buffer full — advance head to drop oldest byte
        LOG_HEAD.store((head + 1) % LOG_BUFFER_SIZE, Ordering::Release);
    }

    LOG_BUFFER[tail].store(byte, Ordering::Relaxed);
    LOG_TAIL.store(next_tail, Ordering::Release);
    LOG_SEQUENCE.store(sequence.wrapping_add(2), Ordering::Release);
}

/// Held by whoever captures through `capture_bytes` or `capture_fmt`: x86 has
/// two log paths (COM1's queue, and the log UART's ring), each under a lock of
/// its own, so `capture_byte`'s single writer is made here. Taken with
/// interrupts masked, for one record.
static CAPTURING: AtomicBool = AtomicBool::new(false);

fn capturing(f: impl FnOnce()) {
    while CAPTURING
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    f();
    CAPTURING.store(false, Ordering::Release);
}

/// Capture one record's bytes, uninterrupted by another record.
/// Call with interrupts masked.
pub fn capture_bytes<'a>(bytes: impl IntoIterator<Item = &'a u8>) {
    if is_ready() {
        capturing(|| bytes.into_iter().copied().for_each(capture_byte));
    }
}

/// Capture the line `args` formats to, uninterrupted by another record: the
/// x86 log's records, as they enter the ring the log UART is written from,
/// so `/proc/kmsg` has a record as soon as its call returns, as on ARM64.
/// Call with interrupts masked.
pub fn capture_fmt(args: core::fmt::Arguments) {
    struct Capture;
    impl core::fmt::Write for Capture {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            s.bytes().for_each(capture_byte);
            Ok(())
        }
    }
    if is_ready() {
        capturing(|| {
            let _ = core::fmt::Write::write_fmt(&mut Capture, args);
        });
    }
}

/// Read available log data into a String.
///
/// This is a non-destructive read that returns all available data
/// but does NOT advance the head pointer, so the same data can be
/// read again (useful for /proc/kmsg which should show full log).
pub fn read_all() -> alloc::string::String {
    if !is_ready() {
        return alloc::string::String::new();
    }

    let head = LOG_HEAD.load(Ordering::Acquire);
    let tail = LOG_TAIL.load(Ordering::Acquire);

    if head == tail {
        return alloc::string::String::new();
    }

    let available = if tail > head {
        tail - head
    } else {
        LOG_BUFFER_SIZE - head + tail
    };

    let mut result = alloc::vec::Vec::with_capacity(available);
    let mut current = head;

    for _ in 0..available {
        result.push(LOG_BUFFER[current].load(Ordering::Relaxed));
        current = (current + 1) % LOG_BUFFER_SIZE;
    }

    alloc::string::String::from_utf8_lossy(&result).into_owned()
}

/// Copy the most recent log bytes (up to `buf.len()`) into `buf`, oldest first.
///
/// Allocation-free and lock-free, for the panic diagnostics screen. A write racing
/// the copy can overwrite the oldest bytes mid-copy (the bytes are atomics, so
/// that is a stale line, not a data race); the caller treats it as best effort.
pub fn copy_tail(buf: &mut [u8]) -> usize {
    if !is_ready() || buf.is_empty() {
        return 0;
    }
    let head = LOG_HEAD.load(Ordering::Acquire);
    let tail = LOG_TAIL.load(Ordering::Acquire);
    let available = (tail + LOG_BUFFER_SIZE - head) % LOG_BUFFER_SIZE;
    let count = available.min(buf.len());
    let start = (tail + LOG_BUFFER_SIZE - count) % LOG_BUFFER_SIZE;
    for (i, slot) in buf[..count].iter_mut().enumerate() {
        *slot = LOG_BUFFER[(start + i) % LOG_BUFFER_SIZE].load(Ordering::Relaxed);
    }
    count
}

/// Root-only proc snapshot: absolute byte offsets let a reader distinguish new
/// bytes in a full ring from an unchanged snapshot. Retry a concurrent capture;
/// a busy ring returns no snapshot and the reader retries on its next pass.
pub fn sequenced_snapshot() -> alloc::string::String {
    use alloc::format;
    for _ in 0..8 {
        let sequence = LOG_SEQUENCE.load(Ordering::SeqCst);
        if sequence & 1 != 0 { continue; }
        let mut bytes = alloc::vec![0u8; LOG_BUFFER_SIZE - 1];
        let count = copy_tail(&mut bytes);
        bytes.truncate(count);
        core::sync::atomic::fence(Ordering::SeqCst);
        if LOG_SEQUENCE.load(Ordering::Acquire) == sequence {
            let end = sequence / 2;
            // Procfs serves text. Use an ASCII hex payload to preserve arbitrary
            // bytes without changing the absolute byte offsets through UTF-8.
            let mut out = format!("BREENIX-KMSG 1 start={} end={}\n", end - count, end);
            for byte in bytes { use core::fmt::Write; let _ = write!(out, "{:02x}", byte); }
            out.push('\n');
            return out;
        }
    }
    alloc::string::String::new()
}
