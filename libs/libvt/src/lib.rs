#![no_std]

extern crate alloc;
use alloc::{format, string::String, vec, vec::Vec, collections::VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color { pub r: u8, pub g: u8, pub b: u8 }
impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self { Self { r, g, b } }
}

/// Reserved for the scrollback implementation in a later unit.
#[derive(Default)]
pub struct Scrollback { pub lines: VecDeque<Vec<Cell>> }

// Terminal colors
pub const BG_COLOR: Color = Color::rgb(30, 30, 40);
pub const FG_COLOR: Color = Color::rgb(204, 204, 204);

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

// Unicode Mn and Me ranges from vendor/unicode/UnicodeData.txt.
const COMBINING: &[(u32, u32)] = &[
    (0x300, 0x36f),
    (0x483, 0x489),
    (0x591, 0x5bd),
    (0x5bf, 0x5bf),
    (0x5c1, 0x5c2),
    (0x5c4, 0x5c5),
    (0x5c7, 0x5c7),
    (0x610, 0x61a),
    (0x64b, 0x65f),
    (0x670, 0x670),
    (0x6d6, 0x6dc),
    (0x6df, 0x6e4),
    (0x6e7, 0x6e8),
    (0x6ea, 0x6ed),
    (0x711, 0x711),
    (0x730, 0x74a),
    (0x7a6, 0x7b0),
    (0x7eb, 0x7f3),
    (0x7fd, 0x7fd),
    (0x816, 0x819),
    (0x81b, 0x823),
    (0x825, 0x827),
    (0x829, 0x82d),
    (0x859, 0x85b),
    (0x897, 0x89f),
    (0x8ca, 0x8e1),
    (0x8e3, 0x902),
    (0x93a, 0x93a),
    (0x93c, 0x93c),
    (0x941, 0x948),
    (0x94d, 0x94d),
    (0x951, 0x957),
    (0x962, 0x963),
    (0x981, 0x981),
    (0x9bc, 0x9bc),
    (0x9c1, 0x9c4),
    (0x9cd, 0x9cd),
    (0x9e2, 0x9e3),
    (0x9fe, 0x9fe),
    (0xa01, 0xa02),
    (0xa3c, 0xa3c),
    (0xa41, 0xa42),
    (0xa47, 0xa48),
    (0xa4b, 0xa4d),
    (0xa51, 0xa51),
    (0xa70, 0xa71),
    (0xa75, 0xa75),
    (0xa81, 0xa82),
    (0xabc, 0xabc),
    (0xac1, 0xac5),
    (0xac7, 0xac8),
    (0xacd, 0xacd),
    (0xae2, 0xae3),
    (0xafa, 0xaff),
    (0xb01, 0xb01),
    (0xb3c, 0xb3c),
    (0xb3f, 0xb3f),
    (0xb41, 0xb44),
    (0xb4d, 0xb4d),
    (0xb55, 0xb56),
    (0xb62, 0xb63),
    (0xb82, 0xb82),
    (0xbc0, 0xbc0),
    (0xbcd, 0xbcd),
    (0xc00, 0xc00),
    (0xc04, 0xc04),
    (0xc3c, 0xc3c),
    (0xc3e, 0xc40),
    (0xc46, 0xc48),
    (0xc4a, 0xc4d),
    (0xc55, 0xc56),
    (0xc62, 0xc63),
    (0xc81, 0xc81),
    (0xcbc, 0xcbc),
    (0xcbf, 0xcbf),
    (0xcc6, 0xcc6),
    (0xccc, 0xccd),
    (0xce2, 0xce3),
    (0xd00, 0xd01),
    (0xd3b, 0xd3c),
    (0xd41, 0xd44),
    (0xd4d, 0xd4d),
    (0xd62, 0xd63),
    (0xd81, 0xd81),
    (0xdca, 0xdca),
    (0xdd2, 0xdd4),
    (0xdd6, 0xdd6),
    (0xe31, 0xe31),
    (0xe34, 0xe3a),
    (0xe47, 0xe4e),
    (0xeb1, 0xeb1),
    (0xeb4, 0xebc),
    (0xec8, 0xece),
    (0xf18, 0xf19),
    (0xf35, 0xf35),
    (0xf37, 0xf37),
    (0xf39, 0xf39),
    (0xf71, 0xf7e),
    (0xf80, 0xf84),
    (0xf86, 0xf87),
    (0xf8d, 0xf97),
    (0xf99, 0xfbc),
    (0xfc6, 0xfc6),
    (0x102d, 0x1030),
    (0x1032, 0x1037),
    (0x1039, 0x103a),
    (0x103d, 0x103e),
    (0x1058, 0x1059),
    (0x105e, 0x1060),
    (0x1071, 0x1074),
    (0x1082, 0x1082),
    (0x1085, 0x1086),
    (0x108d, 0x108d),
    (0x109d, 0x109d),
    (0x135d, 0x135f),
    (0x1712, 0x1714),
    (0x1732, 0x1733),
    (0x1752, 0x1753),
    (0x1772, 0x1773),
    (0x17b4, 0x17b5),
    (0x17b7, 0x17bd),
    (0x17c6, 0x17c6),
    (0x17c9, 0x17d3),
    (0x17dd, 0x17dd),
    (0x180b, 0x180d),
    (0x180f, 0x180f),
    (0x1885, 0x1886),
    (0x18a9, 0x18a9),
    (0x1920, 0x1922),
    (0x1927, 0x1928),
    (0x1932, 0x1932),
    (0x1939, 0x193b),
    (0x1a17, 0x1a18),
    (0x1a1b, 0x1a1b),
    (0x1a56, 0x1a56),
    (0x1a58, 0x1a5e),
    (0x1a60, 0x1a60),
    (0x1a62, 0x1a62),
    (0x1a65, 0x1a6c),
    (0x1a73, 0x1a7c),
    (0x1a7f, 0x1a7f),
    (0x1ab0, 0x1add),
    (0x1ae0, 0x1aeb),
    (0x1b00, 0x1b03),
    (0x1b34, 0x1b34),
    (0x1b36, 0x1b3a),
    (0x1b3c, 0x1b3c),
    (0x1b42, 0x1b42),
    (0x1b6b, 0x1b73),
    (0x1b80, 0x1b81),
    (0x1ba2, 0x1ba5),
    (0x1ba8, 0x1ba9),
    (0x1bab, 0x1bad),
    (0x1be6, 0x1be6),
    (0x1be8, 0x1be9),
    (0x1bed, 0x1bed),
    (0x1bef, 0x1bf1),
    (0x1c2c, 0x1c33),
    (0x1c36, 0x1c37),
    (0x1cd0, 0x1cd2),
    (0x1cd4, 0x1ce0),
    (0x1ce2, 0x1ce8),
    (0x1ced, 0x1ced),
    (0x1cf4, 0x1cf4),
    (0x1cf8, 0x1cf9),
    (0x1dc0, 0x1dff),
    (0x20d0, 0x20f0),
    (0x2cef, 0x2cf1),
    (0x2d7f, 0x2d7f),
    (0x2de0, 0x2dff),
    (0x302a, 0x302d),
    (0x3099, 0x309a),
    (0xa66f, 0xa672),
    (0xa674, 0xa67d),
    (0xa69e, 0xa69f),
    (0xa6f0, 0xa6f1),
    (0xa802, 0xa802),
    (0xa806, 0xa806),
    (0xa80b, 0xa80b),
    (0xa825, 0xa826),
    (0xa82c, 0xa82c),
    (0xa8c4, 0xa8c5),
    (0xa8e0, 0xa8f1),
    (0xa8ff, 0xa8ff),
    (0xa926, 0xa92d),
    (0xa947, 0xa951),
    (0xa980, 0xa982),
    (0xa9b3, 0xa9b3),
    (0xa9b6, 0xa9b9),
    (0xa9bc, 0xa9bd),
    (0xa9e5, 0xa9e5),
    (0xaa29, 0xaa2e),
    (0xaa31, 0xaa32),
    (0xaa35, 0xaa36),
    (0xaa43, 0xaa43),
    (0xaa4c, 0xaa4c),
    (0xaa7c, 0xaa7c),
    (0xaab0, 0xaab0),
    (0xaab2, 0xaab4),
    (0xaab7, 0xaab8),
    (0xaabe, 0xaabf),
    (0xaac1, 0xaac1),
    (0xaaec, 0xaaed),
    (0xaaf6, 0xaaf6),
    (0xabe5, 0xabe5),
    (0xabe8, 0xabe8),
    (0xabed, 0xabed),
    (0xfb1e, 0xfb1e),
    (0xfe00, 0xfe0f),
    (0xfe20, 0xfe2f),
    (0x101fd, 0x101fd),
    (0x102e0, 0x102e0),
    (0x10376, 0x1037a),
    (0x10a01, 0x10a03),
    (0x10a05, 0x10a06),
    (0x10a0c, 0x10a0f),
    (0x10a38, 0x10a3a),
    (0x10a3f, 0x10a3f),
    (0x10ae5, 0x10ae6),
    (0x10d24, 0x10d27),
    (0x10d69, 0x10d6d),
    (0x10eab, 0x10eac),
    (0x10efa, 0x10eff),
    (0x10f46, 0x10f50),
    (0x10f82, 0x10f85),
    (0x11001, 0x11001),
    (0x11038, 0x11046),
    (0x11070, 0x11070),
    (0x11073, 0x11074),
    (0x1107f, 0x11081),
    (0x110b3, 0x110b6),
    (0x110b9, 0x110ba),
    (0x110c2, 0x110c2),
    (0x11100, 0x11102),
    (0x11127, 0x1112b),
    (0x1112d, 0x11134),
    (0x11173, 0x11173),
    (0x11180, 0x11181),
    (0x111b6, 0x111be),
    (0x111c9, 0x111cc),
    (0x111cf, 0x111cf),
    (0x1122f, 0x11231),
    (0x11234, 0x11234),
    (0x11236, 0x11237),
    (0x1123e, 0x1123e),
    (0x11241, 0x11241),
    (0x112df, 0x112df),
    (0x112e3, 0x112ea),
    (0x11300, 0x11301),
    (0x1133b, 0x1133c),
    (0x11340, 0x11340),
    (0x11366, 0x1136c),
    (0x11370, 0x11374),
    (0x113bb, 0x113c0),
    (0x113ce, 0x113ce),
    (0x113d0, 0x113d0),
    (0x113d2, 0x113d2),
    (0x113e1, 0x113e2),
    (0x11438, 0x1143f),
    (0x11442, 0x11444),
    (0x11446, 0x11446),
    (0x1145e, 0x1145e),
    (0x114b3, 0x114b8),
    (0x114ba, 0x114ba),
    (0x114bf, 0x114c0),
    (0x114c2, 0x114c3),
    (0x115b2, 0x115b5),
    (0x115bc, 0x115bd),
    (0x115bf, 0x115c0),
    (0x115dc, 0x115dd),
    (0x11633, 0x1163a),
    (0x1163d, 0x1163d),
    (0x1163f, 0x11640),
    (0x116ab, 0x116ab),
    (0x116ad, 0x116ad),
    (0x116b0, 0x116b5),
    (0x116b7, 0x116b7),
    (0x1171d, 0x1171d),
    (0x1171f, 0x1171f),
    (0x11722, 0x11725),
    (0x11727, 0x1172b),
    (0x1182f, 0x11837),
    (0x11839, 0x1183a),
    (0x1193b, 0x1193c),
    (0x1193e, 0x1193e),
    (0x11943, 0x11943),
    (0x119d4, 0x119d7),
    (0x119da, 0x119db),
    (0x119e0, 0x119e0),
    (0x11a01, 0x11a0a),
    (0x11a33, 0x11a38),
    (0x11a3b, 0x11a3e),
    (0x11a47, 0x11a47),
    (0x11a51, 0x11a56),
    (0x11a59, 0x11a5b),
    (0x11a8a, 0x11a96),
    (0x11a98, 0x11a99),
    (0x11b60, 0x11b60),
    (0x11b62, 0x11b64),
    (0x11b66, 0x11b66),
    (0x11c30, 0x11c36),
    (0x11c38, 0x11c3d),
    (0x11c3f, 0x11c3f),
    (0x11c92, 0x11ca7),
    (0x11caa, 0x11cb0),
    (0x11cb2, 0x11cb3),
    (0x11cb5, 0x11cb6),
    (0x11d31, 0x11d36),
    (0x11d3a, 0x11d3a),
    (0x11d3c, 0x11d3d),
    (0x11d3f, 0x11d45),
    (0x11d47, 0x11d47),
    (0x11d90, 0x11d91),
    (0x11d95, 0x11d95),
    (0x11d97, 0x11d97),
    (0x11ef3, 0x11ef4),
    (0x11f00, 0x11f01),
    (0x11f36, 0x11f3a),
    (0x11f40, 0x11f40),
    (0x11f42, 0x11f42),
    (0x11f5a, 0x11f5a),
    (0x13440, 0x13440),
    (0x13447, 0x13455),
    (0x1611e, 0x16129),
    (0x1612d, 0x1612f),
    (0x16af0, 0x16af4),
    (0x16b30, 0x16b36),
    (0x16f4f, 0x16f4f),
    (0x16f8f, 0x16f92),
    (0x16fe4, 0x16fe4),
    (0x1bc9d, 0x1bc9e),
    (0x1cf00, 0x1cf2d),
    (0x1cf30, 0x1cf46),
    (0x1d167, 0x1d169),
    (0x1d17b, 0x1d182),
    (0x1d185, 0x1d18b),
    (0x1d1aa, 0x1d1ad),
    (0x1d242, 0x1d244),
    (0x1da00, 0x1da36),
    (0x1da3b, 0x1da6c),
    (0x1da75, 0x1da75),
    (0x1da84, 0x1da84),
    (0x1da9b, 0x1da9f),
    (0x1daa1, 0x1daaf),
    (0x1e000, 0x1e006),
    (0x1e008, 0x1e018),
    (0x1e01b, 0x1e021),
    (0x1e023, 0x1e024),
    (0x1e026, 0x1e02a),
    (0x1e08f, 0x1e08f),
    (0x1e130, 0x1e136),
    (0x1e2ae, 0x1e2ae),
    (0x1e2ec, 0x1e2ef),
    (0x1e4ec, 0x1e4ef),
    (0x1e5ee, 0x1e5ef),
    (0x1e6e3, 0x1e6e3),
    (0x1e6e6, 0x1e6e6),
    (0x1e6ee, 0x1e6ef),
    (0x1e6f5, 0x1e6f5),
    (0x1e8d0, 0x1e8d6),
    (0x1e944, 0x1e94a),
    (0xe0100, 0xe01ef)
];

