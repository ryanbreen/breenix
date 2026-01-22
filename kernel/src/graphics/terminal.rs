//! Terminal pane component for split-screen rendering.
//!
//! Provides a bounded text rendering area that can be used for terminal emulation.
//! Supports character-based positioning, scrolling, and basic ANSI escape sequences.

// Public API module - methods are intentionally available for future use
#![allow(dead_code)]

use super::font::{Font, FontMetrics};
use super::primitives::{draw_char, fill_rect, Canvas, Color, Rect, TextStyle};

/// A terminal pane that renders text within a bounded framebuffer region.
///
/// The pane manages a character grid and renders text using the graphics primitives.
/// It supports basic terminal operations like newlines, carriage returns, backspace,
/// and scrolling.
pub struct TerminalPane {
    // Region bounds (pixels)
    x: usize,
    y: usize,
    width: usize,
    height: usize,

    // Character grid dimensions
    cols: usize,
    rows: usize,

    // Cursor position (character coordinates)
    cursor_col: usize,
    cursor_row: usize,

    // Colors
    fg_color: Color,
    bg_color: Color,

    // Font metrics
    font: Font,
    metrics: FontMetrics,

    // ANSI escape sequence parsing state
    ansi_state: AnsiState,
    ansi_params: [u8; 16],
    ansi_param_idx: usize,
}

/// ANSI escape sequence parser state
#[derive(Debug, Clone, Copy, PartialEq)]
enum AnsiState {
    Normal,
    Escape, // Saw ESC (0x1B)
    Csi,    // Saw ESC[
}

impl TerminalPane {
    /// Create a new terminal pane at the specified position and size.
    ///
    /// # Arguments
    /// * `x` - X position in pixels (left edge)
    /// * `y` - Y position in pixels (top edge)
    /// * `width` - Width in pixels
    /// * `height` - Height in pixels
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        let font = Font::default_font();
        let metrics = font.metrics();

        // Calculate character grid dimensions
        let cols = width / metrics.char_advance();
        let rows = height / metrics.line_height();

