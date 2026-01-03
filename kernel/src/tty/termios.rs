//! POSIX termios structure and constants
//!
//! This module provides the terminal I/O interface structures and constants
//! as defined by POSIX.1-2017. These are used to configure terminal behavior
//! including input processing, output processing, control modes, and local modes.

// Allow dead code for now - this is a public API that will be used by:
// - Phase 2: Line discipline implementation
// - Phase 3: TTY driver implementation
// - Phase 4: ioctl handlers (tcgetattr, tcsetattr, etc.)
// All constants and methods here are part of the POSIX termios specification.
#![allow(dead_code)]

use core::default::Default;

/// Number of control characters in the c_cc array
pub const NCCS: usize = 32;

// =============================================================================
// Input Flags (c_iflag)
// =============================================================================

/// Enable input parity checking
pub const INPCK: u32 = 0o000020;

/// Strip character to 7 bits
pub const ISTRIP: u32 = 0o000040;

/// Map NL to CR on input
pub const INLCR: u32 = 0o000100;

/// Ignore CR on input
pub const IGNCR: u32 = 0o000200;

/// Map CR to NL on input (unless IGNCR is set)
pub const ICRNL: u32 = 0o000400;

/// Enable XON/XOFF flow control on output
pub const IXON: u32 = 0o002000;

/// Any character will restart after stop
pub const IXANY: u32 = 0o004000;

/// Enable XON/XOFF flow control on input
pub const IXOFF: u32 = 0o010000;

// =============================================================================
// Output Flags (c_oflag)
// =============================================================================

/// Enable output processing
pub const OPOST: u32 = 0o000001;

/// Map NL to CR-NL on output
pub const ONLCR: u32 = 0o000004;

// =============================================================================
// Local Flags (c_lflag)
// =============================================================================

/// Enable signals (INTR, QUIT, SUSP)
pub const ISIG: u32 = 0o000001;

/// Canonical mode (line-by-line input)
pub const ICANON: u32 = 0o000002;

/// Enable echo
pub const ECHO: u32 = 0o000010;

/// Echo ERASE as backspace-space-backspace
pub const ECHOE: u32 = 0o000020;

/// Echo KILL by erasing each character on the line
pub const ECHOK: u32 = 0o000040;

/// Echo NL even if ECHO is not set
pub const ECHONL: u32 = 0o000100;

/// Disable flushing after interrupt or quit
pub const NOFLSH: u32 = 0o000200;

/// Send SIGTTOU for background output
pub const TOSTOP: u32 = 0o000400;

/// Enable implementation-defined input processing
pub const IEXTEN: u32 = 0o100000;

// =============================================================================
// Control Character Indices (c_cc)
// =============================================================================

/// Interrupt character (SIGINT) - typically Ctrl+C
pub const VINTR: usize = 0;

/// Quit character (SIGQUIT) - typically Ctrl+\
pub const VQUIT: usize = 1;

/// Erase character - typically DEL or Ctrl+H
pub const VERASE: usize = 2;

/// Kill line character - typically Ctrl+U
pub const VKILL: usize = 3;

/// End of file character - typically Ctrl+D
pub const VEOF: usize = 4;

/// Timeout in deciseconds for non-canonical read
pub const VTIME: usize = 5;

/// Minimum number of characters for non-canonical read
pub const VMIN: usize = 6;

/// Suspend character (SIGTSTP) - typically Ctrl+Z
pub const VSUSP: usize = 10;

/// Start character for XON/XOFF - typically Ctrl+Q
pub const VSTART: usize = 8;

/// Stop character for XON/XOFF - typically Ctrl+S
pub const VSTOP: usize = 9;

/// Literal next character - typically Ctrl+V
pub const VLNEXT: usize = 15;

/// Word erase character - typically Ctrl+W
pub const VWERASE: usize = 14;

// =============================================================================
// Default Control Character Values
// =============================================================================

