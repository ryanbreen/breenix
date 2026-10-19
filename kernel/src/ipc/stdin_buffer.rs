use super::STDIN_BUF_SIZE;

/// Stdin ring buffer
pub(super) struct StdinBuffer {
    /// Circular buffer storage
    buffer: [u8; STDIN_BUF_SIZE],
    /// Read position
    read_pos: usize,
    /// Write position
    write_pos: usize,
    /// Number of bytes in buffer
    pub(super) len: usize,
    // EOF is a record at a byte boundary, not an in-band byte. The shared
    // byte/EOF capacity is bounded to Linux's 4095-byte input limit.
    produced: u64,
    consumed: u64,
    eof_positions: [u64; STDIN_BUF_SIZE],
    eof_read: usize,
    eof_write: usize,
    eof_len: usize,
}

impl StdinBuffer {
    /// Create a new empty stdin buffer
    pub(super) const fn new() -> Self {
        StdinBuffer {
            buffer: [0; STDIN_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            len: 0,
            produced: 0,
            consumed: 0,
            eof_positions: [0; STDIN_BUF_SIZE],
            eof_read: 0,
            eof_write: 0,
            eof_len: 0,
        }
    }

    /// Push a single byte to the buffer
    /// Returns true if the byte was added, false if buffer is full
    pub(super) fn push_byte(&mut self, byte: u8) -> bool {
        if self.is_full() {
            return false;
        }

        self.buffer[self.write_pos] = byte;
        self.write_pos = (self.write_pos + 1) % STDIN_BUF_SIZE;
        self.len += 1;
        self.produced = self.produced.wrapping_add(1);
        // Buffer -> queue order matches checked publication. Deferred delivery
        // avoids acquiring the scheduler in the input producer.
        true
    }

    pub(super) fn is_full(&self) -> bool {
        self.len + self.eof_len >= STDIN_BUF_SIZE - 1
    }

    /// Read bytes from the buffer into the provided slice
    /// Returns the number of bytes actually read
    pub(super) fn read_bytes(&mut self, buf: &mut [u8]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        if self.eof_ready() {
            self.eof_read = (self.eof_read + 1) % STDIN_BUF_SIZE;
            self.eof_len -= 1;
            return 0;
        }
        let to_read = buf.len().min(self.len);
        let mut read = 0;

        while read < to_read && !self.eof_ready() {
            buf[read] = self.buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % STDIN_BUF_SIZE;
            read += 1;
            self.consumed = self.consumed.wrapping_add(1);
        }

        self.len -= read;
        read
    }

    pub(super) fn eof_ready(&self) -> bool {
        self.eof_len != 0 && self.eof_positions[self.eof_read] == self.consumed
    }

    pub(super) fn push_eof(&mut self) -> bool {
        if self.len + self.eof_len >= STDIN_BUF_SIZE - 1 {
            return false;
        }
        self.eof_positions[self.eof_write] = self.produced;
        self.eof_write = (self.eof_write + 1) % STDIN_BUF_SIZE;
        self.eof_len += 1;
        true
    }

    pub(super) fn flush(&mut self) {
        self.len = 0;
        self.read_pos = self.write_pos;
        self.consumed = self.produced;
        self.eof_len = 0;
        self.eof_read = self.eof_write;
    }

    /// Check if neither bytes nor an EOF record can be read.
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0 && !self.eof_ready()
    }

    /// Get number of bytes available to read
    pub(super) fn available(&self) -> usize {
        self.len
    }
}

