//! TTY Subsystem for Breenix
//!
//! Provides POSIX-compliant terminal semantics including:
//! - Line discipline (canonical vs raw mode)
//! - Signal generation (SIGINT, SIGTSTP, etc.)
//! - Terminal attributes (termios)
//! - Echo and line editing

pub mod driver;
pub mod ioctl;
pub mod line_discipline;
pub mod pty;
pub mod termios;

// Re-export for external use
// Allow unused - these are public API re-exports for Phase 4+ syscalls and ioctls
#[allow(unused_imports)]
pub use line_discipline::LineDiscipline;
#[allow(unused_imports)]
pub use termios::Termios;

// Re-export driver functions for external use
// Allow unused - these are public API re-exports for keyboard interrupt integration
#[allow(unused_imports)]
pub use driver::{console, push_char, push_char_nonblock, write_output, TtyDevice};

/// Initialize the TTY subsystem
///
/// This creates the console TTY device (TTY 0) and sets up
/// the infrastructure for terminal I/O.
pub fn init() {
    driver::init_console();
    pty::init();
    log::info!("TTY subsystem initialized");
}
