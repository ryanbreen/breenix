//! bterm — Standalone terminal emulator using Breengel windowing.
//!
//! Each tab spawns its own PTY + shell (bsh). Keyboard input flows to the
//! active tab's PTY master; PTY output is fed through a VT100/ANSI parser
//! and drawn into the Breengel window's GPU surface.
//!
//! Keyboard shortcuts:
//!   Ctrl+T       — open a new tab
//!   Ctrl+W       — close the active tab (exits if last tab)
//!   Ctrl+Plus/=  — increase font size
//!   Ctrl+Minus   — decrease font size

use std::process;

use breengel::{Window, Event, CachedFont, TabBar, Rect, Theme, Color, Surface};
use libbreenix::io;
use libbreenix::fs;
use libbreenix::process::{fork, exec, setsid, ForkResult};
use libbreenix::pty;
use libbreenix::termios;
use libbreenix::types::Fd;
use libbreenix::time;

use libgfx::bitmap_font;
use libgfx::shapes;
use libgfx::ttf_font;

use libbui::{InputState, WidgetEvent};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Min/max font size for Ctrl+/- resizing.
const MIN_FONT_SIZE: f32 = 8.0;
const MAX_FONT_SIZE: f32 = 32.0;
const FONT_SIZE_STEP: f32 = 1.0;

/// Bitmap font cell dimensions (fallback when TTF is unavailable).
const BITMAP_CELL_W: usize = 7;
const BITMAP_CELL_H: usize = 18;

/// Tab bar height in logical UI units.
const TAB_BAR_HEIGHT: i32 = 24;

// Terminal colors
const BG_COLOR: Color = Color::rgb(30, 30, 40);
const FG_COLOR: Color = Color::rgb(204, 204, 204);

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

// ─── Window dimensions ──────────────────────────────────────────────────────

const WIN_WIDTH: u32 = 750;
const WIN_HEIGHT: u32 = 550;

// ─── Character Cell ─────────────────────────────────────────────────────────

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

// ─── ANSI Parser State Machine ──────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum AnsiState {
    Normal,
    Escape,
    Csi,
    CsiParam,
    OscString,
}