/// Ctrl+C (ETX)
const CTRL_C: u8 = 0x03;

/// Ctrl+\ (FS)
const CTRL_BACKSLASH: u8 = 0x1C;

/// DEL character
const DEL: u8 = 0x7F;

/// Ctrl+U (NAK)
const CTRL_U: u8 = 0x15;

/// Ctrl+D (EOT)
const CTRL_D: u8 = 0x04;

/// Ctrl+Z (SUB)
const CTRL_Z: u8 = 0x1A;

/// Ctrl+Q (DC1/XON)
const CTRL_Q: u8 = 0x11;

/// Ctrl+S (DC3/XOFF)
const CTRL_S: u8 = 0x13;

/// Ctrl+V (SYN)
const CTRL_V: u8 = 0x16;

/// Ctrl+W (ETB)
const CTRL_W: u8 = 0x17;

// =============================================================================
// Termios Structure
// =============================================================================

/// Terminal I/O settings structure
///
/// This structure contains all the configuration for a terminal device,
/// following the POSIX termios specification.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Termios {
    /// Input mode flags
    pub c_iflag: u32,

    /// Output mode flags
    pub c_oflag: u32,

    /// Control mode flags
    pub c_cflag: u32,

    /// Local mode flags
    pub c_lflag: u32,

    /// Line discipline (typically 0 for N_TTY)
    pub c_line: u8,

    /// Control characters array
    pub c_cc: [u8; NCCS],

    /// Input baud rate
    pub c_ispeed: u32,

    /// Output baud rate
    pub c_ospeed: u32,
}

impl Default for Termios {
    /// Create termios with sane defaults
    ///
    /// Default settings:
    /// - Canonical mode enabled (line-by-line input)
    /// - Echo enabled
    /// - Signal generation enabled
    /// - CR mapped to NL on input
    /// - NL mapped to CR-NL on output
    /// - Standard control characters
    fn default() -> Self {
        let mut c_cc = [0u8; NCCS];

        // Set default control characters
        c_cc[VINTR] = CTRL_C;      // Ctrl+C for SIGINT
        c_cc[VQUIT] = CTRL_BACKSLASH; // Ctrl+\ for SIGQUIT
        c_cc[VERASE] = DEL;        // DEL for erase
        c_cc[VKILL] = CTRL_U;      // Ctrl+U for kill line
        c_cc[VEOF] = CTRL_D;       // Ctrl+D for EOF
        c_cc[VTIME] = 0;           // No timeout
        c_cc[VMIN] = 1;            // Minimum 1 character for read
        c_cc[VSUSP] = CTRL_Z;      // Ctrl+Z for SIGTSTP
        c_cc[VSTART] = CTRL_Q;     // Ctrl+Q for XON
        c_cc[VSTOP] = CTRL_S;      // Ctrl+S for XOFF
        c_cc[VLNEXT] = CTRL_V;     // Ctrl+V for literal next
        c_cc[VWERASE] = CTRL_W;    // Ctrl+W for word erase

        Self {
            // Input: map CR to NL
            c_iflag: ICRNL,

            // Output: enable processing, map NL to CR-NL
            c_oflag: OPOST | ONLCR,

            // Control: no special settings needed for basic operation
            c_cflag: 0,

            // Local: canonical mode, echo, signals, extended processing
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN,

            // Line discipline 0 (N_TTY)
            c_line: 0,

            c_cc,

            // Default baud rates (not really relevant for virtual terminals)
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }
}

impl Termios {
    /// Create a new termios with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if canonical (line) mode is enabled
    ///
    /// In canonical mode, input is processed line-by-line and line editing
    /// characters (ERASE, KILL, etc.) are interpreted.
    #[inline]
    pub fn is_canonical(&self) -> bool {
        (self.c_lflag & ICANON) != 0
    }

