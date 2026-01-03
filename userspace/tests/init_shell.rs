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
use libbreenix::io::{close, dup2, pipe, print, println, read, write};
use libbreenix::process::{exec, fork, waitpid, wexitstatus, wifexited, yield_now};
use libbreenix::signal::{sigaction, Sigaction, SIGINT};
use libbreenix::termios::{
    cfmakeraw, lflag, oflag, tcgetattr, tcsetattr, Termios, TCSANOW,
};
use libbreenix::time::now_monotonic;
use libbreenix::types::fd::{STDIN, STDOUT};
use libbreenix::Timespec;

// ============================================================================
// Program Registry - Lookup table for external commands
// ============================================================================

/// An entry in the program registry representing an external command
/// that can be executed via fork+exec.
#[derive(Clone, Copy)]
pub struct ProgramEntry {
    /// The command name as typed by the user (e.g., "hello")
    pub name: &'static str,
    /// The null-terminated binary name for exec (e.g., b"hello_world\0")
    /// This must match the binary name in the test disk.
    pub binary_name: &'static [u8],
    /// Brief description for help text
    pub description: &'static str,
}

/// Static registry of all known external programs.
/// These are programs built in userspace/tests/ and loaded from the test disk.
static PROGRAM_REGISTRY: &[ProgramEntry] = &[
    ProgramEntry {
        name: "hello",
        binary_name: b"hello_world\0",
        description: "Print hello world message",
    },
    ProgramEntry {
        name: "hello_world",
        binary_name: b"hello_world\0",
        description: "Print hello world message",
    },
    ProgramEntry {
        name: "counter",
        binary_name: b"counter\0",
        description: "Count from 0 to 9",
    },
    ProgramEntry {
        name: "spinner",
        binary_name: b"spinner\0",
        description: "Display spinning animation",
    },
    ProgramEntry {
        name: "hello_time",
        binary_name: b"hello_time\0",
        description: "Print hello with timestamp",
    },
    ProgramEntry {
        name: "fork_test",
        binary_name: b"fork_test\0",
        description: "Test fork syscall",
    },
    ProgramEntry {
        name: "pipe_test",
        binary_name: b"pipe_test\0",
        description: "Test pipe syscall",
    },
    ProgramEntry {
        name: "signal_test",
        binary_name: b"signal_test\0",
        description: "Test signal handling",
    },
];

/// Find a program in the registry by command name.
///
/// Returns Some(ProgramEntry) if found, None otherwise.
pub fn find_program(name: &str) -> Option<&'static ProgramEntry> {
    for entry in PROGRAM_REGISTRY {
        if entry.name == name {
            return Some(entry);
        }
    }
    None
}

/// Try to execute an external command via fork+exec.
///
/// Arguments:
/// - `cmd_name`: The command name to execute
/// - `_args`: Arguments (currently unused)
/// - `background`: If true, don't wait for the child process
///
/// Returns:
/// - Ok(exit_code) if the program was found and executed (0 for background)
/// - Err(()) if the program was not found in the registry
pub fn try_execute_external(cmd_name: &str, _args: &str, background: bool) -> Result<i32, ()> {
    let entry = find_program(cmd_name).ok_or(())?;

    if !background {
        print("Running: ");
        println(entry.name);
    }

    // Fork a child process
    let pid = fork();

    if pid < 0 {
        // Fork failed
        print("Error: fork failed with code ");
        print_num((-pid) as u64);
        println("");
        return Ok(-1);
    }

    if pid == 0 {
        // Child process - exec the program
        let result = exec(entry.binary_name);

        // If exec returns, it failed
        print("Error: exec failed with code ");
        print_num((-result) as u64);
        println("");

        // Exit the child with error code
        libbreenix::process::exit(1);
    }

    // Parent process
    if background {
        // Background execution - don't wait, just print job notification
        let job_id = get_next_job_id();
        print_job_notification(job_id, pid);
        return Ok(0);
    }

    // Foreground execution - wait for child to complete
    let mut status: i32 = 0;
    let wait_result = waitpid(pid as i32, &mut status as *mut i32, 0);

    if wait_result < 0 {
        print("Error: waitpid failed with code ");
        print_num((-wait_result) as u64);
        println("");
        return Ok(-1);
    }

    // Extract and return exit code
    if wifexited(status) {
        let exit_code = wexitstatus(status);
        if exit_code != 0 {
            print("Process exited with code: ");
            print_num(exit_code as u64);
            println("");
        }
        Ok(exit_code)
    } else {
        println("Process terminated abnormally");
        Ok(-1)
    }
}

