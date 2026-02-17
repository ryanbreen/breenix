use alloc::vec::Vec;

use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::input::InputState;
use crate::rect::Rect;
use crate::text;
use crate::theme::Theme;

const TITLE_BAR_H: i32 = 24;
const BUTTON_BAR_H: i32 = 32;
const BUTTON_W: i32 = 60;
const BUTTON_H: i32 = 22;
const BUTTON_GAP: i32 = 8;
const ROW_PAD_LEFT: i32 = 4;

/// A file or directory entry for the picker.
pub struct FileEntry {
    pub name: Vec<u8>,
    pub is_dir: bool,
    pub size: u64,
}

/// Result of a FilePicker update cycle.
pub enum FilePickerResult {
    /// Still open, no action taken.
    Active,
    /// User confirmed selection of the entry at this index.
    Selected(usize),
    /// User wants to navigate into the directory at this index.
    NavigateDir(usize),
    /// User cancelled the dialog.
    Cancelled,
}

/// Modal file picker dialog with scrollable file list.
pub struct FilePicker {
    rect: Rect,
    path: Vec<u8>,
    entries: Vec<FileEntry>,
    selected: Option<usize>,
    scroll_offset: usize,
    visible_rows: usize,
    row_height: i32,
    list_area: Rect,
    open_btn: Rect,
    cancel_btn: Rect,
    open_hovered: bool,
    open_pressed: bool,
    cancel_hovered: bool,
    cancel_pressed: bool,
}

impl FilePicker {
    pub fn new(rect: Rect, path: Vec<u8>, entries: Vec<FileEntry>, theme: &Theme) -> Self {
        let row_height = text::text_height(theme) + 4;
        let (_, body) = rect.split_top(TITLE_BAR_H);
        let (btn_bar, list_body) = body.split_bottom(BUTTON_BAR_H);
        let list_area = list_body.inset(1);
        let visible_rows = if row_height > 0 {
            (list_area.h / row_height).max(0) as usize
        } else {
            0
        };

        let total_btn_w = BUTTON_W * 2 + BUTTON_GAP;
        let btn_x = btn_bar.x + btn_bar.w - total_btn_w - 8;
        let btn_y = btn_bar.y + (btn_bar.h - BUTTON_H) / 2;
        let cancel_btn = Rect::new(btn_x, btn_y, BUTTON_W, BUTTON_H);
        let open_btn = Rect::new(btn_x + BUTTON_W + BUTTON_GAP, btn_y, BUTTON_W, BUTTON_H);

        Self {
            rect,
            path,
            entries,
            selected: None,
            scroll_offset: 0,
            visible_rows,
            row_height,
            list_area,
            open_btn,
            cancel_btn,
            open_hovered: false,
            open_pressed: false,
            cancel_hovered: false,
            cancel_pressed: false,
        }
    }

    /// Replace the displayed directory and entries, resetting scroll and selection.
    pub fn navigate(&mut self, path: Vec<u8>, entries: Vec<FileEntry>) {
        self.path = path;
        self.entries = entries;
        self.selected = None;
        self.scroll_offset = 0;
    }

