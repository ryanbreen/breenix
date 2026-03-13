//! bterm — Standalone terminal emulator using Breengel windowing.
//!
//! Each tab spawns its own PTY + shell (bsh). Keyboard input flows to the
//! active tab's PTY master; PTY output is fed through a VT100/ANSI parser
//! and rendered into the Breengel window's framebuffer.
//!
//! Keyboard shortcuts:
//!   Ctrl+T  — open a new tab
//!   Ctrl+W  — close the active tab (exits if last tab)

use std::process;

use breengel::{Window, Event, TabBar, Rect, Theme, Color, FrameBuf};
use libbreenix::io;
use libbreenix::fs;
use libbreenix::process::{fork, exec, setsid, ForkResult};
use libbreenix::pty;
use libbreenix::types::Fd;
use libbreenix::time;

use libfont::{Font, CachedFont};
use libgfx::bitmap_font;
use libgfx::ttf_font;

use libbui::{InputState, WidgetEvent};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Default TrueType font pixel size.
const TTF_FONT_SIZE: f32 = 16.0;

/// Path to the TrueType monospace font on the ext2 filesystem.
const FONT_PATH: &str = "/usr/share/fonts/DejaVuSansMono.ttf";

/// Noto Sans Mono 16px cell dimensions for terminal text.
/// CELL_W must match bitmap_font::metrics().char_width (7 for size_16).
const CELL_W: usize = 7;
const CELL_H: usize = 18;

/// Tab bar height in pixels.
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
              clip_w: usize, clip_h: usize, mut ttf: Option<&mut CachedFont>) {
        if !self.dirty { return; }
        self.dirty = false;
        let max_x = (x_off + clip_w).min(fb.width);
        let max_y = (y_off + clip_h).min(fb.height);
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
                    if let Some(ref mut font) = ttf {
                        ttf_font::draw_char(fb, *font, cell.ch as char, px as i32, py as i32, TTF_FONT_SIZE, fg);
                    } else {
                        bitmap_font::draw_char(fb, cell.ch as char, px, py, fg);
                    }
                }
            }
            // Cursor underline
            if row == self.cursor_y && self.cursor_x < self.cols {
                let cx = x_off + self.cursor_x * CELL_W;
                let cw = CELL_W;
                for dy in 0..2usize { for dx in 0..cw {
                    if cx + dx < max_x && py + CELL_H - 2 + dy < max_y {
                        fb.put_pixel(cx + dx, py + CELL_H - 2 + dy, Color::WHITE);
                    }
                }}
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
        Err(_) => return (Fd::from_raw(0), -1),
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
            // Open PTY slave using stack-allocated path
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
        Ok(ForkResult::Parent(child_pid)) => (master_fd, child_pid.raw() as i64),
        Err(_) => { let _ = io::close(master_fd); (Fd::from_raw(0), -1) }
    }
}

// ─── Tab helpers ────────────────────────────────────────────────────────────

/// Compute terminal grid dimensions from the content area.
fn term_grid(content_w: u32, content_h: u32) -> (usize, usize) {
    let cols = content_w as usize / CELL_W;
    let rows = content_h as usize / CELL_H;
    (cols, rows)
}

/// Generate a label for a new tab (e.g. b"shell 1", b"shell 2", ...).
/// We use a static counter to keep labels unique.
static mut TAB_COUNTER: usize = 0;

/// Tab label storage — we need 'static lifetimes for TabBar labels.
/// We keep a pool of leaked &'static [u8] slices.
fn make_tab_label() -> &'static [u8] {
    let n = unsafe {
        TAB_COUNTER += 1;
        TAB_COUNTER
    };
    // Format a label like "shell 1"
    let s = format!("shell {}", n);
    // Leak to get a 'static lifetime — tabs are long-lived, this is fine.
    let boxed: Box<[u8]> = s.into_bytes().into_boxed_slice();
    Box::leak(boxed)
}

fn spawn_tab(cols: usize, rows: usize) -> Tab {
    spawn_tab_cmd(cols, rows, b"/bin/bsh\0")
}

