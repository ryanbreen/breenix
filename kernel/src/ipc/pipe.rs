//! Pipe buffer implementation
//!
//! Pipes provide unidirectional byte streams for inter-process communication.
//! This module implements the kernel-side pipe buffer that connects the
//! read and write ends of a pipe.

use alloc::vec::Vec;

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
}

impl PipeBuffer {
    /// Create a new pipe buffer
    pub fn new() -> Self {
        let mut buffer = Vec::with_capacity(PIPE_BUF_SIZE);
        buffer.resize(PIPE_BUF_SIZE, 0);
        PipeBuffer {
            buffer,
            read_pos: 0,
            write_pos: 0,
            len: 0,
            readers: 1,
            writers: 1,
            read_waiters: Vec::new(),
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
        Ok(read)
    }

    /// Write to the pipe buffer
    ///
    /// Returns:
    /// - Ok(n) where n > 0: n bytes were written
    /// - Err(32): EPIPE - broken pipe (no readers)
    /// - Err(11): EAGAIN - would block (buffer full)
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, i32> {
        if self.readers == 0 {
            // No readers - broken pipe
            return Err(32); // EPIPE
        }

        let available = PIPE_BUF_SIZE - self.len;
        if available == 0 {
            // Buffer is full - would block
            return Err(11); // EAGAIN
        }

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
        self.len < PIPE_BUF_SIZE && self.readers > 0
    }

    /// Close the read end of the pipe
    pub fn close_read(&mut self) {
        if self.readers > 0 {
            self.readers -= 1;
        }
    }

    /// Close the write end of the pipe
    pub fn close_write(&mut self) {
        if self.writers > 0 {
            self.writers -= 1;
            // If this was the last writer, wake any readers so they get EOF
            if self.writers == 0 {
                self.wake_read_waiters();
            }
        }
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