    /// Get the currently selected entry, if any.
    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.selected.and_then(|i| self.entries.get(i))
    }

    /// Process input and return the result.
    pub fn update(&mut self, input: &InputState) -> FilePickerResult {
        self.open_hovered = self.open_btn.contains(input.mouse_x, input.mouse_y);
        self.cancel_hovered = self.cancel_btn.contains(input.mouse_x, input.mouse_y);

        if self.open_hovered && input.mouse_pressed {
            self.open_pressed = true;
        }
        if self.cancel_hovered && input.mouse_pressed {
            self.cancel_pressed = true;
        }

        if input.mouse_released {
            if self.cancel_pressed && self.cancel_hovered {
                self.cancel_pressed = false;
                return FilePickerResult::Cancelled;
            }
            if self.open_pressed && self.open_hovered {
                self.open_pressed = false;
                if let Some(idx) = self.selected {
                    return self.activate(idx);
                }
            }
            self.open_pressed = false;
            self.cancel_pressed = false;
        }

        // Scroll indicator clicks
        if input.mouse_pressed && self.list_area.contains(input.mouse_x, input.mouse_y) {
            let rel_y = input.mouse_y - self.list_area.y;

            // Top scroll indicator region
            if self.scroll_offset > 0 && rel_y < self.row_height {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                return FilePickerResult::Active;
            }

            // Bottom scroll indicator region
            let has_more_below = self.scroll_offset + self.visible_rows < self.entries.len();
            if has_more_below {
                let bottom_start = self.list_area.h - self.row_height;
                if rel_y >= bottom_start {
                    self.scroll_offset += 1;
                    return FilePickerResult::Active;
                }
            }

            // Click on an entry row
            let row = (rel_y / self.row_height) as usize;
            let entry_idx = self.scroll_offset + row;
            if entry_idx < self.entries.len() {
                if self.selected == Some(entry_idx) {
                    // Double-click (click already-selected) activates
                    return self.activate(entry_idx);
                }
                self.selected = Some(entry_idx);
            }
        }

        FilePickerResult::Active
    }

    /// Draw the complete dialog.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        // Background
        shapes::fill_rect(fb, self.rect.x, self.rect.y, self.rect.w, self.rect.h, theme.panel_bg);

        // Title bar
        let title_rect = Rect::new(self.rect.x, self.rect.y, self.rect.w, TITLE_BAR_H);
        shapes::fill_rect(fb, title_rect.x, title_rect.y, title_rect.w, title_rect.h, theme.widget_bg);
        text::draw_text_centered(fb, &self.path, &title_rect, theme.text_primary, theme);
        shapes::draw_line(
            fb,
            self.rect.x,
            self.rect.y + TITLE_BAR_H,
            self.rect.x + self.rect.w - 1,
            self.rect.y + TITLE_BAR_H,
            theme.border,
        );

        // List area background
        shapes::fill_rect(
            fb,
            self.list_area.x,
            self.list_area.y,
            self.list_area.w,
            self.list_area.h,
            theme.panel_bg,
        );

        // Draw entries
        let end = (self.scroll_offset + self.visible_rows).min(self.entries.len());
        for i in self.scroll_offset..end {
            let row = (i - self.scroll_offset) as i32;
            let ry = self.list_area.y + row * self.row_height;
            let row_rect = Rect::new(self.list_area.x, ry, self.list_area.w, self.row_height);

            // Highlight selected row
            if self.selected == Some(i) {
                shapes::fill_rect(fb, row_rect.x, row_rect.y, row_rect.w, row_rect.h, theme.accent);
            }

            let text_y = ry + (self.row_height - text::text_height(theme)) / 2;
            let entry = &self.entries[i];

            if entry.is_dir {
                // "[D] dirname"
                let mut label = Vec::with_capacity(entry.name.len() + 4);
                label.extend_from_slice(b"[D] ");
                label.extend_from_slice(&entry.name);
                text::draw_text(
                    fb,
                    &label,
                    self.list_area.x + ROW_PAD_LEFT,
                    text_y,
                    theme.text_primary,
                    theme,
                );
            } else {
                // Filename on left
                text::draw_text(
                    fb,
                    &entry.name,
                    self.list_area.x + ROW_PAD_LEFT,
                    text_y,
                    theme.text_primary,
                    theme,
                );
                // Size on right
                let mut size_buf = [0u8; 16];
                let size_len = format_file_size(entry.size, &mut size_buf);
                let size_text = &size_buf[..size_len];
                let sw = text::text_width(size_text, theme);
                let sx = self.list_area.x + self.list_area.w - sw - ROW_PAD_LEFT;
                text::draw_text(fb, size_text, sx, text_y, theme.text_secondary, theme);
            }
        }

        // Scroll indicators
        if self.scroll_offset > 0 {
            let indicator = b"^ more ^";
            let iw = text::text_width(indicator, theme);
            let ix = self.list_area.x + (self.list_area.w - iw) / 2;
            let iy = self.list_area.y + 1;
            text::draw_text(fb, indicator, ix, iy, theme.text_secondary, theme);
        }
        if self.scroll_offset + self.visible_rows < self.entries.len() {
            let indicator = b"v more v";
            let iw = text::text_width(indicator, theme);
            let ix = self.list_area.x + (self.list_area.w - iw) / 2;
            let iy = self.list_area.y + self.list_area.h - self.row_height + 1;
            text::draw_text(fb, indicator, ix, iy, theme.text_secondary, theme);
        }

        // Button bar separator
        let sep_y = self.rect.y + self.rect.h - BUTTON_BAR_H;
        shapes::draw_line(
            fb,
            self.rect.x,
            sep_y,
            self.rect.x + self.rect.w - 1,
            sep_y,
            theme.border,
        );

        // Button bar background
        shapes::fill_rect(
            fb,
            self.rect.x,
            sep_y + 1,
            self.rect.w,
            BUTTON_BAR_H - 1,
            theme.widget_bg,
        );

        // Cancel button
        self.draw_button(fb, &self.cancel_btn, b"Cancel", self.cancel_hovered, self.cancel_pressed, theme);

        // Open button
        self.draw_button(fb, &self.open_btn, b"Open", self.open_hovered, self.open_pressed, theme);

        // Outer border
        shapes::draw_rect(fb, self.rect.x, self.rect.y, self.rect.w, self.rect.h, theme.border);
    }

    fn draw_button(
        &self,
        fb: &mut FrameBuf,
        rect: &Rect,
        label: &[u8],
        hovered: bool,
        pressed: bool,
        theme: &Theme,
    ) {
        let bg = if pressed {
            theme.widget_bg_active
        } else if hovered {
            theme.widget_bg_hover
        } else {
            theme.widget_bg
        };
        shapes::fill_rect(fb, rect.x, rect.y, rect.w, rect.h, bg);
        shapes::draw_rect(fb, rect.x, rect.y, rect.w, rect.h, theme.border);
        text::draw_text_centered(fb, label, rect, theme.text_primary, theme);
    }

    fn activate(&self, idx: usize) -> FilePickerResult {
        if let Some(entry) = self.entries.get(idx) {
            if entry.is_dir {
                FilePickerResult::NavigateDir(idx)
            } else {
                FilePickerResult::Selected(idx)
            }
        } else {
            FilePickerResult::Active
        }
    }
}

