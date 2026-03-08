//! Breenix Window Manager (bwm) — Floating Window Manager
//!
//! Userspace floating window manager with GPU-accelerated compositing.
//! Windows are independent, draggable rectangles on a decorative background.
//! Each window has a title bar that serves as a drag handle.
//!
//! Default windows:
//!   Shell terminal (bsh) — 750x550
//!   Bounce demo — 400x350
//!   Log viewer (bless) — 500x300

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
const TITLE_BAR_HEIGHT: usize = 24;

/// Window border/shadow width
const BORDER_WIDTH: usize = 2;

/// Noto Sans Mono 16px cell dimensions for terminal text.
/// CELL_W must match bitmap_font::metrics().char_width (7 for size_16).
const CELL_W: usize = 7;
const CELL_H: usize = 18;

/// Terminal padding inside window content area
const TERM_PADDING: usize = 4;

// Colors
const BG_COLOR: Color = Color::rgb(20, 25, 40);
const FG_COLOR: Color = Color::rgb(220, 225, 235);
const TITLE_FOCUSED_BG: Color = Color::rgb(55, 75, 120);
const TITLE_UNFOCUSED_BG: Color = Color::rgb(38, 48, 70);
const TITLE_TEXT: Color = Color::rgb(200, 205, 215);
const TITLE_FOCUSED_TEXT: Color = Color::rgb(255, 255, 255);
const WIN_BORDER_COLOR: Color = Color::rgb(60, 75, 110);
const WIN_BORDER_FOCUSED: Color = Color::rgb(90, 120, 180);
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
        Self { ch: b' ', fg: FG_COLOR, bg: BG_COLOR, bold: false }
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
            cols, rows,
            cells: vec![Cell::blank(); cols * rows],
            cursor_x: 0, cursor_y: 0,
            fg: FG_COLOR, bg: BG_COLOR, bold: false,
            ansi_state: AnsiState::Normal,
            ansi_params: [0; 8],
            ansi_param_idx: 0,
            dirty: true,
        }
    }

    fn cell(&self, x: usize, y: usize) -> &Cell { &self.cells[y * self.cols + x] }
    fn cell_mut(&mut self, x: usize, y: usize) -> &mut Cell { &mut self.cells[y * self.cols + x] }

    fn scroll_up(&mut self) {
        let cols = self.cols;
        for y in 1..self.rows {
            for x in 0..cols {
                self.cells[(y - 1) * cols + x] = self.cells[y * cols + x];
            }
        }
        let last_row = self.rows - 1;
        for x in 0..cols { self.cells[last_row * cols + x] = Cell::blank(); }
        self.dirty = true;
    }

    fn put_char(&mut self, ch: u8) {
        if self.cursor_x >= self.cols { self.cursor_x = 0; self.cursor_y += 1; }
        if self.cursor_y >= self.rows { self.scroll_up(); self.cursor_y = self.rows - 1; }
        let fg = self.fg; let bg = self.bg; let bold = self.bold;
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
                    self.cursor_x = 0; self.cursor_y += 1;
                    if self.cursor_y >= self.rows { self.scroll_up(); self.cursor_y = self.rows - 1; }
                    self.dirty = true;
                }
                b'\r' => { self.cursor_x = 0; self.dirty = true; }
                b'\t' => {
                    let next_tab = (self.cursor_x + 8) & !7;
                    while self.cursor_x < next_tab && self.cursor_x < self.cols { self.put_char(b' '); }
                }
                0x08 => { if self.cursor_x > 0 { self.cursor_x -= 1; self.dirty = true; } }
                0x07 => {}
                ch if ch >= 0x20 => self.put_char(ch),
                _ => {}
            },
            AnsiState::Escape => match byte {
                b'[' => { self.ansi_state = AnsiState::Csi; self.ansi_params = [0; 8]; self.ansi_param_idx = 0; }
                b']' => { self.ansi_state = AnsiState::OscString; }
                _ => { self.ansi_state = AnsiState::Normal; }
            },
            AnsiState::Csi | AnsiState::CsiParam => {
                if byte >= b'0' && byte <= b'9' {
                    self.ansi_state = AnsiState::CsiParam;
                    let idx = self.ansi_param_idx;
                    if idx < 8 {
                        self.ansi_params[idx] = self.ansi_params[idx].saturating_mul(10).saturating_add((byte - b'0') as u16);
                    }
                } else if byte == b';' {
                    if self.ansi_param_idx < 7 { self.ansi_param_idx += 1; }
                } else {
                    let nparams = self.ansi_param_idx + 1;
                    self.execute_csi(byte, nparams);
                    self.ansi_state = AnsiState::Normal;
                }
            }
            AnsiState::OscString => {
                if byte == 0x07 || byte == b'\\' { self.ansi_state = AnsiState::Normal; }
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
            b'A' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_y = self.cursor_y.saturating_sub(n); self.dirty = true; }
            b'B' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_y = (self.cursor_y + n).min(self.rows - 1); self.dirty = true; }
            b'C' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_x = (self.cursor_x + n).min(self.cols - 1); self.dirty = true; }
            b'D' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_x = self.cursor_x.saturating_sub(n); self.dirty = true; }
            b'G' => { let col = if p0 > 0 { p0 - 1 } else { 0 }; self.cursor_x = col.min(self.cols - 1); self.dirty = true; }
            b'J' => {
                match p0 {
                    0 => {
                        for x in self.cursor_x..self.cols { *self.cell_mut(x, self.cursor_y) = Cell::blank(); }
                        for y in (self.cursor_y + 1)..self.rows { for x in 0..self.cols { *self.cell_mut(x, y) = Cell::blank(); } }
                    }
                    1 => {
                        for y in 0..self.cursor_y { for x in 0..self.cols { *self.cell_mut(x, y) = Cell::blank(); } }
                        for x in 0..=self.cursor_x.min(self.cols - 1) { *self.cell_mut(x, self.cursor_y) = Cell::blank(); }
                    }
                    2 | 3 => {
                        for y in 0..self.rows { for x in 0..self.cols { *self.cell_mut(x, y) = Cell::blank(); } }
                        self.cursor_x = 0; self.cursor_y = 0;
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            b'K' => {
                match p0 {
                    0 => { for x in self.cursor_x..self.cols { *self.cell_mut(x, self.cursor_y) = Cell::blank(); } }
                    1 => { for x in 0..=self.cursor_x.min(self.cols - 1) { *self.cell_mut(x, self.cursor_y) = Cell::blank(); } }
                    2 => { for x in 0..self.cols { *self.cell_mut(x, self.cursor_y) = Cell::blank(); } }
                    _ => {}
                }
                self.dirty = true;
            }
            b'd' => { let row = if p0 > 0 { p0 - 1 } else { 0 }; self.cursor_y = row.min(self.rows - 1); self.dirty = true; }
            b'r' => { self.cursor_x = 0; self.cursor_y = 0; self.dirty = true; }
            b'm' => {
                if nparams == 1 && p0 == 0 { self.fg = FG_COLOR; self.bg = BG_COLOR; self.bold = false; }
                else {
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
                                c.r = c.r.saturating_add(60); c.g = c.g.saturating_add(60); c.b = c.b.saturating_add(60);
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

    fn render(&mut self, fb: &mut FrameBuf, x_off: usize, y_off: usize,
              clip_w: usize, clip_h: usize) {
        if !self.dirty { return; }
        self.dirty = false;
        let max_x = (x_off + clip_w).min(fb.width);
        let max_y = (y_off + clip_h).min(fb.height);
        let fm = bitmap_font::metrics();
        for row in 0..self.rows {
            let py = y_off + row * CELL_H;
            if py + CELL_H > max_y { break; }
            for col in 0..self.cols {
                let cell = self.cell(col, row);
                let px = x_off + col * CELL_W;
                if px + CELL_W > max_x { break; }
                for dy in 0..CELL_H { for dx in 0..CELL_W { fb.put_pixel(px + dx, py + dy, cell.bg); } }
                if cell.ch != b' ' {
                    let fg = if cell.bold {
                        Color::rgb(cell.fg.r.saturating_add(40), cell.fg.g.saturating_add(40), cell.fg.b.saturating_add(40))
                    } else { cell.fg };
                    bitmap_font::draw_char(fb, cell.ch as char, px, py, fg);
                }
            }
            // Cursor underline
            if row == self.cursor_y && self.cursor_x < self.cols {
                let cx = x_off + self.cursor_x * CELL_W;
                let cw = fm.char_width.min(CELL_W);
                for dy in 0..2usize { for dx in 0..cw {
                    if cx + dx < max_x && py + CELL_H - 2 + dy < max_y {
                        fb.put_pixel(cx + dx, py + CELL_H - 2 + dy, Color::WHITE);
                    }
                }}
            }
        }
    }

}

// ─── Input Parser ────────────────────────────────────────────────────────────

struct InputParser {
    state: InputState,
    esc_buf: [u8; 8],
    esc_len: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum InputState { Normal, Escape, CsiOrSS3 }

enum InputEvent {
    Char(u8),
    FunctionKey(u8),
    ArrowUp, ArrowDown, ArrowRight, ArrowLeft,
}

impl InputParser {
    fn new() -> Self { Self { state: InputState::Normal, esc_buf: [0; 8], esc_len: 0 } }

    fn feed(&mut self, byte: u8) -> Option<InputEvent> {
        match self.state {
            InputState::Normal => {
                if byte == 0x1b { self.state = InputState::Escape; self.esc_len = 0; None }
                else { Some(InputEvent::Char(byte)) }
            }
            InputState::Escape => {
                if byte == b'[' || byte == b'O' {
                    self.state = InputState::CsiOrSS3; self.esc_buf[0] = byte; self.esc_len = 1; None
                } else { self.state = InputState::Normal; Some(InputEvent::Char(byte)) }
            }
            InputState::CsiOrSS3 => {
                if self.esc_len < 7 { self.esc_buf[self.esc_len] = byte; self.esc_len += 1; }
                if byte >= 0x40 && byte <= 0x7e { self.state = InputState::Normal; self.decode_escape() }
                else { None }
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
                    if self.esc_len == 3 { if num == 1 { return Some(InputEvent::FunctionKey(1)); } }
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

// ─── Floating Window ─────────────────────────────────────────────────────────

struct Window {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    title: [u8; 32],
    title_len: usize,
    kind: WindowKind,
}

enum WindowKind {
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

/// Extract program name from a null-terminated path like b"/bin/bsh\0" → "bsh"
fn title_from_program(program: &[u8]) -> ([u8; 32], usize) {
    let mut buf = [0u8; 32];
    // Find the last '/' before the null terminator
    let path_end = program.iter().position(|&b| b == 0).unwrap_or(program.len());
    let name_start = program[..path_end].iter().rposition(|&b| b == b'/').map(|p| p + 1).unwrap_or(0);
    let name = &program[name_start..path_end];
    let len = name.len().min(32);
    buf[..len].copy_from_slice(&name[..len]);
    (buf, len)
}

impl Window {
    fn title_bytes(&self) -> &[u8] { &self.title[..self.title_len] }

    fn content_x(&self) -> i32 { self.x + BORDER_WIDTH as i32 }
    fn content_y(&self) -> i32 { self.y + TITLE_BAR_HEIGHT as i32 + BORDER_WIDTH as i32 }
    fn content_width(&self) -> usize { self.width.saturating_sub(BORDER_WIDTH * 2) }
    fn content_height(&self) -> usize { self.height.saturating_sub(TITLE_BAR_HEIGHT + BORDER_WIDTH * 2) }

    fn term_dims(&self) -> (usize, usize) {
        let pw = self.content_width().saturating_sub(TERM_PADDING * 2);
        let ph = self.content_height().saturating_sub(TERM_PADDING * 2);
        (pw / CELL_W, ph / CELL_H)
    }

    fn total_height(&self) -> usize { self.height }

    /// Test if point is in the title bar
    fn hit_title(&self, mx: i32, my: i32) -> bool {
        mx >= self.x && mx < self.x + self.width as i32
            && my >= self.y && my < self.y + TITLE_BAR_HEIGHT as i32
    }

    /// Test if point is anywhere in the window (title + content)
    fn hit_any(&self, mx: i32, my: i32) -> bool {
        mx >= self.x && mx < self.x + self.width as i32
            && my >= self.y && my < self.y + self.total_height() as i32
    }
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

    // Drop shadow (offset 3px right+down, dark)
    let shadow = Color::rgb(8, 10, 18);
    fill_rect(fb, win.x + 3, win.y + 3, win.width, win.total_height(), shadow);

    // Border
    fill_rect(fb, win.x, win.y, win.width, win.total_height(), border_color);

    // Title bar
    fill_rect(fb, win.x + bw as i32, win.y + bw as i32,
              win.width - bw * 2, TITLE_BAR_HEIGHT - bw, title_bg);
    draw_text_at(fb, win.title_bytes(), win.x + 8, win.y + 4 + bw as i32, title_fg);

    // Content background
    fill_rect(fb, win.content_x(), win.content_y(),
              win.content_width(), win.content_height(), BG_COLOR);
}

/// Paint the decorative desktop background — gradient with grid
fn paint_background(fb: &mut FrameBuf) {
    let w = fb.width;
    let h = fb.height;
    for y in 0..h {
        for x in 0..w {
            // Vertical gradient: dark navy top → dark purple bottom
            let t = (y * 255 / h) as u8;
            let r = 12u8.saturating_add(t / 12);  // 12..33
            let g = 16u8.saturating_add(t / 20);  // 16..28
            let b = 38u8.saturating_add(t / 6);   // 38..80
            // Subtle grid every 64px
            let grid = (x % 64 == 0 || y % 64 == 0) as u8;
            let r2 = r.saturating_add(grid * 6);
            let g2 = g.saturating_add(grid * 8);
            let b2 = b.saturating_add(grid * 12);
            fb.put_pixel(x, y, Color::rgb(r2, g2, b2));
        }
    }
}

// ─── Process Spawning ────────────────────────────────────────────────────────

fn spawn_child(path: &[u8], _name: &str) -> (Fd, i64) {
    let (master_fd, slave_name) = match pty::openpty() {
        Ok((m, s)) => (m, s),
        Err(_) => return (Fd::from_raw(0), -1),
    };

    let len = slave_name.iter().position(|&b| b == 0).unwrap_or(slave_name.len());
    let slave_path_str = core::str::from_utf8(&slave_name[..len]).unwrap_or("/dev/pts/0");
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
            if slave_fd.raw() > 2 { let _ = io::close(slave_fd); }
            let _ = exec(path);
            libbreenix::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => (master_fd, child_pid.raw() as i64),
        Err(_) => { let _ = io::close(master_fd); (Fd::from_raw(0), -1) }
    }
}

// ─── Main ────────────────────────────────────────────────────────────────────

fn main() {
    print!("[bwm] Breenix Floating Window Manager starting...\n");

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
    let mut composite_buf = vec![0u32; screen_w * screen_h];

    let mut fb = unsafe {
        FrameBuf::from_raw(
            composite_buf.as_mut_ptr() as *mut u8,
            screen_w, screen_h, screen_w * bpp, bpp, info.is_bgr(),
        )
    };

    // Paint decorative background
    paint_background(&mut fb);

    // Spawn children
    let (shell_master, shell_pid) = spawn_child(b"/bin/bsh\0", "bsh");
    let (bless_master, bless_pid) = spawn_child(b"/bin/bless\0", "bless");

    // Create floating windows proportional to screen size
    let shell_w: usize = (screen_w * 55 / 100).max(400);  // ~55% of screen width
    let shell_h: usize = (screen_h * 60 / 100).max(300);  // ~60% of screen height
    let bounce_w: usize = 420;                              // fixed size (client window)
    let bounce_h: usize = 380;
    let right_w: usize = (screen_w * 35 / 100).max(300);   // ~35% for right-side windows
    let logs_w: usize = right_w;
    let logs_h: usize = (screen_h * 30 / 100).max(200);

    let shell_cols = (shell_w - BORDER_WIDTH * 2 - TERM_PADDING * 2) / CELL_W;
    let shell_rows = (shell_h - TERM_PADDING * 2) / CELL_H;
    let logs_cols = (logs_w - BORDER_WIDTH * 2 - TERM_PADDING * 2) / CELL_W;
    let logs_rows = (logs_h - TERM_PADDING * 2) / CELL_H;

    let (bsh_title, bsh_title_len) = title_from_program(b"/bin/bsh\0");
    let (bounce_title, bounce_title_len) = title_from_program(b"/bin/bounce\0");
    let (bless_title, bless_title_len) = title_from_program(b"/bin/bless\0");

    let mut windows: Vec<Window> = vec![
        Window {
            x: 30, y: 30, width: shell_w, height: shell_h + TITLE_BAR_HEIGHT,
            title: bsh_title, title_len: bsh_title_len,
            kind: WindowKind::Terminal {
                emu: TermEmu::new(shell_cols, shell_rows),
                master_fd: Some(shell_master),
                child_pid: shell_pid,
                program: b"/bin/bsh\0",
            },
        },
        Window {
            x: (screen_w as i32 - bounce_w as i32 - 40),
            y: 30,
            width: bounce_w,
            height: bounce_h + TITLE_BAR_HEIGHT,
            title: bounce_title, title_len: bounce_title_len,
            kind: WindowKind::ClientWindow { window_id: 0 },
        },
        Window {
            x: (screen_w as i32 - logs_w as i32 - 40),
            y: (bounce_h as i32 + TITLE_BAR_HEIGHT as i32 + 90),
            width: logs_w,
            height: logs_h + TITLE_BAR_HEIGHT,
            title: bless_title, title_len: bless_title_len,
            kind: WindowKind::Terminal {
                emu: TermEmu::new(logs_cols, logs_rows),
                master_fd: Some(bless_master),
                child_pid: bless_pid,
                program: b"/bin/bless\0",
            },
        },
    ];

    // Set PTY window sizes
    let ws_shell = libbreenix::termios::Winsize {
        ws_row: shell_rows as u16, ws_col: shell_cols as u16, ws_xpixel: 0, ws_ypixel: 0,
    };
    let _ = libbreenix::termios::set_winsize(shell_master, &ws_shell);
    let ws_logs = libbreenix::termios::Winsize {
        ws_row: logs_rows as u16, ws_col: logs_cols as u16, ws_xpixel: 0, ws_ypixel: 0,
    };
    let _ = libbreenix::termios::set_winsize(bless_master, &ws_logs);

    print!("[bwm] Shell: {}x{} cells, Logs: {}x{} cells\n", shell_cols, shell_rows, logs_cols, logs_rows);

    // Enter raw mode on stdin
    let mut orig_termios = libbreenix::termios::Termios::default();
    let _ = libbreenix::termios::tcgetattr(Fd::from_raw(0), &mut orig_termios);
    let mut raw = orig_termios;
    libbreenix::termios::cfmakeraw(&mut raw);
    let _ = libbreenix::termios::tcsetattr(Fd::from_raw(0), libbreenix::termios::TCSANOW, &raw);

    let mut focused_win: usize = 0;
    let mut input_parser = InputParser::new();
    let mut frame: u32 = 0;
    let mut prev_mouse_buttons: u32 = 0;
    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut dragging: Option<(usize, i32, i32)> = None; // (win_idx, offset_x, offset_y)
    let mut content_dirty = true; // only true when actual content changes

    // Initial draw — frame + content per window in Z-order
    redraw_all_windows(&mut fb, &mut windows, focused_win);
    // Upload initial frame
    let _ = graphics::virgl_composite_windows(&composite_buf, screen_w as u32, screen_h as u32, true);

    // Main loop
    let mut read_buf = [0u8; 512];
    let mut poll_fds = [
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }, // stdin
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }, // shell
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }, // logs
    ];

    loop {
        // Setup poll FDs
        let mut nfds = 1;
        poll_fds[0].revents = 0;
        let mut shell_poll_idx: Option<usize> = None;
        let mut logs_poll_idx: Option<usize> = None;

        if let WindowKind::Terminal { master_fd: Some(fd), .. } = &windows[0].kind {
            poll_fds[nfds].fd = fd.raw() as i32; poll_fds[nfds].revents = 0;
            shell_poll_idx = Some(nfds); nfds += 1;
        }
        if let WindowKind::Terminal { master_fd: Some(fd), .. } = &windows[2].kind {
            poll_fds[nfds].fd = fd.raw() as i32; poll_fds[nfds].revents = 0;
            logs_poll_idx = Some(nfds); nfds += 1;
        }

        let _nready = io::poll(&mut poll_fds[..nfds], 1).unwrap_or(0);

        // Read stdin (keyboard) → route to focused terminal
        if poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0 {
            if let Ok(n) = io::read(Fd::from_raw(0), &mut read_buf) {
                for i in 0..n {
                    if let Some(event) = input_parser.feed(read_buf[i]) {
                        match event {
                            InputEvent::FunctionKey(k) if k >= 1 && k <= 3 => {
                                let new_focus = (k - 1) as usize;
                                if new_focus < windows.len() && new_focus != focused_win {
                                    let old = focused_win;
                                    focused_win = new_focus;
                                    draw_window_frame(&mut fb, &windows[old], false);
                                    render_window_content(&mut windows[old], &mut fb);
                                    draw_window_frame(&mut fb, &windows[focused_win], true);
                                    render_window_content(&mut windows[focused_win], &mut fb);
                                    content_dirty = true;
                                }
                            }
                            InputEvent::Char(c) => {
                                if let WindowKind::Terminal { master_fd: Some(fd), .. } = &windows[focused_win].kind {
                                    let _ = io::write(*fd, &[c]);
                                }
                            }
                            InputEvent::ArrowUp => { write_escape_to_focused(&windows, focused_win, b"\x1b[A"); }
                            InputEvent::ArrowDown => { write_escape_to_focused(&windows, focused_win, b"\x1b[B"); }
                            InputEvent::ArrowRight => { write_escape_to_focused(&windows, focused_win, b"\x1b[C"); }
                            InputEvent::ArrowLeft => { write_escape_to_focused(&windows, focused_win, b"\x1b[D"); }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Read shell PTY output
        if let Some(idx) = shell_poll_idx {
            if poll_fds[idx].revents & io::poll_events::POLLIN as i16 != 0 {
                if let WindowKind::Terminal { master_fd: Some(fd), ref mut emu, .. } = windows[0].kind {
                    if let Ok(n) = io::read(fd, &mut read_buf) {
                        for i in 0..n { emu.feed(read_buf[i]); }
                        content_dirty = true;
                    }
                }
            }
        }

        // Read logs PTY output
        if let Some(idx) = logs_poll_idx {
            if poll_fds[idx].revents & io::poll_events::POLLIN as i16 != 0 {
                if let WindowKind::Terminal { master_fd: Some(fd), ref mut emu, .. } = windows[2].kind {
                    if let Ok(n) = io::read(fd, &mut read_buf) {
                        for i in 0..n { emu.feed(read_buf[i]); }
                        content_dirty = true;
                    }
                }
            }
        }

        // Mouse input
        if let Ok((mx, my, buttons)) = graphics::mouse_state() {
            let new_mx = mx as i32;
            let new_my = my as i32;
            if new_mx != mouse_x || new_my != mouse_y {
                mouse_x = new_mx;
                mouse_y = new_my;

                // Handle window dragging
                if let Some((win_idx, off_x, off_y)) = dragging {
                    let new_x = mouse_x - off_x;
                    let new_y = mouse_y - off_y;
                    if new_x != windows[win_idx].x || new_y != windows[win_idx].y {
                        // Repaint background where window was
                        repaint_window_area(&mut fb, &windows[win_idx], screen_w, screen_h);
                        windows[win_idx].x = new_x;
                        windows[win_idx].y = new_y;
                        // Redraw all windows (proper z-order)
                        redraw_all_windows(&mut fb, &mut windows, focused_win);
                        content_dirty = true;
                    }
                }
            }

            let pressed = buttons & 1 != 0;
            let was_pressed = prev_mouse_buttons & 1 != 0;

            if pressed && !was_pressed {
                // Mouse down — check windows (top-to-bottom z-order: last is on top)
                for i in (0..windows.len()).rev() {
                    let ht = windows[i].hit_title(mouse_x, mouse_y);
                    let ha = windows[i].hit_any(mouse_x, mouse_y);
                    if ht {
                        dragging = Some((i, mouse_x - windows[i].x, mouse_y - windows[i].y));
                    }
                    if ht || ha {
                        if i != focused_win {
                            let old = focused_win;
                            focused_win = i;
                            draw_window_frame(&mut fb, &windows[old], false);
                            render_window_content(&mut windows[old], &mut fb);
                            draw_window_frame(&mut fb, &windows[focused_win], true);
                            render_window_content(&mut windows[focused_win], &mut fb);
                            content_dirty = true;
                        }
                        break;
                    }
                }
            }

            if !pressed && was_pressed {
                dragging = None;
            }

            prev_mouse_buttons = buttons;
        }

        // Discover client windows (bounce)
        discover_client_windows(&mut windows);
        update_client_window_positions(&windows);

        // Render terminal content (only if dirty), clipped to window bounds
        for win in windows.iter_mut() {
            let cx = win.content_x() + TERM_PADDING as i32;
            let cy = win.content_y() + TERM_PADDING as i32;
            let cw = win.content_width().saturating_sub(TERM_PADDING * 2);
            let ch = win.content_height().saturating_sub(TERM_PADDING * 2);
            if let WindowKind::Terminal { ref mut emu, .. } = win.kind {
                if emu.dirty && cx >= 0 && cy >= 0 {
                    emu.render(&mut fb, cx as usize, cy as usize, cw, ch);
                    content_dirty = true;
                }
            }
        }

        // Composite to GPU — cursor is rendered by the kernel compositor,
        // so cursor movement alone does NOT require a full 4.9MB upload.
        if content_dirty {
            let _ = graphics::virgl_composite_windows(
                &composite_buf, screen_w as u32, screen_h as u32, true,
            );
            content_dirty = false;
        } else {
            // Nothing changed in window content — still call compositor to
            // wake blocked clients (frame pacing) and let kernel update cursor.
            let _ = graphics::virgl_composite_windows(
                &composite_buf, screen_w as u32, screen_h as u32, false,
            );
        }

        frame = frame.wrapping_add(1);

        // Reap dead children
        let mut status: i32 = 0;
        loop {
            match waitpid(-1, &mut status as *mut i32, WNOHANG) {
                Ok(pid) if pid.raw() > 0 => {
                    let rpid = pid.raw() as i64;
                    for win in windows.iter_mut() {
                        let (cols, rows) = win.term_dims();
                        if let WindowKind::Terminal { ref mut child_pid, ref mut master_fd, program, .. } = win.kind {
                            if rpid == *child_pid {
                                print!("[bwm] Child exited, respawning after cooldown...\n");
                                if let Some(old_fd) = master_fd.take() { let _ = io::close(old_fd); }
                                // Cooldown to prevent tight respawn loop if child fails immediately
                                let _ = libbreenix::time::nanosleep(&libbreenix::types::Timespec {
                                    tv_sec: 1, tv_nsec: 0,
                                });
                                let (m, p) = spawn_child(program, "child");
                                *master_fd = Some(m);
                                *child_pid = p;
                                let ws = libbreenix::termios::Winsize {
                                    ws_row: rows as u16, ws_col: cols as u16, ws_xpixel: 0, ws_ypixel: 0,
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

fn write_escape_to_focused(windows: &[Window], focused_win: usize, seq: &[u8]) {
    if let WindowKind::Terminal { master_fd: Some(fd), .. } = &windows[focused_win].kind {
        let _ = io::write(*fd, seq);
    }
}

fn render_window_content(win: &mut Window, fb: &mut FrameBuf) {
    let cx = win.content_x() + TERM_PADDING as i32;
    let cy = win.content_y() + TERM_PADDING as i32;
    let cw = win.content_width().saturating_sub(TERM_PADDING * 2);
    let ch = win.content_height().saturating_sub(TERM_PADDING * 2);
    if let WindowKind::Terminal { ref mut emu, .. } = win.kind {
        emu.dirty = true;
        if cx >= 0 && cy >= 0 { emu.render(fb, cx as usize, cy as usize, cw, ch); }
    }
}

/// Repaint the background area where a window was (before moving it)
fn repaint_window_area(fb: &mut FrameBuf, win: &Window, screen_w: usize, screen_h: usize) {
    let x0 = (win.x - 1).max(0) as usize;
    let y0 = (win.y - 1).max(0) as usize;
    let x1 = ((win.x + win.width as i32 + 4) as usize).min(screen_w);
    let y1 = ((win.y + win.total_height() as i32 + 4) as usize).min(screen_h);
    for y in y0..y1 {
        for x in x0..x1 {
            let t = (y * 255 / screen_h) as u8;
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

/// Redraw all windows in z-order (index 0 = bottom).
/// Draws frame + content for each window so later windows correctly overlap earlier ones.
fn redraw_all_windows(fb: &mut FrameBuf, windows: &mut [Window], focused_win: usize) {
    for i in 0..windows.len() {
        draw_window_frame(fb, &windows[i], i == focused_win);
        render_window_content(&mut windows[i], fb);
    }
}

/// Discover client windows by querying the kernel's window registry.
fn discover_client_windows(windows: &mut [Window]) {
    let needs_discovery = windows.iter().any(|w| matches!(w.kind, WindowKind::ClientWindow { window_id: 0 }));
    if !needs_discovery { return; }

    let mut win_infos = [graphics::WindowInfo {
        buffer_id: 0, owner_pid: 0, width: 0, height: 0,
        x: 0, y: 0, title_len: 0, title: [0; 64],
    }; 16];
    if let Ok(count) = graphics::list_windows(&mut win_infos) {
        for win in windows.iter_mut() {
            if let WindowKind::ClientWindow { ref mut window_id } = win.kind {
                if *window_id == 0 && count > 0 {
                    *window_id = win_infos[0].buffer_id;
                }
            }
        }
    }
}

/// Tell the kernel where to composite client windows
fn update_client_window_positions(windows: &[Window]) {
    for win in windows.iter() {
        if let WindowKind::ClientWindow { window_id } = win.kind {
            if window_id != 0 {
                let _ = graphics::set_window_position(
                    window_id,
                    win.content_x(),
                    win.content_y(),
                );
            }
        }
    }
}