    /// Set or clear canonical (line) mode
    ///
    /// When enabled, input is line-buffered and line editing characters
    /// (ERASE, KILL, etc.) are interpreted. When disabled (raw mode),
    /// characters are passed through immediately without buffering.
    #[inline]
    pub fn set_canonical(&mut self, enable: bool) {
        if enable {
            self.c_lflag |= ICANON;
        } else {
            self.c_lflag &= !ICANON;
        }
    }

    /// Check if echo is enabled
    ///
    /// When echo is enabled, input characters are echoed back to the terminal.
    #[inline]
    pub fn is_echo(&self) -> bool {
        (self.c_lflag & ECHO) != 0
    }

    /// Check if signal generation is enabled
    ///
    /// When enabled, control characters like Ctrl+C generate signals
    /// (SIGINT, SIGQUIT, SIGTSTP).
    #[inline]
    pub fn is_sig(&self) -> bool {
        (self.c_lflag & ISIG) != 0
    }

    /// Check if output processing is enabled
    #[inline]
    pub fn is_opost(&self) -> bool {
        (self.c_oflag & OPOST) != 0
    }

    /// Check if NL should be mapped to CR-NL on output
    #[inline]
    pub fn is_onlcr(&self) -> bool {
        (self.c_oflag & ONLCR) != 0
    }

    /// Check if CR should be mapped to NL on input
    #[inline]
    pub fn is_icrnl(&self) -> bool {
        (self.c_iflag & ICRNL) != 0
    }

    /// Get the interrupt character (usually Ctrl+C)
    #[inline]
    pub fn intr_char(&self) -> u8 {
        self.c_cc[VINTR]
    }

    /// Get the quit character (usually Ctrl+\)
    #[inline]
    pub fn quit_char(&self) -> u8 {
        self.c_cc[VQUIT]
    }

    /// Get the suspend character (usually Ctrl+Z)
    #[inline]
    pub fn susp_char(&self) -> u8 {
        self.c_cc[VSUSP]
    }

    /// Get the EOF character (usually Ctrl+D)
    #[inline]
    pub fn eof_char(&self) -> u8 {
        self.c_cc[VEOF]
    }

    /// Get the erase character (usually DEL or Backspace)
    #[inline]
    pub fn erase_char(&self) -> u8 {
        self.c_cc[VERASE]
    }

    /// Get the kill (line erase) character (usually Ctrl+U)
    #[inline]
    pub fn kill_char(&self) -> u8 {
        self.c_cc[VKILL]
    }

    /// Get the VMIN value (minimum characters for non-canonical read)
    #[inline]
    pub fn vmin(&self) -> u8 {
        self.c_cc[VMIN]
    }

    /// Get the VTIME value (timeout in deciseconds for non-canonical read)
    #[inline]
    pub fn vtime(&self) -> u8 {
        self.c_cc[VTIME]
    }

    /// Set raw mode (disable canonical processing, echo, and signals)
    ///
    /// This is commonly used by applications that want to handle all
    /// input processing themselves (e.g., text editors, shells with
    /// line editing).
    pub fn set_raw(&mut self) {
        // Disable canonical mode, echo, and signals
        self.c_lflag &= !(ICANON | ECHO | ECHOE | ECHOK | ECHONL | ISIG | IEXTEN);

        // Disable input processing
        self.c_iflag &= !(INPCK | ISTRIP | INLCR | IGNCR | ICRNL | IXON | IXANY | IXOFF);

        // Set VMIN=1 and VTIME=0 for character-at-a-time input
        self.c_cc[VMIN] = 1;
        self.c_cc[VTIME] = 0;
    }