/// Format a file size into a human-readable string.
/// Returns the number of bytes written to `buf`.
fn format_file_size(size: u64, buf: &mut [u8; 16]) -> usize {
    if size < 1024 {
        // "N B"
        let n = write_u64(size, buf);
        buf[n] = b' ';
        buf[n + 1] = b'B';
        n + 2
    } else if size < 1024 * 1024 {
        // "N.D KB"
        let whole = size / 1024;
        let frac = (size % 1024) * 10 / 1024;
        let n = write_u64(whole, buf);
        buf[n] = b'.';
        buf[n + 1] = b'0' + frac as u8;
        buf[n + 2] = b' ';
        buf[n + 3] = b'K';
        buf[n + 4] = b'B';
        n + 5
    } else {
        // "N.D MB"
        let whole = size / (1024 * 1024);
        let frac = (size % (1024 * 1024)) * 10 / (1024 * 1024);
        let n = write_u64(whole, buf);
        buf[n] = b'.';
        buf[n + 1] = b'0' + frac as u8;
        buf[n + 2] = b' ';
        buf[n + 3] = b'M';
        buf[n + 4] = b'B';
        n + 5
    }
}

/// Write a u64 as decimal digits into `buf`. Returns number of bytes written.
fn write_u64(val: u64, buf: &mut [u8; 16]) -> usize {
    if val == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = val;
    let mut count = 0;
    while n > 0 {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }
    for i in 0..count {
        buf[i] = digits[count - 1 - i];
    }
    count
}
