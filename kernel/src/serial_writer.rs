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
//! UART ownership distinguishes output classes. The TTY producer additionally
//! separates userspace threads before emitting their records. Output written
//! without the port's exclusion (the
//! lock-free fatal, trace and capture writers, which must not wait on a holder
//! that may never release) neither reads nor updates this state.
//!
//! The TTY assembles userspace line fragments before handing a line to the
//! port. Kernel diagnostics therefore cannot appear between write(2)s in a
//! record, and different threads assemble different lines. Unterminated output
//! (including prompts) is handed on after 100 ms by the idle drain. The screen
//! grid still receives terminal bytes immediately; only shared serial needs
//! record assembly. Fatal, lock-free hardware writers remain emergency paths.
use alloc::vec::Vec;
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
    /// Suite evidence, excluded from terminal and producer capture.
    Evidence = 4,
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
    // Evidence has its own complete lines and must preserve producer framing.
    if writer == Writer::Evidence { return; }
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

/// Maximum time a userspace line waits for its newline. A fixed deadline from
/// its first fragment also bounds a writer that keeps producing without LF.
const PARTIAL_LINE_MS: u64 = 100;

struct PartialLine {
    thread: u64,
    started: u64,
    bytes: Vec<u8>,
}

struct TerminalLines {
    partial: Vec<PartialLine>,
    /// A timed-out fragment may leave the UART in this thread's line. Start
    /// another thread's record on its own line, even though both are TTY output.
    open_thread: Option<u64>,
}

impl TerminalLines {
    const fn new() -> Self {
        Self { partial: Vec::new(), open_thread: None }
    }

    fn emit(&mut self, thread: u64, bytes: &[u8], write: &mut impl FnMut(&[u8])) {
        if bytes.is_empty() { return; }
        if self.open_thread.is_some_and(|old| old != thread)
            && !matches!(bytes, [b'\n', ..] | [b'\r', b'\n', ..])
        {
            write(b"\r\n");
        }
        write(bytes);
        self.open_thread = (bytes.last() != Some(&b'\n')).then_some(thread);
    }

    fn drain(&mut self, now: u64, all: bool, write: &mut impl FnMut(&[u8])) {
        let mut at = 0;
        while at < self.partial.len() {
            if all || now.saturating_sub(self.partial[at].started) >= PARTIAL_LINE_MS {
                let line = self.partial.remove(at);
                self.emit(line.thread, &line.bytes, write);
            } else {
                at += 1;
            }
        }
    }

    fn append(&mut self, thread: u64, now: u64, bytes: &[u8], write: &mut impl FnMut(&[u8])) {
        self.drain(now, false, write);
        for segment in bytes.split_inclusive(|&byte| byte == b'\n') {
            let existing = self.partial.iter().position(|line| line.thread == thread);
            if let Some(at) = existing {
                self.partial[at].bytes.extend_from_slice(segment);
                if segment.last() == Some(&b'\n') {
                    let line = self.partial.remove(at);
                    self.emit(thread, &line.bytes, write);
                }
            } else if segment.last() == Some(&b'\n') {
                // A whole line needs no allocation and takes the existing
                // serial exclusion once, including its newline.
                self.emit(thread, segment, write);
            } else if !segment.is_empty() {
                self.partial.push(PartialLine { thread, started: now, bytes: segment.to_vec() });
            }
        }
    }
}

// Thread-context producers only. IRQ echo and kernel diagnostics never take
// this lock. Interrupts stay masked across the copy and port handoff, so a
// holder cannot be switched out underneath another producer on the same CPU.
static TERMINAL_LINES: spin::Mutex<TerminalLines> = spin::Mutex::new(TerminalLines::new());

/// Assemble console userspace output at its producer, across write(2)s and
/// ONLCR chunks. The sink must hand each supplied slice to serial in one piece.
/// Distinct threads never contribute bytes to the same pending line.
pub fn terminal(bytes: &[u8], mut write: impl FnMut(&[u8])) {
    let thread = crate::task::scheduler::current_thread_id().unwrap_or(0);
    let now = crate::time::get_monotonic_time();
    crate::arch_without_interrupts(|| {
        TERMINAL_LINES.lock().append(thread, now, bytes, &mut write);
    });
}

/// Service unterminated output from the existing idle serial drain, even if
/// neither its writer nor a kernel diagnostic produces any more output.
pub fn drain_terminal() {
    let now = crate::time::get_monotonic_time();
    crate::arch_without_interrupts(|| {
        if let Some(mut lines) = TERMINAL_LINES.try_lock() {
            lines.drain(now, false, &mut |line| crate::serial::write_bytes_atomic(Writer::Tty, line));
        }
    });
}

/// tcdrain must also hand pending producer fragments to the serial queue.
pub fn drain_terminal_all() {
    crate::arch_without_interrupts(|| {
        TERMINAL_LINES.lock().drain(0, true, &mut |line| crate::serial::write_bytes_atomic(Writer::Tty, line));
    });
}

/// A terminal flush discards its pending fragments as well as queued bytes.
pub fn discard_terminal() {
    crate::arch_without_interrupts(|| TERMINAL_LINES.lock().partial.clear());
}
