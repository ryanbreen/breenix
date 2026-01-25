//! Simple kernel-mode shell for ARM64.
//!
//! This is a minimal shell that runs in kernel mode, processing
//! VirtIO keyboard input directly. It provides basic commands
//! for system interaction.
//!
//! In the future, this can be replaced with a proper userspace shell
//! with TTY layer support.

use crate::arch_impl::aarch64::timer;
use crate::graphics::terminal_manager;
use alloc::format;

/// Maximum line buffer size
const MAX_LINE_LEN: usize = 256;

/// Shell state for ARM64
pub struct ShellState {
    /// Line buffer for current command
    line_buffer: [u8; MAX_LINE_LEN],
    /// Current position in line buffer (end of input)
    line_pos: usize,
}

impl ShellState {
    /// Create a new shell state
    pub const fn new() -> Self {
        Self {
            line_buffer: [0; MAX_LINE_LEN],
            line_pos: 0,
        }
    }

    /// Process a character from keyboard input.
    ///
    /// Returns true if a command was executed (meaning we need a new prompt).
    pub fn process_char(&mut self, c: char) -> bool {
        match c {
            '\n' | '\r' => {
                // Enter pressed - execute command
                terminal_manager::write_char_to_shell('\n');
                self.execute_line();
                // Clear buffer for next command
                self.line_pos = 0;
                // Show new prompt
                terminal_manager::write_str_to_shell("breenix> ");
                true
            }
            '\x08' | '\x7f' => {
                // Backspace (0x08) or DEL (0x7f)
                if self.line_pos > 0 {
                    self.line_pos -= 1;
                    // Erase character from display: backspace, space, backspace
                    terminal_manager::write_char_to_shell('\x08');
                    terminal_manager::write_char_to_shell(' ');
                    terminal_manager::write_char_to_shell('\x08');
                }
                false
            }
            c if c.is_ascii() && !c.is_control() => {
                // Regular printable character
                if self.line_pos < MAX_LINE_LEN - 1 {
                    self.line_buffer[self.line_pos] = c as u8;
                    self.line_pos += 1;
                    terminal_manager::write_char_to_shell(c);
                }
                false
            }
            _ => false,
        }
    }

    /// Execute the current line buffer as a command
    fn execute_line(&self) {
        // Get the line as a string
        let line = match core::str::from_utf8(&self.line_buffer[..self.line_pos]) {
            Ok(s) => s.trim(),
            Err(_) => {
                terminal_manager::write_str_to_shell("Error: invalid UTF-8 input\n");
                return;
            }
        };

        // Empty line - just show new prompt
        if line.is_empty() {
            return;
        }

        // Parse command and arguments
        let mut parts = line.split_whitespace();
        let cmd = match parts.next() {
            Some(c) => c,
            None => return,
        };

        // Execute built-in commands
        match cmd {
            "help" => self.cmd_help(),
            "echo" => self.cmd_echo(line),
            "clear" => self.cmd_clear(),
            "time" | "uptime" => self.cmd_time(),
            "uname" => self.cmd_uname(),
            "ps" => self.cmd_ps(),
            "mem" | "free" => self.cmd_mem(),
            _ => {
                terminal_manager::write_str_to_shell("Unknown command: ");
                terminal_manager::write_str_to_shell(cmd);
                terminal_manager::write_str_to_shell("\nType 'help' for available commands.\n");
            }
        }
    }

    /// Display help text
    fn cmd_help(&self) {
        terminal_manager::write_str_to_shell(
            "Breenix ARM64 Shell Commands:\n\
             \n\
             help     - Show this help message\n\
             echo     - Print arguments to terminal\n\
             clear    - Clear the terminal screen\n\
             time     - Show system uptime\n\
             uname    - Show system information\n\
             ps       - Show running processes\n\
             mem      - Show memory usage\n\
             \n",
        );
    }

    /// Echo command - print arguments
    fn cmd_echo(&self, line: &str) {
        // Find the first space after "echo" and print everything after it
        if let Some(pos) = line.find(' ') {
            let args = line[pos + 1..].trim_start();
            terminal_manager::write_str_to_shell(args);
        }
        terminal_manager::write_str_to_shell("\n");
    }

    /// Clear the terminal
    fn cmd_clear(&self) {
        terminal_manager::clear_shell();
    }

    /// Show system uptime
    fn cmd_time(&self) {
        match timer::monotonic_time() {
            Some((secs, nanos)) => {
                let millis = nanos / 1_000_000;
                let output = format!("Uptime: {}.{:03} seconds\n", secs, millis);
                terminal_manager::write_str_to_shell(&output);
            }
            None => {
                terminal_manager::write_str_to_shell("Error: timer not available\n");
            }
        }
    }

    /// Show system information
    fn cmd_uname(&self) {
        terminal_manager::write_str_to_shell("Breenix 0.1.0 aarch64\n");
    }

    /// Show process list (placeholder)
    fn cmd_ps(&self) {
        terminal_manager::write_str_to_shell(
            "PID  STATE  NAME\n\
             0    R      kernel\n",
        );
    }

    /// Show memory usage (placeholder)
    fn cmd_mem(&self) {
        terminal_manager::write_str_to_shell(
            "Memory usage:\n\
             Total:     512 MB\n\
             Available: 256 KB (heap)\n\
             Note: ARM64 heap is a simple bump allocator\n",
        );
    }
}

impl Default for ShellState {
    fn default() -> Self {
        Self::new()
    }
}
