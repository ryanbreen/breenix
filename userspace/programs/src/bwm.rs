//! Breenix Window Manager (bwm) — Pure Compositor + Input Router
//!
//! Discovers Breengel client windows registered via the kernel window buffer API,
//! composites them onto the screen with GPU acceleration (VirGL), and routes
//! keyboard/mouse input to the focused window via kernel ring buffers.
//!
//! BWM does NOT spawn child processes or emulate terminals. Those responsibilities
//! belong to init (process spawning) and bterm (terminal emulation).

use std::process;

use libbreenix::graphics::{self, WindowInputEvent, input_event_type};
use libbreenix::io;
use libbreenix::signal;
use libbreenix::types::Fd;

use libgfx::bitmap_font;
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Title bar height in pixels
const TITLE_BAR_HEIGHT: usize = 32;

/// Window border/shadow width
const BORDER_WIDTH: usize = 2;

/// Noto Sans Mono 16px cell dimensions for title bar text.
const CELL_W: usize = 7;
const CELL_H: usize = 18;

/// Top taskbar height
const TASKBAR_HEIGHT: usize = 28;

/// Bottom app bar height
const APPBAR_HEIGHT: usize = 36;

/// Chrome button size (close/minimize)
const CHROME_BTN_SIZE: usize = 20;

/// Padding between chrome buttons
const CHROME_BTN_PAD: usize = 4;

/// Space reserved in title bar for chrome buttons
const CHROME_RESERVED: usize = 52;

// Colors
const TITLE_FOCUSED_BG: Color = Color::rgb(40, 100, 220);
const TITLE_UNFOCUSED_BG: Color = Color::rgb(45, 50, 65);
const TITLE_TEXT: Color = Color::rgb(160, 165, 175);
const TITLE_FOCUSED_TEXT: Color = Color::rgb(255, 255, 255);
const WIN_BORDER_COLOR: Color = Color::rgb(50, 55, 70);
const WIN_BORDER_FOCUSED: Color = Color::rgb(60, 130, 255);

// Taskbar/Appbar colors
const TASKBAR_BG: Color = Color::rgb(20, 22, 30);
const TASKBAR_TEXT: Color = Color::rgb(180, 185, 195);
const APPBAR_BG: Color = Color::rgb(25, 28, 38);
const APPBAR_BORDER: Color = Color::rgb(50, 55, 70);
const APPBAR_BTN_BG: Color = Color::rgb(40, 45, 60);
const APPBAR_BTN_FOCUSED: Color = Color::rgb(40, 100, 220);
const APPBAR_BTN_MINIMIZED: Color = Color::rgb(30, 33, 45);
const APPBAR_BTN_TEXT: Color = Color::rgb(160, 165, 175);

// Chrome button colors
const CLOSE_BTN_BG: Color = Color::rgb(200, 50, 50);
const CLOSE_BTN_TEXT: Color = Color::rgb(255, 255, 255);
const MINIMIZE_BTN_BG: Color = Color::rgb(80, 85, 100);
const MINIMIZE_BTN_TEXT: Color = Color::rgb(255, 255, 255);

// ─── Input Parser ────────────────────────────────────────────────────────────
// Parses stdin bytes (keyboard input) into InputEvents that BWM can either
// handle internally (F-key focus switching) or route to the focused client
// window as WindowInputEvents.

struct InputParser {
    state: InputState,
    esc_buf: [u8; 8],
    esc_len: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum InputState { Normal, Escape, CsiOrSS3 }

enum InputEvent {
    /// A regular key press: ascii byte + modifiers (bit 1 = ctrl)
    Key { ascii: u8, keycode: u16, modifiers: u16 },
    /// F1..F5 for BWM-internal focus switching
    FunctionKey(u8),
}

impl InputParser {
    fn new() -> Self { Self { state: InputState::Normal, esc_buf: [0; 8], esc_len: 0 } }

    fn feed(&mut self, byte: u8) -> Option<InputEvent> {
        match self.state {
            InputState::Normal => {
                if byte == 0x1b { self.state = InputState::Escape; self.esc_len = 0; None }
                else { Some(Self::byte_to_key_event(byte)) }
            }
            InputState::Escape => {
                if byte == b'[' || byte == b'O' {
                    self.state = InputState::CsiOrSS3; self.esc_buf[0] = byte; self.esc_len = 1; None
                } else { self.state = InputState::Normal; Some(Self::byte_to_key_event(byte)) }
            }
            InputState::CsiOrSS3 => {
                if self.esc_len < 7 { self.esc_buf[self.esc_len] = byte; self.esc_len += 1; }
                if byte >= 0x40 && byte <= 0x7e { self.state = InputState::Normal; self.decode_escape() }
                else { None }
            }
        }
    }

    /// Convert a raw stdin byte into a Key event with appropriate keycode/modifiers.
    fn byte_to_key_event(byte: u8) -> InputEvent {
        match byte {
            // Enter (before Ctrl range since 0x0D falls in 0x01..=0x1A)
            0x0D => InputEvent::Key { ascii: 13, keycode: 0x28, modifiers: 0 },
            // Backspace (0x08 falls in Ctrl range; 0x7F does not)
            0x08 | 0x7F => InputEvent::Key { ascii: 8, keycode: 0x2A, modifiers: 0 },
            // Tab (0x09 falls in Ctrl range)
            0x09 => InputEvent::Key { ascii: 9, keycode: 0x2B, modifiers: 0 },
            // Ctrl+A through Ctrl+Z (0x01..0x1A), excluding Enter/Backspace/Tab matched above
            0x01..=0x1A => InputEvent::Key {
                ascii: byte,
                keycode: byte as u16, // raw control code
                modifiers: 0x02,      // ctrl bit
            },
            // Printable ASCII + space
            _ => InputEvent::Key { ascii: byte, keycode: byte as u16, modifiers: 0 },
        }
    }

