//! TTY Device Driver
//!
//! This module implements the TTY device driver that integrates the line discipline
//! with the rest of the kernel. It provides:
//!
//! - Input character processing from keyboard interrupts
//! - Read operations for userspace processes
//! - Terminal attribute management (termios)
//! - Signal delivery to foreground process groups
//! - Blocked reader management for blocking reads

// Note: Some functions are used for Phase 4+ TTY syscalls and ioctls.
// Functions that are part of the public API but not yet called:
// - get_termios, set_termios: Phase 4 ioctl (tcgetattr, tcsetattr)
// - get_foreground_pgrp: Phase 4 ioctl (TIOCGPGRP)
// - flush_input: Phase 4 ioctl (TCFLSH)
// - has_data: used internally

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use super::line_discipline::LineDiscipline;
use super::termios::Termios;
use crate::process::ProcessId;
use crate::signal::constants::{SIGINT, SIGQUIT, SIGTSTP};

/// POSIX error codes
/// Used by TtyDevice::read() for the line discipline read path.
/// Currently unused because keyboard input bypasses TTY (uses stdin directly).
/// Will be used when termios ioctls are fully implemented.
#[allow(dead_code)]
const EAGAIN: i32 = 11;

/// Static list of thread IDs blocked waiting for TTY input
static BLOCKED_READERS: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

/// #821 census. `deferred` counts the times the input IRQ entry saw a console
/// with no foreground process group and recorded the adoption for the
/// thread-context consumer instead of resolving a pid in the interrupt;
/// `adopted` counts the times that consumer took it. Both are read by the
/// boot-test oracle, which reports deltas across its own injections.
static TTY_IRQ_PM_DEFERRED: AtomicU64 = AtomicU64::new(0);
static TTY_IRQ_PM_ADOPTED: AtomicU64 = AtomicU64::new(0);

/// Times the input IRQ entry deferred a foreground-pgrp adoption (#821).
pub fn tty_irq_pm_deferred_count() -> u64 {
    TTY_IRQ_PM_DEFERRED.load(Ordering::Relaxed)
}

/// Times a thread-context reader took a deferred foreground-pgrp adoption (#821).
pub fn tty_irq_pm_adopted_count() -> u64 {
    TTY_IRQ_PM_ADOPTED.load(Ordering::Relaxed)
}

/// #822. The value published for `TtyDevice::foreground_pgrp` while the field
/// is unset.
///
/// 0 of the 3 production writers can produce this value. `tty/ioctl.rs`'s
/// TIOCSPGRP returns EINVAL for a negative pgrp, so it publishes a value in
/// `[0, i32::MAX]`; `process/creation.rs`'s 2 sites publish `pid.as_u64()`. A
/// ratchet leg pins the TIOCSPGRP refusal for that reason.
const FOREGROUND_PGRP_UNSET: u64 = u64::MAX;

/// #822 census: 6 counters, read by the boot-test oracle as deltas across its
/// own injections.
///
/// `TTY_IRQ_FG_LOCK_TOUCHES` counts each of the 5 acquisitions of a console
/// `foreground_pgrp` mutex in this file -- blocking and `try_lock` alike --
/// taken while a TTY interrupt entry's scope is open on this CPU;
/// `TTY_IRQ_FG_BLOCKING_ACQUIRES` counts the blocking subset. Both reading 0 is
/// the property this round establishes: the interrupt side answers
/// foreground-pgrp questions from a snapshot and takes 0 acquisitions.
///
/// `TTY_IRQ_FG_SNAPSHOT_READS` counts the snapshot reads that replaced those
/// acquisitions. The `SIGNAL_` three record the last interrupt-side signal
/// dispatch: how many there have been, the pid it was aimed at, and the signal
/// number. They are 3 separate relaxed cells rather than one transaction, so a
/// reader that samples them while 2 CPUs are dispatching can pair a count with
/// the other CPU's pid; the oracle reads them after a single injection with
/// preemption disabled, where that cannot happen.
static TTY_IRQ_FG_LOCK_TOUCHES: AtomicU64 = AtomicU64::new(0);
static TTY_IRQ_FG_BLOCKING_ACQUIRES: AtomicU64 = AtomicU64::new(0);
static TTY_IRQ_FG_SNAPSHOT_READS: AtomicU64 = AtomicU64::new(0);
static TTY_IRQ_FG_SIGNAL_CALLS: AtomicU64 = AtomicU64::new(0);
static TTY_IRQ_FG_SIGNAL_LAST_PID: AtomicU64 = AtomicU64::new(0);
static TTY_IRQ_FG_SIGNAL_LAST_SIG: AtomicU64 = AtomicU64::new(0);

/// Acquisitions of a console `foreground_pgrp` mutex taken inside a TTY
/// interrupt entry (#822). A reading of 0 is the property.
pub fn tty_irq_fg_lock_touches() -> u64 {
    TTY_IRQ_FG_LOCK_TOUCHES.load(Ordering::Relaxed)
}

/// The blocking subset of `tty_irq_fg_lock_touches` (#822). A reading above 0
/// is an interrupt entry waiting for a lock a thread can hold unmasked.
pub fn tty_irq_fg_blocking_acquires() -> u64 {
    TTY_IRQ_FG_BLOCKING_ACQUIRES.load(Ordering::Relaxed)
}

/// Times an interrupt-side path answered a foreground-pgrp question from the
/// lock-free snapshot (#822).
pub fn tty_irq_fg_snapshot_reads() -> u64 {
    TTY_IRQ_FG_SNAPSHOT_READS.load(Ordering::Relaxed)
}

/// Interrupt-side signal dispatches, and the pid and signal number of the last
/// one (#822).
pub fn tty_irq_fg_signal_census() -> (u64, u64, u64) {
    (
        TTY_IRQ_FG_SIGNAL_CALLS.load(Ordering::Relaxed),
        TTY_IRQ_FG_SIGNAL_LAST_PID.load(Ordering::Relaxed),
        TTY_IRQ_FG_SIGNAL_LAST_SIG.load(Ordering::Relaxed),
    )
}

