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
