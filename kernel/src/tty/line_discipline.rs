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
use alloc::vec::Vec;

use super::termios::{Termios, ECHO, ECHOE, ECHOK, ICRNL, ISIG, VWERASE};

// Signal numbers from kernel/src/signal/constants.rs
const SIGINT: u32 = 2;
const SIGQUIT: u32 = 3;
const SIGTSTP: u32 = 20;

/// Maximum line buffer size for canonical mode
const MAX_CANON: usize = 4096;

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

    /// Queue for raw mode (characters available immediately)
    raw_queue: VecDeque<u8>,

    /// Current column position for echo handling
    column: usize,
}

impl LineDiscipline {
    /// Create a new line discipline with default termios settings
    pub fn new() -> Self {
        Self {
            termios: Termios::default(),
            line_buffer: Vec::with_capacity(MAX_CANON),
            cooked_queue: VecDeque::new(),
            raw_queue: VecDeque::new(),
            column: 0,
        }
    }

    /// Create a new line discipline with custom termios settings
    pub fn with_termios(termios: Termios) -> Self {
        Self {
            termios,
            line_buffer: Vec::with_capacity(MAX_CANON),
            cooked_queue: VecDeque::new(),
            raw_queue: VecDeque::new(),
            column: 0,
        }
    }

    /// Get a reference to the current termios settings
    pub fn termios(&self) -> &Termios {
        &self.termios
    }

    /// Get a mutable reference to the termios settings
    pub fn termios_mut(&mut self) -> &mut Termios {
        &mut self.termios
    }

    /// Set new termios settings
    pub fn set_termios(&mut self, termios: Termios) {
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
        // Map CR to NL if ICRNL is set
        let c = if (self.termios.c_iflag & ICRNL) != 0 && c == b'\r' {
            b'\n'
        } else {
            c
        };

        // Check for signal characters first (if ISIG enabled)
        if (self.termios.c_lflag & ISIG) != 0 {
            if c == self.termios.intr_char() {
                // Ctrl+C -> SIGINT
                self.echo_control_char(c, echo_fn);
                echo_fn(b'\n');
                self.column = 0;
                return Some(SIGINT);
            } else if c == self.termios.quit_char() {
                // Ctrl+\ -> SIGQUIT
                self.echo_control_char(c, echo_fn);
                echo_fn(b'\n');
                self.column = 0;
                return Some(SIGQUIT);
            } else if c == self.termios.susp_char() {
                // Ctrl+Z -> SIGTSTP
                self.echo_control_char(c, echo_fn);
                echo_fn(b'\n');
                self.column = 0;
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
        // Handle ERASE - accept both configured erase_char (typically DEL 0x7F) AND
        // backspace (0x08). Keyboards typically send 0x08 for the backspace key,
        // but POSIX termios defaults VERASE to DEL. Accepting both ensures backspace
        // works out of the box regardless of keyboard mapping.
        if c == self.termios.erase_char() || c == 0x08 {
            self.handle_erase(echo_fn);
            return;
        }

        // Handle KILL (erase entire line)
        if c == self.termios.kill_char() {
            self.handle_kill(echo_fn);
            return;
        }

        // Handle WERASE (word erase, Ctrl+W)
        if c == self.termios.c_cc[VWERASE] {
            self.handle_word_erase(echo_fn);
            return;
        }

        // Handle EOF (Ctrl+D)
        if c == self.termios.eof_char() {
            self.handle_eof(echo_fn);
            return;
        }

        // Handle newline - complete the line
        if c == b'\n' {
            // Add newline to buffer and move to cooked queue
            if self.line_buffer.len() < MAX_CANON {
                self.line_buffer.push(c);
            }
            self.complete_line();

            // Echo newline
            if (self.termios.c_lflag & ECHO) != 0 {
                echo_fn(b'\n');
            }
            self.column = 0;
            return;
        }

        // Regular character - add to line buffer
        if self.line_buffer.len() < MAX_CANON {
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
        }
    }

    /// Process character in raw (non-canonical) mode
    ///
    /// In raw mode, characters are passed through immediately without
    /// any special processing.
    fn process_raw(&mut self, c: u8, echo_fn: &mut dyn FnMut(u8)) {
        // Add to raw queue immediately
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
            if (self.termios.c_lflag & ECHOE) != 0 {
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
        if (self.termios.c_lflag & ECHOK) != 0 {
            // Echo newline to move to new line
            echo_fn(b'\n');
            self.column = 0;
        } else if (self.termios.c_lflag & ECHOE) != 0 {
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
        while self.line_buffer.last().map_or(false, |&c| c == b' ' || c == b'\t') {
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
    fn handle_eof(&mut self, echo_fn: &mut dyn FnMut(u8)) {
        // Echo ^D if echo enabled
        if (self.termios.c_lflag & ECHO) != 0 {
            self.echo_control_char(0x04, echo_fn);
        }

        if self.line_buffer.is_empty() {
            // Empty line with EOF - signal end of input
            // We add a special marker that read() will interpret as EOF
            self.cooked_queue.push_back(0xFF); // Internal EOF marker
        } else {
            // Non-empty line - complete it without adding newline
            self.complete_line();
        }
    }

    /// Move the line buffer contents to the cooked queue
    fn complete_line(&mut self) {
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
        if buf.is_empty() {
            return Ok(0);
        }

        let queue = if self.termios.is_canonical() {
            &mut self.cooked_queue
        } else {
            &mut self.raw_queue
        };

        if queue.is_empty() {
            return Ok(0);
        }

        // Check for EOF marker in canonical mode
        if self.termios.is_canonical() {
            if let Some(&0xFF) = queue.front() {
                queue.pop_front();
                return Err(EOF_MARKER);
            }
        }

        let mut count = 0;
        for byte in buf.iter_mut() {
            if let Some(c) = queue.pop_front() {
                // Stop at EOF marker
                if self.termios.is_canonical() && c == 0xFF {
                    return Err(EOF_MARKER);
                }
                *byte = c;
                count += 1;

                // In canonical mode, stop at newline
                if self.termios.is_canonical() && c == b'\n' {
                    break;
                }
            } else {
                break;
            }
        }

        Ok(count)
    }

    /// Check if there is data available for reading
    pub fn has_data(&self) -> bool {
        if self.termios.is_canonical() {
            !self.cooked_queue.is_empty()
        } else {
            !self.raw_queue.is_empty()
        }
    }

    /// Flush all input queues
    pub fn flush_input(&mut self) {
        self.line_buffer.clear();
        self.cooked_queue.clear();
        self.raw_queue.clear();
        self.column = 0;
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

        // Type "helo", backspace, "l\n"
        for c in b"helo" {
            ld.input_char(*c, &mut null_echo);
        }
        ld.input_char(0x7F, &mut null_echo); // DEL (erase)
        ld.input_char(0x7F, &mut null_echo); // DEL (erase)
        for c in b"lo\n" {
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
}