/// Count an acquisition of a console `foreground_pgrp` mutex against an open
/// TTY interrupt entry (#822).
///
/// Called immediately BEFORE each acquisition, so a call that waits on a lock
/// its own CPU already owns is counted rather than lost -- the reading #821's
/// detector takes for the same reason.
#[inline(always)]
fn note_foreground_pgrp_acquisition(blocking: bool) {
    if crate::process::in_no_blocking_process_manager_scope() {
        TTY_IRQ_FG_LOCK_TOUCHES.fetch_add(1, Ordering::Relaxed);
        if blocking {
            TTY_IRQ_FG_BLOCKING_ACQUIRES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Maximum UART bytes emitted while interrupts are masked for one TTY write.
const SERIAL_ATOMIC_OUTPUT_BYTES: usize = 256;

fn for_each_line_segment(buf: &[u8], mut visit: impl FnMut(&[u8])) {
    for segment in buf.split_inclusive(|byte| *byte == b'\n') {
        visit(segment);
    }
}

// =============================================================================
// Test-only Signal Tracking
//
// When running tests, we track signal delivery attempts so tests can verify
// the signal flow without requiring full kernel process management.
// =============================================================================

#[cfg(test)]
mod signal_tracking {
    use alloc::collections::VecDeque;
    use spin::Mutex;

    /// Record of a signal delivery attempt
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SignalRecord {
        pub pid: u64,
        pub signal: u32,
    }

    /// Global list of signal delivery attempts for testing
    pub static SIGNAL_RECORDS: Mutex<VecDeque<SignalRecord>> = Mutex::new(VecDeque::new());

    /// Record a signal delivery attempt
    pub fn record_signal(pid: u64, signal: u32) {
        let mut records = SIGNAL_RECORDS.lock();
        records.push_back(SignalRecord { pid, signal });
    }

    /// Get all recorded signals and clear the list
    pub fn take_signals() -> VecDeque<SignalRecord> {
        let mut records = SIGNAL_RECORDS.lock();
        core::mem::take(&mut *records)
    }

    /// Clear all recorded signals
    pub fn clear_signals() {
        let mut records = SIGNAL_RECORDS.lock();
        records.clear();
    }
}

/// TTY device structure
///
/// Each TtyDevice represents a single terminal device (e.g., /dev/tty0).
/// It contains a line discipline for input processing and tracks the
/// foreground process group for signal delivery.
pub struct TtyDevice {
    /// TTY number (0 for console)
    pub num: u32,

    /// Line discipline for input processing
    ldisc: Mutex<LineDiscipline>,

    /// Foreground process group ID
    /// Signals (SIGINT, SIGQUIT, SIGTSTP) are sent to this group
    foreground_pgrp: Mutex<Option<u64>>,

    /// Session leader process ID (for future use)
    /// The session leader is the process that opened the controlling terminal
    session: Mutex<Option<ProcessId>>,

    /// #821: an input byte arrived while `foreground_pgrp` was unset, so the
    /// adoption of a foreground process group is owed to the next
    /// thread-context reader. Lock-free because the only writer that sets it
    /// runs in interrupt context.
    adopt_pending: AtomicBool,

    /// #822: `foreground_pgrp`'s value, published for interrupt-context
    /// readers, so that the 2 of them take 0 acquisitions of that mutex.
    ///
    /// # Ordering
    ///
    /// Both writes of the field go through `store_foreground_pgrp`, which
    /// holds the mutex across the field write and this store, and writes the
    /// field first. Writers are therefore serialised by the mutex, so this
    /// cell's value sequence is the field's own value sequence, lagging it by
    /// at most the remainder of one critical section and not leading it. An
    /// interrupt reader loads it `Acquire` and gets a value the field held at
    /// some instant -- during a `tcsetpgrp` window, the outgoing one. The
    /// `try_lock` this replaced gave a busy lock no value at all, and dropped
    /// the signal it was resolving.
    foreground_pgrp_snapshot: AtomicU64,
}

impl TtyDevice {
    /// Create a new TTY device
    ///
    /// # Arguments
    /// * `num` - TTY device number (0 for console)
    pub fn new(num: u32) -> Self {
        Self {
            num,
            ldisc: Mutex::new(LineDiscipline::new()),
            foreground_pgrp: Mutex::new(None),
            session: Mutex::new(None),
            adopt_pending: AtomicBool::new(false),
            foreground_pgrp_snapshot: AtomicU64::new(FOREGROUND_PGRP_UNSET),
        }
    }

    /// Process an input character from the keyboard
    ///
    /// This is the main entry point for keyboard input. The character is processed
    /// by the line discipline, which may:
    /// - Add it to the input buffer
    /// - Generate a signal (Ctrl+C, etc.)
    /// - Echo it back to the terminal
    ///
    /// This method acquires locks and should not be called from interrupt context
    /// without care. For interrupt context, use `input_char_nonblock`.
    #[allow(dead_code)] // Used by push_char in keyboard_task (conditionally compiled)
    pub fn input_char(&self, c: u8) {
        let mut ldisc = self.ldisc.lock();

        // Process the character through the line discipline
        // The echo callback writes to serial output
        let signal = ldisc.input_char(c, &mut |echo_c| {
            self.output_char(echo_c);
        });

        // If a signal was generated, send it to the foreground process group
        if let Some(sig) = signal {
            self.send_signal_to_foreground(sig);
        }

        // Transfer data from line discipline to stdin buffer when available
        // This bridges the TTY layer (which handles line editing, echo, signals)
        // with the stdin buffer (which userspace reads via read() syscall)
        if ldisc.has_data() {
            let mut buf = [0u8; 256];
            loop {
                match ldisc.read(&mut buf) {
                    Ok(0) => break, // No more data
                    Ok(n) => {
                        // Push each byte to stdin (no echo - TTY already handled that)
                        for &byte in &buf[..n] {
                            crate::ipc::stdin::push_byte_from_irq(byte);
                        }
                    }
                    Err(super::line_discipline::EOF_MARKER) => {
                        // EOF on empty line - don't push anything, read() will return 0
                        break;
                    }
                    Err(_) => break, // Other error, stop reading
                }
            }
            drop(ldisc); // Release lock before waking
            Self::wake_blocked_readers();
        }
    }

    /// Process an input character in a non-blocking manner
    ///
    /// This version uses try_lock and is safe for interrupt context.
    /// Returns true if the character was processed, false if the lock was busy.
    ///
    /// Used by keyboard interrupt handler when TTY is routed from interrupt context.
    ///
    /// # #821
    ///
    /// This body takes NO blocking `PROCESS_MANAGER` acquisition. It used to
    /// call `crate::process::current_pid()` here, which blocks in `manager()`,
    /// while a thread-context holder of the same lock can be running with
    /// interrupts live. The pid that call resolved was only ever used to name a
    /// foreground process group, and the pid an interrupt can see is the
    /// interrupted thread's, not the terminal's reader; so the adoption is
    /// recorded here and taken by the thread-context consumer
    /// (`adopt_deferred_foreground_pgrp`, driven from the stdin read path),
    /// where the reader's own pid is already in hand. The scope below makes a
    /// reintroduction fail loudly rather than silently.
    #[allow(dead_code)]
    pub fn input_char_nonblock(&self, c: u8) -> bool {
        let _no_blocking_pm = crate::process::NoBlockingProcessManagerScope::enter();

        // Owe a foreground process group to the next thread-context reader
        // while the field is unset. This lets signals work before a shell has
        // called tcsetpgrp.
        //
        // #822: read from the lock-free snapshot rather than the mutex. The
        // `try_lock` this replaced had a degrade arm that answered "set" when
        // the lock was busy, so a keystroke that arrived during a `tcsetpgrp`
        // silently skipped the deferral; the snapshot answers with the value
        // the field last held instead.
        if self.foreground_pgrp_snapshot().is_none() {
            self.adopt_pending.store(true, Ordering::Release);
            TTY_IRQ_PM_DEFERRED.fetch_add(1, Ordering::Relaxed);
        }

        let mut ldisc = match self.ldisc.try_lock() {
            Some(guard) => guard,
            None => return false,
        };

        // Process the character through line discipline (echo, signals, line editing)
        let signal = ldisc.input_char(c, &mut |echo_c| {
            self.output_char_nonblock(echo_c);
        });

        // Transfer data from line discipline to stdin buffer when available
        // This bridges the TTY layer (which handles line editing, echo, signals)
        // with the stdin buffer (which userspace reads via read() syscall)
        let has_data = ldisc.has_data();
        if has_data {
            let mut buf = [0u8; 256];
            loop {
                match ldisc.read(&mut buf) {
                    Ok(0) => break, // No more data
                    Ok(n) => {
                        // Push each byte to stdin (no echo - TTY already handled that)
                        for &byte in &buf[..n] {
                            crate::ipc::stdin::push_byte_from_irq(byte);
                        }
                    }
                    Err(super::line_discipline::EOF_MARKER) => {
                        // EOF on empty line - don't push anything, read() will return 0
                        break;
                    }
                    Err(_) => break, // Other error, stop reading
                }
            }
        }
        drop(ldisc);

        // Send signal if generated (non-blocking)
        if let Some(sig) = signal {
            self.send_signal_to_foreground_nonblock(sig);
        }

        // Wake blocked readers if data available
        if has_data {
            Self::wake_blocked_readers_nonblock();
        }

        true
    }

    /// Read data from the TTY
    ///
    /// This reads data that has been processed by the line discipline.
    /// In canonical mode, this returns complete lines.
    /// In raw mode, this returns individual characters.
    ///
    /// Currently unused because keyboard input bypasses TTY (uses stdin directly).
    /// Will be used when termios ioctls are fully implemented and shell uses TTY raw mode.
    ///
    /// # Arguments
    /// * `buf` - Buffer to read data into
    ///
    /// # Returns
    /// * `Ok(n)` - Number of bytes read (0 indicates EOF in canonical mode)
    /// * `Err(EAGAIN)` - No data available (for non-blocking reads)
    #[allow(dead_code)]
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let mut ldisc = self.ldisc.lock();

        // Check if data is available
        if !ldisc.has_data() {
            return Err(EAGAIN);
        }

        // Read from the line discipline
        match ldisc.read(buf) {
            Ok(n) => Ok(n),
            Err(super::line_discipline::EOF_MARKER) => Ok(0), // EOF returns 0 bytes
            Err(e) => Err(e),
        }
    }

    /// Check if there is data available to read
    ///
    /// Used by poll/select syscalls to check for readable data.
    #[allow(dead_code)]
    pub fn has_data(&self) -> bool {
        self.ldisc.lock().has_data()
    }

    /// Get the current termios settings
    ///
    /// Used by tcgetattr ioctl (Phase 4).
    #[allow(dead_code)]
    pub fn get_termios(&self) -> Termios {
        *self.ldisc.lock().termios()
    }

    /// Set the termios settings
    ///
    /// Used by tcsetattr ioctl (Phase 4).
    #[allow(dead_code)]
    pub fn set_termios(&self, termios: &Termios) {
        self.ldisc.lock().set_termios(*termios);
    }

    /// Set the foreground process group
    ///
    /// The foreground process group receives signals generated by the TTY
    /// (e.g., SIGINT from Ctrl+C).
    pub fn set_foreground_pgrp(&self, pgrp: u64) {
        self.store_foreground_pgrp(Some(pgrp));
    }

    /// Write `foreground_pgrp` and publish its snapshot, both under the mutex
    /// and in that order (#822).
    ///
    /// This is the sole writer of the field. Keeping the publication inside
    /// the critical section is what makes the snapshot's value sequence the
    /// field's own: 2 threads writing concurrently serialise here, so they
    /// cannot publish out of the order in which they wrote.
    fn store_foreground_pgrp(&self, pgrp: Option<u64>) {
        note_foreground_pgrp_acquisition(true);
        let mut guard = self.foreground_pgrp.lock();
        *guard = pgrp;
        self.foreground_pgrp_snapshot
            .store(pgrp.unwrap_or(FOREGROUND_PGRP_UNSET), Ordering::Release);
        drop(guard);
    }

    /// The foreground process group as last published, read without touching
    /// the mutex (#822).
    ///
    /// This is what interrupt-context paths use. See the field's own comment
    /// for the ordering it gives them.
    fn foreground_pgrp_snapshot(&self) -> Option<u64> {
        TTY_IRQ_FG_SNAPSHOT_READS.fetch_add(1, Ordering::Relaxed);
        match self.foreground_pgrp_snapshot.load(Ordering::Acquire) {
            FOREGROUND_PGRP_UNSET => None,
            pgrp => Some(pgrp),
        }
    }

    /// Get the foreground process group
    ///
    /// Used by TIOCGPGRP ioctl (Phase 4).
    #[allow(dead_code)]
    pub fn get_foreground_pgrp(&self) -> Option<u64> {
        note_foreground_pgrp_acquisition(true);
        *self.foreground_pgrp.lock()
    }

    /// Take an adoption the input IRQ entry deferred (#821).
    ///
    /// This is the thread-context half of the auto-set the IRQ path used to do
    /// inline. `pid` is the reading thread's own process, which is the answer
    /// the IRQ path could not have: an interrupt sees whichever thread it
    /// preempted, a reader is the terminal's foreground job by construction.
    ///
    /// Blocking on `foreground_pgrp` is legal here because this runs in thread
    /// context. The check-then-set shape is the one the IRQ path already had,
    /// so a second setter racing this one wins exactly as it did before; #822
    /// owns that lock's own discipline and is untouched.
    pub fn adopt_deferred_foreground_pgrp(&self, pid: ProcessId) {
        if !self.adopt_pending.swap(false, Ordering::AcqRel) {
            return;
        }
        if self.get_foreground_pgrp().is_none() {
            self.set_foreground_pgrp(pid.as_u64());
            TTY_IRQ_PM_ADOPTED.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Whether an adoption is owed to the next reader (#821).
    #[cfg(any(feature = "boot_tests", feature = "btrt"))]
    pub fn foreground_pgrp_adoption_pending(&self) -> bool {
        self.adopt_pending.load(Ordering::Acquire)
    }

    /// Clear the foreground process group.
    ///
    /// Boot-test only: the #821 oracle needs the console in the state the
    /// defect is reachable from (no foreground pgrp), and restores what it
    /// found afterwards. The 4 production writers of this field each set it to
    /// a pgrp: `kernel/src/process/creation.rs` at 2 sites,
    /// `kernel/src/tty/ioctl.rs` at 1, and this round's
    /// `adopt_deferred_foreground_pgrp`.
    #[cfg(any(feature = "boot_tests", feature = "btrt"))]
    pub fn set_foreground_pgrp_raw_for_test(&self, pgrp: Option<u64>) {
        self.store_foreground_pgrp(pgrp);
    }

    /// The published snapshot, read raw (#822 oracle).
    ///
    /// Boot-test only, and deliberately not counted in
    /// `TTY_IRQ_FG_SNAPSHOT_READS`: the oracle uses this to compare the
    /// snapshot against the field, not to exercise the interrupt path.
    #[cfg(any(feature = "boot_tests", feature = "btrt"))]
    pub fn foreground_pgrp_snapshot_for_test(&self) -> Option<u64> {
        match self.foreground_pgrp_snapshot.load(Ordering::Acquire) {
            FOREGROUND_PGRP_UNSET => None,
            pgrp => Some(pgrp),
        }
    }

    /// Whether `foreground_pgrp` is held right now (#822 oracle).
    ///
    /// Boot-test only. This is the oracle's independent reading that the lock
    /// really was owned at the instant of its injection, the way #821's
    /// `pm_busy_probe` reads PROCESS_MANAGER.
    #[cfg(any(feature = "boot_tests", feature = "btrt"))]
    pub fn foreground_pgrp_busy_for_test(&self) -> bool {
        note_foreground_pgrp_acquisition(false);
        self.foreground_pgrp.try_lock().is_none()
    }

    /// Hold `foreground_pgrp` across `hold`, the way `tcsetpgrp` holds it: in
    /// thread context, with interrupts unmasked (#822 oracle).
    ///
    /// Boot-test only. The mutex is a plain spin lock with no mask operation
    /// of its own, so a thread-context holder is unmasked by construction --
    /// which is the exposed shape the IRQ-context lock census names for this
    /// lock, and the one the oracle drives an input byte against.
    #[cfg(any(feature = "boot_tests", feature = "btrt"))]
    pub fn hold_foreground_pgrp_for_test(&self, hold: &mut dyn FnMut()) {
        note_foreground_pgrp_acquisition(true);
        let guard = self.foreground_pgrp.lock();
        hold();
        drop(guard);
    }

    /// Bytes the line discipline is holding in its canonical edit buffer.
    ///
    /// Boot-test only: the #821 oracle's reading that an injected byte reached
    /// the line discipline. `bytes_available()` cannot say so in canonical
    /// mode, where a line is not readable until its newline arrives.
    #[cfg(any(feature = "boot_tests", feature = "btrt"))]
    pub fn input_line_pending(&self) -> usize {
        self.ldisc.lock().pending_line_bytes()
    }

    /// Set the session leader
    #[allow(dead_code)]
    pub fn set_session(&self, pid: ProcessId) {
        *self.session.lock() = Some(pid);
    }

    /// Get the session leader
    #[allow(dead_code)]
    pub fn get_session(&self) -> Option<ProcessId> {
        *self.session.lock()
    }

    /// Flush the input queues
    ///
    /// Used by TCFLSH ioctl (Phase 4).
    #[allow(dead_code)]
    pub fn flush_input(&self) {
        self.ldisc.lock().flush_input();
    }

    /// Write a character to the terminal output
    ///
    /// This writes to serial immediately and queues for deferred framebuffer rendering.
    /// The render queue is drained by a dedicated kernel task with its own stack,
    /// avoiding the stack overflow that occurred with direct framebuffer calls.
    #[allow(dead_code)] // Used by input_char (conditionally compiled)
    pub fn output_char(&self, c: u8) {
        // Handle NL -> CR-NL translation if ONLCR is set
        let termios = self.ldisc.lock().termios().clone();
        if termios.is_opost() && termios.is_onlcr() && c == b'\n' {
            crate::serial::write_byte(b'\r');
            #[cfg(any(target_arch = "aarch64", feature = "interactive"))]
            let _ = crate::graphics::render_queue::queue_byte(b'\r');
        }
        crate::serial::write_byte(c);
        #[cfg(any(target_arch = "aarch64", feature = "interactive"))]
        let _ = crate::graphics::render_queue::queue_byte(c);
    }

    /// Write a buffer of bytes to the terminal output
    ///
    /// This is the optimized path for bulk output - it acquires the termios
    /// settings once and processes all bytes with those settings. This avoids
    /// the lock acquire/release overhead per character that output_char() has.
    ///
    /// Framebuffer rendering is deferred to a separate kernel task via the
    /// render queue, avoiding the stack overflow from deep rendering call stacks.
    ///
    /// # Arguments
    /// * `buf` - Buffer of bytes to write
    pub fn write_bytes(&self, buf: &[u8]) {
        // Get termios settings once for the entire buffer
        let termios = self.ldisc.lock().termios().clone();
        let do_onlcr = termios.is_opost() && termios.is_onlcr();

        // Each lock acquisition covers at most one line and one bounded UART
        // chunk, never the whole userspace buffer. Build ONLCR expansion before
        // entering the serial primitive so CR-LF stays in the same atomic write.
        for_each_line_segment(buf, |line| {
            let mut expanded = [0u8; SERIAL_ATOMIC_OUTPUT_BYTES];
            let mut expanded_len = 0usize;

            for &c in line {
                let output_len = if do_onlcr && c == b'\n' { 2 } else { 1 };
                if expanded_len + output_len > expanded.len() {
                    crate::serial::write_bytes_atomic(&expanded[..expanded_len]);
                    expanded_len = 0;
                }
                if output_len == 2 {
                    expanded[expanded_len] = b'\r';
                    expanded_len += 1;
                }
                expanded[expanded_len] = c;
                expanded_len += 1;
            }

            if expanded_len != 0 {
                crate::serial::write_bytes_atomic(&expanded[..expanded_len]);
            }

            // Both architectures use the render queue — it's lock-free on the
            // producer side and never drops data unless the buffer overflows.
            for &c in line {
                if do_onlcr && c == b'\n' {
                    #[cfg(any(target_arch = "aarch64", feature = "interactive"))]
                    let _ = crate::graphics::render_queue::queue_byte(b'\r');
                }
                #[cfg(any(target_arch = "aarch64", feature = "interactive"))]
                let _ = crate::graphics::render_queue::queue_byte(c);
            }
        });
    }

    /// Write a character to the terminal output (non-blocking)
    ///
    /// This version avoids blocking and is safe for interrupt context.
    /// Used by input_char_nonblock for echo in interrupt context.
    ///
    /// # Lock ordering
    ///
    /// On ARM64, this uses raw UART writes (no lock) instead of
    /// `crate::serial::write_byte()` to avoid acquiring SERIAL1 from IRQ
    /// context. On SMP, another CPU may hold SERIAL1 and be waiting for the
    /// SCHEDULER lock that this IRQ path also needs -- using the lock here
    /// would create a deadlock. The raw UART write is safe because PL011
    /// TX FIFO writes are atomic at the register level.
    ///
    /// On x86_64, `write_byte()` disables interrupts and acquires the lock,
    /// which is safe because x86_64 uses separate COM1/COM2 ports.
    pub fn output_char_nonblock(&self, c: u8) {
        // Try to get termios settings without blocking
        let do_crlf = if let Some(ldisc) = self.ldisc.try_lock() {
            let termios = ldisc.termios().clone();
            drop(ldisc);
            termios.is_opost() && termios.is_onlcr() && c == b'\n'
        } else {
            false
        };

        // Handle CR-LF translation
        if do_crlf {
            #[cfg(target_arch = "aarch64")]
            crate::serial_aarch64::raw_serial_char(b'\r');
            #[cfg(target_arch = "x86_64")]
            crate::serial::write_byte(b'\r');
            // Queue for deferred framebuffer rendering
            #[cfg(any(target_arch = "aarch64", feature = "interactive"))]
            let _ = crate::graphics::render_queue::queue_byte(b'\r');
        }

        // Write the character -- lock-free on ARM64, locked on x86_64
        #[cfg(target_arch = "aarch64")]
        crate::serial_aarch64::raw_serial_char(c);
        #[cfg(target_arch = "x86_64")]
        crate::serial::write_byte(c);

        // Queue for deferred framebuffer rendering
        // This is lock-free and safe for interrupt context
        #[cfg(any(target_arch = "aarch64", feature = "interactive"))]
        {
            let _queued = crate::graphics::render_queue::queue_byte(c);
        }
    }

    /// Send a signal to the foreground process group
    ///
    /// This is called when the line discipline generates a signal
    /// (e.g., SIGINT from Ctrl+C).
    #[allow(dead_code)] // Used by input_char (conditionally compiled) and tests
    pub fn send_signal_to_foreground(&self, sig: u32) {
        note_foreground_pgrp_acquisition(true);
        let pgrp = match *self.foreground_pgrp.lock() {
            Some(pgrp) => pgrp,
            None => {
                log::debug!("TTY{}: Signal {} but no foreground pgrp", self.num, sig);
                return;
            }
        };

        log::debug!(
            "TTY{}: Sending signal {} to foreground pgrp {}",
            self.num,
            sig,
            pgrp
        );

        // For now, treat pgrp as a PID directly
        // TODO: When process groups are fully implemented, iterate over all
        // processes in the group
        let pid = ProcessId::new(pgrp);
        Self::send_signal_to_process(pid, sig);
    }

    /// Send a signal to the foreground process group (non-blocking)
    ///
    /// Used by input_char_nonblock in interrupt context.
    ///
    /// # Lock ordering
    ///
    /// This runs from IRQ context so must NOT use serial_println! (acquires
    /// SERIAL1). Uses raw UART for any diagnostic output on ARM64.
    ///
    /// # #822
    ///
    /// The foreground pgrp comes from the lock-free snapshot, so this takes 0
    /// acquisitions of a lock a thread can hold with interrupts unmasked. What
    /// stood here was a `try_lock` whose degrade arm RETURNED: a Ctrl+C that
    /// arrived while `tcsetpgrp` held the mutex was dropped, and the drop was
    /// reported through `serial_println!` on x86 -- itself a lock, taken from
    /// interrupt context. Both are gone: a snapshot load has an answer for
    /// each of the 2 field states, so there is no arm to degrade into and no
    /// diagnostic to print.
    #[allow(dead_code)]
    fn send_signal_to_foreground_nonblock(&self, sig: u32) {
        if let Some(pgrp) = self.foreground_pgrp_snapshot() {
            let pid = ProcessId::new(pgrp);
            Self::send_signal_to_process_nonblock(pid, sig);
        }
        // No diagnostic for "no pgrp set" -- this is a normal condition
        // during early boot before any shell sets its foreground pgrp.
    }

    /// Send a signal to a specific process
    #[allow(dead_code)] // Used by send_signal_to_foreground (conditionally compiled) and tests
    fn send_signal_to_process(pid: ProcessId, sig: u32) {
        // In test mode, record the signal delivery attempt
        #[cfg(test)]
        {
            signal_tracking::record_signal(pid.as_u64(), sig);
        }

        // In production mode, actually deliver the signal
        #[cfg(not(test))]
        {
            use crate::process;

            // Get the process manager and set the signal pending
            let mut manager = process::manager();
            if let Some(ref mut pm) = *manager {
                if let Some(proc) = pm.get_process_mut(pid) {
                    proc.signals.set_pending(sig);

                    let sig_name = match sig {
                        SIGINT => "SIGINT",
                        SIGQUIT => "SIGQUIT",
                        SIGTSTP => "SIGTSTP",
                        _ => "UNKNOWN",
                    };
                    log::info!(
                        "TTY: Sent {} to process {} (PID {})",
                        sig_name,
                        proc.name,
                        pid.as_u64()
                    );

                    // If process is blocked waiting for signal, wake it
                    if let Some(ref thread) = proc.main_thread {
                        let thread_id = thread.id;
                        drop(manager);

                        // Wake the thread if it's blocked on a signal
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.unblock_for_signal(thread_id);
                        });
                    }
                }
            }
        }
    }

    /// Send a signal to a specific process (non-blocking)
    ///
    /// Used by send_signal_to_foreground_nonblock in interrupt context.
    ///
    /// # Lock ordering
    ///
    /// This runs from IRQ context and acquires PROCESS_MANAGER (level 2) and
    /// SCHEDULER (level 1) via `with_scheduler`. It must NOT use serial_println!
    /// (acquires SERIAL1, level 4) because SCHEDULER is a higher-priority lock.
    /// Its diagnostic is a raw UART write on both architectures.
    ///
    /// The x86_64 arm printed through `serial_println!` until this round --
    /// `SERIAL1.lock()`, blocking, taken from an interrupt entry while the
    /// `PROCESS_MANAGER` guard above it is still held, which is the acquisition
    /// the paragraph above forbids and the one scheduler.rs's lock-order
    /// comment names ("never acquire SERIAL1 while holding SCHEDULER or
    /// PROCESS_MANAGER"). It also formatted a string on that path. Both are
    /// gone; what is left writes bytes straight at the UART.
    /// claim-lint:ok: kernel/src/task/scheduler.rs:21 is the quoted line.
    #[allow(dead_code)]
    fn send_signal_to_process_nonblock(pid: ProcessId, sig: u32) {
        use crate::process;

        // #822 census, recorded before the lookup so it reads the dispatch the
        // interrupt side actually made rather than only the ones that found a
        // live row. See the statics' own comment for what a concurrent
        // dispatch on another CPU can do to the triple.
        TTY_IRQ_FG_SIGNAL_LAST_PID.store(pid.as_u64(), Ordering::Relaxed);
        TTY_IRQ_FG_SIGNAL_LAST_SIG.store(u64::from(sig), Ordering::Relaxed);
        TTY_IRQ_FG_SIGNAL_CALLS.fetch_add(1, Ordering::Relaxed);

        // Try to get the process manager without blocking
        if let Some(mut manager) = process::try_manager() {
            if let Some(ref mut pm) = *manager {
                if let Some(proc) = pm.get_process_mut(pid) {
                    proc.signals.set_pending(sig);

                    // Lock-free diagnostic output, one write per architecture
                    // and the same subject on both. The signal number and the
                    // pid are NOT written here: they are already published
                    // lock-free, by the census stores at the top of this
                    // function, and read back from thread context.
                    #[cfg(target_arch = "aarch64")]
                    {
                        crate::serial_aarch64::raw_serial_str(b"[TTY] sig sent to PID\n");
                    }
                    #[cfg(target_arch = "x86_64")]
                    {
                        crate::tracing::output::raw_serial_str("[TTY] sig sent to PID\n");
                    }

                    // CRITICAL: Wake the thread if it's blocked so it can receive the signal.
                    // Use unblock() instead of unblock_for_signal() because:
                    // - unblock_for_signal() only handles BlockedOnSignal state
                    // - unblock() handles BOTH Blocked (stdin read) and BlockedOnSignal
                    if let Some(ref thread) = proc.main_thread {
                        let thread_id = thread.id;
                        drop(manager);

                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.unblock(thread_id);
                        });
                    }
                }
                // No diagnostic for "process not found" in IRQ context --
                // this can happen normally during process teardown.
            }
        }
        // No diagnostic for "manager lock busy" in IRQ context --
        // the signal will be retried on next input.
    }

    /// Register a thread as blocked waiting for TTY input
    ///
    /// The thread will be woken when input becomes available.
    ///
    /// Currently unused because keyboard input bypasses TTY (uses stdin directly).
    /// Will be used when termios ioctls are fully implemented.
    #[allow(dead_code)]
    pub fn register_blocked_reader(thread_id: u64) {
        let mut readers = BLOCKED_READERS.lock();
        if !readers.contains(&thread_id) {
            readers.push_back(thread_id);
        }
    }

    /// Unregister a thread from blocked readers
    ///
    /// Called when a thread successfully reads data or encounters an error,
    /// after having previously registered as a blocked reader.
    ///
    /// Currently unused because keyboard input bypasses TTY (uses stdin directly).
    /// Will be used when termios ioctls are fully implemented.
    #[allow(dead_code)]
    pub fn unregister_blocked_reader(thread_id: u64) {
        let mut readers = BLOCKED_READERS.lock();
        readers.retain(|&id| id != thread_id);
    }

    /// Wake all blocked readers
    ///
    /// This is called when new input is available.
    #[allow(dead_code)] // Used by input_char (conditionally compiled)
    fn wake_blocked_readers() {
        let mut readers = BLOCKED_READERS.lock();
        while let Some(thread_id) = readers.pop_front() {
            // Wake the thread using the scheduler
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(thread_id);
            });
        }
    }

    /// Wake all blocked readers (non-blocking)
    ///
    /// Used by input_char_nonblock in interrupt context.
    #[allow(dead_code)]
    fn wake_blocked_readers_nonblock() {
        if let Some(mut readers) = BLOCKED_READERS.try_lock() {
            let thread_ids: VecDeque<u64> = readers.drain(..).collect();
            drop(readers);

            if thread_ids.is_empty() {
                return;
            }

            for thread_id in thread_ids {
                // Wake without blocking - the with_scheduler closure runs synchronously
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.unblock(thread_id);
                });
            }

            // Trigger reschedule so the woken thread runs soon
            crate::task::scheduler::set_need_resched();
        }
    }
}

