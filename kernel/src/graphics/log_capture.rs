//! Lock-free byte ring buffer for capturing serial output.
//!
//! Serial output is tee'd into this buffer by `serial_aarch64::_print()`.
//! The render thread drains it periodically, splits into lines, and feeds
//! those lines to the Logs terminal pane.
//!
//! The buffer is single-producer (serial output path, which already holds
//! the SERIAL1 lock) and single-consumer (render thread), so we only need
//! atomic head/tail indices — no additional locks.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Size of the capture ring buffer in bytes (32 KB).
const CAPTURE_SIZE: usize = 32 * 1024;

/// The capture ring buffer — static array, no mutex.
/// Safety: Only modified by `capture_byte` (producer) at tail, and
/// `drain` (consumer) at head. Single producer guaranteed by SERIAL1 lock.
static mut CAPTURE_BUFFER: [u8; CAPTURE_SIZE] = [0u8; CAPTURE_SIZE];

/// Head index (where the consumer reads from).
static CAPTURE_HEAD: AtomicUsize = AtomicUsize::new(0);

/// Tail index (where the producer writes to).
static CAPTURE_TAIL: AtomicUsize = AtomicUsize::new(0);

/// Flag indicating the capture buffer is initialized and ready.
static CAPTURE_READY: AtomicBool = AtomicBool::new(false);

/// Initialize the log capture ring buffer.
pub fn init() {
    CAPTURE_READY.store(true, Ordering::SeqCst);
}

/// Check if the log capture buffer is ready.
#[inline]
pub fn is_ready() -> bool {
    CAPTURE_READY.load(Ordering::Relaxed)
}

/// Capture a single byte into the ring buffer.
///
/// Called from `serial_aarch64::_print()` while SERIAL1 lock is held,
/// so this is effectively single-producer. No additional locking needed.
/// Must be safe to call with interrupts disabled.
#[inline]
pub fn capture_byte(byte: u8) {
    if !is_ready() {
        return;
    }

    let tail = CAPTURE_TAIL.load(Ordering::Relaxed);
    let next_tail = (tail + 1) % CAPTURE_SIZE;
    let head = CAPTURE_HEAD.load(Ordering::Acquire);

    if next_tail == head {
        // Buffer full — drop the byte rather than blocking
        return;
    }

    // Safety: Single producer (SERIAL1 lock held), writes only at tail.
    unsafe {
        CAPTURE_BUFFER[tail] = byte;
    }
    CAPTURE_TAIL.store(next_tail, Ordering::Release);
}

/// Drain pending bytes from the ring buffer into `buf`.
///
/// Returns the number of bytes copied. Called by the render thread
/// (single consumer).
pub fn drain(buf: &mut [u8]) -> usize {
    if !is_ready() || buf.is_empty() {
        return 0;
    }

    let head = CAPTURE_HEAD.load(Ordering::Relaxed);
    let tail = CAPTURE_TAIL.load(Ordering::Acquire);

    if head == tail {
        return 0;
    }

    // Calculate available bytes
    let available = if tail > head {
        tail - head
    } else {
        CAPTURE_SIZE - head + tail
    };

    let to_copy = available.min(buf.len());
    let mut copied = 0;
    let mut current = head;

    // Safety: Consumer only reads from head, producer only writes at tail.
    while copied < to_copy {
        unsafe {
            buf[copied] = CAPTURE_BUFFER[current];
        }
        current = (current + 1) % CAPTURE_SIZE;
        copied += 1;
    }

    CAPTURE_HEAD.store(current, Ordering::Release);
    copied
}

/// Check if there is pending data in the capture buffer.
#[inline]
pub fn has_pending_data() -> bool {
    let head = CAPTURE_HEAD.load(Ordering::Acquire);
    let tail = CAPTURE_TAIL.load(Ordering::Acquire);
    head != tail
}
