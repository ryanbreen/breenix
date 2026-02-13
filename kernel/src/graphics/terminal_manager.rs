//! Multi-terminal manager with tabbed interface.
//!
//! Provides a terminal manager that supports multiple terminal panes
//! with keyboard-based navigation and a tabbed header UI.

use super::font::Font;
use super::primitives::{draw_text, fill_rect, Canvas, Color, Rect, TextStyle};
use super::terminal::TerminalPane;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// Architecture-specific framebuffer imports
#[cfg(target_arch = "x86_64")]
use crate::logger::SHELL_FRAMEBUFFER;
#[cfg(target_arch = "aarch64")]
use super::arm64_fb::SHELL_FRAMEBUFFER;

/// Tab header height in pixels
const TAB_HEIGHT: usize = 24;

/// Tab padding
const TAB_PADDING: usize = 12;

/// Maximum log lines to keep in buffer
const LOG_BUFFER_SIZE: usize = 1000;

/// Scrollbar width in pixels
const SCROLLBAR_WIDTH: usize = 6;

/// Scrollbar track color
const SCROLLBAR_TRACK_COLOR: Color = Color::rgb(30, 40, 55);

/// Scrollbar thumb color
const SCROLLBAR_THUMB_COLOR: Color = Color::rgb(100, 120, 150);

/// Terminal identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalId {
    Shell = 0,
    Logs = 1,
}

impl TerminalId {
    #[allow(dead_code)]
    pub fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(TerminalId::Shell),
            1 => Some(TerminalId::Logs),
            _ => None,
        }
    }
}

/// Log line buffer for the Logs terminal
struct LogBuffer {
    lines: VecDeque<String>,
    max_lines: usize,
}

