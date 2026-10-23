//! TTY Line Discipline Implementation
//!
//! The line discipline is responsible for processing input characters according
//! to the terminal settings (termios). It implements:
//!
//! - **Canonical mode**: Line-by-line input with line editing support
//! - **Raw mode**: Characters passed through immediately
//! - **Signal generation**: Ctrl+C, Ctrl+\, Ctrl+Z generate signals
//! - **Echo handling**: Characters echoed back to the terminal
//!
//! This is the N_TTY line discipline, the default for Unix-like systems.

// Allow dead code for now - this is infrastructure for Phase 3+ TTY driver
#![allow(dead_code)]

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;

use super::termios::{Termios, ECHO, ECHOE, ECHOK, ICRNL, ISIG, VWERASE, ECHONL, IGNCR, INLCR, ISTRIP, IEXTEN, NOFLSH, VEOL, VEOL2, IXON, IXANY, VSTART, VSTOP};

// Signal numbers from kernel/src/signal/constants.rs
const SIGINT: u32 = 2;
const SIGQUIT: u32 = 3;
const SIGTSTP: u32 = 20;

/// Maximum line buffer size for canonical mode
pub const MAX_CANON: usize = 4096;
pub const MAX_INPUT: usize = MAX_CANON - 1;

/// Special marker returned when EOF is detected on empty line
pub const EOF_MARKER: i32 = -1;

/// Line discipline for TTY devices
///
/// Processes input characters according to termios settings, handling:
/// - Line editing in canonical mode (backspace, kill line, etc.)
/// - Signal generation (Ctrl+C, Ctrl+\, Ctrl+Z)
/// - Echo (including control character representation as ^X)
/// - Raw mode passthrough
pub struct LineDiscipline {
    /// Terminal attributes controlling behavior
    termios: Termios,

    /// Line buffer for canonical mode editing
    /// Characters accumulate here until newline or EOF
    line_buffer: Vec<u8>,

    /// Completed lines ready for reading (canonical mode)
    cooked_queue: VecDeque<u8>,

    /// Remaining bytes in each completed line; zero is an out-of-band EOF.
    line_records: VecDeque<usize>,
    /// EOF records consume input capacity without carrying a byte.
    eof_records: usize,

    /// Queue for raw mode (characters available immediately)
    raw_queue: VecDeque<u8>,

    /// Input/settings mutation sequence for lock-free waitqueue rechecks.
    input_epoch: Arc<AtomicU64>,

    /// Current column position for echo handling
    column: usize,

    /// Output flow controlled by received START/STOP characters.
    flow_stopped: bool,
}

impl LineDiscipline {
    /// Create a new line discipline with default termios settings
    pub fn new() -> Self {
        Self::try_new().expect("boot console line discipline allocation")
    }

    /// Reserve every input queue before the discipline can reach an IRQ.
    /// User-created terminals propagate allocation failure as ENOMEM.
    pub fn try_new() -> Result<Self, i32> {
        let mut line_buffer = Vec::new();
        let mut cooked_queue = VecDeque::new();
        let mut line_records = VecDeque::new();
        let mut raw_queue = VecDeque::new();
        line_buffer.try_reserve_exact(MAX_CANON).map_err(|_| 12)?;
        cooked_queue.try_reserve_exact(MAX_CANON).map_err(|_| 12)?;
        line_records.try_reserve_exact(MAX_CANON).map_err(|_| 12)?;
        raw_queue.try_reserve_exact(MAX_CANON).map_err(|_| 12)?;
        Ok(Self {
            termios: Termios::default(), line_buffer, cooked_queue,
            line_records, raw_queue, eof_records: 0,
            input_epoch: Arc::try_new(AtomicU64::new(0)).map_err(|_| 12)?,
            column: 0, flow_stopped: false,
        })
    }

    pub fn with_termios(termios: Termios) -> Self {
        let mut discipline = Self::new();
        discipline.set_termios(termios);
        discipline
    }

    fn input_used(&self) -> usize {
        self.line_buffer.len() + self.cooked_queue.len() + self.raw_queue.len()
            + self.eof_records
    }

    /// Canonical overflow still accepts editing and a delimiter when no
    /// completed record can be read. Raw input is backpressured at 4095.
    pub fn input_room(&self) -> bool {
        self.input_used() < MAX_INPUT
            || (self.termios.is_canonical() && self.line_records.is_empty())
    }

    /// Flow and editing controls can make progress without storing a byte,
    /// even at capacity; otherwise a full input queue could prevent START.
    pub fn can_receive(&self, c: u8) -> bool {
        if self.input_room() { return true; }
        let mut c = if self.termios.c_iflag & ISTRIP != 0 { c & 0x7f } else { c };
        if c == b'\r' {
            if self.termios.c_iflag & IGNCR != 0 { return true; }
            if self.termios.c_iflag & ICRNL != 0 { c = b'\n'; }
        } else if c == b'\n' && self.termios.c_iflag & INLCR != 0 { c = b'\r'; }
        if c != 0 {
            if self.termios.c_iflag & IXON != 0
                && (c == self.termios.c_cc[VSTART] || c == self.termios.c_cc[VSTOP]) { return true; }
            if self.termios.c_lflag & ISIG != 0
                && [self.termios.intr_char(), self.termios.quit_char(), self.termios.susp_char()].contains(&c) { return true; }
            if self.termios.is_canonical()
                && (c == self.termios.erase_char() || c == self.termios.kill_char()
                    || (self.termios.c_lflag & IEXTEN != 0 && c == self.termios.c_cc[VWERASE])
                    || (c == self.termios.eof_char() && !self.line_buffer.is_empty())) { return true; }
        }
        self.termios.is_canonical() && self.input_used() < MAX_CANON
            && (c == b'\n' || (c != 0 && (c == self.termios.c_cc[VEOL]
                || (self.termios.c_lflag & IEXTEN != 0 && c == self.termios.c_cc[VEOL2]))))
    }

