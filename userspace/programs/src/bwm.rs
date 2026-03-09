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

// Colors
const TITLE_FOCUSED_BG: Color = Color::rgb(40, 100, 220);
const TITLE_UNFOCUSED_BG: Color = Color::rgb(45, 50, 65);
const TITLE_TEXT: Color = Color::rgb(160, 165, 175);
const TITLE_FOCUSED_TEXT: Color = Color::rgb(255, 255, 255);
const WIN_BORDER_COLOR: Color = Color::rgb(50, 55, 70);
const WIN_BORDER_FOCUSED: Color = Color::rgb(60, 130, 255);
const CONTENT_BG: Color = Color::rgb(20, 25, 40);

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
    /// Cached client pixels for z-order repair (updated on each read_window_buffer)
    pixel_cache: Vec<u32>,
    cache_w: u32,
    cache_h: u32,
}

impl Window {
    fn title_bytes(&self) -> &[u8] { &self.title[..self.title_len] }

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
        (self.x, self.y, self.x + self.width as i32, self.y + self.total_height() as i32)
    }
}

fn rects_overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
    a.0 < b.2 && a.2 > b.0 && a.1 < b.3 && a.3 > b.1
}

// ─── Drawing Helpers ─────────────────────────────────────────────────────────

fn fill_rect(fb: &mut FrameBuf, x: i32, y: i32, w: usize, h: usize, color: Color) {
    for dy in 0..h as i32 {
        let py = y + dy;
        if py < 0 || py >= fb.height as i32 { continue; }
        for dx in 0..w as i32 {
            let px = x + dx;
            if px < 0 || px >= fb.width as i32 { continue; }
            fb.put_pixel(px as usize, py as usize, color);
        }
    }
}

fn draw_text_at(fb: &mut FrameBuf, text: &[u8], x: i32, y: i32, color: Color) {
    if y < 0 || y >= fb.height as i32 { return; }
    for (i, &ch) in text.iter().enumerate() {
        let px = x + (i as i32) * CELL_W as i32;
        if px < 0 || px + CELL_W as i32 > fb.width as i32 { continue; }
        bitmap_font::draw_char(fb, ch as char, px as usize, y as usize, color);
    }
}

/// Draw a floating window frame (border + title bar + content bg)
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
    let text_y = win.y + bw as i32 + ((TITLE_BAR_HEIGHT - bw - CELL_H) / 2) as i32;
    draw_text_at(fb, win.title_bytes(), win.x + 8, text_y, title_fg);
    fill_rect(fb, win.content_x(), win.content_y(),
              win.content_width(), win.content_height(), CONTENT_BG);
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

fn discover_windows(windows: &mut Vec<Window>, screen_w: usize, screen_h: usize) -> bool {
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
        let cascade_x = 30 + (n as i32 * 50) % ((screen_w as i32 - 500).max(100));
        let cascade_y = 30 + (n as i32 * 50) % ((screen_h as i32 - 500).max(100));

        let total_w = info.width as usize + BORDER_WIDTH * 2;
        let total_h = info.height as usize + TITLE_BAR_HEIGHT + BORDER_WIDTH * 2;

        let mut title = [0u8; 32];
        let title_len = (info.title_len as usize).min(32);
        title[..title_len].copy_from_slice(&info.title[..title_len]);

        print!("[bwm] Discovered window '{}' (id={}, {}x{}) at ({},{})\n",
            core::str::from_utf8(&title[..title_len]).unwrap_or("?"),
            info.buffer_id, info.width, info.height, cascade_x, cascade_y);

        windows.push(Window {
            x: cascade_x, y: cascade_y, width: total_w, height: total_h,
            title, title_len, window_id: info.buffer_id,
            pixel_cache: Vec::new(), cache_w: 0, cache_h: 0,
        });
        added = true;
    }

    removed || added
}

// ─── Client Pixel Blitting ──────────────────────────────────────────────────

