//! Lock-free ring buffer for deferred framebuffer rendering.
//!
//! This module provides a static ring buffer that decouples byte production
//! (from keyboard IRQ, TTY driver, etc.) from rendering (on a dedicated kthread).
//!
//! The ring buffer is designed to be interrupt-safe:
//! - No locks (uses atomics for head/tail)
//! - O(1) enqueue operation
//! - Single producer assumed (keyboard IRQ context)
//! - Single consumer assumed (render kthread)

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Ring buffer size (16KB should handle bursts of input)
const BUFFER_SIZE: usize = 16 * 1024;

/// Static ring buffer for queued bytes
static BUFFER: RenderQueue = RenderQueue::new();

/// Flag indicating the render system is ready to accept bytes
static RENDER_READY: AtomicBool = AtomicBool::new(false);

/// Lock-free single-producer single-consumer ring buffer.
struct RenderQueue {
    /// The actual buffer storage
    data: [AtomicU8; BUFFER_SIZE],
    /// Write position (producer increments)
    head: AtomicUsize,
    /// Read position (consumer increments)
    tail: AtomicUsize,
}

/// Atomic u8 wrapper for the buffer elements
struct AtomicU8(core::cell::UnsafeCell<u8>);

// SAFETY: AtomicU8 uses atomic operations for all access
unsafe impl Sync for AtomicU8 {}

impl AtomicU8 {
    const fn new(val: u8) -> Self {
        Self(core::cell::UnsafeCell::new(val))
    }

    #[inline]
    fn store(&self, val: u8) {
        // SAFETY: Single producer ensures no concurrent writes
        unsafe { *self.0.get() = val; }
    }

    #[inline]
    fn load(&self) -> u8 {
        // SAFETY: Store completes before head is updated, load happens after tail check
        unsafe { *self.0.get() }
    }
}

impl RenderQueue {
    const fn new() -> Self {
        // Initialize all elements to 0
        const ZERO: AtomicU8 = AtomicU8::new(0);
        Self {
            data: [ZERO; BUFFER_SIZE],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Enqueue a byte. Returns true if successful, false if buffer is full.
    #[inline]
    fn push(&self, byte: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        let next_head = (head + 1) % BUFFER_SIZE;

        // Check if buffer is full
        if next_head == tail {
            return false;
        }

        // Store the byte
        self.data[head].store(byte);

        // Update head with release ordering so consumer sees the write
        self.head.store(next_head, Ordering::Release);

        true
    }

    /// Dequeue a byte. Returns None if buffer is empty.
    #[inline]
    fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        // Check if buffer is empty
        if tail == head {
            return None;
        }

        // Load the byte
        let byte = self.data[tail].load();

        // Update tail with release ordering
        let next_tail = (tail + 1) % BUFFER_SIZE;
        self.tail.store(next_tail, Ordering::Release);

        Some(byte)
    }

    /// Check if there's pending data without consuming it.
    #[inline]
    fn has_data(&self) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        tail != head
    }

    /// Get approximate number of bytes in buffer.
    #[inline]
    fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        if head >= tail {
            head - tail
        } else {
            BUFFER_SIZE - tail + head
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Queue a byte for deferred rendering.
///
/// This is O(1) and safe to call from interrupt context.
/// If the render system isn't ready yet, the byte is silently dropped.
/// If the buffer is full, the byte is silently dropped.
#[inline]
pub fn queue_byte(byte: u8) -> bool {
    if !RENDER_READY.load(Ordering::Acquire) {
        return false;
    }
    BUFFER.push(byte)
}

/// Queue a string for deferred rendering.
///
/// Queues each byte of the string. Safe to call from interrupt context.
pub fn queue_str(s: &str) {
    for byte in s.bytes() {
        queue_byte(byte);
    }
}

/// Check if there's pending data to render.
#[inline]
pub fn has_pending_data() -> bool {
    BUFFER.has_data()
}

/// Get approximate number of bytes pending.
#[inline]
pub fn pending_count() -> usize {
    BUFFER.len()
}

/// Drain all pending bytes and render them.
///
/// This should only be called from the render kthread.
/// Returns the number of bytes rendered.
pub fn drain_and_render() -> usize {
    use crate::logger::SHELL_FRAMEBUFFER;

    let mut count = 0;

    // Get the framebuffer - we're on the render thread so we can wait for the lock
    let fb = match SHELL_FRAMEBUFFER.get() {
        Some(fb) => fb,
        None => return 0,
    };

    // Lock the framebuffer for the duration of rendering
    let mut guard = fb.lock();

    // Drain all pending bytes
    while let Some(byte) = BUFFER.pop() {
        guard.write_char(byte as char);
        count += 1;
    }

    // Flush if we rendered anything
    if count > 0 {
        if let Some(db) = guard.double_buffer_mut() {
            db.flush_if_dirty();
        }
    }

    count
}

/// Mark the render system as ready to accept bytes.
///
/// Called by the render task after initialization.
pub fn set_ready() {
    RENDER_READY.store(true, Ordering::Release);
    log::info!("RENDER_QUEUE: Ready to accept bytes");
}

/// Check if render system is ready.
pub fn is_ready() -> bool {
    RENDER_READY.load(Ordering::Acquire)
}
