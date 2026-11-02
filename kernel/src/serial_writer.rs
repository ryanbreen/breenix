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
//! that may stay stopped) neither reads nor updates this state.
//!
//! The TTY assembles userspace line fragments before handing a line to the
//! port. Fragments retained together are handed off together; a deadline,
//! storage pressure or emergency drain can end that grouping. Different threads
//! assemble different lines in first-fragment order. Unterminated output
//! (including prompts) is handed on after 100 ms by the idle drain. The screen
//! grid still receives terminal bytes immediately; only shared serial needs
//! record assembly. Fatal, lock-free hardware writers remain emergency paths.
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

// The total retained payload and each UART handoff are bounded independently
// of the caller's write length. Fixed storage makes allocation failure impossible
// while the producer lock and interrupt mask are held.
const LINE_BYTES: usize = 4096;
const LINE_SLOTS: usize = 8;

struct PartialLine {
    thread: u64,
    started: u64,
    bytes: [u8; LINE_BYTES],
    len: usize,
    complete: bool,
}

impl PartialLine {
    const fn new() -> Self {
        Self { thread: 0, started: 0, bytes: [0; LINE_BYTES], len: 0, complete: false }
    }
}

struct TerminalLines {
    partial: [PartialLine; LINE_SLOTS],
    len: usize,
    retained: usize,
    /// A timed-out fragment may leave the UART in this thread's line.
    open_thread: Option<u64>,
}

impl TerminalLines {
    const fn new() -> Self {
        Self {
            partial: [const { PartialLine::new() }; LINE_SLOTS],
            len: 0, retained: 0, open_thread: None,
        }
    }

    fn emit_first(&mut self, write: &mut impl FnMut(&[u8])) {
        if self.len == 0 { return; }
        let line = &self.partial[0];
        let bytes = &line.bytes[..line.len];
        if self.open_thread.is_some_and(|old| old != line.thread)
            && line_open()
            && !matches!(bytes, [b'\n', ..] | [b'\r', b'\n', ..])
        {
            write(b"\r\n");
        }
        write(bytes);
        self.open_thread = (bytes.last() != Some(&b'\n')).then_some(line.thread);
        self.retained -= line.len;
        self.len -= 1;
        // Keep first-fragment order while recycling the fixed buffers.
        for at in 0..self.len { self.partial.swap(at, at + 1); }
        self.partial[self.len].len = 0;
        self.partial[self.len].complete = false;
    }

    fn drain(&mut self, now: u64, all: bool, write: &mut impl FnMut(&[u8])) {
        while self.len != 0 && (all || self.partial[0].complete
            || now.saturating_sub(self.partial[0].started) >= PARTIAL_LINE_MS)
        {
            self.emit_first(write);
        }
    }

    fn append(&mut self, thread: u64, now: u64, bytes: &[u8], write: &mut impl FnMut(&[u8])) {
        self.drain(now, false, write);
        for segment in bytes.split_inclusive(|&byte| byte == b'\n') {
            let mut remaining = segment;
            while !remaining.is_empty() {
                let existing = self.partial[..self.len].iter()
                    .position(|line| line.thread == thread && !line.complete);
                if self.retained == LINE_BYTES || (existing.is_none() && self.len == LINE_SLOTS) {
                    self.emit_first(write);
                    continue;
                }
                let at = existing.unwrap_or(self.len);
                if at == self.len {
                    self.partial[at].thread = thread;
                    self.partial[at].started = now;
                    self.len += 1;
                }
                let n = remaining.len().min(LINE_BYTES - self.retained);
                let line = &mut self.partial[at];
                line.bytes[line.len..line.len + n].copy_from_slice(&remaining[..n]);
                line.len += n;
                self.retained += n;
                remaining = &remaining[n..];
                line.complete = line.bytes[line.len - 1] == b'\n';
                // Later completed records wait behind the oldest fragment.
                self.drain(now, false, write);
            }
        }
    }
}

// Thread-context producers only. Kernel diagnostics bypass
// this lock. Interrupts stay masked across the copy and port handoff, so a
// holder cannot be switched out underneath another producer on the same CPU.
static TERMINAL_LINES: spin::Mutex<TerminalLines> = spin::Mutex::new(TerminalLines::new());

/// Assemble console userspace output at its producer, across write(2)s and
/// ONLCR chunks. The sink must hand each supplied slice to serial in one piece.
/// Pending lines are keyed by their originating thread.
pub fn terminal(bytes: &[u8], mut write: impl FnMut(&[u8])) {
    let thread = crate::task::scheduler::current_thread_id().unwrap_or(0);
    let now = crate::time::get_monotonic_time();
    for chunk in bytes.chunks(LINE_BYTES) {
        crate::arch_without_interrupts(|| {
            TERMINAL_LINES.lock().append(thread, now, chunk, &mut write);
        });
    }
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
    crate::arch_without_interrupts(|| {
        let mut lines = TERMINAL_LINES.lock();
        for line in &mut lines.partial { line.len = 0; line.complete = false; }
        lines.len = 0;
        lines.retained = 0;
        lines.open_thread = None;
    });
}

/// Drain fragments before panic output without waiting for a stopped producer.
/// The emergency sink bypasses the ordinary queue and its locks.
pub fn drain_terminal_emergency() {
    crate::arch_without_interrupts(|| {
        if let Some(mut lines) = TERMINAL_LINES.try_lock() {
            lines.drain(0, true, &mut crate::serial::write_terminal_emergency);
        }
    });
}

/// Order echo after older output. A busy producer leaves echo in the TTY's
/// fixed pending ring; interrupt context does not spin on the producer lock.
pub fn terminal_echo(write: impl FnOnce()) -> bool {
    crate::arch_without_interrupts(|| {
        let Some(mut lines) = TERMINAL_LINES.try_lock() else { return false };
        lines.drain(0, true, &mut |line| crate::serial::write_bytes_atomic(Writer::Tty, line));
        write();
        lines.open_thread = None;
        true
    })
}
