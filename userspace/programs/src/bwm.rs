//! Breenix Window Manager (bwm)
//!
//! Userspace window manager that provides tabbed terminal sessions.
//! Takes over the display from the kernel terminal manager and renders
//! its own tab bar + terminal content to the framebuffer.
//!
//! Tabs:
//!   F1 - Shell (bsh)
//!   F2 - Logs (kernel log viewer via /proc/kmsg)
//!   F3 - Btop (system monitor)

use std::process;

use libbreenix::graphics;
use libbreenix::io;
use libbreenix::process::{fork, exec, waitpid, setsid, ForkResult, WNOHANG};
use libbreenix::pty;
use libbreenix::types::Fd;

use libgfx::bitmap_font;
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Tab indices
const TAB_SHELL: usize = 0;
const TAB_LOGS: usize = 1;
const TAB_BTOP: usize = 2;

/// Tab bar height in pixels (matches kernel terminal_manager TAB_HEIGHT)
const TAB_BAR_HEIGHT: usize = 24;

/// Separator height below tab bar (matches kernel terminal_manager)
const SEPARATOR_HEIGHT: usize = 2;

/// Pane padding (matches kernel terminal_manager pane_padding)
const PANE_PADDING: usize = 4;

/// Noto Sans Mono 16px cell dimensions.
/// Raster width is 7px but we use 6px cell advance for tighter terminal text.
/// Characters overlap by 1px at their anti-aliased edges.
const CELL_W: usize = 6;
const CELL_H: usize = 18;
/// Actual glyph raster width (for clipping / two-pass render).
const GLYPH_W: usize = 7;

/// Colors (match kernel terminal_manager exactly)
const BG_COLOR: Color = Color::rgb(20, 30, 50);
const FG_COLOR: Color = Color::rgb(255, 255, 255);
const TAB_BG: Color = Color::rgb(40, 50, 70);
const TAB_ACTIVE_BG: Color = Color::rgb(60, 80, 120);
const TAB_INACTIVE_UNREAD_BG: Color = Color::rgb(80, 60, 60);
const TAB_TEXT: Color = Color::rgb(255, 255, 255);
const TAB_INACTIVE_TEXT: Color = Color::rgb(180, 180, 180);
const TAB_SHORTCUT_TEXT: Color = Color::rgb(120, 140, 160);
const SEPARATOR_COLOR: Color = Color::rgb(60, 80, 100);
const CURSOR_COLOR: Color = Color::rgb(255, 255, 255);
const UNREAD_DOT: Color = Color::rgb(255, 100, 100);

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
    Escape,     // Got ESC
    Csi,        // Got ESC [
    CsiParam,   // Collecting CSI parameters
    OscString,  // Got ESC ] (operating system command, skip until ST)
}

