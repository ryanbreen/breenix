//! Kernel log ring buffer for /proc/kmsg
//!
//! Captures serial output bytes into a lock-free ring buffer so that userspace
//! programs can read kernel logs via /proc/kmsg.
//!
//! On ARM64, this reuses the existing `graphics::log_capture` buffer.
//! On x86_64, this module provides its own capture ring buffer.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Size of the log ring buffer (32 KB).
const LOG_BUFFER_SIZE: usize = 32 * 1024;

/// The log ring buffer.
static mut LOG_BUFFER: [u8; LOG_BUFFER_SIZE] = [0u8; LOG_BUFFER_SIZE];

/// Read index (consumer).
static LOG_HEAD: AtomicUsize = AtomicUsize::new(0);

/// Write index (producer).
static LOG_TAIL: AtomicUsize = AtomicUsize::new(0);

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

    let tail = LOG_TAIL.load(Ordering::Relaxed);
    let next_tail = (tail + 1) % LOG_BUFFER_SIZE;
    let head = LOG_HEAD.load(Ordering::Acquire);

    if next_tail == head {
        // Buffer full — advance head to drop oldest byte
        LOG_HEAD.store((head + 1) % LOG_BUFFER_SIZE, Ordering::Release);
    }

    unsafe {
        LOG_BUFFER[tail] = byte;
    }
    LOG_TAIL.store(next_tail, Ordering::Release);
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
        unsafe {
            result.push(LOG_BUFFER[current]);
        }
        current = (current + 1) % LOG_BUFFER_SIZE;
    }

    alloc::string::String::from_utf8_lossy(&result).into_owned()
}
