//! The screen console's retained character grid.
//!
//! The grid is what the console shows, as text: one byte per cell, rows in a
//! ring so a scroll moves no text, the cursor, and a generation that changes
//! with each write. Kernel log lines and the console TTY's output are written
//! here; the render thread draws the screen from snapshots of it, and the
//! read-back check compares the screen against glyphs drawn from it.
//!
//! Output is interpreted as a terminal does it: CR returns to the first column,
//! LF moves down a row (scrolling at the bottom), BS moves left, HT moves to the
//! next multiple of eight, and a character written in the last column wraps on
//! the next one. CSI sequences move the cursor (`A` `B` `C` `D` `G` `H` `d` `f`)
//! and erase (`J`, `K`); other sequences, and SGR attributes, are consumed
//! without effect. A UTF-8 character takes one cell and is shown as `?`, which
//! is what the console font draws for characters it does not have.

/// The largest grid kept: 2048 x 2304 pixels in the console font.
pub const MAX_COLS: usize = 256;
pub const MAX_ROWS: usize = 128;

/// The blank cell.
pub const BLANK: u8 = b' ';

const MAX_PARAMS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    Esc,
    /// In a control sequence; `private` is set by a `?` and similar prefixes.
    Csi { private: bool },
}

pub struct Grid {
    cols: usize,
    rows: usize,
    /// Row `r` of the screen is ring row `(top + r) % rows`.
    top: usize,
    cells: [u8; MAX_COLS * MAX_ROWS],
    row: usize,
    col: usize,
    /// A character was written in the last column: the next one starts a row.
    pending_wrap: bool,
    escape: Escape,
    params: [u16; MAX_PARAMS],
    /// The parameter the next digit belongs to.
    param_index: usize,
    generation: u64,
}

impl Grid {
    pub const fn new() -> Self {
        Self {
            cols: 0,
            rows: 0,
            top: 0,
            cells: [BLANK; MAX_COLS * MAX_ROWS],
            row: 0,
            col: 0,
            pending_wrap: false,
            escape: Escape::None,
            params: [0; MAX_PARAMS],
            param_index: 0,
            generation: 0,
        }
    }