fn spawn_tab_cmd(cols: usize, rows: usize, cmd: &[u8]) -> Tab {
    let (master_fd, child_pid) = spawn_child(cmd);
    // Set master fd to non-blocking so we can poll without blocking the event loop
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

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    // Load TrueType font from ext2 filesystem
    let font_data = std::fs::read(FONT_PATH).ok();
    let font_parsed = font_data.as_ref().and_then(|data| Font::parse(data).ok());
    let mut ttf_font: Option<CachedFont> = font_parsed.map(|f| CachedFont::new(f, 256));

    // Create window
    let mut win = match Window::new(b"Terminal", WIN_WIDTH, WIN_HEIGHT) {
        Ok(w) => w,
        Err(e) => {
            print!("[bterm] ERROR: failed to create window: {}\n", e);
            process::exit(1);
        }
    };

    // Calculate content area (below tab bar)
    let content_w = WIN_WIDTH;
    let content_h = WIN_HEIGHT - TAB_BAR_HEIGHT as u32;
    let (cols, rows) = term_grid(content_w, content_h);

    // Create tab bar
    let btop_label = make_static_label("btop");
    let mut tab_bar = TabBar::new(
        Rect::new(0, 0, WIN_WIDTH as i32, TAB_BAR_HEIGHT),
        vec![btop_label],
    );
    let theme = Theme::dark();

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

    loop {
        // ── 1. Poll Breengel events ─────────────────────────────────
        let events = win.poll_events();
        let mut mouse_x: i32 = 0;
        let mut mouse_y: i32 = 0;
        let mut buttons: u32 = prev_buttons;

        for event in &events {
            match event {
                Event::CloseRequested => {
                    process::exit(0);
                }
                Event::KeyPress { ascii, keycode, modifiers } => {
                    if modifiers.ctrl {
                        // Ctrl+T: new tab
                        if *ascii == b't' - b'a' + 1 || *ascii == b'T' - b'A' + 1
                           || *keycode == b't' as u16 || *keycode == b'T' as u16
                        {
                            let label = make_tab_label();
                            let idx = tab_bar.add_tab(label);
                            tabs.push(spawn_tab(cols, rows));
                            tab_bar.set_selected(idx);
                            // Force full redraw
                            if let Some(tab) = tabs.get_mut(idx) {
                                tab.emu.dirty = true;
                            }
                            continue;
                        }
                        // Ctrl+W: close tab
                        if *ascii == b'w' - b'a' + 1 || *ascii == b'W' - b'A' + 1
                           || *keycode == b'w' as u16 || *keycode == b'W' as u16
                        {
                            let sel = tab_bar.selected();
                            if tabs.len() <= 1 {
                                // Last tab — exit
                                process::exit(0);
                            }
                            // Close PTY and remove tab
                            let _ = io::close(tabs[sel].master_fd);
                            tabs.remove(sel);
                            tab_bar.remove_tab(sel);
                            // Force redraw on newly selected tab
                            let new_sel = tab_bar.selected();
                            if let Some(tab) = tabs.get_mut(new_sel) {
                                tab.emu.dirty = true;
                            }
                            continue;
                        }
                        // Ctrl+C: send interrupt to PTY
                        if *ascii == 3 {
                            let sel = tab_bar.selected();
                            if let Some(tab) = tabs.get(sel) {
                                let _ = io::write(tab.master_fd, &[3]);
                            }
                            continue;
                        }
                    }

                    // Arrow keys (USB HID keycodes)
                    let sel = tab_bar.selected();
                    if let Some(tab) = tabs.get(sel) {
                        match *keycode {
                            79 => { let _ = io::write(tab.master_fd, b"\x1b[C"); } // Right
                            80 => { let _ = io::write(tab.master_fd, b"\x1b[D"); } // Left
                            81 => { let _ = io::write(tab.master_fd, b"\x1b[B"); } // Down
                            82 => { let _ = io::write(tab.master_fd, b"\x1b[A"); } // Up
                            _ => {
                                // Regular key
                                if *ascii > 0 {
                                    let _ = io::write(tab.master_fd, &[*ascii]);
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
                    if *button == 0 || *button == 1 {
                        if *pressed {
                            buttons |= 1;
                        } else {
                            buttons &= !1;
                        }
                    }
                }
                _ => {}
            }
        }

        // Pass mouse state to TabBar for tab switching
        let input = InputState::from_raw(mouse_x, mouse_y, buttons, prev_buttons);
        if let WidgetEvent::ValueChanged(_) = tab_bar.update(&input) {
            // Tab changed — force redraw of newly selected tab
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
        let need_redraw = any_dirty || !events.is_empty();

        if need_redraw {
            let fb = win.framebuf();

            // Draw tab bar
            tab_bar.draw(fb, &theme);

            // Render active tab's terminal emulator into content area
            if let Some(tab) = tabs.get_mut(sel) {
                tab.emu.render(
                    fb,
                    0,
                    TAB_BAR_HEIGHT as usize,
                    content_w as usize,
                    content_h as usize,
                    ttf_font.as_mut(),
                );
            }

            let _ = win.present();
        } else {
            // Nothing to draw — sleep briefly to avoid busy-waiting
            let _ = time::nanosleep(&sleep_ts);
        }
    }
}