    pub fn flow_stopped(&self) -> bool { self.flow_stopped }

    /// Capture this discipline's sequence while holding its object lock. A
    /// waiter compares it after registration without taking that lock again.
    pub(super) fn input_epoch(&self) -> Arc<AtomicU64> {
        self.input_epoch.clone()
    }

    /// Get a reference to the current termios settings
    pub fn termios(&self) -> &Termios {
        &self.termios
    }

    /// Get a mutable reference to the termios settings
    pub fn termios_mut(&mut self) -> &mut Termios {
        self.input_epoch.fetch_add(1, Ordering::Release);
        &mut self.termios
    }

    /// Set new termios settings
    pub fn set_termios(&mut self, termios: Termios) {
        self.input_epoch.fetch_add(1, Ordering::Release);
        if self.termios.is_canonical() && !termios.is_canonical() {
            self.raw_queue.append(&mut self.cooked_queue);
            self.raw_queue.extend(self.line_buffer.drain(..));
            self.line_records.clear();
            self.eof_records = 0;
        } else if !self.termios.is_canonical() && termios.is_canonical() {
            // Linux makes previously received raw input readable as one record,
            // without requiring an additional newline or adding an EOF byte.
            let pending = self.raw_queue.len();
            self.cooked_queue.append(&mut self.raw_queue);
            if pending > 0 { self.line_records.push_back(pending); }
        }
        if termios.c_iflag & IXON == 0 { self.flow_stopped = false; }
        self.termios = termios;
    }

    /// Process an input character
    ///
    /// This is the main entry point for input processing. The character is
    /// processed according to the current termios settings.
    ///
    /// # Arguments
    /// * `c` - The input character to process
    /// * `echo_fn` - Callback function for echoing characters back to terminal
    ///
    /// # Returns
    /// * `Some(signal)` - If the character generated a signal (SIGINT, SIGQUIT, SIGTSTP)
    /// * `None` - If no signal was generated
    pub fn input_char(&mut self, c: u8, echo_fn: &mut dyn FnMut(u8)) -> Option<u32> {
        self.input_epoch.fetch_add(1, Ordering::Release);
        let mut c = if self.termios.c_iflag & ISTRIP != 0 { c & 0x7f } else { c };
        if c == b'\r' {
            if self.termios.c_iflag & IGNCR != 0 { return None; }
            if self.termios.c_iflag & ICRNL != 0 { c = b'\n'; }
        } else if c == b'\n' && self.termios.c_iflag & INLCR != 0 {
            c = b'\r';
        }

        if self.termios.c_iflag & IXON != 0 {
            if c != 0 && c == self.termios.c_cc[VSTOP] {
                self.flow_stopped = true;
                return None;
            }
            if c != 0 && c == self.termios.c_cc[VSTART] {
                self.flow_stopped = false;
                return None;
            }
            if self.termios.c_iflag & IXANY != 0 { self.flow_stopped = false; }
        }

        // Check for signal characters first (if ISIG enabled)
        if (self.termios.c_lflag & ISIG) != 0 {
            if c != 0 && c == self.termios.intr_char() {
                // Ctrl+C -> SIGINT
                self.echo_control_char(c, echo_fn);
                if self.termios.c_lflag & ECHO != 0 { echo_fn(b'\n'); }
                self.column = 0;
                if self.termios.c_lflag & NOFLSH == 0 { self.flush_input(); }
                return Some(SIGINT);
            } else if c != 0 && c == self.termios.quit_char() {
                // Ctrl+\ -> SIGQUIT
                self.echo_control_char(c, echo_fn);
                if self.termios.c_lflag & ECHO != 0 { echo_fn(b'\n'); }
                self.column = 0;
                if self.termios.c_lflag & NOFLSH == 0 { self.flush_input(); }
                return Some(SIGQUIT);
            } else if c != 0 && c == self.termios.susp_char() {
                // Ctrl+Z -> SIGTSTP
                self.echo_control_char(c, echo_fn);
                if self.termios.c_lflag & ECHO != 0 { echo_fn(b'\n'); }
                self.column = 0;
                if self.termios.c_lflag & NOFLSH == 0 { self.flush_input(); }
                return Some(SIGTSTP);
            }
        }

        // Process character based on canonical/raw mode
        if self.termios.is_canonical() {
            self.process_canonical(c, echo_fn);
        } else {
            self.process_raw(c, echo_fn);
        }

        None
    }