// ─── Character Cell ─────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Cell {
    const fn blank() -> Self {
        Self { ch: ' ', fg: FG_COLOR, bg: BG_COLOR, bold: false, underline: false, inverse: false }
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
    OscEscape,
    CsiDiscard,
}

// ─── Terminal Emulator ──────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct SavedCursor {
    x: usize,
    y: usize,
    fg: Color,
    bg: Color,
    bold: bool,
    underline: bool,
    inverse: bool,
    autowrap: bool,
    pending_wrap: bool,
}

impl Default for SavedCursor {
    fn default() -> Self {
        Self { x: 0, y: 0, fg: FG_COLOR, bg: BG_COLOR, bold: false, underline: false, inverse: false,
            autowrap: true, pending_wrap: false }
    }
}

struct MainScreen {
    cells: Vec<Cell>,
    cursor: SavedCursor,
    saved_cursor: SavedCursor,
    scroll_top: usize,
    scroll_bottom: usize,
}

pub struct TermEmu {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    cursor_x: usize,
    cursor_y: usize,
    fg: Color,
    bg: Color,
    bold: bool,
    underline: bool,
    inverse: bool,
    autowrap: bool,
    application_cursor_keys: bool,
    pending_wrap: bool,
    saved_cursor: SavedCursor,
    main_screen: Option<MainScreen>,
    scroll_top: usize,
    scroll_bottom: usize,
    tabs: Vec<bool>,
    ansi_private: bool,
    alternate: bool,
    cursor_visible: bool,
    ansi_state: AnsiState,
    ansi_params: [u16; 16],
    ansi_len: usize,
    ansi_invalid: bool,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_expected: usize,
    ansi_param_idx: usize,
    pub scrollback: Scrollback,
    pub replies: VecDeque<u8>,
    title: String,
    osc: Vec<u8>,
}

