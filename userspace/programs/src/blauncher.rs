//! blauncher -- Standalone Breenix Quick Launcher
//!
//! A Spotlight/Quicksilver-style application launcher. Creates a fullscreen
//! chromeless window with a dark overlay, centered search panel with animated
//! SDF border, and a filterable app list with mouse hover/click support.
//!
//! Controls:
//!   Type          Filter the app list (case-insensitive substring)
//!   Up/Down       Navigate selection
//!   Enter         Launch selected app
//!   Escape        Dismiss
//!   Click         Select row (double-click launches)
//!   Click outside Dismiss

use std::process;

use breengel::{CachedFont, Window, Event};
use libbreenix::process::{fork, execv, ForkResult};
use libbreenix::time;

use libgfx::bitmap_font;
use libgfx::color::Color;
use libgfx::shapes;
use libgfx::surface::{Rect as Clip, Surface};
use libgfx::ttf_font;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PANEL_W: i32 = 500;
const CORNER_RADIUS: f32 = 12.0;
const BORDER_WIDTH: f32 = 8.0;
const INNER_RADIUS: f32 = 4.0;
const ITEM_H: i32 = 36;
const SEARCH_H: i32 = 44;
const PADDING: i32 = 16;
const MAX_VISIBLE: usize = 12;

// Colors
const PANEL_BG: Color = Color::rgb(42, 45, 53);
const SEARCH_BG: Color = Color::rgb(30, 32, 40);
const SEARCH_BORDER: Color = Color::rgb(62, 66, 80);
const HOVER_BG: Color = Color::rgb(53, 56, 64);
const SELECTED_BG_COLOR: Color = Color::rgb(40, 80, 160);
const TEXT_COLOR: Color = Color::rgb(220, 220, 230);
const DIM_COLOR: Color = Color::rgb(130, 130, 145);
const PLACEHOLDER_COLOR: Color = Color::rgb(100, 100, 115);
const HINT_COLOR: Color = Color::rgb(80, 78, 90);
const CURSOR_COLOR: Color = Color::rgb(220, 220, 230);
const GUI_BADGE: Color = Color::rgb(60, 200, 80);
const CLI_BADGE: Color = Color::rgb(60, 120, 220);

// Keycodes (USB HID)
const KEY_UP: u32 = breengel::keys::UP;
const KEY_DOWN: u32 = breengel::keys::DOWN;
const KEY_ENTER: u32 = breengel::keys::ENTER;
const KEY_ESCAPE: u32 = breengel::keys::ESC;

// ---------------------------------------------------------------------------
// App catalog
// ---------------------------------------------------------------------------

struct AppEntry {
    name: &'static str,
    description: &'static str,
    binary: &'static [u8],
    is_gui: bool,
}

const APPS: &[AppEntry] = &[
    AppEntry { name: "Terminal",     description: "Terminal emulator",     binary: b"/bin/bterm\0",       is_gui: true },
    AppEntry { name: "System Check", description: "System health",        binary: b"/bin/bcheck\0",      is_gui: true },
    AppEntry { name: "Log Viewer",   description: "View system logs",     binary: b"/bin/bless\0",       is_gui: true },
    AppEntry { name: "Bounce",       description: "Physics demo",         binary: b"/bin/bounce\0",      is_gui: true },
    AppEntry { name: "Breenix Log",  description: "System log viewer",    binary: b"/bin/blog\0",        is_gui: true },
    AppEntry { name: "Font Picker",  description: "Configure fonts",      binary: b"/bin/bfontpicker\0", is_gui: true },
    AppEntry { name: "Shell",        description: "Breenix shell",        binary: b"bsh\0",              is_gui: false },
    AppEntry { name: "URL Fetch",    description: "Fetch URLs",           binary: b"burl\0",             is_gui: false },
    AppEntry { name: "cat",          description: "Display file contents", binary: b"cat\0",             is_gui: false },
    AppEntry { name: "ls",           description: "List directory",       binary: b"ls\0",               is_gui: false },
    AppEntry { name: "echo",         description: "Print text",           binary: b"echo\0",             is_gui: false },
    AppEntry { name: "ps",           description: "List processes",       binary: b"ps\0",               is_gui: false },
];

// ---------------------------------------------------------------------------
// no_std math helpers
// ---------------------------------------------------------------------------