    /// Size the grid, at most `MAX_COLS` x `MAX_ROWS`, and clear it.
    pub fn set_size(&mut self, cols: usize, rows: usize) {
        self.cols = cols.min(MAX_COLS);
        self.rows = rows.min(MAX_ROWS);
        self.top = 0;
        self.cells.fill(BLANK);
        self.row = 0;
        self.col = 0;
        self.pending_wrap = false;
        self.escape = Escape::None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Changes with each write that can change what the grid shows.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The cursor as (row, column).
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Screen row `row`, `cols` bytes.
    pub fn row(&self, row: usize) -> &[u8] {
        let start = ((self.top + row) % self.rows.max(1)) * MAX_COLS;
        &self.cells[start..start + self.cols]
    }

    fn row_mut(&mut self, row: usize) -> &mut [u8] {
        let start = ((self.top + row) % self.rows.max(1)) * MAX_COLS;
        let cols = self.cols;
        &mut self.cells[start..start + cols]
    }

    /// Start a fresh row unless the cursor is already at the start of one, so
    /// a kernel log line does not land inside a line of TTY output.
    pub fn begin_line(&mut self) {
        if self.col != 0 || self.pending_wrap {
            self.carriage_return();
            self.line_feed();
        }
        self.escape = Escape::None;
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        for &byte in bytes {
            self.byte(byte);
        }
        self.generation = self.generation.wrapping_add(1);
    }

    fn byte(&mut self, byte: u8) {
        match self.escape {
            Escape::None => self.plain(byte),
            Escape::Esc => {
                self.escape = if byte == b'[' {
                    self.params = [0; MAX_PARAMS];
                    self.param_index = 0;
                    Escape::Csi { private: false }
                } else {
                    Escape::None
                };
            }
            Escape::Csi { private } => match byte {
                b'0'..=b'9' => {
                    let index = self.param_index;
                    self.params[index] = self.params[index]
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                }
                b';' => self.param_index = (self.param_index + 1).min(MAX_PARAMS - 1),
                b'<'..=b'?' => self.escape = Escape::Csi { private: true },
                0x20..=0x2f => {}
                0x40..=0x7e => {
                    self.escape = Escape::None;
                    if !private {
                        self.control(byte);
                    }
                }
                _ => self.escape = Escape::None,
            },
        }
    }

    fn plain(&mut self, byte: u8) {
        match byte {
            b'\r' => self.carriage_return(),
            b'\n' | 0x0b | 0x0c => {
                self.pending_wrap = false;
                self.line_feed();
            }
            0x08 => {
                self.pending_wrap = false;
                self.col = self.col.saturating_sub(1);
            }
            b'\t' => {
                self.pending_wrap = false;
                self.col = ((self.col / 8 + 1) * 8).min(self.cols - 1);
            }
            0x1b => self.escape = Escape::Esc,
            0x20..=0x7e => self.put(byte),
            // A UTF-8 character: its first byte takes the cell.
            0xc0..=0xff => self.put(b'?'),
            // Other control characters, DEL and UTF-8 continuation bytes.
            _ => {}
        }
    }

    fn put(&mut self, byte: u8) {
        if self.pending_wrap {
            self.carriage_return();
            self.line_feed();
        }
        let col = self.col;
        self.row_mut(self.row)[col] = byte;
        if self.col + 1 == self.cols {
            self.pending_wrap = true;
        } else {
            self.col += 1;
        }
    }

    fn carriage_return(&mut self) {
        self.col = 0;
        self.pending_wrap = false;
    }

    fn line_feed(&mut self) {
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            self.top = (self.top + 1) % self.rows;
            self.row_mut(self.rows - 1).fill(BLANK);
        }
    }

    /// Parameter `index`, or `default` when it is absent or 0.
    fn param(&self, index: usize, default: usize) -> usize {
        match self.params[index] {
            0 => default,
            value => usize::from(value),
        }
    }

    fn control(&mut self, command: u8) {
        self.pending_wrap = false;
        let last_row = self.rows - 1;
        let last_col = self.cols - 1;
        match command {
            b'A' => self.row = self.row.saturating_sub(self.param(0, 1)),
            b'B' => self.row = (self.row + self.param(0, 1)).min(last_row),
            b'C' => self.col = (self.col + self.param(0, 1)).min(last_col),
            b'D' => self.col = self.col.saturating_sub(self.param(0, 1)),
            b'G' => self.col = (self.param(0, 1) - 1).min(last_col),
            b'd' => self.row = (self.param(0, 1) - 1).min(last_row),
            b'H' | b'f' => {
                self.row = (self.param(0, 1) - 1).min(last_row);
                self.col = (self.param(1, 1) - 1).min(last_col);
            }
            b'J' => match self.params[0] {
                0 => {
                    self.erase_in_row(self.row, self.col, self.cols);
                    for row in self.row + 1..self.rows {
                        self.row_mut(row).fill(BLANK);
                    }
                }
                1 => {
                    for row in 0..self.row {
                        self.row_mut(row).fill(BLANK);
                    }
                    self.erase_in_row(self.row, 0, self.col + 1);
                }
                2 | 3 => {
                    for row in 0..self.rows {
                        self.row_mut(row).fill(BLANK);
                    }
                }
                _ => {}
            },
            b'K' => match self.params[0] {
                0 => self.erase_in_row(self.row, self.col, self.cols),
                1 => self.erase_in_row(self.row, 0, self.col + 1),
                2 => self.erase_in_row(self.row, 0, self.cols),
                _ => {}
            },
            _ => {}
        }
    }

    fn erase_in_row(&mut self, row: usize, from: usize, to: usize) {
        let to = to.min(self.cols);
        if from < to {
            self.row_mut(row)[from..to].fill(BLANK);
        }
    }
}