    /// Process character in canonical (cooked) mode
    ///
    /// In canonical mode:
    /// - Input is line-buffered
    /// - Line editing characters (ERASE, KILL, WERASE) are interpreted
    /// - Lines are completed on newline or EOF
    fn process_canonical(&mut self, c: u8, echo_fn: &mut dyn FnMut(u8)) {
        // A disabled (NUL) control character is ordinary input.
        if c != 0 && c == self.termios.erase_char() {
            self.handle_erase(echo_fn);
            return;
        }

        // Handle KILL (erase entire line)
        if c != 0 && c == self.termios.kill_char() {
            self.handle_kill(echo_fn);
            return;
        }

        // Handle WERASE (word erase, Ctrl+W)
        if c != 0 && self.termios.c_lflag & IEXTEN != 0 && c == self.termios.c_cc[VWERASE] {
            self.handle_word_erase(echo_fn);
            return;
        }

        // Handle EOF (Ctrl+D)
        if c != 0 && c == self.termios.eof_char() {
            self.handle_eof();
            return;
        }

        // Handle newline - complete the line
        if c == b'\n' || (c != 0 && (c == self.termios.c_cc[VEOL]
            || (self.termios.c_lflag & IEXTEN != 0 && c == self.termios.c_cc[VEOL2]))) {
            // Add delimiter to buffer and move to cooked queue
            if self.input_used() < MAX_CANON && self.line_records.len() < MAX_CANON {
                self.line_buffer.push(c);
                self.complete_line();
            }

            // Echo newline
            if self.termios.c_lflag & ECHO != 0
                || (c == b'\n' && self.termios.c_lflag & ECHONL != 0) {
                echo_fn(c);
            }
            self.column = 0;
            return;
        }

        // Regular character - add to line buffer
        if self.input_used() < MAX_INPUT {
            self.line_buffer.push(c);

            // Echo if enabled
            if (self.termios.c_lflag & ECHO) != 0 {
                if c < 0x20 && c != b'\t' {
                    // Control character - echo as ^X
                    self.echo_control_char(c, echo_fn);
                } else {
                    echo_fn(c);
                    if c == b'\t' {
                        // Tab advances to next 8-column boundary
                        self.column = (self.column + 8) & !7;
                    } else {
                        self.column += 1;
                    }
                }
            }
        } else {
            // Rejected canonical input must be distinguishable from a stall,
            // including when normal echo is disabled.
            echo_fn(0x07);
        }
    }

    /// Process character in raw (non-canonical) mode
    ///
    /// In raw mode, characters are passed through immediately without
    /// any special processing.
    fn process_raw(&mut self, c: u8, echo_fn: &mut dyn FnMut(u8)) {
        // Add to raw queue immediately
        if self.input_used() >= MAX_INPUT { return; }
        self.raw_queue.push_back(c);

        // Echo if enabled
        if (self.termios.c_lflag & ECHO) != 0 {
            if c < 0x20 && c != b'\n' && c != b'\r' && c != b'\t' {
                // Control character - echo as ^X
                self.echo_control_char(c, echo_fn);
            } else {
                echo_fn(c);
                if c == b'\n' || c == b'\r' {
                    self.column = 0;
                } else if c == b'\t' {
                    self.column = (self.column + 8) & !7;
                } else {
                    self.column += 1;
                }
            }
        }
    }

    /// Handle ERASE character (backspace/DEL)
    fn handle_erase(&mut self, echo_fn: &mut dyn FnMut(u8)) {
        if let Some(deleted) = self.line_buffer.pop() {
            // Echo backspace-space-backspace if ECHOE is set
            if self.termios.c_lflag & (ECHO | ECHOE) == (ECHO | ECHOE) {
                if deleted < 0x20 {
                    // Was a control character displayed as ^X, need to erase 2 chars
                    echo_fn(0x08); // BS
                    echo_fn(b' ');
                    echo_fn(0x08); // BS
                    echo_fn(0x08); // BS
                    echo_fn(b' ');
                    echo_fn(0x08); // BS
                    self.column = self.column.saturating_sub(2);
                } else if deleted == b'\t' {
                    // Tab is tricky - we don't know exact width, just do one BS
                    echo_fn(0x08);
                    echo_fn(b' ');
                    echo_fn(0x08);
                    self.column = self.column.saturating_sub(1);
                } else {
                    echo_fn(0x08); // BS
                    echo_fn(b' ');
                    echo_fn(0x08); // BS
                    self.column = self.column.saturating_sub(1);
                }
            }
        }
    }

    /// Handle KILL character (Ctrl+U) - erase entire line
    fn handle_kill(&mut self, echo_fn: &mut dyn FnMut(u8)) {
        if self.line_buffer.is_empty() {
            return;
        }

        // Echo kill if ECHOK or ECHOE is set
        if self.termios.c_lflag & (ECHO | ECHOK) == (ECHO | ECHOK) {
            // Echo newline to move to new line
            echo_fn(b'\n');
            self.column = 0;
        } else if self.termios.c_lflag & (ECHO | ECHOE) == (ECHO | ECHOE) {
            // Erase each character visually
            while !self.line_buffer.is_empty() {
                self.handle_erase(echo_fn);
            }
        }

        self.line_buffer.clear();
    }

    /// Handle WERASE character (Ctrl+W) - erase last word
    fn handle_word_erase(&mut self, echo_fn: &mut dyn FnMut(u8)) {
        // Skip trailing whitespace
        while self
            .line_buffer
            .last()
            .map_or(false, |&c| c == b' ' || c == b'\t')
        {
            self.handle_erase(echo_fn);
        }

        // Delete non-whitespace characters until whitespace or start
        while self
            .line_buffer
            .last()
            .map_or(false, |&c| c != b' ' && c != b'\t')
        {
            self.handle_erase(echo_fn);
        }
    }

    /// Handle EOF character (Ctrl+D)
    fn handle_eof(&mut self) {
        if self.line_buffer.is_empty() {
            // Empty line with EOF - signal end of input
            // Keep EOF distinct from every possible input byte.
            if self.input_used() < MAX_INPUT && self.line_records.len() < MAX_CANON {
                self.line_records.push_back(0);
                self.eof_records += 1;
            }
        } else {
            // Non-empty line - complete it without adding newline
            self.complete_line();
        }
    }

    /// Move the line buffer contents to the cooked queue
    fn complete_line(&mut self) {
        self.line_records.push_back(self.line_buffer.len());
        for byte in self.line_buffer.drain(..) {
            self.cooked_queue.push_back(byte);
        }
    }