// ─── Terminal Emulator (per-tab) ─────────────────────────────────────────────

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

    /// Scroll the screen up by one line
    fn scroll_up(&mut self) {
        let cols = self.cols;
        // Move lines up
        for y in 1..self.rows {
            for x in 0..cols {
                self.cells[(y - 1) * cols + x] = self.cells[y * cols + x];
            }
        }
        // Clear bottom line
        let last_row = self.rows - 1;
        for x in 0..cols {
            self.cells[last_row * cols + x] = Cell::blank();
        }
        self.dirty = true;
    }

    /// Put a character at cursor, advance cursor
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

    /// Process a single byte through the ANSI state machine
    fn feed(&mut self, byte: u8) {
        match self.ansi_state {
            AnsiState::Normal => match byte {
                0x1b => self.ansi_state = AnsiState::Escape,
                b'\n' => {
                    self.cursor_x = 0; // LF implies CR in terminal emulators
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
                    // Backspace
                    if self.cursor_x > 0 {
                        self.cursor_x -= 1;
                        self.dirty = true;
                    }
                }
                0x07 => {} // Bell - ignore
                ch if ch >= 0x20 => self.put_char(ch),
                _ => {} // Ignore other control characters
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
                    // SS3 - skip next byte (function key)
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
                    // Final byte - execute CSI command
                    let nparams = self.ansi_param_idx + 1;
                    self.execute_csi(byte, nparams);
                    self.ansi_state = AnsiState::Normal;
                }
            }
            AnsiState::OscString => {
                // Skip until BEL or ST (ESC \)
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
                // CUP: Cursor Position (row;col, 1-based)
                let row = if p0 > 0 { p0 - 1 } else { 0 };
                let col = if p1 > 0 { p1 - 1 } else { 0 };
                self.cursor_y = row.min(self.rows - 1);
                self.cursor_x = col.min(self.cols - 1);
                self.dirty = true;
            }
            b'A' => {
                // CUU: Cursor Up
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_y = self.cursor_y.saturating_sub(n);
                self.dirty = true;
            }
            b'B' => {
                // CUD: Cursor Down
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_y = (self.cursor_y + n).min(self.rows - 1);
                self.dirty = true;
            }
            b'C' => {
                // CUF: Cursor Forward
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_x = (self.cursor_x + n).min(self.cols - 1);
                self.dirty = true;
            }
            b'D' => {
                // CUB: Cursor Back
                let n = if p0 > 0 { p0 } else { 1 };
                self.cursor_x = self.cursor_x.saturating_sub(n);
                self.dirty = true;
            }
            b'J' => {
                // ED: Erase in Display
                match p0 {
                    0 => {
                        // Clear from cursor to end of screen
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
                        // Clear from start to cursor
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
                        // Clear entire screen
                        for i in 0..self.cells.len() {
                            self.cells[i] = Cell::blank();
                        }
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            b'K' => {
                // EL: Erase in Line
                match p0 {
                    0 => {
                        // Clear from cursor to end of line
                        for x in self.cursor_x..self.cols {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                    }
                    1 => {
                        // Clear from start to cursor
                        for x in 0..=self.cursor_x.min(self.cols - 1) {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                    }
                    2 => {
                        // Clear entire line
                        for x in 0..self.cols {
                            *self.cell_mut(x, self.cursor_y) = Cell::blank();
                        }
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            b'm' => {
                // SGR: Select Graphic Rendition
                for i in 0..nparams {
                    let code = self.ansi_params[i];
                    match code {
                        0 => {
                            self.fg = FG_COLOR;
                            self.bg = BG_COLOR;
                            self.bold = false;
                        }
                        1 => self.bold = true,
                        22 => self.bold = false,
                        30..=37 => self.fg = ANSI_COLORS[(code - 30) as usize],
                        39 => self.fg = FG_COLOR,
                        40..=47 => self.bg = ANSI_COLORS[(code - 40) as usize],
                        49 => self.bg = BG_COLOR,
                        90..=97 => {
                            // Bright foreground colors
                            let mut c = ANSI_COLORS[(code - 90) as usize];
                            c.r = c.r.saturating_add(55);
                            c.g = c.g.saturating_add(55);
                            c.b = c.b.saturating_add(55);
                            self.fg = c;
                        }
                        _ => {} // Ignore unsupported SGR codes
                    }
                }
                // Handle bare ESC[m (no params = reset)
                if nparams == 1 && self.ansi_params[0] == 0 {
                    self.fg = FG_COLOR;
                    self.bg = BG_COLOR;
                    self.bold = false;
                }
            }
            b'L' => {
                // IL: Insert Lines
                let n = if p0 > 0 { p0 } else { 1 };
                let cols = self.cols;
                for _ in 0..n {
                    if self.cursor_y < self.rows - 1 {
                        // Shift lines down
                        for y in (self.cursor_y + 1..self.rows).rev() {
                            for x in 0..cols {
                                self.cells[y * cols + x] = self.cells[(y - 1) * cols + x];
                            }
                        }
                    }
                    // Clear current line
                    for x in 0..cols {
                        self.cells[self.cursor_y * cols + x] = Cell::blank();
                    }
                }
                self.dirty = true;
            }
            b'M' => {
                // DL: Delete Lines
                let n = if p0 > 0 { p0 } else { 1 };
                let cols = self.cols;
                for _ in 0..n {
                    // Shift lines up
                    for y in self.cursor_y..self.rows - 1 {
                        for x in 0..cols {
                            self.cells[y * cols + x] = self.cells[(y + 1) * cols + x];
                        }
                    }
                    // Clear bottom line
                    let last = self.rows - 1;
                    for x in 0..cols {
                        self.cells[last * cols + x] = Cell::blank();
                    }
                }
                self.dirty = true;
            }
            b'G' => {
                // CHA: Cursor Horizontal Absolute
                let col = if p0 > 0 { p0 - 1 } else { 0 };
                self.cursor_x = col.min(self.cols - 1);
                self.dirty = true;
            }
            b'd' => {
                // VPA: Vertical Position Absolute
                let row = if p0 > 0 { p0 - 1 } else { 0 };
                self.cursor_y = row.min(self.rows - 1);
                self.dirty = true;
            }
            b'r' => {
                // DECSTBM: Set Scrolling Region (ignored for now)
            }
            b'h' | b'l' => {
                // SM/RM: Set/Reset Mode (ignored - includes cursor visibility etc.)
            }
            b'~' => {
                // Special keys (F1~, F2~, etc.) - handled at input level
            }
            _ => {} // Ignore unknown CSI commands
        }
    }

    /// Feed a slice of bytes through the emulator
    fn feed_bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.feed(b);
        }
    }

    /// Render the terminal emulator content to a framebuffer region.
    ///
    /// Two-pass rendering: backgrounds first, then glyphs. This allows
    /// glyphs (GLYPH_W=7) to overflow beyond their cell (CELL_W=5) without
    /// being erased by the next cell's background fill.
    fn render(&mut self, fb: &mut FrameBuf, x_off: usize, y_off: usize) {
        if !self.dirty {
            return;
        }
        self.dirty = false;

        for row in 0..self.rows {
            let py = y_off + row * CELL_H;

            // Pass 1: draw all cell backgrounds for this row
            for col in 0..self.cols {
                let cell = self.cell(col, row);
                let px = x_off + col * CELL_W;
                for dy in 0..CELL_H {
                    for dx in 0..CELL_W {
                        fb.put_pixel(px + dx, py + dy, cell.bg);
                    }
                }
            }
            // Clear the overflow region after the last cell (GLYPH_W - CELL_W pixels)
            let overflow_start = x_off + self.cols * CELL_W;
            for dy in 0..CELL_H {
                for dx in 0..GLYPH_W {
                    fb.put_pixel(overflow_start + dx, py + dy, BG_COLOR);
                }
            }

            // Pass 2: draw all glyphs for this row (may extend into next cell)
            for col in 0..self.cols {
                let cell = self.cell(col, row);
                if cell.ch > b' ' && cell.ch < 127 {
                    let fg = if cell.bold {
                        Color::rgb(
                            cell.fg.r.saturating_add(55),
                            cell.fg.g.saturating_add(55),
                            cell.fg.b.saturating_add(55),
                        )
                    } else {
                        cell.fg
                    };
                    let px = x_off + col * CELL_W;
                    bitmap_font::draw_char(fb, cell.ch as char, px, py + 1, fg);
                }
            }
        }

        // Draw cursor
        if self.cursor_x < self.cols && self.cursor_y < self.rows {
            let cx = x_off + self.cursor_x * CELL_W;
            let cy = y_off + self.cursor_y * CELL_H + CELL_H - 2;
            for dx in 0..CELL_W {
                fb.put_pixel(cx + dx, cy, CURSOR_COLOR);
                fb.put_pixel(cx + dx, cy + 1, CURSOR_COLOR);
            }
        }
    }
}

// ─── Tab State ───────────────────────────────────────────────────────────────

struct Tab {
    name: &'static str,
    shortcut: &'static str,
    emu: TermEmu,
    master_fd: Option<Fd>,
    child_pid: i64,
    has_unread: bool,
}

// ─── Escape Sequence Parser for Keyboard Input ──────────────────────────────

enum KeyEvent {
    Char(u8),
    EscSeq([u8; 8], usize),
    F1,
    F2,
    F3,
    None,
}

struct InputParser {
    buf: [u8; 8],
    len: usize,
}

impl InputParser {
    fn new() -> Self {
        Self { buf: [0; 8], len: 0 }
    }

    fn feed(&mut self, byte: u8) -> KeyEvent {
        self.buf[self.len] = byte;
        self.len += 1;

        // Check for escape sequences
        if self.buf[0] == 0x1b {
            if self.len == 1 {
                return KeyEvent::None; // Wait for more bytes
            }
            if self.len == 2 {
                if self.buf[1] == b'[' || self.buf[1] == b'O' {
                    return KeyEvent::None; // Wait for more
                }
                // ESC + something else: return ESC then the byte
                self.len = 0;
                return KeyEvent::Char(byte);
            }
            // len >= 3: check for complete sequences

            // SS3 F-keys (ESC O P/Q/R)
            if self.len == 3 && self.buf[1] == b'O' {
                let result = match self.buf[2] {
                    b'P' => KeyEvent::F1,
                    b'Q' => KeyEvent::F2,
                    b'R' => KeyEvent::F3,
                    _ => {
                        self.len = 0;
                        return KeyEvent::Char(byte);
                    }
                };
                self.len = 0;
                return result;
            }

            // CSI sequences (ESC [ ...)
            if self.buf[1] == b'[' {
                let last = self.buf[self.len - 1];
                // CSI final byte is in 0x40-0x7E (@..~)
                if last >= 0x40 && last <= 0x7E {
                    // Check CSI F-key forms first (ESC [ 1 1 ~ etc.)
                    if self.len == 5 {
                        match (self.buf[2], self.buf[3], self.buf[4]) {
                            (b'1', b'1', b'~') => { self.len = 0; return KeyEvent::F1; }
                            (b'1', b'2', b'~') => { self.len = 0; return KeyEvent::F2; }
                            (b'1', b'3', b'~') => { self.len = 0; return KeyEvent::F3; }
                            _ => {}
                        }
                    }
                    // Complete CSI sequence (arrows, home, end, delete, etc.)
                    let buf = self.buf;
                    let len = self.len;
                    self.len = 0;
                    return KeyEvent::EscSeq(buf, len);
                }
                // Not complete yet
                if self.len >= 8 {
                    self.len = 0;
                    return KeyEvent::Char(byte);
                }
                return KeyEvent::None;
            }

            // Unknown escape sequence, give up
            self.len = 0;
            return KeyEvent::Char(byte);
        }

        // Regular character
        self.len = 0;
        KeyEvent::Char(byte)
    }

    /// Flush pending escape sequence as individual chars
    fn flush(&mut self) -> Option<u8> {
        if self.len > 0 {
            let byte = self.buf[0];
            // Shift buffer
            for i in 0..self.len - 1 {
                self.buf[i] = self.buf[i + 1];
            }
            self.len -= 1;
            Some(byte)
        } else {
            None
        }
    }
}

// ─── Helper Functions ────────────────────────────────────────────────────────

/// Spawn a child process connected to a PTY
fn spawn_child(path: &[u8], _name: &str) -> (Fd, i64) {
    // Create PTY pair
    let (master_fd, slave_path) = match pty::openpty() {
        Ok(pair) => pair,
        Err(_) => return (Fd::from_raw(0), -1),
    };

    let slave_path_slice = pty::slave_path_bytes(&slave_path);

    match fork() {
        Ok(ForkResult::Child) => {
            // Child process: set up PTY slave as stdin/stdout/stderr
            // Diagnostic markers to serial (fd 1 = StdIo before dup2)
            let _ = io::write(Fd::from_raw(1), b"[child:");
            let _ = io::write(Fd::from_raw(1), _name.as_bytes());
            let _ = io::write(Fd::from_raw(1), b":fork]\n");

            let _ = setsid(); // New session

            // Close ALL inherited FDs > 2 (master PTY FDs from parent BWM)
            // This prevents leaking master FDs to child processes which would
            // keep PTY refcounts elevated and prevent proper cleanup.
            for fd_num in 3..20 {
                let _ = io::close(Fd::from_raw(fd_num));
            }

            // Build null-terminated path for open
            let mut open_path = [0u8; 64];
            let copy_len = slave_path_slice.len().min(63);
            open_path[..copy_len].copy_from_slice(&slave_path_slice[..copy_len]);

            let path_str = core::str::from_utf8(&open_path[..copy_len]).unwrap_or("/dev/pts/0");

            // Open the slave PTY
            let slave_fd = match libbreenix::fs::open(path_str, 0x02) {
                // O_RDWR
                Ok(fd) => fd,
                Err(_) => {
                    let _ = io::write(Fd::from_raw(1), b"[child:");
                    let _ = io::write(Fd::from_raw(1), _name.as_bytes());
                    let _ = io::write(Fd::from_raw(1), b":open_fail]\n");
                    libbreenix::process::exit(126);
                }
            };

            // Dup to stdin/stdout/stderr
            let _ = io::dup2(slave_fd, Fd::from_raw(0));
            let _ = io::dup2(slave_fd, Fd::from_raw(1));
            let _ = io::dup2(slave_fd, Fd::from_raw(2));

            // Close original if it's not 0, 1, or 2
            if slave_fd.raw() > 2 {
                let _ = io::close(slave_fd);
            }

            // Exec the program (after dup2, stdout goes to PTY slave)
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

/// Read /proc/kmsg for kernel log content
fn read_kmsg() -> Vec<u8> {
    let mut buf = vec![0u8; 4096];
    match libbreenix::fs::open("/proc/kmsg", 0) {
        Ok(fd) => {
            let mut total = 0;
            loop {
                match io::read(fd, &mut buf[total..]) {
                    Ok(n) if n > 0 => {
                        total += n;
                        if total >= buf.len() - 256 {
                            buf.resize(buf.len() + 4096, 0);
                        }
                    }
                    _ => break,
                }
            }
            let _ = io::close(fd);
            buf.truncate(total);
            buf
        }
        Err(_) => Vec::new(),
    }
}

// ─── Tab Bar Hit Testing ─────────────────────────────────────────────────────

/// Hit-test the tab bar to determine which tab was clicked.
/// `local_x` is in pane-local coordinates (0 = left edge of right pane).
/// Returns the tab index if a tab was hit, None otherwise.
fn hit_test_tab_bar(tabs: &[Tab], local_x: usize, _width: usize) -> Option<usize> {
    let mut tab_x: usize = 4;
    for (i, tab) in tabs.iter().enumerate() {
        let title_width = tab.name.as_bytes().len() * TAB_CHAR_W;
        let shortcut_width = (tab.shortcut.as_bytes().len() + 2) * TAB_CHAR_W;
        let tab_padding = 12;
        let tab_width = title_width + shortcut_width + tab_padding * 2;

        if local_x >= tab_x && local_x < tab_x + tab_width {
            return Some(i);
        }

        tab_x += tab_width + 4;
    }
    None
}

// ─── Tab Bar Rendering ───────────────────────────────────────────────────────

/// Tab label character advance width (same as terminal CELL_W).
const TAB_CHAR_W: usize = CELL_W;
/// Noto glyph height for vertical centering in tab bar.
const TAB_GLYPH_H: usize = 16;

/// Draw text at CELL_W spacing (tighter than the default font raster width).
fn draw_text_tight(fb: &mut FrameBuf, text: &[u8], x: usize, y: usize, fg: Color) {
    for (i, &ch) in text.iter().enumerate() {
        bitmap_font::draw_char(fb, ch as char, x + i * CELL_W, y, fg);
    }
}

fn draw_tab_bar(fb: &mut FrameBuf, tabs: &[Tab], active: usize, width: usize) {
    // Tab bar background (matches kernel: rgb(40, 50, 70))
    for y in 0..TAB_BAR_HEIGHT {
        for x in 0..width {
            fb.put_pixel(x, y, TAB_BG);
        }
    }

    // Draw variable-width tabs matching kernel terminal_manager layout
    let mut tab_x: usize = 4;
    for (i, tab) in tabs.iter().enumerate() {
        let title_bytes = tab.name.as_bytes();
        let shortcut_bytes = tab.shortcut.as_bytes();

        let title_width = title_bytes.len() * TAB_CHAR_W;
        let shortcut_width = (shortcut_bytes.len() + 2) * TAB_CHAR_W; // +2 for []
        let tab_padding = 12;
        let tab_width = title_width + shortcut_width + tab_padding * 2;

        // Tab background color (matches kernel colors exactly)
        let bg = if i == active {
            TAB_ACTIVE_BG
        } else if tabs[i].has_unread {
            TAB_INACTIVE_UNREAD_BG
        } else {
            Color::rgb(30, 40, 55)
        };

        // Tab background rect
        for y in 2..TAB_BAR_HEIGHT - 2 {
            for x in tab_x..tab_x + tab_width {
                if x < width {
                    fb.put_pixel(x, y, bg);
                }
            }
        }

        // Title text (anti-aliased noto font, at CELL_W spacing)
        let title_color = if i == active { TAB_TEXT } else { TAB_INACTIVE_TEXT };
        let text_x = tab_x + tab_padding / 2;
        let text_y = (TAB_BAR_HEIGHT - TAB_GLYPH_H) / 2;
        draw_text_tight(fb, title_bytes, text_x, text_y, title_color);

        // Shortcut text "[F1]"
        let mut shortcut_label = [0u8; 8];
        let mut sp = 0;
        shortcut_label[sp] = b'['; sp += 1;
        for &b in shortcut_bytes {
            if sp < 7 { shortcut_label[sp] = b; sp += 1; }
        }
        if sp < 8 { shortcut_label[sp] = b']'; sp += 1; }
        let shortcut_x = text_x + title_width + 4;
        draw_text_tight(fb, &shortcut_label[..sp], shortcut_x, text_y, TAB_SHORTCUT_TEXT);

        // Unread indicator: 4x4 dot at top-right of tab
        if tab.has_unread && i != active {
            let dot_x = tab_x + tab_width - 8;
            let dot_y = 6;
            for dy in 0..4_usize {
                for dx in 0..4_usize {
                    if dot_x + dx < width {
                        fb.put_pixel(dot_x + dx, dot_y + dy, UNREAD_DOT);
                    }
                }
            }
        }

        tab_x += tab_width + 4;
    }

    // Separator line below tab bar
    for y in TAB_BAR_HEIGHT..TAB_BAR_HEIGHT + SEPARATOR_HEIGHT {
        for x in 0..width {
            fb.put_pixel(x, y, SEPARATOR_COLOR);
        }
    }
}

// ─── Main ────────────────────────────────────────────────────────────────────

fn main() {
    print!("[bwm] Breenix Window Manager starting...\n");

    // Step 1: Take over the display from kernel terminal manager
    if let Err(e) = graphics::take_over_display() {
        print!("[bwm] WARNING: take_over_display failed: {}\n", e);
    }

    // Step 2: Get framebuffer info and mmap
    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(e) => {
            print!("[bwm] ERROR: fbinfo failed: {}\n", e);
            process::exit(1);
        }
    };

    let fb_ptr = match graphics::fb_mmap() {
        Ok(ptr) => ptr,
        Err(e) => {
            print!("[bwm] ERROR: fb_mmap failed: {}\n", e);
            process::exit(1);
        }
    };

    // After take_over_display, fb_mmap maps the right pane (local coords 0,0 = top-left of right pane)
    let full_width = info.width as usize;
    let height = info.height as usize;
    let bpp = info.bytes_per_pixel as usize;
    let pane_width = full_width - (full_width / 2 + 4); // right pane after divider

    let mut fb = unsafe {
        FrameBuf::from_raw(
            fb_ptr,
            pane_width,
            height,
            pane_width * bpp,
            bpp,
            info.is_bgr(),
        )
    };

    // Calculate terminal dimensions (below tab bar + separator + padding)
    let term_y_offset = TAB_BAR_HEIGHT + SEPARATOR_HEIGHT + PANE_PADDING;
    let term_x_offset = PANE_PADDING;
    let term_pixel_width = pane_width.saturating_sub(PANE_PADDING * 2);
    let term_pixel_height = height.saturating_sub(term_y_offset + PANE_PADDING);
    let term_cols = term_pixel_width / CELL_W;
    let term_rows = term_pixel_height / CELL_H;

    let font_m = bitmap_font::metrics();
    print!("[bwm] Font metrics: char_width={}, char_height={}, line_height={}\n",
           font_m.char_width, font_m.char_height, font_m.line_height());
    print!("[bwm] Cell: {}x{} (CELL_W x CELL_H)\n", CELL_W, CELL_H);
    print!("[bwm] Display: {}x{}, pane: {}x{}, terminal: {}x{} cells\n",
           full_width, height, pane_width, height, term_cols, term_rows);

    // Step 3: Enter raw mode on stdin
    let mut orig_termios = libbreenix::termios::Termios::default();
    let _ = libbreenix::termios::tcgetattr(Fd::from_raw(0), &mut orig_termios);
    let mut raw = orig_termios;
    libbreenix::termios::cfmakeraw(&mut raw);
    let _ = libbreenix::termios::tcsetattr(Fd::from_raw(0), libbreenix::termios::TCSANOW, &raw);

    // Step 4: Create tabs with PTY children
    let (shell_master, shell_pid) = spawn_child(b"/bin/bsh\0", "bsh");
    let (btop_master, btop_pid) = spawn_child(b"/bin/btop\0", "btop");

    // Set PTY window size so child processes know the terminal dimensions
    let ws = libbreenix::termios::Winsize {
        ws_row: term_rows as u16,
        ws_col: term_cols as u16,
        ws_xpixel: term_pixel_width as u16,
        ws_ypixel: term_pixel_height as u16,
    };
    let _ = libbreenix::termios::set_winsize(shell_master, &ws);
    let _ = libbreenix::termios::set_winsize(btop_master, &ws);

    let mut tabs = [
        Tab {
            name: "Shell",
            shortcut: "F1",
            emu: TermEmu::new(term_cols, term_rows),
            master_fd: Some(shell_master),
            child_pid: shell_pid,
            has_unread: false,
        },
        Tab {
            name: "Logs",
            shortcut: "F2",
            emu: TermEmu::new(term_cols, term_rows),
            master_fd: None, // Logs tab reads /proc/kmsg directly
            child_pid: -1,
            has_unread: false,
        },
        Tab {
            name: "Btop",
            shortcut: "F3",
            emu: TermEmu::new(term_cols, term_rows),
            master_fd: Some(btop_master),
            child_pid: btop_pid,
            has_unread: false,
        },
    ];

    let mut active_tab: usize = TAB_SHELL;
    let mut input_parser = InputParser::new();
    let mut kmsg_offset: usize = 0; // Track how much of /proc/kmsg we've shown
    let mut frame: u32 = 0;
    let mut prev_mouse_buttons: u32 = 0; // For detecting click transitions
    let pane_x_offset = full_width / 2 + 4; // Right pane starts after divider

    // Initial render
    fb.clear(BG_COLOR);
    draw_tab_bar(&mut fb, &tabs, active_tab, pane_width);
    tabs[active_tab].emu.render(&mut fb, term_x_offset, term_y_offset);
    let _ = graphics::fb_flush();

    // Step 5: Main loop
    let mut read_buf = [0u8; 512];
    let mut needs_flush = false;

    // Pre-allocate poll fds (avoid Vec allocation per frame)
    let mut poll_fds = [
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 },
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 },
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 },
    ];

    loop {
        // Reset revents and set up poll fds
        let mut nfds = 1; // Always poll stdin (index 0)
        poll_fds[0].revents = 0;

        // Shell PTY master
        let shell_poll_idx = if let Some(fd) = tabs[TAB_SHELL].master_fd {
            poll_fds[nfds].fd = fd.raw() as i32;
            poll_fds[nfds].revents = 0;
            let idx = nfds;
            nfds += 1;
            Some(idx)
        } else {
            None
        };

        // Btop PTY master
        let btop_poll_idx = if let Some(fd) = tabs[TAB_BTOP].master_fd {
            poll_fds[nfds].fd = fd.raw() as i32;
            poll_fds[nfds].revents = 0;
            let idx = nfds;
            nfds += 1;
            Some(idx)
        } else {
            None
        };

        // Poll with 100ms timeout (for periodic /proc/kmsg reads)
        let _nready = io::poll(&mut poll_fds[..nfds], 100).unwrap_or(0);

        // Check stdin
        let stdin_had_data = poll_fds[0].revents & (io::poll_events::POLLIN as i16) != 0;
        if stdin_had_data {
            match io::read(Fd::from_raw(0), &mut read_buf) {
                Ok(n) if n > 0 => {
                    let mut char_buf = [0u8; 512];
                    let mut char_count = 0;
                    for i in 0..n {
                        match input_parser.feed(read_buf[i]) {
                            KeyEvent::Char(ch) => {
                                char_buf[char_count] = ch;
                                char_count += 1;
                            }
                            KeyEvent::EscSeq(buf, len) => {
                                // Flush accumulated chars first
                                if char_count > 0 {
                                    if let Some(fd) = tabs[active_tab].master_fd {
                                        let _ = io::write(fd, &char_buf[..char_count]);
                                    }
                                    char_count = 0;
                                }
                                if let Some(fd) = tabs[active_tab].master_fd {
                                    let _ = io::write(fd, &buf[..len]);
                                }
                            }
                            KeyEvent::F1 => {
                                if char_count > 0 {
                                    if let Some(fd) = tabs[active_tab].master_fd {
                                        let _ = io::write(fd, &char_buf[..char_count]);
                                    }
                                    char_count = 0;
                                }
                                if active_tab != TAB_SHELL {
                                    active_tab = TAB_SHELL;
                                    tabs[TAB_SHELL].has_unread = false;
                                    tabs[active_tab].emu.dirty = true;
                                    draw_tab_bar(&mut fb, &tabs, active_tab, pane_width);
                                    needs_flush = true;
                                }
                            }
                            KeyEvent::F2 => {
                                if char_count > 0 {
                                    if let Some(fd) = tabs[active_tab].master_fd {
                                        let _ = io::write(fd, &char_buf[..char_count]);
                                    }
                                    char_count = 0;
                                }
                                if active_tab != TAB_LOGS {
                                    active_tab = TAB_LOGS;
                                    tabs[TAB_LOGS].has_unread = false;
                                    tabs[active_tab].emu.dirty = true;
                                    draw_tab_bar(&mut fb, &tabs, active_tab, pane_width);
                                    needs_flush = true;
                                }
                            }
                            KeyEvent::F3 => {
                                if char_count > 0 {
                                    if let Some(fd) = tabs[active_tab].master_fd {
                                        let _ = io::write(fd, &char_buf[..char_count]);
                                    }
                                    char_count = 0;
                                }
                                if active_tab != TAB_BTOP {
                                    active_tab = TAB_BTOP;
                                    tabs[TAB_BTOP].has_unread = false;
                                    tabs[active_tab].emu.dirty = true;
                                    draw_tab_bar(&mut fb, &tabs, active_tab, pane_width);
                                    needs_flush = true;
                                }
                            }
                            KeyEvent::None => {}
                        }
                    }
                    // Flush remaining accumulated chars
                    if char_count > 0 {
                        if let Some(fd) = tabs[active_tab].master_fd {
                            let _ = io::write(fd, &char_buf[..char_count]);
                        }
                    }
                }
                _ => {}
            }
        }

        // Flush pending escape bytes only when stdin had no data (poll timeout)
        if !stdin_had_data {
            while let Some(byte) = input_parser.flush() {
                if let Some(fd) = tabs[active_tab].master_fd {
                    let _ = io::write(fd, &[byte]);
                }
            }
        }

        // Check Shell PTY master
        if let Some(idx) = shell_poll_idx {
            if poll_fds[idx].revents & (io::poll_events::POLLIN as i16) != 0 {
                if let Some(fd) = tabs[TAB_SHELL].master_fd {
                    match io::read(fd, &mut read_buf) {
                        Ok(n) if n > 0 => {
                            tabs[TAB_SHELL].emu.feed_bytes(&read_buf[..n]);
                            if active_tab != TAB_SHELL {
                                tabs[TAB_SHELL].has_unread = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Handle hangup: child exited, stop polling this fd
            if poll_fds[idx].revents & (io::poll_events::POLLHUP as i16) != 0 {
                if let Some(fd) = tabs[TAB_SHELL].master_fd.take() {
                    let _ = io::close(fd);
                }
            }
        }

        // Check Btop PTY master
        if let Some(idx) = btop_poll_idx {
            if poll_fds[idx].revents & (io::poll_events::POLLIN as i16) != 0 {
                if let Some(fd) = tabs[TAB_BTOP].master_fd {
                    match io::read(fd, &mut read_buf) {
                        Ok(n) if n > 0 => {
                            tabs[TAB_BTOP].emu.feed_bytes(&read_buf[..n]);
                            if active_tab != TAB_BTOP {
                                tabs[TAB_BTOP].has_unread = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Handle hangup: child exited, stop polling this fd
            if poll_fds[idx].revents & (io::poll_events::POLLHUP as i16) != 0 {
                if let Some(fd) = tabs[TAB_BTOP].master_fd.take() {
                    let _ = io::close(fd);
                }
            }
        }

        // Check for mouse clicks on tab bar
        if let Ok((mx, my, buttons)) = graphics::mouse_state() {
            // Detect rising edge (button just pressed)
            if buttons & 1 != 0 && prev_mouse_buttons & 1 == 0 {
                // Convert screen coords to pane-local coords
                if mx as usize >= pane_x_offset && (my as usize) < TAB_BAR_HEIGHT {
                    let local_x = mx as usize - pane_x_offset;
                    if let Some(clicked_tab) = hit_test_tab_bar(&tabs, local_x, pane_width) {
                        if clicked_tab != active_tab {
                            active_tab = clicked_tab;
                            tabs[active_tab].has_unread = false;
                            tabs[active_tab].emu.dirty = true;
                            draw_tab_bar(&mut fb, &tabs, active_tab, pane_width);
                            needs_flush = true;
                        }
                    }
                }
            }
            prev_mouse_buttons = buttons;
        }

        // Periodic: read /proc/kmsg for Logs tab (~every 10 frames = ~1 Hz)
        if frame % 10 == 0 {
            let kmsg = read_kmsg();
            if kmsg.len() > kmsg_offset {
                let new_data = &kmsg[kmsg_offset..];
                tabs[TAB_LOGS].emu.feed_bytes(new_data);
                kmsg_offset = kmsg.len();
                if active_tab != TAB_LOGS {
                    tabs[TAB_LOGS].has_unread = true;
                }
            }
        }

        // Reap dead children (non-blocking)
        let mut status: i32 = 0;
        loop {
            match waitpid(-1, &mut status as *mut i32, WNOHANG) {
                Ok(pid) if pid.raw() > 0 => {
                    let rpid = pid.raw() as i64;
                    // Check if a child died and respawn
                    if rpid == tabs[TAB_SHELL].child_pid {
                        print!("[bwm] Shell exited, respawning...\n");
                        // Close old master FD to release the PTY pair
                        if let Some(old_fd) = tabs[TAB_SHELL].master_fd.take() {
                            let _ = io::close(old_fd);
                        }
                        let (m, p) = spawn_child(b"/bin/bsh\0", "bsh");
                        tabs[TAB_SHELL].master_fd = Some(m);
                        tabs[TAB_SHELL].child_pid = p;
                    } else if rpid == tabs[TAB_BTOP].child_pid {
                        print!("[bwm] btop exited, respawning...\n");
                        // Close old master FD to release the PTY pair
                        if let Some(old_fd) = tabs[TAB_BTOP].master_fd.take() {
                            let _ = io::close(old_fd);
                        }
                        let (m, p) = spawn_child(b"/bin/btop\0", "btop");
                        tabs[TAB_BTOP].master_fd = Some(m);
                        tabs[TAB_BTOP].child_pid = p;
                    }
                }
                _ => break,
            }
        }

        // Render active tab (only if dirty)
        if tabs[active_tab].emu.dirty {
            tabs[active_tab].emu.render(&mut fb, term_x_offset, term_y_offset);
            needs_flush = true;
        }

        // Redraw tab bar periodically (for unread indicators)
        if frame % 10 == 0 {
            draw_tab_bar(&mut fb, &tabs, active_tab, pane_width);
            needs_flush = true;
        }

        // Only flush when something changed
        if needs_flush {
            let _ = graphics::fb_flush();
            needs_flush = false;
        }

        frame = frame.wrapping_add(1);
    }
}