fn abs_f32(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

fn floor_f32(x: f32) -> f32 {
    let i = x as i32;
    if (x < 0.0) && ((i as f32) != x) { (i - 1) as f32 } else { i as f32 }
}

fn fmod_pos(x: f32, m: f32) -> f32 {
    let r = x - floor_f32(x / m) * m;
    if r < 0.0 { r + m } else { r }
}

// ---------------------------------------------------------------------------
// Monotonic clock
// ---------------------------------------------------------------------------

fn clock_ms() -> u64 {
    time::now_monotonic()
        .map(|ts| ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

fn to_lower(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' { b + 32 } else { b }
}

fn matches_query(text: &str, query: &[u8], query_len: usize) -> bool {
    if query_len == 0 { return true; }
    let text = text.as_bytes();
    if query_len > text.len() { return false; }
    for start in 0..=(text.len() - query_len) {
        let mut ok = true;
        for i in 0..query_len {
            if to_lower(text[start + i]) != to_lower(query[i]) {
                ok = false;
                break;
            }
        }
        if ok { return true; }
    }
    false
}

fn get_filtered_apps(query: &[u8], query_len: usize) -> ([usize; 16], usize) {
    let mut indices = [0usize; 16];
    let mut count = 0;
    for (i, app) in APPS.iter().enumerate() {
        if count >= 16 { break; }
        if matches_query(app.name, query, query_len)
            || matches_query(app.description, query, query_len)
        {
            indices[count] = i;
            count += 1;
        }
    }
    (indices, count)
}

// ---------------------------------------------------------------------------
// Border gradient
// ---------------------------------------------------------------------------

fn lerp_color(a: u32, b: u32, t: u32) -> u32 {
    let t = if t > 256 { 256 } else { t };
    let inv = 256 - t;
    let rb = ((a & 0xFF) * inv + (b & 0xFF) * t) / 256;
    let rg = (((a >> 8) & 0xFF) * inv + ((b >> 8) & 0xFF) * t) / 256;
    let rr = (((a >> 16) & 0xFF) * inv + ((b >> 16) & 0xFF) * t) / 256;
    rb | (rg << 8) | (rr << 16) | 0xFF000000
}

/// Symmetric conic gradient: white at position 0, medium blue at 0.5.
/// The darkest stop is still a clearly-visible blue so the border
/// does not disappear against the dark panel/background.
fn gradient_color(pos: f32) -> u32 {
    // u32 layout: 0xAARRGGBB (alpha high byte, blue low byte)
    const STOPS: [u32; 5] = [
        0xFFFFFFFF, // white       #ffffff
        0xFFA0C8F0, // pale blue   #a0c8f0
        0xFF6090D8, // light blue  #6090d8
        0xFF4070C0, // med blue    #4070c0
        0xFF3060B0, // blue        #3060b0 — visible against black & gray
    ];
    // Mirror around 0.5 for symmetry
    let p = if pos > 0.5 { 1.0 - pos } else { pos };
    // p in 0..0.5 → scale to 0..4 (4 segments between 5 stops)
    let t = p * 8.0;
    let seg = (t as usize).min(3);
    let frac = t - seg as f32;
    let frac_int = ((frac * 256.0) as u32).min(256);
    lerp_color(STOPS[seg], STOPS[seg + 1], frac_int)
}

fn border_perimeter_pos(px: f32, py: f32, lx: i32, ly: i32, lw: i32, lh: i32) -> f32 {
    let left = lx as f32;
    let top = ly as f32;
    let right = (lx + lw - 1) as f32;
    let bottom = (ly + lh - 1) as f32;

    let dist_top = abs_f32(py - top);
    let dist_right = abs_f32(right - px);
    let dist_bottom = abs_f32(bottom - py);
    let dist_left = abs_f32(px - left);

    let perimeter = ((lw.max(1) + lh.max(1)) * 2) as f32;
    let distance = if dist_top <= dist_right && dist_top <= dist_bottom && dist_top <= dist_left {
        px - left
    } else if dist_right <= dist_bottom && dist_right <= dist_left {
        lw as f32 + (py - top)
    } else if dist_bottom <= dist_left {
        lw as f32 + lh as f32 + (right - px)
    } else {
        lw as f32 + lh as f32 + lw as f32 + (bottom - py)
    };

    fmod_pos(distance / perimeter, 1.0)
}

fn color_of(word: u32) -> Color {
    Color::rgb((word >> 16) as u8, (word >> 8) as u8, word as u8)
}

/// Length of one border segment along the window's edge, in pixels.
const BORDER_SEGMENT: i32 = 8;

/// Animated rounded-rectangle border with a dark-blue-to-white conic gradient that
/// rotates clockwise like a progress spinner, one full revolution every 360 frames (~6
/// seconds at 60 fps). The ring between the window's edge (radius 12) and the panel body
/// (inset 8, radius 4) is drawn in segments of `BORDER_SEGMENT` pixels along the edge, each
/// in the gradient's colour at its centre: the window's rounded rectangle clipped to the
/// segment, which the panel body then covers on the inside.
fn draw_sinuous_border(fb: &mut Surface, lw: i32, lh: i32, frame: u32) {
    let rotation = frame as f32 / 360.0;
    let depth = CORNER_RADIUS as i32;
    let seg = BORDER_SEGMENT;
    // (x, y, w, h) of each segment, clockwise from the top left.
    let mut segments = Vec::new();
    for x in (0..lw).step_by(seg as usize) {
        segments.push((x, 0, seg.min(lw - x), depth));
    }
    for y in (depth..lh - depth).step_by(seg as usize) {
        segments.push((lw - depth, y, depth, seg.min(lh - depth - y)));
    }
    for x in (0..lw).step_by(seg as usize).rev() {
        segments.push((x, lh - depth, seg.min(lw - x), depth));
    }
    for y in (depth..lh - depth).step_by(seg as usize).rev() {
        segments.push((0, y, depth, seg.min(lh - depth - y)));
    }
    fb.batch(|fb| {
        for (x, y, w, h) in segments {
            let (cx, cy) = (x as f32 + w as f32 * 0.5, y as f32 + h as f32 * 0.5);
            let pos = border_perimeter_pos(cx, cy, 0, 0, lw, lh);
            let colour = color_of(gradient_color(fmod_pos(pos - rotation, 1.0)));
            fb.push_clip(Clip::new(x, y, w, h));
            shapes::rounded_rect(fb, 0, 0, lw, lh, CORNER_RADIUS as i32, colour);
            fb.pop_clip();
        }
    });
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct LauncherState {
    query: [u8; 64],
    query_len: usize,
    selected: usize,
    hovered: Option<usize>,
    frame_count: u32,
    cursor_blink: u32,
    last_click_ms: u64,
    last_click_idx: usize,
}

impl LauncherState {
    fn new() -> Self {
        Self {
            query: [0; 64],
            query_len: 0,
            selected: 0,
            hovered: None,
            frame_count: 0,
            cursor_blink: 0,
            last_click_ms: 0,
            last_click_idx: usize::MAX,
        }
    }
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

struct PanelLayout {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    search_x: i32,
    search_y: i32,
    search_w: i32,
    list_y: i32,
    visible_count: usize,
}

fn compute_layout(win_w: usize, _win_h: usize, filtered_count: usize) -> PanelLayout {
    let visible = filtered_count.min(MAX_VISIBLE);
    let panel_h = PADDING + SEARCH_H + 8 + (visible as i32 * ITEM_H) + 24 + PADDING;
    // Panel centered horizontally in the window, at the top
    let panel_x = (win_w as i32 - PANEL_W) / 2;
    let panel_y = 0;
    let search_x = panel_x + PADDING;
    let search_y = panel_y + PADDING;
    let search_w = PANEL_W - PADDING * 2;
    let list_y = search_y + SEARCH_H + 8;

    PanelLayout {
        x: panel_x,
        y: panel_y,
        w: PANEL_W,
        h: panel_h,
        search_x,
        search_y,
        search_w,
        list_y,
        visible_count: visible,
    }
}

// ---------------------------------------------------------------------------
// Rendering: the shapes, then the text, which the shapes do not overlap.
// ---------------------------------------------------------------------------

fn render(
    fb: &mut Surface, w: usize, h: usize,
    state: &LauncherState,
    ttf: &mut Option<CachedFont>,
    font_size: f32,
) {
    let (filtered, filtered_count) = get_filtered_apps(&state.query, state.query_len);
    let layout = compute_layout(w, h, filtered_count);

    // Panel background color everywhere, so the animated border ring pops as a bright
    // contrasting element at the window edge.
    fb.clear(PANEL_BG);

    // Animated border at the window edge, then the panel body (inside the border, with
    // matching inner corner radius) over its inner side.
    draw_sinuous_border(fb, w as i32, h as i32, state.frame_count);
    let bw = BORDER_WIDTH as i32;
    shapes::rounded_rect(fb, bw, bw, w as i32 - 2 * bw, h as i32 - 2 * bw, INNER_RADIUS as i32, PANEL_BG);

    // Search field border (1px) and background
    shapes::rounded_rect(fb, layout.search_x - 1, layout.search_y - 1,
        layout.search_w + 2, SEARCH_H + 2, 8, SEARCH_BORDER);
    shapes::rounded_rect(fb, layout.search_x, layout.search_y,
        layout.search_w, SEARCH_H, 7, SEARCH_BG);

    // Blinking cursor in search field — compute text height and query width
    let char_h = font_line_height(ttf.as_ref(), font_size);
    let query_str = core::str::from_utf8(&state.query[..state.query_len]).unwrap_or("");
    let query_w = measure_text_ttf(ttf.as_mut(), query_str, font_size);
    let text_y = layout.search_y + (SEARCH_H - char_h) / 2;
    if (state.cursor_blink / 25) % 2 == 0 {
        let cursor_x = layout.search_x + 12 + query_w;
        shapes::fill_rect(fb, cursor_x, text_y, 2, char_h, CURSOR_COLOR);
    }

    // App list: row highlights and badge dots
    for idx in 0..layout.visible_count {
        let app = &APPS[filtered[idx]];
        let row_y = layout.list_y + idx as i32 * ITEM_H;

        if idx == state.selected {
            shapes::rounded_rect(fb, layout.x + 8, row_y, layout.w - 16, ITEM_H - 2, 6, SELECTED_BG_COLOR);
        } else if state.hovered == Some(idx) {
            shapes::rounded_rect(fb, layout.x + 8, row_y, layout.w - 16, ITEM_H - 2, 6, HOVER_BG);
        }

        // App type badge dot
        let badge = if app.is_gui { GUI_BADGE } else { CLI_BADGE };
        shapes::fill_circle(fb, layout.x + 22, row_y + ITEM_H / 2, 4, badge);
    }

    let mut text = Text::default();

    // Search text or placeholder
    if state.query_len == 0 {
        text.push("Search applications...", layout.search_x + 12, text_y, font_size, PLACEHOLDER_COLOR);
    } else {
        text.push(query_str, layout.search_x + 12, text_y, font_size, TEXT_COLOR);
    }

    // Smaller size for description text
    let desc_size = (font_size * 0.85).max(10.0);

    // App list text
    for idx in 0..layout.visible_count {
        let app = &APPS[filtered[idx]];
        let row_y = layout.list_y + idx as i32 * ITEM_H;
        text.push(app.name, layout.x + 36, row_y + 4, font_size, TEXT_COLOR);
        text.push(app.description, layout.x + 36, row_y + 20, desc_size, DIM_COLOR);
    }

    // Footer hint
    let footer_y = layout.list_y + layout.visible_count as i32 * ITEM_H + 4;
    text.push("Enter to launch | Esc to close", layout.x + PADDING, footer_y, desc_size, HINT_COLOR);
    text.finish(fb, ttf.as_mut());
}

// ---------------------------------------------------------------------------
// TTF text helpers (bitmap fallback)
// ---------------------------------------------------------------------------

/// A frame's text: each string as `ttf_font::draw_text` draws it (or the bitmap font without
/// TrueType), with the strings' glyphs sharing draws.
#[derive(Default)]
struct Text<'a> {
    strings: Vec<(&'a str, i32, i32, f32, Color)>,
}

impl<'a> Text<'a> {
    fn push(&mut self, text: &'a str, x: i32, y: i32, size: f32, color: Color) {
        self.strings.push((text, x, y, size, color));
    }

    fn finish(self, fb: &mut Surface, ttf: Option<&mut CachedFont>) {
        if let Some(f) = ttf {
            let clip = Clip::new(0, 0, fb.width() as i32, fb.height() as i32);
            let runs: Vec<ttf_font::TextRun<'_>> = self.strings.iter().map(|(text, x, y, size, fg)| ttf_font::TextRun {
                bytes: text.as_bytes(), x: *x, y: *y, size: *size, fg: *fg, clip,
            }).collect();
            ttf_font::draw_runs(fb, f, &runs);
        } else {
            for (text, x, y, _, fg) in self.strings {
                bitmap_font::draw_text(fb, text.as_bytes(), x as usize, y as usize, fg);
            }
        }
    }
}

/// Measure text width using TTF font if available, falling back to bitmap.
fn measure_text_ttf(ttf: Option<&mut CachedFont>, text: &str, size: f32) -> i32 {
    if let Some(f) = ttf {
        ttf_font::text_width(f, text, size)
    } else {
        let m = bitmap_font::metrics();
        (text.len() * m.char_width) as i32
    }
}

/// Get the line height for the current font configuration.
fn font_line_height(ttf: Option<&CachedFont>, size: f32) -> i32 {
    if let Some(f) = ttf {
        let m = f.metrics(size);
        m.line_height as i32
    } else {
        bitmap_font::metrics().char_height as i32
    }
}

// ---------------------------------------------------------------------------
// App launching
// ---------------------------------------------------------------------------

fn launch_app(app: &AppEntry) {
    if app.is_gui {
        match fork() {
            Ok(ForkResult::Child) => {
                let argv: [*const u8; 2] = [app.binary.as_ptr(), core::ptr::null()];
                let _ = execv(app.binary, argv.as_ptr());
                libbreenix::process::exit(1);
            }
            Ok(ForkResult::Parent(_)) => {}
            Err(_) => {}
        }
    } else {
        // CLI app: just launch bterm
        match fork() {
            Ok(ForkResult::Child) => {
                let path = b"/bin/bterm\0";
                let argv: [*const u8; 2] = [path.as_ptr(), core::ptr::null()];
                let _ = execv(path, argv.as_ptr());
                libbreenix::process::exit(1);
            }
            Ok(ForkResult::Parent(_)) => {}
            Err(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------

/// Returns the app list row index under the given screen coordinates,
/// or None if the point is outside the list area.
fn hit_test_row(
    x: i32, y: i32,
    layout: &PanelLayout,
) -> Option<usize> {
    if x < layout.x + 8 || x >= layout.x + layout.w - 8 {
        return None;
    }
    if y < layout.list_y || y >= layout.list_y + layout.visible_count as i32 * ITEM_H {
        return None;
    }
    let row = ((y - layout.list_y) / ITEM_H) as usize;
    if row < layout.visible_count {
        Some(row)
    } else {
        None
    }
}

/// Returns true if the point is inside the panel bounds.
fn point_in_panel(x: i32, y: i32, layout: &PanelLayout) -> bool {
    x >= layout.x && x < layout.x + layout.w
        && y >= layout.y && y < layout.y + layout.h
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    println!("[blauncher] starting");

    // Panel-sized chromeless window (dimmer provides desktop darkening)
    let panel_w = PANEL_W as u32 + 2 * PADDING as u32;
    let panel_max_h = (PADDING + SEARCH_H + 8 + (MAX_VISIBLE as i32 * ITEM_H) + 24 + PADDING) as u32;

    // Create chromeless window (title prefix \x01), BWM will center it
    let mut win = match Window::new(b"\x01Launcher", panel_w, panel_max_h) {
        Ok(w) => w,
        Err(e) => {
            println!("[blauncher] Window::new failed: {}", e);
            process::exit(1);
        }
    };

    // Load display font (TTF) with bitmap fallback
    let mut ttf_font: Option<CachedFont> = win.take_display_font();
    let font_size = if win.display_size() >= 6.0 { win.display_size() } else { 14.0 };
    if ttf_font.is_some() {
        println!("[blauncher] TTF display font loaded, size={}", font_size);
    } else {
        println!("[blauncher] TTF font not available, using bitmap fallback");
    }

    let w = panel_w as usize;
    let h = panel_max_h as usize;

    let mut state = LauncherState::new();
    let start_ms = clock_ms();
    let mut first_frame = true;

    loop {
        // Poll events
        let events = win.poll_events();

        for event in events {
            match event {
                Event::KeyPress { ascii, keycode, .. } => {
                    match keycode {
                        KEY_ESCAPE => {
                            process::exit(0);
                        }
                        KEY_UP => {
                            state.selected = state.selected.saturating_sub(1);
                        }
                        KEY_DOWN => {
                            let (_, filtered_count) = get_filtered_apps(
                                &state.query, state.query_len,
                            );
                            let max_vis = filtered_count.min(MAX_VISIBLE);
                            if max_vis > 0 {
                                state.selected = (state.selected + 1).min(max_vis - 1);
                            }
                        }
                        KEY_ENTER => {
                            let (filtered, filtered_count) = get_filtered_apps(
                                &state.query, state.query_len,
                            );
                            let max_vis = filtered_count.min(MAX_VISIBLE);
                            if max_vis > 0 && state.selected < max_vis {
                                let app_idx = filtered[state.selected];
                                launch_app(&APPS[app_idx]);
                                process::exit(0);
                            }
                        }
                        _ => {
                            if ascii == b'\r' || ascii == b'\n' {
                                // Enter via ASCII
                                let (filtered, filtered_count) = get_filtered_apps(
                                    &state.query, state.query_len,
                                );
                                let max_vis = filtered_count.min(MAX_VISIBLE);
                                if max_vis > 0 && state.selected < max_vis {
                                    let app_idx = filtered[state.selected];
                                    launch_app(&APPS[app_idx]);
                                    process::exit(0);
                                }
                            } else if ascii == 0x1b {
                                // Escape via ASCII
                                process::exit(0);
                            } else if ascii == 8 || ascii == 0x7f {
                                // Backspace
                                if state.query_len > 0 {
                                    state.query_len -= 1;
                                    state.selected = 0;
                                }
                            } else if ascii >= 0x20 && ascii < 0x7f && state.query_len < 63 {
                                // Printable character -> append to query
                                state.query[state.query_len] = ascii;
                                state.query_len += 1;
                                state.selected = 0;
                            }
                        }
                    }
                }
                Event::MouseMove { x, y } => {
                    let (_, filtered_count) = get_filtered_apps(
                        &state.query, state.query_len,
                    );
                    let layout = compute_layout(w, h, filtered_count.min(MAX_VISIBLE));
                    state.hovered = hit_test_row(x, y, &layout);
                }
                Event::MouseButton { button: breengel::buttons::LEFT, pressed: true, x, y } => {
                    let (filtered, filtered_count) = get_filtered_apps(
                        &state.query, state.query_len,
                    );
                    let max_vis = filtered_count.min(MAX_VISIBLE);
                    let layout = compute_layout(w, h, max_vis);

                    if let Some(row) = hit_test_row(x, y, &layout) {
                        let now_ms = clock_ms();

                        // Double-click detection
                        if row == state.last_click_idx
                            && now_ms.saturating_sub(state.last_click_ms) < 400
                        {
                            // Double-click -> launch
                            if row < max_vis {
                                let app_idx = filtered[row];
                                launch_app(&APPS[app_idx]);
                                process::exit(0);
                            }
                        } else {
                            // Single click -> select
                            state.selected = row;
                            state.last_click_ms = now_ms;
                            state.last_click_idx = row;
                        }
                    } else if !point_in_panel(x, y, &layout)
                        && clock_ms().saturating_sub(start_ms) > 500
                    {
                        // Click outside panel -> dismiss (grace period prevents
                        // accidental dismissal from clicks during spawn)
                        process::exit(0);
                    }
                }
                Event::FocusLost => {
                    // Grace period prevents spurious dismiss during startup
                    if clock_ms().saturating_sub(start_ms) > 500 {
                        process::exit(0);
                    }
                }
                Event::CloseRequested => {
                    process::exit(0);
                }
                _ => {}
            }
        }

        let elapsed_ms = clock_ms().saturating_sub(start_ms);
        state.frame_count = (elapsed_ms.saturating_mul(60) / 1000) as u32;
        state.cursor_blink = (elapsed_ms / 16) as u32;

        // Draw the whole frame into the acquired buffer, then publish it.
        match win.surface() {
            Ok(fb) => render(fb, w, h, &state, &mut ttf_font, font_size),
            Err(e) => {
                println!("[blauncher] ERROR: no window surface: {}", e);
                process::exit(1);
            }
        }
        let presented = win.present();
        if first_frame {
            first_frame = false;
            match presented {
                Ok(()) => println!("[blauncher] first frame presented: {}x{}", w, h),
                Err(e) => println!("[blauncher] first frame present failed: {}", e),
            }
        }

        // ~60 FPS
        let _ = time::sleep_ms(16);
    }
}
