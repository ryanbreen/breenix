//! Gus Kit — MS Paint-style drawing application for Breenix
//!
//! Features:
//! - Drawing tools: Pencil, Brush, Line, Rectangle, Circle, Fill, Eraser
//! - HSV color picker with hue bar, saturation/value square, recent colors
//! - Variable brush sizes (S/M/L)
//! - BMP file saving to /tmp/guskit_N.bmp
//!
//! Created for Gus!

use std::process;

use libbreenix::fs::File;
use libbreenix::graphics;
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use libimg::bmp;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const TOOLBAR_Y: usize = 0;
const SIZE_BAR_Y: usize = 18;
const HUE_BAR_Y: usize = 36;
const HUE_BAR_H: usize = 18;
const SV_SQUARE_Y: usize = 54;
const SV_SQUARE_SIZE: usize = 100;
const CANVAS_Y: usize = 154;

const BUTTON_W: usize = 36;
const BUTTON_H: usize = 16;
const BUTTON_PAD: usize = 2;

const COLOR_SWATCH_SIZE: usize = 16;
const RECENT_COLOR_SIZE: usize = 10;

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

fn draw_size_bar(fb: &mut FrameBuf, current_size: BrushSize, width: usize) {
    // Size buttons
    let mut x = BUTTON_PAD;
    for size in SIZES {
        let sw = 20;
        draw_button(fb, x, SIZE_BAR_Y + 1, sw, size.label(), size == current_size);
        x += sw + BUTTON_PAD;
    }

    // Action buttons on the right
    let actions: [&[u8]; 3] = [b"Save", b"Clr", b"Quit"];
    let action_w = 32;
    let mut ax = width - (actions.len() * (action_w + BUTTON_PAD)) - BUTTON_PAD;
    for label in actions {
        draw_button(fb, ax, SIZE_BAR_Y + 1, action_w, label, false);
        ax += action_w + BUTTON_PAD;
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

fn draw_recent_colors(fb: &mut FrameBuf, recent: &[Color; 8]) {
    let base_x = SV_SQUARE_SIZE + 8;
    let base_y = SV_SQUARE_Y + 2;

    font::draw_text(fb, b"Recent:", base_x, base_y, Color::GRAY, 1);

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

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Save,
    Clear,
    Quit,
}

fn hit_action(mx: usize, my: usize, width: usize) -> Option<Action> {
    if my < SIZE_BAR_Y + 1 || my >= SIZE_BAR_Y + 1 + BUTTON_H {
        return None;
    }
    let actions = [Action::Save, Action::Clear, Action::Quit];
    let action_w = 32;
    let mut ax = width - (actions.len() * (action_w + BUTTON_PAD)) - BUTTON_PAD;
    for action in actions {
        if mx >= ax && mx < ax + action_w {
            return Some(action);
        }
        ax += action_w + BUTTON_PAD;
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
    let prefix = b"/tmp/guskit_";
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
// Main
// ---------------------------------------------------------------------------

fn main() {
    println!("Gus Kit starting!");

    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(_) => {
            println!("Error: Could not get framebuffer info");
            process::exit(1);
        }
    };

    let width = info.left_pane_width() as usize;
    let height = info.height as usize;
    let bpp = info.bytes_per_pixel as usize;

    let fb_ptr = match graphics::fb_mmap() {
        Ok(ptr) => ptr,
        Err(e) => {
            println!("Error: Could not mmap framebuffer ({})", e);
            process::exit(1);
        }
    };

    let mut fb = unsafe {
        FrameBuf::from_raw(
            fb_ptr,
            width,
            height,
            width * bpp,
            bpp,
            info.is_bgr(),
        )
    };

    // Canvas dimensions
    let canvas_w = width;
    let canvas_h = height.saturating_sub(CANVAS_Y);
    let mut canvas = vec![255u8; canvas_w * canvas_h * 3]; // white background

    // State
    let mut tool = Tool::Pencil;
    let mut hue: u16 = 0;
    let mut saturation: u8 = 255;
    let mut value: u8 = 255;
    let mut color = hsv_to_rgb(hue, saturation, value);
    let mut brush_size = BrushSize::Medium;
    let mut mouse_down = false;
    let mut drag_start: (i32, i32) = (0, 0);
    let mut prev_mouse: (i32, i32) = (0, 0);
    let mut recent_colors = [
        Color::BLACK,
        Color::RED,
        Color::GREEN,
        Color::BLUE,
        Color::YELLOW,
        Color::MAGENTA,
        Color::CYAN,
        Color::WHITE,
    ];
    let mut save_counter: u32 = 0;
    let mut prev_buttons: u32 = 0;

    let bg = Color::rgb(40, 40, 40);

    loop {
        // Poll mouse
        let (raw_mx, raw_my, buttons) = match graphics::mouse_state() {
            Ok((x, y, b)) => (x, y, b),
            Err(_) => (0, 0, 0),
        };

        // Clamp mouse to left pane
        let mx = (raw_mx as usize).min(width.saturating_sub(1)) as i32;
        let my = (raw_my as usize).min(height.saturating_sub(1)) as i32;
        let left_down = (buttons & 1) != 0;
        let was_down = (prev_buttons & 1) != 0;

        // Mouse press
        if left_down && !was_down {
            mouse_down = true;
            let umx = mx as usize;
            let umy = my as usize;

            // Hit test UI elements first
            if let Some(t) = hit_tool(umx, umy) {
                tool = t;
                mouse_down = false;
            } else if let Some(s) = hit_size(umx, umy) {
                brush_size = s;
                mouse_down = false;
            } else if let Some(action) = hit_action(umx, umy, width) {
                mouse_down = false;
                match action {
                    Action::Save => {
                        let (path_buf, path_len) = format_save_path(save_counter);
                        let path = core::str::from_utf8(&path_buf[..path_len]).unwrap_or("/tmp/guskit.bmp");
                        let bmp_data = bmp::encode_bmp_24(canvas_w as u32, canvas_h as u32, &canvas);
                        if let Ok(file) = File::create(path) {
                            let _ = file.write(&bmp_data);
                        }
                        save_counter += 1;
                    }
                    Action::Clear => {
                        for b in canvas.iter_mut() {
                            *b = 255;
                        }
                    }
                    Action::Quit => {
                        process::exit(0);
                    }
                }
            } else if let Some(h) = hit_hue_bar(umx, umy, width) {
                hue = h;
                color = hsv_to_rgb(hue, saturation, value);
                mouse_down = false;
            } else if let Some((s, v)) = hit_sv_square(umx, umy) {
                saturation = s;
                value = v;
                color = hsv_to_rgb(hue, saturation, value);
                mouse_down = false;
            } else if let Some(i) = hit_recent(umx, umy) {
                color = recent_colors[i];
                // Reverse-derive HSV (approximate) not needed, just use color directly
                mouse_down = false;
            } else if let Some((cx, cy)) = hit_canvas(umx, umy, canvas_w, canvas_h) {
                // Start drawing on canvas
                drag_start = (cx, cy);
                prev_mouse = (cx, cy);

                match tool {
                    Tool::Pencil => {
                        canvas_put_pixel(&mut canvas, canvas_w, canvas_h, cx, cy, color);
                    }
                    Tool::Brush => {
                        canvas_fill_circle(&mut canvas, canvas_w, canvas_h, cx, cy, brush_size.radius(), color);
                    }
                    Tool::Eraser => {
                        canvas_fill_circle(&mut canvas, canvas_w, canvas_h, cx, cy, brush_size.radius(), Color::WHITE);
                    }
                    Tool::Fill => {
                        canvas_flood_fill(&mut canvas, canvas_w, canvas_h, cx, cy, color);
                        mouse_down = false;
                    }
                    Tool::Line | Tool::Rect | Tool::Circle => {
                        // Shape tools: just record start, preview during drag
                    }
                }
            }
        }

        // Mouse held (dragging)
        if left_down && was_down && mouse_down {
            if let Some((cx, cy)) = hit_canvas(mx as usize, my as usize, canvas_w, canvas_h) {
                match tool {
                    Tool::Pencil => {
                        canvas_draw_line(&mut canvas, canvas_w, canvas_h, prev_mouse.0, prev_mouse.1, cx, cy, color);
                    }
                    Tool::Brush => {
                        canvas_brush_line(&mut canvas, canvas_w, canvas_h, prev_mouse.0, prev_mouse.1, cx, cy, brush_size.radius(), color);
                    }
                    Tool::Eraser => {
                        canvas_brush_line(&mut canvas, canvas_w, canvas_h, prev_mouse.0, prev_mouse.1, cx, cy, brush_size.radius(), Color::WHITE);
                    }
                    _ => {}
                }
                prev_mouse = (cx, cy);
            }
        }

        // Mouse release
        if !left_down && was_down && mouse_down {
            mouse_down = false;
            if let Some((cx, cy)) = hit_canvas(mx as usize, my as usize, canvas_w, canvas_h) {
                match tool {
                    Tool::Line => {
                        canvas_draw_line(&mut canvas, canvas_w, canvas_h, drag_start.0, drag_start.1, cx, cy, color);
                        add_recent(&mut recent_colors, color);
                    }
                    Tool::Rect => {
                        let rx = drag_start.0.min(cx);
                        let ry = drag_start.1.min(cy);
                        let rw = (drag_start.0 - cx).abs();
                        let rh = (drag_start.1 - cy).abs();
                        canvas_fill_rect(&mut canvas, canvas_w, canvas_h, rx, ry, rw, rh, color);
                        add_recent(&mut recent_colors, color);
                    }
                    Tool::Circle => {
                        let dx = (cx - drag_start.0) as i64;
                        let dy = (cy - drag_start.1) as i64;
                        let radius = isqrt((dx * dx + dy * dy) as u64) as i32;
                        canvas_fill_circle(&mut canvas, canvas_w, canvas_h, drag_start.0, drag_start.1, radius, color);
                        add_recent(&mut recent_colors, color);
                    }
                    Tool::Pencil | Tool::Brush | Tool::Eraser => {
                        add_recent(&mut recent_colors, color);
                    }
                    _ => {}
                }
            }
        }

        prev_buttons = buttons;

        // -- Render --

        // Background for UI area
        shapes::fill_rect(&mut fb, 0, 0, width as i32, CANVAS_Y as i32, bg);

        // Blit canvas
        blit_canvas(&mut fb, &canvas, canvas_w, canvas_h);

        // Shape preview (rubber-band on framebuf only)
        if mouse_down && tool.is_shape() {
            if let Some((cx, cy)) = hit_canvas(mx as usize, my as usize, canvas_w, canvas_h) {
                let preview_color = color;
                match tool {
                    Tool::Line => {
                        shapes::draw_line(
                            &mut fb,
                            drag_start.0,
                            drag_start.1 + CANVAS_Y as i32,
                            cx,
                            cy + CANVAS_Y as i32,
                            preview_color,
                        );
                    }
                    Tool::Rect => {
                        let rx = drag_start.0.min(cx);
                        let ry = drag_start.1.min(cy) + CANVAS_Y as i32;
                        let rw = (drag_start.0 - cx).abs();
                        let rh = (drag_start.1 - cy).abs();
                        shapes::draw_rect(&mut fb, rx, ry, rw, rh, preview_color);
                    }
                    Tool::Circle => {
                        let dx = (cx - drag_start.0) as i64;
                        let dy = (cy - drag_start.1) as i64;
                        let radius = isqrt((dx * dx + dy * dy) as u64) as i32;
                        shapes::draw_circle(
                            &mut fb,
                            drag_start.0,
                            drag_start.1 + CANVAS_Y as i32,
                            radius,
                            preview_color,
                        );
                    }
                    _ => {}
                }
            }
        }

        // Draw UI
        draw_toolbar(&mut fb, tool, width);
        draw_size_bar(&mut fb, brush_size, width);
        draw_hue_bar(&mut fb, width, hue);
        draw_current_color_swatch(&mut fb, width, color);
        draw_sv_square(&mut fb, hue, saturation, value);
        draw_recent_colors(&mut fb, &recent_colors);

        // Cursor
        if mx >= 0 && mx < width as i32 && my >= 0 && my < height as i32 {
            draw_cursor(&mut fb, mx, my, width as i32, height as i32);
        }

        // Flush
        if let Some(dirty) = fb.take_dirty() {
            let _ = graphics::fb_flush_rect(dirty.x, dirty.y, dirty.w, dirty.h);
        } else {
            let _ = graphics::fb_flush();
        }

        let _ = time::sleep_ms(16);
    }
}
