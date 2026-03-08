//! Breenix Window Manager (bwm) — Tiling Window Manager
//!
//! Userspace tiling window manager with GPU-accelerated compositing.
//! Windows are arranged in a tiled layout with draggable boundaries.
//! Each window has a title bar showing the process name, which also
//! serves as a drag handle for repositioning.
//!
//! Default layout:
//!   Left (60%): Shell terminal (bsh)
//!   Right top (40%x50%): Bounce demo
//!   Right bottom (40%x50%): Log viewer (bless)

use std::process;

use libbreenix::graphics;
use libbreenix::io;
use libbreenix::fs;
use libbreenix::process::{fork, exec, waitpid, setsid, ForkResult, WNOHANG};
use libbreenix::pty;
use libbreenix::types::Fd;

use libgfx::bitmap_font;
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Title bar height in pixels
const TITLE_BAR_HEIGHT: usize = 22;

/// Border width between tiles
const BORDER_WIDTH: usize = 4;

/// Hit target for border dragging (larger than visual border)
const BORDER_HIT_TARGET: usize = 8;

/// Noto Sans Mono 16px cell dimensions for terminal text.
/// CELL_W must match bitmap_font::metrics().char_width (7 for size_16).
const CELL_W: usize = 7;
const CELL_H: usize = 18;
/// Terminal padding inside tile content area
const TERM_PADDING: usize = 4;

// Colors
const BG_COLOR: Color = Color::rgb(20, 30, 50);
const FG_COLOR: Color = Color::rgb(255, 255, 255);
const TITLE_FOCUSED_BG: Color = Color::rgb(50, 70, 110);
const TITLE_UNFOCUSED_BG: Color = Color::rgb(35, 45, 65);
const TITLE_TEXT: Color = Color::rgb(220, 220, 220);
const TITLE_FOCUSED_TEXT: Color = Color::rgb(255, 255, 255);
const BORDER_COLOR: Color = Color::rgb(40, 55, 80);
const BORDER_ACTIVE_COLOR: Color = Color::rgb(80, 120, 180);
const CURSOR_COLOR: Color = Color::rgb(255, 255, 255);
const DESKTOP_BG: Color = Color::rgb(25, 35, 55);

// ANSI color palette (standard 8 colors)
const ANSI_COLORS: [Color; 8] = [
    Color::rgb(0, 0, 0),       // 0: black
    Color::rgb(205, 49, 49),   // 1: red
    Color::rgb(13, 188, 121),  // 2: green
    Color::rgb(229, 229, 16),  // 3: yellow
    Color::rgb(36, 114, 200),  // 4: blue
    Color::rgb(188, 63, 188),  // 5: magenta
    Color::rgb(17, 168, 205),  // 6: cyan
    Color::rgb(229, 229, 229), // 7: white
];

// ─── Mouse Cursor ───────────────────────────────────────────────────────────

/// Arrow cursor bitmap (1 = white, 2 = black outline, 0 = transparent). 12x18.
const MOUSE_CURSOR: [[u8; 12]; 18] = [
    [2,0,0,0,0,0,0,0,0,0,0,0],
    [2,2,0,0,0,0,0,0,0,0,0,0],
    [2,1,2,0,0,0,0,0,0,0,0,0],
    [2,1,1,2,0,0,0,0,0,0,0,0],
    [2,1,1,1,2,0,0,0,0,0,0,0],
    [2,1,1,1,1,2,0,0,0,0,0,0],
    [2,1,1,1,1,1,2,0,0,0,0,0],
    [2,1,1,1,1,1,1,2,0,0,0,0],
    [2,1,1,1,1,1,1,1,2,0,0,0],
    [2,1,1,1,1,1,1,1,1,2,0,0],
    [2,1,1,1,1,1,1,1,1,1,2,0],
    [2,1,1,1,1,1,2,2,2,2,2,0],
    [2,1,1,1,1,2,0,0,0,0,0,0],
    [2,1,1,2,1,1,2,0,0,0,0,0],
    [2,1,2,0,2,1,1,2,0,0,0,0],
    [2,2,0,0,2,1,1,2,0,0,0,0],
    [2,0,0,0,0,2,1,2,0,0,0,0],
    [0,0,0,0,0,2,2,0,0,0,0,0],
];

const CURSOR_W: usize = 12;
const CURSOR_H: usize = 18;

struct CursorState {
    saved_bg: [u32; CURSOR_W * CURSOR_H],
    drawn: bool,
    last_x: usize,
    last_y: usize,
}

impl CursorState {
    fn new() -> Self {
        Self { saved_bg: [0; CURSOR_W * CURSOR_H], drawn: false, last_x: 0, last_y: 0 }
    }

    fn erase(&mut self, fb: &mut FrameBuf) {
        if !self.drawn { return; }
        let w = fb.width;
        let h = fb.height;
        for row in 0..CURSOR_H {
            let py = self.last_y + row;
            if py >= h { break; }
            for col in 0..CURSOR_W {
                let px = self.last_x + col;
                if px >= w { break; }
                if MOUSE_CURSOR[row][col] != 0 {
                    let packed = self.saved_bg[row * CURSOR_W + col];
                    let r = ((packed >> 16) & 0xFF) as u8;
                    let g = ((packed >> 8) & 0xFF) as u8;
                    let b = (packed & 0xFF) as u8;
                    fb.put_pixel(px, py, Color::rgb(r, g, b));
                }
            }
        }
        self.drawn = false;
    }

