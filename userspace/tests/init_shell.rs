//! Minimal Interactive Shell for Breenix OS
//!
//! This is meant to run as PID 1 (init). It provides a simple REPL that:
//! 1. Prints a welcome banner
//! 2. Shows a prompt "breenix> "
//! 3. Reads a line of input (blocking read from stdin)
//! 4. Parses and executes simple commands
//! 5. Loops forever
//!
//! Features for testing TTY line discipline:
//! - "raw" command: switches to raw mode and shows keypresses
//! - "cooked" command: switches back to canonical mode
//! - Ctrl+C handling: shows ^C and gives new prompt
//! - Line editing: backspace works in canonical mode

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};
use libbreenix::io::{print, println, read, write};
use libbreenix::process::yield_now;
use libbreenix::signal::{sigaction, Sigaction, SIGINT};
use libbreenix::termios::{
    cfmakeraw, lflag, oflag, tcgetattr, tcsetattr, Termios, TCSANOW,
};
use libbreenix::time::now_monotonic;
use libbreenix::types::fd::{STDIN, STDOUT};
use libbreenix::Timespec;

// Line buffer for reading input
static mut LINE_BUF: [u8; 256] = [0; 256];
static mut LINE_LEN: usize = 0;

// EAGAIN error code
const EAGAIN: i64 = 11;

// Global flag to track SIGINT received
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

// Saved termios for restoration
static mut SAVED_TERMIOS: Option<Termios> = None;

/// SIGINT handler - just sets a flag
extern "C" fn sigint_handler(_sig: i32) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
}

/// Print a single character
fn print_char(c: u8) {
    let _ = write(STDOUT, &[c]);
}

/// Print a number (u64)
fn print_num(mut n: u64) {
    if n == 0 {
        print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }

    // Print in reverse order
    while i > 0 {
        i -= 1;
        print_char(buf[i]);
    }
}

/// Read a line from stdin, handling backspace and yielding on EAGAIN
/// Returns None if interrupted by SIGINT (Ctrl+C)
fn read_line() -> Option<&'static str> {
    unsafe {
        LINE_LEN = 0;

        loop {
            // Check for SIGINT (Ctrl+C)
            if SIGINT_RECEIVED.swap(false, Ordering::SeqCst) {
                print("^C\n");
                LINE_LEN = 0;
                return None; // Signal that we got interrupted
            }

            let mut c = [0u8; 1];
            let n = read(STDIN, &mut c);

            if n == -EAGAIN || n == 0 {
                // No data available - yield and retry
                yield_now();
                continue;
            }

            if n < 0 {
                // Other error (possibly EINTR from signal) - yield and retry
                yield_now();
                continue;
            }

            let ch = c[0];

            // Handle newline - end of input
            if ch == b'\n' || ch == b'\r' {
                println("");
                LINE_BUF[LINE_LEN] = 0;
                return Some(core::str::from_utf8(&LINE_BUF[..LINE_LEN]).unwrap_or(""));
            }

            // Handle backspace (ASCII DEL or BS)
            if ch == 0x7f || ch == 0x08 {
                if LINE_LEN > 0 {
                    LINE_LEN -= 1;
                    // Move cursor back, print space, move back again
                    print("\x08 \x08");
                }
                continue;
            }

            // Handle printable characters
            if LINE_LEN < 255 && ch >= 0x20 && ch < 0x7f {
                LINE_BUF[LINE_LEN] = ch;
                LINE_LEN += 1;
                // Note: Kernel echoes characters in push_byte, so no shell echo needed
            }
        }
    }
}

/// Trim leading and trailing whitespace from a string
fn trim(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();

    // Trim leading whitespace
    while start < end && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }

    // Trim trailing whitespace
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t' || bytes[end - 1] == 0)
    {
        end -= 1;
    }

    core::str::from_utf8(&bytes[start..end]).unwrap_or("")
}