/// Core pixel blit — direct u32 writes to compositor buffer for speed.
/// Bypasses FrameBuf::put_pixel which does per-pixel bounds checking + color conversion.
fn blit_pixels_to_fb(fb: &mut FrameBuf, win: &Window, src: &[u32], w: usize, h: usize) {
    let cx = win.content_x();
    let cy = win.content_y();
    let cw = win.content_width();
    let ch = win.content_height();
    let pw = w.min(cw);
    let ph = h.min(ch);
    let fb_w = fb.width;
    let fb_h = fb.height;
    // Get raw u32 pointer to compositor buffer
    let fb_ptr = fb.raw_ptr() as *mut u32;
    for row in 0..ph {
        let py = (cy + row as i32) as usize;
        if py >= fb_h { continue; }
        let dst_row_start = py * fb_w;
        let src_row_start = row * w;
        let x_start = cx.max(0) as usize;
        let x_end = ((cx + pw as i32) as usize).min(fb_w);
        let src_offset = if cx < 0 { (-cx) as usize } else { 0 };
        if x_start >= x_end { continue; }
        let count = x_end - x_start;
        let si = src_row_start + src_offset;
        if si + count > src.len() { continue; }
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr().add(si),
                fb_ptr.add(dst_row_start + x_start),
                count,
            );
        }
    }
}

/// Read client window pixels, update cache, and blit to compositor.
/// Returns true if new data was available.
fn blit_client_pixels(fb: &mut FrameBuf, win: &mut Window, buf: &mut [u8]) -> bool {
    let (w, h) = match graphics::read_window_buffer(win.window_id, buf) {
        Ok((w, h)) if w > 0 && h > 0 => (w, h),
        _ => return false,
    };
    let pixel_count = (w as usize) * (h as usize);
    let src = unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u32, pixel_count.min(buf.len() / 4)) };

    // Update pixel cache for z-order repair (reuse allocation)
    if win.pixel_cache.len() != src.len() {
        win.pixel_cache.resize(src.len(), 0);
    }
    win.pixel_cache.copy_from_slice(src);
    win.cache_w = w;
    win.cache_h = h;

    blit_pixels_to_fb(fb, win, src, w as usize, h as usize);
    true
}

/// Re-blit a window's cached pixels (for z-order repair).
fn blit_cached_pixels(fb: &mut FrameBuf, win: &Window) {
    if win.pixel_cache.is_empty() { return; }
    blit_pixels_to_fb(fb, win, &win.pixel_cache, win.cache_w as usize, win.cache_h as usize);
}