    fn draw(&mut self, fb: &mut FrameBuf, mx: usize, my: usize) {
        let w = fb.width;
        let h = fb.height;
        for row in 0..CURSOR_H {
            let py = my + row;
            if py >= h { break; }
            for col in 0..CURSOR_W {
                let px = mx + col;
                if px >= w { break; }
                if MOUSE_CURSOR[row][col] != 0 {
                    let c = fb.get_pixel(px, py);
                    self.saved_bg[row * CURSOR_W + col] =
                        ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32);
                }
            }
        }
        for row in 0..CURSOR_H {
            let py = my + row;
            if py >= h { break; }
            for col in 0..CURSOR_W {
                let px = mx + col;
                if px >= w { break; }
                match MOUSE_CURSOR[row][col] {
                    1 => fb.put_pixel(px, py, Color::WHITE),
                    2 => fb.put_pixel(px, py, Color::rgb(0, 0, 0)),
                    _ => {}
                }
            }
        }
        self.last_x = mx;
        self.last_y = my;
        self.drawn = true;
    }

    fn update(&mut self, fb: &mut FrameBuf, mx: usize, my: usize) {
        if self.drawn && self.last_x == mx && self.last_y == my {
            return;
        }
        self.erase(fb);
        self.draw(fb, mx, my);
    }
}

// ─── Character Cell ──────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    fg: Color,
    bg: Color,
    bold: bool,
}

impl Cell {
    const fn blank() -> Self {
        Self {
            ch: b' ',
            fg: FG_COLOR,
            bg: BG_COLOR,
            bold: false,
        }
    }
}

// ─── ANSI Parser State Machine ───────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum AnsiState {
    Normal,
    Escape,
    Csi,
    CsiParam,
    OscString,
}

// ─── Terminal Emulator ───────────────────────────────────────────────────────

struct TermEmu {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    cursor_x: usize,
    cursor_y: usize,
    fg: Color,
    bg: Color,
    bold: bool,
    ansi_state: AnsiState,
    ansi_params: [u16; 8],
    ansi_param_idx: usize,
    dirty: bool,
}