/// Global console TTY device
static CONSOLE_TTY: Mutex<Option<Arc<TtyDevice>>> = Mutex::new(None);

/// Get a reference to the console TTY device
///
/// Returns None if the TTY subsystem has not been initialized.
pub fn console() -> Option<Arc<TtyDevice>> {
    CONSOLE_TTY.lock().clone()
}

/// Push a character to the console TTY
///
/// This is the main entry point for keyboard input.
/// It processes the character through the line discipline.
#[allow(dead_code)] // Used by keyboard_task (conditionally compiled)
pub fn push_char(c: u8) {
    if let Some(tty) = console() {
        tty.input_char(c);
    }
}

/// Write output through TTY processing
///
/// This is the POSIX-correct way to write to stdout/stderr.
/// The TTY layer handles output processing (OPOST, ONLCR, etc.).
///
/// This function uses the optimized `write_bytes` path which acquires
/// the termios settings once for the entire buffer, avoiding lock
/// acquire/release overhead per character.
///
/// # Arguments
/// * `buf` - Buffer of bytes to write
///
/// # Returns
/// Number of bytes written
pub fn write_output(buf: &[u8]) -> usize {
    if let Some(tty) = console() {
        // Use the optimized batched write that locks termios once
        tty.write_bytes(buf);
        buf.len()
    } else {
        // Fallback: write directly to serial if TTY not initialized
        for &c in buf {
            crate::serial::write_byte(c);
        }
        buf.len()
    }
}