impl LogBuffer {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(max_lines),
            max_lines,
        }
    }

    fn push(&mut self, line: String) {
        if self.lines.len() >= self.max_lines {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    #[allow(dead_code)] // Part of LineHistory API for future scrollback
    fn iter(&self) -> impl Iterator<Item = &String> {
        self.lines.iter()
    }
}

/// Multi-terminal manager with tabbed interface
pub struct TerminalManager {
    /// Terminal pane (shared, only active terminal renders here)
    terminal_pane: TerminalPane,
    /// Currently active terminal index
    active_idx: usize,
    /// Region bounds for the terminal area
    region_x: usize,
    region_y: usize,
    region_width: usize,
    region_height: usize,
    /// Cursor visibility state
    cursor_visible: bool,
    /// Font for tab rendering
    font: Font,
    /// Log buffer for the Logs terminal
    log_buffer: LogBuffer,
    /// Tab information
    tab_titles: [&'static str; 2],
    tab_shortcuts: [&'static str; 2],
    /// Unread indicators
    has_unread: [bool; 2],
    /// Scroll offset for Logs tab (0 = following tail, >0 = scrolled up)
    logs_scroll_offset: usize,
    /// Saved pixel data for the shell terminal area (saved when switching away)
    shell_pixel_backup: Option<Vec<u8>>,
    /// Saved cursor position for the shell terminal (col, row)
    shell_cursor_backup: (usize, usize),
}

impl TerminalManager {
    /// Create a new terminal manager.
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        let font = Font::default_font();

        // Calculate terminal pane area (below the tab bar)
        let pane_y = y + TAB_HEIGHT + 2;
        let pane_height = height.saturating_sub(TAB_HEIGHT + 2);
        let pane_padding = 4;

        let terminal_pane = TerminalPane::new(
            x + pane_padding,
            pane_y + pane_padding,
            width.saturating_sub(pane_padding * 2),
            pane_height.saturating_sub(pane_padding * 2),
        );

        Self {
            terminal_pane,
            active_idx: 0,
            region_x: x,
            region_y: y,
            region_width: width,
            region_height: height,
            cursor_visible: true,
            font,
            log_buffer: LogBuffer::new(LOG_BUFFER_SIZE),
            tab_titles: ["Shell", "Logs"],
            tab_shortcuts: ["F1", "F2"],
            has_unread: [false, false],
            logs_scroll_offset: 0,
            shell_pixel_backup: None,
            shell_cursor_backup: (0, 0),
        }
    }

    /// Get the active terminal ID.
    #[allow(dead_code)]
    pub fn active_terminal(&self) -> TerminalId {
        TerminalId::from_index(self.active_idx).unwrap_or(TerminalId::Shell)
    }

    /// Switch to a different terminal.
    pub fn switch_to(&mut self, id: TerminalId, canvas: &mut impl Canvas) {
        let new_idx = id as usize;
        if new_idx >= 2 || new_idx == self.active_idx {
            return;
        }

        // Hide cursor
        self.terminal_pane.draw_cursor(canvas, false);

        // If leaving the Shell tab, save its pixel content and cursor position
        if self.active_idx == TerminalId::Shell as usize {
            self.shell_cursor_backup = self.terminal_pane.cursor();
            self.shell_pixel_backup = Some(self.save_terminal_pixels(canvas));
        }

        self.active_idx = new_idx;
        self.has_unread[new_idx] = false;

        // Clear terminal area and redraw content
        self.clear_terminal_area(canvas);
        self.draw_tab_bar(canvas);

        // Restore content for the new active terminal
        match id {
            TerminalId::Shell => {
                // Restore the saved shell pixel content
                if let Some(ref saved) = self.shell_pixel_backup {
                    self.restore_terminal_pixels(canvas, saved);
                }
                let (col, row) = self.shell_cursor_backup;
                self.terminal_pane.set_cursor(col, row);
            }
            TerminalId::Logs => {
                // Reset scroll to follow tail when switching to Logs
                self.logs_scroll_offset = 0;
                self.render_logs_view(canvas);
            }
        }

        self.terminal_pane.draw_cursor(canvas, self.cursor_visible);
    }

    /// Clear the terminal content area.
    fn clear_terminal_area(&mut self, canvas: &mut impl Canvas) {
        let pane_y = self.region_y + TAB_HEIGHT + 2;
        let pane_height = self.region_height.saturating_sub(TAB_HEIGHT + 2);

        fill_rect(
            canvas,
            Rect {
                x: self.region_x as i32,
                y: pane_y as i32,
                width: self.region_width as u32,
                height: pane_height as u32,
            },
            Color::rgb(20, 30, 50),
        );

        // Reset terminal pane cursor position
        self.terminal_pane.set_cursor(0, 0);
    }

    /// Save the terminal content area pixels to a Vec.
    fn save_terminal_pixels(&self, canvas: &impl Canvas) -> Vec<u8> {
        let pane_y = self.region_y + TAB_HEIGHT + 2;
        let pane_height = self.region_height.saturating_sub(TAB_HEIGHT + 2);
        let bpp = canvas.bytes_per_pixel();
        let stride = canvas.stride();
        let buffer = canvas.buffer();
        let row_bytes = self.region_width * bpp;

        let mut saved = Vec::with_capacity(row_bytes * pane_height);
        for row in 0..pane_height {
            let offset = (pane_y + row) * stride * bpp + self.region_x * bpp;
            if offset + row_bytes <= buffer.len() {
                saved.extend_from_slice(&buffer[offset..offset + row_bytes]);
            }
        }
        saved
    }

    /// Restore previously saved terminal content area pixels.
    fn restore_terminal_pixels(&self, canvas: &mut impl Canvas, saved: &[u8]) {
        let pane_y = self.region_y + TAB_HEIGHT + 2;
        let pane_height = self.region_height.saturating_sub(TAB_HEIGHT + 2);
        let bpp = canvas.bytes_per_pixel();
        let stride = canvas.stride();
        let row_bytes = self.region_width * bpp;

        let buffer = canvas.buffer_mut();
        let mut src_offset = 0;
        for row in 0..pane_height {
            let dst_offset = (pane_y + row) * stride * bpp + self.region_x * bpp;
            if dst_offset + row_bytes <= buffer.len() && src_offset + row_bytes <= saved.len() {
                buffer[dst_offset..dst_offset + row_bytes]
                    .copy_from_slice(&saved[src_offset..src_offset + row_bytes]);
            }
            src_offset += row_bytes;
        }
        canvas.mark_dirty_region(self.region_x, pane_y, self.region_width, pane_height);
    }

    /// Initialize the terminal manager.
    pub fn init(&mut self, canvas: &mut impl Canvas) {
        // Clear entire region
        fill_rect(
            canvas,
            Rect {
                x: self.region_x as i32,
                y: self.region_y as i32,
                width: self.region_width as u32,
                height: self.region_height as u32,
            },
            Color::rgb(20, 30, 50),
        );

        self.draw_tab_bar(canvas);

        // Write shell welcome message
        self.terminal_pane.write_str(canvas, "Breenix Shell\r\n");
        self.terminal_pane.write_str(canvas, "=============\r\n\r\n");
        self.terminal_pane.write_str(canvas, "Press F1 for Shell, F2 for Logs\r\n\r\n");
        self.terminal_pane.draw_cursor(canvas, true);
    }

    /// Draw the tab bar.
    fn draw_tab_bar(&self, canvas: &mut impl Canvas) {
        let metrics = self.font.metrics();

        // Tab bar background
        fill_rect(
            canvas,
            Rect {
                x: self.region_x as i32,
                y: self.region_y as i32,
                width: self.region_width as u32,
                height: TAB_HEIGHT as u32,
            },
            Color::rgb(40, 50, 70),
        );

        // Draw tabs
        let mut tab_x = self.region_x + 4;
        for idx in 0..2 {
            let is_active = idx == self.active_idx;
            let title = self.tab_titles[idx];
            let shortcut = self.tab_shortcuts[idx];

            let title_width = title.len() * metrics.char_advance();
            let shortcut_width = (shortcut.len() + 2) * metrics.char_advance();
            let tab_width = title_width + shortcut_width + TAB_PADDING * 2;

            // Tab background
            let bg_color = if is_active {
                Color::rgb(60, 80, 120)
            } else if self.has_unread[idx] {
                Color::rgb(80, 60, 60)
            } else {
                Color::rgb(30, 40, 55)
            };

            fill_rect(
                canvas,
                Rect {
                    x: tab_x as i32,
                    y: (self.region_y + 2) as i32,
                    width: tab_width as u32,
                    height: (TAB_HEIGHT - 4) as u32,
                },
                bg_color,
            );

            // Title
            let title_style = TextStyle::new()
                .with_color(if is_active {
                    Color::WHITE
                } else {
                    Color::rgb(180, 180, 180)
                })
                .with_font(self.font);

            let text_y = self.region_y + (TAB_HEIGHT - metrics.line_height()) / 2;
            draw_text(
                canvas,
                (tab_x + TAB_PADDING / 2) as i32,
                text_y as i32,
                title,
                &title_style,
            );

            // Shortcut
            let shortcut_style = TextStyle::new()
                .with_color(Color::rgb(120, 140, 160))
                .with_font(self.font);

            let shortcut_text = alloc::format!("[{}]", shortcut);
            draw_text(
                canvas,
                (tab_x + TAB_PADDING / 2 + title_width + 4) as i32,
                text_y as i32,
                &shortcut_text,
                &shortcut_style,
            );

            // Unread dot
            if self.has_unread[idx] && !is_active {
                fill_rect(
                    canvas,
                    Rect {
                        x: (tab_x + tab_width - 8) as i32,
                        y: (self.region_y + 6) as i32,
                        width: 4,
                        height: 4,
                    },
                    Color::rgb(255, 100, 100),
                );
            }

            tab_x += tab_width + 4;
        }

        // Separator line
        fill_rect(
            canvas,
            Rect {
                x: self.region_x as i32,
                y: (self.region_y + TAB_HEIGHT) as i32,
                width: self.region_width as u32,
                height: 2,
            },
            Color::rgb(60, 80, 100),
        );
    }

    /// Write a character to the shell terminal.
    /// Only renders if Shell is the active terminal.
    pub fn write_char_to_shell(&mut self, canvas: &mut impl Canvas, c: char) {
        if self.active_idx == TerminalId::Shell as usize {
            self.terminal_pane.draw_cursor(canvas, false);
            self.terminal_pane.write_char(canvas, c);
            self.terminal_pane.draw_cursor(canvas, self.cursor_visible);
        }
        // If shell is not active, characters are lost (shell will redraw on switch)
    }

    /// Write a string to the shell terminal.
    #[allow(dead_code)]
    pub fn write_str_to_shell(&mut self, canvas: &mut impl Canvas, s: &str) {
        if self.active_idx == TerminalId::Shell as usize {
            self.terminal_pane.draw_cursor(canvas, false);
            self.terminal_pane.write_str(canvas, s);
            self.terminal_pane.draw_cursor(canvas, self.cursor_visible);
        }
    }

    /// Write bytes to the shell terminal (batched version for efficient output).
    ///
    /// This is more efficient than calling write_char_to_shell per character
    /// as it hides/shows cursor once for the entire batch.
    pub fn write_bytes_to_shell(&mut self, canvas: &mut impl Canvas, bytes: &[u8]) {
        if self.active_idx == TerminalId::Shell as usize {
            // Hide cursor ONCE at the start
            self.terminal_pane.draw_cursor(canvas, false);
            // Write all bytes
            self.terminal_pane.write_bytes(canvas, bytes);
            // Show cursor ONCE at the end
            self.terminal_pane.draw_cursor(canvas, self.cursor_visible);
        }
    }

    /// Add a log line to the logs buffer and display if Logs is active.
    pub fn add_log_line(&mut self, canvas: &mut impl Canvas, line: &str) {
        // Store in buffer
        self.log_buffer.push(String::from(line));

        // Display if Logs is active
        if self.active_idx == TerminalId::Logs as usize {
            if self.logs_scroll_offset == 0 {
                // Following tail — render the new line at bottom
                self.terminal_pane.draw_cursor(canvas, false);
                self.terminal_pane.write_str(canvas, line);
                self.terminal_pane.write_str(canvas, "\r\n");
                self.terminal_pane.draw_cursor(canvas, self.cursor_visible);
                // Redraw scrollbar since total lines changed
                self.draw_scrollbar(canvas);
            } else {
                // User has scrolled up — keep view frozen, bump offset
                // so they stay on the same lines
                self.logs_scroll_offset += 1;
            }
        } else {
            // Mark as unread
            if !self.has_unread[TerminalId::Logs as usize] {
                self.has_unread[TerminalId::Logs as usize] = true;
                self.draw_tab_bar(canvas);
            }
        }
    }

    /// Render the full Logs view based on scroll_offset.
    ///
    /// Clears the terminal area, renders the visible window of log lines,
    /// and draws the scrollbar.
    fn render_logs_view(&mut self, canvas: &mut impl Canvas) {
        // Clear terminal content area
        self.clear_terminal_area(canvas);

        let visible_rows = self.terminal_pane.rows();
        let total_lines = self.log_buffer.lines.len();

        if total_lines == 0 {
            self.draw_scrollbar(canvas);
            return;
        }

        // Calculate the range of lines to display
        // scroll_offset == 0 means show the last `visible_rows` lines (tail)
        // scroll_offset == N means the bottom visible line is N lines above the tail
        let end = total_lines.saturating_sub(self.logs_scroll_offset);
        let start = end.saturating_sub(visible_rows);

        for line in self.log_buffer.lines.iter().skip(start).take(end - start) {
            self.terminal_pane.write_str(canvas, line);
            self.terminal_pane.write_str(canvas, "\r\n");
        }

        self.draw_scrollbar(canvas);
    }

    /// Scroll the logs view up (toward older lines).
    pub fn scroll_logs_up(&mut self, canvas: &mut impl Canvas) {
        let visible_rows = self.terminal_pane.rows();
        let total_lines = self.log_buffer.lines.len();
        let max_offset = total_lines.saturating_sub(visible_rows);

        if self.logs_scroll_offset < max_offset {
            self.logs_scroll_offset += 1;
            self.render_logs_view(canvas);
        }
    }

    /// Scroll the logs view down (toward newer lines).
    pub fn scroll_logs_down(&mut self, canvas: &mut impl Canvas) {
        if self.logs_scroll_offset > 0 {
            self.logs_scroll_offset -= 1;
            self.render_logs_view(canvas);
        }
    }

    /// Draw a scrollbar on the right edge of the terminal area.
    fn draw_scrollbar(&self, canvas: &mut impl Canvas) {
        let pane_y = self.region_y + TAB_HEIGHT + 2;
        let pane_height = self.region_height.saturating_sub(TAB_HEIGHT + 2);

        let track_x = self.region_x + self.region_width - SCROLLBAR_WIDTH;

        // Draw track
        fill_rect(
            canvas,
            Rect {
                x: track_x as i32,
                y: pane_y as i32,
                width: SCROLLBAR_WIDTH as u32,
                height: pane_height as u32,
            },
            SCROLLBAR_TRACK_COLOR,
        );

        let total_lines = self.log_buffer.lines.len();
        let visible_rows = self.terminal_pane.rows();

        if total_lines <= visible_rows {
            // Everything fits — no thumb needed (or fill the whole track)
            return;
        }

        // Thumb height: proportional to visible / total
        let thumb_height = ((visible_rows as u64 * pane_height as u64) / total_lines as u64)
            .max(8) as usize; // minimum 8px

        // Thumb position: based on scroll_offset
        // scroll_offset == 0 → thumb at bottom
        // scroll_offset == max → thumb at top
        let max_offset = total_lines.saturating_sub(visible_rows);
        let scroll_range = pane_height.saturating_sub(thumb_height);
        let thumb_y = if max_offset > 0 {
            pane_y + (self.logs_scroll_offset as u64 * scroll_range as u64 / max_offset as u64) as usize
        } else {
            pane_y
        };
        // Invert: scroll_offset 0 = bottom, max = top
        let thumb_y_inverted = pane_y + scroll_range - (thumb_y - pane_y);

        fill_rect(
            canvas,
            Rect {
                x: track_x as i32,
                y: thumb_y_inverted as i32,
                width: SCROLLBAR_WIDTH as u32,
                height: thumb_height as u32,
            },
            SCROLLBAR_THUMB_COLOR,
        );
    }

    /// Toggle cursor visibility.
    pub fn toggle_cursor(&mut self, canvas: &mut impl Canvas) {
        self.cursor_visible = !self.cursor_visible;
        self.terminal_pane.draw_cursor(canvas, self.cursor_visible);
    }

    /// Get terminal dimensions.
    #[allow(dead_code)]
    pub fn dimensions(&self) -> (usize, usize) {
        (self.terminal_pane.cols(), self.terminal_pane.rows())
    }
}

/// Global terminal manager
pub static TERMINAL_MANAGER: Mutex<Option<TerminalManager>> = Mutex::new(None);

/// Flag to prevent recursive calls
static IN_TERMINAL_CALL: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Initialize the terminal manager.
pub fn init_terminal_manager(x: usize, y: usize, width: usize, height: usize) {
    let manager = TerminalManager::new(x, y, width, height);
    *TERMINAL_MANAGER.lock() = Some(manager);
}

/// Check if terminal manager is active.
pub fn is_terminal_manager_active() -> bool {
    if let Some(guard) = TERMINAL_MANAGER.try_lock() {
        guard.is_some()
    } else {
        false
    }
}

/// Write a character to the shell terminal.
pub fn write_char_to_shell(c: char) -> bool {
    // Prevent recursive calls
    if IN_TERMINAL_CALL.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return false;
    }

    let result = (|| {
        let mut guard = TERMINAL_MANAGER.try_lock()?;
        let manager = guard.as_mut()?;
        let fb = SHELL_FRAMEBUFFER.get()?;
        let mut fb_guard = fb.try_lock()?;

        manager.write_char_to_shell(&mut *fb_guard, c);

        // Flush framebuffer
        #[cfg(target_arch = "x86_64")]
        if let Some(db) = fb_guard.double_buffer_mut() {
            db.flush_if_dirty();
        }
        #[cfg(target_arch = "aarch64")]
        fb_guard.flush();
        Some(())
    })()
    .is_some();

    IN_TERMINAL_CALL.store(false, core::sync::atomic::Ordering::SeqCst);
    result
}

/// Write a string to the shell terminal.
#[allow(dead_code)]
pub fn write_str_to_shell(s: &str) -> bool {
    if IN_TERMINAL_CALL.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return false;
    }

    let result = (|| {
        let mut guard = TERMINAL_MANAGER.try_lock()?;
        let manager = guard.as_mut()?;
        let fb = SHELL_FRAMEBUFFER.get()?;
        let mut fb_guard = fb.try_lock()?;

        manager.write_str_to_shell(&mut *fb_guard, s);

        // Flush framebuffer
        #[cfg(target_arch = "x86_64")]
        if let Some(db) = fb_guard.double_buffer_mut() {
            db.flush_if_dirty();
        }
        #[cfg(target_arch = "aarch64")]
        fb_guard.flush();
        Some(())
    })()
    .is_some();

    IN_TERMINAL_CALL.store(false, core::sync::atomic::Ordering::SeqCst);
    result
}