impl TermEmu {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(); cols * rows],
            cursor_x: 0,
            cursor_y: 0,
            fg: FG_COLOR,
            bg: BG_COLOR,
            bold: false,
            ansi_state: AnsiState::Normal,
            ansi_params: [0; 8],
            ansi_param_idx: 0,
            dirty: true,
        }
    }

    fn cell(&self, x: usize, y: usize) -> &Cell {
        &self.cells[y * self.cols + x]
    }

    fn cell_mut(&mut self, x: usize, y: usize) -> &mut Cell {
        &mut self.cells[y * self.cols + x]
    }

    fn scroll_up(&mut self) {
        let cols = self.cols;
        for y in 1..self.rows {
            for x in 0..cols {
                self.cells[(y - 1) * cols + x] = self.cells[y * cols + x];
            }
        }
        let last_row = self.rows - 1;
        for x in 0..cols {
            self.cells[last_row * cols + x] = Cell::blank();
        }
        self.dirty = true;
    }

    fn put_char(&mut self, ch: u8) {
        if self.cursor_x >= self.cols {
            self.cursor_x = 0;
            self.cursor_y += 1;
        }
        if self.cursor_y >= self.rows {
            self.scroll_up();
            self.cursor_y = self.rows - 1;
        }
        let fg = self.fg;
        let bg = self.bg;
        let bold = self.bold;
        let idx = self.cursor_y * self.cols + self.cursor_x;
        self.cells[idx] = Cell { ch, fg, bg, bold };
        self.cursor_x += 1;
        self.dirty = true;
    }

    fn feed(&mut self, byte: u8) {
        match self.ansi_state {
            AnsiState::Normal => match byte {
                0x1b => self.ansi_state = AnsiState::Escape,
                b'\n' => {
                    self.cursor_x = 0;
                    self.cursor_y += 1;
                    if self.cursor_y >= self.rows {
                        self.scroll_up();
                        self.cursor_y = self.rows - 1;
                    }
                    self.dirty = true;
                }
                b'\r' => {
                    self.cursor_x = 0;
                    self.dirty = true;
                }
                b'\t' => {
                    let next_tab = (self.cursor_x + 8) & !7;
                    while self.cursor_x < next_tab && self.cursor_x < self.cols {
                        self.put_char(b' ');
                    }
                }
                0x08 => {
                    if self.cursor_x > 0 {
                        self.cursor_x -= 1;
                        self.dirty = true;
                    }
                }
                0x07 => {}
                ch if ch >= 0x20 => self.put_char(ch),
                _ => {}
            },
            AnsiState::Escape => match byte {
                b'[' => {
                    self.ansi_state = AnsiState::Csi;
                    self.ansi_params = [0; 8];
                    self.ansi_param_idx = 0;
                }
                b']' => {
                    self.ansi_state = AnsiState::OscString;
                }
                b'O' => {
                    self.ansi_state = AnsiState::Normal;
                }
                _ => {
                    self.ansi_state = AnsiState::Normal;
                }
            },
            AnsiState::Csi | AnsiState::CsiParam => {
                if byte >= b'0' && byte <= b'9' {
                    self.ansi_state = AnsiState::CsiParam;
                    let idx = self.ansi_param_idx;
                    if idx < 8 {
                        self.ansi_params[idx] = self.ansi_params[idx]
                            .saturating_mul(10)
                            .saturating_add((byte - b'0') as u16);
                    }
                } else if byte == b';' {
                    if self.ansi_param_idx < 7 {
                        self.ansi_param_idx += 1;
                    }
                } else {
                    let nparams = self.ansi_param_idx + 1;
                    self.execute_csi(byte, nparams);
                    self.ansi_state = AnsiState::Normal;
                }
            }
            AnsiState::OscString => {
                if byte == 0x07 || byte == b'\\' {
                    self.ansi_state = AnsiState::Normal;
                }
            }
        }
    }

    fn execute_csi(&mut self, cmd: u8, nparams: usize) {
        let p0 = self.ansi_params[0] as usize;
        let p1 = if nparams > 1 { self.ansi_params[1] as usize } else { 0 };

        match cmd {
            b'H' | b'f' => {
                let row = if p0 > 0 { p0 - 1 } else { 0 };
                let col = if p1 > 0 { p1 - 1 } else { 0 };
                self.cursor_y = row.min(self.rows - 1);
                self.cursor_x = col.min(self.cols - 1);
                self.dirty = true;
            }
            b'A' => {
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_y = self.cursor_y.saturating_sub(n);
                self.dirty = true;
            }
            b'B' => {
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_y = (self.cursor_y + n).min(self.rows - 1);
                self.dirty = true;
            }
            b'C' => {
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_x = (self.cursor_x + n).min(self.cols - 1);
                self.dirty = true;
            }
            b'D' => {
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_x = self.cursor_x.saturating_sub(n);
                self.dirty = true;
            }
            b'G' => {
                let col = if p0 > 0 { p0 - 1 } else { 0 };
                self.cursor_x = col.min(self.cols - 1);
                self.dirty = true;
            }
            b'J' => {
                match p0 {
                    0 => {
                        for x in self.cursor_x..self.cols {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                        for y in (self.cursor_y + 1)..self.rows {
                            for x in 0..self.cols {
                                *self.cell_mut(x, y) = Cell::blank();
                            }
                        }
                    }
                    1 => {
                        for y in 0..self.cursor_y {
                            for x in 0..self.cols {
                                *self.cell_mut(x, y) = Cell::blank();
                            }
                        }
                        for x in 0..=self.cursor_x.min(self.cols - 1) {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                    }
                    2 | 3 => {
                        for y in 0..self.rows {
                            for x in 0..self.cols {
                                *self.cell_mut(x, y) = Cell::blank();
                            }
                        }
                        self.cursor_x = 0;
                        self.cursor_y = 0;
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            b'K' => {
                match p0 {
                    0 => {
                        for x in self.cursor_x..self.cols {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                    }
                    1 => {
                        for x in 0..=self.cursor_x.min(self.cols - 1) {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                    }
                    2 => {
                        for x in 0..self.cols {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            b'd' => {
                let row = if p0 > 0 { p0 - 1 } else { 0 };
                self.cursor_y = row.min(self.rows - 1);
                self.dirty = true;
            }
            b'r' => {
                self.cursor_x = 0;
                self.cursor_y = 0;
                self.dirty = true;
            }
            b'm' => {
                if nparams == 1 && p0 == 0 {
                    self.fg = FG_COLOR;
                    self.bg = BG_COLOR;
                    self.bold = false;
                } else {
                    for i in 0..nparams {
                        let code = self.ansi_params[i];
                        match code {
                            0 => { self.fg = FG_COLOR; self.bg = BG_COLOR; self.bold = false; }
                            1 => { self.bold = true; }
                            22 => { self.bold = false; }
                            30..=37 => { self.fg = ANSI_COLORS[(code - 30) as usize]; }
                            39 => { self.fg = FG_COLOR; }
                            40..=47 => { self.bg = ANSI_COLORS[(code - 40) as usize]; }
                            49 => { self.bg = BG_COLOR; }
                            90..=97 => {
                                let mut c = ANSI_COLORS[(code - 90) as usize];
                                c.r = c.r.saturating_add(60);
                                c.g = c.g.saturating_add(60);
                                c.b = c.b.saturating_add(60);
                                self.fg = c;
                            }
                            _ => {}
                        }
                    }
                }
            }
            b'l' | b'h' => {}
            _ => {}
        }
    }

    fn render(&mut self, fb: &mut FrameBuf, x_off: usize, y_off: usize) {
        if !self.dirty { return; }
        self.dirty = false;

        let fm = bitmap_font::metrics();

        for row in 0..self.rows {
            let py = y_off + row * CELL_H;
            if py + CELL_H > fb.height { break; }
            for col in 0..self.cols {
                let cell = self.cell(col, row);
                let px = x_off + col * CELL_W;
                if px + CELL_W > fb.width { break; }

                // Fill background
                for dy in 0..CELL_H {
                    for dx in 0..CELL_W {
                        fb.put_pixel(px + dx, py + dy, cell.bg);
                    }
                }
                // Draw glyph using bitmap_font (alpha-blends against bg already in fb)
                if cell.ch != b' ' {
                    let fg = if cell.bold {
                        Color::rgb(
                            cell.fg.r.saturating_add(40),
                            cell.fg.g.saturating_add(40),
                            cell.fg.b.saturating_add(40),
                        )
                    } else {
                        cell.fg
                    };
                    bitmap_font::draw_char(fb, cell.ch as char, px, py, fg);
                }
            }
            // Cursor underline
            if row == self.cursor_y && self.cursor_x < self.cols {
                let cx = x_off + self.cursor_x * CELL_W;
                let cw = fm.char_width.min(CELL_W);
                for dy in 0..2usize {
                    for dx in 0..cw {
                        if cx + dx < fb.width && py + CELL_H - 2 + dy < fb.height {
                            fb.put_pixel(cx + dx, py + CELL_H - 2 + dy, CURSOR_COLOR);
                        }
                    }
                }
            }
        }
    }

    /// Resize the terminal, preserving content where possible
    fn resize(&mut self, new_cols: usize, new_rows: usize) {
        if new_cols == self.cols && new_rows == self.rows { return; }
        let mut new_cells = vec![Cell::blank(); new_cols * new_rows];
        let copy_cols = self.cols.min(new_cols);
        let copy_rows = self.rows.min(new_rows);
        for y in 0..copy_rows {
            for x in 0..copy_cols {
                new_cells[y * new_cols + x] = self.cells[y * self.cols + x];
            }
        }
        self.cells = new_cells;
        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_x = self.cursor_x.min(new_cols.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(new_rows.saturating_sub(1));
        self.dirty = true;
    }
}

// ─── Input Parser ────────────────────────────────────────────────────────────

struct InputParser {
    state: InputState,
    esc_buf: [u8; 8],
    esc_len: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum InputState {
    Normal,
    Escape,
    CsiOrSS3,
}

enum InputEvent {
    Char(u8),
    FunctionKey(u8),
    ArrowUp,
    ArrowDown,
    ArrowRight,
    ArrowLeft,
}

impl InputParser {
    fn new() -> Self {
        Self { state: InputState::Normal, esc_buf: [0; 8], esc_len: 0 }
    }

    fn feed(&mut self, byte: u8) -> Option<InputEvent> {
        match self.state {
            InputState::Normal => {
                if byte == 0x1b {
                    self.state = InputState::Escape;
                    self.esc_len = 0;
                    None
                } else {
                    Some(InputEvent::Char(byte))
                }
            }
            InputState::Escape => {
                if byte == b'[' || byte == b'O' {
                    self.state = InputState::CsiOrSS3;
                    self.esc_buf[0] = byte;
                    self.esc_len = 1;
                    None
                } else {
                    self.state = InputState::Normal;
                    Some(InputEvent::Char(byte))
                }
            }
            InputState::CsiOrSS3 => {
                if self.esc_len < 7 {
                    self.esc_buf[self.esc_len] = byte;
                    self.esc_len += 1;
                }
                if byte >= 0x40 && byte <= 0x7e {
                    self.state = InputState::Normal;
                    self.decode_escape()
                } else {
                    None
                }
            }
        }
    }

    fn decode_escape(&self) -> Option<InputEvent> {
        if self.esc_len < 2 { return None; }
        let final_byte = self.esc_buf[self.esc_len - 1];

        if self.esc_buf[0] == b'[' {
            match final_byte {
                b'A' => return Some(InputEvent::ArrowUp),
                b'B' => return Some(InputEvent::ArrowDown),
                b'C' => return Some(InputEvent::ArrowRight),
                b'D' => return Some(InputEvent::ArrowLeft),
                b'~' if self.esc_len >= 3 => {
                    let num = self.esc_buf[1] - b'0';
                    if self.esc_len == 3 {
                        match num {
                            1 => return Some(InputEvent::FunctionKey(1)),
                            _ => {}
                        }
                    } else if self.esc_len == 4 {
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

// ─── Tiling Layout ───────────────────────────────────────────────────────────

/// Each tile occupies a rectangle and contains either a terminal or a client window.
struct Tile {
    // Layout position and size (in screen pixels)
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    // Title
    title: &'static str,
    // Content
    kind: TileKind,
}

enum TileKind {
    Terminal {
        emu: TermEmu,
        master_fd: Option<Fd>,
        child_pid: i64,
        program: &'static [u8],
    },
    ClientWindow {
        window_id: u32,
    },
}

impl Tile {
    /// Content area (below title bar)
    fn content_x(&self) -> usize { self.x }
    fn content_y(&self) -> usize { self.y + TITLE_BAR_HEIGHT }
    fn content_width(&self) -> usize { self.width }
    fn content_height(&self) -> usize { self.height.saturating_sub(TITLE_BAR_HEIGHT) }

    /// Terminal dimensions in cells for this tile's content area
    fn term_dims(&self) -> (usize, usize) {
        let pw = self.content_width().saturating_sub(TERM_PADDING * 2);
        let ph = self.content_height().saturating_sub(TERM_PADDING * 2);
        (pw / CELL_W, ph / CELL_H)
    }
}

/// The tiling layout: a vertical split, with the right side optionally split horizontally.
struct TileLayout {
    screen_w: usize,
    screen_h: usize,
    /// Vertical split position (x coordinate of the border)
    vsplit_x: usize,
    /// Horizontal split position on the right side (y coordinate)
    hsplit_y: usize,
}

impl TileLayout {
    fn new(screen_w: usize, screen_h: usize) -> Self {
        let vsplit_x = screen_w * 60 / 100; // 60% left
        let hsplit_y = screen_h / 2;         // 50/50 right
        Self { screen_w, screen_h, vsplit_x, hsplit_y }
    }

    /// Recompute tile positions from split ratios. Returns (left, right_top, right_bottom).
    fn compute_tile_rects(&self) -> [(usize, usize, usize, usize); 3] {
        let vx = self.vsplit_x;
        let hy = self.hsplit_y;
        let sw = self.screen_w;
        let sh = self.screen_h;
        let bw = BORDER_WIDTH;

        // Left tile: full height
        let left = (0, 0, vx, sh);
        // Right top: from vsplit to screen right, top to hsplit
        let rt = (vx + bw, 0, sw - vx - bw, hy);
        // Right bottom: from vsplit to screen right, hsplit to bottom
        let rb = (vx + bw, hy + bw, sw - vx - bw, sh - hy - bw);

        [left, rt, rb]
    }

    /// Apply computed rects to tiles
    fn apply_to_tiles(&self, tiles: &mut [Tile]) {
        let rects = self.compute_tile_rects();
        for (i, tile) in tiles.iter_mut().enumerate() {
            if i < 3 {
                tile.x = rects[i].0;
                tile.y = rects[i].1;
                tile.width = rects[i].2;
                tile.height = rects[i].3;
            }
        }
    }
}

/// What the mouse is currently dragging
#[derive(Clone, Copy, PartialEq)]
enum DragState {
    None,
    VSplit,              // Dragging the vertical split border
    HSplit,              // Dragging the horizontal split border
    TitleBar(usize),     // Dragging tile i's title bar
}

// ─── Drawing Helpers ─────────────────────────────────────────────────────────

fn draw_text_tight(fb: &mut FrameBuf, text: &[u8], x: usize, y: usize, color: Color) {
    let fm = bitmap_font::metrics();
    for (i, &ch) in text.iter().enumerate() {
        let px = x + i * CELL_W;
        if px + fm.char_width > fb.width { break; }
        bitmap_font::draw_char(fb, ch as char, px, y, color);
    }
}

fn fill_rect(fb: &mut FrameBuf, x: usize, y: usize, w: usize, h: usize, color: Color) {
    for dy in 0..h {
        let py = y + dy;
        if py >= fb.height { break; }
        for dx in 0..w {
            let px = x + dx;
            if px >= fb.width { break; }
            fb.put_pixel(px, py, color);
        }
    }
}

fn draw_title_bar(fb: &mut FrameBuf, tile: &Tile, focused: bool) {
    let bg = if focused { TITLE_FOCUSED_BG } else { TITLE_UNFOCUSED_BG };
    let fg = if focused { TITLE_FOCUSED_TEXT } else { TITLE_TEXT };
    fill_rect(fb, tile.x, tile.y, tile.width, TITLE_BAR_HEIGHT, bg);
    let text_x = tile.x + 8;
    let text_y = tile.y + 4;
    draw_text_tight(fb, tile.title.as_bytes(), text_x, text_y, fg);
}

fn draw_borders(fb: &mut FrameBuf, layout: &TileLayout, drag: DragState) {
    let vx = layout.vsplit_x;
    let hy = layout.hsplit_y;
    let sw = layout.screen_w;
    let sh = layout.screen_h;
    let bw = BORDER_WIDTH;

    // Vertical border
    let vcolor = if drag == DragState::VSplit { BORDER_ACTIVE_COLOR } else { BORDER_COLOR };
    fill_rect(fb, vx, 0, bw, sh, vcolor);

    // Horizontal border (right side only)
    let hcolor = if drag == DragState::HSplit { BORDER_ACTIVE_COLOR } else { BORDER_COLOR };
    fill_rect(fb, vx + bw, hy, sw - vx - bw, bw, hcolor);
}

// ─── Process Spawning ────────────────────────────────────────────────────────

fn spawn_child(path: &[u8], _name: &str) -> (Fd, i64) {
    let (master_fd, slave_name) = match pty::openpty() {
        Ok((m, s)) => (m, s),
        Err(_) => return (Fd::from_raw(0), -1),
    };

    // slave_name is [u8; 32] with null-terminated path
    let len = slave_name.iter().position(|&b| b == 0).unwrap_or(slave_name.len());
    let slave_path_str = core::str::from_utf8(&slave_name[..len]).unwrap_or("/dev/pts/0");
    // fs::open requires null-terminated str, append \0
    let mut slave_path_buf = String::from(slave_path_str);
    slave_path_buf.push('\0');

    match fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(master_fd);
            let _ = setsid();
            let slave_fd = match fs::open(&slave_path_buf, fs::O_RDWR) {
                Ok(fd) => fd,
                Err(_) => libbreenix::process::exit(126),
            };
            let _ = libbreenix::termios::set_controlling_terminal(slave_fd);
            let _ = io::dup2(slave_fd, Fd::from_raw(0));
            let _ = io::dup2(slave_fd, Fd::from_raw(1));
            let _ = io::dup2(slave_fd, Fd::from_raw(2));
            if slave_fd.raw() > 2 {
                let _ = io::close(slave_fd);
            }
            let _ = exec(path);
            libbreenix::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            (master_fd, child_pid.raw() as i64)
        }
        Err(_) => {
            let _ = io::close(master_fd);
            (Fd::from_raw(0), -1)
        }
    }
}

// ─── Hit Testing ─────────────────────────────────────────────────────────────

fn hit_test_vsplit(layout: &TileLayout, mx: usize, _my: usize) -> bool {
    let vx = layout.vsplit_x;
    mx >= vx.saturating_sub(BORDER_HIT_TARGET / 2)
        && mx < vx + BORDER_WIDTH + BORDER_HIT_TARGET / 2
}

fn hit_test_hsplit(layout: &TileLayout, mx: usize, my: usize) -> bool {
    let vx = layout.vsplit_x + BORDER_WIDTH;
    let hy = layout.hsplit_y;
    mx >= vx
        && my >= hy.saturating_sub(BORDER_HIT_TARGET / 2)
        && my < hy + BORDER_WIDTH + BORDER_HIT_TARGET / 2
}

fn hit_test_title_bar(tiles: &[Tile], mx: usize, my: usize) -> Option<usize> {
    for (i, tile) in tiles.iter().enumerate() {
        if mx >= tile.x && mx < tile.x + tile.width
            && my >= tile.y && my < tile.y + TITLE_BAR_HEIGHT
        {
            return Some(i);
        }
    }
    None
}

fn hit_test_tile(tiles: &[Tile], mx: usize, my: usize) -> Option<usize> {
    for (i, tile) in tiles.iter().enumerate() {
        if mx >= tile.x && mx < tile.x + tile.width
            && my >= tile.y && my < tile.y + tile.height
        {
            return Some(i);
        }
    }
    None
}

// ─── Main ────────────────────────────────────────────────────────────────────

fn main() {
    print!("[bwm] Breenix Tiling Window Manager starting...\n");

    // Step 1: Take over display
    if let Err(e) = graphics::take_over_display() {
        print!("[bwm] WARNING: take_over_display failed: {}\n", e);
    }

    // Step 2: Get framebuffer info
    let info = {
        let mut result = None;
        for attempt in 0..10 {
            match graphics::fbinfo() {
                Ok(info) => { result = Some(info); break; }
                Err(_) if attempt < 9 => {
                    let _ = libbreenix::time::nanosleep(&libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 10_000_000 });
                }
                Err(e) => {
                    print!("[bwm] ERROR: fbinfo failed: {}\n", e);
                    process::exit(1);
                }
            }
        }
        result.unwrap()
    };

    let screen_w = info.width as usize;
    let screen_h = info.height as usize;
    let bpp = info.bytes_per_pixel as usize;

    // GPU compositing mode check
    let gpu_compositing = {
        let test_pixel: [u32; 1] = [0xFF000000];
        graphics::virgl_composite(&test_pixel, 1, 1).is_ok()
    };

    if !gpu_compositing {
        print!("[bwm] ERROR: GPU compositing required for tiling WM\n");
        process::exit(1);
    }

    print!("[bwm] GPU compositing mode (VirGL)\n");
    let mut composite_buf = vec![0u32; screen_w * screen_h];

    let mut fb = unsafe {
        FrameBuf::from_raw(
            composite_buf.as_mut_ptr() as *mut u8,
            screen_w,
            screen_h,
            screen_w * bpp,
            bpp,
            info.is_bgr(),
        )
    };

    // Step 3: Create tiling layout
    let mut layout = TileLayout::new(screen_w, screen_h);

    // Step 4: Spawn children and create tiles
    let (shell_master, shell_pid) = spawn_child(b"/bin/bsh\0", "bsh");
    let (bless_master, bless_pid) = spawn_child(b"/bin/bless\0", "bless");

    let rects = layout.compute_tile_rects();

    // Compute initial terminal sizes
    let shell_cols = (rects[0].2.saturating_sub(TERM_PADDING * 2)) / CELL_W;
    let shell_rows = (rects[0].3.saturating_sub(TITLE_BAR_HEIGHT + TERM_PADDING * 2)) / CELL_H;
    let logs_cols = (rects[2].2.saturating_sub(TERM_PADDING * 2)) / CELL_W;
    let logs_rows = (rects[2].3.saturating_sub(TITLE_BAR_HEIGHT + TERM_PADDING * 2)) / CELL_H;

    let mut tiles: Vec<Tile> = vec![
        Tile {
            x: rects[0].0, y: rects[0].1, width: rects[0].2, height: rects[0].3,
            title: "Shell",
            kind: TileKind::Terminal {
                emu: TermEmu::new(shell_cols, shell_rows),
                master_fd: Some(shell_master),
                child_pid: shell_pid,
                program: b"/bin/bsh\0",
            },
        },
        Tile {
            x: rects[1].0, y: rects[1].1, width: rects[1].2, height: rects[1].3,
            title: "Bounce",
            kind: TileKind::ClientWindow { window_id: 0 },
        },
        Tile {
            x: rects[2].0, y: rects[2].1, width: rects[2].2, height: rects[2].3,
            title: "Logs",
            kind: TileKind::Terminal {
                emu: TermEmu::new(logs_cols, logs_rows),
                master_fd: Some(bless_master),
                child_pid: bless_pid,
                program: b"/bin/bless\0",
            },
        },
    ];

    // Set PTY window sizes
    let ws_shell = libbreenix::termios::Winsize {
        ws_row: shell_rows as u16, ws_col: shell_cols as u16,
        ws_xpixel: 0, ws_ypixel: 0,
    };
    let _ = libbreenix::termios::set_winsize(shell_master, &ws_shell);
    let ws_logs = libbreenix::termios::Winsize {
        ws_row: logs_rows as u16, ws_col: logs_cols as u16,
        ws_xpixel: 0, ws_ypixel: 0,
    };
    let _ = libbreenix::termios::set_winsize(bless_master, &ws_logs);

    let font_m = bitmap_font::metrics();
    print!("[bwm] Font metrics: char_width={}, char_height={}, line_height={}\n",
           font_m.char_width, font_m.char_height, font_m.line_height());
    print!("[bwm] Display: {}x{}, shell: {}x{} cells, logs: {}x{} cells\n",
           screen_w, screen_h, shell_cols, shell_rows, logs_cols, logs_rows);

    // Step 5: Enter raw mode on stdin
    let mut orig_termios = libbreenix::termios::Termios::default();
    let _ = libbreenix::termios::tcgetattr(Fd::from_raw(0), &mut orig_termios);
    let mut raw = orig_termios;
    libbreenix::termios::cfmakeraw(&mut raw);
    let _ = libbreenix::termios::tcsetattr(Fd::from_raw(0), libbreenix::termios::TCSANOW, &raw);

    let mut focused_tile: usize = 0;
    let mut input_parser = InputParser::new();
    let mut frame: u32 = 0;
    let mut prev_mouse_buttons: u32 = 0;
    let mut mouse_x: usize = 0;
    let mut mouse_y: usize = 0;
    let mut cursor_state = CursorState::new();
    let mut drag_state = DragState::None;
    let mut needs_full_redraw = true;

    // Initial render
    fb.clear(DESKTOP_BG);
    draw_borders(&mut fb, &layout, drag_state);
    for (i, tile) in tiles.iter().enumerate() {
        draw_title_bar(&mut fb, tile, i == focused_tile);
    }
    // Render terminal tiles
    for tile in tiles.iter_mut() {
        let cx = tile.content_x() + TERM_PADDING;
        let cy = tile.content_y() + TERM_PADDING;
        if let TileKind::Terminal { ref mut emu, .. } = tile.kind {
            emu.render(&mut fb, cx, cy);
        }
    }

    let _ = graphics::virgl_composite_windows(&composite_buf, screen_w as u32, screen_h as u32, true);

    // Step 6: Main loop
    let mut read_buf = [0u8; 512];
    let mut needs_flush = false;

    // Collect FDs for polling
    let mut poll_fds = [
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }, // stdin
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }, // shell
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }, // logs
    ];

    loop {
        // Setup poll FDs
        let mut nfds = 1; // stdin
        poll_fds[0].revents = 0;

        let mut shell_poll_idx: Option<usize> = None;
        let mut logs_poll_idx: Option<usize> = None;

        if let TileKind::Terminal { master_fd: Some(fd), .. } = &tiles[0].kind {
            poll_fds[nfds].fd = fd.raw() as i32;
            poll_fds[nfds].revents = 0;
            shell_poll_idx = Some(nfds);
            nfds += 1;
        }
        if let TileKind::Terminal { master_fd: Some(fd), .. } = &tiles[2].kind {
            poll_fds[nfds].fd = fd.raw() as i32;
            poll_fds[nfds].revents = 0;
            logs_poll_idx = Some(nfds);
            nfds += 1;
        }

        let _nready = io::poll(&mut poll_fds[..nfds], 0).unwrap_or(0);

        // Read stdin (keyboard) → route to focused terminal
        if poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0 {
            if let Ok(n) = io::read(Fd::from_raw(0), &mut read_buf) {
                for i in 0..n {
                    let byte = read_buf[i];
                    if let Some(event) = input_parser.feed(byte) {
                        match event {
                            InputEvent::FunctionKey(k) if k >= 1 && k <= 3 => {
                                // F1-F3: switch focus
                                let new_focus = (k - 1) as usize;
                                if new_focus < tiles.len() && new_focus != focused_tile {
                                    let old = focused_tile;
                                    focused_tile = new_focus;
                                    draw_title_bar(&mut fb, &tiles[old], false);
                                    draw_title_bar(&mut fb, &tiles[focused_tile], true);
                                    needs_flush = true;
                                }
                            }
                            InputEvent::Char(c) => {
                                if let TileKind::Terminal { master_fd: Some(fd), .. } = &tiles[focused_tile].kind {
                                    let _ = io::write(*fd, &[c]);
                                }
                            }
                            InputEvent::ArrowUp => {
                                if let TileKind::Terminal { master_fd: Some(fd), .. } = &tiles[focused_tile].kind {
                                    let _ = io::write(*fd, b"\x1b[A");
                                }
                            }
                            InputEvent::ArrowDown => {
                                if let TileKind::Terminal { master_fd: Some(fd), .. } = &tiles[focused_tile].kind {
                                    let _ = io::write(*fd, b"\x1b[B");
                                }
                            }
                            InputEvent::ArrowRight => {
                                if let TileKind::Terminal { master_fd: Some(fd), .. } = &tiles[focused_tile].kind {
                                    let _ = io::write(*fd, b"\x1b[C");
                                }
                            }
                            InputEvent::ArrowLeft => {
                                if let TileKind::Terminal { master_fd: Some(fd), .. } = &tiles[focused_tile].kind {
                                    let _ = io::write(*fd, b"\x1b[D");
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Read shell PTY output
        if let Some(idx) = shell_poll_idx {
            if poll_fds[idx].revents & io::poll_events::POLLIN as i16 != 0 {
                if let TileKind::Terminal { master_fd: Some(fd), ref mut emu, .. } = tiles[0].kind {
                    if let Ok(n) = io::read(fd, &mut read_buf) {
                        for i in 0..n {
                            emu.feed(read_buf[i]);
                        }
                        needs_flush = true;
                    }
                }
            }
        }

        // Read logs PTY output
        if let Some(idx) = logs_poll_idx {
            if poll_fds[idx].revents & io::poll_events::POLLIN as i16 != 0 {
                if let TileKind::Terminal { master_fd: Some(fd), ref mut emu, .. } = tiles[2].kind {
                    if let Ok(n) = io::read(fd, &mut read_buf) {
                        for i in 0..n {
                            emu.feed(read_buf[i]);
                        }
                        needs_flush = true;
                    }
                }
            }
        }

        // Mouse input
        if let Ok((mx, my, buttons)) = graphics::mouse_state() {
            let new_mx = mx as usize;
            let new_my = my as usize;
            if new_mx != mouse_x || new_my != mouse_y {
                mouse_x = new_mx;
                mouse_y = new_my;
                needs_flush = true;

                // Handle dragging
                match drag_state {
                    DragState::VSplit => {
                        let new_x = mouse_x.clamp(200, screen_w - 200);
                        if new_x != layout.vsplit_x {
                            layout.vsplit_x = new_x;
                            layout.apply_to_tiles(&mut tiles);
                            resize_terminal_tiles(&mut tiles);
                            needs_full_redraw = true;
                        }
                    }
                    DragState::HSplit => {
                        let new_y = mouse_y.clamp(100, screen_h - 100);
                        if new_y != layout.hsplit_y {
                            layout.hsplit_y = new_y;
                            layout.apply_to_tiles(&mut tiles);
                            resize_terminal_tiles(&mut tiles);
                            needs_full_redraw = true;
                        }
                    }
                    _ => {}
                }
            }

            // Button press
            let pressed = buttons & 1 != 0;
            let was_pressed = prev_mouse_buttons & 1 != 0;

            if pressed && !was_pressed {
                // Mouse down: start drag or focus
                if hit_test_vsplit(&layout, mouse_x, mouse_y) {
                    drag_state = DragState::VSplit;
                } else if hit_test_hsplit(&layout, mouse_x, mouse_y) {
                    drag_state = DragState::HSplit;
                } else if let Some(tile_idx) = hit_test_title_bar(&tiles, mouse_x, mouse_y) {
                    drag_state = DragState::TitleBar(tile_idx);
                    if tile_idx != focused_tile {
                        let old = focused_tile;
                        focused_tile = tile_idx;
                        draw_title_bar(&mut fb, &tiles[old], false);
                        draw_title_bar(&mut fb, &tiles[focused_tile], true);
                        needs_flush = true;
                    }
                } else if let Some(tile_idx) = hit_test_tile(&tiles, mouse_x, mouse_y) {
                    if tile_idx != focused_tile {
                        let old = focused_tile;
                        focused_tile = tile_idx;
                        draw_title_bar(&mut fb, &tiles[old], false);
                        draw_title_bar(&mut fb, &tiles[focused_tile], true);
                        needs_flush = true;
                    }
                }
            }

            if !pressed && was_pressed {
                // Mouse up: end drag
                if let DragState::TitleBar(src) = drag_state {
                    // Drop on another tile = swap
                    if let Some(dst) = hit_test_tile(&tiles, mouse_x, mouse_y) {
                        if dst != src {
                            // Swap tile contents (titles and kinds)
                            let src_title = tiles[src].title;
                            let dst_title = tiles[dst].title;
                            tiles[src].title = dst_title;
                            tiles[dst].title = src_title;

                            // Swap kinds using indices
                            let (left, right) = if src < dst {
                                tiles.split_at_mut(dst)
                            } else {
                                tiles.split_at_mut(src)
                            };
                            if src < dst {
                                core::mem::swap(&mut left[src].kind, &mut right[0].kind);
                            } else {
                                core::mem::swap(&mut right[0].kind, &mut left[dst].kind);
                            }

                            resize_terminal_tiles(&mut tiles);
                            needs_full_redraw = true;
                        }
                    }
                }
                if drag_state != DragState::None {
                    needs_full_redraw = true;
                }
                drag_state = DragState::None;
            }

            prev_mouse_buttons = buttons;
        }

        // Discover client windows (bounce) if not yet found
        discover_client_windows(&mut tiles);

        // Update bounce window position
        update_client_window_positions(&tiles);

        // Erase cursor before content render
        let content_dirty = needs_full_redraw
            || tiles.iter().any(|t| matches!(&t.kind, TileKind::Terminal { emu, .. } if emu.dirty))
            || frame % 200 == 0;
        if content_dirty {
            cursor_state.erase(&mut fb);
        }

        // Full redraw if layout changed
        if needs_full_redraw {
            fb.clear(DESKTOP_BG);
            draw_borders(&mut fb, &layout, drag_state);
            for (i, tile) in tiles.iter().enumerate() {
                draw_title_bar(&mut fb, tile, i == focused_tile);
                // Fill content area with BG_COLOR for terminals
                if matches!(tile.kind, TileKind::Terminal { .. }) {
                    fill_rect(&mut fb, tile.content_x(), tile.content_y(),
                              tile.content_width(), tile.content_height(), BG_COLOR);
                }
            }
            // Mark all terminal emus as dirty
            for tile in tiles.iter_mut() {
                if let TileKind::Terminal { ref mut emu, .. } = tile.kind {
                    emu.dirty = true;
                }
            }
            needs_full_redraw = false;
            needs_flush = true;
        }

        // Render terminal content
        for tile in tiles.iter_mut() {
            let cx = tile.content_x() + TERM_PADDING;
            let cy = tile.content_y() + TERM_PADDING;
            if let TileKind::Terminal { ref mut emu, .. } = tile.kind {
                if emu.dirty {
                    emu.render(&mut fb, cx, cy);
                    needs_flush = true;
                }
            }
        }

        // Periodic full redraw (for cursor blink, etc.)
        if frame % 200 == 0 {
            for (i, tile) in tiles.iter().enumerate() {
                draw_title_bar(&mut fb, tile, i == focused_tile);
            }
            needs_flush = true;
        }

        // Draw mouse cursor
        if mouse_x > 0 || mouse_y > 0 {
            cursor_state.update(&mut fb, mouse_x, mouse_y);
            needs_flush = true;
        }

        // Composite
        if needs_flush {
            let _ = graphics::virgl_composite_windows(
                &composite_buf, screen_w as u32, screen_h as u32, needs_flush,
            );
        } else {
            // Still call compositor to service frame pacing (wake blocked clients)
            let _ = graphics::virgl_composite_windows(
                &composite_buf, screen_w as u32, screen_h as u32, false,
            );
        }
        needs_flush = false;
        frame = frame.wrapping_add(1);

        // Reap dead children
        let mut status: i32 = 0;
        loop {
            match waitpid(-1, &mut status as *mut i32, WNOHANG) {
                Ok(pid) if pid.raw() > 0 => {
                    let rpid = pid.raw() as i64;
                    for tile in tiles.iter_mut() {
                        let (cols, rows) = tile.term_dims();
                        if let TileKind::Terminal { ref mut child_pid, ref mut master_fd, program, .. } = tile.kind {
                            if rpid == *child_pid {
                                print!("[bwm] Child exited, respawning...\n");
                                if let Some(old_fd) = master_fd.take() {
                                    let _ = io::close(old_fd);
                                }
                                let (m, p) = spawn_child(program, "child");
                                *master_fd = Some(m);
                                *child_pid = p;
                                let ws = libbreenix::termios::Winsize {
                                    ws_row: rows as u16, ws_col: cols as u16,
                                    ws_xpixel: 0, ws_ypixel: 0,
                                };
                                let _ = libbreenix::termios::set_winsize(m, &ws);
                            }
                        }
                    }
                }
                _ => break,
            }
        }
    }
}

/// Resize all terminal emulators to match their tile dimensions
fn resize_terminal_tiles(tiles: &mut [Tile]) {
    for tile in tiles.iter_mut() {
        let (cols, rows) = tile.term_dims();
        if let TileKind::Terminal { ref mut emu, ref master_fd, .. } = tile.kind {
            if cols > 0 && rows > 0 && (cols != emu.cols || rows != emu.rows) {
                emu.resize(cols, rows);
                if let Some(fd) = master_fd {
                    let ws = libbreenix::termios::Winsize {
                        ws_row: rows as u16, ws_col: cols as u16,
                        ws_xpixel: 0, ws_ypixel: 0,
                    };
                    let _ = libbreenix::termios::set_winsize(*fd, &ws);
                }
            }
        }
    }
}

/// Discover client windows by querying the kernel's window registry.
/// Assigns window IDs to ClientWindow tiles that have window_id == 0.
fn discover_client_windows(tiles: &mut [Tile]) {
    // Check if any tiles need discovery
    let needs_discovery = tiles.iter().any(|t| matches!(t.kind, TileKind::ClientWindow { window_id: 0 }));
    if !needs_discovery { return; }

    let mut win_infos = [graphics::WindowInfo {
        buffer_id: 0, owner_pid: 0, width: 0, height: 0,
        x: 0, y: 0, title_len: 0, title: [0; 64],
    }; 16];
    if let Ok(count) = graphics::list_windows(&mut win_infos) {
        for tile in tiles.iter_mut() {
            if let TileKind::ClientWindow { ref mut window_id } = tile.kind {
                if *window_id == 0 && count > 0 {
                    // Take the first available window
                    *window_id = win_infos[0].buffer_id;
                }
            }
        }
    }
}

/// Tell the kernel where to composite client windows (bounce)
fn update_client_window_positions(tiles: &[Tile]) {
    for tile in tiles.iter() {
        if let TileKind::ClientWindow { window_id } = tile.kind {
            if window_id != 0 {
                let _ = graphics::set_window_position(
                    window_id,
                    tile.content_x() as i32,
                    tile.content_y() as i32,
                );
            }
        }
    }
}
