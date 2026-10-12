//! Which writer's line the console port is in the middle of (issue 1387).
//!
//! Several writers share the console port: userspace output through the TTY,
//! the x86-64 kernel command console (its prompt and its echo) and kernel
//! output. Each writes a unit at a time under the port's exclusion, but a unit
//! need not end its line: a prompt, an echoed character or a breadcrumb does
//! not. A unit that begins while another writer's line is still open starts a
//! fresh line first, so a line on the port only ever holds one writer's bytes
//! and a record a writer starts on a fresh line keeps it.
use core::sync::atomic::{AtomicU8, Ordering};

/// A writer to the console port.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Writer {
    /// Kernel output: `serial_println!`, diagnostics, fatal reports.
    Kernel = 1,
    /// Userspace output through the console TTY, and its echo.
    Tty = 2,
    /// The x86-64 kernel serial command console.
    Console = 3,
}

/// The writer whose unterminated line the port is in, or 0 at the start of a line.
static OPEN: AtomicU8 = AtomicU8::new(0);

/// What `writer` writes ahead of `unit`: a line break when another writer's
/// line is open and `unit` does not begin with a line break of its own. Called
/// while the port is held.
pub fn lead(writer: Writer, unit: &[u8]) -> &'static [u8] {
    let open = OPEN.load(Ordering::Relaxed);
    if open == 0 || open == writer as u8 || matches!(unit.first(), None | Some(b'\r' | b'\n')) {
        b""
    } else {
        b"\r\n"
    }
}

/// Note that `writer` wrote `byte` to the port last. Called while the port is held.
pub fn wrote(writer: Writer, byte: u8) {
    OPEN.store(
        if byte == b'\n' { 0 } else { writer as u8 },
        Ordering::Relaxed,
    );
}
