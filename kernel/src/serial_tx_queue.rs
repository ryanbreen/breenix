//! COM1 byte ownership: terminal flushes preserve kernel log records.

pub(crate) struct TxQueue<const N: usize> {
    bytes: [u8; N],
    kernel: [bool; N],
    head: usize,
    pub(crate) len: usize,
    /// Bytes the UART's FIFO can still take without a status read: set to the
    /// FIFO's depth when the status register shows the transmitter empty, and
    /// counted down by every byte written under SERIAL1, so writers taking
    /// turns byte by byte never overrun it.
    /// claim-lint:ok: tests/serial_log_loss.rs verifies flush preserves FIFO credit.
    pub(crate) room: usize,
}

impl<const N: usize> TxQueue<N> {
    pub(crate) const fn new() -> Self {
        Self { bytes: [0; N], kernel: [false; N], head: 0, len: 0, room: 0 }
    }

    pub(crate) fn push(&mut self, byte: u8, kernel: bool) -> bool {
        if self.len == N {
            return false;
        }
        self.bytes[(self.head + self.len) % N] = byte;
        self.kernel[(self.head + self.len) % N] = kernel;
        self.len += 1;
        true
    }

    pub(crate) fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = self.bytes[(self.head + i) % N];
        }
        self.head = (self.head + n) % N;
        self.len -= n;
        n
    }
    /// POSIX flush discards terminal output, never kernel evidence sharing
    /// the UART. Compact in place across wrap; a removed terminal suffix
    /// gets a line boundary before the next retained kernel record.
    /// claim-lint:ok: tests/serial_log_loss.rs tests wrapped flush and retained records.
    pub(crate) fn flush_terminal(&mut self) -> Option<u8> {
        let mut kept = 0;
        let mut gap = false;
        for offset in 0..self.len {
            let at = (self.head + offset) % N;
            if !self.kernel[at] {
                gap = true;
                continue;
            }
            let byte = self.bytes[at];
            if gap && byte != b'\n' {
                let to = (self.head + kept) % N;
                self.bytes[to] = b'\n';
                self.kernel[to] = true;
                kept += 1;
            }
            gap = false;
            let to = (self.head + kept) % N;
            self.bytes[to] = byte;
            self.kernel[to] = true;
            kept += 1;
        }
        self.len = kept;
        (kept != 0).then(|| self.bytes[(self.head + kept - 1) % N])
    }

}