/// Write bytes to the shell terminal (batched version for efficient output).
///
/// This is more efficient than calling write_char_to_shell per character
/// as it acquires locks once for the entire buffer and batches cursor operations.
#[allow(dead_code)]
pub fn write_bytes_to_shell(bytes: &[u8]) -> bool {
    if !write_bytes_to_shell_internal(bytes) {
        return false;
    }

    // Flush after writing
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        #[cfg(target_arch = "x86_64")]
        if let Some(mut fb_guard) = fb.try_lock() {
            if let Some(db) = fb_guard.double_buffer_mut() {
                db.flush_if_dirty();
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            // ARM64 VirtIO GPU requires explicit flush to display changes.
            // Use blocking lock to ensure flush always happens - this is critical
            // for prompts and other output that don't end in newline.
            let fb_guard = fb.lock();
            fb_guard.flush();
        }
    }
    true
}

/// Write bytes to the shell terminal without flushing.
///
/// Internal version for use by render thread which handles its own flushing.
/// This avoids double-flushing when the render thread batches work.
pub fn write_bytes_to_shell_internal(bytes: &[u8]) -> bool {
    if IN_TERMINAL_CALL.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return false;
    }

    let result = (|| {
        let mut guard = TERMINAL_MANAGER.try_lock()?;
        let manager = guard.as_mut()?;
        let fb = SHELL_FRAMEBUFFER.get()?;
        let mut fb_guard = fb.try_lock()?;

        // Use the batched method which hides cursor once, writes all, shows cursor once
        manager.write_bytes_to_shell(&mut *fb_guard, bytes);

        Some(())
    })()
    .is_some();

    IN_TERMINAL_CALL.store(false, core::sync::atomic::Ordering::SeqCst);
    result
}