// ─── Terminal Emulator ──────────────────────────────────────────────────────

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

    fn resize(&mut self, new_cols: usize, new_rows: usize) {
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }
        let mut new_cells = vec![Cell::blank(); new_cols * new_rows];
        let copy_cols = self.cols.min(new_cols);
        let copy_rows = self.rows.min(new_rows);
        for y in 0..copy_rows {
            for x in 0..copy_cols {
                new_cells[y * new_cols + x] = self.cells[y * self.cols + x];
            }
        }
        self.cols = new_cols;
        self.rows = new_rows;
        self.cells = new_cells;
        self.cursor_x = self.cursor_x.min(new_cols.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(new_rows.saturating_sub(1));
        self.dirty = true;
    }

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

    /// Draw the grid: each row's cell backgrounds as runs of one colour, then the characters
    /// at their cells' origins in as few draws as their colours allow, then the cursor's
    /// underline. A cell is drawn only when it lies inside the clip area, edge to edge.
    fn render(&mut self, fb: &mut Surface, x_off: usize, y_off: usize,
              clip_w: usize, clip_h: usize, cell_w: usize, cell_h: usize,
              font_size: f32, ttf: Option<&mut CachedFont>) {
        if !self.dirty { return; }
        self.dirty = false;
        let max_x = (x_off + clip_w).min(fb.width());
        let max_y = (y_off + clip_h).min(fb.height());
        let rows = self.rows.min(max_y.saturating_sub(y_off) / cell_h.max(1));
        let cols = self.cols.min(max_x.saturating_sub(x_off) / cell_w.max(1));
        let fg_of = |cell: &Cell| if cell.bold {
            Color::rgb(cell.fg.r.saturating_add(40), cell.fg.g.saturating_add(40), cell.fg.b.saturating_add(40))
        } else { cell.fg };
        fb.batch(|fb| {
            for row in 0..rows {
                let py = y_off + row * cell_h;
                let mut col = 0;
                while col < cols {
                    let bg = self.cell(col, row).bg;
                    let start = col;
                    while col < cols && self.cell(col, row).bg == bg {
                        col += 1;
                    }
                    shapes::fill_rect(fb, (x_off + start * cell_w) as i32, py as i32,
                        ((col - start) * cell_w) as i32, cell_h as i32, bg);
                }
            }
        });
        let glyphs: Vec<(usize, usize, Color)> = (0..rows)
            .flat_map(|row| (0..cols).map(move |col| (col, row)))
            .filter(|&(col, row)| self.cell(col, row).ch != b' ')
            .map(|(col, row)| (col, row, fg_of(self.cell(col, row))))
            .collect();
        if let Some(font) = ttf {
            let clip = libgfx::surface::Rect::new(0, 0, max_x as i32, max_y as i32);
            let runs: Vec<ttf_font::TextRun<'_>> = glyphs.iter().map(|&(col, row, fg)| ttf_font::TextRun {
                bytes: core::slice::from_ref(&self.cells[row * self.cols + col].ch),
                x: (x_off + col * cell_w) as i32,
                y: (y_off + row * cell_h) as i32,
                size: font_size,
                fg,
                clip,
            }).collect();
            ttf_font::draw_runs(fb, font, &runs);
        } else {
            // The bitmap font's advance is the cell width: a row's characters of one colour
            // in a run are one draw.
            let mut i = 0;
            while i < glyphs.len() {
                let (col, row, fg) = glyphs[i];
                let mut text = vec![self.cell(col, row).ch];
                let mut j = i + 1;
                while j < glyphs.len() && glyphs[j].1 == row && glyphs[j].2 == fg
                    && glyphs[j].0 == col + text.len() && cell_w == bitmap_font::metrics().char_width {
                    text.push(self.cell(glyphs[j].0, row).ch);
                    j += 1;
                }
                bitmap_font::draw_text(fb, &text, x_off + col * cell_w, y_off + row * cell_h, fg);
                i = j;
            }
        }
        // Cursor underline
        if self.cursor_y < rows && self.cursor_x < self.cols {
            let cx = x_off + self.cursor_x * cell_w;
            let cy = y_off + self.cursor_y * cell_h + cell_h - 2;
            let w = cell_w.min(max_x.saturating_sub(cx));
            let h = 2usize.min(max_y.saturating_sub(cy));
            if w > 0 && h > 0 {
                shapes::fill_rect(fb, cx as i32, cy as i32, w as i32, h as i32, Color::WHITE);
            }
        }
    }
}

// ─── Tab ────────────────────────────────────────────────────────────────────

struct Tab {
    emu: TermEmu,
    master_fd: Fd,
    #[allow(dead_code)] // used for future kill/waitpid on tab close
    child_pid: i64,
}

// ─── Process Spawning ───────────────────────────────────────────────────────

fn spawn_child(path: &[u8]) -> (Fd, i64) {
    let (master_fd, slave_name) = match pty::openpty() {
        Ok((m, s)) => (m, s),
        Err(_) => {
            print!("[bterm] openpty failed\n");
            return (Fd::from_raw(u64::MAX), -1);
        }
    };

    // Build slave path on the STACK (not heap) to avoid CoW corruption after fork.
    // slave_name is [u8; 32] containing e.g. "/dev/pts/0\0..."
    let mut slave_path: [u8; 33] = [0; 33];
    let len = slave_name.iter().position(|&b| b == 0).unwrap_or(slave_name.len()).min(32);
    slave_path[..len].copy_from_slice(&slave_name[..len]);
    slave_path[len] = 0; // null terminator
    let slave_path_str = core::str::from_utf8(&slave_path[..=len]).unwrap_or("/dev/pts/0\0");

    match fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(master_fd);
            let _ = setsid();
            let slave_fd = match fs::open(slave_path_str, fs::O_RDWR) {
                Ok(fd) => fd,
                Err(_) => { libbreenix::process::exit(126); }
            };
            let _ = libbreenix::termios::set_controlling_terminal(slave_fd);
            let _ = io::dup2(slave_fd, Fd::from_raw(0));
            let _ = io::dup2(slave_fd, Fd::from_raw(1));
            let _ = io::dup2(slave_fd, Fd::from_raw(2));
            if slave_fd.raw() > 2 { let _ = io::close(slave_fd); }
            let _ = exec(path);
            libbreenix::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            print!("[bterm] spawned child pid={}\n", child_pid.raw());
            (master_fd, child_pid.raw() as i64)
        }
        Err(_) => {
            print!("[bterm] fork failed\n");
            let _ = io::close(master_fd);
            (Fd::from_raw(u64::MAX), -1)
        }
    }
}

