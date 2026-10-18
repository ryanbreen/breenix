//! Retained terminal cells and streaming UTF-8/control-function decoding.
//!
//! The contract is docs/console/contract.md section 5. Cells live in storage the
//! grid is given once (`set_storage`), not allocated while writing: kernel
//! records and TTY echo can arrive with IRQs masked. The alternate screen has
//! separate storage; full-screen scrolling rotates rows and partial-region
//! scrolling moves only those rows.
//!
//! Lines scrolled off the top of the main screen go into the scrollback
//! (section 1.6) once it is installed (`set_history`): a ring of whole rows.
//! The view is how many lines back from the live screen the VT is shown.

pub const MAX_COLS: usize = 255;
pub const MAX_ROWS: usize = 255;
const MAX_PARAMS: usize = 16;
/// Lines of scrollback per terminal (contract section 1.6).
pub const HISTORY_LINES: usize = 1000;

pub const BOLD: u8 = 1;
pub const UNDERLINE: u8 = 2;
pub const REVERSE: u8 = 4;
pub const INVISIBLE: u8 = 8;

/// One repertoire code point, palette pair and attribute flags (four bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Cell {
    pub glyph: u16,
    pub colours: u8,
    pub flags: u8,
}

pub const BLANK: Cell = Cell {
    glyph: b' ' as u16,
    colours: 7,
    flags: 0,
};
const ZERO: Cell = Cell {
    glyph: 0,
    colours: 0,
    flags: 0,
};

impl Cell {
    pub fn character(self) -> char {
        char::from_u32(u32::from(self.glyph)).unwrap_or('?')
    }

    /// Displayed palette pair, also the vcsa projection defined in section 3.
    pub fn displayed_colours(self) -> (usize, usize) {
        let (mut fg, mut bg) = (
            usize::from(self.colours & 15),
            usize::from(self.colours >> 4),
        );
        if self.flags & REVERSE != 0 {
            core::mem::swap(&mut fg, &mut bg);
        }
        if self.flags & INVISIBLE != 0 {
            fg = bg;
        }
        (fg, bg)
    }
}

#[derive(Clone, Copy)]
struct Cursor {
    row: usize,
    col: usize,
    pen: Cell,
    pending_wrap: bool,
}

impl Cursor {
    const ZERO: Self = Self {
        row: 0,
        col: 0,
        pen: ZERO,
        pending_wrap: false,
    };
    const NEW: Self = Self {
        row: 0,
        col: 0,
        pen: BLANK,
        pending_wrap: false,
    };
}

#[derive(Clone, Copy)]
struct State {
    cursor: Cursor,
    saved: Cursor,
    top: usize,
    first: usize,
    last: usize,
}