/// Write bytes to the shell terminal using blocking locks.
///
/// This version waits for locks rather than failing on contention.
/// Safe to call from render thread context (not interrupt context).
/// Does not flush - caller is responsible for flushing.
///
/// Note: This does NOT check IN_TERMINAL_CALL because the render thread
/// is a separate thread that can safely wait for locks. The IN_TERMINAL_CALL
/// guard is for preventing recursion within the same thread.
pub fn write_bytes_to_shell_blocking(bytes: &[u8]) -> bool {
    // Use blocking locks - this is safe because render thread is not in interrupt context
    let mut guard = TERMINAL_MANAGER.lock();
    let manager = match guard.as_mut() {
        Some(m) => m,
        None => return false,
    };

    let fb = match SHELL_FRAMEBUFFER.get() {
        Some(f) => f,
        None => return false,
    };

    let mut fb_guard = fb.lock();

    // Use the batched method which hides cursor once, writes all, shows cursor once
    manager.write_bytes_to_shell(&mut *fb_guard, bytes);

    true
}

/// Add a log line to the logs terminal.
pub fn write_str_to_logs(s: &str) -> bool {
    if IN_TERMINAL_CALL.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return false;
    }

    let result = (|| {
        let mut guard = TERMINAL_MANAGER.try_lock()?;
        let manager = guard.as_mut()?;
        let fb = SHELL_FRAMEBUFFER.get()?;
        let mut fb_guard = fb.try_lock()?;

        // Remove trailing \r\n since add_log_line adds it
        let line = s.trim_end_matches('\n').trim_end_matches('\r');
        manager.add_log_line(&mut *fb_guard, line);

        // Flush framebuffer
        #[cfg(target_arch = "x86_64")]
        if let Some(db) = fb_guard.double_buffer_mut() {
            db.flush_if_dirty();
        }
        #[cfg(target_arch = "aarch64")]
        fb_guard.flush();
        Some(())
    })()
    .is_some();

    IN_TERMINAL_CALL.store(false, core::sync::atomic::Ordering::SeqCst);
    result
}

