//! Kernel-mode shell for ARM64.
//!
//! This is a temporary shell that runs in kernel mode, providing basic
//! interaction while ARM64 userspace exec is being developed.
//!
//! Once ARM64 supports userspace fork/exec, this will be replaced by
//! the same init_shell that runs on x86_64.

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
        let (cmd, args) = match line.find(' ') {
            Some(pos) => (&line[..pos], line[pos + 1..].trim()),
            None => (line, ""),
        };

        // Execute built-in commands
        match cmd {
            "help" => self.cmd_help(),
            "echo" => self.cmd_echo(args),
            "clear" => self.cmd_clear(),
            "time" | "uptime" => self.cmd_uptime(),
            "uname" => self.cmd_uname(),
            "ps" => self.cmd_ps(),
            "mem" | "free" => self.cmd_mem(),
            "exit" | "quit" => self.cmd_exit(),
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
            "========================================\n\
             Breenix ARM64 Kernel Shell\n\
             ========================================\n\n\
             Commands:\n\
             \n\
             help     - Show this help message\n\
             echo     - Print arguments to terminal\n\
             clear    - Clear the terminal screen\n\
             uptime   - Show time since boot\n\
             uname    - Show system information\n\
             ps       - List running processes\n\
             mem      - Show memory usage\n\
             exit     - (Cannot exit kernel shell)\n\
             \n\
             Note: This is a temporary kernel-mode shell.\n\
             Once ARM64 userspace exec is ready, the full\n\
             init_shell (with ls, cat, cd, etc.) will run here.\n\
             \n\
             Press Ctrl-A X to exit QEMU.\n\
             \n",
        );
    }

    /// Echo command - print arguments
    fn cmd_echo(&self, args: &str) {
        terminal_manager::write_str_to_shell(args);
        terminal_manager::write_str_to_shell("\n");
    }

    /// Clear the terminal
    fn cmd_clear(&self) {
        terminal_manager::clear_shell();
    }

    /// Show system uptime
    fn cmd_uptime(&self) {
        match timer::monotonic_time() {
            Some((secs, nanos)) => {
                let total_secs = secs;
                let hours = total_secs / 3600;
                let mins = (total_secs % 3600) / 60;
                let secs_rem = total_secs % 60;
                let millis = nanos / 1_000_000;

                terminal_manager::write_str_to_shell("up ");
                if hours > 0 {
                    let output = format!("{} hour{}, ", hours, if hours == 1 { "" } else { "s" });
                    terminal_manager::write_str_to_shell(&output);
                }
                if mins > 0 || hours > 0 {
                    let output = format!(
                        "{} minute{}, ",
                        mins,
                        if mins == 1 { "" } else { "s" }
                    );
                    terminal_manager::write_str_to_shell(&output);
                }
                let output = format!(
                    "{}.{:03} second{}\n",
                    secs_rem,
                    millis,
                    if secs_rem == 1 && millis == 0 { "" } else { "s" }
                );
                terminal_manager::write_str_to_shell(&output);
            }
            None => {
                terminal_manager::write_str_to_shell("Error: timer not available\n");
            }
        }
    }

    /// Show system information
    fn cmd_uname(&self) {
        terminal_manager::write_str_to_shell("Breenix 0.1.0 aarch64 ARM Cortex-A72\n");
    }

    /// Show process list
    fn cmd_ps(&self) {
        terminal_manager::write_str_to_shell("  PID  STATE  NAME\n");
        terminal_manager::write_str_to_shell("    0  R      kernel\n");
        terminal_manager::write_str_to_shell("    1  R      shell\n");
    }

    /// Show memory usage
    fn cmd_mem(&self) {
        terminal_manager::write_str_to_shell("Memory usage:\n");
        terminal_manager::write_str_to_shell("  Total RAM:   512 MB (QEMU virt machine)\n");
        terminal_manager::write_str_to_shell("  Kernel heap: 256 KB pre-allocated\n");
        terminal_manager::write_str_to_shell("  Allocator:   bump allocator (ARM64)\n");
    }

    /// Exit command (cannot actually exit kernel shell)
    fn cmd_exit(&self) {
        terminal_manager::write_str_to_shell("Cannot exit kernel shell!\n");
        terminal_manager::write_str_to_shell("Press Ctrl-A X to exit QEMU.\n");
    }
}

impl Default for ShellState {
    fn default() -> Self {
        Self::new()
    }
}