/// Check if a string starts with a prefix
fn starts_with(s: &str, prefix: &str) -> bool {
    if s.len() < prefix.len() {
        return false;
    }
    s.as_bytes()[..prefix.len()] == *prefix.as_bytes()
}

/// Handle the "help" command
fn cmd_help() {
    println("Available commands:");
    println("  help   - Show this help message");
    println("  echo   - Echo text back to the terminal");
    println("  ps     - List processes (placeholder)");
    println("  uptime - Show time since boot");
    println("  clear  - Clear the screen (ANSI escape sequence)");
    println("  raw    - Switch to raw mode and show keypresses");
    println("  cooked - Switch back to canonical (cooked) mode");
    println("  exit   - Attempt to exit (init cannot exit)");
    println("");
    println("TTY testing:");
    println("  - Ctrl+C shows ^C and gives new prompt");
    println("  - Backspace works for line editing");
    println("  - In raw mode, each keypress is shown immediately");
}

/// Handle the "echo" command
fn cmd_echo(args: &str) {
    println(args);
}

/// Handle the "ps" command
fn cmd_ps() {
    println("  PID  CMD");
    println("    1  init");
}

/// Handle the "uptime" command
fn cmd_uptime() {
    let ts: Timespec = now_monotonic();
    let secs = ts.tv_sec as u64;
    let mins = secs / 60;
    let hours = mins / 60;

    print("up ");

    if hours > 0 {
        print_num(hours);
        print(" hour");
        if hours != 1 {
            print("s");
        }
        print(", ");
    }

    if mins > 0 || hours > 0 {
        print_num(mins % 60);
        print(" minute");
        if mins % 60 != 1 {
            print("s");
        }
        print(", ");
    }

    print_num(secs % 60);
    print(" second");
    if secs % 60 != 1 {
        print("s");
    }
    println("");
}

/// Handle the "clear" command
fn cmd_clear() {
    // Use ANSI escape sequences to clear screen and move cursor to home
    // ESC[2J - Clear entire screen
    // ESC[H  - Move cursor to home position (1,1)
    print("\x1b[2J\x1b[H");
}

/// Handle the "exit" command
fn cmd_exit() {
    println("Cannot exit init!");
    println("The init process must run forever.");
}

/// Print a byte in hexadecimal
fn print_hex_byte(b: u8) {
    let high = b >> 4;
    let low = b & 0x0F;
    let high_char = if high < 10 {
        b'0' + high
    } else {
        b'a' + (high - 10)
    };
    let low_char = if low < 10 { b'0' + low } else { b'a' + (low - 10) };
    print_char(high_char);
    print_char(low_char);
}

/// Handle the "raw" command - switch to raw mode and show keypresses
fn cmd_raw() {
    println("Switching to raw mode...");
    println("Press keys to see their codes. Press 'q' to exit raw mode.");
    println("");

    // Get current terminal settings
    let mut termios = Termios::default();
    if tcgetattr(0, &mut termios).is_err() {
        println("Error: Could not get terminal attributes");
        return;
    }

    // Save original settings for restoration
    let original_termios = termios;
    unsafe {
        SAVED_TERMIOS = Some(original_termios);
    }

    // Switch to raw mode
    cfmakeraw(&mut termios);

    // Keep output processing enabled so newlines work correctly
    termios.c_oflag |= oflag::OPOST | oflag::ONLCR;

    if tcsetattr(0, TCSANOW, &termios).is_err() {
        println("Error: Could not set raw mode");
        return;
    }

    println("Raw mode enabled. Type keys:");
    println("");

    // Read and display keypresses until 'q' is pressed
    loop {
        let mut c = [0u8; 1];
        let n = read(STDIN, &mut c);

        if n == -EAGAIN || n == 0 {
            yield_now();
            continue;
        }

        if n < 0 {
            yield_now();
            continue;
        }

        let ch = c[0];

        // Display the keypress
        print("Key: ");

        // Show printable representation or control code name
        if ch >= 0x20 && ch < 0x7f {
            print("'");
            print_char(ch);
            print("' ");
        } else if ch == 0x1b {
            print("ESC ");
        } else if ch < 0x20 {
            print("^");
            print_char(b'@' + ch);
            print(" ");
        } else if ch == 0x7f {
            print("DEL ");
        } else {
            print("    ");
        }

        print("(0x");
        print_hex_byte(ch);
        print(", ");
        print_num(ch as u64);
        println(")");

        // Exit on 'q'
        if ch == b'q' || ch == b'Q' {
            println("");
            println("Exiting raw mode...");
            break;
        }
    }

    // Restore original terminal settings
    if tcsetattr(0, TCSANOW, &original_termios).is_err() {
        println("Warning: Could not restore terminal settings");
    }

    println("Back to canonical mode.");
}