/// Add a log line to the logs terminal using blocking locks.
///
/// Safe to call from the render thread (not interrupt context).
/// Does not flush — caller is responsible for flushing.
pub fn write_str_to_logs_blocking(s: &str) -> bool {
    let mut guard = TERMINAL_MANAGER.lock();
    let manager = match guard.as_mut() {
        Some(m) => m,
        None => return false,
    };

    let fb = match SHELL_FRAMEBUFFER.get() {
        Some(f) => f,
        None => return false,
    };

    let mut fb_guard = fb.lock();

    let line = s.trim_end_matches('\n').trim_end_matches('\r');
    manager.add_log_line(&mut *fb_guard, line);

    true
}

/// Handle UP/DOWN arrow keys for log scrolling.
///
/// Called from the input interrupt handler. Returns true if the key was
/// consumed (i.e., Logs tab is active and scrolling was performed).
///
/// Linux evdev keycodes: UP=103, DOWN=108
pub fn handle_logs_arrow_key(keycode: u8) -> bool {
    // Only handle when Logs tab is active
    let mut guard = match TERMINAL_MANAGER.try_lock() {
        Some(g) => g,
        None => return false,
    };
    let manager = match guard.as_mut() {
        Some(m) => m,
        None => return false,
    };

    if manager.active_idx != TerminalId::Logs as usize {
        return false;
    }

    let fb = match SHELL_FRAMEBUFFER.get() {
        Some(f) => f,
        None => return false,
    };
    let mut fb_guard = match fb.try_lock() {
        Some(g) => g,
        None => return false,
    };

    match keycode {
        103 => manager.scroll_logs_up(&mut *fb_guard),   // UP
        108 => manager.scroll_logs_down(&mut *fb_guard), // DOWN
        _ => return false,
    }

    #[cfg(target_arch = "aarch64")]
    fb_guard.flush();
    #[cfg(target_arch = "x86_64")]
    if let Some(db) = fb_guard.double_buffer_mut() {
        db.flush_if_dirty();
    }

    true
}

