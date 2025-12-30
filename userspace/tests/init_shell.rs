//! Minimal Interactive Shell for Breenix OS
//!
//! This is meant to run as PID 1 (init). It provides a simple REPL that:
//! 1. Prints a welcome banner
//! 2. Shows a prompt "breenix> "
//! 3. Reads a line of input (blocking read from stdin)
//! 4. Parses and executes simple commands
//! 5. Loops forever

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{print, println, read, write};
use libbreenix::process::yield_now;
use libbreenix::time::now_monotonic;
use libbreenix::types::fd::{STDIN, STDOUT};
use libbreenix::Timespec;

// Line buffer for reading input
static mut LINE_BUF: [u8; 256] = [0; 256];
static mut LINE_LEN: usize = 0;

// EAGAIN error code
const EAGAIN: i64 = 11;

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
fn read_line() -> &'static str {
    unsafe {
        LINE_LEN = 0;

        loop {
            let mut c = [0u8; 1];
            let n = read(STDIN, &mut c);

            if n == -EAGAIN || n == 0 {
                // No data available - yield and retry
                yield_now();
                continue;
            }

            if n < 0 {
                // Other error - yield and retry
                yield_now();
                continue;
            }

            let ch = c[0];

            // Handle newline - end of input
            if ch == b'\n' || ch == b'\r' {
                println("");
                LINE_BUF[LINE_LEN] = 0;
                return core::str::from_utf8(&LINE_BUF[..LINE_LEN]).unwrap_or("");
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
    println("  clear  - Clear the screen");
    println("  exit   - Attempt to exit (init cannot exit)");
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
    // Print 25 newlines to "clear" the screen
    for _ in 0..25 {
        println("");
    }
}

/// Handle the "exit" command
fn cmd_exit() {
    println("Cannot exit init!");
    println("The init process must run forever.");
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    print_banner();

    // Main REPL loop
    loop {
        print("breenix> ");
        let line = read_line();
        handle_command(line);
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
