//! Multi-terminal manager with tabbed interface.
//!
//! Provides a terminal manager that supports multiple terminal panes
//! with keyboard-based navigation and a tabbed header UI.

use super::font::Font;
use super::primitives::{draw_text, fill_rect, Canvas, Color, Rect, TextStyle};
use super::terminal::TerminalPane;
use alloc::collections::VecDeque;
use alloc::string::String;
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
const LOG_BUFFER_SIZE: usize = 50; // Reduced from 200 for faster tab switching

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

        self.active_idx = new_idx;
        self.has_unread[new_idx] = false;

        // Clear terminal area and redraw content
        self.clear_terminal_area(canvas);
        self.draw_tab_bar(canvas);

        // Restore content for the new active terminal
        match id {
            TerminalId::Shell => {
                // Shell: just show prompt (shell will redraw its state)
                self.terminal_pane.write_str(canvas, "breenix> ");
            }
            TerminalId::Logs => {
                // Logs: replay from buffer
                // Optimization: only replay lines that will be visible
                // Skip early lines that would scroll off, avoiding expensive scroll operations
                let visible_rows = self.terminal_pane.rows();
                let total_lines = self.log_buffer.lines.len();
                let skip_count = total_lines.saturating_sub(visible_rows);

                for line in self.log_buffer.lines.iter().skip(skip_count) {
                    self.terminal_pane.write_str(canvas, line);
                    self.terminal_pane.write_str(canvas, "\r\n");
                }
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
            self.terminal_pane.draw_cursor(canvas, false);
            self.terminal_pane.write_str(canvas, line);
            self.terminal_pane.write_str(canvas, "\r\n");
            self.terminal_pane.draw_cursor(canvas, self.cursor_visible);
        } else {
            // Mark as unread
            if !self.has_unread[TerminalId::Logs as usize] {
                self.has_unread[TerminalId::Logs as usize] = true;
                self.draw_tab_bar(canvas);
            }
        }
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
