//! Panic handling for Breenix userspace programs.
//!
//! This module provides a panic handler that prints panic information
//! to stderr and exits the process with code 101 (Rust's panic exit code).

use core::panic::PanicInfo;
use crate::io::StderrWriter;
use core::fmt::Write;
use libbreenix::process::exit;

/// Panic handler for Breenix userspace programs.
///
/// This function is called when a panic occurs. It:
/// 1. Prints the panic message to stderr
/// 2. Exits the process with code 101
///
/// # Note
/// This handler uses abort semantics (no unwinding) since Breenix
/// userspace programs are compiled with panic=abort.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut stderr = StderrWriter;

    // Print panic header
    let _ = stderr.write_str("\n=== PANIC ===\n");

    // Print location if available
    if let Some(location) = info.location() {
        let _ = write!(
            stderr,
            "panicked at {}:{}:{}\n",
            location.file(),
            location.line(),
            location.column()
        );
    }

    // Print the panic message
    let _ = write!(stderr, "message: {}\n", info.message());

    let _ = stderr.write_str("=============\n");

    // Exit with Rust's panic exit code
    exit(101);
}
