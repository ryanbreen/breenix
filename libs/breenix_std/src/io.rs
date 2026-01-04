//! I/O primitives for Breenix userspace.
//!
//! This module provides stdout/stderr writers that implement core::fmt::Write,
//! enabling the use of format strings with print!/println! macros.

use core::fmt::{self, Write};
use libbreenix::io::write;
use libbreenix::types::fd;

/// Writer for stdout that implements core::fmt::Write.
pub struct StdoutWriter;

impl Write for StdoutWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let mut written = 0;
        while written < bytes.len() {
            let result = write(fd::STDOUT, &bytes[written..]);
            if result < 0 {
                return Err(fmt::Error);
            }
            written += result as usize;
        }
        Ok(())
    }
}

/// Writer for stderr that implements core::fmt::Write.
pub struct StderrWriter;

impl Write for StderrWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let mut written = 0;
        while written < bytes.len() {
            let result = write(fd::STDERR, &bytes[written..]);
            if result < 0 {
                return Err(fmt::Error);
            }
            written += result as usize;
        }
        Ok(())
    }
}

/// Internal function to print formatted output to stdout.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = StdoutWriter.write_fmt(args);
}

/// Internal function to print formatted output to stderr.
#[doc(hidden)]
pub fn _eprint(args: fmt::Arguments) {
    let _ = StderrWriter.write_fmt(args);
}

/// Print to standard output.
///
/// Equivalent to the `print!` macro from std.
///
/// # Example
/// ```ignore
/// use breenix_std::print;
/// print!("Hello, {}!", "world");
/// ```
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::io::_print(format_args!($($arg)*))
    };
}

/// Print to standard output with a newline.
///
/// Equivalent to the `println!` macro from std.
///
/// # Example
/// ```ignore
/// use breenix_std::println;
/// println!("Hello, {}!", "world");
/// ```
#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::io::_print(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// Print to standard error.
///
/// Equivalent to the `eprint!` macro from std.
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        $crate::io::_eprint(format_args!($($arg)*))
    };
}

/// Print to standard error with a newline.
///
/// Equivalent to the `eprintln!` macro from std.
#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::eprint!("\n")
    };
    ($($arg:tt)*) => {
        $crate::io::_eprint(format_args!("{}\n", format_args!($($arg)*)))
    };
}
