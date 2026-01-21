//! Terminal emulator for split-screen mode.
//!
//! Provides a terminal emulator that renders shell output to a bounded
//! framebuffer region and routes keyboard input through the TTY layer.

// Public API module - functions are intentionally available for future use
#![allow(dead_code)]

use alloc::sync::Arc;
use spin::Mutex;

use crate::graphics::primitives::Canvas;
use crate::graphics::terminal::TerminalPane;
use crate::tty::pty::PtyPair;

/// Terminal emulator that connects a TerminalPane to a PTY.
///
/// The emulator:
/// - Polls the PTY master for output and renders to the terminal pane
/// - Sends keyboard input to the PTY master
pub struct TerminalEmulator {
    /// PTY pair for shell communication
    pty: Arc<PtyPair>,
    /// Terminal pane for rendering
    pane: TerminalPane,
    /// Cursor blink state
    cursor_visible: bool,
    /// Blink counter for cursor
    blink_counter: u32,
}

impl TerminalEmulator {
    /// Create a new terminal emulator with a PTY.
    ///
    /// # Arguments
    /// * `x` - X position of terminal pane (pixels)
    /// * `y` - Y position of terminal pane (pixels)
    /// * `width` - Width of terminal pane (pixels)
    /// * `height` - Height of terminal pane (pixels)
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Result<Self, &'static str> {
        // Allocate a PTY pair
        let pty = crate::tty::pty::allocate().map_err(|_| "Failed to allocate PTY")?;

        // Unlock the PTY for use
        pty.unlock();

        // Create terminal pane
        let pane = TerminalPane::new(x, y, width, height);

        Ok(Self {
            pty,
            pane,
            cursor_visible: true,
            blink_counter: 0,
        })
    }

    /// Get the PTY number for this terminal.
    pub fn pty_num(&self) -> u32 {
        self.pty.pty_num
    }

    /// Get the slave device path for the PTY.
    pub fn slave_path(&self) -> alloc::string::String {
        self.pty.slave_path()
    }

    /// Get the terminal dimensions in characters.
    pub fn dimensions(&self) -> (usize, usize) {
        (self.pane.cols(), self.pane.rows())
    }

    /// Initialize the terminal (clear and show initial content).
    pub fn init(&mut self, canvas: &mut impl Canvas) {
        self.pane.clear(canvas);
        self.pane.draw_cursor(canvas, true);
    }

    /// Poll for output from the PTY master and render to terminal.
    ///
    /// Returns the number of bytes processed.
    pub fn poll_output(&mut self, canvas: &mut impl Canvas) -> usize {
        let mut buf = [0u8; 256];
        let mut total_read = 0;

        // Read in chunks until no more data available
        loop {
            match self.pty.master_read(&mut buf) {
                Ok(n) if n > 0 => {
                    // Hide cursor while writing
                    self.pane.draw_cursor(canvas, false);

                    // Process output bytes
                    self.pane.write_bytes(canvas, &buf[..n]);
                    total_read += n;

                    // Show cursor after writing
                    self.pane.draw_cursor(canvas, self.cursor_visible);
                }
                _ => break,
            }
        }

        total_read
    }

    /// Send input to the PTY master (from keyboard).
    pub fn send_input(&self, data: &[u8]) -> Result<usize, i32> {
        self.pty.master_write(data)
    }

    /// Send a single character to the PTY master.
    pub fn send_char(&self, c: u8) -> Result<usize, i32> {
        self.pty.master_write(&[c])
    }

    /// Write directly to the terminal pane (bypassing PTY).
    ///
    /// Useful for initial messages or system notifications.
    pub fn write_direct(&mut self, canvas: &mut impl Canvas, s: &str) {
        self.pane.draw_cursor(canvas, false);
        self.pane.write_str(canvas, s);
        self.pane.draw_cursor(canvas, self.cursor_visible);
    }

    /// Toggle cursor visibility (for blinking effect).
    pub fn toggle_cursor(&mut self, canvas: &mut impl Canvas) {
        self.cursor_visible = !self.cursor_visible;
        self.pane.draw_cursor(canvas, self.cursor_visible);
    }

    /// Update blink counter and toggle cursor if needed.
    ///
    /// Call this periodically (e.g., from timer interrupt).
    /// Returns true if cursor was toggled.
    pub fn update_blink(&mut self, canvas: &mut impl Canvas) -> bool {
        self.blink_counter += 1;
        if self.blink_counter >= 50 {
            // Toggle every ~500ms at 100Hz
            self.blink_counter = 0;
            self.toggle_cursor(canvas);
            true
        } else {
            false
        }
    }

    /// Clear the terminal.
    pub fn clear(&mut self, canvas: &mut impl Canvas) {
        self.pane.clear(canvas);
        self.pane.draw_cursor(canvas, self.cursor_visible);
    }

    /// Get mutable access to the terminal pane.
    pub fn pane_mut(&mut self) -> &mut TerminalPane {
        &mut self.pane
    }

    /// Get read access to the terminal pane.
    pub fn pane(&self) -> &TerminalPane {
        &self.pane
    }

    /// Get the PTY pair for direct access.
    pub fn pty(&self) -> &Arc<PtyPair> {
        &self.pty
    }
}

/// Global terminal emulator for split-screen mode.
///
/// This is initialized when split-screen mode is activated.
pub static TERMINAL_EMULATOR: spin::Once<Mutex<TerminalEmulator>> = spin::Once::new();

/// Initialize the global terminal emulator.
pub fn init_terminal(x: usize, y: usize, width: usize, height: usize) -> Result<(), &'static str> {
    let emulator = TerminalEmulator::new(x, y, width, height)?;

    TERMINAL_EMULATOR.call_once(|| Mutex::new(emulator));

    log::info!("Terminal emulator initialized");
    Ok(())
}

/// Get the global terminal emulator.
pub fn terminal() -> Option<&'static Mutex<TerminalEmulator>> {
    TERMINAL_EMULATOR.get()
}

/// Send a character to the terminal emulator (from keyboard interrupt).
///
/// This is called from the keyboard handler in split-screen mode.
pub fn send_keyboard_char(c: u8) {
    if let Some(term) = terminal() {
        if let Some(guard) = term.try_lock() {
            let _ = guard.send_char(c);
        }
    }
}

/// Poll terminal output and render to framebuffer.
///
/// This should be called periodically (e.g., from the async executor).
pub fn poll_terminal_output(canvas: &mut impl Canvas) -> usize {
    if let Some(term) = terminal() {
        if let Some(mut guard) = term.try_lock() {
            return guard.poll_output(canvas);
        }
    }
    0
}