/// List all available external programs (for help command)
pub fn list_external_programs() {
    println("External programs:");
    for entry in PROGRAM_REGISTRY {
        print("  ");
        print(entry.name);
        // Pad to align descriptions
        let padding = 12 - entry.name.len();
        for _ in 0..padding {
            print(" ");
        }
        print("- ");
        println(entry.description);
    }
}

// ============================================================================
// Pipeline Support - Parse and execute cmd1 | cmd2 | cmd3
// ============================================================================

/// Maximum number of commands in a pipeline
const MAX_PIPELINE_COMMANDS: usize = 8;

/// A parsed command in a pipeline
#[derive(Clone, Copy)]
struct PipelineCommand<'a> {
    /// The command name (first word)
    name: &'a str,
    /// The full command string including arguments
    full: &'a str,
}

/// Check if the input contains a pipe character
fn contains_pipe(s: &str) -> bool {
    for c in s.as_bytes() {
        if *c == b'|' {
            return true;
        }
    }
    false
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

// ============================================================================
// Background Process Support - cmd &
// ============================================================================

/// Check if a command should be run in the background (ends with &)
fn is_background_command(input: &str) -> bool {
    let trimmed = trim(input);
    trimmed.ends_with('&')
}

/// Strip the background operator from a command
fn strip_background_operator(input: &str) -> &str {
    let trimmed = trim(input);
    if trimmed.ends_with('&') {
        trim(&trimmed[..trimmed.len() - 1])
    } else {
        trimmed
    }
}

/// Get the next job ID and increment the counter
fn get_next_job_id() -> usize {
    unsafe {
        let job_id = NEXT_JOB_ID;
        NEXT_JOB_ID += 1;
        job_id
    }
}

/// Print a background job notification
fn print_job_notification(job_id: usize, pid: i64) {
    print("[");
    print_num(job_id as u64);
    print("] ");
    print_num(pid as u64);
    println("");
}

/// Split a string by pipe character, returning up to MAX_PIPELINE_COMMANDS segments.
/// Each segment is trimmed of whitespace.
/// Returns the number of commands found.
fn split_pipeline<'a>(
    input: &'a str,
    commands: &mut [PipelineCommand<'a>; MAX_PIPELINE_COMMANDS],
) -> usize {
    let bytes = input.as_bytes();
    let mut count = 0;
    let mut start = 0;

    for (i, &c) in bytes.iter().enumerate() {
        if c == b'|' {
            if count < MAX_PIPELINE_COMMANDS {
                let segment = trim(&input[start..i]);
                if !segment.is_empty() {
                    // Extract command name (first word)
                    let name_end = segment
                        .as_bytes()
                        .iter()
                        .position(|&ch| ch == b' ')
                        .unwrap_or(segment.len());
                    commands[count] = PipelineCommand {
                        name: &segment[..name_end],
                        full: segment,
                    };
                    count += 1;
                }
            }
            start = i + 1;
        }
    }

    // Handle the last segment after the final pipe (or entire string if no pipe)
    if count < MAX_PIPELINE_COMMANDS && start < bytes.len() {
        let segment = trim(&input[start..]);
        if !segment.is_empty() {
            let name_end = segment
                .as_bytes()
                .iter()
                .position(|&ch| ch == b' ')
                .unwrap_or(segment.len());
            commands[count] = PipelineCommand {
                name: &segment[..name_end],
                full: segment,
            };
            count += 1;
        }
    }

    count
}

/// Execute a single command in a pipeline context.
/// This handles built-in commands that can participate in pipelines (like echo).
/// For external commands, it calls exec.
///
/// This function never returns - it either execs or exits.
fn execute_pipeline_command(cmd: &PipelineCommand) -> ! {
    // Handle built-in commands that can participate in pipelines
    if cmd.name == "echo" {
        // Extract args after "echo "
        let args = if cmd.full.len() > 5 {
            trim(&cmd.full[5..])
        } else {
            ""
        };
        println(args);
        libbreenix::process::exit(0);
    }

    // Try external command
    if let Some(entry) = find_program(cmd.name) {
        let result = exec(entry.binary_name);
        // If exec returns, it failed
        print("exec failed: ");
        print_num((-result) as u64);
        println("");
        libbreenix::process::exit(1);
    }

    // Command not found
    print("command not found: ");
    println(cmd.name);
    libbreenix::process::exit(127)
}

/// Execute a pipeline of commands.
///
/// For a pipeline like: cmd1 | cmd2 | cmd3
///
/// This creates:
/// - pipe1 between cmd1 and cmd2
/// - pipe2 between cmd2 and cmd3
///
/// Each command is forked as a child process:
/// - cmd1: stdout -> pipe1[write]
/// - cmd2: stdin <- pipe1[read], stdout -> pipe2[write]
/// - cmd3: stdin <- pipe2[read]
///
/// Returns true if pipeline was executed (even with errors), false if no valid commands.
fn execute_pipeline(input: &str) -> bool {
    let mut commands: [PipelineCommand; MAX_PIPELINE_COMMANDS] = [PipelineCommand {
        name: "",
        full: "",
    }; MAX_PIPELINE_COMMANDS];

    let cmd_count = split_pipeline(input, &mut commands);

    if cmd_count == 0 {
        return false;
    }

    // Single command - no pipeline needed, fall back to normal handling
    if cmd_count == 1 {
        return false;
    }

    // Verify all commands exist before starting pipeline
    for i in 0..cmd_count {
        let cmd = &commands[i];
        if cmd.name != "echo" && find_program(cmd.name).is_none() {
            print("command not found: ");
            println(cmd.name);
            return true; // Return true since we handled the error
        }
    }

    // Store PIDs of all child processes
    let mut child_pids: [i64; MAX_PIPELINE_COMMANDS] = [0; MAX_PIPELINE_COMMANDS];

    // Store the read end of the previous pipe (-1 means no previous pipe)
    let mut prev_read_fd: i32 = -1;

    for i in 0..cmd_count {
        let is_last = i == cmd_count - 1;

        // Create pipe for this stage (except for the last command)
        let mut pipefd: [i32; 2] = [-1, -1];
        if !is_last {
            let ret = pipe(&mut pipefd);
            if ret < 0 {
                print("pipe failed: ");
                print_num((-ret) as u64);
                println("");
                // Clean up previous pipe if exists
                if prev_read_fd >= 0 {
                    close(prev_read_fd as u64);
                }
                return true;
            }
        }

        // Fork child for this command
        let pid = fork();

        if pid < 0 {
            print("fork failed: ");
            print_num((-pid) as u64);
            println("");
            // Clean up pipes
            if prev_read_fd >= 0 {
                close(prev_read_fd as u64);
            }
            if !is_last {
                close(pipefd[0] as u64);
                close(pipefd[1] as u64);
            }
            return true;
        }

        if pid == 0 {
            // ========== CHILD PROCESS ==========

            // Set up stdin from previous pipe (if not first command)
            if prev_read_fd >= 0 {
                // Redirect stdin to read from previous pipe
                dup2(prev_read_fd as u64, STDIN);
                close(prev_read_fd as u64);
            }

            // Set up stdout to current pipe (if not last command)
            if !is_last {
                // Redirect stdout to write end of current pipe
                dup2(pipefd[1] as u64, STDOUT);
                close(pipefd[0] as u64); // Close read end in child
                close(pipefd[1] as u64); // Close write end after dup
            }

            // Execute the command (never returns)
            execute_pipeline_command(&commands[i]);
        }

        // ========== PARENT PROCESS ==========

        // Store child PID
        child_pids[i] = pid;

        // Close the previous read fd (child has it now)
        if prev_read_fd >= 0 {
            close(prev_read_fd as u64);
        }

        // Close write end of current pipe (child has it)
        // Save read end for next iteration
        if !is_last {
            close(pipefd[1] as u64);
            prev_read_fd = pipefd[0];
        }
    }

    // Wait for all children to complete
    for i in 0..cmd_count {
        if child_pids[i] > 0 {
            let mut status: i32 = 0;
            waitpid(child_pids[i] as i32, &mut status as *mut i32, 0);
        }
    }

    true
}

// Line buffer for reading input
static mut LINE_BUF: [u8; 256] = [0; 256];
static mut LINE_LEN: usize = 0;

// EAGAIN error code
const EAGAIN: i64 = 11;

// Global flag to track SIGINT received
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);