    fn decode_escape(&self) -> Option<InputEvent> {
        if self.esc_len < 2 { return None; }
        let final_byte = self.esc_buf[self.esc_len - 1];
        if self.esc_buf[0] == b'[' {
            match final_byte {
                // Arrow keys -> Key events with USB HID keycodes
                b'A' => return Some(InputEvent::Key { ascii: 0, keycode: 0x52, modifiers: 0 }), // Up
                b'B' => return Some(InputEvent::Key { ascii: 0, keycode: 0x51, modifiers: 0 }), // Down
                b'C' => return Some(InputEvent::Key { ascii: 0, keycode: 0x4F, modifiers: 0 }), // Right
                b'D' => return Some(InputEvent::Key { ascii: 0, keycode: 0x50, modifiers: 0 }), // Left
                b'~' if self.esc_len >= 3 => {
                    let _num = self.esc_buf[1] - b'0';
                    if self.esc_len == 3 { if _num == 1 { return Some(InputEvent::FunctionKey(1)); } }
                    else if self.esc_len == 4 {
                        let num2 = (self.esc_buf[1] - b'0') * 10 + (self.esc_buf[2] - b'0');
                        match num2 {
                            11 => return Some(InputEvent::FunctionKey(1)),
                            12 => return Some(InputEvent::FunctionKey(2)),
                            13 => return Some(InputEvent::FunctionKey(3)),
                            14 => return Some(InputEvent::FunctionKey(4)),
                            15 => return Some(InputEvent::FunctionKey(5)),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        } else if self.esc_buf[0] == b'O' {
            match final_byte {
                b'P' => return Some(InputEvent::FunctionKey(1)),
                b'Q' => return Some(InputEvent::FunctionKey(2)),
                b'R' => return Some(InputEvent::FunctionKey(3)),
                b'S' => return Some(InputEvent::FunctionKey(4)),
                _ => {}
            }
        }
        None
    }
}

// ─── Window ─────────────────────────────────────────────────────────────────

struct Window {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    title: [u8; 32],
    title_len: usize,
    window_id: u32,
    owner_pid: u32,
    minimized: bool,
    /// Stable ordering for appbar (assigned at discovery time, never changes)
    creation_order: u32,
}

impl Window {
    fn content_x(&self) -> i32 { self.x + BORDER_WIDTH as i32 }
    fn content_y(&self) -> i32 { self.y + TITLE_BAR_HEIGHT as i32 + BORDER_WIDTH as i32 }
    fn content_width(&self) -> usize { self.width.saturating_sub(BORDER_WIDTH * 2) }
    fn content_height(&self) -> usize { self.height.saturating_sub(TITLE_BAR_HEIGHT + BORDER_WIDTH * 2) }

    fn total_height(&self) -> usize { self.height }

    fn hit_title(&self, mx: i32, my: i32) -> bool {
        mx >= self.x && mx < self.x + self.width as i32
            && my >= self.y && my < self.y + TITLE_BAR_HEIGHT as i32
    }

    fn hit_any(&self, mx: i32, my: i32) -> bool {
        mx >= self.x && mx < self.x + self.width as i32
            && my >= self.y && my < self.y + self.total_height() as i32
    }

    fn hit_content(&self, mx: i32, my: i32) -> bool {
        mx >= self.content_x() && mx < self.content_x() + self.content_width() as i32
            && my >= self.content_y() && my < self.content_y() + self.content_height() as i32
    }

    fn bounds(&self) -> (i32, i32, i32, i32) {
        // +3 accounts for the drop shadow drawn at (x+3, y+3) in draw_window_frame
        (self.x, self.y, self.x + self.width as i32 + 3, self.y + self.total_height() as i32 + 3)
    }

    fn close_btn_rect(&self) -> (i32, i32, usize, usize) {
        let bx = self.x + self.width as i32 - BORDER_WIDTH as i32
            - CHROME_BTN_PAD as i32 - CHROME_BTN_SIZE as i32;
        let by = self.y + BORDER_WIDTH as i32
            + ((TITLE_BAR_HEIGHT - BORDER_WIDTH - CHROME_BTN_SIZE) / 2) as i32;
        (bx, by, CHROME_BTN_SIZE, CHROME_BTN_SIZE)
    }

    fn minimize_btn_rect(&self) -> (i32, i32, usize, usize) {
        let (cx, cy, _, _) = self.close_btn_rect();
        (cx - CHROME_BTN_PAD as i32 - CHROME_BTN_SIZE as i32, cy, CHROME_BTN_SIZE, CHROME_BTN_SIZE)
    }

    fn hit_close_button(&self, mx: i32, my: i32) -> bool {
        let (bx, by, bw, bh) = self.close_btn_rect();
        mx >= bx && mx < bx + bw as i32 && my >= by && my < by + bh as i32
    }

    fn hit_minimize_button(&self, mx: i32, my: i32) -> bool {
        let (bx, by, bw, bh) = self.minimize_btn_rect();
        mx >= bx && mx < bx + bw as i32 && my >= by && my < by + bh as i32
    }
}

// ─── Drawing Helpers ─────────────────────────────────────────────────────────

fn fill_rect(fb: &mut FrameBuf, x: i32, y: i32, w: usize, h: usize, color: Color) {
    libgfx::shapes::fill_rect(fb, x, y, w as i32, h as i32, color);
}

fn draw_text_at(fb: &mut FrameBuf, text: &[u8], x: i32, y: i32, color: Color) {
    if y < 0 || y >= fb.height as i32 { return; }
    for (i, &ch) in text.iter().enumerate() {
        let px = x + (i as i32) * CELL_W as i32;
        if px < 0 || px + CELL_W as i32 > fb.width as i32 { continue; }
        bitmap_font::draw_char(fb, ch as char, px as usize, y as usize, color);
    }
}

/// Draw a floating window frame (border + title bar + chrome buttons + content bg)
fn draw_window_frame(fb: &mut FrameBuf, win: &Window, focused: bool) {
    let border_color = if focused { WIN_BORDER_FOCUSED } else { WIN_BORDER_COLOR };
    let title_bg = if focused { TITLE_FOCUSED_BG } else { TITLE_UNFOCUSED_BG };
    let title_fg = if focused { TITLE_FOCUSED_TEXT } else { TITLE_TEXT };
    let bw = BORDER_WIDTH;

    let shadow = Color::rgb(8, 10, 18);
    fill_rect(fb, win.x + 3, win.y + 3, win.width, win.total_height(), shadow);
    fill_rect(fb, win.x, win.y, win.width, win.total_height(), border_color);
    fill_rect(fb, win.x + bw as i32, win.y + bw as i32,
              win.width - bw * 2, TITLE_BAR_HEIGHT - bw, title_bg);

    // Title text (truncated to not overlap chrome buttons)
    let max_title_w = win.width.saturating_sub(bw * 2 + 8 + CHROME_RESERVED);
    let max_chars = max_title_w / CELL_W;
    let text_len = win.title_len.min(max_chars);
    let text_y = win.y + bw as i32 + ((TITLE_BAR_HEIGHT - bw - CELL_H) / 2) as i32;
    draw_text_at(fb, &win.title[..text_len], win.x + 8, text_y, title_fg);

    // Close button (rightmost in title bar)
    let (cbx, cby, cbw, cbh) = win.close_btn_rect();
    fill_rect(fb, cbx, cby, cbw, cbh, CLOSE_BTN_BG);
    let cx = cbx + (cbw as i32 - CELL_W as i32) / 2;
    let cy = cby + (cbh as i32 - CELL_H as i32) / 2;
    draw_text_at(fb, b"x", cx, cy, CLOSE_BTN_TEXT);

    // Minimize button (left of close)
    let (mbx, mby, mbw, mbh) = win.minimize_btn_rect();
    fill_rect(fb, mbx, mby, mbw, mbh, MINIMIZE_BTN_BG);
    let mx = mbx + (mbw as i32 - CELL_W as i32) / 2;
    let my = mby + (mbh as i32 - CELL_H as i32) / 2;
    draw_text_at(fb, b"-", mx, my, MINIMIZE_BTN_TEXT);

    // Content area NOT filled here — GPU composites per-window textures over it.
}

/// Paint the decorative desktop background — gradient with grid
fn paint_background(fb: &mut FrameBuf) {
    let w = fb.width;
    let h = fb.height;
    for y in 0..h {
        for x in 0..w {
            let t = (y * 255 / h) as u8;
            let r = 12u8.saturating_add(t / 12);
            let g = 16u8.saturating_add(t / 20);
            let b = 38u8.saturating_add(t / 6);
            let grid = (x % 64 == 0 || y % 64 == 0) as u8;
            let r2 = r.saturating_add(grid * 6);
            let g2 = g.saturating_add(grid * 8);
            let b2 = b.saturating_add(grid * 12);
            fb.put_pixel(x, y, Color::rgb(r2, g2, b2));
        }
    }
}

// ─── Taskbar & App Bar ──────────────────────────────────────────────────────

/// US Eastern Time offset from UTC in seconds.
/// DST (EDT, UTC-4) runs from 2nd Sunday of March to 1st Sunday of November.
/// Standard (EST, UTC-5) applies the rest of the year.
fn eastern_offset_secs(utc_secs: i64) -> i64 {
    // Compute day-of-year, month, day-of-week from Unix timestamp
    let days = (utc_secs / 86400) as i64;

    // Compute year/month/day
    let mut y = 1970i64;
    let mut rem = days;
    loop {
        let ydays = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if rem < ydays { break; }
        rem -= ydays;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays: [i64; 12] = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0u8;
    for i in 0..12 {
        if rem < mdays[i] { month = i as u8 + 1; break; }
        rem -= mdays[i];
    }
    let day = rem as u8 + 1;

    // 2nd Sunday of March: find first Sunday in March, add 7
    // 1st Sunday of November: find first Sunday in November
    // Check month boundaries for DST transitions
    // DST starts: 2nd Sunday of March at 2:00 AM EST (7:00 AM UTC)
    // DST ends: 1st Sunday of November at 2:00 AM EDT (6:00 AM UTC)
    let hour_utc = ((utc_secs % 86400) / 3600) as u8;

    // March: find day-of-week of March 1
    let jan1_dow = {
        let mut d = 0i64;
        for yr in 1970..y {
            d += if yr % 4 == 0 && (yr % 100 != 0 || yr % 400 == 0) { 366 } else { 365 };
        }
        ((d % 7 + 4) % 7) as u8 // 0=Sun
    };
    // Day of week of March 1 = (jan1_dow + 31 + feb_days) % 7
    let feb = if leap { 29 } else { 28 };
    let mar1_dow = (jan1_dow + 31 + feb) % 7;
    let dst_start_day = if mar1_dow == 0 { 8u8 } else { (14 - mar1_dow + 1) as u8 }; // 2nd Sunday
    // Day of week of Nov 1
    let days_to_nov1: u16 = [31u16, feb as u16, 31, 30, 31, 30, 31, 31, 30, 31].iter().sum();
    let nov1_dow = (jan1_dow as u16 + days_to_nov1) % 7;
    let dst_end_day = if nov1_dow == 0 { 1u8 } else { 7 - nov1_dow as u8 + 1 };

    let is_dst = match month {
        1..=2 => false,
        4..=10 => true,
        3 => day > dst_start_day || (day == dst_start_day && hour_utc >= 7),
        11 => day < dst_end_day || (day == dst_end_day && hour_utc < 6),
        12 => false,
        _ => false,
    };

    if is_dst { -4 * 3600 } else { -5 * 3600 }
}

fn format_clock(utc_secs: i64, buf: &mut [u8; 11]) {
    let offset = eastern_offset_secs(utc_secs);
    let local = utc_secs + offset;
    // Handle day wrap
    let t = ((local % 86400) + 86400) % 86400;
    let s = (t % 60) as u8;
    let m = ((t / 60) % 60) as u8;
    let h = ((t / 3600) % 24) as u8;
    buf[0] = b'0' + h / 10;
    buf[1] = b'0' + h % 10;
    buf[2] = b':';
    buf[3] = b'0' + m / 10;
    buf[4] = b'0' + m % 10;
    buf[5] = b':';
    buf[6] = b'0' + s / 10;
    buf[7] = b'0' + s % 10;
    buf[8] = b' ';
    buf[9] = b'E';
    buf[10] = b'T';
}

fn draw_taskbar(fb: &mut FrameBuf, clock_text: &[u8]) {
    let w = fb.width;
    fill_rect(fb, 0, 0, w, TASKBAR_HEIGHT, TASKBAR_BG);
    // "Breenix" label on the left
    let label_y = ((TASKBAR_HEIGHT - CELL_H) / 2 + 1) as i32;
    draw_text_at(fb, b"Breenix", 8, label_y, TASKBAR_TEXT);
    // Clock on the right
    if !clock_text.is_empty() {
        let clock_x = w as i32 - (clock_text.len() as i32 * CELL_W as i32) - 8;
        draw_text_at(fb, clock_text, clock_x, label_y, TASKBAR_TEXT);
    }
}

fn appbar_button_width(title_len: usize) -> usize {
    let text_w = title_len * CELL_W + 16;
    text_w.max(60).min(180)
}

/// Build indices sorted by creation_order (stable appbar layout).
fn sorted_by_creation(windows: &[Window]) -> ([usize; 16], usize) {
    let n = windows.len().min(16);
    let mut idx = [0usize; 16];
    for i in 0..n { idx[i] = i; }
    // Insertion sort (at most 16 elements)
    for i in 1..n {
        let mut j = i;
        while j > 0 && windows[idx[j]].creation_order < windows[idx[j - 1]].creation_order {
            idx.swap(j, j - 1);
            j -= 1;
        }
    }
    (idx, n)
}

fn draw_appbar(fb: &mut FrameBuf, windows: &[Window], focused_win: usize) {
    let screen_h = fb.height;
    let screen_w = fb.width;
    let bar_y = (screen_h - APPBAR_HEIGHT) as i32;

    // Background
    fill_rect(fb, 0, bar_y, screen_w, APPBAR_HEIGHT, APPBAR_BG);
    // 1px top border
    fill_rect(fb, 0, bar_y, screen_w, 1, APPBAR_BORDER);

    // Window buttons in stable creation order
    let (sorted, n) = sorted_by_creation(windows);
    let mut btn_x: i32 = 4;
    let btn_h: usize = APPBAR_HEIGHT - 8;
    let btn_y = bar_y + 4;

    for k in 0..n {
        let i = sorted[k];
        let win = &windows[i];
        let btn_w = appbar_button_width(win.title_len);

        let bg = if i == focused_win && !win.minimized {
            APPBAR_BTN_FOCUSED
        } else if win.minimized {
            APPBAR_BTN_MINIMIZED
        } else {
            APPBAR_BTN_BG
        };

        fill_rect(fb, btn_x, btn_y, btn_w, btn_h, bg);

        // Title text (truncated to fit button)
        let max_chars = (btn_w.saturating_sub(12)) / CELL_W;
        let text_len = win.title_len.min(max_chars);
        let text_y = btn_y + ((btn_h - CELL_H) / 2 + 1) as i32;
        draw_text_at(fb, &win.title[..text_len], btn_x + 6, text_y, APPBAR_BTN_TEXT);

        btn_x += btn_w as i32 + 2;
        if btn_x >= screen_w as i32 - 4 { break; }
    }
}

fn appbar_hit_test(windows: &[Window], screen_w: usize, screen_h: usize, mx: i32, my: i32) -> Option<usize> {
    let bar_y = (screen_h - APPBAR_HEIGHT) as i32;
    if my < bar_y || my >= screen_h as i32 { return None; }

    let (sorted, n) = sorted_by_creation(windows);
    let mut btn_x: i32 = 4;
    let btn_h = (APPBAR_HEIGHT - 8) as i32;
    let btn_y = bar_y + 4;

    for k in 0..n {
        let i = sorted[k];
        let btn_w = appbar_button_width(windows[i].title_len) as i32;
        if mx >= btn_x && mx < btn_x + btn_w
            && my >= btn_y && my < btn_y + btn_h
        {
            return Some(i); // Returns the actual Vec index, not the sorted position
        }
        btn_x += btn_w + 2;
        if btn_x >= screen_w as i32 - 4 { break; }
    }
    None
}

fn next_visible_window(windows: &[Window], current: usize) -> usize {
    for i in (0..windows.len()).rev() {
        if !windows[i].minimized {
            return i;
        }
    }
    current
}

// ─── Input Routing ──────────────────────────────────────────────────────────

fn route_keyboard_to_focused(windows: &[Window], focused_win: usize, event: &WindowInputEvent) {
    if focused_win < windows.len() && windows[focused_win].window_id != 0 {
        let _ = graphics::write_window_input(windows[focused_win].window_id, event);
    }
}

fn send_focus_event(windows: &[Window], win_idx: usize, event_type: u16) {
    if win_idx < windows.len() && windows[win_idx].window_id != 0 {
        let event = WindowInputEvent {
            event_type,
            keycode: 0, mouse_x: 0, mouse_y: 0, modifiers: 0, _pad: 0,
        };
        let _ = graphics::write_window_input(windows[win_idx].window_id, &event);
    }
}

fn route_mouse_button_to_focused(
    windows: &[Window], focused_win: usize,
    button: u16, pressed: bool, win_local_x: i16, win_local_y: i16,
) {
    if focused_win < windows.len() && windows[focused_win].window_id != 0 {
        let event = WindowInputEvent {
            event_type: input_event_type::MOUSE_BUTTON,
            keycode: button,
            mouse_x: win_local_x,
            mouse_y: win_local_y,
            modifiers: 0,
            _pad: if pressed { 1 } else { 0 },
        };
        let _ = graphics::write_window_input(windows[focused_win].window_id, &event);
    }
}

fn route_mouse_move_to_focused(
    windows: &[Window], focused_win: usize,
    win_local_x: i16, win_local_y: i16,
) {
    if focused_win < windows.len() && windows[focused_win].window_id != 0 {
        let event = WindowInputEvent {
            event_type: input_event_type::MOUSE_MOVE,
            keycode: 0,
            mouse_x: win_local_x, mouse_y: win_local_y,
            modifiers: 0, _pad: 0,
        };
        let _ = graphics::write_window_input(windows[focused_win].window_id, &event);
    }
}

// ─── Window Discovery ───────────────────────────────────────────────────────

fn discover_windows(windows: &mut Vec<Window>, screen_w: usize, screen_h: usize, next_order: &mut u32) -> bool {
    let mut win_infos = [graphics::WindowInfo {
        buffer_id: 0, owner_pid: 0, width: 0, height: 0,
        x: 0, y: 0, title_len: 0, title: [0; 64],
    }; 16];
    let count = match graphics::list_windows(&mut win_infos) {
        Ok(c) => c as usize,
        Err(_) => return false,
    };

    let before = windows.len();
    windows.retain(|w| {
        win_infos[..count].iter().any(|info| info.buffer_id == w.window_id)
    });
    let removed = before > windows.len();

    let mut added = false;
    for i in 0..count {
        let info = &win_infos[i];
        if info.buffer_id == 0 { continue; }
        if windows.iter().any(|w| w.window_id == info.buffer_id) { continue; }

        let n = windows.len();
        let usable_h = screen_h.saturating_sub(TASKBAR_HEIGHT + APPBAR_HEIGHT);
        let cascade_x = 30 + (n as i32 * 50) % ((screen_w as i32 - 500).max(100));
        let cascade_y = TASKBAR_HEIGHT as i32 + 10
            + (n as i32 * 50) % ((usable_h as i32 - 500).max(100));

        let total_w = info.width as usize + BORDER_WIDTH * 2;
        let total_h = info.height as usize + TITLE_BAR_HEIGHT + BORDER_WIDTH * 2;

        let mut title = [0u8; 32];
        let title_len = (info.title_len as usize).min(32);
        title[..title_len].copy_from_slice(&info.title[..title_len]);

        print!("[bwm] Discovered window '{}' (id={}, {}x{}) at ({},{})\n",
            core::str::from_utf8(&title[..title_len]).unwrap_or("?"),
            info.buffer_id, info.width, info.height, cascade_x, cascade_y);

        // Tell kernel where the client content goes on screen (for GPU compositing).
        // z_order = index in windows vec (0 = bottom). New windows are pushed to
        // the end, so they get the highest z_order.
        let content_x = cascade_x + BORDER_WIDTH as i32;
        let content_y = cascade_y + TITLE_BAR_HEIGHT as i32 + BORDER_WIDTH as i32;
        let z_order = windows.len() as u32; // will be at this index after push
        let _ = graphics::set_window_position(info.buffer_id, content_x, content_y, z_order);

        let order = *next_order;
        *next_order += 1;
        windows.push(Window {
            x: cascade_x, y: cascade_y, width: total_w, height: total_h,
            title, title_len, window_id: info.buffer_id,
            owner_pid: info.owner_pid,
            minimized: false,
            creation_order: order,
        });
        added = true;
    }

    removed || added
}

/// Update kernel z-order for all windows. Called after any z-order change
/// (raise-to-front, new window, etc.) so the GPU compositor draws quads
/// in correct back-to-front order.
fn update_kernel_z_order(windows: &[Window]) {
    for (i, win) in windows.iter().enumerate() {
        if win.window_id != 0 {
            let _ = graphics::set_window_position(win.window_id, win.content_x(), win.content_y(), i as u32);
        }
    }
}

/// Redraw all windows in z-order (index 0 = bottom), plus taskbar and app bar.
/// Window frames and decorations go into the compositor buffer; GPU compositing
/// handles client content via per-window textured quads.
fn redraw_all_windows(fb: &mut FrameBuf, windows: &[Window], focused_win: usize, clock_text: &[u8]) {
    draw_taskbar(fb, clock_text);
    for i in 0..windows.len() {
        if windows[i].minimized { continue; }
        draw_window_frame(fb, &windows[i], i == focused_win);
        // Window content rendered by GPU from per-window textures — no CPU blit needed
    }
    draw_appbar(fb, windows, focused_win);
}

/// Compose a full-redraw into `vram` without flashing.
///
/// When a shadow buffer is available (SVGA3 direct-mapped VRAM), composes into
/// the shadow first, then bulk-copies to VRAM so the display update is atomic.
/// Otherwise, composes directly into the primary buffer.
fn compose_full_redraw(
    vram: &mut [u32],
    fb: &mut FrameBuf,
    shadow: &mut Option<(&mut [u32], FrameBuf)>,
    bg: &[u32],
    windows: &[Window],
    focused: usize,
    clock: &[u8],
) {
    if let Some((ref mut sbuf, ref mut sfb)) = shadow {
        sbuf.copy_from_slice(bg);
        redraw_all_windows(sfb, windows, focused, clock);
        vram.copy_from_slice(sbuf);
    } else {
        vram.copy_from_slice(bg);
        redraw_all_windows(fb, windows, focused, clock);
    }
}

/// Partial redraw: only update a dirty sub-region of the screen.
///
/// Used during drag to avoid full-screen VRAM copies. On SVGA3 (VMware),
/// VRAM is uncacheable so writing 9.2MB per frame kills drag performance.
/// Partial redraw limits VRAM writes to just the union of old+new window bounds.
fn compose_partial_redraw(
    vram: &mut [u32],
    fb: &mut FrameBuf,
    shadow: &mut Option<(&mut [u32], FrameBuf)>,
    bg: &[u32],
    windows: &[Window],
    focused: usize,
    clock: &[u8],
    dx0: usize, dy0: usize, dx1: usize, dy1: usize,
) {
    let screen_w = fb.width;
    let screen_h = fb.height;
    let dx1 = dx1.min(screen_w);
    let dy1 = dy1.min(screen_h);
    if dx0 >= dx1 || dy0 >= dy1 { return; }

    if let Some((ref mut sbuf, ref mut sfb)) = shadow {
        // 1. Restore background in dirty region only
        for row in dy0..dy1 {
            let start = row * screen_w + dx0;
            let end = row * screen_w + dx1;
            sbuf[start..end].copy_from_slice(&bg[start..end]);
        }
        // 2. Redraw UI elements (frames only — content rendered by GPU)
        if dy0 < TASKBAR_HEIGHT {
            draw_taskbar(sfb, clock);
        }
        for i in 0..windows.len() {
            if windows[i].minimized { continue; }
            let (wx0, wy0, wx1, wy1) = windows[i].bounds();
            if (wx1 as usize) > dx0 && (wx0 as usize) < dx1
                && (wy1 as usize) > dy0 && (wy0 as usize) < dy1
            {
                draw_window_frame(sfb, &windows[i], i == focused);
            }
        }
        if dy1 > screen_h - APPBAR_HEIGHT {
            draw_appbar(sfb, windows, focused);
        }
        // 3. Copy only dirty region from shadow to VRAM
        for row in dy0..dy1 {
            let start = row * screen_w + dx0;
            let end = row * screen_w + dx1;
            vram[start..end].copy_from_slice(&sbuf[start..end]);
        }
    } else {
        // Non-shadow path: restore bg region, redraw affected windows (frames only)
        for row in dy0..dy1 {
            let start = row * screen_w + dx0;
            let end = row * screen_w + dx1;
            vram[start..end].copy_from_slice(&bg[start..end]);
        }
        if dy0 < TASKBAR_HEIGHT {
            draw_taskbar(fb, clock);
        }
        for i in 0..windows.len() {
            if windows[i].minimized { continue; }
            let (wx0, wy0, wx1, wy1) = windows[i].bounds();
            if (wx1 as usize) > dx0 && (wx0 as usize) < dx1
                && (wy1 as usize) > dy0 && (wy0 as usize) < dy1
            {
                draw_window_frame(fb, &windows[i], i == focused);
            }
        }
        if dy1 > screen_h - APPBAR_HEIGHT {
            draw_appbar(fb, windows, focused);
        }
    }
}

// ─── Main ────────────────────────────────────────────────────────────────────

fn main() {
    print!("[bwm] Breenix Window Manager starting...\n");

    if let Err(e) = graphics::take_over_display() {
        print!("[bwm] WARNING: take_over_display failed: {}\n", e);
    }

    let info = {
        let mut result = None;
        for attempt in 0..10 {
            match graphics::fbinfo() {
                Ok(info) => { result = Some(info); break; }
                Err(_) if attempt < 9 => {
                    let _ = libbreenix::time::nanosleep(&libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 10_000_000 });
                }
                Err(e) => { print!("[bwm] ERROR: fbinfo failed: {}\n", e); process::exit(1); }
            }
        }
        result.unwrap()
    };

    let screen_w = info.width as usize;
    let screen_h = info.height as usize;
    let bpp = info.bytes_per_pixel as usize;

    let gpu_compositing = {
        let test_pixel: [u32; 1] = [0xFF000000];
        graphics::virgl_composite(&test_pixel, 1, 1).is_ok()
    };
    if !gpu_compositing {
        print!("[bwm] ERROR: GPU compositing required\n");
        process::exit(1);
    }

    print!("[bwm] GPU compositing mode (VirGL), display: {}x{}\n", screen_w, screen_h);

    // Try to map COMPOSITE_TEX directly into our address space.
    // If successful, all pixel writes go straight to GPU texture backing (zero-copy).
    let (composite_buf, direct_mapped) = match graphics::map_compositor_texture() {
        Ok((ptr, tex_w, tex_h)) => {
            let mapped_w = tex_w as usize;
            let mapped_h = tex_h as usize;
            print!("[bwm] Direct compositor mapping: {}x{} at {:p}\n", mapped_w, mapped_h, ptr);
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, mapped_w * mapped_h) };
            (buf, true)
        }
        Err(_) => {
            print!("[bwm] Fallback: heap-allocated compositor buffer\n");
            // Leak a Vec to get a &'static mut slice — BWM runs for the lifetime of the OS
            let v = vec![0u32; screen_w * screen_h];
            let leaked = v.leak();
            (leaked as &mut [u32], false)
        }
    };

    let mut fb = unsafe {
        FrameBuf::from_raw(
            composite_buf.as_mut_ptr() as *mut u8,
            screen_w, screen_h, screen_w * bpp, bpp, info.is_bgr(),
        )
    };

    // Shadow buffer for double-buffered full redraws on direct-mapped VRAM (SVGA3).
    // VRAM traces auto-display every pixel write, causing a flash when we do
    // "clear to bg → redraw all windows". The shadow lets us compose off-screen,
    // then bulk-copy the final result to VRAM in one memcpy.
    // For VirGL (non-direct-mapped), composite_buf is a heap buffer so no flash.
    let mut shadow_fb: Option<(&'static mut [u32], FrameBuf)> = if direct_mapped {
        let v = vec![0u32; screen_w * screen_h];
        let buf = v.leak();
        let sfb = unsafe {
            FrameBuf::from_raw(
                buf.as_mut_ptr() as *mut u8,
                screen_w, screen_h, screen_w * bpp, bpp, info.is_bgr(),
            )
        };
        Some((buf, sfb))
    } else {
        None
    };

    // Paint decorative background and cache it for fast restoration
    paint_background(&mut fb);
    let bg_cache = composite_buf.to_vec();

    // Enter raw mode on stdin
    let mut orig_termios = libbreenix::termios::Termios::default();
    let _ = libbreenix::termios::tcgetattr(Fd::from_raw(0), &mut orig_termios);
    let mut raw = orig_termios;
    libbreenix::termios::cfmakeraw(&mut raw);
    let _ = libbreenix::termios::tcsetattr(Fd::from_raw(0), libbreenix::termios::TCSANOW, &raw);

    let mut windows: Vec<Window> = Vec::new();
    let mut focused_win: usize = 0;
    let mut input_parser = InputParser::new();
    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut prev_buttons: u32 = 0;
    let mut dragging: Option<(usize, i32, i32)> = None;
    let mut full_redraw = true;
    let mut content_dirty = false;
    let mut windows_dirty = false;

    // Clock state
    let mut last_clock_sec: i64 = -1;
    let mut clock_text = [0u8; 11];
    format_clock(0, &mut clock_text);
    let mut frame_counter: u32 = 0;
    let mut next_creation_order: u32 = 0;

    // Initial composite
    if direct_mapped {
        // Data is already in COMPOSITE_TEX — just tell kernel to upload + display
        let _ = graphics::virgl_composite_windows_rect(&[], 0, 0, 1, 0, 0, screen_w as u32, screen_h as u32);
    } else {
        let _ = graphics::virgl_composite_windows(&composite_buf, screen_w as u32, screen_h as u32, true);
    }

    let mut read_buf = [0u8; 512];
    let mut poll_fds = [io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }];

    // Registry generation tracking for compositor_wait
    let mut registry_gen: u32 = 0;

    // Initial window discovery (before entering event loop)
    if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order) {
        focused_win = next_visible_window(&windows, 0);
        compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text);
        full_redraw = true;
    }

