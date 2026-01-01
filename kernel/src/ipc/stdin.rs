//! Stdin ring buffer for keyboard input
//!
//! This module provides a kernel-side ring buffer for stdin input.
//! Characters from the keyboard interrupt handler are pushed here,
//! and userspace processes can read from it via the read() syscall.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

/// Default stdin buffer size
pub const STDIN_BUF_SIZE: usize = 4096;

/// Thread IDs blocked waiting for stdin input
static BLOCKED_READERS: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

/// The global stdin buffer
static STDIN_BUFFER: Mutex<StdinBuffer> = Mutex::new(StdinBuffer::new());

/// Stdin ring buffer
pub struct StdinBuffer {
    /// Circular buffer storage
    buffer: [u8; STDIN_BUF_SIZE],
    /// Read position
    read_pos: usize,
    /// Write position
    write_pos: usize,
    /// Number of bytes in buffer
    len: usize,
}

impl StdinBuffer {
    /// Create a new empty stdin buffer
    pub const fn new() -> Self {
        StdinBuffer {
            buffer: [0; STDIN_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            len: 0,
        }
    }

    /// Push a single byte to the buffer
    /// Returns true if the byte was added, false if buffer is full
    fn push_byte(&mut self, byte: u8) -> bool {
        if self.len >= STDIN_BUF_SIZE {
            return false;
        }

        self.buffer[self.write_pos] = byte;
        self.write_pos = (self.write_pos + 1) % STDIN_BUF_SIZE;
        self.len += 1;
        true
    }

    /// Read bytes from the buffer into the provided slice
    /// Returns the number of bytes actually read
    fn read_bytes(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.len);
        let mut read = 0;

        while read < to_read {
            buf[read] = self.buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % STDIN_BUF_SIZE;
            read += 1;
        }

        self.len -= read;
        read
    }

    /// Check if buffer is empty
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get number of bytes available to read
    #[allow(dead_code)]
    fn available(&self) -> usize {
        self.len
    }
}

/// Push a byte to the stdin buffer
/// Called from keyboard task when a character is typed
///
/// Note: With TTY integration, keyboard input now goes through the TTY layer
/// which handles echo, line editing, and signals. This function is kept for
/// fallback when TTY is not initialized.
#[allow(dead_code)]
pub fn push_byte(byte: u8) {
    let mut buffer = STDIN_BUFFER.lock();
    if buffer.push_byte(byte) {
        // Echo character to serial output for now
        crate::serial::write_byte(byte);

        // Drop buffer lock before waking readers to avoid deadlock
        drop(buffer);

        // Wake any blocked readers
        wake_blocked_readers();
    } else {
        log::warn!("stdin buffer full, dropping byte: 0x{:02x}", byte);
    }
}

/// Push a byte to stdin from interrupt context (uses try_lock to avoid deadlock)
/// Returns true if the byte was pushed, false if locks couldn't be acquired
pub fn push_byte_from_irq(byte: u8) -> bool {
    // Try to acquire the buffer lock - don't block in interrupt context
    if let Some(mut buffer) = STDIN_BUFFER.try_lock() {
        if buffer.push_byte(byte) {
            // Echo character to serial output (COM1)
            crate::serial::write_byte(byte);

            // In interactive mode, also echo to framebuffer so user sees their input
            #[cfg(feature = "interactive")]
            {
                crate::logger::write_char_to_framebuffer(byte);
            }

            drop(buffer);

            // Try to wake blocked readers (may fail if scheduler lock is held)
            wake_blocked_readers_try();
            return true;
        }
    }
    false
}

/// Try to wake blocked readers without blocking (for interrupt context)
fn wake_blocked_readers_try() {
    let readers: alloc::vec::Vec<u64> = {
        if let Some(mut blocked) = BLOCKED_READERS.try_lock() {
            blocked.drain(..).collect()
        } else {
            return; // Can't get lock, readers will be woken when they retry
        }
    };

    if readers.is_empty() {
        return;
    }

    // Try to wake threads via the scheduler's non-blocking path
    // Note: We use the with_scheduler variant here which may need to disable
    // interrupts, but since we're already in an interrupt handler with a
    // non-reentrant interrupt, this is safe.
    crate::task::scheduler::with_scheduler(|sched| {
        for thread_id in readers {
            sched.unblock(thread_id);
        }
    });

    // Trigger reschedule so the woken thread runs soon
    crate::task::scheduler::set_need_resched();
}

/// Read bytes from stdin buffer
/// Returns Ok(n) with bytes read, or Err(EAGAIN) if would block
pub fn read_bytes(buf: &mut [u8]) -> Result<usize, i32> {
    let mut buffer = STDIN_BUFFER.lock();

    if buffer.is_empty() {
        // No data available - would block
        return Err(11); // EAGAIN
    }

    Ok(buffer.read_bytes(buf))
}

/// Check if stdin has data available
#[allow(dead_code)]
pub fn has_data() -> bool {
    !STDIN_BUFFER.lock().is_empty()
}

/// Register a thread as waiting for stdin input
///
/// Note: With TTY integration, blocked readers are now registered through
/// TtyDevice::register_blocked_reader. This function is kept for fallback.
#[allow(dead_code)]
pub fn register_blocked_reader(thread_id: u64) {
    let mut blocked = BLOCKED_READERS.lock();
    if !blocked.contains(&thread_id) {
        blocked.push_back(thread_id);
        log::trace!("stdin: Thread {} blocked waiting for input", thread_id);
    }
}

/// Unregister a thread from waiting for stdin (e.g., on signal or timeout)
#[allow(dead_code)]
pub fn unregister_blocked_reader(thread_id: u64) {
    let mut blocked = BLOCKED_READERS.lock();
    blocked.retain(|&id| id != thread_id);
}

/// Wake all threads blocked on stdin read
///
/// Note: With TTY integration, blocked readers are woken through
/// TtyDevice::wake_blocked_readers. This function is kept for fallback.
#[allow(dead_code)]
fn wake_blocked_readers() {
    let readers: Vec<u64> = {
        let mut blocked = BLOCKED_READERS.lock();
        blocked.drain(..).collect()
    };

    if readers.is_empty() {
        return;
    }

    log::trace!("stdin: Waking {} blocked readers", readers.len());

    // Wake each blocked thread
    crate::task::scheduler::with_scheduler(|sched| {
        for thread_id in readers {
            sched.unblock(thread_id);
            log::trace!("stdin: Woke thread {}", thread_id);
        }
    });

    // Trigger reschedule to let woken threads run
    crate::task::scheduler::set_need_resched();
}

/// Get the number of bytes available in the stdin buffer
#[allow(dead_code)]
pub fn available() -> usize {
    STDIN_BUFFER.lock().len
}
