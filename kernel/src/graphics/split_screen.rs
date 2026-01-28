//! Split-screen layout manager.
//!
//! Provides a layout manager for creating split-screen views with
//! a graphics pane on the left and a terminal pane on the right.

// Public API module - methods are intentionally available for future use
#![allow(dead_code)]

use super::primitives::{draw_vline, Canvas, Color, Rect};
use super::terminal::TerminalPane;
use spin::Mutex;

// Architecture-specific framebuffer imports
#[cfg(target_arch = "x86_64")]
use crate::logger::SHELL_FRAMEBUFFER;
#[cfg(target_arch = "aarch64")]
use super::arm64_fb::SHELL_FRAMEBUFFER;

/// Global split-screen state.
///
/// When split-screen mode is active, this holds the terminal pane
/// and is used to route output to the right side of the screen.
pub static SPLIT_SCREEN_MODE: Mutex<Option<SplitScreenState>> = Mutex::new(None);

/// Split-screen layout manager.
///
/// Manages a 50/50 horizontal split with:
/// - Left pane: Graphics area (for demo or other visuals)
/// - Right pane: Terminal emulator
pub struct SplitScreen {
    /// Total width of the framebuffer
    total_width: usize,
    /// Total height of the framebuffer
    total_height: usize,
    /// X position of the vertical divider
    divider_x: usize,
    /// Width of the divider in pixels
    divider_width: usize,
    /// Left pane bounds (graphics area)
    left_pane: Rect,
    /// Right pane bounds (terminal area)
    right_pane: Rect,
    /// Terminal pane for the right side
    terminal: TerminalPane,
}

impl SplitScreen {
    /// Create a new split-screen layout.
    ///
    /// # Arguments
    /// * `width` - Total framebuffer width
    /// * `height` - Total framebuffer height
    pub fn new(width: usize, height: usize) -> Self {
        let divider_width = 4;
        let divider_x = width / 2;

        // Left pane: from 0 to divider
        let left_pane = Rect {
            x: 0,
            y: 0,
            width: divider_x as u32,
            height: height as u32,
        };

        // Right pane: from divider to end (with padding)
        let right_start = divider_x + divider_width;
        let right_width = width.saturating_sub(right_start);
        let right_pane = Rect {
            x: right_start as i32,
            y: 0,
            width: right_width as u32,
            height: height as u32,
        };

        // Create terminal pane with some padding
        let terminal_padding = 8;
        let terminal = TerminalPane::new(
            right_start + terminal_padding,
            terminal_padding,
            right_width.saturating_sub(terminal_padding * 2),
            height.saturating_sub(terminal_padding * 2),
        );

        Self {
            total_width: width,
            total_height: height,
            divider_x,
            divider_width,
            left_pane,
            right_pane,
            terminal,
        }
    }

    /// Get the left pane bounds (for graphics rendering).
    pub fn left_pane(&self) -> Rect {
        self.left_pane
    }

    /// Get the right pane bounds (for terminal).
    pub fn right_pane(&self) -> Rect {
        self.right_pane
    }

    /// Get mutable access to the terminal pane.
    pub fn terminal_mut(&mut self) -> &mut TerminalPane {
        &mut self.terminal
    }

    /// Get read access to the terminal pane.
    pub fn terminal(&self) -> &TerminalPane {
        &self.terminal
    }

    /// Draw the vertical divider between panes.
    pub fn draw_divider(&self, canvas: &mut impl Canvas) {
        let divider_color = Color::rgb(60, 80, 100);

        // Draw multiple vertical lines to create the divider
        for i in 0..self.divider_width {
            let x = (self.divider_x + i) as i32;
            draw_vline(canvas, x, 0, self.total_height as i32 - 1, divider_color);
        }
    }

    /// Initialize the split screen (clear both panes and draw divider).
    pub fn init(&mut self, canvas: &mut impl Canvas) {
        // Clear the entire screen first
        use super::primitives::fill_rect;
        fill_rect(
            canvas,
            Rect {
                x: 0,
                y: 0,
                width: self.total_width as u32,
                height: self.total_height as u32,
            },
            Color::rgb(20, 30, 50), // Dark background
        );

        // Draw the divider
        self.draw_divider(canvas);

        // Clear the terminal pane with its background
        self.terminal.clear(canvas);
    }

    /// Get framebuffer dimensions.
    pub fn dimensions(&self) -> (usize, usize) {
        (self.total_width, self.total_height)
    }
}

/// Helper struct for rendering graphics demo within a bounded region.
pub struct ClippedRegion {
    /// Offset X (add to all X coordinates)
    pub offset_x: i32,
    /// Offset Y (add to all Y coordinates)
    pub offset_y: i32,
    /// Maximum width for rendering
    pub width: u32,
    /// Maximum height for rendering
    pub height: u32,
}

impl ClippedRegion {
    /// Create a clipped region from a Rect.
    pub fn from_rect(rect: Rect) -> Self {
        Self {
            offset_x: rect.x,
            offset_y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }

    /// Transform a coordinate for this region.
    pub fn transform(&self, x: i32, y: i32) -> (i32, i32) {
        (x + self.offset_x, y + self.offset_y)
    }

    /// Check if a point is within this region (in transformed coordinates).
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let (tx, ty) = self.transform(x, y);
        tx >= self.offset_x
            && tx < self.offset_x + self.width as i32
            && ty >= self.offset_y
            && ty < self.offset_y + self.height as i32
    }
}

