//! Pipe buffer implementation
//!
//! Pipes provide unidirectional byte streams for inter-process communication.
//! This module implements the kernel-side pipe buffer that connects the
//! read and write ends of a pipe.

use crate::syscall::errno::{EAGAIN, EPIPE};
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Writes at or below this limit are indivisible; this is not the capacity.
pub const PIPE_BUF: usize = 4096;

/// Transfer outcomes stay distinct from userspace mode-dependent errno.
pub enum WriteAttempt {
    WouldBlock,
    BrokenPipe,
}

/// Owned close notifications. Deliver via `deliver()` after all PM/object
/// guards drop, or via `deliver_deferred()` when the caller cannot drop its
/// PM guard first (P-2/#919; see `Process::close_all_fds()`).
#[must_use = "close notifications must be delivered via deliver() or deliver_deferred()"]
pub struct CloseNotifications {
    writers: Option<Arc<WaitQueueHead>>,
}

impl CloseNotifications {
    /// Deliver an immediate wake. The caller must have already released the
    /// buffer's own lock (true by construction: `close_read()`/`close_write()`
    /// return an owned value, not a borrow) and must not be holding
    /// PROCESS_MANAGER, since this can acquire the scheduler lock directly
    /// (Level 1 under Level 2 is a lock-order violation; see `scheduler.rs`).
    pub fn deliver(self) {
        if let Some(queue) = self.writers {
            queue.wake_up();
        }
    }

    /// Deliver a wake from a context that still holds PROCESS_MANAGER (or any
    /// other lock that must not nest under SCHEDULER). Routes through the
    /// lock-free ISR wake buffer instead of acquiring the scheduler lock
    /// inline in the common case (the buffer's own full-buffer fallback can
    /// still take it); see `WaitQueueHead::wake_up_deferred()`.
    pub fn deliver_deferred(self) {
        if let Some(queue) = self.writers {
            queue.wake_up_deferred();
        }
    }
}

/// Default pipe buffer size (matches Linux)
pub const PIPE_BUF_SIZE: usize = 65536;

/// Pipe buffer - a circular buffer with reader/writer tracking
pub struct PipeBuffer {
    /// The buffer storage
    buffer: Vec<u8>,
    /// Read position in the circular buffer
    read_pos: usize,
    /// Write position in the circular buffer
    write_pos: usize,
    /// Number of bytes currently in the buffer
    len: usize,
    /// Number of active readers (0 = broken pipe on write)
    readers: usize,
    /// Number of active writers (0 = EOF on read)
    writers: usize,
    /// Threads waiting to read from this pipe
    read_waiters: Vec<u64>,
    /// Data writers, distinct from FIFO open waiters and legacy pipe readers.
    pub write_waiters: Arc<WaitQueueHead>,
}

impl PipeBuffer {
    /// Create a new pipe buffer
    pub fn new() -> Self {
        let mut pipe = Self::new_zero_refs();
        pipe.readers = 1;
        pipe.writers = 1;
        pipe
    }

    /// FIFO buffers acquire their first references through open, not creation.
    pub fn new_zero_refs() -> Self {
        let mut buffer = Vec::with_capacity(PIPE_BUF_SIZE);
        buffer.resize(PIPE_BUF_SIZE, 0);
        PipeBuffer {
            buffer,
            read_pos: 0,
            write_pos: 0,
            len: 0,
            readers: 0,
            writers: 0,
            read_waiters: Vec::new(),
            write_waiters: Arc::new(WaitQueueHead::new()),
        }
    }

    /// Read from the pipe buffer
    ///
    /// Returns:
    /// - Ok(n) where n > 0: n bytes were read
    /// - Ok(0): EOF (no writers remaining)
    /// - Err(11): EAGAIN - would block (buffer empty but writers exist)
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        if self.len == 0 {
            // Buffer is empty
            if self.writers == 0 {
                // No writers - EOF
                return Ok(0);
            } else {
                // Writers exist but no data - would block
                return Err(11); // EAGAIN
            }
        }

        // Read up to buf.len() bytes
        let to_read = buf.len().min(self.len);
        let mut read = 0;

        while read < to_read {
            buf[read] = self.buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PIPE_BUF_SIZE;
            read += 1;
        }