impl State {
    const ZERO: Self = Self {
        cursor: Cursor::ZERO,
        saved: Cursor::ZERO,
        top: 0,
        first: 0,
        last: 0,
    };
    const NEW: Self = Self {
        cursor: Cursor::NEW,
        saved: Cursor::NEW,
        top: 0,
        first: 0,
        last: 0,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Escape {
    Ground,
    Esc,
    EscIgnore,
    Csi,
    /// Consume unsupported control strings through ST; OSC also accepts BEL.
    String {
        osc: bool,
        esc: bool,
    },
}

pub struct Grid {
    cols: usize,
    rows: usize,
    /// The main and alternate screens, `cols` cells per row.
    cells: &'static mut [Cell],
    alternate: &'static mut [Cell],
    /// Scrollback: `HISTORY_LINES` rows of `cols` cells, the oldest at
    /// `history_start`, `history_len` of them filled.
    history: &'static mut [Cell],
    history_start: usize,
    history_len: usize,
    /// Lines back from the live screen that are shown (0: the live screen).
    view: usize,
    alt: bool,
    state: State,
    main: State,
    visible: bool,
    autowrap: bool,
    tabs: [bool; MAX_COLS],
    escape: Escape,
    params: [u16; MAX_PARAMS],
    param_index: usize,
    private: bool,
    ignored: bool,
    utf_value: u32,
    utf_min: u32,
    utf_left: u8,
    generation: u64,
}

impl Grid {
    pub const fn new() -> Self {
        Self {
            cols: 0,
            rows: 0,
            // set_storage provides the cells before the grid has a size.
            cells: &mut [],
            alternate: &mut [],
            history: &mut [],
            history_start: 0,
            history_len: 0,
            view: 0,
            alt: false,
            state: State::ZERO,
            main: State::ZERO,
            visible: false,
            autowrap: false,
            tabs: [false; MAX_COLS],
            escape: Escape::Ground,
            params: [0; MAX_PARAMS],
            param_index: 0,
            private: false,
            ignored: false,
            utf_value: 0,
            utf_min: 0,
            utf_left: 0,
            generation: 0,
        }
    }

    /// Give the grid its screens' cells. Each must hold the size it is then
    /// given (`set_size`); a size they cannot hold leaves the grid empty.
    pub fn set_storage(&mut self, cells: &'static mut [Cell], alternate: &'static mut [Cell]) {
        self.cells = cells;
        self.alternate = alternate;
    }

    pub fn set_size(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.min(MAX_COLS), rows.min(MAX_ROWS));
        if cols * rows > self.cells.len().min(self.alternate.len()) {
            self.cols = 0;
            self.rows = 0;
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.history_start = 0;
        self.history_len = 0;
        self.view = 0;
        self.reset();
    }

    /// Install the scrollback: `HISTORY_LINES` rows of the grid's width.
    pub fn set_history(&mut self, history: &'static mut [Cell]) {
        if history.len() >= HISTORY_LINES * self.cols && self.cols != 0 {
            self.history = history;
            self.history_start = 0;
            self.history_len = 0;
        }
    }

    pub fn has_history(&self) -> bool {
        !self.history.is_empty()
    }

    fn reset(&mut self) {
        self.alt = false;
        self.state = State {
            last: self.rows.saturating_sub(1),
            ..State::NEW
        };
        self.main = self.state;
        let size = self.cols * self.rows;
        self.cells[..size].fill(BLANK);
        self.alternate[..size].fill(BLANK);
        self.visible = true;
        self.autowrap = true;
        for (col, tab) in self.tabs.iter_mut().enumerate() {
            *tab = col > 0 && col % 8 == 0;
        }
        self.escape = Escape::Ground;
        self.utf_left = 0;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cursor(&self) -> (usize, usize) {
        (self.state.cursor.row, self.state.cursor.col)
    }
    pub fn cursor_visible(&self) -> bool {
        self.visible
    }

    /// A row of the live screen.
    pub fn row(&self, row: usize) -> &[Cell] {
        let start = ((self.state.top + row) % self.rows.max(1)) * self.cols;
        let cells: &[Cell] = if self.alt { self.alternate } else { self.cells };
        &cells[start..start + self.cols]
    }

    fn row_mut(&mut self, row: usize) -> &mut [Cell] {
        let start = ((self.state.top + row) % self.rows.max(1)) * self.cols;
        let cells: &mut [Cell] = if self.alt {
            self.alternate
        } else {
            self.cells
        };
        &mut cells[start..start + self.cols]
    }

    /// A row as the view shows it: from the scrollback while the view is
    /// scrolled back, else from the live screen.
    pub fn view_row(&self, row: usize) -> &[Cell] {
        let back = self.view();
        if back == 0 {
            return self.row(row);
        }
        let line = self.history_len - back + row;
        if line < self.history_len {
            let start = ((self.history_start + line) % HISTORY_LINES) * self.cols;
            &self.history[start..start + self.cols]
        } else {
            self.row(line - self.history_len)
        }
    }

    /// Lines back from the live screen the view shows; the alternate screen
    /// has no scrollback, so it is shown live.
    pub fn view(&self) -> usize {
        if self.alt { 0 } else { self.view }
    }

    /// Move the view `lines` back (positive) or forward (negative), within
    /// the scrollback; true when it moved. No effect on the alternate screen.
    pub fn scroll_view(&mut self, lines: isize) -> bool {
        if self.alt {
            return false;
        }
        let view = if lines >= 0 {
            self.view.saturating_add(lines as usize).min(self.history_len)
        } else {
            self.view.saturating_sub(lines.unsigned_abs())
        };
        if view == self.view {
            return false;
        }
        self.view = view;
        self.generation = self.generation.wrapping_add(1);
        true
    }

    /// Return the view to the live screen; true when it moved.
    pub fn view_live(&mut self) -> bool {
        self.scroll_view(-(self.view as isize))
    }

    /// Keep rows `first..first + count` of the live screen, which are about
    /// to scroll off its top, in the scrollback. A scrolled-back view keeps
    /// showing the same lines.
    fn keep_history(&mut self, first: usize, count: usize) {
        if self.history.is_empty() || self.alt {
            return;
        }
        for row in first..first + count {
            let slot = (self.history_start + self.history_len) % HISTORY_LINES;
            let (cols, start) = (self.cols, ((self.state.top + row) % self.rows) * self.cols);
            self.history[slot * cols..(slot + 1) * cols].copy_from_slice(&self.cells[start..start + cols]);
            if self.history_len == HISTORY_LINES {
                self.history_start = (self.history_start + 1) % HISTORY_LINES;
            } else {
                self.history_len += 1;
            }
            if self.view != 0 {
                self.view = (self.view + 1).min(self.history_len);
            }
        }
    }

    pub fn begin_line(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        if self.state.cursor.col != 0 || self.state.cursor.pending_wrap {
            self.state.cursor.col = 0;
            self.state.cursor.pending_wrap = false;
            self.index();
        }
        self.escape = Escape::Ground;
        self.utf_left = 0;
    }

    /// Kernel records select the main screen and bypass terminal control functions.
    /// TTY parser state and drawing modes survive; records use the default pen
    /// and the whole screen so an application's SGR/scroll region cannot hide them.
    pub fn write_kernel(&mut self, bytes: &[u8], begin: bool) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        self.alternate_screen(false);
        let (pen, first, last, autowrap) = (
            self.state.cursor.pen,
            self.state.first,
            self.state.last,
            self.autowrap,
        );
        let parser = (self.escape, self.utf_value, self.utf_min, self.utf_left);
        self.state.cursor.pen = BLANK;
        self.state.first = 0;
        self.state.last = self.rows - 1;
        self.autowrap = true;
        self.escape = Escape::Ground;
        self.utf_left = 0;
        if begin {
            self.begin_line();
        }
        for &byte in bytes {
            match byte {
                b'\n' => {
                    self.utf_left = 0;
                    self.c0(b'\r');
                    self.c0(b'\n');
                }
                0x20..=0x7e | 0x80..=0xff => self.byte(byte, &mut |_| {}),
                _ => {}
            }
        }
        self.state.cursor.pen = pen;
        self.state.first = first;
        self.state.last = last;
        self.autowrap = autowrap;
        (self.escape, self.utf_value, self.utf_min, self.utf_left) = parser;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Write terminal output without collecting terminal-input replies.
    pub fn write(&mut self, bytes: &[u8]) {
        self.write_with_reply(bytes, |_| {});
    }

    /// TTY output supplies its input sink for DSR replies, without echo.
    pub fn write_with_reply(&mut self, bytes: &[u8], mut reply: impl FnMut(&[u8])) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        for &byte in bytes {
            self.byte(byte, &mut reply);
        }
        self.generation = self.generation.wrapping_add(1);
    }

    fn byte(&mut self, byte: u8, reply: &mut impl FnMut(&[u8])) {
        if let Escape::String { osc, esc } = self.escape {
            if byte == 0x18 || byte == 0x1a || (osc && byte == 7) || (esc && byte == b'\\') {
                self.escape = Escape::Ground;
            } else {
                self.escape = Escape::String {
                    osc,
                    esc: byte == 0x1b,
                };
            }
            return;
        }
        // An incomplete scalar is replaced before reconsidering a non-continuation
        // byte (including ESC). A split write retains the decoder's state.
        if self.escape == Escape::Ground && self.utf_left != 0 {
            if byte & 0xc0 == 0x80 {
                self.utf_value = (self.utf_value << 6) | u32::from(byte & 63);
                self.utf_left -= 1;
                if self.utf_left == 0 {
                    let value = self.utf_value;
                    self.put(
                        if value >= self.utf_min
                            && value <= 0x10ffff
                            && !(0xd800..=0xdfff).contains(&value)
                        {
                            value
                        } else {
                            0x3f
                        },
                    );
                }
                return;
            }
            self.utf_left = 0;
            self.put(0x3f);
        }
        if byte == 0x18 || byte == 0x1a {
            self.escape = Escape::Ground;
            return;
        }
        if byte == 0x1b {
            self.escape = Escape::Esc;
            return;
        }
        if byte < 0x20 {
            self.c0(byte);
            return;
        }
        match self.escape {
            Escape::Ground => match byte {
                0x20..=0x7e => self.put(u32::from(byte)),
                0xc2..=0xdf => {
                    self.utf_value = u32::from(byte & 31);
                    self.utf_min = 0x80;
                    self.utf_left = 1;
                }
                0xe0..=0xef => {
                    self.utf_value = u32::from(byte & 15);
                    self.utf_min = 0x800;
                    self.utf_left = 2;
                }
                0xf0..=0xf4 => {
                    self.utf_value = u32::from(byte & 7);
                    self.utf_min = 0x10000;
                    self.utf_left = 3;
                }
                0x80..=0xff => self.put(0x3f),
                _ => {}
            },
            Escape::Esc => {
                self.escape = Escape::Ground;
                match byte {
                    b'[' => {
                        self.escape = Escape::Csi;
                        self.params.fill(0);
                        self.param_index = 0;
                        self.private = false;
                        self.ignored = false;
                    }
                    b']' | b'P' | b'X' | b'^' | b'_' => {
                        self.escape = Escape::String {
                            osc: byte == b']',
                            esc: false,
                        }
                    }
                    0x20..=0x2f => self.escape = Escape::EscIgnore,
                    b'c' => self.reset(),
                    b'7' => self.state.saved = self.state.cursor,
                    b'8' => {
                        self.state.cursor = self.state.saved;
                        self.state.cursor.pending_wrap = false;
                    }
                    b'D' => {
                        self.state.cursor.pending_wrap = false;
                        self.index();
                    }
                    b'E' => {
                        self.state.cursor.col = 0;
                        self.state.cursor.pending_wrap = false;
                        self.index();
                    }
                    b'M' => {
                        self.state.cursor.pending_wrap = false;
                        self.reverse_index();
                    }
                    b'H' => self.tabs[self.state.cursor.col] = true,
                    _ => {}
                }
            }
            Escape::EscIgnore => {
                if (0x30..=0x7e).contains(&byte) {
                    self.escape = Escape::Ground;
                }
            }
            Escape::Csi => match byte {
                b'0'..=b'9' => {
                    if self.param_index < MAX_PARAMS {
                        let p = &mut self.params[self.param_index];
                        *p = p.saturating_mul(10).saturating_add(u16::from(byte - b'0'));
                    }
                }
                b';' => self.param_index = (self.param_index + 1).min(MAX_PARAMS),
                b'?' if self.param_index == 0 && self.params[0] == 0 && !self.private => {
                    self.private = true
                }
                0x20..=0x3f => self.ignored = true,
                0x40..=0x7e => {
                    self.escape = Escape::Ground;
                    if !self.ignored {
                        self.control(byte, reply);
                    }
                }
                _ => {}
            },
            Escape::String { .. } => {}
        }
    }

    fn c0(&mut self, byte: u8) {
        match byte {
            8 => {
                self.state.cursor.pending_wrap = false;
                self.state.cursor.col = self.state.cursor.col.saturating_sub(1);
            }
            9 => {
                self.state.cursor.pending_wrap = false;
                self.state.cursor.col = (self.state.cursor.col + 1..self.cols)
                    .find(|&col| self.tabs[col])
                    .unwrap_or(self.cols - 1);
            }
            10..=12 => {
                self.state.cursor.pending_wrap = false;
                self.index();
            }
            13 => {
                self.state.cursor.pending_wrap = false;
                self.state.cursor.col = 0;
            }
            _ => {}
        }
    }

    fn put(&mut self, value: u32) {
        if self.state.cursor.pending_wrap {
            self.state.cursor.pending_wrap = false;
            self.state.cursor.col = 0;
            self.index();
        }
        let glyph = match value {
            0x20..=0x7e | 0xa0..=0x17f | 0x2500..=0x257f => value as u16,
            _ => b'?' as u16,
        };
        let cursor = self.state.cursor;
        self.row_mut(cursor.row)[cursor.col] = Cell {
            glyph,
            ..cursor.pen
        };
        if cursor.col + 1 == self.cols {
            self.state.cursor.pending_wrap = self.autowrap;
        } else {
            self.state.cursor.col += 1;
        }
    }

    fn blank(&self) -> Cell {
        Cell {
            colours: self.state.cursor.pen.colours,
            ..BLANK
        }
    }

    fn index(&mut self) {
        let row = self.state.cursor.row;
        if row == self.state.last {
            self.scroll(self.state.first, self.state.last, 1, false);
        } else {
            self.state.cursor.row = (row + 1).min(self.rows - 1);
        }
    }

    fn reverse_index(&mut self) {
        if self.state.cursor.row == self.state.first {
            self.scroll(self.state.first, self.state.last, 1, true);
        } else {
            self.state.cursor.row = self.state.cursor.row.saturating_sub(1);
        }
    }

    fn scroll(&mut self, first: usize, last: usize, count: usize, down: bool) {
        let count = count.min(last - first + 1);
        if first == 0 && !down {
            self.keep_history(first, count);
        }
        let blank = self.blank();
        if first == 0 && last + 1 == self.rows {
            self.state.top =
                (self.state.top + if down { self.rows - count } else { count }) % self.rows;
        } else if down {
            for row in (first + count..=last).rev() {
                self.copy_row(row - count, row);
            }
        } else {
            for row in first..last + 1 - count {
                self.copy_row(row + count, row);
            }
        }
        let erased = if down {
            first..first + count
        } else {
            last + 1 - count..last + 1
        };
        for row in erased {
            self.row_mut(row).fill(blank);
        }
    }

    fn copy_row(&mut self, from: usize, to: usize) {
        let mut row = [BLANK; MAX_COLS];
        let cols = self.cols;
        row[..cols].copy_from_slice(self.row(from));
        self.row_mut(to).copy_from_slice(&row[..cols]);
    }

    fn param(&self, index: usize, default: usize) -> usize {
        match self.params[index] {
            0 => default,
            n => usize::from(n),
        }
    }

    fn erase(&mut self, row: usize, from: usize, to: usize) {
        let blank = self.blank();
        self.row_mut(row)[from..to].fill(blank);
    }

    fn control(&mut self, command: u8, reply: &mut impl FnMut(&[u8])) {
        if self.private {
            if command == b'h' || command == b'l' {
                let on = command == b'h';
                for index in 0..(self.param_index + 1).min(MAX_PARAMS) {
                    match self.params[index] {
                        7 => {
                            self.autowrap = on;
                            if !on {
                                self.state.cursor.pending_wrap = false;
                            }
                        }
                        25 => self.visible = on,
                        1049 => self.alternate_screen(on),
                        _ => {}
                    }
                }
            }
            return;
        }
        let (row, col) = self.cursor();
        let n = self.param(0, 1);
        // SGR, reports, tab-stop changes and unknown functions do not move the
        // cursor and must not discard a pending wrap.
        if matches!(command, b'A'..=b'H' | b'f' | b'd' | b'J'..=b'M' | b'@' | b'P' | b'X' | b'S' | b'T' | b'r' | b'u')
        {
            self.state.cursor.pending_wrap = false;
        }
        match command {
            b'A' => self.state.cursor.row = row.saturating_sub(n),
            b'B' => self.state.cursor.row = (row + n).min(self.rows - 1),
            b'C' => self.state.cursor.col = (col + n).min(self.cols - 1),
            b'D' => self.state.cursor.col = col.saturating_sub(n),
            b'E' => {
                self.state.cursor.row = (row + n).min(self.rows - 1);
                self.state.cursor.col = 0;
            }
            b'F' => {
                self.state.cursor.row = row.saturating_sub(n);
                self.state.cursor.col = 0;
            }
            b'G' => self.state.cursor.col = (n - 1).min(self.cols - 1),
            b'd' => self.state.cursor.row = (n - 1).min(self.rows - 1),
            b'H' | b'f' => {
                self.state.cursor.row = (n - 1).min(self.rows - 1);
                self.state.cursor.col = (self.param(1, 1) - 1).min(self.cols - 1);
            }
            b'J' => match self.params[0] {
                0 => {
                    self.erase(row, col, self.cols);
                    for r in row + 1..self.rows {
                        self.erase(r, 0, self.cols);
                    }
                }
                1 => {
                    for r in 0..row {
                        self.erase(r, 0, self.cols);
                    }
                    self.erase(row, 0, col + 1);
                }
                2 => {
                    for r in 0..self.rows {
                        self.erase(r, 0, self.cols);
                    }
                }
                _ => {}
            },
            b'K' => match self.params[0] {
                0 => self.erase(row, col, self.cols),
                1 => self.erase(row, 0, col + 1),
                2 => self.erase(row, 0, self.cols),
                _ => {}
            },
            b'L' | b'M' if row >= self.state.first && row <= self.state.last => {
                self.scroll(row, self.state.last, n, command == b'L')
            }
            b'S' | b'T' => self.scroll(self.state.first, self.state.last, n, command == b'T'),
            b'@' | b'P' => {
                let count = n.min(self.cols - col);
                let cols = self.cols;
                let blank = self.blank();
                let cells = self.row_mut(row);
                if command == b'@' {
                    cells.copy_within(col..cols - count, col + count);
                    cells[col..col + count].fill(blank);
                } else {
                    cells.copy_within(col + count..cols, col);
                    cells[cols - count..cols].fill(blank);
                }
            }
            b'X' => self.erase(row, col, (col + n).min(self.cols)),
            b'g' => match self.params[0] {
                0 => self.tabs[col] = false,
                3 => self.tabs.fill(false),
                _ => {}
            },
            b'r' => {
                let first = n - 1;
                let last = self.param(1, self.rows) - 1;
                if first < last && last < self.rows {
                    self.state.first = first;
                    self.state.last = last;
                    self.state.cursor.row = 0;
                    self.state.cursor.col = 0;
                }
            }
            b's' => {
                self.state.saved.row = row;
                self.state.saved.col = col;
            }
            b'u' => {
                self.state.cursor.row = self.state.saved.row;
                self.state.cursor.col = self.state.saved.col;
            }
            b'n' => match self.params[0] {
                5 => reply(b"\x1b[0n"),
                6 => {
                    let mut bytes = [0; 16];
                    bytes[..2].copy_from_slice(b"\x1b[");
                    let mut len = 2;
                    for (number, end) in [(row + 1, b';'), (col + 1, b'R')] {
                        for divisor in [100, 10, 1] {
                            if number >= divisor || divisor == 1 {
                                bytes[len] = b'0' + (number / divisor % 10) as u8;
                                len += 1;
                            }
                        }
                        bytes[len] = end;
                        len += 1;
                    }
                    reply(&bytes[..len]);
                }
                _ => {}
            },
            b'm' => self.sgr(),
            _ => {}
        }
    }

    fn alternate_screen(&mut self, on: bool) {
        if self.alt == on {
            return;
        }
        if on {
            self.main = self.state;
            self.state = State {
                last: self.rows - 1,
                ..State::NEW
            };
            self.state.cursor.pen = self.main.cursor.pen;
            let size = self.cols * self.rows;
            self.alternate[..size].fill(BLANK);
        } else {
            self.state = self.main;
            self.state.cursor.pending_wrap &= self.autowrap;
        }
        self.alt = on;
    }

    fn sgr(&mut self) {
        let count = (self.param_index + 1).min(MAX_PARAMS);
        let mut i = 0;
        while i < count {
            let parameter = self.params[i];
            let pen = &mut self.state.cursor.pen;
            match parameter {
                0 => *pen = BLANK,
                1 => pen.flags |= BOLD,
                22 => pen.flags &= !BOLD,
                4 => pen.flags |= UNDERLINE,
                24 => pen.flags &= !UNDERLINE,
                7 => pen.flags |= REVERSE,
                27 => pen.flags &= !REVERSE,
                8 => pen.flags |= INVISIBLE,
                28 => pen.flags &= !INVISIBLE,
                30..=37 => pen.colours = (pen.colours & 0xf0) | (parameter - 30) as u8,
                90..=97 => pen.colours = (pen.colours & 0xf0) | (parameter - 90 + 8) as u8,
                40..=47 => pen.colours = (pen.colours & 15) | ((parameter - 40) as u8) << 4,
                100..=107 => pen.colours = (pen.colours & 15) | ((parameter - 100 + 8) as u8) << 4,
                39 => pen.colours = (pen.colours & 0xf0) | 7,
                49 => pen.colours &= 15,
                38 | 48 => {
                    let (length, colour) = match self.params.get(i + 1).filter(|_| i + 1 < count) {
                        Some(5) if i + 2 < count => (3, indexed_colour(self.params[i + 2])),
                        Some(2) if i + 4 < count => {
                            let rgb = [self.params[i + 2], self.params[i + 3], self.params[i + 4]];
                            (
                                5,
                                if rgb.iter().all(|&v| v <= 255) {
                                    Some(super::raster::nearest_colour(rgb.map(|v| v as u8)))
                                } else {
                                    None
                                },
                            )
                        }
                        _ => {
                            break;
                        }
                    };
                    if let Some(colour) = colour {
                        if parameter == 38 {
                            pen.colours = (pen.colours & 0xf0) | colour;
                        } else {
                            pen.colours = (pen.colours & 15) | colour << 4;
                        }
                    }
                    i += length;
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn indexed_colour(index: u16) -> Option<u8> {
    if index < 16 {
        return Some(index as u8);
    }
    let rgb = match index {
        16..=231 => {
            let n = index - 16;
            [n / 36, (n / 6) % 6, n % 6].map(|v| if v == 0 { 0 } else { (55 + 40 * v) as u8 })
        }
        232..=255 => [8 + 10 * (index - 232) as u8; 3],
        _ => return None,
    };
    Some(super::raster::nearest_colour(rgb))
}