// ─── Tab helpers ────────────────────────────────────────────────────────────

/// Generate a label for a new tab.
/// We use a static counter to keep labels unique.
static mut TAB_COUNTER: usize = 0;

/// Tab label storage — we need 'static lifetimes for TabBar labels.
fn make_tab_label() -> &'static [u8] {
    let n = unsafe {
        TAB_COUNTER += 1;
        TAB_COUNTER
    };
    let s = format!("shell {}", n);
    let boxed: Box<[u8]> = s.into_bytes().into_boxed_slice();
    Box::leak(boxed)
}

fn spawn_tab(cols: usize, rows: usize) -> Tab {
    spawn_tab_cmd(cols, rows, b"/bin/bsh\0")
}

fn spawn_tab_cmd(cols: usize, rows: usize, cmd: &[u8]) -> Tab {
    let (master_fd, child_pid) = spawn_child(cmd);
    let _ = io::fcntl_setfl(master_fd, io::status_flags::O_NONBLOCK);
    Tab {
        emu: TermEmu::new(cols, rows),
        master_fd,
        child_pid,
    }
}

fn make_static_label(s: &str) -> &'static [u8] {
    let boxed: Box<[u8]> = s.as_bytes().to_vec().into_boxed_slice();
    Box::leak(boxed)
}