    /// Reset to cooked (default) mode
    pub fn set_cooked(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_termios() {
        let termios = Termios::default();

        // Check default modes
        assert!(termios.is_canonical());
        assert!(termios.is_echo());
        assert!(termios.is_sig());
        assert!(termios.is_opost());
        assert!(termios.is_onlcr());
        assert!(termios.is_icrnl());

        // Check default control characters
        assert_eq!(termios.intr_char(), CTRL_C);
        assert_eq!(termios.eof_char(), CTRL_D);
        assert_eq!(termios.susp_char(), CTRL_Z);
    }

    #[test]
    fn test_default_termios_all_control_chars() {
        let termios = Termios::default();

        // Check all control character accessors
        assert_eq!(termios.intr_char(), CTRL_C);         // Ctrl+C for SIGINT
        assert_eq!(termios.quit_char(), CTRL_BACKSLASH); // Ctrl+\ for SIGQUIT
        assert_eq!(termios.susp_char(), CTRL_Z);         // Ctrl+Z for SIGTSTP
        assert_eq!(termios.eof_char(), CTRL_D);          // Ctrl+D for EOF
        assert_eq!(termios.erase_char(), DEL);           // DEL for backspace
        assert_eq!(termios.kill_char(), CTRL_U);         // Ctrl+U for kill line
    }

    #[test]
    fn test_default_vmin_vtime() {
        let termios = Termios::default();

        // Default VMIN=1 (minimum 1 char for read), VTIME=0 (no timeout)
        assert_eq!(termios.vmin(), 1);
        assert_eq!(termios.vtime(), 0);
    }

    #[test]
    fn test_default_flags() {
        let termios = Termios::default();

        // Input flags: ICRNL should be set (CR -> NL mapping)
        assert_eq!(termios.c_iflag, ICRNL);

        // Output flags: OPOST and ONLCR should be set
        assert_eq!(termios.c_oflag, OPOST | ONLCR);

        // Local flags: canonical mode, echo, signals, extended
        assert_eq!(
            termios.c_lflag,
            ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN
        );

        // Line discipline should be 0 (N_TTY)
        assert_eq!(termios.c_line, 0);
    }

    #[test]
    fn test_raw_mode() {
        let mut termios = Termios::default();
        termios.set_raw();

        assert!(!termios.is_canonical());
        assert!(!termios.is_echo());
        assert!(!termios.is_sig());
        assert_eq!(termios.vmin(), 1);
        assert_eq!(termios.vtime(), 0);
    }

    #[test]
    fn test_raw_mode_disables_all_processing() {
        let mut termios = Termios::default();
        termios.set_raw();

        // Local flags should have ICANON, ECHO, ECHOE, ECHOK, ECHONL, ISIG, IEXTEN cleared
        assert_eq!(termios.c_lflag & ICANON, 0);
        assert_eq!(termios.c_lflag & ECHO, 0);
        assert_eq!(termios.c_lflag & ECHOE, 0);
        assert_eq!(termios.c_lflag & ECHOK, 0);
        assert_eq!(termios.c_lflag & ECHONL, 0);
        assert_eq!(termios.c_lflag & ISIG, 0);
        assert_eq!(termios.c_lflag & IEXTEN, 0);

        // Input flags should have special processing cleared
        assert_eq!(termios.c_iflag & INPCK, 0);
        assert_eq!(termios.c_iflag & ISTRIP, 0);
        assert_eq!(termios.c_iflag & INLCR, 0);
        assert_eq!(termios.c_iflag & IGNCR, 0);
        assert_eq!(termios.c_iflag & ICRNL, 0);
        assert_eq!(termios.c_iflag & IXON, 0);
        assert_eq!(termios.c_iflag & IXANY, 0);
        assert_eq!(termios.c_iflag & IXOFF, 0);

        // VMIN and VTIME should be set for character-at-a-time input
        assert_eq!(termios.c_cc[VMIN], 1);
        assert_eq!(termios.c_cc[VTIME], 0);
    }

    #[test]
    fn test_cooked_mode_reset() {
        let mut termios = Termios::default();
        termios.set_raw();
        termios.set_cooked();

        assert!(termios.is_canonical());
        assert!(termios.is_echo());
        assert!(termios.is_sig());
    }

    #[test]
    fn test_cooked_mode_restores_defaults() {
        let mut termios = Termios::default();

        // First set raw mode (modifies everything)
        termios.set_raw();

        // Then reset to cooked mode
        termios.set_cooked();

        // Verify it matches a fresh default
        let default = Termios::default();
        assert_eq!(termios.c_iflag, default.c_iflag);
        assert_eq!(termios.c_oflag, default.c_oflag);
        assert_eq!(termios.c_lflag, default.c_lflag);
        assert_eq!(termios.c_cc, default.c_cc);
    }

    #[test]
    fn test_new_equals_default() {
        let from_new = Termios::new();
        let from_default = Termios::default();

        assert_eq!(from_new.c_iflag, from_default.c_iflag);
        assert_eq!(from_new.c_oflag, from_default.c_oflag);
        assert_eq!(from_new.c_cflag, from_default.c_cflag);
        assert_eq!(from_new.c_lflag, from_default.c_lflag);
        assert_eq!(from_new.c_line, from_default.c_line);
        assert_eq!(from_new.c_cc, from_default.c_cc);
    }

    #[test]
    fn test_flag_methods_match_manual_check() {
        let termios = Termios::default();

        // Verify is_* methods match manual flag checks
        assert_eq!(termios.is_canonical(), (termios.c_lflag & ICANON) != 0);
        assert_eq!(termios.is_echo(), (termios.c_lflag & ECHO) != 0);
        assert_eq!(termios.is_sig(), (termios.c_lflag & ISIG) != 0);
        assert_eq!(termios.is_opost(), (termios.c_oflag & OPOST) != 0);
        assert_eq!(termios.is_onlcr(), (termios.c_oflag & ONLCR) != 0);
        assert_eq!(termios.is_icrnl(), (termios.c_iflag & ICRNL) != 0);
    }

    #[test]
    fn test_control_char_indices() {
        let termios = Termios::default();

        // Verify control character accessors use correct indices
        assert_eq!(termios.intr_char(), termios.c_cc[VINTR]);
        assert_eq!(termios.quit_char(), termios.c_cc[VQUIT]);
        assert_eq!(termios.susp_char(), termios.c_cc[VSUSP]);
        assert_eq!(termios.eof_char(), termios.c_cc[VEOF]);
        assert_eq!(termios.erase_char(), termios.c_cc[VERASE]);
        assert_eq!(termios.kill_char(), termios.c_cc[VKILL]);
        assert_eq!(termios.vmin(), termios.c_cc[VMIN]);
        assert_eq!(termios.vtime(), termios.c_cc[VTIME]);
    }

    #[test]
    fn test_set_canonical_enable() {
        let mut termios = Termios::default();

        // Start in canonical mode
        assert!(termios.is_canonical());

        // Disable canonical mode
        termios.set_canonical(false);
        assert!(!termios.is_canonical());
        assert_eq!(termios.c_lflag & ICANON, 0);

        // Re-enable canonical mode
        termios.set_canonical(true);
        assert!(termios.is_canonical());
        assert_ne!(termios.c_lflag & ICANON, 0);
    }

    #[test]
    fn test_set_canonical_preserves_other_flags() {
        let mut termios = Termios::default();
        let original_lflag = termios.c_lflag;

        // Disable canonical mode
        termios.set_canonical(false);

        // Other flags should be preserved
        assert_eq!(termios.c_lflag | ICANON, original_lflag);

        // Re-enable canonical mode
        termios.set_canonical(true);

        // Should be back to original
        assert_eq!(termios.c_lflag, original_lflag);
    }

    #[test]
    fn test_set_canonical_idempotent() {
        let mut termios = Termios::default();

        // Multiple calls to set_canonical(true) should be idempotent
        termios.set_canonical(true);
        termios.set_canonical(true);
        assert!(termios.is_canonical());

        // Multiple calls to set_canonical(false) should be idempotent
        termios.set_canonical(false);
        termios.set_canonical(false);
        assert!(!termios.is_canonical());
    }
}