    /// Echo a control character as ^X
    fn echo_control_char(&mut self, c: u8, echo_fn: &mut dyn FnMut(u8)) {
        if (self.termios.c_lflag & ECHO) != 0 {
            echo_fn(b'^');
            echo_fn(c + 0x40); // Convert control char to letter (^C, ^D, etc.)
            self.column += 2;
        }
    }

    /// Read data from the line discipline
    ///
    /// In canonical mode, reads from the cooked queue (completed lines).
    /// In raw mode, reads from the raw queue.
    ///
    /// # Arguments
    /// * `buf` - Buffer to read data into
    ///
    /// # Returns
    /// * `Ok(n)` - Number of bytes read
    /// * `Err(-1)` - EOF (Ctrl+D on empty line)
    /// * `Err(errno)` - Error code
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        self.input_epoch.fetch_add(1, Ordering::Release);
        if buf.is_empty() {
            return Ok(0);
        }

        if self.termios.is_canonical() {
            let Some(remaining) = self.line_records.front_mut() else { return Ok(0); };
            if *remaining == 0 {
                self.line_records.pop_front();
                self.eof_records -= 1;
                return Err(EOF_MARKER);
            }
            let count = buf.len().min(*remaining);
            for byte in &mut buf[..count] {
                *byte = self.cooked_queue.pop_front().expect("canonical record length");
            }
            *remaining -= count;
            if *remaining == 0 { self.line_records.pop_front(); }
            Ok(count)
        } else {
            let count = buf.len().min(self.raw_queue.len());
            for byte in &mut buf[..count] {
                *byte = self.raw_queue.pop_front().expect("raw queue length");
            }
            Ok(count)
        }
    }

    /// Read up to `max` bytes, at most the rest of one canonical record, to
    /// the user address `buf_ptr` while the input lock is held, with Linux's
    /// handling of a bad address: the bytes taken, and the read's result
    /// (`tty::read_to_user`). An end-of-file record takes no bytes.
    pub fn read_user(&mut self, buf_ptr: u64, max: usize) -> (usize, Result<usize, i32>) {
        let n = if self.termios.is_canonical() {
            match self.line_records.front() {
                Some(&0) => { self.line_records.pop_front(); self.eof_records -= 1; return (0, Err(EOF_MARKER)); }
                Some(&n) => n.min(max),
                None => 0,
            }
        } else { self.raw_queue.len().min(max) };
        super::read_to_user(buf_ptr, n, |chunk| self.read(chunk).unwrap_or(0))
    }

    /// Check if there is data available for reading
    pub fn has_data(&self) -> bool {
        if self.termios.is_canonical() {
            !self.line_records.is_empty()
        } else {
            !self.raw_queue.is_empty()
        }
    }

    /// Flush all input queues
    pub fn flush_input(&mut self) {
        self.input_epoch.fetch_add(1, Ordering::Release);
        self.line_buffer.clear();
        self.cooked_queue.clear();
        self.line_records.clear();
        self.eof_records = 0;
        self.raw_queue.clear();
        self.column = 0;
    }

    /// Bytes held in the canonical edit buffer, before the line is complete.
    ///
    /// Boot-test only: the #821 oracle needs a reading that an injected byte
    /// reached the line discipline, and `bytes_available()` reports the cooked
    /// queue, which stays empty in canonical mode until a newline arrives.
    #[cfg(any(feature = "boot_tests", feature = "btrt"))]
    pub fn pending_line_bytes(&self) -> usize {
        self.line_buffer.len()
    }

    /// Get the number of bytes available for reading
    pub fn bytes_available(&self) -> usize {
        if self.termios.is_canonical() {
            self.cooked_queue.len()
        } else {
            self.raw_queue.len()
        }
    }

    /// Get current column position
    pub fn column(&self) -> usize {
        self.column
    }
}

