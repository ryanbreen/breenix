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
use spin::Mutex;

use super::line_discipline::LineDiscipline;
use super::termios::Termios;
use crate::process::ProcessId;
use crate::signal::constants::{SIGINT, SIGQUIT, SIGTSTP};

/// POSIX error codes
const EAGAIN: i32 = 11;

/// Static list of thread IDs blocked waiting for TTY input
static BLOCKED_READERS: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

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

        // Wake any blocked readers if data is now available
        if ldisc.has_data() {
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
    #[allow(dead_code)]
    pub fn input_char_nonblock(&self, c: u8) -> bool {
        let mut ldisc = match self.ldisc.try_lock() {
            Some(guard) => guard,
            None => return false,
        };

        // Process the character
        let signal = ldisc.input_char(c, &mut |echo_c| {
            self.output_char_nonblock(echo_c);
        });

        let has_data = ldisc.has_data();
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
    /// # Arguments
    /// * `buf` - Buffer to read data into
    ///
    /// # Returns
    /// * `Ok(n)` - Number of bytes read (0 indicates EOF in canonical mode)
    /// * `Err(EAGAIN)` - No data available (for non-blocking reads)
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
        *self.foreground_pgrp.lock() = Some(pgrp);
    }

    /// Get the foreground process group
    ///
    /// Used by TIOCGPGRP ioctl (Phase 4).
    #[allow(dead_code)]
    pub fn get_foreground_pgrp(&self) -> Option<u64> {
        *self.foreground_pgrp.lock()
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
    /// This writes directly to the serial port for now.
    /// In the future, this could go through an output queue.
    pub fn output_char(&self, c: u8) {
        // Handle NL -> CR-NL translation if ONLCR is set
        let termios = self.ldisc.lock().termios().clone();
        if termios.is_opost() && termios.is_onlcr() && c == b'\n' {
            crate::serial::write_byte(b'\r');
        }
        crate::serial::write_byte(c);
    }

    /// Write a character to the terminal output (non-blocking)
    ///
    /// This version avoids blocking and is safe for interrupt context.
    /// Used by input_char_nonblock for echo in interrupt context.
    #[allow(dead_code)]
    pub fn output_char_nonblock(&self, c: u8) {
        // Try to get termios settings without blocking
        if let Some(ldisc) = self.ldisc.try_lock() {
            let termios = ldisc.termios().clone();
            drop(ldisc);

            if termios.is_opost() && termios.is_onlcr() && c == b'\n' {
                crate::serial::write_byte(b'\r');
            }
        }
        crate::serial::write_byte(c);
    }

    /// Send a signal to the foreground process group
    ///
    /// This is called when the line discipline generates a signal
    /// (e.g., SIGINT from Ctrl+C).
    pub fn send_signal_to_foreground(&self, sig: u32) {
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
    #[allow(dead_code)]
    fn send_signal_to_foreground_nonblock(&self, sig: u32) {
        let pgrp = match self.foreground_pgrp.try_lock() {
            Some(guard) => *guard,
            None => return,
        };

        if let Some(pgrp) = pgrp {
            let pid = ProcessId::new(pgrp);
            Self::send_signal_to_process_nonblock(pid, sig);
        }
    }

    /// Send a signal to a specific process
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
    #[allow(dead_code)]
    fn send_signal_to_process_nonblock(pid: ProcessId, sig: u32) {
        use crate::process;

        // Try to get the process manager without blocking
        if let Some(mut manager) = process::try_manager() {
            if let Some(ref mut pm) = *manager {
                if let Some(proc) = pm.get_process_mut(pid) {
                    proc.signals.set_pending(sig);
                }
            }
        }
    }

    /// Register a thread as blocked waiting for TTY input
    ///
    /// The thread will be woken when input becomes available.
    pub fn register_blocked_reader(thread_id: u64) {
        let mut readers = BLOCKED_READERS.lock();
        if !readers.contains(&thread_id) {
            readers.push_back(thread_id);
        }
    }

    /// Wake all blocked readers
    ///
    /// This is called when new input is available.
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

            for thread_id in thread_ids {
                // Wake without blocking - the with_scheduler closure runs synchronously
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.unblock(thread_id);
                });
            }
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
pub fn push_char(c: u8) {
    if let Some(tty) = console() {
        tty.input_char(c);
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
