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
use libgfx::framebuf::FrameBuf;
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
const KEY_UP: u16 = 0x52;
const KEY_DOWN: u16 = 0x51;
const KEY_ENTER: u16 = 0x28;
const KEY_ESCAPE: u16 = 0x29;

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
    AppEntry { name: "Gus Kit",      description: "Drawing app",          binary: b"/bin/guskit\0",      is_gui: true },
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

fn sqrt_approx(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    let bits = x.to_bits();
    let guess = f32::from_bits((bits >> 1) + 0x1FBB4F2E);
    let s = (guess + x / guess) * 0.5;
    (s + x / s) * 0.5
}

fn atan2_approx(y: f32, x: f32) -> f32 {
    let pi: f32 = 3.14159265;
    if x == 0.0 && y == 0.0 { return 0.0; }
    let ax = abs_f32(x);
    let ay = abs_f32(y);
    let mn = if ax < ay { ax } else { ay };
    let mx = if ax > ay { ax } else { ay };
    let a = mn / mx;
    let s = a * a;
    let r = ((-0.0464964749 * s + 0.15931422) * s - 0.327622764) * s * a + a;
    let r = if ay > ax { pi * 0.5 - r } else { r };
    let r = if x < 0.0 { pi - r } else { r };
    if y < 0.0 { -r } else { r }
}

fn sdf_rounded_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let dx = abs_f32(px - cx) - (hw - r);
    let dy = abs_f32(py - cy) - (hh - r);
    let dx_pos = if dx > 0.0 { dx } else { 0.0 };
    let dy_pos = if dy > 0.0 { dy } else { 0.0 };
    let corner_dist = sqrt_approx(dx_pos * dx_pos + dy_pos * dy_pos) - r;
    let inside_max = if dx > dy { dx } else { dy };
    let inside = if inside_max < 0.0 { inside_max } else { 0.0 };
    corner_dist + inside
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
// Raw u32 pixel helpers (BGRA format)
// ---------------------------------------------------------------------------

#[inline]
fn plot_pixel_u32(buf: &mut [u32], bw: usize, bh: usize, x: i32, y: i32, color: u32) {
    if x >= 0 && y >= 0 && (x as usize) < bw && (y as usize) < bh {
        buf[y as usize * bw + x as usize] = color;
    }
}

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
/// never disappears against the dark panel/background.
fn gradient_color(pos: f32) -> u32 {
    // u32 layout: 0xAARRGGBB (alpha high byte, blue low byte)
    const STOPS: [u32; 5] = [
        0xFFFFFFFF, // white       #ffffff
        0xFFA0C8F0, // pale blue   #a0c8f0
        0xFF6090D8, // light blue  #6090d8
        0xFF4070C0, // med blue    #4070c0
        0xFF3060B0, // blue        #3060b0 — always visible vs black & gray
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

fn color_to_bgra(c: Color) -> u32 {
    c.b as u32 | ((c.g as u32) << 8) | ((c.r as u32) << 16) | 0xFF000000
}

/// Draw a filled rounded rectangle into a u32 BGRA buffer.
fn draw_rounded_rect_u32(
    buf: &mut [u32], buf_w: usize, buf_h: usize,
    x: i32, y: i32, w: i32, h: i32, color: u32, radius: i32,
) {
    for row in 0..h {
        for col in 0..w {
            let px = x + col;
            let py = y + row;
            if px < 0 || py < 0 || px >= buf_w as i32 || py >= buf_h as i32 { continue; }

            let in_corner = (col < radius && row < radius)
                || (col >= w - radius && row < radius)
                || (col < radius && row >= h - radius)
                || (col >= w - radius && row >= h - radius);

            if in_corner {
                let cx = if col < radius { radius } else { w - radius };
                let cy = if row < radius { radius } else { h - radius };
                let dx = col - cx;
                let dy = row - cy;
                if dx * dx + dy * dy > radius * radius { continue; }
            }

            buf[py as usize * buf_w + px as usize] = color;
        }
    }
}

/// Draw a small filled circle (badge dot).
fn draw_circle_u32(
    buf: &mut [u32], buf_w: usize, buf_h: usize,
    cx: i32, cy: i32, radius: i32, color: u32,
) {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy <= radius * radius {
                plot_pixel_u32(buf, buf_w, buf_h, cx + dx, cy + dy, color);
            }
        }
    }
}

