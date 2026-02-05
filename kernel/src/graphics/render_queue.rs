//! Deferred framebuffer rendering queue.
//!
//! This module provides a ring buffer for queueing text output that will be
//! rendered to the framebuffer by a dedicated kernel task. This architecture
//! solves the kernel stack overflow problem: the deep call stack through
//! terminal_manager → terminal_pane → font rendering (500KB+) now runs on
//! the render task's own stack, not on syscall/interrupt stacks.
//!
//! ## Architecture
//!
//! ```text
//! Syscall/IRQ context          Render task (own stack)
//! ──────────────────           ──────────────────────
//!        │                              │
//!  queue_byte()  ──────────────►  drain_and_render()
//!  (shallow stack)               (deep stack OK)
//!        │                              │
//!   Ring Buffer ◄──────────────────────►│
//! ```
//!
//! ## Implementation Notes
//!
//! Uses a lock-free single-producer approach for the hot path (queue_byte).
//! The buffer is a static array accessed via raw pointers to avoid any
//! mutex overhead that was causing stack issues.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Size of the render queue in bytes.
/// 16KB is enough for several screens of text.
const QUEUE_SIZE: usize = 16 * 1024;

/// The render queue ring buffer - static array, no mutex.
/// Safety: Only modified by queue_byte (producer) and drain_and_render (consumer).
/// Producer only writes at tail, consumer only reads from head.
static mut QUEUE_BUFFER: [u8; QUEUE_SIZE] = [0u8; QUEUE_SIZE];

/// Head index (where to read from) - only modified by consumer.
static QUEUE_HEAD: AtomicUsize = AtomicUsize::new(0);

/// Tail index (where to write to) - only modified by producer.
static QUEUE_TAIL: AtomicUsize = AtomicUsize::new(0);

/// Flag indicating the queue is initialized and ready.
static QUEUE_READY: AtomicBool = AtomicBool::new(false);

// Note: Wake flag is in render_task module, not here. See wake_render_thread().

/// Simple spinlock for producer synchronization (multiple producers possible).
static PRODUCER_LOCK: AtomicBool = AtomicBool::new(false);

/// Initialize the render queue.
/// Called during kernel initialization.
pub fn init() {
    QUEUE_READY.store(true, Ordering::SeqCst);
    crate::serial_println!("[render_queue] Initialized ({}KB buffer)", QUEUE_SIZE / 1024);
}

/// Check if the render queue is ready.
#[inline]
pub fn is_ready() -> bool {
    QUEUE_READY.load(Ordering::Relaxed)
}

/// Try to acquire producer lock. Returns true if acquired.
#[inline]
fn try_lock_producer() -> bool {
    PRODUCER_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
}

/// Release producer lock.
#[inline]
fn unlock_producer() {
    PRODUCER_LOCK.store(false, Ordering::Release);
}

/// Queue a single byte for rendering.
/// Returns true if queued, false if queue is full or not ready.
///
/// This function is designed to be very cheap - just a buffer write.
#[inline]
pub fn queue_byte(byte: u8) -> bool {
    if !is_ready() {
        return false;
    }

    // Try to get producer lock, don't block
    if !try_lock_producer() {
        return false;
    }

    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let tail = QUEUE_TAIL.load(Ordering::Relaxed);
    let next_tail = (tail + 1) % QUEUE_SIZE;

    if next_tail == head {
        // Queue is full
        unlock_producer();
        return false;
    }

    // Safety: We hold the producer lock, and only write at tail index
    unsafe {
        QUEUE_BUFFER[tail] = byte;
    }
    QUEUE_TAIL.store(next_tail, Ordering::Release);

    // Wake the render thread to process the new data
    super::render_task::wake_render_thread();

    unlock_producer();
    true
}

