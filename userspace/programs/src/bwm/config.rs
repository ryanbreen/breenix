use super::*;

// ─── Constants ───────────────────────────────────────────────────────────────
// Chrome sizes are logical units: `px` multiplies them by the UI scale. Client windows
// keep their pixel sizes.

/// Title bar height
pub(super) const TITLE_BAR_HEIGHT: usize = 32;

/// Window border/shadow width
pub(super) const BORDER_WIDTH: usize = 2;

/// How far a window's drop shadow is offset right and down
pub(super) const SHADOW_OFFSET: usize = 3;

/// TrueType font size for UI text, in logical units.
pub(super) const TTF_FONT_SIZE: f32 = 14.0;

/// The console's size rule (`docs/console/contract.md`): the largest font whose text grid
/// on the display is at least this many columns by rows.
pub(super) const MIN_TEXT_COLS: usize = 128;
pub(super) const MIN_TEXT_ROWS: usize = 36;

/// bwm's UI scale and the bitmap font it draws chrome text with.
pub(super) struct Ui {
    pub(super) scale: usize,
    pub(super) font: bitmap_font::RasterHeight,
    /// Text cell: the glyph advance, and the glyph height with its line spacing.
    pub(super) cell_w: usize,
    pub(super) cell_h: usize,
}

impl Ui {
    /// Choose the scale for a `width` x `height` display as the console chooses its font:
    /// the largest of bwm's text sizes, 16 px at scale 1 and 32 px at scale 2 (each its
    /// own raster), whose grid is at least 128 columns by 36 rows; otherwise scale 1.
    pub(super) fn for_display(width: usize, height: usize) -> Ui {
        let sizes = [(2, bitmap_font::RasterHeight::Size32), (1, bitmap_font::RasterHeight::Size16)];
        let uis = sizes.map(|(scale, font)| {
            let m = bitmap_font::metrics_at(font);
            Ui { scale, font, cell_w: m.char_width, cell_h: m.line_height() }
        });
        let [two, one] = uis;
        if width / two.cell_w >= MIN_TEXT_COLS && height / two.cell_h >= MIN_TEXT_ROWS {
            two
        } else {
            one
        }
    }
}

pub(super) static UI: std::sync::OnceLock<Ui> = std::sync::OnceLock::new();

pub(super) fn ui() -> &'static Ui {
    UI.get().expect("bwm's UI scale is chosen at start")
}

/// `n` logical units in pixels.
pub(super) fn px(n: usize) -> usize {
    n * ui().scale
}

/// `n` logical units in pixels, signed.
pub(super) fn pxi(n: i32) -> i32 {
    n * ui().scale as i32
}

/// Top taskbar height
pub(super) const TASKBAR_HEIGHT: usize = 28;

/// Bottom app bar height
pub(super) const APPBAR_HEIGHT: usize = 36;

/// Chrome button size (close/minimize)
pub(super) const CHROME_BTN_SIZE: usize = 20;

/// Padding between chrome buttons
pub(super) const CHROME_BTN_PAD: usize = 4;

/// Space reserved in title bar for chrome buttons
pub(super) const CHROME_RESERVED: usize = 52;

/// Resize grab zone width in pixels (edges and corners)
pub(super) const RESIZE_GRAB: usize = 8;

/// Minimum total window width (including decorations)
pub(super) const MIN_WIN_WIDTH: usize = 120;

/// Minimum total window height (including decorations)
pub(super) const MIN_WIN_HEIGHT: usize = 80 + TITLE_BAR_HEIGHT + BORDER_WIDTH * 2;

/// Maximum remembered window positions/sizes
pub(super) const MAX_DEFAULTS: usize = 16;

// Colors
pub(super) const TITLE_FOCUSED_BG: Color = Color::rgb(40, 100, 220);
pub(super) const TITLE_UNFOCUSED_BG: Color = Color::rgb(45, 50, 65);
pub(super) const TITLE_TEXT: Color = Color::rgb(160, 165, 175);
pub(super) const TITLE_FOCUSED_TEXT: Color = Color::rgb(255, 255, 255);
pub(super) const WIN_BORDER_COLOR: Color = Color::rgb(50, 55, 70);
pub(super) const WIN_BORDER_FOCUSED: Color = Color::rgb(60, 130, 255);

// Taskbar/Appbar colors
pub(super) const TASKBAR_BG: Color = Color::rgb(20, 22, 30);
pub(super) const TASKBAR_TEXT: Color = Color::rgb(180, 185, 195);
pub(super) const APPBAR_BG: Color = Color::rgb(25, 28, 38);
pub(super) const APPBAR_BORDER: Color = Color::rgb(50, 55, 70);
pub(super) const APPBAR_BTN_BG: Color = Color::rgb(40, 45, 60);
pub(super) const APPBAR_BTN_FOCUSED: Color = Color::rgb(40, 100, 220);
pub(super) const APPBAR_BTN_MINIMIZED: Color = Color::rgb(30, 33, 45);
pub(super) const APPBAR_BTN_TEXT: Color = Color::rgb(160, 165, 175);

// Chrome button colors
pub(super) const CLOSE_BTN_BG: Color = Color::rgb(200, 50, 50);
pub(super) const CLOSE_BTN_TEXT: Color = Color::rgb(255, 255, 255);
pub(super) const MINIMIZE_BTN_BG: Color = Color::rgb(80, 85, 100);
pub(super) const MINIMIZE_BTN_TEXT: Color = Color::rgb(255, 255, 255);

pub(super) fn monotonic_ms() -> Option<u64> {
    libbreenix::time::now_monotonic()
        .ok()
        .map(|ts| ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000)
}

// ─── Main ────────────────────────────────────────────────────────────────────

/// How long bwm sleeps for input or a scene change before it reads the pointer again: the
/// pointer has no descriptor of its own.
pub(super) const POINTER_POLL_MS: i32 = 10;

/// How long bwm sleeps between attempts to compose while another process holds the display.
pub(super) const DISPLAY_WAIT_MS: i32 = 100;

/// How many composed frames a client's first-frame check may find it had committed again
/// before that check is reported failed.
pub(super) const CHECK_MOVED_LIMIT: u32 = 200;

/// Report a complete scanout comparison for a newly shown or resized client.
pub(super) fn report_check(id: u64, result: &Result<u64, CheckError>) {
    match result {
        Ok(pixels) => print!("[bwm] composited client frame: window {} on screen, {} pixels exact\n", id, pixels),
        Err(CheckError::Moved(e)) | Err(CheckError::Failed(e)) => print!("[bwm] client frame check FAILED: window {}: {}\n", id, e),
    }
}