impl Default for LineDiscipline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn null_echo(_c: u8) {}

    #[test]
    fn test_canonical_mode_basic() {
        let mut ld = LineDiscipline::new();

        // Type "hello\n"
        for c in b"hello\n" {
            ld.input_char(*c, &mut null_echo);
        }

        assert!(ld.has_data());

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 6);
        assert_eq!(&buf[..n], b"hello\n");
    }

    #[test]
    fn test_signal_generation() {
        let mut ld = LineDiscipline::new();

        // Ctrl+C should generate SIGINT
        let sig = ld.input_char(0x03, &mut null_echo);
        assert_eq!(sig, Some(SIGINT));

        // Ctrl+\ should generate SIGQUIT
        let sig = ld.input_char(0x1C, &mut null_echo);
        assert_eq!(sig, Some(SIGQUIT));

        // Ctrl+Z should generate SIGTSTP
        let sig = ld.input_char(0x1A, &mut null_echo);
        assert_eq!(sig, Some(SIGTSTP));
    }

    #[test]
    fn test_signal_disabled() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().c_lflag &= !ISIG;

        // With ISIG disabled, Ctrl+C should not generate signal
        let sig = ld.input_char(0x03, &mut null_echo);
        assert_eq!(sig, None);
    }

    #[test]
    fn test_backspace() {
        let mut ld = LineDiscipline::new();

        // Type "helX", erase twice, then "llo\n"
        for c in b"helX" {
            ld.input_char(*c, &mut null_echo);
        }
        ld.input_char(0x7F, &mut null_echo); // DEL (erase)
        ld.input_char(0x7F, &mut null_echo); // DEL (erase)
        for c in b"llo\n" {
            ld.input_char(*c, &mut null_echo);
        }

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello\n");
    }

    #[test]
    fn test_kill_line() {
        let mut ld = LineDiscipline::new();

        // Type "wrong", Ctrl+U, "right\n"
        for c in b"wrong" {
            ld.input_char(*c, &mut null_echo);
        }
        ld.input_char(0x15, &mut null_echo); // Ctrl+U (kill)
        for c in b"right\n" {
            ld.input_char(*c, &mut null_echo);
        }

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"right\n");
    }

    #[test]
    fn test_raw_mode() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().set_raw();

        // In raw mode, each character is immediately available
        ld.input_char(b'a', &mut null_echo);
        assert!(ld.has_data());

        let mut buf = [0u8; 1];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 1);
        assert_eq!(buf[0], b'a');
    }

    #[test]
    fn test_cr_to_nl_mapping() {
        let mut ld = LineDiscipline::new();

        // Type "hello\r" - CR should be mapped to NL
        for c in b"hello\r" {
            ld.input_char(*c, &mut null_echo);
        }

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 6);
        assert_eq!(&buf[..n], b"hello\n");
    }

    #[test]
    fn test_eof_on_empty_line() {
        let mut ld = LineDiscipline::new();

        // Ctrl+D on empty line should return EOF
        ld.input_char(0x04, &mut null_echo);

        let mut buf = [0u8; 32];
        let result = ld.read(&mut buf);
        assert_eq!(result, Err(EOF_MARKER));
    }

    #[test]
    fn test_eof_completes_line() {
        let mut ld = LineDiscipline::new();

        // Type "hello" then Ctrl+D - should complete line without newline
        for c in b"hello" {
            ld.input_char(*c, &mut null_echo);
        }
        ld.input_char(0x04, &mut null_echo); // Ctrl+D

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf[..n], b"hello");
    }

    #[test]
    fn test_word_erase() {
        let mut ld = LineDiscipline::new();

        // Type "hello world", Ctrl+W, "\n"
        for c in b"hello world" {
            ld.input_char(*c, &mut null_echo);
        }
        ld.input_char(0x17, &mut null_echo); // Ctrl+W (word erase)
        ld.input_char(b'\n', &mut null_echo);

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello \n");
    }

    #[test]
    fn test_echo() {
        let mut ld = LineDiscipline::new();
        let mut echoed = Vec::new();

        // Type 'a' with echo enabled
        ld.input_char(b'a', &mut |c| echoed.push(c));
        assert_eq!(echoed, vec![b'a']);
    }

    #[test]
    fn test_control_char_echo() {
        let mut ld = LineDiscipline::new();
        let mut echoed = Vec::new();

        // Ctrl+C should echo as ^C (then newline due to signal)
        ld.input_char(0x03, &mut |c| echoed.push(c));
        assert_eq!(echoed, vec![b'^', b'C', b'\n']);
    }

    #[test]
    fn test_flush_input() {
        let mut ld = LineDiscipline::new();

        // Add some data
        for c in b"hello" {
            ld.input_char(*c, &mut null_echo);
        }

        ld.flush_input();

        assert!(!ld.has_data());
        assert_eq!(ld.bytes_available(), 0);
    }

    #[test]
    fn test_canonical_mode_buffers_until_newline() {
        let mut ld = LineDiscipline::new();

        // Type characters without newline - should not be available yet
        for c in b"hello" {
            ld.input_char(*c, &mut null_echo);
        }

        // Data should NOT be available until newline
        assert!(!ld.has_data());
        assert_eq!(ld.bytes_available(), 0);

        // Now add newline
        ld.input_char(b'\n', &mut null_echo);

        // Now data should be available
        assert!(ld.has_data());
        assert_eq!(ld.bytes_available(), 6); // "hello\n"
    }

    #[test]
    fn test_backspace_removes_last_char() {
        let mut ld = LineDiscipline::new();

        // Type "abc"
        for c in b"abc" {
            ld.input_char(*c, &mut null_echo);
        }

        // Delete 'c'
        ld.input_char(0x7F, &mut null_echo);

        // Type 'x' and newline
        ld.input_char(b'x', &mut null_echo);
        ld.input_char(b'\n', &mut null_echo);

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"abx\n");
    }

    #[test]
    fn test_backspace_on_empty_buffer() {
        let mut ld = LineDiscipline::new();

        // Backspace on empty buffer should do nothing
        ld.input_char(0x7F, &mut null_echo);
        ld.input_char(0x7F, &mut null_echo);

        // Type "hello\n"
        for c in b"hello\n" {
            ld.input_char(*c, &mut null_echo);
        }

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello\n");
    }

    #[test]
    fn test_kill_line_clears_entire_line() {
        let mut ld = LineDiscipline::new();

        // Type "this is a long line"
        for c in b"this is a long line" {
            ld.input_char(*c, &mut null_echo);
        }

        // Kill entire line with Ctrl+U
        ld.input_char(0x15, &mut null_echo);

        // Type "new\n"
        for c in b"new\n" {
            ld.input_char(*c, &mut null_echo);
        }

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"new\n");
    }

    #[test]
    fn test_kill_line_on_empty_buffer() {
        let mut ld = LineDiscipline::new();

        // Ctrl+U on empty buffer should do nothing
        ld.input_char(0x15, &mut null_echo);

        // Type "hello\n"
        for c in b"hello\n" {
            ld.input_char(*c, &mut null_echo);
        }

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello\n");
    }

    #[test]
    fn test_word_erase_removes_last_word() {
        let mut ld = LineDiscipline::new();

        // Type "one two three"
        for c in b"one two three" {
            ld.input_char(*c, &mut null_echo);
        }

        // Ctrl+W erases "three"
        ld.input_char(0x17, &mut null_echo);

        // Complete the line
        ld.input_char(b'\n', &mut null_echo);

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"one two \n");
    }

    #[test]
    fn test_word_erase_skips_trailing_whitespace() {
        let mut ld = LineDiscipline::new();

        // Type "hello   " (with trailing spaces)
        for c in b"hello   " {
            ld.input_char(*c, &mut null_echo);
        }

        // Ctrl+W should erase the spaces AND "hello"
        ld.input_char(0x17, &mut null_echo);

        // Complete with newline
        ld.input_char(b'\n', &mut null_echo);

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        // Should just be newline after erasing "hello   "
        assert_eq!(&buf[..n], b"\n");
    }

    #[test]
    fn test_raw_mode_no_line_editing() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().set_raw();

        // Type "abc" then backspace
        for c in b"abc" {
            ld.input_char(*c, &mut null_echo);
        }
        ld.input_char(0x7F, &mut null_echo); // This should NOT erase - just add DEL to queue

        let mut buf = [0u8; 8];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 4);
        // In raw mode: a, b, c, DEL (0x7F)
        assert_eq!(buf[0], b'a');
        assert_eq!(buf[1], b'b');
        assert_eq!(buf[2], b'c');
        assert_eq!(buf[3], 0x7F);
    }

    #[test]
    fn test_raw_mode_no_signals() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().set_raw();

        // Ctrl+C in raw mode should NOT generate signal (ISIG is cleared)
        let sig = ld.input_char(0x03, &mut null_echo);
        assert_eq!(sig, None);

        // The Ctrl+C should be in the raw queue as a regular character
        assert!(ld.has_data());
        let mut buf = [0u8; 1];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 1);
        assert_eq!(buf[0], 0x03);
    }

    #[test]
    fn test_signal_ctrl_c_is_sigint() {
        let mut ld = LineDiscipline::new();

        let sig = ld.input_char(0x03, &mut null_echo);
        assert_eq!(sig, Some(SIGINT));
    }

    #[test]
    fn test_signal_ctrl_backslash_is_sigquit() {
        let mut ld = LineDiscipline::new();

        let sig = ld.input_char(0x1C, &mut null_echo);
        assert_eq!(sig, Some(SIGQUIT));
    }

    #[test]
    fn test_signal_ctrl_z_is_sigtstp() {
        let mut ld = LineDiscipline::new();

        let sig = ld.input_char(0x1A, &mut null_echo);
        assert_eq!(sig, Some(SIGTSTP));
    }

    #[test]
    fn test_cr_to_nl_mapping_with_icrnl_enabled() {
        let mut ld = LineDiscipline::new();
        // ICRNL is enabled by default

        // Type "hi" followed by CR
        for c in b"hi\r" {
            ld.input_char(*c, &mut null_echo);
        }

        let mut buf = [0u8; 8];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        // CR should have been mapped to NL
        assert_eq!(&buf[..n], b"hi\n");
    }

    #[test]
    fn test_cr_not_mapped_when_icrnl_disabled() {
        let mut ld = LineDiscipline::new();
        // Disable ICRNL
        ld.termios_mut().c_iflag &= !ICRNL;

        // We need to complete the line somehow. In this test, CR won't be mapped to NL,
        // so we need to send an actual NL to complete the line.
        for c in b"hi\r" {
            ld.input_char(*c, &mut null_echo);
        }
        // Line buffer now has "hi\r" but no newline, so not complete yet
        assert!(!ld.has_data());

        // Add actual newline to complete
        ld.input_char(b'\n', &mut null_echo);

        let mut buf = [0u8; 8];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 4);
        // CR should NOT have been mapped
        assert_eq!(&buf[..n], b"hi\r\n");
    }

    #[test]
    fn test_echo_regular_char() {
        let mut ld = LineDiscipline::new();
        let mut echoed = Vec::new();

        ld.input_char(b'x', &mut |c| echoed.push(c));
        assert_eq!(echoed, vec![b'x']);
    }

    #[test]
    fn test_echo_newline() {
        let mut ld = LineDiscipline::new();
        let mut echoed = Vec::new();

        // Type "hi\n"
        for c in b"hi\n" {
            ld.input_char(*c, &mut |c| echoed.push(c));
        }

        // Should echo 'h', 'i', '\n'
        assert_eq!(echoed, vec![b'h', b'i', b'\n']);
    }

    #[test]
    fn test_echo_control_char_as_caret() {
        let mut ld = LineDiscipline::new();
        let mut echoed = Vec::new();

        // Ctrl+A (0x01) should echo as ^A
        // But since ISIG is enabled and Ctrl+C generates SIGINT, let's use Ctrl+A
        // Actually, only INTR (Ctrl+C), QUIT (Ctrl+\), SUSP (Ctrl+Z) are signal chars
        // Ctrl+A (0x01) is not a signal char, so it goes to the line buffer
        ld.input_char(0x01, &mut |c| echoed.push(c));

        // Should echo as ^A
        assert_eq!(echoed, vec![b'^', b'A']);
    }

    #[test]
    fn test_no_echo_when_echo_disabled() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().c_lflag &= !ECHO;
        let mut echoed = Vec::new();

        // Type "hello\n" with echo disabled
        for c in b"hello\n" {
            ld.input_char(*c, &mut |c| echoed.push(c));
        }

        // Nothing should be echoed
        assert!(echoed.is_empty());
    }

    #[test]
    fn test_multiple_lines() {
        let mut ld = LineDiscipline::new();

        // Type two complete lines
        for c in b"line1\nline2\n" {
            ld.input_char(*c, &mut null_echo);
        }

        // Read first line
        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"line1\n");

        // Read second line
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"line2\n");
    }

    #[test]
    fn test_read_empty_buffer() {
        let mut ld = LineDiscipline::new();

        let mut buf = [0u8; 32];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_read_with_zero_length_buffer() {
        let mut ld = LineDiscipline::new();

        // Add some data
        for c in b"hello\n" {
            ld.input_char(*c, &mut null_echo);
        }

        // Read with empty buffer
        let mut buf = [];
        let n = ld.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_termios_accessors() {
        let mut ld = LineDiscipline::new();

        // Get termios reference
        let termios = ld.termios();
        assert!(termios.is_canonical());

        // Get mutable termios and modify
        ld.termios_mut().set_raw();
        assert!(!ld.termios().is_canonical());

        // Set new termios
        let new_termios = super::super::termios::Termios::default();
        ld.set_termios(new_termios);
        assert!(ld.termios().is_canonical());
    }

    #[test]
    fn test_column_tracking() {
        let mut ld = LineDiscipline::new();

        // Start at column 0
        assert_eq!(ld.column(), 0);

        // Type characters
        for c in b"hello" {
            ld.input_char(*c, &mut null_echo);
        }

        // Column should be 5
        assert_eq!(ld.column(), 5);
    }

    #[test]
    fn test_with_termios_constructor() {
        let mut custom_termios = super::super::termios::Termios::default();
        custom_termios.set_raw();

        let ld = LineDiscipline::with_termios(custom_termios);

        // Should have raw mode settings
        assert!(!ld.termios().is_canonical());
        assert!(!ld.termios().is_echo());
    }

    #[test]
    fn test_default_line_discipline() {
        let ld = LineDiscipline::default();

        // Should be equivalent to LineDiscipline::new()
        assert!(ld.termios().is_canonical());
        assert!(ld.termios().is_echo());
        assert!(!ld.has_data());
    }

    #[test]
    fn test_flush_clears_all_queues() {
        let mut ld = LineDiscipline::new();

        // Add data to line buffer (incomplete line)
        for c in b"incomplete" {
            ld.input_char(*c, &mut null_echo);
        }

        // Add data to cooked queue (complete line)
        for c in b"complete\n" {
            ld.input_char(*c, &mut null_echo);
        }

        // Flush everything
        ld.flush_input();

        // Nothing should be available
        assert!(!ld.has_data());
        assert_eq!(ld.bytes_available(), 0);
        assert_eq!(ld.column(), 0);
    }
    #[test]
    fn eof_records_preserve_ff_and_partial_lines() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().c_lflag &= !ECHO;
        for c in [0xff, b'x', 4, 4, b'y', b'\n'] { ld.input_char(c, &mut null_echo); }
        let mut buf = [0; 8];
        assert_eq!(ld.read(&mut buf[..1]), Ok(1));
        assert_eq!(buf[0], 0xff);
        assert_eq!(ld.read(&mut buf), Ok(1));
        assert_eq!(buf[0], b'x');
        assert!(ld.has_data());
        assert_eq!(ld.read(&mut buf), Err(EOF_MARKER));
        assert_eq!(ld.read(&mut buf), Ok(2));
        assert_eq!(&buf[..2], b"y\n");
    }

    #[test]
    fn eol2_requires_iexten_and_echonl_only_echoes_newline() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().c_lflag = super::super::termios::ICANON | ECHONL;
        ld.termios_mut().c_cc[VEOL2] = b'!';
        let mut echo = Vec::new();
        for c in b"a!" { ld.input_char(*c, &mut |c| echo.push(c)); }
        assert!(!ld.has_data());
        ld.termios_mut().c_lflag |= IEXTEN;
        ld.input_char(b'!', &mut |c| echo.push(c));
        let mut buf = [0; 8];
        assert_eq!(ld.read(&mut buf), Ok(3));
        assert_eq!(&buf[..3], b"a!!");
        assert!(echo.is_empty());
        ld.input_char(b'\n', &mut |c| echo.push(c));
        assert_eq!(echo, b"\n");
    }

    #[test]
    fn input_transform_order_and_signal_flush() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().c_iflag = ISTRIP | IGNCR | ICRNL | INLCR;
        for c in [0xc1, b'\r', b'\n'] { ld.input_char(c, &mut null_echo); }
        assert!(!ld.has_data());
        ld.termios_mut().c_cc[VEOL] = b'!';
        ld.input_char(b'!', &mut null_echo);
        let mut buf = [0; 8];
        assert_eq!(ld.read(&mut buf), Ok(3));
        assert_eq!(&buf[..3], b"A\r!");
        for c in b"old!pending" { ld.input_char(*c, &mut null_echo); }
        assert_eq!(ld.input_char(3, &mut null_echo), Some(SIGINT));
        assert!(!ld.has_data());
        assert!(ld.line_buffer.is_empty());
        ld.termios_mut().c_lflag |= NOFLSH;
        for c in b"kept!" { ld.input_char(*c, &mut null_echo); }
        assert_eq!(ld.input_char(3, &mut null_echo), Some(SIGINT));
        assert_eq!(ld.read(&mut buf), Ok(5));
        assert_eq!(&buf[..5], b"kept!");
    }

    #[test]
    fn canonical_transitions_preserve_queued_and_unfinished_input() {
        let mut ld = LineDiscipline::new();
        for c in b"one\nunfinished" { ld.input_char(*c, &mut null_echo); }
        let mut attrs = *ld.termios();
        attrs.set_raw();
        ld.set_termios(attrs);
        let mut buf = [0; 32];
        assert_eq!(ld.read(&mut buf), Ok(14));
        assert_eq!(&buf[..14], b"one\nunfinished");
        for c in b"raw\ndata" { ld.input_char(*c, &mut null_echo); }
        attrs.set_canonical(true);
        ld.set_termios(attrs);
        assert!(ld.has_data());
        assert_eq!(ld.read(&mut buf[..2]), Ok(2));
        assert_eq!(&buf[..2], b"ra");
        assert_eq!(ld.read(&mut buf), Ok(6));
        assert_eq!(&buf[..6], b"w\ndata");
        assert!(!ld.has_data());
    }

    #[test]
    fn signal_characters_do_not_echo_when_echo_is_disabled() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().c_lflag &= !ECHO;
        let mut echo = Vec::new();
        for c in [3, 28, 26] {
            assert!(ld.input_char(c, &mut |c| echo.push(c)).is_some());
        }
        assert!(echo.is_empty());
    }

    #[test]
    fn input_change_sequence_covers_settings_input_reads_and_flushes() {
        let mut ld = LineDiscipline::new();
        let epoch = ld.input_epoch();
        let mut previous = epoch.load(Ordering::Acquire);
        ld.set_termios(Termios::default());
        assert_ne!(epoch.load(Ordering::Acquire), previous);
        previous = epoch.load(Ordering::Acquire);
        ld.input_char(4, &mut null_echo);
        assert_ne!(epoch.load(Ordering::Acquire), previous);
        previous = epoch.load(Ordering::Acquire);
        assert_eq!(ld.read(&mut [0; 1]), Err(EOF_MARKER));
        assert_ne!(epoch.load(Ordering::Acquire), previous);
        previous = epoch.load(Ordering::Acquire);
        ld.flush_input();
        assert_ne!(epoch.load(Ordering::Acquire), previous);
    }

    #[test]
    fn input_storage_stays_bounded_and_reserves_the_canonical_delimiter() {
        let mut ld = LineDiscipline::new();
        let capacities = (ld.line_buffer.capacity(), ld.cooked_queue.capacity(), ld.raw_queue.capacity(), ld.line_records.capacity());
        for _ in 0..MAX_CANON * 3 { ld.input_char(b'x', &mut null_echo); }
        assert_eq!(ld.line_buffer.len(), MAX_INPUT);
        ld.input_char(b'\n', &mut null_echo);
        let mut data = vec![0; MAX_CANON];
        assert_eq!(ld.read(&mut data), Ok(MAX_CANON));
        assert_eq!(data[MAX_INPUT], b'\n');
        for _ in 0..MAX_CANON * 3 { ld.input_char(4, &mut null_echo); }
        assert!(ld.line_records.len() <= MAX_INPUT);
        ld.flush_input();
        let mut attrs = *ld.termios();
        attrs.set_raw();
        ld.set_termios(attrs);
        for _ in 0..MAX_CANON * 3 { ld.input_char(b'r', &mut null_echo); }
        assert_eq!(ld.bytes_available(), MAX_INPUT);
        assert!(!ld.input_room());
        attrs.set_canonical(true);
        ld.set_termios(attrs);
        attrs.set_raw();
        ld.set_termios(attrs);
        assert_eq!(capacities, (ld.line_buffer.capacity(), ld.cooked_queue.capacity(), ld.raw_queue.capacity(), ld.line_records.capacity()));
    }

    #[test]
    fn erase_kill_and_word_erase_do_not_echo_without_echo() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().c_lflag &= !ECHO;
        let mut echo = Vec::new();
        for &c in b"abc\x7fdef\x17ghi\x15\n" { ld.input_char(c, &mut |c| echo.push(c)); }
        assert!(echo.is_empty());
        assert_eq!(ld.read(&mut [0; 8]), Ok(1));
    }

    #[test]
    fn xon_xoff_consumes_control_characters_and_ixany_resumes() {
        let mut ld = LineDiscipline::new();
        ld.termios_mut().set_raw();
        ld.termios_mut().c_iflag |= IXON;
        for c in [19, b'x'] { ld.input_char(c, &mut null_echo); }
        assert!(ld.flow_stopped());
        ld.input_char(17, &mut null_echo);
        assert!(!ld.flow_stopped());
        let mut buf = [0; 8];
        assert_eq!(ld.read(&mut buf), Ok(1));
        assert_eq!(buf[0], b'x');
        ld.termios_mut().c_iflag |= IXANY;
        ld.input_char(19, &mut null_echo);
        ld.input_char(b'y', &mut null_echo);
        assert!(!ld.flow_stopped());
        assert_eq!(ld.read(&mut buf), Ok(1));
        assert_eq!(buf[0], b'y');
    }

    #[test]
    fn a_read_to_an_address_outside_user_space_takes_no_input() {
        let mut ld = LineDiscipline::new();
        for &c in b"record\n" { ld.input_char(c, &mut null_echo); }
        let mut scratch = [0; 16];
        assert_eq!(ld.read_user(0, 16), (0, Err(14)));
        assert_eq!(ld.bytes_available(), 7);
        assert_eq!(ld.read(&mut scratch), Ok(7));
        assert_eq!(&scratch[..7], b"record\n");
        ld.termios_mut().set_raw();
        ld.input_char(b'x', &mut null_echo);
        assert_eq!(ld.read_user(0, 16), (0, Err(14)));
        assert_eq!(ld.read(&mut scratch), Ok(1));
        assert_eq!(scratch[0], b'x');
    }

}