impl TermEmu {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols, rows,
            cells: vec![Cell::blank(); cols * rows],
            cursor_x: 0, cursor_y: 0,
            fg: FG_COLOR, bg: BG_COLOR, bold: false, underline: false, inverse: false,
            application_cursor_keys: false, autowrap: true, pending_wrap: false, saved_cursor: SavedCursor::default(),
            main_screen: None, scroll_top: 0, scroll_bottom: rows - 1,
            tabs: (0..cols).map(|x| x != 0 && x % 8 == 0).collect(),
            ansi_private: false, alternate: false, cursor_visible: true,
            ansi_state: AnsiState::Normal,
            ansi_params: [0; 16],
            ansi_len: 0, ansi_invalid: false, utf8: [0; 4], utf8_len: 0, utf8_expected: 0,
            ansi_param_idx: 0,
            scrollback: Scrollback::default(),
            replies: VecDeque::new(),
            title: String::from("Terminal"), osc: Vec::new(),
        }
    }

    pub fn cell(&self, x: usize, y: usize) -> &Cell { &self.cells[y * self.cols + x] }
    fn cell_mut(&mut self, x: usize, y: usize) -> &mut Cell { &mut self.cells[y * self.cols + x] }

    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        let new_cols = new_cols.max(1);
        let new_rows = new_rows.max(1);
        if new_cols == self.cols && new_rows == self.rows { return; }
        let resize_cells = |cells: &[Cell]| {
            let mut resized = vec![Cell::blank(); new_cols * new_rows];
            for y in 0..self.rows.min(new_rows) {
                let n = self.cols.min(new_cols);
                resized[y * new_cols..y * new_cols + n]
                    .copy_from_slice(&cells[y * self.cols..y * self.cols + n]);
            }
            resized
        };
        self.cells = resize_cells(&self.cells);
        if let Some(main) = &mut self.main_screen {
            main.cells = resize_cells(&main.cells);
            main.cursor.x = main.cursor.x.min(new_cols - 1);
            main.cursor.y = main.cursor.y.min(new_rows - 1);
            main.cursor.pending_wrap = false;
            main.scroll_top = 0;
            main.scroll_bottom = new_rows - 1;
        }
        let old_cols = self.cols;
        self.tabs.resize(new_cols, false);
        for x in old_cols..new_cols { self.tabs[x] = x != 0 && x % 8 == 0; }
        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_x = self.cursor_x.min(new_cols - 1);
        self.cursor_y = self.cursor_y.min(new_rows - 1);
        self.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
    }

    pub fn cursor(&self) -> (usize, usize) { (self.cursor_x, self.cursor_y) }
    pub fn alternate_screen(&self) -> bool { self.alternate }
    pub fn title(&self) -> &str { &self.title }
    pub fn application_cursor_keys(&self) -> bool { self.application_cursor_keys }
    pub fn cursor_visible(&self) -> bool { self.cursor_visible }
    pub fn size(&self) -> (usize, usize) { (self.cols, self.rows) }
    pub fn feed(&mut self, bytes: &[u8]) { for &byte in bytes { self.feed_byte(byte); } }

    fn cursor_state(&self) -> SavedCursor {
        SavedCursor { x: self.cursor_x, y: self.cursor_y, fg: self.fg, bg: self.bg,
            bold: self.bold, underline: self.underline, inverse: self.inverse, autowrap: self.autowrap, pending_wrap: self.pending_wrap }
    }

    fn restore_cursor(&mut self, saved: SavedCursor) {
        self.cursor_x = saved.x.min(self.cols - 1);
        self.cursor_y = saved.y.min(self.rows - 1);
        self.fg = saved.fg;
        self.bg = saved.bg;
        self.bold = saved.bold;
        self.underline = saved.underline;
        self.inverse = saved.inverse;
        self.autowrap = saved.autowrap;
        self.pending_wrap = saved.pending_wrap && self.cursor_x == self.cols - 1;
    }

    fn alternate_screen_mode(&mut self, enabled: bool) {
        if enabled == self.alternate { return; }
        if enabled {
            let cursor = self.cursor_state();
            self.main_screen = Some(MainScreen {
                cells: core::mem::replace(&mut self.cells, vec![Cell::blank(); self.cols * self.rows]),
                cursor, saved_cursor: self.saved_cursor,
                scroll_top: self.scroll_top, scroll_bottom: self.scroll_bottom,
            });
            self.saved_cursor = SavedCursor::default();
            self.pending_wrap = false;
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
        } else if let Some(main) = self.main_screen.take() {
            self.cells = main.cells;
            self.restore_cursor(main.cursor);
            self.saved_cursor = main.saved_cursor;
            self.scroll_top = main.scroll_top;
            self.scroll_bottom = main.scroll_bottom;
        }
        self.alternate = enabled;
    }

    fn erase_cell(&self) -> Cell {
        Cell { ch: ' ', fg: self.fg, bg: self.bg, bold: self.bold, underline: self.underline, inverse: self.inverse }
    }

    fn scroll_region(&mut self, top: usize, bottom: usize, count: usize, down: bool) {
        let n = count.min(bottom - top + 1);
        let start = top * self.cols;
        let end = (bottom + 1) * self.cols;
        let shift = n * self.cols;
        let blank = self.erase_cell();
        if down {
            self.cells.copy_within(start..end - shift, start + shift);
            self.cells[start..start + shift].fill(blank);
        } else {
            self.cells.copy_within(start + shift..end, start);
            self.cells[end - shift..end].fill(blank);
        }
    }

    fn index(&mut self, reverse: bool) {
        self.pending_wrap = false;
        if reverse {
            if self.cursor_y == self.scroll_top {
                self.scroll_region(self.scroll_top, self.scroll_bottom, 1, true);
            } else { self.cursor_y = self.cursor_y.saturating_sub(1); }
        } else if self.cursor_y == self.scroll_bottom {
            self.scroll_region(self.scroll_top, self.scroll_bottom, 1, false);
        } else { self.cursor_y = (self.cursor_y + 1).min(self.rows - 1); }
    }

    fn edit_characters(&mut self, cmd: u8, count: usize) {
        let start = self.cursor_y * self.cols + self.cursor_x;
        let end = (self.cursor_y + 1) * self.cols;
        let n = count.min(self.cols - self.cursor_x);
        let blank = self.erase_cell();
        match cmd {
            b'@' => {
                self.cells.copy_within(start..end - n, start + n);
                self.cells[start..start + n].fill(blank);
            }
            b'P' => {
                self.cells.copy_within(start + n..end, start);
                self.cells[end - n..end].fill(blank);
            }
            b'X' => self.cells[start..start + n].fill(blank),
            _ => unreachable!(),
        }
    }

    fn put_char(&mut self, ch: char) {
        if COMBINING.binary_search_by(|&(start, end)| {
            if (ch as u32) < start { core::cmp::Ordering::Greater }
            else if (ch as u32) > end { core::cmp::Ordering::Less }
            else { core::cmp::Ordering::Equal }
        }).is_ok() { return; }
        if self.pending_wrap {
            self.cursor_x = 0;
            self.index(false);
        }
        let fg = self.fg; let bg = self.bg; let bold = self.bold;
        let idx = self.cursor_y * self.cols + self.cursor_x;
        self.cells[idx] = Cell { ch, fg, bg, bold, underline: self.underline, inverse: self.inverse };
        if self.cursor_x == self.cols - 1 { self.pending_wrap = self.autowrap; }
        else { self.cursor_x += 1; }
    }

    fn feed_byte(&mut self, byte: u8) {
        if self.ansi_state != AnsiState::Normal && matches!(byte, 0x18 | 0x1a) {
            self.ansi_state = AnsiState::Normal;
            return;
        }
        if self.ansi_state == AnsiState::Normal && self.utf8_len != 0 {
            if byte & 0xc0 == 0x80 {
                self.utf8[self.utf8_len] = byte;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_expected {
                    let ch = core::str::from_utf8(&self.utf8[..self.utf8_len])
                        .ok().and_then(|s| s.chars().next()).unwrap_or('\u{fffd}');
                    self.utf8_len = 0;
                    self.put_char(ch);
                }
                return;
            }
            self.utf8_len = 0;
            self.put_char('\u{fffd}');
        }
        match self.ansi_state {
            AnsiState::Normal => match byte {
                0x1b => self.ansi_state = AnsiState::Escape,
                b'\n' => { self.cursor_x = 0; self.index(false); }
                b'\r' => { self.cursor_x = 0; self.pending_wrap = false; }
                b'\t' => {
                    self.cursor_x = (self.cursor_x + 1..self.cols)
                        .find(|&x| self.tabs[x]).unwrap_or(self.cols - 1);
                    self.pending_wrap = false;
                }
                0x08 => { self.cursor_x = self.cursor_x.saturating_sub(1); self.pending_wrap = false; }
                0x20..=0x7e => self.put_char(byte as char),
                0xc2..=0xf4 => {
                    self.utf8[0] = byte; self.utf8_len = 1;
                    self.utf8_expected = if byte < 0xe0 { 2 } else if byte < 0xf0 { 3 } else { 4 };
                }
                0x80..=0xff => self.put_char('\u{fffd}'),
                _ => {}
            },
            AnsiState::Escape => match byte {
                b'[' => { self.ansi_state = AnsiState::Csi; self.ansi_params = [0; 16]; self.ansi_param_idx = 0; self.ansi_len = 0; self.ansi_invalid = false; self.ansi_private = false; }
                b']' => { self.ansi_state = AnsiState::OscString; self.ansi_len = 0; self.ansi_invalid = false; self.osc.clear(); }
                0x1b => {}
                _ => {
                    match byte {
                        b'D' => self.index(false),
                        b'M' => self.index(true),
                        b'H' => self.tabs[self.cursor_x] = true,
                        b'7' => self.saved_cursor = self.cursor_state(),
                        b'8' => self.restore_cursor(self.saved_cursor),
                        _ => {}
                    }
                    self.ansi_state = AnsiState::Normal;
                }
            },
            AnsiState::Csi | AnsiState::CsiParam => {
                self.ansi_len += 1;
                if self.ansi_len >= 256 { self.ansi_state = if (0x40..=0x7e).contains(&byte) { AnsiState::Normal } else { AnsiState::CsiDiscard }; }
                else if byte == b'?' && self.ansi_len == 1 {
                    self.ansi_private = true;
                } else if byte.is_ascii_digit() && !self.ansi_invalid {
                    self.ansi_state = AnsiState::CsiParam;
                    let p = &mut self.ansi_params[self.ansi_param_idx];
                    *p = p.saturating_mul(10).saturating_add((byte - b'0') as u16).min(9999);
                } else if byte == b';' && !self.ansi_invalid {
                    if self.ansi_param_idx < 15 { self.ansi_param_idx += 1; } else { self.ansi_invalid = true; }
                } else if (0x40..=0x7e).contains(&byte) {
                    if !self.ansi_invalid { self.execute_csi(byte, self.ansi_param_idx + 1); }
                    self.ansi_state = AnsiState::Normal;
                } else { self.ansi_invalid = true; }
            }
            AnsiState::CsiDiscard => { if (0x40..=0x7e).contains(&byte) { self.ansi_state = AnsiState::Normal; } }
            AnsiState::OscString => {
                if byte == 0x07 { self.finish_osc(); }
                else if byte == 0x1b { self.ansi_state = AnsiState::OscEscape; }
                else { self.osc_byte(byte); }
            }
            AnsiState::OscEscape => {
                if byte == b'\\' || byte == 0x07 { self.finish_osc(); }
                else {
                    self.osc_byte(0x1b);
                    if byte != 0x1b { self.osc_byte(byte); self.ansi_state = AnsiState::OscString; }
                }
            }
        }
    }

    fn osc_byte(&mut self, byte: u8) {
        if self.osc.len() < 512 { self.osc.push(byte); } else { self.ansi_invalid = true; }
    }

    fn finish_osc(&mut self) {
        if !self.ansi_invalid && (self.osc.starts_with(b"0;") || self.osc.starts_with(b"2;")) {
            self.title = String::from_utf8_lossy(&self.osc[2..]).into_owned();
        }
        self.ansi_state = AnsiState::Normal;
    }

    fn indexed_color(index: u16) -> Option<Color> {
        match index {
            0..=7 => Some(ANSI_COLORS[index as usize]),
            8..=15 => { let c = ANSI_COLORS[(index - 8) as usize]; Some(Color::rgb(c.r.saturating_add(60), c.g.saturating_add(60), c.b.saturating_add(60))) }
            16..=231 => { let n = index - 16; let v = |x: u16| if x == 0 { 0 } else { (55 + 40 * x) as u8 }; Some(Color::rgb(v(n / 36), v(n / 6 % 6), v(n % 6))) }
            232..=255 => { let v = (8 + 10 * (index - 232)) as u8; Some(Color::rgb(v, v, v)) }
            _ => None,
        }
    }

    fn execute_csi(&mut self, cmd: u8, nparams: usize) {
        let p0 = self.ansi_params[0] as usize;
        let p1 = if nparams > 1 { self.ansi_params[1] as usize } else { 0 };
        if self.ansi_private {
            if matches!(cmd, b'h' | b'l') {
                let enabled = cmd == b'h';
                for i in 0..nparams {
                    match self.ansi_params[i] {
                        1 => self.application_cursor_keys = enabled,
                        7 => { self.autowrap = enabled; self.pending_wrap = false; }
                        25 => self.cursor_visible = enabled,
                        1049 => self.alternate_screen_mode(enabled),
                        _ => {}
                    }
                }
            }
            return;
        }
        if matches!(cmd, b'H' | b'f' | b'A' | b'B' | b'C' | b'D' | b'G' | b'd'
            | b'J' | b'K' | b'L' | b'M' | b'@' | b'P' | b'X') {
            self.pending_wrap = false;
        }
        match cmd {
            b'H' | b'f' => {
                let row = if p0 > 0 { p0 - 1 } else { 0 };
                let col = if p1 > 0 { p1 - 1 } else { 0 };
                self.cursor_y = row.min(self.rows - 1);
                self.cursor_x = col.min(self.cols - 1);
                    }
            b'A' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_y = self.cursor_y.saturating_sub(n); }
            b'B' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_y = (self.cursor_y + n).min(self.rows - 1); }
            b'C' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_x = (self.cursor_x + n).min(self.cols - 1); }
            b'D' => { let n = if p0 > 0 { p0 } else { 1 }; self.cursor_x = self.cursor_x.saturating_sub(n); }
            b'G' => { let col = if p0 > 0 { p0 - 1 } else { 0 }; self.cursor_x = col.min(self.cols - 1); }
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
                    }
            b'K' => {
                match p0 {
                    0 => { for x in self.cursor_x..self.cols { *self.cell_mut(x, self.cursor_y) = Cell::blank(); } }
                    1 => { for x in 0..=self.cursor_x.min(self.cols - 1) { *self.cell_mut(x, self.cursor_y) = Cell::blank(); } }
                    2 => { for x in 0..self.cols { *self.cell_mut(x, self.cursor_y) = Cell::blank(); } }
                    _ => {}
                }
                    }
            b'd' => { let row = if p0 > 0 { p0 - 1 } else { 0 }; self.cursor_y = row.min(self.rows - 1); }
            b'r' => {
                let top = p0.max(1) - 1;
                let bottom = if p1 == 0 { self.rows - 1 } else { p1 - 1 };
                if top < bottom && bottom < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                    self.pending_wrap = false;
                }
            }
            b'L' | b'M' => {
                if (self.scroll_top..=self.scroll_bottom).contains(&self.cursor_y) {
                    self.scroll_region(self.cursor_y, self.scroll_bottom, p0.max(1), cmd == b'L');
                    self.cursor_x = 0;
                }
            }
            b'@' | b'P' | b'X' => self.edit_characters(cmd, p0.max(1)),
            b'g' => match p0 {
                0 => self.tabs[self.cursor_x] = false,
                3 => self.tabs.fill(false),
                _ => {}
            },
            b'n' => match p0 {
                5 => self.replies.extend(b"\x1b[0n"),
                6 => self.replies.extend(format!("\x1b[{};{}R", self.cursor_y + 1, self.cursor_x + 1).bytes()),
                _ => {}
            },
            b'c' if p0 == 0 => self.replies.extend(b"\x1b[?1;2c"),
            b'm' => {
                let mut i = 0;
                while i < nparams {
                    let code = self.ansi_params[i];
                    match code {
                        0 => { self.fg = FG_COLOR; self.bg = BG_COLOR; self.bold = false; self.underline = false; self.inverse = false; }
                        1 => self.bold = true,
                        4 => self.underline = true,
                        7 => self.inverse = true,
                        22 => self.bold = false,
                        24 => self.underline = false,
                        27 => self.inverse = false,
                        30..=37 => self.fg = ANSI_COLORS[(code - 30) as usize],
                        39 => self.fg = FG_COLOR,
                        40..=47 => self.bg = ANSI_COLORS[(code - 40) as usize],
                        49 => self.bg = BG_COLOR,
                        90..=97 => self.fg = Self::indexed_color(code - 90 + 8).unwrap(),
                        100..=107 => self.bg = Self::indexed_color(code - 100 + 8).unwrap(),
                        38 | 48 if i + 1 < nparams => {
                            let mode = self.ansi_params[i + 1];
                            let color = if mode == 5 && i + 2 < nparams {
                                i += 2; Self::indexed_color(self.ansi_params[i])
                            } else if mode == 2 && i + 4 < nparams {
                                let vals = &self.ansi_params[i + 2..i + 5];
                                let color = if vals.iter().all(|&v| v <= 255) { Some(Color::rgb(vals[0] as u8, vals[1] as u8, vals[2] as u8)) } else { None };
                                i += 4; color
                            } else { i += 1; None };
                            if let Some(c) = color { if code == 38 { self.fg = c; } else { self.bg = c; } }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'l' | b'h' => {}
            _ => {}
        }
    }

}
