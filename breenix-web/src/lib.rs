//! Breenix Web — a working Breenix terminal rendered in the browser via WebAssembly.
//!
//! Uses the same graphics, TTY, VFS, and IPC code that runs on the real kernel.
//! The shell executes commands against a real POSIX infrastructure (RamFs,
//! FdTable, PipeBuffer) — not toy reimplementations.

extern crate alloc;

mod block_mem;
mod canvas;
mod commands;
mod hal;
pub mod kernel;
mod shell;

use breenix_core::graphics::primitives::Canvas;
use breenix_core::graphics::terminal::TerminalPane;
use breenix_core::tty::line_discipline::LineDiscipline;
use canvas::WasmCanvas;
use kernel::WasmKernel;
use wasm_bindgen::prelude::*;

/// The main terminal instance exported to JavaScript.
#[wasm_bindgen]
pub struct BreenixTerminal {
    canvas: WasmCanvas,
    terminal: TerminalPane,
    line_disc: LineDiscipline,
    kernel: WasmKernel,
    /// Bytes echoed by the line discipline, waiting to be written to the terminal pane.
    echo_buf: Vec<u8>,
}

#[wasm_bindgen]
impl BreenixTerminal {
    /// Create a new terminal with the given pixel dimensions.
    ///
    /// Boots the kernel, populates the filesystem, and writes the boot banner.
    #[wasm_bindgen(constructor)]
    pub fn new(width: usize, height: usize) -> Self {
        let canvas = WasmCanvas::new(width, height);
        let terminal = TerminalPane::new(0, 0, width, height);
        let line_disc = LineDiscipline::new();
        let kernel = WasmKernel::new();

        let mut term = Self {
            canvas,
            terminal,
            line_disc,
            kernel,
            echo_buf: Vec::new(),
        };

        term.boot();
        term
    }

    /// Write the boot banner and initial shell prompt.
    fn boot(&mut self) {
        self.terminal.clear(&mut self.canvas);

        self.terminal.write_str(
            &mut self.canvas,
            "\x1b[32m  ____  ____  _____ _____ _   _ _____  __\r\n",
        );
        self.terminal.write_str(
            &mut self.canvas,
            " | __ )|  _ \\| ____| ____| \\ | |_ _\\ \\/ /\r\n",
        );
        self.terminal.write_str(
            &mut self.canvas,
            " |  _ \\| |_) |  _| |  _| |  \\| || | \\  / \r\n",
        );
        self.terminal.write_str(
            &mut self.canvas,
            " | |_) |  _ <| |___| |___| |\\  || | /  \\ \r\n",
        );
        self.terminal.write_str(
            &mut self.canvas,
            " |____/|_| \\_\\_____|_____|_| \\_|___/_/\\_\\\r\n\x1b[0m",
        );
        self.terminal.write_str(&mut self.canvas, "\r\n");
        self.terminal.write_str(
            &mut self.canvas,
            "\x1b[36m  Breenix OS v0.1.0 \u{2014} WebAssembly Terminal\x1b[0m\r\n",
        );
        self.terminal.write_str(&mut self.canvas, "\r\n");
        self.terminal.write_str(
            &mut self.canvas,
            "  Real kernel VFS, FD table, and pipe infrastructure.\r\n",
        );
        self.terminal.write_str(
            &mut self.canvas,
            "  Type 'help' for available commands.\r\n",
        );
        self.terminal.write_str(&mut self.canvas, "\r\n");

        self.write_prompt();
    }

    /// Write the shell prompt.
    fn write_prompt(&mut self) {
        let cwd = self.kernel.sys_getcwd();
        let prompt = alloc::format!("\x1b[33mroot@breenix:{}\x1b[0m# ", cwd);
        self.terminal.write_str(&mut self.canvas, &prompt);
    }

    /// Width of the pixel buffer.
    pub fn width(&self) -> usize {
        self.canvas.width()
    }

    /// Height of the pixel buffer.
    pub fn height(&self) -> usize {
        self.canvas.height()
    }

    /// Number of character columns.
    pub fn cols(&self) -> usize {
        self.terminal.cols()
    }

