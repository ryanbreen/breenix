//! Gus Kit — MS Paint-style drawing application for Breenix
//!
//! Features:
//! - Drawing tools: Pencil, Brush, Line, Rectangle, Circle, Fill, Eraser
//! - HSV color picker with hue bar, saturation/value square, recent colors
//! - Variable brush sizes (S/M/L)
//! - BMP file saving to /home/guskit_N.bmp (persists across reboots on ext2)
//! - Real-time collaboration via BCP (Breenix Collaboration Protocol)
//!
//! Collaboration:
//!   guskit --host 7890      # Host a session on port 7890
//!   guskit --join 10.0.2.2:7890  # Join a hosted session
//!
//! Created for Gus!

use std::process;

use breengel::{CachedFont, Event, Window};
use libbreenix::fs::{self, File};
use libbreenix::io::{self, PollFd};
use libbreenix::socket::SockAddrIn;
use libbreenix::time;

use libbui::widget::file_picker::{FileEntry, FilePicker, FilePickerResult};
use libbui::widget::menu_bar::{Menu, MenuAction, MenuBar, MenuEvent, MenuItem, MENU_BAR_HEIGHT};
use libbui::shortcut::Shortcut;
use libbui::Rect as BuiRect;
use libbui::Theme;

use libcollab::{CollabEvent, CollabSession, DrawOp};

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;
use libgfx::ttf_font;

use libimg::bmp;

// ---------------------------------------------------------------------------
// TTF font helper
// ---------------------------------------------------------------------------

/// Draw text using TTF font if available, falling back to bitmap.
fn draw_text_auto(
    fb: &mut FrameBuf,
    ttf: Option<&mut CachedFont>,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: Color,
    bitmap_scale: usize,
) {
    if let Some(f) = ttf {
        ttf_font::draw_text(fb, f, text, x, y, size, color);
    } else {
        font::draw_text(fb, text.as_bytes(), x as usize, y as usize, color, bitmap_scale);
    }
}

// ---------------------------------------------------------------------------
// Menu action constants
// ---------------------------------------------------------------------------

const ACT_NEW: MenuAction = 1;
const ACT_OPEN: MenuAction = 2;
const ACT_SAVE: MenuAction = 3;
const ACT_QUIT: MenuAction = 5;
const ACT_UNDO: MenuAction = 10;
const ACT_REDO: MenuAction = 11;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const TOOLBAR_Y: usize = 22;  // MENU_BAR_HEIGHT as usize
const SIZE_BAR_Y: usize = 40;
const HUE_BAR_Y: usize = 58;
const HUE_BAR_H: usize = 18;
const SV_SQUARE_Y: usize = 76;
const SV_SQUARE_SIZE: usize = 100;
const CANVAS_Y: usize = 176;

const BUTTON_W: usize = 36;
const BUTTON_H: usize = 16;
const BUTTON_PAD: usize = 2;

const COLOR_SWATCH_SIZE: usize = 16;
const RECENT_COLOR_SIZE: usize = 10;

// ---------------------------------------------------------------------------
// Peer cursor colors (unique per peer_id)
// ---------------------------------------------------------------------------

const PEER_COLORS: [Color; 15] = [
    Color::rgb(255, 100, 100), // 1: red
    Color::rgb(100, 200, 255), // 2: light blue
    Color::rgb(100, 255, 100), // 3: green
    Color::rgb(255, 200, 100), // 4: orange
    Color::rgb(200, 100, 255), // 5: purple
    Color::rgb(255, 255, 100), // 6: yellow
    Color::rgb(100, 255, 200), // 7: teal
    Color::rgb(255, 100, 200), // 8: pink
    Color::rgb(200, 255, 100), // 9: lime
    Color::rgb(100, 200, 200), // 10: cyan
    Color::rgb(255, 150, 150), // 11: salmon
    Color::rgb(150, 150, 255), // 12: periwinkle
    Color::rgb(200, 200, 100), // 13: khaki
    Color::rgb(255, 200, 200), // 14: light pink
    Color::rgb(200, 255, 200), // 15: mint
];

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Pencil,
    Brush,
    Line,
    Rect,
    Circle,
    Fill,
    Eraser,
}

const TOOLS: [Tool; 7] = [
    Tool::Pencil,
    Tool::Brush,
    Tool::Line,
    Tool::Rect,
    Tool::Circle,
    Tool::Fill,
    Tool::Eraser,
];

impl Tool {
    fn label(self) -> &'static [u8] {
        match self {
            Tool::Pencil => b"Pen",
            Tool::Brush => b"Brsh",
            Tool::Line => b"Line",
            Tool::Rect => b"Rect",
            Tool::Circle => b"Circ",
            Tool::Fill => b"Fill",
            Tool::Eraser => b"Ers",
        }
    }

    fn is_shape(self) -> bool {
        matches!(self, Tool::Line | Tool::Rect | Tool::Circle)
    }

    fn to_wire_id(self) -> u8 {
        match self {
            Tool::Pencil => 0,
            Tool::Brush => 1,
            Tool::Line => 2,
            Tool::Rect => 3,
            Tool::Circle => 4,
            Tool::Fill => 5,
            Tool::Eraser => 6,
        }
    }
}

// ---------------------------------------------------------------------------
// Brush sizes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum BrushSize {
    Small,
    Medium,
    Large,
}

impl BrushSize {
    fn radius(self) -> i32 {
        match self {
            BrushSize::Small => 2,
            BrushSize::Medium => 5,
            BrushSize::Large => 10,
        }
    }

    fn label(self) -> &'static [u8] {
        match self {
            BrushSize::Small => b"S",
            BrushSize::Medium => b"M",
            BrushSize::Large => b"L",
        }
    }
}

const SIZES: [BrushSize; 3] = [BrushSize::Small, BrushSize::Medium, BrushSize::Large];

// ---------------------------------------------------------------------------
// Remote cursor state
// ---------------------------------------------------------------------------

struct RemoteCursor {
    peer_id: u8,
    x: i16,
    y: i16,
    visible: bool,
    name: [u8; 32],
    name_len: u8,
}

// ---------------------------------------------------------------------------
// Collaboration mode
// ---------------------------------------------------------------------------

enum CollabMode {
    None,
    Host { port: u16 },
    Join { addr: [u8; 4], port: u16 },
}

// ---------------------------------------------------------------------------
// HSV -> RGB conversion
// ---------------------------------------------------------------------------

fn hsv_to_rgb(hue: u16, sat: u8, val: u8) -> Color {
    if sat == 0 {
        return Color::rgb(val, val, val);
    }

    let h = (hue % 360) as u32;
    let s = sat as u32;
    let v = val as u32;

    let sector = h * 6 / 360;
    let f = h * 6 - sector * 360; // 0..359 fractional part scaled by 360

    let p = (v * (255 - s)) / 255;
    let q = (v * (255 - (s * f) / 360)) / 255;
    let t = (v * (255 - (s * (360 - f)) / 360)) / 255;

    let (r, g, b) = match sector {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };

    Color::rgb(r as u8, g as u8, b as u8)
}

// ---------------------------------------------------------------------------
// Canvas pixel helpers
// ---------------------------------------------------------------------------

fn canvas_get(canvas: &[u8], w: usize, x: usize, y: usize) -> Color {
    let i = (y * w + x) * 3;
    Color::rgb(canvas[i], canvas[i + 1], canvas[i + 2])
}

fn canvas_set(canvas: &mut [u8], w: usize, x: usize, y: usize, c: Color) {
    let i = (y * w + x) * 3;
    canvas[i] = c.r;
    canvas[i + 1] = c.g;
    canvas[i + 2] = c.b;
}

// ---------------------------------------------------------------------------
// Canvas drawing operations (write to canvas buffer, not framebuf)
// ---------------------------------------------------------------------------

fn canvas_put_pixel(canvas: &mut [u8], w: usize, h: usize, x: i32, y: i32, color: Color) {
    if x >= 0 && (x as usize) < w && y >= 0 && (y as usize) < h {
        canvas_set(canvas, w, x as usize, y as usize, color);
    }
}