/// Queue multiple bytes for rendering.
/// Returns the number of bytes actually queued.
pub fn queue_bytes(bytes: &[u8]) -> usize {
    if !is_ready() || bytes.is_empty() {
        return 0;
    }

    // Try to get producer lock, don't block
    if !try_lock_producer() {
        return 0;
    }

    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let mut tail = QUEUE_TAIL.load(Ordering::Relaxed);
    let mut queued = 0;

    for &byte in bytes {
        let next_tail = (tail + 1) % QUEUE_SIZE;
        if next_tail == head {
            // Queue is full
            break;
        }
        // Safety: We hold the producer lock
        unsafe {
            QUEUE_BUFFER[tail] = byte;
        }
        tail = next_tail;
        queued += 1;
    }

    if queued > 0 {
        QUEUE_TAIL.store(tail, Ordering::Release);
        // Wake the render thread to process the new data
        super::render_task::wake_render_thread();
    }

    unlock_producer();
    queued
}

/// Check if there's data waiting to be rendered.
#[inline]
pub fn has_pending_data() -> bool {
    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let tail = QUEUE_TAIL.load(Ordering::Acquire);
    head != tail
}

// Note: Wake is now handled via kthread_park/unpark in render_task.rs

/// Drain the queue and render to framebuffer.
/// This is called by the render task.
///
/// Returns the number of bytes rendered.
pub fn drain_and_render() -> usize {
    if !is_ready() {
        return 0;
    }

    // Read current indices
    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let tail = QUEUE_TAIL.load(Ordering::Acquire);

    if head == tail {
        return 0; // Nothing to render
    }

    // Calculate how many bytes to render
    let count = if tail > head {
        tail - head
    } else {
        QUEUE_SIZE - head + tail
    };

    // Copy data out and render in small batches
    const BATCH_SIZE: usize = 256;
    let mut local_buffer = [0u8; BATCH_SIZE];
    let mut rendered = 0;
    let mut current_head = head;

    while rendered < count {
        let batch_count = core::cmp::min(BATCH_SIZE, count - rendered);

        // Copy batch from ring buffer
        // Safety: Consumer only reads from head, producer only writes at tail.
        // The indices are managed by atomic operations.
        for i in 0..batch_count {
            unsafe {
                local_buffer[i] = QUEUE_BUFFER[(current_head + i) % QUEUE_SIZE];
            }
        }

        // Update head to release the buffer space
        current_head = (current_head + batch_count) % QUEUE_SIZE;
        QUEUE_HEAD.store(current_head, Ordering::Release);

        // Now render this batch
        render_batch(&local_buffer[..batch_count]);

        rendered += batch_count;
    }

    rendered
}

/// Render a batch of bytes to the framebuffer.
/// This is where the deep call stack happens, but it's on the render task's stack.
///
/// Since this runs on the render thread (not in interrupt context), we can use
/// blocking locks. This ensures echo characters always get rendered rather than
/// being dropped due to lock contention.
fn render_batch(bytes: &[u8]) {
    // Use terminal manager if active
    // The render thread can use blocking writes since it's not in interrupt context
    if crate::graphics::terminal_manager::is_terminal_manager_active() {
        // Use the blocking version that waits for locks
        let _ = crate::graphics::terminal_manager::write_bytes_to_shell_blocking(bytes);
        return;
    }

    // Fallback to split-screen mode
    if crate::graphics::split_screen::is_split_screen_active() {
        for &byte in bytes {
            let _ = crate::graphics::split_screen::write_char_to_terminal(byte as char);
        }
        return;
    }

    // Fallback to direct framebuffer (x86_64 only)
    // On ARM64, terminal_manager is always active, so this path won't be reached
    #[cfg(target_arch = "x86_64")]
    if let Some(fb) = crate::logger::SHELL_FRAMEBUFFER.get() {
        if let Some(mut guard) = fb.try_lock() {
            for &byte in bytes {
                guard.write_char(byte as char);
            }
        }
    }
}

/// Get queue statistics for debugging.
#[allow(dead_code)]
pub fn stats() -> (usize, usize, usize) {
    let head = QUEUE_HEAD.load(Ordering::Relaxed);
    let tail = QUEUE_TAIL.load(Ordering::Relaxed);
    let used = if tail >= head {
        tail - head
    } else {
        QUEUE_SIZE - head + tail
    };
    (used, QUEUE_SIZE - used, QUEUE_SIZE)
}