    /// Number of character rows.
    pub fn rows(&self) -> usize {
        self.terminal.rows()
    }

    /// Write a string to the terminal (supports ANSI escape sequences).
    /// This is the "output" path — like a program printing to stdout.
    pub fn write_str(&mut self, s: &str) {
        self.terminal.write_str(&mut self.canvas, s);
    }

    /// Process a keyboard event.
    /// `key` is the character (e.g., "a", "Enter", "Backspace").
    /// Returns true if the display needs to be redrawn.
    pub fn key_input(&mut self, key: &str, ctrl: bool, _shift: bool) -> bool {
        let byte: Option<u8> = if ctrl && key.len() == 1 {
            let ch = key.as_bytes()[0];
            if ch.is_ascii_alphabetic() {
                Some(ch.to_ascii_uppercase() - b'A' + 1)
            } else {
                None
            }
        } else {
            match key {
                "Enter" => Some(b'\r'),
                "Backspace" => Some(0x7F),
                "Tab" => Some(b'\t'),
                "Escape" => Some(0x1B),
                _ if key.len() == 1 => Some(key.as_bytes()[0]),
                _ => None,
            }
        };

        let Some(byte) = byte else {
            return false;
        };

        // Feed through the line discipline
        self.echo_buf.clear();
        let echo_buf = &mut self.echo_buf as *mut Vec<u8>;
        let signal = self.line_disc.input_char(byte, &mut |c| {
            // SAFETY: We know echo_buf isn't borrowed elsewhere during this callback.
            unsafe { (*echo_buf).push(c) };
        });

        // Write echoed characters to the terminal pane
        for &c in &self.echo_buf {
            self.terminal.write_char(&mut self.canvas, c as char);
        }

        // Check if a line is ready to read
        if self.line_disc.has_data() {
            let mut buf = [0u8; 4096];
            if let Ok(n) = self.line_disc.read(&mut buf) {
                if n > 0 {
                    if let Ok(line) = core::str::from_utf8(&buf[..n]) {
                        self.execute_line(line);
                    }
                }
            }
        }

        // If a signal was generated (Ctrl+C), print ^C and show new prompt
        if signal.is_some() {
            self.line_disc.flush_input();
            self.terminal.write_str(&mut self.canvas, "^C\r\n");
            self.write_prompt();
        }

        true
    }

    /// Execute a command line through the shell and display the result.
    fn execute_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let (output, _exit_code) = shell::execute(&mut self.kernel, trimmed);
            if !output.is_empty() {
                // Convert \n to \r\n for the terminal
                for line in output.split('\n') {
                    if !line.is_empty() {
                        self.terminal.write_str(&mut self.canvas, line);
                    }
                    self.terminal.write_str(&mut self.canvas, "\r\n");
                }
                // Remove the trailing extra \r\n from the split
                // (split produces an empty last element if string ends with \n)
            }

            // Also drain any kernel output buffer (from sys_write to stdout/stderr)
            let kernel_output = self.kernel.take_output();
            if !kernel_output.is_empty() {
                if let Ok(s) = core::str::from_utf8(&kernel_output) {
                    for line in s.split('\n') {
                        if !line.is_empty() {
                            self.terminal.write_str(&mut self.canvas, line);
                        }
                        self.terminal.write_str(&mut self.canvas, "\r\n");
                    }
                }
            }
        }
        self.write_prompt();
    }

    /// Get a pointer to the RGBA pixel buffer (for ImageData construction).
    pub fn buffer_ptr(&self) -> *const u8 {
        self.canvas.buffer_ptr()
    }

    /// Get the length of the pixel buffer.
    pub fn buffer_len(&self) -> usize {
        self.canvas.buffer_len()
    }

    /// Clear the terminal display.
    pub fn clear(&mut self) {
        self.terminal.clear(&mut self.canvas);
    }

    /// Draw the cursor at its current position.
    pub fn draw_cursor(&mut self, visible: bool) {
        self.terminal.draw_cursor(&mut self.canvas, visible);
    }
}