fn canvas_fill_circle(canvas: &mut [u8], w: usize, h: usize, cx: i32, cy: i32, r: i32, color: Color) {
    for dy in -r..=r {
        let dx_max_sq = (r as i64) * (r as i64) - (dy as i64) * (dy as i64);
        if dx_max_sq < 0 {
            continue;
        }
        let dx_max = isqrt(dx_max_sq as u64) as i32;
        let y = cy + dy;
        if y < 0 || y >= h as i32 {
            continue;
        }
        let x_start = (cx - dx_max).max(0) as usize;
        let x_end = ((cx + dx_max) as usize).min(w - 1);
        for x in x_start..=x_end {
            canvas_set(canvas, w, x, y as usize, color);
        }
    }
}

fn canvas_fill_rect(canvas: &mut [u8], w: usize, h: usize, x: i32, y: i32, rw: i32, rh: i32, color: Color) {
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = ((x + rw) as usize).min(w);
    let y1 = ((y + rh) as usize).min(h);
    for py in y0..y1 {
        for px in x0..x1 {
            canvas_set(canvas, w, px, py, color);
        }
    }
}

fn canvas_draw_line(canvas: &mut [u8], w: usize, h: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut cx = x0;
    let mut cy = y0;

    loop {
        canvas_put_pixel(canvas, w, h, cx, cy, color);
        if cx == x1 && cy == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
    }
}

/// Bresenham line for brush stamps along the path
fn canvas_brush_line(canvas: &mut [u8], w: usize, h: usize, x0: i32, y0: i32, x1: i32, y1: i32, radius: i32, color: Color) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut cx = x0;
    let mut cy = y0;

    loop {
        canvas_fill_circle(canvas, w, h, cx, cy, radius, color);
        if cx == x1 && cy == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
    }
}

/// Queue-based flood fill (iterative BFS)
fn canvas_flood_fill(canvas: &mut [u8], w: usize, h: usize, sx: i32, sy: i32, color: Color) {
    if sx < 0 || sx >= w as i32 || sy < 0 || sy >= h as i32 {
        return;
    }
    let target = canvas_get(canvas, w, sx as usize, sy as usize);
    if target.r == color.r && target.g == color.g && target.b == color.b {
        return;
    }

    let mut queue = Vec::new();
    queue.push((sx, sy));
    canvas_set(canvas, w, sx as usize, sy as usize, color);

    while let Some((x, y)) = queue.pop() {
        let neighbors: [(i32, i32); 4] = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)];
        for (nx, ny) in neighbors {
            if nx >= 0 && nx < w as i32 && ny >= 0 && ny < h as i32 {
                let c = canvas_get(canvas, w, nx as usize, ny as usize);
                if c.r == target.r && c.g == target.g && c.b == target.b {
                    canvas_set(canvas, w, nx as usize, ny as usize, color);
                    queue.push((nx, ny));
                }
            }
        }
    }
}

fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ---------------------------------------------------------------------------
// Apply a DrawOp to the canvas (used for both local and remote ops)
// ---------------------------------------------------------------------------