/// Redraw all windows in z-order (index 0 = bottom).
/// Uses pixel cache for windows that haven't changed since last read.
fn redraw_all_windows(fb: &mut FrameBuf, windows: &mut [Window], focused_win: usize, client_buf: &mut [u8]) {
    for i in 0..windows.len() {
        draw_window_frame(fb, &windows[i], i == focused_win);
        if windows[i].window_id != 0 {
            if !blit_client_pixels(fb, &mut windows[i], client_buf) {
                // No new data from kernel — use cached pixels
                blit_cached_pixels(fb, &windows[i]);
            }
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
    let (mut composite_buf, direct_mapped) = match graphics::map_compositor_texture() {
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
    let mut client_pixel_buf = vec![0u8; screen_w * screen_h * 4];

    // Initial composite
    if direct_mapped {
        // Data is already in COMPOSITE_TEX — just tell kernel to upload + display
        let _ = graphics::virgl_composite_windows_rect(&[], 0, 0, 1, 0, 0, screen_w as u32, screen_h as u32);
    } else {
        let _ = graphics::virgl_composite_windows(&composite_buf, screen_w as u32, screen_h as u32, true);
    }

    let mut read_buf = [0u8; 512];
    let mut poll_fds = [io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }];

    // Performance tracing — measure time spent in each phase of the main loop
    let mut perf_frame: u64 = 0;
    let mut perf_discover_ns: u64 = 0;
    let mut perf_poll_ns: u64 = 0;
    let mut perf_blit_ns: u64 = 0;
    let mut perf_composite_ns: u64 = 0;
    let mut perf_composites: u64 = 0;
    let mut perf_sleeps: u64 = 0;

    fn mono_ns() -> u64 {
        let ts = libbreenix::time::now_monotonic().unwrap_or_default();
        (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
    }

    loop {
        let t0 = mono_ns();

        // ── 1. Discover new/removed client windows ──
        if discover_windows(&mut windows, screen_w, screen_h) {
            if focused_win >= windows.len() {
                focused_win = windows.len().saturating_sub(1);
            }
            // Restore background from cache (fast memcpy, not gradient computation)
            composite_buf.copy_from_slice(&bg_cache);
            redraw_all_windows(&mut fb, &mut windows, focused_win, &mut client_pixel_buf);
            full_redraw = true;
        }

        let t1 = mono_ns();

        // ── 2. Poll stdin (non-blocking) ──
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
                                    composite_buf.copy_from_slice(&bg_cache);
                                    redraw_all_windows(&mut fb, &mut windows, focused_win, &mut client_pixel_buf);
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

        // ── 4. Process mouse input ──
        if let Ok((mx, my, buttons)) = graphics::mouse_state() {
            let new_mx = mx as i32;
            let new_my = my as i32;
            let mouse_moved = new_mx != mouse_x || new_my != mouse_y;

            if mouse_moved {
                mouse_x = new_mx;
                mouse_y = new_my;

                if let Some((win_idx, off_x, off_y)) = dragging {
                    let new_x = mouse_x - off_x;
                    let new_y = mouse_y - off_y;
                    if new_x != windows[win_idx].x || new_y != windows[win_idx].y {
                        windows[win_idx].x = new_x;
                        windows[win_idx].y = new_y;
                        composite_buf.copy_from_slice(&bg_cache);
                        redraw_all_windows(&mut fb, &mut windows, focused_win, &mut client_pixel_buf);
                        full_redraw = true;
                    }
                } else if !windows.is_empty() && focused_win < windows.len()
                    && windows[focused_win].hit_content(mouse_x, mouse_y)
                {
                    let local_x = (mouse_x - windows[focused_win].content_x()) as i16;
                    let local_y = (mouse_y - windows[focused_win].content_y()) as i16;
                    route_mouse_move_to_focused(&windows, focused_win, local_x, local_y);
                }
            }

            // Release: end drag or route release event
            if (buttons & 1) == 0 && (prev_buttons & 1) != 0 {
                if dragging.is_some() {
                    dragging = None;
                } else if !windows.is_empty() && focused_win < windows.len()
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

            if new_click && !windows.is_empty() {
                let mut clicked_idx: Option<usize> = None;
                let mut clicked_title = false;
                for i in (0..windows.len()).rev() {
                    let ht = windows[i].hit_title(mouse_x, mouse_y);
                    let ha = windows[i].hit_any(mouse_x, mouse_y);
                    if ht || ha {
                        clicked_idx = Some(i);
                        clicked_title = ht;
                        break;
                    }
                }
                if let Some(ci) = clicked_idx {
                    if ci < windows.len() - 1 {
                        let win = windows.remove(ci);
                        windows.push(win);
                    }
                    let top = windows.len() - 1;

                    if top != focused_win {
                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                        focused_win = top;
                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                    } else {
                        focused_win = top;
                    }

                    if clicked_title {
                        dragging = Some((top, mouse_x - windows[top].x, mouse_y - windows[top].y));
                    } else if windows[top].hit_content(mouse_x, mouse_y) {
                        let local_x = (mouse_x - windows[top].content_x()) as i16;
                        let local_y = (mouse_y - windows[top].content_y()) as i16;
                        route_mouse_button_to_focused(&windows, focused_win, 1, true, local_x, local_y);
                    }

                    // Fast background restore + full window redraw
                    composite_buf.copy_from_slice(&bg_cache);
                    redraw_all_windows(&mut fb, &mut windows, focused_win, &mut client_pixel_buf);
                    full_redraw = true;
                }
            }
        }

        let t2 = mono_ns();

        // ── 5. Blit all client window pixels + z-order repair ──
        // Fast inner loop (no per-pixel clipping). After blitting dirty windows,
        // repair z-order by re-blitting cached pixels for higher-z overlapping windows.
        // Track dirty bounding box for partial GPU upload.
        let mut updated = [false; 16];
        let mut dirty_x0 = i32::MAX;
        let mut dirty_y0 = i32::MAX;
        let mut dirty_x1 = 0i32;
        let mut dirty_y1 = 0i32;

        for i in 0..windows.len().min(16) {
            if windows[i].window_id != 0 {
                if blit_client_pixels(&mut fb, &mut windows[i], &mut client_pixel_buf) {
                    content_dirty = true;
                    updated[i] = true;
                    let (bx0, by0, bx1, by1) = windows[i].bounds();
                    dirty_x0 = dirty_x0.min(bx0);
                    dirty_y0 = dirty_y0.min(by0);
                    dirty_x1 = dirty_x1.max(bx1);
                    dirty_y1 = dirty_y1.max(by1);
                }
            }
        }

        // Z-order repair: if a lower-z window got new pixels, re-blit all higher-z
        // windows that overlap with it (using their cached pixels).
        for j in 1..windows.len().min(16) {
            if windows[j].pixel_cache.is_empty() { continue; }
            let jb = windows[j].bounds();
            for i in 0..j {
                if !updated[i] { continue; }
                if rects_overlap(windows[i].bounds(), jb) {
                    draw_window_frame(&mut fb, &windows[j], j == focused_win);
                    blit_cached_pixels(&mut fb, &windows[j]);
                    updated[j] = true; // cascade: treat as updated for even higher-z windows
                    content_dirty = true;
                    let (bx0, by0, bx1, by1) = windows[j].bounds();
                    dirty_x0 = dirty_x0.min(bx0);
                    dirty_y0 = dirty_y0.min(by0);
                    dirty_x1 = dirty_x1.max(bx1);
                    dirty_y1 = dirty_y1.max(by1);
                    break;
                }
            }
        }

        let t3 = mono_ns();

        // ── 6. Composite to GPU (only when something changed) ──
        // When direct_mapped, pass empty pixels (kernel skips Phase A copy —
        // our writes went directly into COMPOSITE_TEX backing memory).
        let (cbuf, cw, ch): (&[u32], u32, u32) = if direct_mapped {
            (&[], 0, 0)
        } else {
            (&composite_buf, screen_w as u32, screen_h as u32)
        };
        if full_redraw {
            // Full upload: entire compositor buffer changed (window add/remove/drag)
            let _ = graphics::virgl_composite_windows_rect(
                cbuf, cw, ch,
                1, 0, 0, screen_w as u32, screen_h as u32,
            );
            full_redraw = false;
            content_dirty = false;
            perf_composites += 1;
        } else if content_dirty {
            // Partial upload: only the dirty sub-region (union of updated window bounds)
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
            perf_composites += 1;
        } else {
            // Nothing dirty — brief sleep to avoid burning CPU
            let _ = libbreenix::time::sleep_ms(2);
            perf_sleeps += 1;
        }

        let t4 = mono_ns();

        // Accumulate phase timings
        perf_discover_ns += t1.saturating_sub(t0);
        perf_poll_ns += t2.saturating_sub(t1);
        perf_blit_ns += t3.saturating_sub(t2);
        perf_composite_ns += t4.saturating_sub(t3);
        perf_frame += 1;

        // Dump perf summary every 500 iterations
        if perf_frame % 500 == 0 {
            let to_us = |ns: u64| -> u64 { ns / 1000 / 500 };
            print!("[bwm-perf] iter={} composites={} sleeps={} avg: discover={}us poll={}us blit={}us composite={}us\n",
                perf_frame, perf_composites, perf_sleeps,
                to_us(perf_discover_ns), to_us(perf_poll_ns),
                to_us(perf_blit_ns), to_us(perf_composite_ns),
            );
            perf_discover_ns = 0;
            perf_poll_ns = 0;
            perf_blit_ns = 0;
            perf_composite_ns = 0;
            perf_composites = 0;
            perf_sleeps = 0;
        }
    }
}
