//! Synchronous record admission and console TTY output.
use super::{grid, vt, with_grid, REPLY_QUEUED, RUNNING, SESSION};
#[cfg(any(target_arch = "x86_64", test))]
use core::fmt::{self, Write};
use core::sync::atomic::Ordering;

/// A log line formatted into fixed storage; longer lines are cut short.
#[cfg(any(target_arch = "x86_64", test))]
struct Line {
    bytes: [u8; 256],
    len: usize,
}

#[cfg(any(target_arch = "x86_64", test))]
impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let take = text.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Put a kernel log record on the console, as its own line.
#[cfg(any(target_arch = "x86_64", test))]
pub fn log_record(level: log::Level, args: &fmt::Arguments) {
    if level > log::Level::Info || (SESSION.load(Ordering::Acquire) && level > log::Level::Warn) {
        return;
    }
    let mut line = Line {
        bytes: [0; 256],
        len: 0,
    };
    let _ = write!(line, "{:>5}: {}", level, args);
    with_grid(|console| {
        let tty1 = &mut console.vts[0].grid;
        tty1.write_kernel(&line.bytes[..line.len], true);
        tty1.write_kernel(b"\n", false);
        console.kernel_line_open = false;
    });
}

/// Put kernel serial output on the console: a record from `serial_line`, a
/// line or part of one. It starts a fresh row unless it continues the kernel's
/// own open line, and each line it ends returns to the first column (kernel
/// records end lines with LF alone). Kernel output carries no level, so it is
/// drawn as INFO.
#[cfg(target_arch = "aarch64")]
pub fn log_bytes(bytes: &[u8]) {
    log_bytes_at(log::Level::Info, bytes);
}

/// A serial record with a known severity, retained even during a quiet session
/// when it is a warning or error. Untyped serial output is INFO; diagnostics
/// (`diag_println!`) are DEBUG and stay on serial.
#[cfg(target_arch = "aarch64")]
pub fn log_bytes_at(level: log::Level, bytes: &[u8]) {
    if bytes.is_empty()
        || level > log::Level::Info
        || (SESSION.load(Ordering::Acquire) && level > log::Level::Warn)
    {
        return;
    }
    with_grid(|console| {
        console.vts[0]
            .grid
            .write_kernel(bytes, !console.kernel_line_open);
        console.kernel_line_open = !bytes.ends_with(b"\n");
    });
}

/// Put console TTY output (tty1's) into its grid, and hand `serial` the
/// bytes tty1's serial copy carries (contract section 5.4). While the render
/// thread runs to deliver them, the screen answers tty1's report requests and
/// the serial copy leaves out each one it answered, so a terminal on serial
/// does not answer it too; before that the screen does not answer and serial
/// gets the bytes as written.
pub fn write_tty(bytes: &[u8], mut serial: impl FnMut(&[u8])) {
    const CHUNK: usize = 256;
    vt::ensure_history(0);
    let answering = RUNNING.load(Ordering::Acquire);
    for chunk in bytes.chunks(CHUNK) {
        let mut out = [0u8; CHUNK + grid::HELD];
        let forwarded = with_grid(|console| {
            if console.kernel_line_open {
                console.vts[0].grid.begin_line();
                console.kernel_line_open = false;
            }
            let vt::Vt { grid, replies, .. } = &mut console.vts[0];
            if answering {
                grid.write_forwarding(chunk, |reply| {
                    let kept = replies.push(reply);
                    if kept {
                        REPLY_QUEUED.store(true, Ordering::SeqCst);
                    }
                    kept
                }, &mut out)
            } else {
                grid.write(chunk);
                out[..chunk.len()].copy_from_slice(chunk);
                chunk.len()
            }
        });
        match forwarded {
            Some(n) => serial(&out[..n]),
            None => serial(chunk),
        }
    }
}

/// A session other than init's took the console as its controlling terminal
/// (`true`), or the console is back in init's session or out of sessions (`false`).
pub fn console_session(held: bool) {
    SESSION.store(held, Ordering::Release);
}