/// Toggle cursor in active terminal.
pub fn toggle_cursor() {
    if IN_TERMINAL_CALL.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let _ = (|| {
        let mut guard = TERMINAL_MANAGER.try_lock()?;
        let manager = guard.as_mut()?;
        let fb = SHELL_FRAMEBUFFER.get()?;
        let mut fb_guard = fb.try_lock()?;

        manager.toggle_cursor(&mut *fb_guard);

        // Flush framebuffer
        #[cfg(target_arch = "x86_64")]
        if let Some(db) = fb_guard.double_buffer_mut() {
            db.flush_if_dirty();
        }
        #[cfg(target_arch = "aarch64")]
        fb_guard.flush();
        Some(())
    })();

    IN_TERMINAL_CALL.store(false, core::sync::atomic::Ordering::SeqCst);
}

/// Switch to a specific terminal.
pub fn switch_terminal(id: TerminalId) {
    if IN_TERMINAL_CALL.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let _ = (|| {
        let mut guard = TERMINAL_MANAGER.try_lock()?;
        let manager = guard.as_mut()?;
        let fb = SHELL_FRAMEBUFFER.get()?;
        let mut fb_guard = fb.try_lock()?;

        manager.switch_to(id, &mut *fb_guard);

        // Flush framebuffer
        #[cfg(target_arch = "x86_64")]
        if let Some(db) = fb_guard.double_buffer_mut() {
            // Only flush dirty regions, not entire 8MB buffer
            db.flush();
        }
        #[cfg(target_arch = "aarch64")]
        fb_guard.flush();

        Some(())
    })();

    IN_TERMINAL_CALL.store(false, core::sync::atomic::Ordering::SeqCst);
}