        Self {
            x,
            y,
            width,
            height,
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            fg_color: Color::WHITE,
            bg_color: Color::rgb(20, 30, 50), // Dark blue background
            font,
            metrics,
            ansi_state: AnsiState::Normal,
            ansi_params: [0; 16],
            ansi_param_idx: 0,
        }
    }

    /// Get the number of columns in the terminal.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Get the number of rows in the terminal.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Set the foreground (text) color.
    pub fn set_fg_color(&mut self, color: Color) {
        self.fg_color = color;
    }

    /// Set the background color.
    pub fn set_bg_color(&mut self, color: Color) {
        self.bg_color = color;
    }

    /// Clear the terminal pane with the background color.
    pub fn clear(&mut self, canvas: &mut impl Canvas) {
        fill_rect(
            canvas,
            Rect {
                x: self.x as i32,
                y: self.y as i32,
                width: self.width as u32,
                height: self.height as u32,
            },
            self.bg_color,
        );
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    /// Convert character coordinates to pixel coordinates.
    fn char_to_pixel(&self, col: usize, row: usize) -> (i32, i32) {
        let px = self.x + col * self.metrics.char_advance();
        let py = self.y + row * self.metrics.line_height();
        (px as i32, py as i32)
    }

    /// Write a single character at the cursor position.
    fn write_char_at_cursor(&mut self, canvas: &mut impl Canvas, c: char) {
        let (px, py) = self.char_to_pixel(self.cursor_col, self.cursor_row);

        let style = TextStyle::new()
            .with_color(self.fg_color)
            .with_background(self.bg_color)
            .with_font(self.font);

        draw_char(canvas, px, py, c, &style);
    }

    /// Clear the character at the cursor position.
    fn clear_char_at_cursor(&mut self, canvas: &mut impl Canvas) {
        let (px, py) = self.char_to_pixel(self.cursor_col, self.cursor_row);

        fill_rect(
            canvas,
            Rect {
                x: px,
                y: py,
                width: self.metrics.char_advance() as u32,
                height: self.metrics.line_height() as u32,
            },
            self.bg_color,
        );
    }

    /// Move cursor to the next line.
    fn newline(&mut self, canvas: &mut impl Canvas) {
        self.cursor_col = 0;
        self.cursor_row += 1;

        // Check if we need to scroll
        if self.cursor_row >= self.rows {
            self.scroll_up(canvas);
            self.cursor_row = self.rows.saturating_sub(1);
        }
    }

    /// Move cursor to the beginning of the current line.
    fn carriage_return(&mut self) {
        self.cursor_col = 0;
    }

    /// Handle backspace - move cursor back and clear the character.
    fn backspace(&mut self, canvas: &mut impl Canvas) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.clear_char_at_cursor(canvas);
        }
    }

    /// Scroll the terminal up by one line.
    pub fn scroll_up(&mut self, canvas: &mut impl Canvas) {
        let line_height = self.metrics.line_height();
        let bytes_per_pixel = canvas.bytes_per_pixel();
        let stride = canvas.stride();

        // Calculate source and destination regions
        let src_y = self.y + line_height;
        let dst_y = self.y;
        let copy_height = self.height.saturating_sub(line_height);

        if copy_height == 0 {
            // Terminal is only one line tall, just clear it
            self.clear(canvas);
            return;
        }

        // Copy each row up using the canvas buffer directly
        let buffer = canvas.buffer_mut();
        let row_bytes = self.width * bytes_per_pixel;

        for row_offset in 0..copy_height {
            let src_row = src_y + row_offset;
            let dst_row = dst_y + row_offset;

            let src_start = src_row * stride * bytes_per_pixel + self.x * bytes_per_pixel;
            let dst_start = dst_row * stride * bytes_per_pixel + self.x * bytes_per_pixel;

            if src_start + row_bytes <= buffer.len() && dst_start + row_bytes <= buffer.len() {
                // Copy the row
                buffer.copy_within(src_start..src_start + row_bytes, dst_start);
            }
        }

        // Mark the ENTIRE terminal region as dirty so it gets flushed to the framebuffer
        // (both the copied region and the cleared line)
        canvas.mark_dirty_region(self.x, self.y, self.width, self.height);

        // Clear the last line
        let clear_y = self.y + copy_height;
        fill_rect(
            canvas,
            Rect {
                x: self.x as i32,
                y: clear_y as i32,
                width: self.width as u32,
                height: line_height as u32,
            },
            self.bg_color,
        );
    }

    /// Write a single character to the terminal, handling control characters.
    pub fn write_char(&mut self, canvas: &mut impl Canvas, c: char) {
        let byte = c as u8;

        match self.ansi_state {
            AnsiState::Normal => {
                if byte == 0x1B {
                    // ESC character - start escape sequence
                    self.ansi_state = AnsiState::Escape;
                    return;
                }
                self.write_char_normal(canvas, byte);
            }
            AnsiState::Escape => {
                if byte == b'[' {
                    // CSI sequence (ESC[)
                    self.ansi_state = AnsiState::Csi;
                    self.ansi_param_idx = 0;
                    self.ansi_params = [0; 16];
                    return;
                }
                // Not a CSI sequence, output ESC and character, return to normal
                self.ansi_state = AnsiState::Normal;
                // Skip outputting raw ESC
                self.write_char_normal(canvas, byte);
            }
            AnsiState::Csi => {
                if byte.is_ascii_digit() {
                    // Accumulate parameter digit
                    if self.ansi_param_idx < 16 {
                        self.ansi_params[self.ansi_param_idx] = self.ansi_params[self.ansi_param_idx]
                            .saturating_mul(10)
                            .saturating_add(byte - b'0');
                    }
                    return;
                }
                if byte == b';' {
                    // Next parameter
                    self.ansi_param_idx = (self.ansi_param_idx + 1).min(15);
                    return;
                }
                // Command byte - execute and return to normal
                self.ansi_state = AnsiState::Normal;
                self.execute_csi(canvas, byte);
            }
        }
    }

    /// Write a normal character (not part of escape sequence).
    fn write_char_normal(&mut self, canvas: &mut impl Canvas, byte: u8) {
        match byte {
            b'\n' => self.newline(canvas),
            b'\r' => self.carriage_return(),
            0x08 | 0x7F => self.backspace(canvas), // Backspace or DEL
            b'\t' => {
                // Tab - advance to next tab stop (every 8 columns)
                let next_tab = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next_tab.min(self.cols.saturating_sub(1));
            }
            c => {
                // Check if we need to wrap
                if self.cursor_col >= self.cols {
                    self.newline(canvas);
                }

                // Write the character
                self.write_char_at_cursor(canvas, c as char);
                self.cursor_col += 1;
            }
        }
    }

    /// Execute a CSI (Control Sequence Introducer) command.
    fn execute_csi(&mut self, canvas: &mut impl Canvas, cmd: u8) {
        let param1 = self.ansi_params[0] as usize;
        let param2 = self.ansi_params[1] as usize;

        match cmd {
            b'J' => {
                // Erase in Display
                match param1 {
                    0 => self.clear_to_end_of_screen(canvas),
                    1 => self.clear_to_start_of_screen(canvas),
                    2 => self.clear(canvas),
                    _ => {}
                }
            }
            b'K' => {
                // Erase in Line
                match param1 {
                    0 => self.clear_to_eol(canvas),
                    1 => self.clear_to_sol(canvas),
                    2 => self.clear_line(canvas),
                    _ => {}
                }
            }
            b'H' | b'f' => {
                // Cursor Position
                // Parameters are 1-based, default to 1
                let row = if param1 == 0 { 0 } else { param1.saturating_sub(1) };
                let col = if param2 == 0 { 0 } else { param2.saturating_sub(1) };
                self.set_cursor(col.min(self.cols.saturating_sub(1)), row.min(self.rows.saturating_sub(1)));
            }
            b'A' => {
                // Cursor Up
                let n = if param1 == 0 { 1 } else { param1 };
                self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            b'B' => {
                // Cursor Down
                let n = if param1 == 0 { 1 } else { param1 };
                self.cursor_row = (self.cursor_row + n).min(self.rows.saturating_sub(1));
            }
            b'C' => {
                // Cursor Forward
                let n = if param1 == 0 { 1 } else { param1 };
                self.cursor_col = (self.cursor_col + n).min(self.cols.saturating_sub(1));
            }
            b'D' => {
                // Cursor Back
                let n = if param1 == 0 { 1 } else { param1 };
                self.cursor_col = self.cursor_col.saturating_sub(n);
            }
            b'm' => {
                // SGR (Select Graphic Rendition)
                self.process_sgr();
            }
            _ => {
                // Unknown command - ignore
            }
        }
    }

    /// Process SGR (color/style) escape sequence.
    fn process_sgr(&mut self) {
        // Process all parameters up to ansi_param_idx
        for i in 0..=self.ansi_param_idx {
            match self.ansi_params[i] {
                0 => {
                    // Reset
                    self.fg_color = Color::WHITE;
                    self.bg_color = Color::rgb(20, 30, 50);
                }
                1 => {
                    // Bold (brighten foreground)
                    self.fg_color = Color::rgb(
                        self.fg_color.r.saturating_add(40),
                        self.fg_color.g.saturating_add(40),
                        self.fg_color.b.saturating_add(40),
                    );
                }
                30 => self.fg_color = Color::BLACK,
                31 => self.fg_color = Color::RED,
                32 => self.fg_color = Color::GREEN,
                33 => self.fg_color = Color::rgb(255, 255, 0), // Yellow
                34 => self.fg_color = Color::BLUE,
                35 => self.fg_color = Color::rgb(255, 0, 255), // Magenta
                36 => self.fg_color = Color::rgb(0, 255, 255), // Cyan
                37 => self.fg_color = Color::WHITE,
                40 => self.bg_color = Color::BLACK,
                41 => self.bg_color = Color::RED,
                42 => self.bg_color = Color::GREEN,
                43 => self.bg_color = Color::rgb(255, 255, 0),
                44 => self.bg_color = Color::BLUE,
                45 => self.bg_color = Color::rgb(255, 0, 255),
                46 => self.bg_color = Color::rgb(0, 255, 255),
                47 => self.bg_color = Color::WHITE,
                _ => {} // Ignore unknown SGR codes
            }
        }
    }

    /// Set cursor position (in character coordinates).
    pub fn set_cursor(&mut self, col: usize, row: usize) {
        self.cursor_col = col.min(self.cols.saturating_sub(1));
        self.cursor_row = row.min(self.rows.saturating_sub(1));
    }

    /// Get current cursor position.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_col, self.cursor_row)
    }

    /// Clear from cursor to end of screen.
    fn clear_to_end_of_screen(&mut self, canvas: &mut impl Canvas) {
        // Clear rest of current line
        self.clear_to_eol(canvas);

        // Clear all lines below
        let start_row = self.cursor_row + 1;
        for row in start_row..self.rows {
            let (px, py) = self.char_to_pixel(0, row);
            fill_rect(
                canvas,
                Rect {
                    x: px,
                    y: py,
                    width: self.width as u32,
                    height: self.metrics.line_height() as u32,
                },
                self.bg_color,
            );
        }
    }

    /// Clear from start of screen to cursor.
    fn clear_to_start_of_screen(&mut self, canvas: &mut impl Canvas) {
        // Clear all lines above
        for row in 0..self.cursor_row {
            let (px, py) = self.char_to_pixel(0, row);
            fill_rect(
                canvas,
                Rect {
                    x: px,
                    y: py,
                    width: self.width as u32,
                    height: self.metrics.line_height() as u32,
                },
                self.bg_color,
            );
        }

        // Clear current line up to cursor
        self.clear_to_sol(canvas);
    }

    /// Clear from cursor to end of line.
    fn clear_to_eol(&mut self, canvas: &mut impl Canvas) {
        let (px, py) = self.char_to_pixel(self.cursor_col, self.cursor_row);
        let clear_width = self.x + self.width - px as usize;

        fill_rect(
            canvas,
            Rect {
                x: px,
                y: py,
                width: clear_width as u32,
                height: self.metrics.line_height() as u32,
            },
            self.bg_color,
        );
    }

    /// Clear from start of line to cursor.
    fn clear_to_sol(&mut self, canvas: &mut impl Canvas) {
        let (_, py) = self.char_to_pixel(0, self.cursor_row);
        let clear_width = (self.cursor_col + 1) * self.metrics.char_advance();

        fill_rect(
            canvas,
            Rect {
                x: self.x as i32,
                y: py,
                width: clear_width as u32,
                height: self.metrics.line_height() as u32,
            },
            self.bg_color,
        );
    }

    /// Clear entire current line.
    fn clear_line(&mut self, canvas: &mut impl Canvas) {
        let (_, py) = self.char_to_pixel(0, self.cursor_row);

        fill_rect(
            canvas,
            Rect {
                x: self.x as i32,
                y: py,
                width: self.width as u32,
                height: self.metrics.line_height() as u32,
            },
            self.bg_color,
        );
    }

    /// Write a string to the terminal.
    pub fn write_str(&mut self, canvas: &mut impl Canvas, s: &str) {
        for c in s.chars() {
            self.write_char(canvas, c);
        }
    }

    /// Write bytes to the terminal (processes ANSI escape sequences).
    pub fn write_bytes(&mut self, canvas: &mut impl Canvas, bytes: &[u8]) {
        for &byte in bytes {
            self.write_char(canvas, byte as char);
        }
    }

    /// Draw a cursor at the current position.
    pub fn draw_cursor(&mut self, canvas: &mut impl Canvas, visible: bool) {
        let (px, py) = self.char_to_pixel(self.cursor_col, self.cursor_row);

        // Draw underscore-style cursor at bottom of character cell
        let cursor_height = 2;
        let cursor_y = py + self.metrics.line_height() as i32 - cursor_height;

        let color = if visible { self.fg_color } else { self.bg_color };

        fill_rect(
            canvas,
            Rect {
                x: px,
                y: cursor_y,
                width: self.metrics.char_advance() as u32,
                height: cursor_height as u32,
            },
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_pane_dimensions() {
        let pane = TerminalPane::new(0, 0, 800, 600);
        // Should have some reasonable number of cols/rows
        assert!(pane.cols() > 0);
        assert!(pane.rows() > 0);
    }

    #[test]
    fn cursor_wrapping() {
        let mut pane = TerminalPane::new(0, 0, 800, 600);
        // Set cursor beyond cols - should stay within bounds
        pane.set_cursor(1000, 0);
        let (col, _) = pane.cursor();
        assert!(col < pane.cols());
    }
}