/// Handle the "cooked" command - switch back to canonical mode
fn cmd_cooked() {
    println("Switching to canonical (cooked) mode...");

    // Check if we have saved settings
    let restored = unsafe {
        if let Some(ref original) = SAVED_TERMIOS {
            if tcsetattr(0, TCSANOW, original).is_ok() {
                true
            } else {
                false
            }
        } else {
            // No saved settings, set up default canonical mode
            let mut termios = Termios::default();
            if tcgetattr(0, &mut termios).is_err() {
                println("Error: Could not get terminal attributes");
                return;
            }

            // Enable canonical mode, echo, and signals
            termios.c_lflag |= lflag::ICANON | lflag::ECHO | lflag::ECHOE | lflag::ISIG;
            termios.c_oflag |= oflag::OPOST | oflag::ONLCR;

            if tcsetattr(0, TCSANOW, &termios).is_ok() {
                true
            } else {
                false
            }
        }
    };

    if restored {
        println("Canonical mode enabled.");
        println("Line editing and signals are now active.");
    } else {
        println("Error: Could not set canonical mode");
    }
}

/// Handle an unknown command
fn cmd_unknown(cmd: &str) {
    print("Unknown command: ");
    println(cmd);
    println("Type 'help' for available commands.");
}

/// Parse and execute a command line
fn handle_command(line: &str) {
    let line = trim(line);

    if line.is_empty() {
        return;
    }

    // Match commands
    if line == "help" {
        cmd_help();
    } else if line == "echo" {
        cmd_echo("");
    } else if starts_with(line, "echo ") {
        // Get everything after "echo "
        let args = &line[5..];
        cmd_echo(args);
    } else if line == "ps" {
        cmd_ps();
    } else if line == "uptime" {
        cmd_uptime();
    } else if line == "clear" {
        cmd_clear();
    } else if line == "raw" {
        cmd_raw();
    } else if line == "cooked" {
        cmd_cooked();
    } else if line == "exit" || line == "quit" {
        cmd_exit();
    } else {
        // Extract the first word as the command name
        let cmd_end = line
            .as_bytes()
            .iter()
            .position(|&c| c == b' ')
            .unwrap_or(line.len());
        let cmd = &line[..cmd_end];
        cmd_unknown(cmd);
    }
}

/// Print the welcome banner
fn print_banner() {
    println("");
    println("========================================");
    println("     Breenix OS Interactive Shell");
    println("========================================");
    println("");
    println("Welcome to Breenix! Type 'help' for available commands.");
    println("");
}

/// Set up signal handlers
fn setup_signal_handlers() {
    // Set up SIGINT handler for Ctrl+C
    let action = Sigaction::new(sigint_handler);
    if sigaction(SIGINT, Some(&action), None).is_err() {
        println("Warning: Could not set up SIGINT handler");
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Set up signal handlers before anything else
    setup_signal_handlers();

    print_banner();

    // Main REPL loop
    loop {
        print("breenix> ");

        // read_line returns None if interrupted by Ctrl+C
        if let Some(line) = read_line() {
            handle_command(line);
        }
        // If None (interrupted), just continue to print new prompt
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("PANIC in init shell!");
    // Init cannot exit, so just loop forever
    loop {
        core::hint::spin_loop();
    }
}
