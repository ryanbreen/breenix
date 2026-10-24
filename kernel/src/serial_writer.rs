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
//! The TTY admits up to 4096 input bytes, after output processing, as one
//! serial unit (#2411/W23). The port's exclusion keeps kernel output out of
//! that unit: SERIAL1 on x86-64, pinned UART ownership on ARM64. Larger writes
//! may have kernel output between their units. [`exclusive`] keeps the
//! console's prompt and echo out of those gaps and excludes other TTY writes;
//! its holder runs with preemption disabled. Kernel output uses the port's
//! exclusion directly, since it can run in interrupt and fault context.
use core::sync::atomic::{AtomicU8, Ordering};

/// A writer to the console port.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Writer {
    /// Formatted kernel output: `serial_println!` and log records.
    Kernel = 1,
    /// Userspace output through the console TTY, and its echo.
    Tty = 2,
    /// The kernel serial console's prompt and echo.
    Console = 3,
    /// ARM64's raw diagnostic fragments, distinct from formatted log lines.
    #[cfg(target_arch = "aarch64")]
    Diagnostic = 4,
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

/// The port is in the middle of some writer's line. Called while the port is held.
pub fn line_open() -> bool {
    OPEN.load(Ordering::Relaxed) != 0
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