/// Push a character to the console TTY (non-blocking)
///
/// This version is safe for interrupt context.
/// Returns true if the character was processed, false otherwise.
///
/// Used by keyboard interrupt handler when routing through TTY from ISR.
#[allow(dead_code)]
pub fn push_char_nonblock(c: u8) -> bool {
    // Try to get console without blocking
    if let Some(guard) = CONSOLE_TTY.try_lock() {
        if let Some(ref tty) = *guard {
            let tty = Arc::clone(tty);
            drop(guard);
            return tty.input_char_nonblock(c);
        }
    }
    false
}

/// Take any foreground-pgrp adoption the input IRQ entry deferred (#821).
///
/// Called from the stdin read path, which is the console's thread-context
/// consumer and already holds the reading process's own pid. A read with no
/// adoption owed costs one atomic swap.
pub fn adopt_foreground_pgrp_from_reader(pid: ProcessId) {
    if let Some(tty) = console() {
        tty.adopt_deferred_foreground_pgrp(pid);
    }
}

/// Initialize the console TTY device
pub fn init_console() {
    let tty = Arc::new(TtyDevice::new(0));
    *CONSOLE_TTY.lock() = Some(tty);
    log::info!("Console TTY initialized");
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // TtyDevice Construction Tests
    // =========================================================================

    #[test]
    fn test_tty_device_construction() {
        let tty = TtyDevice::new(0);
        assert_eq!(tty.num, 0);

        let tty5 = TtyDevice::new(5);
        assert_eq!(tty5.num, 5);
    }

    #[test]
    fn test_tty_device_initial_state() {
        let tty = TtyDevice::new(0);

        // No foreground pgrp initially
        assert_eq!(tty.get_foreground_pgrp(), None);

        // No session initially
        assert_eq!(tty.get_session(), None);

        // No data available initially
        assert!(!tty.has_data());
    }

    // =========================================================================
    // Foreground Process Group Tests
    // =========================================================================

    #[test]
    fn test_set_and_get_foreground_pgrp() {
        let tty = TtyDevice::new(0);

        // Initially no pgrp
        assert_eq!(tty.get_foreground_pgrp(), None);

        // Set pgrp
        tty.set_foreground_pgrp(1234);
        assert_eq!(tty.get_foreground_pgrp(), Some(1234));

        // Can change pgrp
        tty.set_foreground_pgrp(5678);
        assert_eq!(tty.get_foreground_pgrp(), Some(5678));
    }

    #[test]
    fn test_foreground_pgrp_zero_is_valid() {
        let tty = TtyDevice::new(0);

        // Process group 0 is a valid (special) value in POSIX
        tty.set_foreground_pgrp(0);
        assert_eq!(tty.get_foreground_pgrp(), Some(0));
    }

    // =========================================================================
    // Session Management Tests
    // =========================================================================

    #[test]
    fn test_set_and_get_session() {
        let tty = TtyDevice::new(0);

        // Initially no session
        assert_eq!(tty.get_session(), None);

        // Set session
        let pid = ProcessId::new(42);
        tty.set_session(pid);
        assert_eq!(tty.get_session(), Some(ProcessId::new(42)));

        // Can change session
        let new_pid = ProcessId::new(99);
        tty.set_session(new_pid);
        assert_eq!(tty.get_session(), Some(ProcessId::new(99)));
    }

    // =========================================================================
    // Termios Management Tests
    // =========================================================================

    #[test]
    fn test_get_termios_returns_default() {
        let tty = TtyDevice::new(0);
        let termios = tty.get_termios();

        // Should be in canonical mode by default
        assert!(termios.is_canonical());
        assert!(termios.is_echo());
    }

    #[test]
    fn test_set_termios() {
        let tty = TtyDevice::new(0);

        // Get and modify termios
        let mut termios = tty.get_termios();
        termios.set_raw();

        // Set modified termios
        tty.set_termios(&termios);

        // Verify it was set
        let updated = tty.get_termios();
        assert!(!updated.is_canonical());
    }

    // =========================================================================
    // Input Buffer Management Tests
    // =========================================================================

    #[test]
    fn test_flush_input_clears_buffer() {
        let tty = TtyDevice::new(0);

        // The ldisc starts empty, so flush should succeed
        tty.flush_input();

        // Still no data
        assert!(!tty.has_data());
    }

    #[test]
    fn test_read_returns_eagain_when_no_data() {
        let tty = TtyDevice::new(0);

        let mut buf = [0u8; 32];
        let result = tty.read(&mut buf);

        assert_eq!(result, Err(EAGAIN));
    }

    // =========================================================================
    // Blocked Reader Registration Tests
    // =========================================================================

    #[test]
    fn test_register_blocked_reader() {
        // Clear any existing blocked readers first
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }

        // Register a reader
        TtyDevice::register_blocked_reader(100);

        // Verify it's in the list
        {
            let readers = BLOCKED_READERS.lock();
            assert!(readers.contains(&100));
        }

        // Clean up
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }
    }

    #[test]
    fn test_register_blocked_reader_deduplication() {
        // Clear any existing blocked readers
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }

        // Register same reader multiple times
        TtyDevice::register_blocked_reader(200);
        TtyDevice::register_blocked_reader(200);
        TtyDevice::register_blocked_reader(200);

        // Should only appear once
        {
            let readers = BLOCKED_READERS.lock();
            let count = readers.iter().filter(|&&id| id == 200).count();
            assert_eq!(count, 1);
        }

        // Clean up
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }
    }

    #[test]
    fn test_register_multiple_blocked_readers() {
        // Clear any existing blocked readers
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }

        // Register multiple readers
        TtyDevice::register_blocked_reader(301);
        TtyDevice::register_blocked_reader(302);
        TtyDevice::register_blocked_reader(303);

        // All should be in the list
        {
            let readers = BLOCKED_READERS.lock();
            assert!(readers.contains(&301));
            assert!(readers.contains(&302));
            assert!(readers.contains(&303));
            assert_eq!(readers.len(), 3);
        }

        // Clean up
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }
    }

    // =========================================================================
    // Signal Delivery Flow Tests
    //
    // These tests verify the signal delivery *flow* using the test-mode
    // signal tracking mechanism. They check that:
    // 1. When Ctrl+C is input, a signal delivery is attempted
    // 2. The signal delivery respects the foreground pgrp setting
    // 3. Signal delivery is skipped when no foreground pgrp is set
    // 4. The correct signal is sent to the correct process
    // =========================================================================

    #[test]
    fn test_signal_not_delivered_without_foreground_pgrp() {
        // Clear any previous signal records
        signal_tracking::clear_signals();

        let tty = TtyDevice::new(0);

        // Don't set foreground pgrp
        assert_eq!(tty.get_foreground_pgrp(), None);

        // Try to send signals - should not result in any signal records
        tty.send_signal_to_foreground(SIGINT);
        tty.send_signal_to_foreground(SIGQUIT);
        tty.send_signal_to_foreground(SIGTSTP);

        // No signals should have been recorded (no foreground pgrp)
        let signals = signal_tracking::take_signals();
        assert!(
            signals.is_empty(),
            "Expected no signals when no foreground pgrp set, got {:?}",
            signals
        );
    }

    #[test]
    fn test_signal_delivery_with_foreground_pgrp() {
        // Clear any previous signal records
        signal_tracking::clear_signals();

        let tty = TtyDevice::new(0);

        // Set a foreground pgrp
        tty.set_foreground_pgrp(1000);
        assert_eq!(tty.get_foreground_pgrp(), Some(1000));

        // Send SIGINT to foreground
        tty.send_signal_to_foreground(SIGINT);

        // Check that the signal was recorded
        let signals = signal_tracking::take_signals();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].pid, 1000);
        assert_eq!(signals[0].signal, SIGINT);
    }

    #[test]
    fn test_sigquit_delivery() {
        signal_tracking::clear_signals();

        let tty = TtyDevice::new(0);
        tty.set_foreground_pgrp(2000);

        tty.send_signal_to_foreground(SIGQUIT);

        let signals = signal_tracking::take_signals();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].pid, 2000);
        assert_eq!(signals[0].signal, SIGQUIT);
    }

    #[test]
    fn test_sigtstp_delivery() {
        signal_tracking::clear_signals();

        let tty = TtyDevice::new(0);
        tty.set_foreground_pgrp(3000);

        tty.send_signal_to_foreground(SIGTSTP);

        let signals = signal_tracking::take_signals();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].pid, 3000);
        assert_eq!(signals[0].signal, SIGTSTP);
    }

    #[test]
    fn test_multiple_signals_to_same_pgrp() {
        signal_tracking::clear_signals();

        let tty = TtyDevice::new(0);
        tty.set_foreground_pgrp(4000);

        // Send multiple signals
        tty.send_signal_to_foreground(SIGINT);
        tty.send_signal_to_foreground(SIGQUIT);
        tty.send_signal_to_foreground(SIGTSTP);

        let signals = signal_tracking::take_signals();
        assert_eq!(signals.len(), 3);

        // All signals should be to the same process
        assert!(signals.iter().all(|s| s.pid == 4000));

        // Signals should be in order: SIGINT, SIGQUIT, SIGTSTP
        assert_eq!(signals[0].signal, SIGINT);
        assert_eq!(signals[1].signal, SIGQUIT);
        assert_eq!(signals[2].signal, SIGTSTP);
    }

    #[test]
    fn test_changing_foreground_pgrp_affects_signal_delivery() {
        signal_tracking::clear_signals();

        let tty = TtyDevice::new(0);

        // Set first pgrp and send signal
        tty.set_foreground_pgrp(5000);
        tty.send_signal_to_foreground(SIGINT);

        // Change pgrp and send another signal
        tty.set_foreground_pgrp(6000);
        tty.send_signal_to_foreground(SIGINT);

        let signals = signal_tracking::take_signals();
        assert_eq!(signals.len(), 2);

        // First signal should be to pid 5000
        assert_eq!(signals[0].pid, 5000);

        // Second signal should be to pid 6000
        assert_eq!(signals[1].pid, 6000);
    }

    // =========================================================================
    // Integration Tests: LineDiscipline Signal Generation -> Driver Delivery
    //
    // These tests verify that when the line discipline generates a signal,
    // the driver's input_char method correctly routes it to signal delivery.
    // =========================================================================

    #[test]
    fn test_line_discipline_signal_generation() {
        // Create a line discipline and verify signal generation
        let mut ldisc = LineDiscipline::new();

        // Ctrl+C should generate SIGINT
        let signal = ldisc.input_char(0x03, &mut |_| {});
        assert_eq!(signal, Some(SIGINT));

        // Ctrl+\ should generate SIGQUIT
        let signal = ldisc.input_char(0x1C, &mut |_| {});
        assert_eq!(signal, Some(SIGQUIT));

        // Ctrl+Z should generate SIGTSTP
        let signal = ldisc.input_char(0x1A, &mut |_| {});
        assert_eq!(signal, Some(SIGTSTP));
    }

    #[test]
    fn test_signal_constants_match_posix() {
        // POSIX signal numbers
        assert_eq!(SIGINT, 2);
        assert_eq!(SIGQUIT, 3);
        assert_eq!(SIGTSTP, 20);
    }

    #[test]
    fn test_direct_send_signal_to_process() {
        signal_tracking::clear_signals();

        // Call send_signal_to_process directly
        let pid = ProcessId::new(7777);
        TtyDevice::send_signal_to_process(pid, SIGINT);

        let signals = signal_tracking::take_signals();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].pid, 7777);
        assert_eq!(signals[0].signal, SIGINT);
    }

    #[test]
    fn test_send_signal_with_zero_pid() {
        signal_tracking::clear_signals();

        let tty = TtyDevice::new(0);
        tty.set_foreground_pgrp(0); // Special "all processes in session" in POSIX

        tty.send_signal_to_foreground(SIGINT);

        let signals = signal_tracking::take_signals();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].pid, 0);
    }

    #[test]
    fn test_signal_record_equality() {
        let r1 = signal_tracking::SignalRecord {
            pid: 100,
            signal: SIGINT,
        };
        let r2 = signal_tracking::SignalRecord {
            pid: 100,
            signal: SIGINT,
        };
        let r3 = signal_tracking::SignalRecord {
            pid: 100,
            signal: SIGQUIT,
        };

        assert_eq!(r1, r2);
        assert_ne!(r1, r3);
    }

    // =========================================================================
    // EAGAIN Error Code Tests
    // =========================================================================

    #[test]
    fn test_eagain_constant() {
        // POSIX defines EAGAIN as 11
        assert_eq!(EAGAIN, 11);
    }

    // =========================================================================
    // Console TTY Global State Tests
    //
    // Note: These tests interact with global state and should be run carefully.
    // In a real test harness, we'd isolate these or use test fixtures.
    // =========================================================================

    #[test]
    fn test_console_before_init_returns_none() {
        // Save current state
        let saved = CONSOLE_TTY.lock().take();

        // console() should return None when not initialized
        assert!(console().is_none());

        // Restore state
        *CONSOLE_TTY.lock() = saved;
    }

    #[test]
    fn test_console_after_init() {
        // Save current state
        let saved = CONSOLE_TTY.lock().take();

        // Initialize
        let tty = Arc::new(TtyDevice::new(0));
        *CONSOLE_TTY.lock() = Some(tty);

        // console() should return Some
        assert!(console().is_some());
        assert_eq!(console().unwrap().num, 0);

        // Restore state
        *CONSOLE_TTY.lock() = saved;
    }

    #[test]
    fn test_push_char_without_console() {
        // Save current state
        let saved = CONSOLE_TTY.lock().take();

        // push_char should not panic when console is not initialized
        push_char(b'a');
        push_char(b'\n');

        // Restore state
        *CONSOLE_TTY.lock() = saved;
    }

    // =========================================================================
    // Thread Safety Tests
    //
    // These tests verify that concurrent access to TTY state is safe.
    // =========================================================================

    #[test]
    fn test_foreground_pgrp_lock_safety() {
        let tty = Arc::new(TtyDevice::new(0));

        // Rapid get/set operations should not deadlock
        for i in 0..100 {
            tty.set_foreground_pgrp(i);
            let _ = tty.get_foreground_pgrp();
        }
    }

    #[test]
    fn test_termios_lock_safety() {
        let tty = Arc::new(TtyDevice::new(0));

        // Rapid get/set operations should not deadlock
        for _ in 0..100 {
            let termios = tty.get_termios();
            tty.set_termios(&termios);
        }
    }

    // =========================================================================
    // ProcessId Type Tests
    // =========================================================================

    #[test]
    fn test_process_id_construction() {
        let pid = ProcessId::new(42);
        assert_eq!(pid.as_u64(), 42);

        let pid_zero = ProcessId::new(0);
        assert_eq!(pid_zero.as_u64(), 0);
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_large_pgrp_value() {
        let tty = TtyDevice::new(0);
        let large_pgrp = u64::MAX;

        tty.set_foreground_pgrp(large_pgrp);
        assert_eq!(tty.get_foreground_pgrp(), Some(large_pgrp));
    }

    #[test]
    fn test_large_tty_number() {
        let tty = TtyDevice::new(u32::MAX);
        assert_eq!(tty.num, u32::MAX);
    }

    #[test]
    fn test_many_blocked_readers() {
        // Clear any existing blocked readers
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }

        // Register many readers
        for i in 1000..1100 {
            TtyDevice::register_blocked_reader(i);
        }

        // All should be present
        {
            let readers = BLOCKED_READERS.lock();
            assert_eq!(readers.len(), 100);
        }

        // Clean up
        {
            let mut readers = BLOCKED_READERS.lock();
            readers.clear();
        }
    }
}