        self.len -= read;
        if read > 0 {
            self.write_waiters.wake_up();
        }
        Ok(read)
    }

    /// Write to the pipe buffer
    ///
    /// Returns:
    /// - Ok(n) where n > 0: n bytes were written
    /// - Err(32): EPIPE - broken pipe (no readers)
    /// - Err(11): EAGAIN - would block (buffer full)
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, i32> {
        self.try_write(buf, false).map_err(|attempt| match attempt {
            WriteAttempt::WouldBlock => EAGAIN,
            WriteAttempt::BrokenPipe => EPIPE,
        })
    }

    /// Shared readiness: poll asks for one byte; an atomic writer asks for its complete request.
    pub fn has_write_space(&self, required: usize) -> bool {
        self.readers > 0 && (PIPE_BUF_SIZE - self.len) >= required
    }

    /// The caller holds the buffer mutex for this entire check and copy.
    /// `atomic_request` describes the original request, even after progress.
    pub fn try_write(&mut self, buf: &[u8], atomic_request: bool) -> Result<usize, WriteAttempt> {
        if self.readers == 0 {
            return Err(WriteAttempt::BrokenPipe);
        }
        let required = if atomic_request { buf.len() } else { 1 };
        if !self.has_write_space(required) {
            return Err(WriteAttempt::WouldBlock);
        }
        let available = PIPE_BUF_SIZE - self.len;

        // Write up to available space
        let to_write = buf.len().min(available);
        let mut written = 0;

        while written < to_write {
            self.buffer[self.write_pos] = buf[written];
            self.write_pos = (self.write_pos + 1) % PIPE_BUF_SIZE;
            written += 1;
        }

        self.len += written;

        // Wake any threads waiting to read
        if written > 0 {
            self.wake_read_waiters();
        }

        Ok(written)
    }

    /// Wake all threads waiting to read from this pipe
    fn wake_read_waiters(&mut self) {
        let waiters: Vec<u64> = self.read_waiters.drain(..).collect();
        for tid in waiters {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(tid);
            });
        }
    }

    /// Check if pipe is readable (has data or EOF)
    #[allow(dead_code)]
    pub fn is_readable(&self) -> bool {
        self.len > 0 || self.writers == 0
    }

    /// Check if pipe is writable (has space and readers exist)
    #[allow(dead_code)]
    pub fn is_writable(&self) -> bool {
        self.has_write_space(1)
    }

    /// Close the read end of the pipe
    #[must_use]
    pub fn close_read(&mut self) -> CloseNotifications {
        let last_reader = self.readers == 1;
        if self.readers > 0 {
            self.readers -= 1;
        }
        CloseNotifications {
            writers: last_reader.then(|| self.write_waiters.clone()),
        }
    }

    /// Preserve legacy reader EOF notification; new writer-queue notifications
    /// are returned for delivery by the caller. The two legacy fault exits retain
    /// their existing read-waiter behavior (#919), outside the writer repair.
    #[must_use]
    pub fn close_write(&mut self) -> CloseNotifications {
        if self.writers > 0 {
            self.writers -= 1;
            if self.writers == 0 {
                self.wake_read_waiters();
            }
        }
        CloseNotifications { writers: None }
    }

    /// Add a reader (used when duplicating pipe read fds)
    pub fn add_reader(&mut self) {
        self.readers += 1;
    }

    /// Add a writer (used when duplicating pipe write fds)
    pub fn add_writer(&mut self) {
        self.writers += 1;
    }

    /// Register a thread as waiting to read from this pipe
    pub fn add_read_waiter(&mut self, tid: u64) {
        if !self.read_waiters.contains(&tid) {
            self.read_waiters.push(tid);
        }
    }

    /// Unregister a thread from the read wait list
    #[allow(dead_code)]
    pub fn remove_read_waiter(&mut self, tid: u64) {
        self.read_waiters.retain(|&t| t != tid);
    }

    /// Check if the pipe has data or is at EOF (used for blocking decisions)
    pub fn has_data_or_eof(&self) -> bool {
        self.len > 0 || self.writers == 0
    }

    /// Get the number of bytes available to read
    #[allow(dead_code)]
    pub fn available(&self) -> usize {
        self.len
    }

    /// Get the space available for writing
    #[allow(dead_code)]
    pub fn space(&self) -> usize {
        PIPE_BUF_SIZE - self.len
    }

    /// Check if pipe has active readers (used by write to detect broken pipe)
    #[allow(dead_code)]
    pub fn has_readers(&self) -> bool {
        self.readers > 0
    }

    /// Check if pipe has active writers (used by read to detect EOF)
    #[allow(dead_code)]
    pub fn has_writers(&self) -> bool {
        self.writers > 0
    }
}

impl Default for PipeBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a new pipe
///
/// Returns (read_buffer, write_buffer) where both point to the same underlying buffer
/// wrapped in Arc<Mutex> for shared access.
pub fn create_pipe() -> (
    alloc::sync::Arc<spin::Mutex<PipeBuffer>>,
    alloc::sync::Arc<spin::Mutex<PipeBuffer>>,
) {
    let buffer = alloc::sync::Arc::new(spin::Mutex::new(PipeBuffer::new()));
    (buffer.clone(), buffer)
}