/// State for split-screen mode.
///
/// Contains the terminal pane that receives shell output.
pub struct SplitScreenState {
    /// Terminal pane for the right side
    pub terminal: TerminalPane,
    /// Whether cursor is currently visible (for blinking)
    pub cursor_visible: bool,
}

impl SplitScreenState {
    /// Create a new split-screen state.
    pub fn new(terminal: TerminalPane) -> Self {
        Self {
            terminal,
            cursor_visible: true,
        }
    }
}

/// Check if split-screen mode is active.
pub fn is_split_screen_active() -> bool {
    if let Some(guard) = SPLIT_SCREEN_MODE.try_lock() {
        guard.is_some()
    } else {
        false
    }
}

/// Write a character to the split-screen terminal.
///
/// Returns true if the character was written, false if split-screen is not active
/// or the lock couldn't be acquired.
pub fn write_char_to_terminal(c: char) -> bool {
    if let Some(mut guard) = SPLIT_SCREEN_MODE.try_lock() {
        if let Some(ref mut state) = *guard {
            // Get the framebuffer to render to
            if let Some(fb) = SHELL_FRAMEBUFFER.get() {
                if let Some(mut fb_guard) = fb.try_lock() {
                    // Hide cursor, write char, show cursor
                    state.terminal.draw_cursor(&mut *fb_guard, false);
                    state.terminal.write_char(&mut *fb_guard, c);
                    state.terminal.draw_cursor(&mut *fb_guard, state.cursor_visible);

                    // Flush framebuffer
                    #[cfg(target_arch = "x86_64")]
                    if let Some(db) = fb_guard.double_buffer_mut() {
                        db.flush_if_dirty();
                    }
                    #[cfg(target_arch = "aarch64")]
                    fb_guard.flush();

                    return true;
                }
            }
        }
    }
    false
}

/// Write a string to the split-screen terminal.
///
/// Returns true if written successfully.
pub fn write_str_to_terminal(s: &str) -> bool {
    if let Some(mut guard) = SPLIT_SCREEN_MODE.try_lock() {
        if let Some(ref mut state) = *guard {
            if let Some(fb) = SHELL_FRAMEBUFFER.get() {
                if let Some(mut fb_guard) = fb.try_lock() {
                    state.terminal.draw_cursor(&mut *fb_guard, false);
                    state.terminal.write_str(&mut *fb_guard, s);
                    state.terminal.draw_cursor(&mut *fb_guard, state.cursor_visible);

                    // Flush framebuffer
                    #[cfg(target_arch = "x86_64")]
                    if let Some(db) = fb_guard.double_buffer_mut() {
                        db.flush_if_dirty();
                    }
                    #[cfg(target_arch = "aarch64")]
                    fb_guard.flush();

                    return true;
                }
            }
        }
    }
    false
}

/// Toggle cursor visibility in split-screen terminal.
pub fn toggle_terminal_cursor() {
    if let Some(mut guard) = SPLIT_SCREEN_MODE.try_lock() {
        if let Some(ref mut state) = *guard {
            if let Some(fb) = SHELL_FRAMEBUFFER.get() {
                if let Some(mut fb_guard) = fb.try_lock() {
                    state.cursor_visible = !state.cursor_visible;
                    state.terminal.draw_cursor(&mut *fb_guard, state.cursor_visible);

                    // Flush framebuffer
                    #[cfg(target_arch = "x86_64")]
                    if let Some(db) = fb_guard.double_buffer_mut() {
                        db.flush_if_dirty();
                    }
                    #[cfg(target_arch = "aarch64")]
                    fb_guard.flush();
                }
            }
        }
    }
}

/// Activate split-screen mode with the given terminal pane.
pub fn activate_split_screen(terminal: TerminalPane) {
    let mut guard = SPLIT_SCREEN_MODE.lock();
    *guard = Some(SplitScreenState::new(terminal));
    #[cfg(target_arch = "x86_64")]
    log::info!("Split-screen mode activated");
    #[cfg(target_arch = "aarch64")]
    crate::serial_println!("[split-screen] Split-screen mode activated");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_screen_creation() {
        let split = SplitScreen::new(1920, 1080);
        let (w, h) = split.dimensions();
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
    }

    #[test]
    fn pane_bounds_non_overlapping() {
        let split = SplitScreen::new(1920, 1080);
        let left = split.left_pane();
        let right = split.right_pane();

        // Right pane should start after left pane
        assert!(right.x >= left.x + left.width as i32);
    }

    #[test]
    fn terminal_pane_accessible() {
        let mut split = SplitScreen::new(1920, 1080);
        let terminal = split.terminal_mut();
        assert!(terminal.cols() > 0);
        assert!(terminal.rows() > 0);
    }

    #[test]
    fn clipped_region_transform() {
        let region = ClippedRegion::from_rect(Rect {
            x: 100,
            y: 50,
            width: 400,
            height: 300,
        });

        let (tx, ty) = region.transform(10, 20);
        assert_eq!(tx, 110);
        assert_eq!(ty, 70);
    }
}