/// Clear the shell terminal.
pub fn clear_shell() {
    if IN_TERMINAL_CALL.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let _ = (|| {
        let mut guard = TERMINAL_MANAGER.try_lock()?;
        let manager = guard.as_mut()?;
        let fb = SHELL_FRAMEBUFFER.get()?;
        let mut fb_guard = fb.try_lock()?;

        // Only clear if shell is the active terminal
        if manager.active_idx == TerminalId::Shell as usize {
            manager.clear_terminal_area(&mut *fb_guard);
            manager.draw_tab_bar(&mut *fb_guard);
            manager.terminal_pane.draw_cursor(&mut *fb_guard, manager.cursor_visible);
        }

        // Flush framebuffer
        #[cfg(target_arch = "x86_64")]
        if let Some(db) = fb_guard.double_buffer_mut() {
            db.flush_if_dirty();
        }
        #[cfg(target_arch = "aarch64")]
        fb_guard.flush();
        Some(())
    })();

    IN_TERMINAL_CALL.store(false, core::sync::atomic::Ordering::SeqCst);
}

/// Handle a mouse click for tab switching.
///
/// Called from the tablet interrupt handler when BTN_LEFT is pressed.
/// Uses try_lock to remain safe in interrupt context. Returns true
/// if the click was in the tab bar and a terminal switch occurred.
pub fn handle_mouse_click(x: usize, y: usize) -> bool {
    let mut guard = match TERMINAL_MANAGER.try_lock() {
        Some(g) => g,
        None => return false,
    };
    let manager = match guard.as_mut() {
        Some(m) => m,
        None => return false,
    };

    // Check if click is in the tab bar region
    if y < manager.region_y || y >= manager.region_y + TAB_HEIGHT {
        return false;
    }

    // Hit-test tabs (same layout as draw_tab_bar)
    let metrics = manager.font.metrics();
    let mut tab_x = manager.region_x + 4;
    for idx in 0..2 {
        let title = manager.tab_titles[idx];
        let shortcut = manager.tab_shortcuts[idx];
        let title_width = title.len() * metrics.char_advance();
        let shortcut_width = (shortcut.len() + 2) * metrics.char_advance();
        let tab_width = title_width + shortcut_width + TAB_PADDING * 2;

        if x >= tab_x && x < tab_x + tab_width {
            // Clicked on this tab
            if idx != manager.active_idx {
                drop(guard); // Release lock before calling switch_terminal
                let id = if idx == 0 { TerminalId::Shell } else { TerminalId::Logs };
                switch_terminal(id);
                return true;
            }
            return false; // Already on this tab
        }

        tab_x += tab_width + 4;
    }

    false
}

/// Handle keyboard input for terminal switching.
/// Returns true if the key was handled.
pub fn handle_terminal_key(scancode: u8) -> bool {
    match scancode {
        0x3B => {
            switch_terminal(TerminalId::Shell);
            true
        }
        0x3C => {
            switch_terminal(TerminalId::Logs);
            true
        }
        _ => false,
    }
}