/// Compute cell dimensions from TTF font metrics.
fn ttf_cell_dims(font: &mut CachedFont, size: f32) -> (usize, usize) {
    let metrics = font.metrics(size);
    let glyph_m = font.glyph_index('M');
    let advance = font.advance_width(glyph_m, size);
    // Guard against NaN/Inf: if advance or ascender is unreasonable, fall back to bitmap dims
    if advance <= 0.0 || advance > 200.0 || metrics.ascender <= 0.0 || metrics.ascender > 200.0 {
        return (BITMAP_CELL_W, BITMAP_CELL_H);
    }
    // Use i32 intermediate to avoid potential f32-to-usize codegen issues
    let w = (advance + 0.5) as i32;
    let h = (metrics.ascender + 0.99) as i32 + ((-metrics.descender) + 0.99) as i32;
    (w.max(1) as usize, h.max(1) as usize)
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    print!("[bterm] starting\n");
    // Create window (loads system font automatically from /etc/fonts.conf)
    let mut win = match Window::new(b"Terminal", WIN_WIDTH, WIN_HEIGHT) {
        Ok(w) => w,
        Err(e) => {
            print!("[bterm] ERROR: failed to create window: {}\n", e);
            process::exit(1);
        }
    };
    win.set_font_poll_interval(100); // ~1s at 10ms sleep

    let mut font_size = if win.mono_size() >= 6.0 { win.mono_size() } else { 14.0 };
    print!("[bterm] config: mono_size={} font_size={}\n", win.mono_size(), font_size);

    // Take TrueType font for local use (needed alongside the window's surface)
    let mut ttf_font: Option<CachedFont> = win.take_mono_font();

    // Compute cell dimensions from font metrics (or fall back to bitmap constants)
    let (mut cell_w, mut cell_h) = if let Some(ref mut font) = ttf_font {
        print!("[bterm] font_size={} ", font_size);
        let dims = ttf_cell_dims(font, font_size);
        print!("cell={}x{}\n", dims.0, dims.1);
        dims
    } else {
        (BITMAP_CELL_W, BITMAP_CELL_H)
    };

    // Widgets use logical UI coordinates; the terminal grid uses device pixels.
    let ui_scale = win.surface().expect("terminal surface").ui_scale() as i32;
    let tab_bar_pixels = (TAB_BAR_HEIGHT * ui_scale) as u32;
    // Calculate content area (below tab bar)
    let mut content_w = WIN_WIDTH;
    let mut content_h = WIN_HEIGHT.saturating_sub(tab_bar_pixels);
    let mut cols = (content_w as usize / cell_w).max(1);
    let mut rows = (content_h as usize / cell_h).max(1);

    // Create tab bar
    let theme = Theme::dark();
    let btop_label = make_static_label("btop");
    let mut tab_bar = TabBar::new(
        Rect::new(0, 0, WIN_WIDTH as i32 / ui_scale, TAB_BAR_HEIGHT),
        vec![btop_label], &theme);

    // Spawn initial tabs: btop (default visible) + shell
    let mut tabs: Vec<Tab> = vec![spawn_tab_cmd(cols, rows, b"/bin/btop\0")];

    // Add shell in a second tab
    let shell_label = make_tab_label();
    tab_bar.add_tab(shell_label);
    tabs.push(spawn_tab(cols, rows));

    // Mouse state for InputState edge detection
    let mut prev_buttons: u32 = 0;

    // Read buffer for PTY output
    let mut pty_buf = [0u8; 4096];

    // Sleep duration for the main loop (10ms)
    let sleep_ts = libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 10_000_000 };
    let mut first_frame = true;
    let mut first_present = true;

    loop {
        // ── 1. Poll Breengel events ─────────────────────────────────
        let events = win.poll_events();
        let mut mouse_x: i32 = 0;
        let mut mouse_y: i32 = 0;
        let mut buttons: u32 = prev_buttons;
        let mut font_changed = false;

        for event in &events {
            match event {
                Event::CloseRequested => {
                    process::exit(0);
                }
                Event::KeyPress { ascii, keycode, modifiers, text, .. } => {
                    // Debug: log key events for +/- and ctrl
                    if *ascii == b'+' || *ascii == b'=' || *ascii == b'-'
                       || *keycode == breengel::keys::MINUS || *keycode == breengel::keys::EQUAL
                    {
                        print!("[bterm] key: ascii={} keycode=0x{:02X} ctrl={}\n",
                               *ascii, *keycode, modifiers.ctrl);
                    }
                    if modifiers.ctrl {
                        // Ctrl+T: new tab
                        if *ascii == b't' - b'a' + 1 || *ascii == b'T' - b'A' + 1
                           || *keycode == breengel::keys::T
                        {
                            let label = make_tab_label();
                            let idx = tab_bar.add_tab(label);
                            tabs.push(spawn_tab(cols, rows));
                            tab_bar.set_selected(idx);
                            if let Some(tab) = tabs.get_mut(idx) {
                                tab.emu.dirty = true;
                            }
                            continue;
                        }
                        // Ctrl+W: close tab
                        if *ascii == b'w' - b'a' + 1 || *ascii == b'W' - b'A' + 1
                           || *keycode == breengel::keys::W
                        {
                            let sel = tab_bar.selected();
                            if tabs.len() <= 1 {
                                process::exit(0);
                            }
                            let _ = io::close(tabs[sel].master_fd);
                            tabs.remove(sel);
                            tab_bar.remove_tab(sel);
                            let new_sel = tab_bar.selected();
                            if let Some(tab) = tabs.get_mut(new_sel) {
                                tab.emu.dirty = true;
                            }
                            continue;
                        }
                        // Ctrl+C: send interrupt to PTY
                        if text.as_bytes() == [3] {
                            let sel = tab_bar.selected();
                            if let Some(tab) = tabs.get(sel) {
                                let _ = io::write(tab.master_fd, &[3]);
                            }
                            continue;
                        }
                        // Ctrl+= or Ctrl++: increase font size
                        if *ascii == b'+' || *ascii == b'=' || *keycode == breengel::keys::EQUAL {
                            if ttf_font.is_some() {
                                let new_size = (font_size + FONT_SIZE_STEP).min(MAX_FONT_SIZE);
                                if new_size != font_size {
                                    print!("[bterm] font size: {} -> {}\n", font_size, new_size);
                                    font_size = new_size;
                                    font_changed = true;
                                }
                            }
                            continue;
                        }
                        // Ctrl+-: decrease font size
                        if *ascii == b'-' || *keycode == breengel::keys::MINUS {
                            if ttf_font.is_some() {
                                let new_size = (font_size - FONT_SIZE_STEP).max(MIN_FONT_SIZE);
                                if new_size != font_size {
                                    print!("[bterm] font size: {} -> {}\n", font_size, new_size);
                                    font_size = new_size;
                                    font_changed = true;
                                }
                            }
                            continue;
                        }
                    }

                    // Arrow keys (Linux key codes); Enter sends a carriage return, as a
                    // terminal's Enter does; any other key sends its text, control codes
                    // included.
                    let sel = tab_bar.selected();
                    if let Some(tab) = tabs.get(sel) {
                        match *keycode {
                            breengel::keys::RIGHT => { let _ = io::write(tab.master_fd, b"\x1b[C"); }
                            breengel::keys::LEFT => { let _ = io::write(tab.master_fd, b"\x1b[D"); }
                            breengel::keys::DOWN => { let _ = io::write(tab.master_fd, b"\x1b[B"); }
                            breengel::keys::UP => { let _ = io::write(tab.master_fd, b"\x1b[A"); }
                            breengel::keys::ENTER => { let _ = io::write(tab.master_fd, b"\r"); }
                            _ => {
                                if !text.is_empty() {
                                    let _ = io::write(tab.master_fd, text.as_bytes());
                                }
                            }
                        }
                    }
                }
                Event::MouseMove { x, y } => {
                    mouse_x = *x;
                    mouse_y = *y;
                }
                Event::MouseButton { button, pressed, x, y } => {
                    mouse_x = *x;
                    mouse_y = *y;
                    if *button == breengel::buttons::LEFT {
                        if *pressed {
                            buttons |= 1;
                        } else {
                            buttons &= !1;
                        }
                    }
                }
                Event::FontChanged => {
                    // Window loaded the new font internally — swap it in
                    win.put_mono_font(ttf_font.take());
                    ttf_font = win.take_mono_font();
                    font_size = win.mono_size();
                    print!("[bterm] font changed: {} size={}\n", win.mono_path(), font_size);
                    font_changed = true;
                }
                Event::ResizeRequested { width, height } => {
                    if let Err(e) = win.apply_resize(*width, *height) {
                        print!("[bterm] resize refused: {}x{}: {}\n", width, height, e);
                    }
                }
                Event::Resized { width, height } => {
                    content_w = *width;
                    content_h = height.saturating_sub(tab_bar_pixels);
                    let new_cols = (content_w as usize / cell_w).max(1);
                    let new_rows = (content_h as usize / cell_h).max(1);
                    if new_cols != cols || new_rows != rows {
                        cols = new_cols;
                        rows = new_rows;
                        for tab in tabs.iter_mut() {
                            tab.emu.resize(cols, rows);
                            let size = termios::Winsize { ws_row: rows as u16, ws_col: cols as u16,
                                ws_xpixel: content_w as u16, ws_ypixel: content_h as u16 };
                            if let Err(e) = termios::set_winsize(tab.master_fd, &size) {
                                print!("[bterm] PTY resize failed: {}\n", e);
                            }
                        }
                    }
                    tab_bar.set_rect(Rect::new(0, 0, *width as i32 / ui_scale, TAB_BAR_HEIGHT));
                    for tab in tabs.iter_mut() {
                        tab.emu.dirty = true;
                    }
                }
                _ => {}
            }
        }

        // Handle font size change: recompute grid, resize all tabs
        if font_changed {
            if let Some(ref mut font) = ttf_font {
                font.clear_cache();
                let (new_cw, new_ch) = ttf_cell_dims(font, font_size);
                cell_w = new_cw;
                cell_h = new_ch;
                let new_cols = (content_w as usize / cell_w).max(1);
                let new_rows = (content_h as usize / cell_h).max(1);
                if new_cols != cols || new_rows != rows {
                    cols = new_cols;
                    rows = new_rows;
                    for tab in tabs.iter_mut() {
                        tab.emu.resize(cols, rows);
                    }
                } else {
                    // Same grid size but different font — force redraw
                    for tab in tabs.iter_mut() {
                        tab.emu.dirty = true;
                    }
                }
            }
        }

        // Pass mouse state to TabBar for tab switching
        let input = InputState::from_raw(mouse_x.div_euclid(ui_scale), mouse_y.div_euclid(ui_scale), buttons, prev_buttons, breengel::clock::now_ns() / 1_000_000);
        if let WidgetEvent::ValueChanged(_) = tab_bar.update(&input) {
            let sel = tab_bar.selected();
            if let Some(tab) = tabs.get_mut(sel) {
                tab.emu.dirty = true;
            }
        }
        prev_buttons = buttons;

        // ── 2. Read PTY output for ALL tabs (non-blocking) ──────────
        for tab in tabs.iter_mut() {
            loop {
                match io::read(tab.master_fd, &mut pty_buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        for i in 0..n {
                            tab.emu.feed(pty_buf[i]);
                        }
                    }
                    Err(_) => break, // EAGAIN or error
                }
            }
        }

        // ── 3. Render ───────────────────────────────────────────────
        let sel = tab_bar.selected();
        let any_dirty = tabs.get(sel).map_or(false, |t| t.emu.dirty);
        let need_redraw = any_dirty || !events.is_empty() || font_changed || first_frame;
        first_frame = false;

        if need_redraw {
            let fb = match win.surface() {
                Ok(fb) => fb,
                Err(e) => {
                    print!("[bterm] ERROR: no window surface: {}\n", e);
                    process::exit(1);
                }
            };

            // Draw tab bar
            tab_bar.draw(fb, &theme);

            // Render active tab's terminal emulator into content area. The surface holds
            // the newest committed frame, so a tab whose grid is unchanged is not redrawn.
            if let Some(tab) = tabs.get_mut(sel) {
                tab.emu.render(
                    fb,
                    0,
                    tab_bar_pixels as usize,
                    content_w as usize,
                    content_h as usize,
                    cell_w,
                    cell_h,
                    font_size,
                    ttf_font.as_mut(),
                );
            }

            match win.present() {
                Err(e) => print!("[bterm] present failed: {}\n", e),
                Ok(()) if first_present => {
                    first_present = false;
                    print!("[bterm] first frame presented: {}x{}\n", win.width(), win.height());
                    // Observe the selected tab's lower highlight in the committed GPU
                    // front: section 14.3's accent at the scaled logical tab height.
                    let mut pixels = vec![0u8; win.width() as usize * win.height() as usize * 4];
                    match libbreenix::window::capture(win.id(), &mut pixels, None) {
                        Ok(_) => {
                            let (x, y) = (4 * ui_scale as usize, (TAB_BAR_HEIGHT as usize - 1) * ui_scale as usize);
                            let at = (y * win.width() as usize + x) * 4;
                            if pixels[at..at + 3] == [220, 130, 70] {
                                print!("[bterm] tabs read back: bounds=0,0,{},{} window={}x{} scale={} highlight exact\n",
                                    win.width(), tab_bar_pixels, win.width(), win.height(), ui_scale);
                            } else {
                                print!("[bterm] tab layout FAILED: highlight at ({},{}) is {:?}\n", x, y, &pixels[at..at+3]);
                            }
                        }
                        Err(e) => print!("[bterm] tab layout FAILED: capture {}\n", e),
                    }
                }
                Ok(()) => {}
            }
        } else {
            // Nothing to draw — sleep briefly to avoid busy-waiting
            let _ = time::nanosleep(&sleep_ts);
        }
    }
}
