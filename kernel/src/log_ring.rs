//! The x86 kernel log ring: which lines it takes, whole, and which it drops.
//!
//! A kernel log line is appended to a fixed ring and written to the log UART
//! later, one FIFO's worth at a time, or one line at a time into the console's
//! queue when the log shares it (`serial`), so a caller that cannot wait on the
//! UART still logs. When the ring is full an ordinary line is dropped whole
//! and the exact lines and wire bytes lost are reported at that position.
//! Draining alone publishes the report, including a final loss with no later log call.
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
    dropped_lines: u64,
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
            dropped_lines: 0,
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

    /// Snapshot the serial synchronization boundary, including loss reports.
    /// A pending loss asks the caller to drain room for its report.
    pub fn sync_target(&mut self) -> Option<u64> {
        self.report_dropped();
        (self.dropped == 0).then_some(self.pushed)
    }

    /// Bytes waiting up to and including the first line end, or the whole
    /// waiting length when no line end is waiting.
    pub fn line_len(&self) -> usize {
        for index in 0..self.len {
            if self.bytes[(self.head + index) % N] == b'\n' {
                return index + 1;
            }
        }
        self.len
    }

    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for slot in &mut out[..n] {
            *slot = self.bytes[self.head];
            self.head = (self.head + 1) % N;
        }
        self.len -= n;
        // Materialize the loss before any later record, even if no producer
        // ever calls append again. pop and line_len share the ring lock.
        self.report_dropped();
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
            lines: 0,
            last: None,
        };
        let _ = writer.write_fmt(args);
        let overflow = writer.overflow;
        let lines = writer.lines + usize::from(writer.last.is_some_and(|last| last != b'\n'));
        if overflow == 0 && self.dropped == 0 && self.len <= N - Self::RESERVE {
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
        if stage && overflow == 0 && self.dropped == 0 {
            return Ok(self.pushed);
        }
        let record = (self.pushed - start_pushed) as usize + overflow;
        self.len = start_len;
        self.pushed = start_pushed;
        if stage && record <= N {
            return Err(if self.dropped == 0 { record } else { record.max(self.drop_report().len) });
        }
        // An ordinary line, or a stage line longer than the whole ring.
        self.dropped += record as u64;
        self.dropped_lines += lines as u64;
        self.report_dropped();
        Ok(self.pushed)
    }

    fn drop_report(&self) -> Report {
        let mut report = Report { bytes: [0; 128], len: 0 };
        if self.len != 0 && self.bytes[(self.head + self.len - 1) % N] != b'\n' {
            let _ = report.write_str("\n");
        }
        // Maximum length is bounded by two u64 decimal counts.
        let _ = write!(report, "[log: {} bytes dropped; {} lines; source=serial-ring]\n",
                       self.dropped, self.dropped_lines);
        report
    }

    /// The report may use the reserve, but never overtake surviving bytes.
    /// Until it fits, newer ordinary records join this same loss interval.
    /// claim-lint:ok: tests/serial_log_loss.rs checks exact counts and ordering through the final drain.
    fn report_dropped(&mut self) {
        if self.dropped == 0 {
            return;
        }
        let report = self.drop_report();
        if self.len + report.len > N {
            return;
        }
        for &byte in &report.bytes[..report.len] {
            self.push(byte);
        }
        self.dropped = 0;
        self.dropped_lines = 0;
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
    lines: usize,
    last: Option<u8>,
}

impl<const N: usize> fmt::Write for RingWriter<'_, N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.lines += usize::from(byte == b'\n');
            self.last = Some(byte);
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
    bytes: [u8; 128],
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
