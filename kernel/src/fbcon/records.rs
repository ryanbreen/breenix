//! Record admission runs synchronously, before getty can print a prompt.
use super::with_grid;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

/// A session other than init's has the console as its controlling terminal.
pub(super) static SESSION: AtomicBool = AtomicBool::new(false);
/// A process has the display.
pub(super) static DISPLAY_TAKEN: AtomicBool = AtomicBool::new(false);
/// A log line formatted into fixed storage; longer lines are cut short.
struct Line {
    bytes: [u8; 256],
    len: usize,
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let take = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Put a kernel log record on the console, as its own line.
pub fn log_record(level: log::Level, args: &fmt::Arguments) {
    if DISPLAY_TAKEN.load(Ordering::Acquire)
        || (SESSION.load(Ordering::Acquire) && level > log::Level::Warn)
    {
        return;
    }
    let mut line = Line {
        bytes: [0; 256],
        len: 0,
    };
    let _ = write!(line, "{level:>5}: {args}");
    with_grid(|grid| {
        grid.begin_line();
        grid.write(&line.bytes[..line.len]);
        grid.write(b"\r\n");
    });
}

/// Put console TTY output on the console.
pub fn write_tty(bytes: &[u8]) {
    if !DISPLAY_TAKEN.load(Ordering::Acquire) {
        with_grid(|grid| grid.write(bytes));
    }
}

/// A session other than init's took the console as its controlling terminal
/// (`true`), or the console is back in init's session or out of sessions (`false`).
pub fn console_session(held: bool) {
    SESSION.store(held, Ordering::Release);
}

/// A process has the display: stop drawing on it.
pub fn take_display() {
    DISPLAY_TAKEN.store(true, Ordering::Release);
}
