//! Which writer's line the console port is in the middle of (issue 1387).
//!
//! Several writers share the console port: userspace output through the TTY,
//! the kernel serial console's prompt and echo, and kernel output. Each writes
//! a unit at a time under the port's exclusion (x86-64's SERIAL1 lock, ARM64's
//! `serial_line` UART ownership), and a unit need not end its line: a prompt,
//! an echoed character or a breadcrumb does not. A unit that begins while
//! another writer's line is open starts a fresh line first, so a record that a
//! writer begins on a fresh line is not appended to another writer's line.
//!
//! A writer here is a class of output, not a thread or a CPU: two processes
//! that each write part of a line to the TTY share that line, as they do on
//! other terminals. Output written without the port's exclusion (the
//! lock-free fatal, trace and capture writers, which must not wait on a holder
//! that may never release) neither reads nor updates this state.
//!
//! The TTY writes a long line as several units of at most 256 bytes, so it
//! does not mask interrupts for the whole line. [`exclusive`] keeps the
//! console's prompt and echo out of the gaps between those units: a TTY write
//! holds it from its first unit to its last, as Linux's tty write lock does,
//! and so does each console write. Its holder runs with preemption disabled. Kernel output does not wait for it, since
//! it is also written from interrupt and fault paths, so a kernel line that
//! arrives between two units of one TTY line still starts a line of its own
//! inside that TTY line.
use core::sync::atomic::{AtomicU8, Ordering};

/// A writer to the console port.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Writer {
    /// Kernel output: `serial_println!`, diagnostics, breadcrumbs.
    Kernel = 1,
    /// Userspace output through the console TTY, and its echo.
    Tty = 2,
    /// The kernel serial console's prompt and echo.
    Console = 3,
}

/// The writer whose unterminated line the port is in, or 0 at the start of a line.
static OPEN: AtomicU8 = AtomicU8::new(0);

/// Held across one TTY write or one console write.
static EXCLUSIVE: spin::Mutex<()> = spin::Mutex::new(());

/// What `writer` writes ahead of `unit`: CR-LF when another writer's line is
/// open and `unit` does not itself begin by ending that line (with LF or
/// CR-LF). Called while the port is held.
pub fn lead(writer: Writer, unit: &[u8]) -> &'static [u8] {
    let open = OPEN.load(Ordering::Relaxed);
    let ends_line = matches!(unit, [] | [b'\n', ..] | [b'\r', b'\n', ..]);
    if open == 0 || open == writer as u8 || ends_line {
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

/// Run `write`, one TTY write or one console write, with no other such write
/// in progress. Thread context only, never an interrupt handler.
///
/// Preemption is disabled from before the wait until the lock is released, as
/// `spin_lock` does. A holder switched out mid-write would otherwise leave the
/// next waiter on its CPU spinning for good: x86-64 syscalls run with
/// interrupts masked, so nothing would switch the holder back in.
pub fn exclusive<R>(write: impl FnOnce() -> R) -> R {
    crate::per_cpu::preempt_disable();
    let turn = EXCLUSIVE.lock();
    let result = write();
    drop(turn);
    crate::per_cpu::preempt_enable();
    result
}

/// Write one unit of the kernel serial console's prompt or echo.
pub fn console(unit: &[u8]) {
    exclusive(|| crate::serial::write_bytes_atomic(Writer::Console, unit));
}