    loop {
        // ── 0. Block until something needs compositing ──
        // compositor_wait blocks in the kernel until: window dirty, mouse moved,
        // registry changed, or timeout. Replaces the old poll+sleep_ms(2) pattern.
        // 16ms timeout ensures keyboard input via stdin is checked at least ~60Hz.
        let (ready, new_reg_gen) = graphics::compositor_wait(16, registry_gen).unwrap_or((0, registry_gen));
        registry_gen = new_reg_gen;

        // ── 1. Discover new/removed client windows (only when registry changed) ──
        if ready & graphics::COMPOSITOR_READY_REGISTRY != 0 {
            if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order) {
                // New windows are pushed to end of Vec (top of z-order).
                // Always focus the topmost visible window so appbar selection
                // matches the visually foregrounded window.
                update_kernel_z_order(&windows);
                focused_win = next_visible_window(&windows, 0);
                compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text);
                full_redraw = true;
            }
        }

        // ── 2. Poll stdin (non-blocking) — keyboard arrives via stdin, not kernel events ──
        poll_fds[0].revents = 0;
        let _ = io::poll(&mut poll_fds, 0);

        // ── 3. Process keyboard input ──
        if poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0 {
            if let Ok(n) = io::read(Fd::from_raw(0), &mut read_buf) {
                for i in 0..n {
                    if let Some(event) = input_parser.feed(read_buf[i]) {
                        match event {
                            InputEvent::FunctionKey(k) if (k as usize) <= windows.len() => {
                                let new_focus = (k - 1) as usize;
                                if new_focus < windows.len() && new_focus != focused_win {
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                    focused_win = new_focus;
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                                    compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text);
                                    full_redraw = true;
                                }
                            }
                            InputEvent::Key { ascii, keycode, modifiers } => {
                                if !windows.is_empty() {
                                    let win_event = WindowInputEvent {
                                        event_type: input_event_type::KEY_PRESS,
                                        keycode,
                                        mouse_x: ascii as i16,
                                        mouse_y: 0,
                                        modifiers,
                                        _pad: 0,
                                    };
                                    route_keyboard_to_focused(&windows, focused_win, &win_event);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Dirty rect tracking — initialized before mouse processing so drag
        // can expand the dirty region. Used by section 5 (client blit) and 6 (composite).
        let mut dirty_x0 = i32::MAX;
        let mut dirty_y0 = i32::MAX;
        let mut dirty_x1 = 0i32;
        let mut dirty_y1 = 0i32;

        // ── 4. Process mouse input (only when mouse changed) ──
        let mut mouse_moved_this_frame = false;
        if ready & graphics::COMPOSITOR_READY_MOUSE != 0 {
            if let Ok((mx, my, buttons)) = graphics::mouse_state() {
                let new_mx = mx as i32;
                let new_my = my as i32;
                let mouse_moved = new_mx != mouse_x || new_my != mouse_y;
                mouse_moved_this_frame = mouse_moved;

                if mouse_moved {
                    mouse_x = new_mx;
                    mouse_y = new_my;

                    if let Some((win_idx, off_x, off_y)) = dragging {
                        let new_x = mouse_x - off_x;
                        // Clamp drag to stay below taskbar
                        let new_y = (mouse_y - off_y).max(TASKBAR_HEIGHT as i32);
                        if new_x != windows[win_idx].x || new_y != windows[win_idx].y {
                            // Capture old bounds before moving
                            let (ox0, oy0, ox1, oy1) = windows[win_idx].bounds();
                            windows[win_idx].x = new_x;
                            windows[win_idx].y = new_y;
                            // Update kernel window position for GPU compositing
                            if windows[win_idx].window_id != 0 {
                                let cx = windows[win_idx].content_x();
                                let cy = windows[win_idx].content_y();
                                let _ = graphics::set_window_position(windows[win_idx].window_id, cx, cy, win_idx as u32);
                            }
                            // Dirty region = union of old and new bounds
                            let (nx0, ny0, nx1, ny1) = windows[win_idx].bounds();
                            let dr_x0 = ox0.min(nx0).max(0) as usize;
                            let dr_y0 = oy0.min(ny0).max(0) as usize;
                            let dr_x1 = ox1.max(nx1) as usize;
                            let dr_y1 = oy1.max(ny1) as usize;
                            compose_partial_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, dr_x0, dr_y0, dr_x1, dr_y1);
                            // Use partial dirty rect instead of full_redraw
                            dirty_x0 = dirty_x0.min(dr_x0 as i32);
                            dirty_y0 = dirty_y0.min(dr_y0 as i32);
                            dirty_x1 = dirty_x1.max(dr_x1 as i32);
                            dirty_y1 = dirty_y1.max(dr_y1 as i32);
                            content_dirty = true;
                        }
                    } else if !windows.is_empty() && focused_win < windows.len()
                        && !windows[focused_win].minimized
                        && windows[focused_win].hit_content(mouse_x, mouse_y)
                    {
                        let local_x = (mouse_x - windows[focused_win].content_x()) as i16;
                        let local_y = (mouse_y - windows[focused_win].content_y()) as i16;
                        route_mouse_move_to_focused(&windows, focused_win, local_x, local_y);
                    }
                }

                // Release: end drag or route release event.
                // Per-endpoint button tracking in the kernel prevents dual USB HID
                // endpoints from racing (one endpoint can't cancel the other's press).
                if (buttons & 1) == 0 && (prev_buttons & 1) != 0 {
                    if dragging.is_some() {
                        dragging = None;
                    } else if !windows.is_empty() && focused_win < windows.len()
                        && !windows[focused_win].minimized
                        && windows[focused_win].hit_content(mouse_x, mouse_y)
                    {
                        let local_x = (mouse_x - windows[focused_win].content_x()) as i16;
                        let local_y = (mouse_y - windows[focused_win].content_y()) as i16;
                        route_mouse_button_to_focused(&windows, focused_win, 1, false, local_x, local_y);
                    }
                }

                // Click: focus change + drag or route click to client
                let new_click = (buttons & 1) != 0 && (prev_buttons & 1) == 0;
                prev_buttons = buttons;

                if new_click {
                    // Check taskbar area — ignore clicks
                    if mouse_y < TASKBAR_HEIGHT as i32 {
                        // Taskbar click — no action
                    }
                    // Check appbar area
                    else if mouse_y >= (screen_h - APPBAR_HEIGHT) as i32 {
                        if let Some(idx) = appbar_hit_test(&windows, screen_w, screen_h, mouse_x, mouse_y) {
                            if idx == focused_win && !windows[idx].minimized {
                                // Click focused window button → minimize
                                windows[idx].minimized = true;
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                focused_win = next_visible_window(&windows, focused_win);
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                            } else {
                                // Click unfocused/minimized → restore and focus
                                windows[idx].minimized = false;
                                // Bring to top of z-order
                                if idx < windows.len() - 1 {
                                    let win = windows.remove(idx);
                                    windows.push(win);
                                    update_kernel_z_order(&windows);
                                }
                                let top = windows.len() - 1;
                                if top != focused_win {
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                    focused_win = top;
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                                } else {
                                    focused_win = top;
                                }
                            }
                            compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text);
                            full_redraw = true;
                        }
                    }
                    // Check windows (existing logic with chrome button additions)
                    else if !windows.is_empty() {
                        let mut clicked_idx: Option<usize> = None;
                        let mut clicked_title = false;
                        for i in (0..windows.len()).rev() {
                            if windows[i].minimized { continue; }
                            let ht = windows[i].hit_title(mouse_x, mouse_y);
                            let ha = windows[i].hit_any(mouse_x, mouse_y);
                            if ht || ha {
                                clicked_idx = Some(i);
                                clicked_title = ht;
                                break;
                            }
                        }
                        if let Some(ci) = clicked_idx {
                            let z_changed = ci < windows.len() - 1;
                            if z_changed {
                                let win = windows.remove(ci);
                                windows.push(win);
                                update_kernel_z_order(&windows);
                            }
                            let top = windows.len() - 1;
                            let focus_changed = top != focused_win;

                            if focus_changed {
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                focused_win = top;
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                            } else {
                                focused_win = top;
                            }

                            if clicked_title {
                                // Check chrome buttons before starting drag
                                if windows[top].hit_close_button(mouse_x, mouse_y) {
                                    // Send CLOSE_REQUESTED to client
                                    let close_event = WindowInputEvent {
                                        event_type: input_event_type::CLOSE_REQUESTED,
                                        keycode: 0, mouse_x: 0, mouse_y: 0, modifiers: 0, _pad: 0,
                                    };
                                    let _ = graphics::write_window_input(windows[top].window_id, &close_event);
                                    // Send SIGTERM to owner process
                                    if windows[top].owner_pid > 0 {
                                        let _ = signal::kill(windows[top].owner_pid as i32, signal::SIGTERM);
                                    }
                                } else if windows[top].hit_minimize_button(mouse_x, mouse_y) {
                                    windows[top].minimized = true;
                                    if focused_win == top {
                                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                        focused_win = next_visible_window(&windows, focused_win);
                                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                                    }
                                    compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text);
                                    full_redraw = true;
                                } else {
                                    dragging = Some((top, mouse_x - windows[top].x, mouse_y - windows[top].y));
                                }
                            } else if windows[top].hit_content(mouse_x, mouse_y) {
                                let local_x = (mouse_x - windows[top].content_x()) as i16;
                                let local_y = (mouse_y - windows[top].content_y()) as i16;
                                route_mouse_button_to_focused(&windows, focused_win, 1, true, local_x, local_y);
                            }

                            // Full redraw for z-order or focus change (unless minimize
                            // already did it, or nothing visual changed)
                            if !full_redraw && (z_changed || focus_changed) {
                                compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text);
                                full_redraw = true;
                            }
                        }
                    }
                }
            }
        }

        // ── 5. Window content handled by GPU ──
        // Per-window textures are uploaded by the kernel directly from MAP_SHARED
        // pages and composited via VirGL SUBMIT_3D with z-order interleaved
        // frame strips. No CPU blit needed. Mark windows_dirty so the composite
        // syscall triggers per-window GPU upload + render WITHOUT re-uploading
        // the full COMPOSITE_TEX (which contains only frames/decorations).
        if ready & graphics::COMPOSITOR_READY_DIRTY != 0 {
            windows_dirty = true;
        }

        // ── 5b. Update clock (once per second) ──
        // Only check realtime every 30 frames (~5-6 checks/sec at 200 FPS)
        frame_counter = frame_counter.wrapping_add(1);
        if frame_counter % 30 == 0 {
            if let Ok(ts) = libbreenix::time::now_realtime() {
                if ts.tv_sec != last_clock_sec {
                    last_clock_sec = ts.tv_sec;
                    format_clock(ts.tv_sec, &mut clock_text);
                    draw_taskbar(&mut fb, &clock_text);
                    dirty_x0 = 0;
                    dirty_y0 = 0;
                    dirty_x1 = dirty_x1.max(screen_w as i32);
                    dirty_y1 = dirty_y1.max(TASKBAR_HEIGHT as i32);
                    content_dirty = true;
                }
            }
        }

        // ── 6. Composite to GPU (only when something changed) ──
        let (cbuf, cw, ch): (&[u32], u32, u32) = if direct_mapped {
            (&[], 0, 0)
        } else {
            (&composite_buf, screen_w as u32, screen_h as u32)
        };
        if full_redraw {
            let _ = graphics::virgl_composite_windows_rect(
                cbuf, cw, ch,
                1, 0, 0, screen_w as u32, screen_h as u32,
            );
            full_redraw = false;
            content_dirty = false;
            windows_dirty = false;
        } else if content_dirty {
            let sw = screen_w as i32;
            let sh = screen_h as i32;
            let dx = dirty_x0.max(0) as u32;
            let dy = dirty_y0.max(0) as u32;
            let dw = (dirty_x1.min(sw) - dirty_x0.max(0)).max(0) as u32;
            let dh = (dirty_y1.min(sh) - dirty_y0.max(0)).max(0) as u32;
            let _ = graphics::virgl_composite_windows_rect(
                cbuf, cw, ch,
                2, dx, dy, dw, dh,
            );
            content_dirty = false;
            windows_dirty = false;
        } else if windows_dirty || mouse_moved_this_frame {
            // Window content and/or mouse-only update: no COMPOSITE_TEX change,
            // but kernel uploads per-window textures and draws cursor via SUBMIT_3D.
            // dirty_mode=0 tells kernel bg_dirty=false → skip COMPOSITE_TEX upload.
            let _ = graphics::virgl_composite_windows_rect(
                cbuf, cw, ch,
                0, 0, 0, 0, 0,
            );
            windows_dirty = false;
        }
        // No sleep — compositor_wait handles blocking
    }
}