// Saved termios for restoration
static mut SAVED_TERMIOS: Option<Termios> = None;

// Background job tracking
static mut NEXT_JOB_ID: usize = 1;

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

/// Handle the "help" command
fn cmd_help() {
    println("Built-in commands:");
    println("  help   - Show this help message");
    println("  echo   - Echo text back to the terminal");
    println("  ps     - List processes (placeholder)");
    println("  uptime - Show time since boot");
    println("  clear  - Clear the screen (ANSI escape sequence)");
    println("  raw    - Switch to raw mode and show keypresses");
    println("  cooked - Switch back to canonical (cooked) mode");
    println("  progs  - List available external programs");
    println("  exit   - Attempt to exit (init cannot exit)");
    println("");
    list_external_programs();
    println("");
    println("Pipes:");
    println("  cmd1 | cmd2      - Pipe output of cmd1 to input of cmd2");
    println("  echo hello | cat - Example: pipe echo output (not yet useful)");
    println("");
    println("Background execution:");
    println("  command &        - Run command in background");
    println("  hello &          - Example: run hello_world in background");
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

    // Check for background operator (&) first
    let background = is_background_command(line);
    let line = if background {
        strip_background_operator(line)
    } else {
        line
    };

    // Check for pipeline (contains '|' with 2+ commands)
    // Note: Background pipelines are not yet supported
    if contains_pipe(line) {
        if background {
            println("Background pipelines not yet supported");
            return;
        }
        if execute_pipeline(line) {
            return;
        }
        // Fall through to normal handling if pipeline fails validation
    }

    // Extract command name and arguments
    let cmd_end = line
        .as_bytes()
        .iter()
        .position(|&c| c == b' ')
        .unwrap_or(line.len());
    let cmd = &line[..cmd_end];
    let args = if cmd_end < line.len() {
        trim(&line[cmd_end + 1..])
    } else {
        ""
    };

    // Built-in commands cannot run in background
    if background {
        // Only external commands can run in background
        if try_execute_external(cmd, args, true).is_err() {
            cmd_unknown(cmd);
        }
        return;
    }

    // Match built-in commands first (foreground only)
    match cmd {
        "help" => cmd_help(),
        "echo" => cmd_echo(args),
        "ps" => cmd_ps(),
        "uptime" => cmd_uptime(),
        "clear" => cmd_clear(),
        "raw" => cmd_raw(),
        "cooked" => cmd_cooked(),
        "progs" => list_external_programs(),
        "exit" | "quit" => cmd_exit(),
        _ => {
            // Try to execute as an external command
            if try_execute_external(cmd, args, false).is_err() {
                cmd_unknown(cmd);
            }
        }
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
