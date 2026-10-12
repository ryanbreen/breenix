//! The x86 kernel log ring: which lines it takes, whole, and which it drops.
//!
//! A kernel log line is appended to a fixed ring and written to COM2 later,
//! one FIFO's worth at a time (`serial`), so a caller that cannot wait on the
//! UART still logs. When the ring is full an ordinary line is dropped whole
//! and the bytes lost are reported, on a line of their own, once there is room.
//!
//! A line that carries a boot-path stage marker (`stage_lines`) is never
//! dropped. Ordinary lines may fill the ring only up to its last
//! [`LogRing::RESERVE`] bytes, which stay free for stage lines; a stage line
//! that does not fit even there is handed back with the room it needs, and its
//! caller writes the ring out until it has that room. Stage lines are a few
//! dozen short lines per boot, so the reserve holds them and that wait is a
//! last resort.
//!
//! Core only: the host test `tests/stage_line_pressure.rs` compiles this file.

use core::fmt::{self, Write};

use super::stage_lines;

pub struct LogRing<const N: usize> {
    bytes: [u8; N],
    head: usize,
    len: usize,
    /// Bytes of dropped lines not yet reported.
    dropped: u64,
    /// Bytes ever appended.
    pushed: u64,
}

impl<const N: usize> LogRing<N> {
    /// The ring's last bytes, which only stage lines may use.
    pub const RESERVE: usize = N / 8;

    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            head: 0,
            len: 0,
            dropped: 0,
            pushed: 0,
        }
    }

    /// Bytes waiting to be written out.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Bytes ever appended, counting the ones already written out.
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for slot in &mut out[..n] {
            *slot = self.bytes[self.head];
            self.head = (self.head + 1) % N;
        }
        self.len -= n;
        n
    }

    /// Append the line `args` formats to, whole, or drop it whole. Returns the
    /// bytes ever appended once it is in or dropped, or, for a stage line
    /// (`markers`) that does not fit, `Err` with the free bytes it needs.
    pub fn append(&mut self, args: fmt::Arguments, markers: &[&str]) -> Result<u64, usize> {
        self.report_dropped();
        let start_len = self.len;
        let start_pushed = self.pushed;
        let mut writer = RingWriter {
            ring: self,
            overflow: 0,
        };
        let _ = writer.write_fmt(args);
        let overflow = writer.overflow;
        if overflow == 0 && self.len <= N - Self::RESERVE {
            return Ok(self.pushed);
        }
        // Only a line that reaches into the reserve needs to know what it is.
        let stage = if overflow == 0 {
            let mut scan = stage_lines::Scan::new(markers);
            for at in start_len..self.len {
                scan.byte(self.bytes[(self.head + at) % N]);
            }
            scan.hit()
        } else {
            stage_lines::formats(markers, args)
        };
        if stage && overflow == 0 {
            return Ok(self.pushed);
        }
        let record = (self.pushed - start_pushed) as usize + overflow;
        self.len = start_len;
        self.pushed = start_pushed;
        if stage && record <= N {
            return Err(record);
        }
        // An ordinary line, or a stage line longer than the whole ring.
        self.dropped += record as u64;
        Ok(self.pushed)
    }

    /// Report dropped bytes on a line of their own once it fits outside the
    /// reserve. Until then the count keeps growing.
    fn report_dropped(&mut self) {
        if self.dropped == 0 {
            return;
        }
        let mut report = Report {
            bytes: [0; 48],
            len: 0,
        };
        let _ = write!(report, "[log: {} bytes dropped]\n", self.dropped);
        if self.len + report.len > N - Self::RESERVE {
            return;
        }
        for &byte in &report.bytes[..report.len] {
            self.push(byte);
        }
        self.dropped = 0;
    }

    fn push(&mut self, byte: u8) -> bool {
        if self.len == N {
            return false;
        }
        self.bytes[(self.head + self.len) % N] = byte;
        self.len += 1;
        self.pushed += 1;
        true
    }
}

impl<const N: usize> Default for LogRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Appends a formatted line to the ring, counting what does not fit.
struct RingWriter<'a, const N: usize> {
    ring: &'a mut LogRing<N>,
    overflow: usize,
}

impl<const N: usize> fmt::Write for RingWriter<'_, N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            // uart_16550::send translates backspace and DEL to BS SPACE BS.
            let wire: &[u8] = if byte == 0x08 || byte == 0x7f {
                &[0x08, b' ', 0x08]
            } else {
                core::slice::from_ref(&byte)
            };
            for &b in wire {
                if !self.ring.push(b) {
                    self.overflow += 1;
                }
            }
        }
        Ok(())
    }
}

/// A drop report, formatted before it is known to fit.
struct Report {
    bytes: [u8; 48],
    len: usize,
}

impl fmt::Write for Report {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        let slot = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        slot.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