fn apply_draw_op(canvas: &mut [u8], cw: usize, ch: usize, op: &DrawOp) {
    match op {
        DrawOp::Pencil { x0, y0, x1, y1, r, g, b } => {
            let color = Color::rgb(*r, *g, *b);
            canvas_draw_line(canvas, cw, ch, *x0 as i32, *y0 as i32, *x1 as i32, *y1 as i32, color);
        }
        DrawOp::Brush { x0, y0, x1, y1, radius, r, g, b } => {
            let color = Color::rgb(*r, *g, *b);
            canvas_brush_line(canvas, cw, ch, *x0 as i32, *y0 as i32, *x1 as i32, *y1 as i32, *radius as i32, color);
        }
        DrawOp::Eraser { x0, y0, x1, y1, radius } => {
            canvas_brush_line(canvas, cw, ch, *x0 as i32, *y0 as i32, *x1 as i32, *y1 as i32, *radius as i32, Color::WHITE);
        }
        DrawOp::Line { x0, y0, x1, y1, r, g, b } => {
            let color = Color::rgb(*r, *g, *b);
            canvas_draw_line(canvas, cw, ch, *x0 as i32, *y0 as i32, *x1 as i32, *y1 as i32, color);
        }
        DrawOp::Rect { x, y, w, h, r, g, b } => {
            let color = Color::rgb(*r, *g, *b);
            canvas_fill_rect(canvas, cw, ch, *x as i32, *y as i32, *w as i32, *h as i32, color);
        }
        DrawOp::Circle { cx, cy, radius, r, g, b } => {
            let color = Color::rgb(*r, *g, *b);
            canvas_fill_circle(canvas, cw, ch, *cx as i32, *cy as i32, *radius as i32, color);
        }
        DrawOp::Fill { x, y, r, g, b } => {
            let color = Color::rgb(*r, *g, *b);
            canvas_flood_fill(canvas, cw, ch, *x as i32, *y as i32, color);
        }
        DrawOp::Clear => {
            for byte in canvas.iter_mut() {
                *byte = 255;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Blit canvas to framebuffer
// ---------------------------------------------------------------------------

fn blit_canvas(fb: &mut FrameBuf, canvas: &[u8], cw: usize, ch: usize) {
    let ptr = fb.raw_ptr();
    let stride = fb.stride;
    let bpp = fb.bpp;
    let is_bgr = fb.is_bgr;

    for y in 0..ch {
        let fb_y = y + CANVAS_Y;
        if fb_y >= fb.height {
            break;
        }
        let row = fb_y * stride;
        let src_row = y * cw * 3;
        for x in 0..cw {
            if x >= fb.width {
                break;
            }
            let si = src_row + x * 3;
            let (r, g, b) = (canvas[si], canvas[si + 1], canvas[si + 2]);
            let o = row + x * bpp;
            let (c0, c1, c2) = if is_bgr { (b, g, r) } else { (r, g, b) };
            unsafe {
                *ptr.add(o) = c0;
                *ptr.add(o + 1) = c1;
                *ptr.add(o + 2) = c2;
                if bpp == 4 {
                    *ptr.add(o + 3) = 0;
                }
            }
        }
    }
    fb.mark_dirty(0, CANVAS_Y as i32, cw as i32, ch as i32);
}

// ---------------------------------------------------------------------------
// UI Drawing
// ---------------------------------------------------------------------------

fn draw_button(fb: &mut FrameBuf, x: usize, y: usize, w: usize, label: &[u8], selected: bool) {
    let bg = if selected { Color::rgb(100, 140, 220) } else { Color::rgb(60, 60, 60) };
    let fg = Color::WHITE;
    shapes::fill_rect(fb, x as i32, y as i32, w as i32, BUTTON_H as i32, bg);
    shapes::draw_rect(fb, x as i32, y as i32, w as i32, BUTTON_H as i32, Color::rgb(120, 120, 120));
    let tw = font::text_width(label, 1);
    let tx = x + (w.saturating_sub(tw)) / 2;
    let ty = y + (BUTTON_H.saturating_sub(7)) / 2;
    font::draw_text(fb, label, tx, ty, fg, 1);
}

fn draw_toolbar(fb: &mut FrameBuf, current_tool: Tool, _width: usize) {
    // Tool buttons
    let mut x = BUTTON_PAD;
    for tool in TOOLS {
        draw_button(fb, x, TOOLBAR_Y + 1, BUTTON_W, tool.label(), tool == current_tool);
        x += BUTTON_W + BUTTON_PAD;
    }
}

fn draw_size_bar(fb: &mut FrameBuf, current_size: BrushSize, _width: usize, collab_status: &[u8], ttf: Option<&mut CachedFont>, font_size: f32) {
    // Size buttons
    let mut x = BUTTON_PAD;
    for size in SIZES {
        let sw = 20;
        draw_button(fb, x, SIZE_BAR_Y + 1, sw, size.label(), size == current_size);
        x += sw + BUTTON_PAD;
    }

    // Collaboration status text
    if !collab_status.is_empty() {
        let status_x = x + 8;
        let status_str = core::str::from_utf8(collab_status).unwrap_or("");
        draw_text_auto(fb, ttf, status_str, status_x as i32, (SIZE_BAR_Y + 5) as i32, font_size, Color::rgb(140, 200, 140), 1);
    }
}

fn draw_hue_bar(fb: &mut FrameBuf, width: usize, current_hue: u16) {
    let bar_w = width.saturating_sub(COLOR_SWATCH_SIZE + 4);
    for x in 0..bar_w {
        let hue = (x * 360 / bar_w) as u16;
        let c = hsv_to_rgb(hue, 255, 255);
        for y in 0..HUE_BAR_H {
            fb.put_pixel(x, HUE_BAR_Y + y, c);
        }
    }

    // Hue indicator
    let indicator_x = (current_hue as usize * bar_w / 360).min(bar_w.saturating_sub(1));
    for y in 0..HUE_BAR_H {
        fb.put_pixel(indicator_x, HUE_BAR_Y + y, Color::WHITE);
    }

    fb.mark_dirty(0, HUE_BAR_Y as i32, bar_w as i32, HUE_BAR_H as i32);
}

fn draw_current_color_swatch(fb: &mut FrameBuf, width: usize, color: Color) {
    let x = (width - COLOR_SWATCH_SIZE - 2) as i32;
    let y = HUE_BAR_Y as i32;
    shapes::fill_rect(fb, x, y, COLOR_SWATCH_SIZE as i32, COLOR_SWATCH_SIZE as i32, color);
    shapes::draw_rect(fb, x, y, COLOR_SWATCH_SIZE as i32, COLOR_SWATCH_SIZE as i32, Color::WHITE);
}

fn draw_sv_square(fb: &mut FrameBuf, hue: u16, sat: u8, val: u8) {
    for y in 0..SV_SQUARE_SIZE {
        for x in 0..SV_SQUARE_SIZE {
            let s = (x * 255 / SV_SQUARE_SIZE) as u8;
            let v = ((SV_SQUARE_SIZE - 1 - y) * 255 / SV_SQUARE_SIZE) as u8;
            let c = hsv_to_rgb(hue, s, v);
            fb.put_pixel(x, SV_SQUARE_Y + y, c);
        }
    }

    // Crosshair at selected point
    let sx = (sat as usize * SV_SQUARE_SIZE / 255).min(SV_SQUARE_SIZE - 1);
    let sy = ((255 - val) as usize * SV_SQUARE_SIZE / 255).min(SV_SQUARE_SIZE - 1);
    let cross_color = if val > 128 { Color::BLACK } else { Color::WHITE };
    for d in 1..=4i32 {
        let px = sx as i32;
        let py = (SV_SQUARE_Y + sy) as i32;
        if px - d >= 0 {
            fb.put_pixel((px - d) as usize, py as usize, cross_color);
        }
        if (px + d) < SV_SQUARE_SIZE as i32 {
            fb.put_pixel((px + d) as usize, py as usize, cross_color);
        }
        if py - d >= SV_SQUARE_Y as i32 {
            fb.put_pixel(px as usize, (py - d) as usize, cross_color);
        }
        if (py + d) < (SV_SQUARE_Y + SV_SQUARE_SIZE) as i32 {
            fb.put_pixel(px as usize, (py + d) as usize, cross_color);
        }
    }

    fb.mark_dirty(0, SV_SQUARE_Y as i32, SV_SQUARE_SIZE as i32, SV_SQUARE_SIZE as i32);
}

fn draw_recent_colors(fb: &mut FrameBuf, recent: &[Color; 8], ttf: Option<&mut CachedFont>, font_size: f32) {
    let base_x = SV_SQUARE_SIZE + 8;
    let base_y = SV_SQUARE_Y + 2;

    draw_text_auto(fb, ttf, "Recent:", base_x as i32, base_y as i32, font_size, Color::GRAY, 1);

    for (i, c) in recent.iter().enumerate() {
        let x = base_x + i * (RECENT_COLOR_SIZE + 2);
        let y = base_y + 10;
        shapes::fill_rect(fb, x as i32, y as i32, RECENT_COLOR_SIZE as i32, RECENT_COLOR_SIZE as i32, *c);
        shapes::draw_rect(fb, x as i32, y as i32, RECENT_COLOR_SIZE as i32, RECENT_COLOR_SIZE as i32, Color::GRAY);
    }
}

fn draw_cursor(fb: &mut FrameBuf, mx: i32, my: i32, width: i32, height: i32) {
    let c = Color::rgb(200, 200, 200);
    for d in 1..=3i32 {
        if mx - d >= 0 {
            fb.put_pixel((mx - d) as usize, my as usize, c);
        }
        if mx + d < width {
            fb.put_pixel((mx + d) as usize, my as usize, c);
        }
        if my - d >= 0 {
            fb.put_pixel(mx as usize, (my - d) as usize, c);
        }
        if my + d < height {
            fb.put_pixel(mx as usize, (my + d) as usize, c);
        }
    }
}

/// Draw a remote peer's cursor as a colored crosshair
fn draw_remote_cursor(fb: &mut FrameBuf, cursor: &RemoteCursor, width: usize, height: usize) {
    if !cursor.visible {
        return;
    }
    let cx = cursor.x as i32;
    let cy = cursor.y as i32 + CANVAS_Y as i32;
    if cx < 0 || cx >= width as i32 || cy < 0 || cy >= height as i32 {
        return;
    }

    let color_idx = ((cursor.peer_id as usize).wrapping_sub(1)) % PEER_COLORS.len();
    let c = PEER_COLORS[color_idx];

    // Draw crosshair (5px arms)
    for d in 1..=5i32 {
        if cx - d >= 0 {
            fb.put_pixel((cx - d) as usize, cy as usize, c);
        }
        if cx + d < width as i32 {
            fb.put_pixel((cx + d) as usize, cy as usize, c);
        }
        if cy - d >= 0 {
            fb.put_pixel(cx as usize, (cy - d) as usize, c);
        }
        if cy + d < height as i32 {
            fb.put_pixel(cx as usize, (cy + d) as usize, c);
        }
    }

    // Draw peer name label above cursor
    if cursor.name_len > 0 {
        let name = &cursor.name[..cursor.name_len as usize];
        let label_x = (cx + 6).max(0) as usize;
        let label_y = (cy - 10).max(0) as usize;
        if label_x < width && label_y < height {
            font::draw_text(fb, name, label_x, label_y, c, 1);
        }
    }
}

// ---------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------

fn hit_tool(mx: usize, my: usize) -> Option<Tool> {
    if my < TOOLBAR_Y + 1 || my >= TOOLBAR_Y + 1 + BUTTON_H {
        return None;
    }
    let mut x = BUTTON_PAD;
    for tool in TOOLS {
        if mx >= x && mx < x + BUTTON_W {
            return Some(tool);
        }
        x += BUTTON_W + BUTTON_PAD;
    }
    None
}

fn hit_size(mx: usize, my: usize) -> Option<BrushSize> {
    if my < SIZE_BAR_Y + 1 || my >= SIZE_BAR_Y + 1 + BUTTON_H {
        return None;
    }
    let sw = 20;
    let mut x = BUTTON_PAD;
    for size in SIZES {
        if mx >= x && mx < x + sw {
            return Some(size);
        }
        x += sw + BUTTON_PAD;
    }
    None
}

fn hit_hue_bar(mx: usize, my: usize, width: usize) -> Option<u16> {
    if my < HUE_BAR_Y || my >= HUE_BAR_Y + HUE_BAR_H {
        return None;
    }
    let bar_w = width.saturating_sub(COLOR_SWATCH_SIZE + 4);
    if mx >= bar_w {
        return None;
    }
    Some((mx * 360 / bar_w) as u16)
}

fn hit_sv_square(mx: usize, my: usize) -> Option<(u8, u8)> {
    if mx >= SV_SQUARE_SIZE || my < SV_SQUARE_Y || my >= SV_SQUARE_Y + SV_SQUARE_SIZE {
        return None;
    }
    let local_y = my - SV_SQUARE_Y;
    let s = (mx * 255 / SV_SQUARE_SIZE) as u8;
    let v = ((SV_SQUARE_SIZE - 1 - local_y) * 255 / SV_SQUARE_SIZE) as u8;
    Some((s, v))
}

fn hit_recent(mx: usize, my: usize) -> Option<usize> {
    let base_x = SV_SQUARE_SIZE + 8;
    let base_y = SV_SQUARE_Y + 2 + 10;
    if my < base_y || my >= base_y + RECENT_COLOR_SIZE {
        return None;
    }
    for i in 0..8 {
        let x = base_x + i * (RECENT_COLOR_SIZE + 2);
        if mx >= x && mx < x + RECENT_COLOR_SIZE {
            return Some(i);
        }
    }
    None
}

fn hit_canvas(mx: usize, my: usize, cw: usize, ch: usize) -> Option<(i32, i32)> {
    if my >= CANVAS_Y && mx < cw && my < CANVAS_Y + ch {
        Some((mx as i32, (my - CANVAS_Y) as i32))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Number formatting helper
// ---------------------------------------------------------------------------

fn format_save_path(counter: u32) -> ([u8; 32], usize) {
    let prefix = b"/home/guskit_";
    let suffix = b".bmp";
    let mut buf = [0u8; 32];
    let mut pos = 0;

    for &b in prefix.iter() {
        buf[pos] = b;
        pos += 1;
    }

    // Format counter
    if counter == 0 {
        buf[pos] = b'0';
        pos += 1;
    } else {
        let mut digits = [0u8; 10];
        let mut n = counter;
        let mut dpos = 0;
        while n > 0 {
            digits[dpos] = b'0' + (n % 10) as u8;
            n /= 10;
            dpos += 1;
        }
        for i in (0..dpos).rev() {
            buf[pos] = digits[i];
            pos += 1;
        }
    }

    for &b in suffix.iter() {
        buf[pos] = b;
        pos += 1;
    }

    (buf, pos)
}

/// Write all bytes to a file, looping on short writes. Returns total bytes written.
///
/// Writes in 32KB chunks to avoid overwhelming the kernel's ext2 block allocator
/// with a single huge write syscall (each write must allocate blocks on the fly).
fn write_all(file: &File, data: &[u8]) -> usize {
    const CHUNK_SIZE: usize = 32 * 1024; // 32KB per write syscall
    let mut offset = 0;
    while offset < data.len() {
        let end = std::cmp::min(offset + CHUNK_SIZE, data.len());
        let chunk = &data[offset..end];
        match file.write(chunk) {
            Ok(0) => break,
            Ok(n) => offset += n,
            Err(_) => break,
        }
    }
    offset
}

// ---------------------------------------------------------------------------
// Add to recent colors
// ---------------------------------------------------------------------------

fn add_recent(recent: &mut [Color; 8], color: Color) {
    // Don't add white (eraser)
    if color.r == 255 && color.g == 255 && color.b == 255 {
        return;
    }
    // Check if already present
    for c in recent.iter() {
        if c.r == color.r && c.g == color.g && c.b == color.b {
            return;
        }
    }
    // Shift and insert at front
    for i in (1..8).rev() {
        recent[i] = recent[i - 1];
    }
    recent[0] = color;
}

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

fn parse_collab_args() -> CollabMode {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => {
                if i + 1 < args.len() {
                    if let Ok(port) = args[i + 1].parse::<u16>() {
                        return CollabMode::Host { port };
                    }
                }
                println!("Usage: guskit --host PORT");
                process::exit(1);
            }
            "--join" => {
                if i + 1 < args.len() {
                    if let Some((addr, port)) = parse_addr_port(&args[i + 1]) {
                        return CollabMode::Join { addr, port };
                    }
                }
                println!("Usage: guskit --join IP:PORT");
                process::exit(1);
            }
            _ => {}
        }
        i += 1;
    }
    CollabMode::None
}

/// Parse "A.B.C.D:PORT" into ([A,B,C,D], PORT)
fn parse_addr_port(s: &str) -> Option<([u8; 4], u16)> {
    let colon = s.rfind(':')?;
    let ip_str = &s[..colon];
    let port_str = &s[colon + 1..];
    let port: u16 = port_str.parse().ok()?;

    let mut addr = [0u8; 4];
    let mut octet_idx = 0;
    for part in ip_str.split('.') {
        if octet_idx >= 4 {
            return None;
        }
        addr[octet_idx] = part.parse().ok()?;
        octet_idx += 1;
    }
    if octet_idx != 4 {
        return None;
    }
    Some((addr, port))
}

/// Format a collab status string into a fixed buffer. Returns length.
fn format_collab_status(session: &CollabSession, buf: &mut [u8; 64]) -> usize {
    let mut pos = 0;
    if session.is_host() {
        let prefix = b"Host | ";
        let len = prefix.len().min(buf.len() - pos);
        buf[pos..pos + len].copy_from_slice(&prefix[..len]);
        pos += len;
    } else {
        let prefix = b"Joined | ";
        let len = prefix.len().min(buf.len() - pos);
        buf[pos..pos + len].copy_from_slice(&prefix[..len]);
        pos += len;
    }

    let count = session.peer_count();
    // Format peer count
    let mut digits = [0u8; 4];
    let mut n = count;
    let mut dpos = 0;
    if n == 0 {
        digits[0] = b'0';
        dpos = 1;
    } else {
        while n > 0 && dpos < 4 {
            digits[dpos] = b'0' + (n % 10) as u8;
            n /= 10;
            dpos += 1;
        }
    }
    for j in (0..dpos).rev() {
        if pos < buf.len() {
            buf[pos] = digits[j];
            pos += 1;
        }
    }
    let suffix = if count == 1 { b" peer" as &[u8] } else { b" peers" };
    let len = suffix.len().min(buf.len() - pos);
    buf[pos..pos + len].copy_from_slice(&suffix[..len]);
    pos += len;

    pos
}

// ---------------------------------------------------------------------------
// File picker helpers
// ---------------------------------------------------------------------------

fn has_bmp_extension(name: &[u8]) -> bool {
    if name.len() < 4 {
        return false;
    }
    let ext = &name[name.len() - 4..];
    ext[0] == b'.'
        && (ext[1] == b'b' || ext[1] == b'B')
        && (ext[2] == b'm' || ext[2] == b'M')
        && (ext[3] == b'p' || ext[3] == b'P')
}

fn join_path(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = Vec::with_capacity(dir.len() + 1 + name.len());
    path.extend_from_slice(dir);
    if !dir.is_empty() && dir[dir.len() - 1] != b'/' {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path
}

fn list_directory(path: &[u8]) -> Vec<FileEntry> {
    let path_str = core::str::from_utf8(path).unwrap_or("/");
    let fd = match fs::open(path_str, fs::O_RDONLY | fs::O_DIRECTORY) {
        Ok(fd) => fd,
        Err(_) => return Vec::new(),
    };

    let mut entries = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = match fs::getdents64(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for ent in fs::DirentIter::new(&buf, n) {
            let name = unsafe { ent.name() };
            // Skip "."
            if name == b"." {
                continue;
            }
            let is_dir = ent.d_type == fs::DT_DIR;
            if !is_dir && !has_bmp_extension(name) {
                continue;
            }
            // Get file size for regular files
            let size = if !is_dir {
                let full_path = join_path(path, name);
                let full_str = core::str::from_utf8(&full_path).unwrap_or("");
                if let Ok(file_fd) = fs::open(full_str, fs::O_RDONLY) {
                    let sz = fs::fstat(file_fd).map(|s| s.st_size as u64).unwrap_or(0);
                    let _ = fs::close(file_fd);
                    sz
                } else {
                    0
                }
            } else {
                0
            };
            entries.push(FileEntry {
                name: name.to_vec(),
                is_dir,
                size,
            });
        }
    }
    let _ = fs::close(fd);

    // Sort: directories first (alphabetically), then files (alphabetically)
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    entries
}

fn dim_background(fb: &mut FrameBuf, width: usize, height: usize) {
    let ptr = fb.raw_ptr();
    let stride = fb.stride;
    let bpp = fb.bpp;
    // Darken every other scanline for a venetian-blind dimming effect
    for y in (0..height).step_by(2) {
        let row_offset = y * stride;
        for x in 0..width {
            let offset = row_offset + x * bpp;
            unsafe {
                *ptr.add(offset) >>= 1;
                *ptr.add(offset + 1) >>= 1;
                *ptr.add(offset + 2) >>= 1;
            }
        }
    }
    fb.mark_dirty(0, 0, width as i32, height as i32);
}

// ---------------------------------------------------------------------------
// GuskitState — all mutable application state
// ---------------------------------------------------------------------------

struct GuskitState {
    // Drawing state
    tool: Tool,
    hue: u16,
    saturation: u8,
    value: u8,
    color: Color,
    brush_size: BrushSize,
    mouse_down: bool,
    drag_start: (i32, i32),
    prev_mouse: (i32, i32),
    recent_colors: [Color; 8],
    save_counter: u32,

    // Canvas
    canvas: Vec<u8>,
    canvas_w: usize,
    canvas_h: usize,

    // Input tracking
    mx: i32,
    my: i32,
    left_down: bool,
    was_down: bool,

    // Collaboration
    session: Option<CollabSession>,
    remote_cursors: Vec<RemoteCursor>,
    cursor_send_counter: u32,
    collab_poll_fds: [PollFd; 20],

    // File picker
    file_picker: Option<FilePicker>,
    file_picker_dir: Vec<u8>,
    picker_theme: Theme,

    // Window dimensions
    width: usize,
    height: usize,
}

impl GuskitState {
    fn new(width: usize, height: usize, collab_mode: CollabMode) -> Self {
        let canvas_w = width;
        let canvas_h = height.saturating_sub(CANVAS_Y);
        let canvas = vec![255u8; canvas_w * canvas_h * 3]; // white background

        let hue: u16 = 0;
        let saturation: u8 = 255;
        let value: u8 = 255;
        let color = hsv_to_rgb(hue, saturation, value);

        let session: Option<CollabSession> = match &collab_mode {
            CollabMode::None => None,
            CollabMode::Host { port } => {
                match CollabSession::host(*port, b"Host", canvas_w as u16, canvas_h as u16) {
                    Ok(s) => {
                        println!("Hosting collab session on port {}", port);
                        Some(s)
                    }
                    Err(e) => {
                        println!("Failed to host: {}", e);
                        None
                    }
                }
            }
            CollabMode::Join { addr, port } => {
                let sock_addr = SockAddrIn::new(*addr, *port);
                match CollabSession::join(&sock_addr, b"Guest") {
                    Ok(s) => {
                        println!(
                            "Joined collab session at {}.{}.{}.{}:{}",
                            addr[0], addr[1], addr[2], addr[3], port
                        );
                        Some(s)
                    }
                    Err(e) => {
                        println!("Failed to join: {}", e);
                        None
                    }
                }
            }
        };

        GuskitState {
            tool: Tool::Pencil,
            hue,
            saturation,
            value,
            color,
            brush_size: BrushSize::Medium,
            mouse_down: false,
            drag_start: (0, 0),
            prev_mouse: (0, 0),
            recent_colors: [
                Color::BLACK,
                Color::RED,
                Color::GREEN,
                Color::BLUE,
                Color::YELLOW,
                Color::MAGENTA,
                Color::CYAN,
                Color::WHITE,
            ],
            save_counter: 0,
            canvas,
            canvas_w,
            canvas_h,
            mx: 0,
            my: 0,
            left_down: false,
            was_down: false,
            session,
            remote_cursors: Vec::new(),
            cursor_send_counter: 0,
            collab_poll_fds: [PollFd::default(); 20],
            file_picker: None,
            file_picker_dir: Vec::from(b"/home" as &[u8]),
            picker_theme: Theme::dark(),
            width,
            height,
        }
    }

    /// Poll collaboration sockets non-blocking (timeout=0). Used in the
    /// Breengel event loop where frame pacing is handled by sleep_ms at the
    /// end of the loop, not inside the collab poll.
    fn poll_collab_io_nonblocking(&mut self) {
        let collab_n = if let Some(ref sess) = self.session {
            sess.poll_fds(&mut self.collab_poll_fds)
        } else {
            0
        };

        if collab_n > 0 {
            let _ = io::poll(&mut self.collab_poll_fds[..collab_n], 0);
            if let Some(ref mut sess) = self.session {
                sess.process_io(&self.collab_poll_fds[..collab_n]);
            }
        }
    }

    /// Drain the CollabSession event queue and apply all pending remote events.
    fn process_collab_events(&mut self) {
        loop {
            let event = if let Some(ref mut sess) = self.session {
                sess.next_event()
            } else {
                break;
            };

            match event {
                None => break,
                Some(CollabEvent::PeerJoined { peer_id, name, name_len }) => {
                    println!("Peer {} joined", peer_id);
                    self.remote_cursors.push(RemoteCursor {
                        peer_id,
                        x: 0,
                        y: 0,
                        visible: false,
                        name,
                        name_len,
                    });
                    // Host: send canvas sync to new joiner
                    if let Some(ref mut sess) = self.session {
                        if sess.is_host() {
                            sess.send_canvas_sync(
                                peer_id,
                                &self.canvas,
                                self.canvas_w as u16,
                                self.canvas_h as u16,
                            );
                        }
                    }
                }
                Some(CollabEvent::PeerLeft { peer_id }) => {
                    println!("Peer {} left", peer_id);
                    self.remote_cursors.retain(|c| c.peer_id != peer_id);
                }
                Some(CollabEvent::DrawOp(op)) => {
                    apply_draw_op(&mut self.canvas, self.canvas_w, self.canvas_h, &op);
                }
                Some(CollabEvent::CursorMoved { peer_id, x, y, visible }) => {
                    if let Some(rc) = self.remote_cursors.iter_mut().find(|c| c.peer_id == peer_id) {
                        rc.x = x;
                        rc.y = y;
                        rc.visible = visible;
                    } else {
                        // Peer cursor we haven't seen yet
                        self.remote_cursors.push(RemoteCursor {
                            peer_id,
                            x,
                            y,
                            visible,
                            name: [0; 32],
                            name_len: 0,
                        });
                    }
                }
                Some(CollabEvent::ToolChanged { .. }) => {
                    // Could update remote cursor appearance
                }
                Some(CollabEvent::SyncChunk { offset, data }) => {
                    // Write chunk data into canvas buffer
                    let start = offset as usize;
                    let end = (start + data.len()).min(self.canvas.len());
                    let copy_len = end.saturating_sub(start);
                    if copy_len > 0 {
                        self.canvas[start..start + copy_len].copy_from_slice(&data[..copy_len]);
                    }
                }
                Some(CollabEvent::SyncComplete) => {
                    println!("Canvas sync complete");
                }
                Some(CollabEvent::SessionEnded) => {
                    println!("Collab session ended");
                    self.session = None;
                    self.remote_cursors.clear();
                    break;
                }
            }
        }
    }

    /// Handle a mouse press event (called when left button transitions from up to down).
    /// Sets mouse_down and handles toolbar/UI hit-testing and canvas tool activation.
    fn handle_mouse_press(&mut self) {
        self.mouse_down = true;
        let umx = self.mx as usize;
        let umy = self.my as usize;

        // Hit test UI elements first
        if let Some(t) = hit_tool(umx, umy) {
            self.tool = t;
            self.mouse_down = false;
            // Notify peers of tool change
            if let Some(ref mut sess) = self.session {
                sess.send_tool_change(
                    self.tool.to_wire_id(),
                    self.brush_size.radius() as u8,
                    self.color.r,
                    self.color.g,
                    self.color.b,
                );
            }
        } else if let Some(s) = hit_size(umx, umy) {
            self.brush_size = s;
            self.mouse_down = false;
            if let Some(ref mut sess) = self.session {
                sess.send_tool_change(
                    self.tool.to_wire_id(),
                    self.brush_size.radius() as u8,
                    self.color.r,
                    self.color.g,
                    self.color.b,
                );
            }
        } else if let Some(h) = hit_hue_bar(umx, umy, self.width) {
            self.hue = h;
            self.color = hsv_to_rgb(self.hue, self.saturation, self.value);
            self.mouse_down = false;
        } else if let Some((s, v)) = hit_sv_square(umx, umy) {
            self.saturation = s;
            self.value = v;
            self.color = hsv_to_rgb(self.hue, self.saturation, self.value);
            self.mouse_down = false;
        } else if let Some(i) = hit_recent(umx, umy) {
            self.color = self.recent_colors[i];
            // Reverse-derive HSV (approximate) not needed, just use color directly
            self.mouse_down = false;
        } else if let Some((cx, cy)) = hit_canvas(umx, umy, self.canvas_w, self.canvas_h) {
            // Start drawing on canvas
            self.drag_start = (cx, cy);
            self.prev_mouse = (cx, cy);

            match self.tool {
                Tool::Pencil => {
                    canvas_put_pixel(&mut self.canvas, self.canvas_w, self.canvas_h, cx, cy, self.color);
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Pencil {
                            x0: cx as i16,
                            y0: cy as i16,
                            x1: cx as i16,
                            y1: cy as i16,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Brush => {
                    canvas_fill_circle(
                        &mut self.canvas,
                        self.canvas_w,
                        self.canvas_h,
                        cx,
                        cy,
                        self.brush_size.radius(),
                        self.color,
                    );
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Brush {
                            x0: cx as i16,
                            y0: cy as i16,
                            x1: cx as i16,
                            y1: cy as i16,
                            radius: self.brush_size.radius() as u8,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Eraser => {
                    canvas_fill_circle(
                        &mut self.canvas,
                        self.canvas_w,
                        self.canvas_h,
                        cx,
                        cy,
                        self.brush_size.radius(),
                        Color::WHITE,
                    );
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Eraser {
                            x0: cx as i16,
                            y0: cy as i16,
                            x1: cx as i16,
                            y1: cy as i16,
                            radius: self.brush_size.radius() as u8,
                        });
                    }
                }
                Tool::Fill => {
                    canvas_flood_fill(&mut self.canvas, self.canvas_w, self.canvas_h, cx, cy, self.color);
                    self.mouse_down = false;
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Fill {
                            x: cx as i16,
                            y: cy as i16,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Line | Tool::Rect | Tool::Circle => {
                    // Shape tools: just record start, preview during drag
                }
            }
        }
    }

    /// Handle continuous drawing while the mouse button is held (drag).
    fn handle_mouse_drag(&mut self) {
        if let Some((cx, cy)) = hit_canvas(self.mx as usize, self.my as usize, self.canvas_w, self.canvas_h) {
            match self.tool {
                Tool::Pencil => {
                    canvas_draw_line(
                        &mut self.canvas,
                        self.canvas_w,
                        self.canvas_h,
                        self.prev_mouse.0,
                        self.prev_mouse.1,
                        cx,
                        cy,
                        self.color,
                    );
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Pencil {
                            x0: self.prev_mouse.0 as i16,
                            y0: self.prev_mouse.1 as i16,
                            x1: cx as i16,
                            y1: cy as i16,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Brush => {
                    canvas_brush_line(
                        &mut self.canvas,
                        self.canvas_w,
                        self.canvas_h,
                        self.prev_mouse.0,
                        self.prev_mouse.1,
                        cx,
                        cy,
                        self.brush_size.radius(),
                        self.color,
                    );
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Brush {
                            x0: self.prev_mouse.0 as i16,
                            y0: self.prev_mouse.1 as i16,
                            x1: cx as i16,
                            y1: cy as i16,
                            radius: self.brush_size.radius() as u8,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Eraser => {
                    canvas_brush_line(
                        &mut self.canvas,
                        self.canvas_w,
                        self.canvas_h,
                        self.prev_mouse.0,
                        self.prev_mouse.1,
                        cx,
                        cy,
                        self.brush_size.radius(),
                        Color::WHITE,
                    );
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Eraser {
                            x0: self.prev_mouse.0 as i16,
                            y0: self.prev_mouse.1 as i16,
                            x1: cx as i16,
                            y1: cy as i16,
                            radius: self.brush_size.radius() as u8,
                        });
                    }
                }
                _ => {}
            }
            self.prev_mouse = (cx, cy);
        }
    }

    /// Handle mouse release: commit shape tools and add to recent colors.
    fn handle_mouse_release(&mut self) {
        self.mouse_down = false;
        if let Some((cx, cy)) =
            hit_canvas(self.mx as usize, self.my as usize, self.canvas_w, self.canvas_h)
        {
            match self.tool {
                Tool::Line => {
                    canvas_draw_line(
                        &mut self.canvas,
                        self.canvas_w,
                        self.canvas_h,
                        self.drag_start.0,
                        self.drag_start.1,
                        cx,
                        cy,
                        self.color,
                    );
                    add_recent(&mut self.recent_colors, self.color);
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Line {
                            x0: self.drag_start.0 as i16,
                            y0: self.drag_start.1 as i16,
                            x1: cx as i16,
                            y1: cy as i16,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Rect => {
                    let rx = self.drag_start.0.min(cx);
                    let ry = self.drag_start.1.min(cy);
                    let rw = (self.drag_start.0 - cx).abs();
                    let rh = (self.drag_start.1 - cy).abs();
                    canvas_fill_rect(&mut self.canvas, self.canvas_w, self.canvas_h, rx, ry, rw, rh, self.color);
                    add_recent(&mut self.recent_colors, self.color);
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Rect {
                            x: rx as i16,
                            y: ry as i16,
                            w: rw as i16,
                            h: rh as i16,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Circle => {
                    let dx = (cx - self.drag_start.0) as i64;
                    let dy = (cy - self.drag_start.1) as i64;
                    let radius = isqrt((dx * dx + dy * dy) as u64) as i32;
                    canvas_fill_circle(
                        &mut self.canvas,
                        self.canvas_w,
                        self.canvas_h,
                        self.drag_start.0,
                        self.drag_start.1,
                        radius,
                        self.color,
                    );
                    add_recent(&mut self.recent_colors, self.color);
                    if let Some(ref mut sess) = self.session {
                        sess.send_op(&DrawOp::Circle {
                            cx: self.drag_start.0 as i16,
                            cy: self.drag_start.1 as i16,
                            radius: radius as i16,
                            r: self.color.r,
                            g: self.color.g,
                            b: self.color.b,
                        });
                    }
                }
                Tool::Pencil | Tool::Brush | Tool::Eraser => {
                    add_recent(&mut self.recent_colors, self.color);
                }
                _ => {}
            }
        }
    }

    /// Process all mouse input for this frame. Handles file picker modal intercept,
    /// then press/drag/release state machine for normal drawing.
    /// `left_down` and `was_down` must already be updated from Breengel events
    /// before calling this method.
    fn process_input(&mut self) {
        // Send cursor update to peers (throttled to ~10Hz)
        self.cursor_send_counter += 1;
        if self.cursor_send_counter >= 6 {
            self.cursor_send_counter = 0;
            if let Some(ref mut sess) = self.session {
                let canvas_y = self.my - CANVAS_Y as i32;
                let on_canvas = self.mx >= 0
                    && self.mx < self.canvas_w as i32
                    && canvas_y >= 0
                    && canvas_y < self.canvas_h as i32;
                sess.send_cursor(self.mx as i16, canvas_y as i16, on_canvas);
            }
        }

        // Build input state for file picker — synthesize button words from tracked state
        let buttons: u32 = if self.left_down { 1 } else { 0 };
        let prev_buttons: u32 = if self.was_down { 1 } else { 0 };
        let input = libbui::InputState::from_raw(self.mx, self.my, buttons, prev_buttons);

        // File picker modal handling — two-phase to avoid overlapping borrows:
        // Phase 1: update picker, extract result + any needed data while picker borrow is live.
        // Phase 2: act on the result after the picker borrow has ended.
        enum PickerAction {
            None,
            OpenFile(Vec<u8>),   // full path bytes to load
            Navigate(Vec<u8>),   // dir entry name to navigate into
            Close,
        }
        let picker_action = if let Some(ref mut picker) = self.file_picker {
            match picker.update(&input) {
                FilePickerResult::Selected(_idx) => {
                    let name_opt = picker.selected_entry().map(|e| e.name.clone());
                    match name_opt {
                        Some(name) => PickerAction::OpenFile(join_path(&self.file_picker_dir, &name)),
                        None => PickerAction::Close,
                    }
                }
                FilePickerResult::NavigateDir(_idx) => {
                    let name_opt = picker.selected_entry().map(|e| e.name.clone());
                    match name_opt {
                        Some(name) => PickerAction::Navigate(name),
                        None => PickerAction::None,
                    }
                }
                FilePickerResult::Cancelled => PickerAction::Close,
                FilePickerResult::Active => PickerAction::None,
            }
        } else {
            PickerAction::None
        };

        // Phase 2: act on result now that the picker borrow is released.
        match picker_action {
            PickerAction::OpenFile(full_path) => {
                let path_str = core::str::from_utf8(&full_path).unwrap_or("");
                if let Ok(file) = File::open(path_str, fs::O_RDONLY) {
                    let mut file_data = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match file.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => file_data.extend_from_slice(&chunk[..n]),
                            Err(_) => break,
                        }
                    }
                    if let Some((bw, bh, rgb)) = bmp::decode_bmp_24(&file_data) {
                        let copy_w = (bw as usize).min(self.canvas_w);
                        let copy_h = (bh as usize).min(self.canvas_h);
                        for b in self.canvas.iter_mut() {
                            *b = 255;
                        }
                        for y in 0..copy_h {
                            for x in 0..copy_w {
                                let si = (y * bw as usize + x) * 3;
                                let di = (y * self.canvas_w + x) * 3;
                                self.canvas[di] = rgb[si];
                                self.canvas[di + 1] = rgb[si + 1];
                                self.canvas[di + 2] = rgb[si + 2];
                            }
                        }
                        println!("Opened {}", path_str);
                    }
                }
                self.file_picker = None;
            }
            PickerAction::Navigate(name) => {
                if name == b".." {
                    if let Some(pos) = self.file_picker_dir.iter().rposition(|&b| b == b'/') {
                        if pos == 0 {
                            self.file_picker_dir = Vec::from(b"/" as &[u8]);
                        } else {
                            self.file_picker_dir.truncate(pos);
                        }
                    }
                } else {
                    self.file_picker_dir = join_path(&self.file_picker_dir, &name);
                }
                let entries = list_directory(&self.file_picker_dir);
                if let Some(ref mut picker) = self.file_picker {
                    picker.navigate(self.file_picker_dir.clone(), entries);
                }
            }
            PickerAction::Close => {
                self.file_picker = None;
            }
            PickerAction::None if self.file_picker.is_some() => {
                // Picker is active but no action needed — do not fall through to normal input
            }
            PickerAction::None => {
                // Normal input handling (not in file picker mode)

                // Mouse press
                if self.left_down && !self.was_down {
                    self.handle_mouse_press();
                }

                // Mouse held (dragging)
                if self.left_down && self.was_down && self.mouse_down {
                    self.handle_mouse_drag();
                }

                // Mouse release
                if !self.left_down && self.was_down && self.mouse_down {
                    self.handle_mouse_release();
                }
            }
        }
    }

    /// Full render pass: background, canvas, shape preview, remote cursors,
    /// toolbar, bars, file picker overlay, local cursor.
    fn render(&self, fb: &mut FrameBuf, ttf: &mut Option<CachedFont>, font_size: f32) {
        let bg = Color::rgb(40, 40, 40);

        // Background for UI area
        shapes::fill_rect(fb, 0, 0, self.width as i32, CANVAS_Y as i32, bg);

        // Blit canvas
        blit_canvas(fb, &self.canvas, self.canvas_w, self.canvas_h);

        // Shape preview (rubber-band on framebuf only)
        if self.mouse_down && self.tool.is_shape() {
            if let Some((cx, cy)) =
                hit_canvas(self.mx as usize, self.my as usize, self.canvas_w, self.canvas_h)
            {
                let preview_color = self.color;
                match self.tool {
                    Tool::Line => {
                        shapes::draw_line(
                            fb,
                            self.drag_start.0,
                            self.drag_start.1 + CANVAS_Y as i32,
                            cx,
                            cy + CANVAS_Y as i32,
                            preview_color,
                        );
                    }
                    Tool::Rect => {
                        let rx = self.drag_start.0.min(cx);
                        let ry = self.drag_start.1.min(cy) + CANVAS_Y as i32;
                        let rw = (self.drag_start.0 - cx).abs();
                        let rh = (self.drag_start.1 - cy).abs();
                        shapes::draw_rect(fb, rx, ry, rw, rh, preview_color);
                    }
                    Tool::Circle => {
                        let dx = (cx - self.drag_start.0) as i64;
                        let dy = (cy - self.drag_start.1) as i64;
                        let radius = isqrt((dx * dx + dy * dy) as u64) as i32;
                        shapes::draw_circle(
                            fb,
                            self.drag_start.0,
                            self.drag_start.1 + CANVAS_Y as i32,
                            radius,
                            preview_color,
                        );
                    }
                    _ => {}
                }
            }
        }

        // Draw remote cursors (ephemeral, on framebuf only)
        for cursor in &self.remote_cursors {
            draw_remote_cursor(fb, cursor, self.width, self.height);
        }

        // Draw UI
        draw_toolbar(fb, self.tool, self.width);

        // Build collab status string
        let mut status_buf = [0u8; 64];
        let status_len = if let Some(ref sess) = self.session {
            format_collab_status(sess, &mut status_buf)
        } else {
            0
        };
        draw_size_bar(fb, self.brush_size, self.width, &status_buf[..status_len], ttf.as_mut(), font_size);

        draw_hue_bar(fb, self.width, self.hue);
        draw_current_color_swatch(fb, self.width, self.color);
        draw_sv_square(fb, self.hue, self.saturation, self.value);
        draw_recent_colors(fb, &self.recent_colors, ttf.as_mut(), font_size);

        // File picker overlay
        if let Some(ref picker) = self.file_picker {
            dim_background(fb, self.width, self.height);
            picker.draw(fb, &self.picker_theme);
        }

        // Cursor
        if self.mx >= 0
            && self.mx < self.width as i32
            && self.my >= 0
            && self.my < self.height as i32
        {
            draw_cursor(fb, self.mx, self.my, self.width as i32, self.height as i32);
        }
    }

    /// Handle a window resize event. Updates `self.width`/`self.height` and,
    /// when no collaboration session is active, reallocates the canvas to fit
    /// the new window dimensions while preserving the existing drawing content.
    fn handle_resize(&mut self, new_w: usize, new_h: usize) {
        self.width = new_w;
        self.height = new_h;

        // During an active collaboration session the canvas is shared with
        // remote peers — do not resize it unilaterally.
        if self.session.is_some() {
            return;
        }

        let new_canvas_w = new_w;
        let new_canvas_h = new_h.saturating_sub(CANVAS_Y);

        if new_canvas_w == 0 || new_canvas_h == 0 {
            return; // window too small to hold a canvas
        }

        if new_canvas_w != self.canvas_w || new_canvas_h != self.canvas_h {
            // Allocate replacement canvas filled with white.
            let mut new_canvas = vec![255u8; new_canvas_w * new_canvas_h * 3];

            // Copy the intersection of the old canvas into the new one.
            let copy_w = self.canvas_w.min(new_canvas_w);
            let copy_h = self.canvas_h.min(new_canvas_h);
            for y in 0..copy_h {
                let src = y * self.canvas_w * 3;
                let dst = y * new_canvas_w * 3;
                new_canvas[dst..dst + copy_w * 3]
                    .copy_from_slice(&self.canvas[src..src + copy_w * 3]);
            }

            self.canvas = new_canvas;
            self.canvas_w = new_canvas_w;
            self.canvas_h = new_canvas_h;
        }

        // A file picker opened at the old dimensions may now be out of bounds.
        self.file_picker = None;
    }
}

// ---------------------------------------------------------------------------
// Menu bar helpers
// ---------------------------------------------------------------------------

fn build_guskit_menus(width: i32) -> MenuBar {
    let rect = BuiRect::new(0, 0, width, MENU_BAR_HEIGHT);
    let menus = vec![
        Menu {
            label: b"File",
            items: vec![
                MenuItem::new(b"New",  ACT_NEW,  Shortcut::ctrl(b'N')),
                MenuItem::new(b"Open", ACT_OPEN, Shortcut::ctrl(b'O')),
                MenuItem::new(b"Save", ACT_SAVE, Shortcut::ctrl(b'S')),
                MenuItem::separator(),
                MenuItem::new(b"Quit", ACT_QUIT, Shortcut::ctrl(b'Q')),
            ],
        },
        Menu {
            label: b"Edit",
            items: vec![
                MenuItem::disabled(b"Undo", ACT_UNDO, Shortcut::ctrl(b'Z')),
                MenuItem::disabled(b"Redo", ACT_REDO, Shortcut::ctrl_shift(b'Z')),
            ],
        },
    ];
    MenuBar::new(rect, menus)
}

fn handle_menu_action(state: &mut GuskitState, action: MenuAction) {
    match action {
        ACT_NEW => {
            for b in state.canvas.iter_mut() {
                *b = 255;
            }
            if let Some(ref mut sess) = state.session {
                sess.send_op(&DrawOp::Clear);
            }
        }
        ACT_OPEN => {
            let entries = list_directory(&state.file_picker_dir);
            let pw = 300.min(state.width as i32 - 20);
            let ph = 350.min(state.height as i32 - 20);
            let px = (state.width as i32 - pw) / 2;
            let py = (state.height as i32 - ph) / 2;
            let picker_rect = BuiRect::new(px, py, pw, ph);
            state.file_picker = Some(FilePicker::new(
                picker_rect,
                state.file_picker_dir.clone(),
                entries,
                &state.picker_theme,
            ));
        }
        ACT_SAVE => {
            let (path_buf, path_len) = format_save_path(state.save_counter);
            let path = core::str::from_utf8(&path_buf[..path_len]).unwrap_or("/home/guskit.bmp");
            let bmp_data =
                bmp::encode_bmp_24(state.canvas_w as u32, state.canvas_h as u32, &state.canvas);
            match File::create(path) {
                Ok(file) => {
                    let written = write_all(&file, &bmp_data);
                    if written == bmp_data.len() {
                        println!("Saved {}", path);
                    } else {
                        println!("Save error: wrote {}/{} bytes", written, bmp_data.len());
                    }
                }
                Err(_) => {
                    println!("Save error: could not create {}", path);
                }
            }
            state.save_counter += 1;
        }
        ACT_QUIT => {
            if let Some(ref mut sess) = state.session {
                sess.disconnect();
            }
            process::exit(0);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Main — thin orchestrator
// ---------------------------------------------------------------------------

fn main() {
    println!("Gus Kit starting!");

    let collab_mode = parse_collab_args();

    let mut win = Window::new(b"Gus Kit", 800, 600)
        .unwrap_or_else(|e| {
            println!("Failed to create window: {}", e);
            process::exit(1);
        });

    let init_width = win.width() as usize;
    let init_height = win.height() as usize;

    let mut state = GuskitState::new(init_width, init_height, collab_mode);

    // Menu bar setup
    let mut menu_bar = build_guskit_menus(init_width as i32);
    let menu_theme = Theme::dark();
    menu_bar.layout(&menu_theme);

    // Load TTF mono font (falls back to bitmap if unavailable)
    let mut mono_font: Option<CachedFont> = win.take_mono_font();
    let font_size = win.mono_size().max(10.0);

    loop {
        // 1. Non-blocking collab I/O poll (frame pacing is at end of loop)
        state.poll_collab_io_nonblocking();
        state.process_collab_events();

        // 2. Breengel events replace legacy graphics::mouse_state()
        state.was_down = state.left_down;
        for event in win.poll_events() {
            match event {
                Event::MouseMove { x, y } => {
                    state.mx = x.max(0).min(state.width as i32 - 1);
                    state.my = y.max(0).min(state.height as i32 - 1);
                }
                Event::MouseButton { button, pressed, x, y } => {
                    if button == 1 {
                        state.left_down = pressed;
                        state.mx = x.max(0).min(state.width as i32 - 1);
                        state.my = y.max(0).min(state.height as i32 - 1);
                    }
                }
                Event::KeyPress { ascii, modifiers, .. } => {
                    if let Some(action) = menu_bar.match_shortcut(ascii, modifiers.ctrl, modifiers.shift, modifiers.alt) {
                        handle_menu_action(&mut state, action);
                    } else {
                        match ascii {
                            b'p' | b'P' => state.tool = Tool::Pencil,
                            b'b' | b'B' => state.tool = Tool::Brush,
                            b'l' | b'L' => state.tool = Tool::Line,
                            b'r' | b'R' => state.tool = Tool::Rect,
                            b'c' | b'C' => state.tool = Tool::Circle,
                            b'f' | b'F' => state.tool = Tool::Fill,
                            b'e' | b'E' => state.tool = Tool::Eraser,
                            b'1' => state.brush_size = BrushSize::Small,
                            b'2' => state.brush_size = BrushSize::Medium,
                            b'3' => state.brush_size = BrushSize::Large,
                            _ => {}
                        }
                    }
                }
                Event::FontChanged => {
                    win.put_mono_font(mono_font.take());
                    mono_font = win.take_mono_font();
                }
                Event::CloseRequested => {
                    if let Some(ref mut sess) = state.session {
                        sess.disconnect();
                    }
                    process::exit(0);
                }
                Event::FocusLost => {
                    menu_bar.close();
                }
                Event::Resized { width: w, height: h } => {
                    state.handle_resize(w as usize, h as usize);
                    menu_bar.set_rect(BuiRect::new(0, 0, w as i32, MENU_BAR_HEIGHT));
                    menu_bar.layout(&menu_theme);
                }
                _ => {}
            }
        }

        // 3. Menu bar mouse handling, then guarded process_input
        let buttons: u32 = if state.left_down { 1 } else { 0 };
        let prev_buttons: u32 = if state.was_down { 1 } else { 0 };
        let menu_input = libbui::InputState::from_raw(state.mx, state.my, buttons, prev_buttons);
        if let MenuEvent::Activated(action) = menu_bar.update(&menu_input, &menu_theme) {
            handle_menu_action(&mut state, action);
        }

        if !menu_bar.is_open() {
            state.process_input();
        }

        // 4. Render into window framebuffer
        state.render(win.framebuf(), &mut mono_font, font_size);
        menu_bar.draw(win.framebuf(), &menu_theme);

        // 5. Present (replaces fb_flush)
        let _ = win.present();

        // 6. Frame pacing ~60 FPS
        let _ = time::sleep_ms(16);
    }
}