/// Animated SDF-based rounded-rectangle border with a dark-blue-to-white
/// conic gradient that rotates clockwise like a progress spinner.
///
/// Uses dual-SDF: outer rounded rect (r=12) and inner rounded rect (r=9)
/// to create a clean 3px border ring at the exact window edge.
/// One full revolution every 360 frames (~6 seconds at 60 fps).
fn draw_sinuous_border(
    buf: &mut [u32], buf_w: usize, buf_h: usize,
    lx: i32, ly: i32, lw: i32, lh: i32, frame: u32,
) {
    let pi: f32 = 3.14159265;
    let bw = BORDER_WIDTH;
    let outer_r = CORNER_RADIUS;
    let inner_r = INNER_RADIUS;

    let cx = lx as f32 + lw as f32 * 0.5;
    let cy = ly as f32 + lh as f32 * 0.5;
    let outer_hw = lw as f32 * 0.5;
    let outer_hh = lh as f32 * 0.5;
    let inner_hw = outer_hw - bw;
    let inner_hh = outer_hh - bw;

    // One revolution per 360 frames (~6 sec at 60 fps)
    let rotation = frame as f32 / 360.0;

    // Iterate only the border region. The margin must cover the full corner
    // radius so the rounded corners are fully rendered (not just bw+2).
    let margin = (outer_r as i32) + 1;
    let strips: [(i32, i32, i32, i32); 4] = [
        (lx, ly, lx + lw, ly + margin),                        // top (incl. corners)
        (lx, ly + lh - margin, lx + lw, ly + lh),              // bottom (incl. corners)
        (lx, ly + margin, lx + margin, ly + lh - margin),      // left side
        (lx + lw - margin, ly + margin, lx + lw, ly + lh - margin), // right side
    ];

    for &(sx0, sy0, sx1, sy1) in &strips {
        let y0 = sy0.max(0);
        let y1 = sy1.min(buf_h as i32);
        let x0 = sx0.max(0);
        let x1 = sx1.min(buf_w as i32);
        for py in y0..y1 {
            for px in x0..x1 {
                let fpx = px as f32 + 0.5;
                let fpy = py as f32 + 0.5;

                // Dual SDF: inside outer rect AND outside inner rect
                let d_outer = sdf_rounded_rect(fpx, fpy, cx, cy, outer_hw, outer_hh, outer_r);
                let d_inner = sdf_rounded_rect(fpx, fpy, cx, cy, inner_hw, inner_hh, inner_r);

                // Anti-aliased alpha from both edges
                let outer_a = (0.5 - d_outer).min(1.0).max(0.0);
                let inner_a = (d_inner + 0.5).min(1.0).max(0.0);
                let alpha = outer_a * inner_a;
                if alpha <= 0.0 { continue; }

                // Conic angle → gradient position → rotate
                let angle = atan2_approx(fpx - cx, -(fpy - cy));
                let pos = (angle + pi) / (2.0 * pi);
                // 3 copies of the gradient around the circle so bright
                // arcs appear on every side simultaneously.
                let phase = fmod_pos(pos * 3.0 - rotation, 1.0);
                let color = gradient_color(phase);

                let idx = py as usize * buf_w + px as usize;
                if alpha >= 0.99 {
                    buf[idx] = color;
                } else {
                    let a = (alpha * 256.0) as u32;
                    let dst = buf[idx];
                    let inv = 256 - a;
                    let rb = ((color & 0xFF) * a + (dst & 0xFF) * inv) / 256;
                    let rg = (((color >> 8) & 0xFF) * a + ((dst >> 8) & 0xFF) * inv) / 256;
                    let rr = (((color >> 16) & 0xFF) * a + ((dst >> 16) & 0xFF) * inv) / 256;
                    buf[idx] = rb | (rg << 8) | (rr << 16) | 0xFF000000;
                }
            }
        }
    }
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
// Rendering — split into two phases to avoid mutable aliasing between the
// raw u32 slice and the FrameBuf (both point to the same pixel memory).
// Phase 1 (render_pixels) does all raw u32 writes.
// Phase 2 (render_text) does all FrameBuf-based text rendering.
// ---------------------------------------------------------------------------

fn render_pixels(
    raw: &mut [u32], w: usize, h: usize,
    state: &LauncherState,
    ttf: &mut Option<CachedFont>,
    font_size: f32,
) {
    let (filtered, filtered_count) = get_filtered_apps(&state.query, state.query_len);
    let layout = compute_layout(w, h, filtered_count);

    // Translucent background (corners outside rounded border show through)
    render_background(raw, w, h, state.frame_count);

    // Animated conic-gradient border at the exact window edge
    let bw = BORDER_WIDTH as i32;
    draw_sinuous_border(raw, w, h, 0, 0, w as i32, h as i32, state.frame_count);

    // Panel body (inside the border, with matching inner corner radius)
    let panel_bg = color_to_bgra(PANEL_BG);
    draw_rounded_rect_u32(raw, w, h,
        bw, bw, w as i32 - 2 * bw, h as i32 - 2 * bw, panel_bg, INNER_RADIUS as i32);

    // Search field border (1px)
    let search_border = color_to_bgra(SEARCH_BORDER);
    draw_rounded_rect_u32(raw, w, h,
        layout.search_x - 1, layout.search_y - 1,
        layout.search_w + 2, SEARCH_H + 2, search_border, 8);
    // Search field background
    let search_bg = color_to_bgra(SEARCH_BG);
    draw_rounded_rect_u32(raw, w, h,
        layout.search_x, layout.search_y,
        layout.search_w, SEARCH_H, search_bg, 7);

    // Blinking cursor in search field — compute text height and query width
    let char_h = font_line_height(ttf.as_ref(), font_size);
    let query_str = core::str::from_utf8(&state.query[..state.query_len]).unwrap_or("");
    let query_w = measure_text_ttf(ttf.as_mut(), query_str, font_size);
    let text_y = layout.search_y + (SEARCH_H - char_h) / 2;
    if (state.cursor_blink / 25) % 2 == 0 {
        let cursor_x = layout.search_x + 12 + query_w;
        let cursor_bgra = color_to_bgra(CURSOR_COLOR);
        for dy in 0..char_h {
            plot_pixel_u32(raw, w, h, cursor_x, text_y + dy, cursor_bgra);
            plot_pixel_u32(raw, w, h, cursor_x + 1, text_y + dy, cursor_bgra);
        }
    }

    // App list: row highlights and badge dots
    for idx in 0..layout.visible_count {
        let app = &APPS[filtered[idx]];
        let row_y = layout.list_y + idx as i32 * ITEM_H;

        // Selected highlight
        if idx == state.selected {
            let sel_bg = color_to_bgra(SELECTED_BG_COLOR);
            draw_rounded_rect_u32(raw, w, h,
                layout.x + 8, row_y, layout.w - 16, ITEM_H - 2, sel_bg, 6);
        } else if state.hovered == Some(idx) {
            // Hover highlight
            let hov_bg = color_to_bgra(HOVER_BG);
            draw_rounded_rect_u32(raw, w, h,
                layout.x + 8, row_y, layout.w - 16, ITEM_H - 2, hov_bg, 6);
        }

        // App type badge dot
        let badge_color = if app.is_gui {
            color_to_bgra(GUI_BADGE)
        } else {
            color_to_bgra(CLI_BADGE)
        };
        draw_circle_u32(raw, w, h,
            layout.x + 22, row_y + ITEM_H / 2, 4, badge_color);
    }
}

fn render_text(
    fb: &mut FrameBuf, w: usize, h: usize, state: &LauncherState,
    ttf: &mut Option<CachedFont>, font_size: f32,
) {
    let (filtered, filtered_count) = get_filtered_apps(&state.query, state.query_len);
    let layout = compute_layout(w, h, filtered_count);
    let char_h = font_line_height(ttf.as_ref(), font_size);
    let text_y = layout.search_y + (SEARCH_H - char_h) / 2;

    // Search text or placeholder
    if state.query_len == 0 {
        draw_text_ttf(
            fb, ttf.as_mut(), "Search applications...",
            layout.search_x + 12, text_y,
            font_size, PLACEHOLDER_COLOR,
        );
    } else {
        let query_str = core::str::from_utf8(&state.query[..state.query_len]).unwrap_or("");
        draw_text_ttf(
            fb, ttf.as_mut(), query_str,
            layout.search_x + 12, text_y,
            font_size, TEXT_COLOR,
        );
    }

    // Smaller size for description text
    let desc_size = (font_size * 0.85).max(10.0);

    // App list text
    for idx in 0..layout.visible_count {
        let app = &APPS[filtered[idx]];
        let row_y = layout.list_y + idx as i32 * ITEM_H;

        // App name
        draw_text_ttf(
            fb, ttf.as_mut(), app.name,
            layout.x + 36, row_y + 4,
            font_size, TEXT_COLOR,
        );

        // Description (slightly smaller)
        draw_text_ttf(
            fb, ttf.as_mut(), app.description,
            layout.x + 36, row_y + 20,
            desc_size, DIM_COLOR,
        );
    }

    // Footer hint
    let footer_y = layout.list_y + layout.visible_count as i32 * ITEM_H + 4;
    draw_text_ttf(
        fb, ttf.as_mut(), "Enter to launch | Esc to close",
        layout.x + PADDING, footer_y,
        desc_size, HINT_COLOR,
    );
}

/// Fill window with translucent black — corners outside the rounded border
/// will show through to the dimmed desktop beneath via GPU alpha compositing.
fn render_background(buf: &mut [u32], w: usize, h: usize, _frame: u32) {
    // Fill with panel background color so the animated border ring pops
    // as a bright contrasting element at the window edge. (B8G8R8X8_UNORM
    // ignores alpha, so translucent fills appear as solid black — no good.)
    let bg = color_to_bgra(PANEL_BG); // #2a2d35
    for px in buf[..w * h].iter_mut() {
        *px = bg;
    }
}

// ---------------------------------------------------------------------------
// TTF text helpers (bitmap fallback)
// ---------------------------------------------------------------------------

/// Draw text using TTF font if available, falling back to bitmap.
fn draw_text_ttf(
    fb: &mut FrameBuf,
    ttf: Option<&mut CachedFont>,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: Color,
) {
    if let Some(f) = ttf {
        ttf_font::draw_text(fb, f, text, x, y, size, color);
    } else {
        bitmap_font::draw_text(fb, text.as_bytes(), x as usize, y as usize, color);
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
                Event::MouseButton { button: 1, pressed: true, x, y } => {
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

        // Advance animation state
        state.frame_count = state.frame_count.wrapping_add(1);
        state.cursor_blink = state.cursor_blink.wrapping_add(1);

        // Render — two phases to avoid aliasing the pixel buffer
        // Phase 1: raw u32 pixel ops (background, panel, border, highlights)
        {
            let fb = win.framebuf();
            let ptr = fb.raw_ptr() as *mut u32;
            let raw = unsafe { core::slice::from_raw_parts_mut(ptr, w * h) };
            render_pixels(raw, w, h, &state, &mut ttf_font, font_size);
        }
        // Phase 2: FrameBuf-based text rendering (TTF with bitmap fallback)
        {
            let fb = win.framebuf();
            render_text(fb, w, h, &state, &mut ttf_font, font_size);
        }
        let _ = win.present();

        // ~60 FPS
        let _ = time::sleep_ms(16);
    }
}
