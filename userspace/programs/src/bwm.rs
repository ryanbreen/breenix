//! Breenix Window Manager (bwm) — Pure Compositor + Input Router
//!
//! Discovers Breengel client windows registered via the kernel window buffer API,
//! composites them onto the screen through the GPU core (`compositor`), and routes
//! keyboard/mouse input to the focused window via kernel ring buffers.
//!
//! BWM does NOT spawn child processes or emulate terminals. Those responsibilities
//! belong to init (process spawning) and bterm (terminal emulation).

#[path = "bwm/compositor.rs"]
mod compositor;

use std::process;

use compositor::{Compositor, Layer, Pointer};

#[cfg(not(target_arch = "aarch64"))]
use libbreenix::fs;
use libbreenix::graphics::{self, WindowInputEvent, input_event_type};
use libbreenix::io;
use libbreenix::signal;
use libbreenix::types::Fd;

#[cfg(not(target_arch = "aarch64"))]
use breengel::FontConfig;
use libfont::CachedFont;
#[cfg(not(target_arch = "aarch64"))]
use libfont::Font;
use libgfx::bitmap_font;
use libgfx::ttf_font;
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;

// ─── Constants ───────────────────────────────────────────────────────────────
// Chrome sizes are logical units: `px` multiplies them by the UI scale. Client windows
// keep their pixel sizes.

/// Title bar height
const TITLE_BAR_HEIGHT: usize = 32;

/// Window border/shadow width
const BORDER_WIDTH: usize = 2;

/// How far a window's drop shadow is offset right and down
const SHADOW_OFFSET: usize = 3;

/// TrueType font size for UI text, in logical units.
const TTF_FONT_SIZE: f32 = 14.0;

/// The console's size rule (`docs/console/contract.md`): the largest font whose text grid
/// on the display is at least this many columns by rows.
const MIN_TEXT_COLS: usize = 128;
const MIN_TEXT_ROWS: usize = 36;

/// bwm's UI scale and the bitmap font it draws chrome text with.
struct Ui {
    scale: usize,
    font: bitmap_font::RasterHeight,
    /// Text cell: the glyph advance, and the glyph height with its line spacing.
    cell_w: usize,
    cell_h: usize,
}

impl Ui {
    /// Choose the scale for a `width` x `height` display as the console chooses its font:
    /// the largest of bwm's text sizes, 16 px at scale 1 and 32 px at scale 2 (each its
    /// own raster), whose grid is at least 128 columns by 36 rows; otherwise scale 1.
    fn for_display(width: usize, height: usize) -> Ui {
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

static UI: std::sync::OnceLock<Ui> = std::sync::OnceLock::new();

fn ui() -> &'static Ui {
    UI.get().expect("bwm's UI scale is chosen at start")
}

/// `n` logical units in pixels.
fn px(n: usize) -> usize {
    n * ui().scale
}

/// `n` logical units in pixels, signed.
fn pxi(n: i32) -> i32 {
    n * ui().scale as i32
}

/// Top taskbar height
const TASKBAR_HEIGHT: usize = 28;

/// Bottom app bar height
const APPBAR_HEIGHT: usize = 36;

/// Chrome button size (close/minimize)
const CHROME_BTN_SIZE: usize = 20;

/// Padding between chrome buttons
const CHROME_BTN_PAD: usize = 4;

/// Space reserved in title bar for chrome buttons
const CHROME_RESERVED: usize = 52;

/// Resize grab zone width in pixels (edges and corners)
const RESIZE_GRAB: usize = 8;

/// Minimum total window width (including decorations)
const MIN_WIN_WIDTH: usize = 120;

/// Minimum total window height (including decorations)
const MIN_WIN_HEIGHT: usize = 80 + TITLE_BAR_HEIGHT + BORDER_WIDTH * 2;

/// Maximum remembered window positions/sizes
const MAX_DEFAULTS: usize = 16;

// Colors
const TITLE_FOCUSED_BG: Color = Color::rgb(40, 100, 220);
const TITLE_UNFOCUSED_BG: Color = Color::rgb(45, 50, 65);
const TITLE_TEXT: Color = Color::rgb(160, 165, 175);
const TITLE_FOCUSED_TEXT: Color = Color::rgb(255, 255, 255);
const WIN_BORDER_COLOR: Color = Color::rgb(50, 55, 70);
const WIN_BORDER_FOCUSED: Color = Color::rgb(60, 130, 255);

// Taskbar/Appbar colors
const TASKBAR_BG: Color = Color::rgb(20, 22, 30);
const TASKBAR_TEXT: Color = Color::rgb(180, 185, 195);
const APPBAR_BG: Color = Color::rgb(25, 28, 38);
const APPBAR_BORDER: Color = Color::rgb(50, 55, 70);
const APPBAR_BTN_BG: Color = Color::rgb(40, 45, 60);
const APPBAR_BTN_FOCUSED: Color = Color::rgb(40, 100, 220);
const APPBAR_BTN_MINIMIZED: Color = Color::rgb(30, 33, 45);
const APPBAR_BTN_TEXT: Color = Color::rgb(160, 165, 175);

// Chrome button colors
const CLOSE_BTN_BG: Color = Color::rgb(200, 50, 50);
const CLOSE_BTN_TEXT: Color = Color::rgb(255, 255, 255);
const MINIMIZE_BTN_BG: Color = Color::rgb(80, 85, 100);
const MINIMIZE_BTN_TEXT: Color = Color::rgb(255, 255, 255);

// ─── Input Parser ────────────────────────────────────────────────────────────
// Parses stdin bytes (keyboard input) into InputEvents that BWM can either
// handle internally (F-key focus switching) or route to the focused client
// window as WindowInputEvents.

struct InputParser {
    state: InputState,
    esc_buf: [u8; 8],
    esc_len: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum InputState { Normal, Escape, CsiOrSS3 }

enum InputEvent {
    /// A regular key press: ascii byte + modifiers (bit 1 = ctrl)
    Key { ascii: u8, keycode: u16, modifiers: u16 },
    /// F1..F5 for BWM-internal focus switching
    FunctionKey(u8),
}

impl InputParser {
    fn new() -> Self { Self { state: InputState::Normal, esc_buf: [0; 8], esc_len: 0 } }

    fn feed(&mut self, byte: u8) -> Option<InputEvent> {
        match self.state {
            InputState::Normal => {
                if byte == 0x1b { self.state = InputState::Escape; self.esc_len = 0; None }
                else { Some(Self::byte_to_key_event(byte)) }
            }
            InputState::Escape => {
                if byte == b'[' || byte == b'O' {
                    self.state = InputState::CsiOrSS3; self.esc_buf[0] = byte; self.esc_len = 1; None
                } else { self.state = InputState::Normal; Some(Self::byte_to_key_event(byte)) }
            }
            InputState::CsiOrSS3 => {
                if self.esc_len < 7 { self.esc_buf[self.esc_len] = byte; self.esc_len += 1; }
                if byte >= 0x40 && byte <= 0x7e { self.state = InputState::Normal; self.decode_escape() }
                else { None }
            }
        }
    }

    /// Convert a raw stdin byte into a Key event with appropriate keycode/modifiers.
    fn byte_to_key_event(byte: u8) -> InputEvent {
        match byte {
            // Enter (before Ctrl range since 0x0D falls in 0x01..=0x1A)
            0x0D => InputEvent::Key { ascii: 13, keycode: 0x28, modifiers: 0 },
            // Backspace (0x08 falls in Ctrl range; 0x7F does not)
            0x08 | 0x7F => InputEvent::Key { ascii: 8, keycode: 0x2A, modifiers: 0 },
            // Tab (0x09 falls in Ctrl range)
            0x09 => InputEvent::Key { ascii: 9, keycode: 0x2B, modifiers: 0 },
            // Ctrl+A through Ctrl+Z (0x01..0x1A), excluding Enter/Backspace/Tab matched above
            0x01..=0x1A => InputEvent::Key {
                ascii: byte,
                keycode: byte as u16, // raw control code
                modifiers: 0x02,      // ctrl bit
            },
            // Printable ASCII + space
            _ => InputEvent::Key { ascii: byte, keycode: byte as u16, modifiers: 0 },
        }
    }

    fn decode_escape(&self) -> Option<InputEvent> {
        if self.esc_len < 2 { return None; }
        let final_byte = self.esc_buf[self.esc_len - 1];
        if self.esc_buf[0] == b'[' {
            match final_byte {
                // Arrow keys -> Key events with USB HID keycodes
                b'A' => return Some(InputEvent::Key { ascii: 0, keycode: 0x52, modifiers: 0 }), // Up
                b'B' => return Some(InputEvent::Key { ascii: 0, keycode: 0x51, modifiers: 0 }), // Down
                b'C' => return Some(InputEvent::Key { ascii: 0, keycode: 0x4F, modifiers: 0 }), // Right
                b'D' => return Some(InputEvent::Key { ascii: 0, keycode: 0x50, modifiers: 0 }), // Left
                b'~' if self.esc_len >= 3 => {
                    let _num = self.esc_buf[1] - b'0';
                    if self.esc_len == 3 { if _num == 1 { return Some(InputEvent::FunctionKey(1)); } }
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

// ─── Hotkey System ──────────────────────────────────────────────────────────
// Configurable hotkey matching with multi-tap support. Hotkeys are loaded from
// /etc/hotkeys.conf at startup. BWM polls modifier state from the kernel each
// frame and detects key combos + multi-tap sequences (e.g. Super+Super).

/// Modifier bitmask constants (matches kernel HID poll_modifier_state)
mod modifier {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8  = 2;
    pub const ALT: u8   = 4;
    pub const SUPER: u8 = 8;
}

/// What action a hotkey triggers
#[derive(Clone)]
enum HotkeyAction {
    /// Spawn a program by path
    Exec(Vec<u8>),
    /// Close the focused window
    Close,
    /// Cycle focus to the next window
    FocusNext,
    /// Cycle focus to the previous window
    FocusPrev,
}

/// A single hotkey binding
#[derive(Clone)]
struct Hotkey {
    /// Required modifier bitmask (e.g. SUPER | ALT)
    modifiers: u8,
    /// ASCII key (0 for modifier-only combos like Super+Super)
    key: u8,
    /// Number of taps required (1 = normal press, 2 = double-tap)
    taps: u8,
    /// What to do when triggered
    action: HotkeyAction,
}

/// Hotkey matcher state machine
struct HotkeyManager {
    bindings: Vec<Hotkey>,
    /// Previous modifier state (for edge detection)
    prev_modifiers: u8,
    /// For multi-tap detection: which modifier was last cleanly released
    tap_modifier: u8,
    /// Timestamp (monotonic ns) of last clean modifier release
    tap_release_ns: u64,
    /// Number of clean taps counted so far
    tap_count: u8,
    /// Whether any non-modifier key was pressed during current modifier hold
    combo_used: bool,
    /// Cooldown frames remaining (prevents rapid re-trigger)
    cooldown: u32,
}

impl HotkeyManager {
    fn new() -> Self {
        Self {
            bindings: Vec::new(),
            prev_modifiers: 0,
            tap_modifier: 0,
            tap_release_ns: 0,
            tap_count: 0,
            combo_used: false,
            cooldown: 0,
        }
    }

    /// Load hotkey config from a file. Format:
    ///   modifier+modifier+key = action
    ///   Super+Super = exec blauncher
    ///   Alt+F4 = close
    ///   Super+Return = exec bterm
    #[cfg(not(target_arch = "aarch64"))]
    fn load_config(&mut self, path: &str) {
        let fd = match fs::open(path, fs::O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => {
                print!("[bwm] hotkeys: no config at {}, using defaults\n", path);
                self.load_defaults();
                return;
            }
        };
        let mut buf = vec![0u8; 4096];
        let n = match io::read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = io::close(fd);
                print!("[bwm] hotkeys: failed to read config, using defaults\n");
                self.load_defaults();
                return;
            }
        };
        let _ = io::close(fd);
        buf.truncate(n);

        let mut found_any = false;
        for line in buf.split(|&b| b == b'\n') {
            let line = trim(line);
            if line.is_empty() || line[0] == b'#' { continue; }
            if let Some(hotkey) = Self::parse_line(line) {
                self.bindings.push(hotkey);
                found_any = true;
            }
        }

        if !found_any {
            print!("[bwm] hotkeys: config empty, using defaults\n");
            self.load_defaults();
        }
    }

    fn load_defaults(&mut self) {
        self.bindings.clear();
        self.bindings.push(Hotkey {
            modifiers: modifier::SUPER, key: 0, taps: 2,
            action: HotkeyAction::Exec(b"/bin/blauncher".to_vec()),
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::SUPER, key: b'\r', taps: 1,
            action: HotkeyAction::Exec(b"/bin/bterm".to_vec()),
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT, key: b'q', taps: 1,
            action: HotkeyAction::Close,
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT, key: b'\t', taps: 1,
            action: HotkeyAction::FocusNext,
        });
        self.bindings.push(Hotkey {
            modifiers: modifier::ALT | modifier::SHIFT, key: b'\t', taps: 1,
            action: HotkeyAction::FocusPrev,
        });
    }

    #[cfg(not(target_arch = "aarch64"))]
    fn parse_line(line: &[u8]) -> Option<Hotkey> {
        // Split on '='
        let eq_pos = line.iter().position(|&b| b == b'=')?;
        let lhs = trim(&line[..eq_pos]);
        let rhs = trim(&line[eq_pos + 1..]);
        if lhs.is_empty() || rhs.is_empty() { return None; }

        // Parse left side: modifier+modifier+key
        let mut modifiers = 0u8;
        let mut key = 0u8;
        let mut taps = 1u8;

        // Count how many times each modifier appears (for multi-tap)
        let mut super_count = 0u8;

        let parts: Vec<&[u8]> = lhs.split(|&b| b == b'+').collect();
        for part in &parts {
            let part = trim(part);
            if part.is_empty() { continue; }
            match &part.to_ascii_lowercase()[..] {
                b"super" | b"cmd" | b"command" | b"win" => {
                    modifiers |= modifier::SUPER;
                    super_count += 1;
                }
                b"alt" => { modifiers |= modifier::ALT; }
                b"ctrl" | b"control" => { modifiers |= modifier::CTRL; }
                b"shift" => { modifiers |= modifier::SHIFT; }
                b"return" | b"enter" => { key = b'\r'; }
                b"tab" => { key = b'\t'; }
                b"space" => { key = b' '; }
                b"backspace" => { key = 0x08; }
                b"escape" | b"esc" => { key = 0x1b; }
                k if k.len() == 1 => { key = k[0].to_ascii_lowercase(); }
                _ => {} // Unknown token, skip
            }
        }

        // Super+Super means double-tap Super (no key, modifier appears twice)
        if super_count >= 2 && key == 0 {
            taps = 2;
        }

        // Parse right side: action
        let action = if rhs.starts_with(b"exec ") || rhs.starts_with(b"exec\t") {
            let path = trim(&rhs[5..]);
            if path.is_empty() { return None; }
            HotkeyAction::Exec(path.to_vec())
        } else {
            match &rhs.to_ascii_lowercase()[..] {
                b"close" => HotkeyAction::Close,
                b"focus_next" => HotkeyAction::FocusNext,
                b"focus_prev" => HotkeyAction::FocusPrev,
                _ => return None,
            }
        };

        Some(Hotkey { modifiers, key, taps, action })
    }

    /// Called every frame with the current modifier bitmask and whether a
    /// non-modifier key was pressed this frame. Returns an action if a
    /// hotkey matched.
    ///
    /// `super_taps` is the count of Super press-edges latched in the kernel HID
    /// path since the previous frame (op=31, read-and-clear). Because the latch
    /// captures every 0→1 SUPER transition the instant the HID report arrives,
    /// it recovers taps whose entire ~30ms high window fell between two bursty
    /// compositor-wait polls — which the level-based edge detection would miss.
    /// SUPER tap counting is driven exclusively by this latch so each physical
    /// press is counted exactly once (no double-count vs. release detection).
    fn update(
        &mut self,
        current_mods: u8,
        key_pressed: Option<u8>,
        super_taps: u32,
    ) -> Option<HotkeyAction> {
        if self.cooldown > 0 {
            self.cooldown -= 1;
        }

        // Track if any non-modifier key was pressed while modifiers are held.
        // A combo (modifier + key) must NOT trigger the no-key double-tap launcher.
        if key_pressed.is_some() && current_mods != 0 {
            self.combo_used = true;
        }

        let prev = self.prev_modifiers;
        self.prev_modifiers = current_mods;

        // Check modifier+key bindings first (single-tap, key != 0)
        if let Some(ascii) = key_pressed {
            if self.cooldown == 0 {
                for binding in &self.bindings {
                    if binding.key != 0 && binding.taps == 1
                        && binding.key == ascii.to_ascii_lowercase()
                        && (current_mods & binding.modifiers) == binding.modifiers
                    {
                        self.cooldown = 30;
                        return Some(binding.action.clone());
                    }
                }
            }
        }

        // ── Super multi-tap detection driven by the kernel press-edge latch ──
        // Each latched press-edge is one physical tap. If the press arrived while
        // a combo was in progress (a non-modifier key was held with Super), the
        // tap is treated as dirty and resets the sequence rather than counting.
        if super_taps > 0 {
            for _ in 0..super_taps {
                if self.combo_used {
                    // Combo in progress: this Super press is part of a combo, not
                    // a clean tap. Reset the tap sequence; do not fire the launcher.
                    self.tap_count = 0;
                    self.tap_release_ns = 0;
                    continue;
                }

                self.tap_modifier = modifier::SUPER;

                let now_ns = match libbreenix::time::now_monotonic() {
                    Ok(ts) => ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64,
                    Err(_) => 0,
                };

                // Count this tap; continue a sequence only if the previous tap
                // was within the 400ms window, otherwise start a fresh sequence.
                if self.tap_count > 0
                    && now_ns.saturating_sub(self.tap_release_ns) < 400_000_000
                {
                    self.tap_count += 1;
                } else {
                    self.tap_count = 1;
                }
                self.tap_release_ns = now_ns;

                // Fire the matching multi-tap binding (e.g. double-tap Super).
                if self.cooldown == 0 {
                    for binding in &self.bindings {
                        if binding.key == 0
                            && binding.modifiers == modifier::SUPER
                            && binding.taps == self.tap_count
                        {
                            self.cooldown = 30;
                            self.tap_count = 0;
                            self.tap_release_ns = 0;
                            return Some(binding.action.clone());
                        }
                    }
                }
            }
        }

        // Reset combo tracking when Super is fully released so the next clean
        // tap sequence isn't suppressed by a stale combo flag.
        let super_was = (prev & modifier::SUPER) != 0;
        let super_now = (current_mods & modifier::SUPER) != 0;
        if !super_now && super_was {
            self.combo_used = false;
        }

        // ── Multi-tap detection for ALT / CTRL / SHIFT via level edges ──
        // (Super is handled above by the latch.) These modifiers are not affected
        // by the launcher drop bug; keep their existing release-edge behavior.
        for &mod_bit in &[modifier::ALT, modifier::CTRL, modifier::SHIFT] {
            let was = (prev & mod_bit) != 0;
            let now = (current_mods & mod_bit) != 0;

            if now && !was {
                if mod_bit != self.tap_modifier {
                    self.tap_modifier = mod_bit;
                    self.tap_count = 0;
                }
                self.combo_used = false;
            } else if !now && was {
                if mod_bit == self.tap_modifier && !self.combo_used {
                    let now_ns = match libbreenix::time::now_monotonic() {
                        Ok(ts) => ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64,
                        Err(_) => 0,
                    };

                    if self.tap_count > 0 && now_ns.saturating_sub(self.tap_release_ns) < 400_000_000 {
                        self.tap_count += 1;
                    } else {
                        self.tap_count = 1;
                    }
                    self.tap_release_ns = now_ns;

                    if self.cooldown == 0 {
                        for binding in &self.bindings {
                            if binding.key == 0
                                && binding.modifiers == mod_bit
                                && binding.taps == self.tap_count
                            {
                                self.cooldown = 30;
                                self.tap_count = 0;
                                self.tap_release_ns = 0;
                                return Some(binding.action.clone());
                            }
                        }
                    }
                } else if mod_bit == self.tap_modifier {
                    self.tap_count = 0;
                    self.tap_release_ns = 0;
                }
            }
        }

        None
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn trim(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| b != b' ' && b != b'\t' && b != b'\r').unwrap_or(s.len());
    let end = s.iter().rposition(|&b| b != b' ' && b != b'\t' && b != b'\r').map(|i| i + 1).unwrap_or(start);
    &s[start..end]
}

/// Read-and-clear the kernel's latched count of Super press-edges (FBDRAW op=31).
///
/// The kernel HID path increments a lock-free atomic on every 0→1 SUPER
/// transition the instant the report arrives, so a tap whose high window fell
/// entirely between two compositor-wait polls is still counted. This drains
/// that latch so missed taps reach the double-tap detector.
#[cfg(target_arch = "aarch64")]
fn take_super_tap_count() -> u32 {
    use libbreenix::graphics::FbDrawCmd;
    use libbreenix::syscall::nr;
    let cmd = FbDrawCmd {
        op: 31,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: 0,
        color: 0,
    };
    let ret =
        unsafe { libbreenix::raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    if ret < 0 {
        0
    } else {
        ret as u32
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn take_super_tap_count() -> u32 {
    0
}

// ─── Resize Edge ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

// ─── Window Defaults (Position/Size Memory) ─────────────────────────────────

struct WindowDefaults {
    title: [u8; 32],
    title_len: usize,
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    valid: bool,
}

fn save_window_defaults(win: &Window, defaults: &mut [WindowDefaults; MAX_DEFAULTS]) {
    let mut slot = None;
    for i in 0..MAX_DEFAULTS {
        if defaults[i].valid
            && defaults[i].title_len == win.title_len
            && defaults[i].title[..win.title_len] == win.title[..win.title_len]
        {
            slot = Some(i);
            break;
        }
    }
    if slot.is_none() {
        for i in 0..MAX_DEFAULTS {
            if !defaults[i].valid {
                slot = Some(i);
                break;
            }
        }
    }
    if let Some(i) = slot {
        defaults[i].title = win.title;
        defaults[i].title_len = win.title_len;
        defaults[i].x = win.x;
        defaults[i].y = win.y;
        defaults[i].width = win.width;
        defaults[i].height = win.height;
        defaults[i].valid = true;
    }
}

fn find_window_defaults(
    title: &[u8], title_len: usize, defaults: &[WindowDefaults; MAX_DEFAULTS],
) -> Option<(i32, i32, usize, usize)> {
    for d in defaults.iter() {
        if d.valid && d.title_len == title_len && d.title[..d.title_len] == title[..title_len] {
            return Some((d.x, d.y, d.width, d.height));
        }
    }
    None
}

// ─── Window ─────────────────────────────────────────────────────────────────

struct Window {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    title: [u8; 32],
    title_len: usize,
    window_id: u32,
    owner_pid: u32,
    minimized: bool,
    /// Stable ordering for appbar (assigned at discovery time, never changes)
    creation_order: u32,
    /// Chromeless windows have no title bar, border, or chrome buttons.
    /// Detected by title prefix \x01.
    chromeless: bool,
}

impl Window {
    fn content_x(&self) -> i32 {
        if self.chromeless { self.x } else { self.x + px(BORDER_WIDTH) as i32 }
    }
    fn content_y(&self) -> i32 {
        if self.chromeless { self.y } else { self.y + px(TITLE_BAR_HEIGHT) as i32 + px(BORDER_WIDTH) as i32 }
    }
    fn content_width(&self) -> usize {
        if self.chromeless { self.width } else { self.width.saturating_sub(px(BORDER_WIDTH) * 2) }
    }
    fn content_height(&self) -> usize {
        if self.chromeless { self.height } else { self.height.saturating_sub(px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2) }
    }

    fn total_height(&self) -> usize { self.height }

    fn hit_title(&self, mx: i32, my: i32) -> bool {
        if self.chromeless { return false; }
        mx >= self.x && mx < self.x + self.width as i32
            && my >= self.y && my < self.y + px(TITLE_BAR_HEIGHT) as i32
    }

    fn hit_any(&self, mx: i32, my: i32) -> bool {
        mx >= self.x && mx < self.x + self.width as i32
            && my >= self.y && my < self.y + self.total_height() as i32
    }

    fn hit_content(&self, mx: i32, my: i32) -> bool {
        mx >= self.content_x() && mx < self.content_x() + self.content_width() as i32
            && my >= self.content_y() && my < self.content_y() + self.content_height() as i32
    }

    fn bounds(&self) -> (i32, i32, i32, i32) {
        if self.chromeless {
            (self.x, self.y, self.x + self.width as i32, self.y + self.height as i32)
        } else {
            // The drop shadow is drawn SHADOW_OFFSET right of and below the frame (draw_window_frame)
            let shadow = px(SHADOW_OFFSET) as i32;
            (self.x, self.y, self.x + self.width as i32 + shadow, self.y + self.total_height() as i32 + shadow)
        }
    }

    fn close_btn_rect(&self) -> (i32, i32, usize, usize) {
        let bx = self.x + self.width as i32 - px(BORDER_WIDTH) as i32
            - px(CHROME_BTN_PAD) as i32 - px(CHROME_BTN_SIZE) as i32;
        let by = self.y + px(BORDER_WIDTH) as i32
            + ((px(TITLE_BAR_HEIGHT) - px(BORDER_WIDTH) - px(CHROME_BTN_SIZE)) / 2) as i32;
        (bx, by, px(CHROME_BTN_SIZE), px(CHROME_BTN_SIZE))
    }

    fn minimize_btn_rect(&self) -> (i32, i32, usize, usize) {
        let (cx, cy, _, _) = self.close_btn_rect();
        (cx - px(CHROME_BTN_PAD) as i32 - px(CHROME_BTN_SIZE) as i32, cy, px(CHROME_BTN_SIZE), px(CHROME_BTN_SIZE))
    }

    fn hit_close_button(&self, mx: i32, my: i32) -> bool {
        let (bx, by, bw, bh) = self.close_btn_rect();
        mx >= bx && mx < bx + bw as i32 && my >= by && my < by + bh as i32
    }

    fn hit_minimize_button(&self, mx: i32, my: i32) -> bool {
        let (bx, by, bw, bh) = self.minimize_btn_rect();
        mx >= bx && mx < bx + bw as i32 && my >= by && my < by + bh as i32
    }

    fn hit_resize_edge(&self, mx: i32, my: i32) -> Option<ResizeEdge> {
        if self.minimized || self.chromeless { return None; }

        let x0 = self.x;
        let y0 = self.y;
        let x1 = self.x + self.width as i32;
        let y1 = self.y + self.total_height() as i32;
        let g = px(RESIZE_GRAB) as i32;

        // Must be within or near the window bounds
        if mx < x0 - g || mx >= x1 + g || my < y0 - g || my >= y1 + g {
            return None;
        }

        let on_left = mx < x0 + g;
        let on_right = mx >= x1 - g;
        let on_top = my < y0 + g;
        let on_bottom = my >= y1 - g;

        match (on_left, on_right, on_top, on_bottom) {
            (true, _, true, _) => Some(ResizeEdge::TopLeft),
            (_, true, true, _) => Some(ResizeEdge::TopRight),
            (true, _, _, true) => Some(ResizeEdge::BottomLeft),
            (_, true, _, true) => Some(ResizeEdge::BottomRight),
            (true, _, _, _) => Some(ResizeEdge::Left),
            (_, true, _, _) => Some(ResizeEdge::Right),
            (_, _, true, _) => Some(ResizeEdge::Top),
            (_, _, _, true) => Some(ResizeEdge::Bottom),
            _ => None,
        }
    }
}

// ─── Drawing Helpers ─────────────────────────────────────────────────────────

fn fill_rect(fb: &mut FrameBuf, x: i32, y: i32, w: usize, h: usize, color: Color) {
    libgfx::shapes::fill_rect(fb, x, y, w as i32, h as i32, color);
}

fn draw_text_at(fb: &mut FrameBuf, text: &[u8], x: i32, y: i32, color: Color,
                ttf: Option<&mut CachedFont>) {
    if y < 0 || y >= fb.height as i32 { return; }
    if let Some(font) = ttf {
        // Use LCD subpixel rendering for crisp window chrome text
        let s = unsafe { core::str::from_utf8_unchecked(text) };
        ttf_font::draw_text_subpixel(fb, font, s, x, y, TTF_FONT_SIZE * ui().scale as f32, color);
    } else {
        let (cell_w, font) = (ui().cell_w as i32, ui().font);
        for (i, &ch) in text.iter().enumerate() {
            let cx = x + i as i32 * cell_w;
            if cx < 0 || cx + cell_w > fb.width as i32 { continue; }
            bitmap_font::draw_char_at(fb, ch as char, cx as usize, y as usize, color, font);
        }
    }
}

/// Draw a floating window frame (border + title bar + chrome buttons + content bg)
fn draw_window_frame(fb: &mut FrameBuf, win: &Window, focused: bool,
                     ui_font: &mut Option<CachedFont>) {
    let border_color = if focused { WIN_BORDER_FOCUSED } else { WIN_BORDER_COLOR };
    let title_bg = if focused { TITLE_FOCUSED_BG } else { TITLE_UNFOCUSED_BG };
    let title_fg = if focused { TITLE_FOCUSED_TEXT } else { TITLE_TEXT };
    let bw = px(BORDER_WIDTH);

    let shadow = Color::rgb(8, 10, 18);
    let offset = px(SHADOW_OFFSET) as i32;
    fill_rect(fb, win.x + offset, win.y + offset, win.width, win.total_height(), shadow);
    fill_rect(fb, win.x, win.y, win.width, win.total_height(), border_color);
    let content_bg = Color::rgb(12, 14, 20);
    fill_rect(fb, win.content_x(), win.content_y(),
              win.content_width(), win.content_height(), content_bg);
    fill_rect(fb, win.x + bw as i32, win.y + bw as i32,
              win.width - bw * 2, px(TITLE_BAR_HEIGHT) - bw, title_bg);

    let max_title_w = win.width.saturating_sub(bw * 2 + px(8) + px(CHROME_RESERVED));
    let max_chars = max_title_w / ui().cell_w;
    let text_len = win.title_len.min(max_chars);
    let text_y = win.y + bw as i32 + ((px(TITLE_BAR_HEIGHT) - bw - ui().cell_h) / 2) as i32;
    draw_text_at(fb, &win.title[..text_len], win.x + pxi(8), text_y, title_fg,
                 ui_font.as_mut());

    let (cbx, cby, cbw, cbh) = win.close_btn_rect();
    fill_rect(fb, cbx, cby, cbw, cbh, CLOSE_BTN_BG);
    let cx = cbx + (cbw as i32 - ui().cell_w as i32) / 2;
    let cy = cby + (cbh as i32 - ui().cell_h as i32) / 2;
    draw_text_at(fb, b"x", cx, cy, CLOSE_BTN_TEXT, ui_font.as_mut());

    let (mbx, mby, mbw, mbh) = win.minimize_btn_rect();
    fill_rect(fb, mbx, mby, mbw, mbh, MINIMIZE_BTN_BG);
    let mx = mbx + (mbw as i32 - ui().cell_w as i32) / 2;
    let my = mby + (mbh as i32 - ui().cell_h as i32) / 2;
    draw_text_at(fb, b"-", mx, my, MINIMIZE_BTN_TEXT, ui_font.as_mut());
}


/// Paint the decorative desktop background — gradient with grid
fn paint_background(fb: &mut FrameBuf) {
    let w = fb.width;
    let h = fb.height;
    let step = px(64);
    for y in 0..h {
        for x in 0..w {
            let t = (y * 255 / h) as u8;
            let r = 12u8.saturating_add(t / 12);
            let g = 16u8.saturating_add(t / 20);
            let b = 38u8.saturating_add(t / 6);
            let grid = (x % step == 0 || y % step == 0) as u8;
            let r2 = r.saturating_add(grid * 6);
            let g2 = g.saturating_add(grid * 8);
            let b2 = b.saturating_add(grid * 12);
            fb.put_pixel(x, y, Color::rgb(r2, g2, b2));
        }
    }
}

// ─── Taskbar & App Bar ──────────────────────────────────────────────────────

/// US Eastern Time offset from UTC in seconds.
/// DST (EDT, UTC-4) runs from 2nd Sunday of March to 1st Sunday of November.
/// Standard (EST, UTC-5) applies the rest of the year.
fn eastern_offset_secs(utc_secs: i64) -> i64 {
    // Compute day-of-year, month, day-of-week from Unix timestamp
    let days = (utc_secs / 86400) as i64;

    // Compute year/month/day
    let mut y = 1970i64;
    let mut rem = days;
    loop {
        let ydays = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if rem < ydays { break; }
        rem -= ydays;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays: [i64; 12] = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0u8;
    for i in 0..12 {
        if rem < mdays[i] { month = i as u8 + 1; break; }
        rem -= mdays[i];
    }
    let day = rem as u8 + 1;

    // 2nd Sunday of March: find first Sunday in March, add 7
    // 1st Sunday of November: find first Sunday in November
    // Check month boundaries for DST transitions
    // DST starts: 2nd Sunday of March at 2:00 AM EST (7:00 AM UTC)
    // DST ends: 1st Sunday of November at 2:00 AM EDT (6:00 AM UTC)
    let hour_utc = ((utc_secs % 86400) / 3600) as u8;

    // March: find day-of-week of March 1
    let jan1_dow = {
        let mut d = 0i64;
        for yr in 1970..y {
            d += if yr % 4 == 0 && (yr % 100 != 0 || yr % 400 == 0) { 366 } else { 365 };
        }
        ((d % 7 + 4) % 7) as u8 // 0=Sun
    };
    // Day of week of March 1 = (jan1_dow + 31 + feb_days) % 7
    let feb = if leap { 29 } else { 28 };
    let mar1_dow = (jan1_dow + 31 + feb) % 7;
    let dst_start_day = if mar1_dow == 0 { 8u8 } else { (14 - mar1_dow + 1) as u8 }; // 2nd Sunday
    // Day of week of Nov 1
    let days_to_nov1: u16 = [31u16, feb as u16, 31, 30, 31, 30, 31, 31, 30, 31].iter().sum();
    let nov1_dow = (jan1_dow as u16 + days_to_nov1) % 7;
    let dst_end_day = if nov1_dow == 0 { 1u8 } else { 7 - nov1_dow as u8 + 1 };

    let is_dst = match month {
        1..=2 => false,
        4..=10 => true,
        3 => day > dst_start_day || (day == dst_start_day && hour_utc >= 7),
        11 => day < dst_end_day || (day == dst_end_day && hour_utc < 6),
        12 => false,
        _ => false,
    };

    if is_dst { -4 * 3600 } else { -5 * 3600 }
}

fn format_clock(utc_secs: i64, buf: &mut [u8; 11]) {
    let offset = eastern_offset_secs(utc_secs);
    let local = utc_secs + offset;
    // Handle day wrap
    let t = ((local % 86400) + 86400) % 86400;
    let s = (t % 60) as u8;
    let m = ((t / 60) % 60) as u8;
    let h = ((t / 3600) % 24) as u8;
    buf[0] = b'0' + h / 10;
    buf[1] = b'0' + h % 10;
    buf[2] = b':';
    buf[3] = b'0' + m / 10;
    buf[4] = b'0' + m % 10;
    buf[5] = b':';
    buf[6] = b'0' + s / 10;
    buf[7] = b'0' + s % 10;
    buf[8] = b' ';
    buf[9] = b'E';
    buf[10] = b'T';
}

fn draw_taskbar(fb: &mut FrameBuf, clock_text: &[u8], ui_font: &mut Option<CachedFont>) {
    let w = fb.width;
    fill_rect(fb, 0, 0, w, px(TASKBAR_HEIGHT), TASKBAR_BG);
    let label_y = ((px(TASKBAR_HEIGHT) - ui().cell_h) / 2 + px(1)) as i32;
    draw_text_at(fb, b"Breenix", pxi(8), label_y, TASKBAR_TEXT, ui_font.as_mut());
    if !clock_text.is_empty() {
        let clock_x = w as i32 - (clock_text.len() as i32 * ui().cell_w as i32) - pxi(8);
        draw_text_at(fb, clock_text, clock_x, label_y, TASKBAR_TEXT, ui_font.as_mut());
    }
}

fn appbar_button_width(title_len: usize) -> usize {
    let text_w = title_len * ui().cell_w + px(16);
    text_w.max(px(60)).min(px(180))
}

/// Build indices sorted by creation_order (stable appbar layout).
fn sorted_by_creation(windows: &[Window]) -> ([usize; 16], usize) {
    let n = windows.len().min(16);
    let mut idx = [0usize; 16];
    for i in 0..n { idx[i] = i; }
    // Insertion sort (at most 16 elements)
    for i in 1..n {
        let mut j = i;
        while j > 0 && windows[idx[j]].creation_order < windows[idx[j - 1]].creation_order {
            idx.swap(j, j - 1);
            j -= 1;
        }
    }
    (idx, n)
}

fn draw_appbar(fb: &mut FrameBuf, windows: &[Window], focused_win: usize,
               ui_font: &mut Option<CachedFont>) {
    let screen_h = fb.height;
    let screen_w = fb.width;
    let bar_y = (screen_h - px(APPBAR_HEIGHT)) as i32;

    fill_rect(fb, 0, bar_y, screen_w, px(APPBAR_HEIGHT), APPBAR_BG);
    fill_rect(fb, 0, bar_y, screen_w, px(1), APPBAR_BORDER);

    let (sorted, n) = sorted_by_creation(windows);
    let mut btn_x: i32 = pxi(4);
    let btn_h: usize = px(APPBAR_HEIGHT) - px(8);
    let btn_y = bar_y + pxi(4);

    for k in 0..n {
        let i = sorted[k];
        let win = &windows[i];
        if win.chromeless { continue; }
        let btn_w = appbar_button_width(win.title_len);

        let bg = if i == focused_win && !win.minimized {
            APPBAR_BTN_FOCUSED
        } else if win.minimized {
            APPBAR_BTN_MINIMIZED
        } else {
            APPBAR_BTN_BG
        };

        fill_rect(fb, btn_x, btn_y, btn_w, btn_h, bg);

        let max_chars = (btn_w.saturating_sub(px(12))) / ui().cell_w;
        let text_len = win.title_len.min(max_chars);
        let text_y = btn_y + ((btn_h - ui().cell_h) / 2 + px(1)) as i32;
        draw_text_at(fb, &win.title[..text_len], btn_x + pxi(6), text_y, APPBAR_BTN_TEXT,
                     ui_font.as_mut());

        btn_x += btn_w as i32 + pxi(2);
        if btn_x >= screen_w as i32 - pxi(4) { break; }
    }
}

fn appbar_hit_test(windows: &[Window], screen_w: usize, screen_h: usize, mx: i32, my: i32) -> Option<usize> {
    let bar_y = (screen_h - px(APPBAR_HEIGHT)) as i32;
    if my < bar_y || my >= screen_h as i32 { return None; }

    let (sorted, n) = sorted_by_creation(windows);
    let mut btn_x: i32 = pxi(4);
    let btn_h = (px(APPBAR_HEIGHT) - px(8)) as i32;
    let btn_y = bar_y + pxi(4);

    for k in 0..n {
        let i = sorted[k];
        if windows[i].chromeless { continue; }
        let btn_w = appbar_button_width(windows[i].title_len) as i32;
        if mx >= btn_x && mx < btn_x + btn_w
            && my >= btn_y && my < btn_y + btn_h
        {
            return Some(i); // Returns the actual Vec index, not the sorted position
        }
        btn_x += btn_w + pxi(2);
        if btn_x >= screen_w as i32 - pxi(4) { break; }
    }
    None
}

fn next_visible_window(windows: &[Window], current: usize) -> usize {
    for i in (0..windows.len()).rev() {
        if !windows[i].minimized {
            return i;
        }
    }
    current
}

// ─── Input Routing ──────────────────────────────────────────────────────────

fn route_keyboard_to_focused(windows: &[Window], focused_win: usize, event: &WindowInputEvent) {
    if focused_win < windows.len() && windows[focused_win].window_id != 0 {
        let _ = graphics::write_window_input(windows[focused_win].window_id, event);
    }
}

fn send_focus_event(windows: &[Window], win_idx: usize, event_type: u16) {
    if win_idx < windows.len() && windows[win_idx].window_id != 0 {
        let event = WindowInputEvent {
            event_type,
            keycode: 0, mouse_x: 0, mouse_y: 0, modifiers: 0, scroll_y: 0,
        };
        let _ = graphics::write_window_input(windows[win_idx].window_id, &event);
    }
}

fn route_mouse_button_to_focused(
    windows: &[Window], focused_win: usize,
    button: u16, pressed: bool, win_local_x: i16, win_local_y: i16,
) {
    if focused_win < windows.len() && windows[focused_win].window_id != 0 {
        let event = WindowInputEvent {
            event_type: input_event_type::MOUSE_BUTTON,
            keycode: button,
            mouse_x: win_local_x,
            mouse_y: win_local_y,
            modifiers: 0,
            scroll_y: if pressed { 1 } else { 0 },
        };
        let _ = graphics::write_window_input(windows[focused_win].window_id, &event);
    }
}

fn route_mouse_move_to_focused(
    windows: &[Window], focused_win: usize,
    win_local_x: i16, win_local_y: i16,
) {
    if focused_win < windows.len() && windows[focused_win].window_id != 0 {
        let event = WindowInputEvent {
            event_type: input_event_type::MOUSE_MOVE,
            keycode: 0,
            mouse_x: win_local_x, mouse_y: win_local_y,
            modifiers: 0, scroll_y: 0,
        };
        let _ = graphics::write_window_input(windows[focused_win].window_id, &event);
    }
}

fn route_mouse_scroll_to_focused(
    windows: &[Window], focused_win: usize,
    win_local_x: i16, win_local_y: i16, scroll_delta: i16,
) {
    if focused_win < windows.len() && windows[focused_win].window_id != 0 {
        let event = WindowInputEvent {
            event_type: input_event_type::MOUSE_SCROLL,
            keycode: 0,
            mouse_x: win_local_x,
            mouse_y: win_local_y,
            modifiers: 0,
            scroll_y: scroll_delta,
        };
        let _ = graphics::write_window_input(windows[focused_win].window_id, &event);
    }
}

// ─── Window Discovery ───────────────────────────────────────────────────────

fn discover_windows(
    windows: &mut Vec<Window>, screen_w: usize, screen_h: usize,
    next_order: &mut u32, win_defaults: &mut [WindowDefaults; MAX_DEFAULTS],
) -> bool {
    let mut win_infos = [graphics::WindowInfo {
        buffer_id: 0, owner_pid: 0, width: 0, height: 0,
        x: 0, y: 0, title_len: 0, title: [0; 64],
    }; 16];
    let count = match graphics::list_windows(&mut win_infos) {
        Ok(c) => c as usize,
        Err(_) => return false,
    };

    // Save defaults for windows about to be removed
    for w in windows.iter() {
        if !win_infos[..count].iter().any(|info| info.buffer_id == w.window_id) {
            save_window_defaults(w, win_defaults);
        }
    }

    let before = windows.len();
    windows.retain(|w| {
        win_infos[..count].iter().any(|info| info.buffer_id == w.window_id)
    });
    let removed = before > windows.len();

    let mut added = false;
    for i in 0..count {
        let info = &win_infos[i];
        if info.buffer_id == 0 { continue; }
        if windows.iter().any(|w| w.window_id == info.buffer_id) { continue; }

        let n = windows.len();
        let usable_h = screen_h.saturating_sub(px(TASKBAR_HEIGHT) + px(APPBAR_HEIGHT));
        // Cascade margins and steps are chrome (scaled); the room left for the window is
        // in client pixels.
        let cascade_x = pxi(30) + (n as i32 * pxi(50)) % ((screen_w as i32 - 500).max(100));
        let cascade_y = px(TASKBAR_HEIGHT) as i32 + pxi(10)
            + (n as i32 * pxi(50)) % ((usable_h as i32 - 500).max(100));

        let mut title = [0u8; 32];
        let title_len = (info.title_len as usize).min(32);
        title[..title_len].copy_from_slice(&info.title[..title_len]);

        // Chromeless windows (title prefix \x01) have no frame/chrome
        let chromeless = title_len > 0 && title[0] == 1;

        let total_w = if chromeless {
            info.width as usize
        } else {
            info.width as usize + px(BORDER_WIDTH) * 2
        };
        let total_h = if chromeless {
            info.height as usize
        } else {
            info.height as usize + px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2
        };

        // Use saved defaults if available, otherwise cascade (or origin for fullscreen chromeless)
        let (win_x, win_y, win_w, win_h) = match find_window_defaults(
            &title[..title_len], title_len, win_defaults,
        ) {
            Some((dx, dy, dw, dh)) => (dx, dy, dw, dh),
            None => {
                if chromeless && total_w >= screen_w && total_h >= screen_h {
                    (0i32, 0i32, total_w, total_h)
                } else if chromeless {
                    // Center small chromeless windows on screen
                    let cx = (screen_w as i32 - total_w as i32) / 2;
                    let cy = screen_h as i32 / 5; // Spotlight-style: 1/5 from top
                    (cx.max(0), cy.max(0), total_w, total_h)
                } else {
                    (cascade_x, cascade_y, total_w, total_h)
                }
            }
        };

        print!("[bwm] Discovered window '{}' (id={}, {}x{}{}) at ({},{})\n",
            core::str::from_utf8(&title[..title_len]).unwrap_or("?"),
            info.buffer_id, info.width, info.height,
            if chromeless { " chromeless" } else { "" },
            win_x, win_y);

        let (content_x, content_y) = if chromeless {
            (win_x, win_y)
        } else {
            (win_x + px(BORDER_WIDTH) as i32, win_y + px(TITLE_BAR_HEIGHT) as i32 + px(BORDER_WIDTH) as i32)
        };
        let z_order = windows.len() as u32;
        let _ = graphics::set_window_position(info.buffer_id, content_x, content_y, z_order);

        // If saved defaults give different content dimensions, notify client
        if !chromeless {
            let default_cw = win_w.saturating_sub(px(BORDER_WIDTH) * 2);
            let default_ch = win_h.saturating_sub(px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2);
            if default_cw != info.width as usize || default_ch != info.height as usize {
                let resize_event = WindowInputEvent {
                    event_type: input_event_type::WINDOW_RESIZED,
                    keycode: default_cw as u16,
                    mouse_x: default_ch as i16,
                    mouse_y: 0,
                    modifiers: 0,
                    scroll_y: 0,
                };
                let _ = graphics::write_window_input(info.buffer_id, &resize_event);
            }
        }

        let order = *next_order;
        *next_order += 1;
        windows.push(Window {
            x: win_x, y: win_y, width: win_w, height: win_h,
            title, title_len, window_id: info.buffer_id,
            owner_pid: info.owner_pid,
            minimized: false,
            creation_order: order,
            chromeless,
        });
        added = true;
    }

    removed || added
}

/// Update kernel z-order for all windows. Called after any z-order change
/// (raise-to-front, new window, etc.) so the GPU compositor draws quads
/// in correct back-to-front order.
fn update_kernel_z_order(windows: &[Window]) {
    for (i, win) in windows.iter().enumerate() {
        if win.window_id != 0 {
            let _ = graphics::set_window_position(win.window_id, win.content_x(), win.content_y(), i as u32);
        }
    }
}

/// Redraw all windows in z-order (index 0 = bottom), plus taskbar and app bar.
/// Window frames and decorations go into the compositor buffer; GPU compositing
/// handles client content via per-window textured quads.
fn redraw_all_windows(fb: &mut FrameBuf, windows: &[Window], focused_win: usize,
                      clock_text: &[u8], ui_font: &mut Option<CachedFont>) {
    draw_taskbar(fb, clock_text, ui_font);
    for i in 0..windows.len() {
        if windows[i].minimized || windows[i].chromeless { continue; }
        draw_window_frame(fb, &windows[i], i == focused_win, ui_font);
    }
    draw_appbar(fb, windows, focused_win, ui_font);
}

fn restore_full_background(vram: &mut [u32], fb: &mut FrameBuf, bg: &[u32]) {
    if bg.is_empty() {
        paint_background(fb);
    } else {
        vram.copy_from_slice(bg);
    }
}

/// Compose a full-redraw into `vram` without flashing.
///
/// When a shadow buffer is available (SVGA3 direct-mapped VRAM), composes into
/// the shadow first, then bulk-copies to VRAM so the display update is atomic.
/// Otherwise, composes directly into the primary buffer.
fn compose_full_redraw(
    vram: &mut [u32],
    fb: &mut FrameBuf,
    shadow: &mut Option<(&mut [u32], FrameBuf)>,
    bg: &[u32],
    windows: &[Window],
    focused: usize,
    clock: &[u8],
    ui_font: &mut Option<CachedFont>,
) {
    if let Some((ref mut sbuf, ref mut sfb)) = shadow {
        restore_full_background(sbuf, sfb, bg);
        redraw_all_windows(sfb, windows, focused, clock, ui_font);
        vram.copy_from_slice(sbuf);
    } else {
        restore_full_background(vram, fb, bg);
        redraw_all_windows(fb, windows, focused, clock, ui_font);
    }
}

/// Partial redraw: only update a dirty sub-region of the screen.
///
/// Used during drag to avoid full-screen VRAM copies. On SVGA3 (VMware),
/// VRAM is uncacheable so writing 9.2MB per frame kills drag performance.
/// Partial redraw limits VRAM writes to just the union of old+new window bounds.
fn compose_partial_redraw(
    vram: &mut [u32],
    fb: &mut FrameBuf,
    shadow: &mut Option<(&mut [u32], FrameBuf)>,
    bg: &[u32],
    windows: &[Window],
    focused: usize,
    clock: &[u8],
    dx0: usize, dy0: usize, dx1: usize, dy1: usize,
    ui_font: &mut Option<CachedFont>,
) {
    let screen_w = fb.width;
    let screen_h = fb.height;
    let dx1 = dx1.min(screen_w);
    let dy1 = dy1.min(screen_h);
    if dx0 >= dx1 || dy0 >= dy1 { return; }
    if bg.is_empty() {
        compose_full_redraw(vram, fb, shadow, bg, windows, focused, clock, ui_font);
        return;
    }

    if let Some((ref mut sbuf, ref mut sfb)) = shadow {
        for row in dy0..dy1 {
            let start = row * screen_w + dx0;
            let end = row * screen_w + dx1;
            sbuf[start..end].copy_from_slice(&bg[start..end]);
        }
        if dy0 < px(TASKBAR_HEIGHT) {
            draw_taskbar(sfb, clock, ui_font);
        }
        for i in 0..windows.len() {
            if windows[i].minimized || windows[i].chromeless { continue; }
            let (wx0, wy0, wx1, wy1) = windows[i].bounds();
            if (wx1 as usize) > dx0 && (wx0 as usize) < dx1
                && (wy1 as usize) > dy0 && (wy0 as usize) < dy1
            {
                draw_window_frame(sfb, &windows[i], i == focused, ui_font);
            }
        }
        if dy1 > screen_h - px(APPBAR_HEIGHT) {
            draw_appbar(sfb, windows, focused, ui_font);
        }
        for row in dy0..dy1 {
            let start = row * screen_w + dx0;
            let end = row * screen_w + dx1;
            vram[start..end].copy_from_slice(&sbuf[start..end]);
        }
    } else {
        for row in dy0..dy1 {
            let start = row * screen_w + dx0;
            let end = row * screen_w + dx1;
            vram[start..end].copy_from_slice(&bg[start..end]);
        }
        if dy0 < px(TASKBAR_HEIGHT) {
            draw_taskbar(fb, clock, ui_font);
        }
        for i in 0..windows.len() {
            if windows[i].minimized || windows[i].chromeless { continue; }
            let (wx0, wy0, wx1, wy1) = windows[i].bounds();
            if (wx1 as usize) > dx0 && (wx0 as usize) < dx1
                && (wy1 as usize) > dy0 && (wy0 as usize) < dy1
            {
                draw_window_frame(fb, &windows[i], i == focused, ui_font);
            }
        }
        if dy1 > screen_h - px(APPBAR_HEIGHT) {
            draw_appbar(fb, windows, focused, ui_font);
        }
    }
}






/// The visible client windows as the compositor stacks them, bottom to top: each one's
/// frame and drop shadow (as `draw_window_frame` paints them) beneath its content.
fn frame_layers(windows: &[Window]) -> Vec<Layer> {
    windows
        .iter()
        .filter(|w| !w.minimized)
        .map(|w| {
            let (mut frame, mut frame_len) = ([[0i32; 4]; 3], 0);
            if !w.chromeless {
                let (x0, y0) = (w.x, w.y);
                let (x1, y1) = (w.x + w.width as i32, w.y + w.total_height() as i32);
                let d = px(SHADOW_OFFSET) as i32;
                frame = [[x0, y0, x1, y1], [x1, y0 + d, x1 + d, y1 + d], [x0 + d, y1, x1, y1 + d]];
                frame_len = 3;
            }
            Layer {
                window_id: w.window_id,
                frame,
                frame_len,
                content: [w.content_x(), w.content_y(), w.content_width() as i32, w.content_height() as i32],
            }
        })
        .collect()
}

// ─── Hotkey Action Execution ─────────────────────────────────────────────────

fn execute_hotkey_action(action: &HotkeyAction, windows: &mut Vec<Window>, focused_win: usize) {
    match action {
        HotkeyAction::Exec(path) => {
            // For blauncher, toggle: if already open, close it; otherwise spawn
            if path == b"/bin/blauncher" {
                let launcher_idx = windows.iter().position(|w|
                    w.chromeless && w.title_len > 1 && w.title[0] == 1
                );
                if let Some(idx) = launcher_idx {
                    let event = WindowInputEvent {
                        event_type: input_event_type::CLOSE_REQUESTED,
                        keycode: 0, mouse_x: 0, mouse_y: 0, modifiers: 0, scroll_y: 0,
                    };
                    let _ = graphics::write_window_input(windows[idx].window_id, &event);
                    return;
                }
            }
            // Spawn the program — build null-terminated path on stack
            let mut spawn_buf = [0u8; 256];
            let len = path.len().min(254);
            spawn_buf[..len].copy_from_slice(&path[..len]);
            spawn_buf[len] = 0;
            let _ = libbreenix::process::spawn(&spawn_buf[..len + 1]);
        }
        HotkeyAction::Close => {
            if !windows.is_empty() && focused_win < windows.len() {
                let event = WindowInputEvent {
                    event_type: input_event_type::CLOSE_REQUESTED,
                    keycode: 0, mouse_x: 0, mouse_y: 0, modifiers: 0, scroll_y: 0,
                };
                let _ = graphics::write_window_input(windows[focused_win].window_id, &event);
            }
        }
        HotkeyAction::FocusNext | HotkeyAction::FocusPrev => {
            // Focus cycling is handled in the main loop since it needs
            // mutable access to focused_win and redraw state.
            // The main loop checks for these after calling update().
        }
    }
}

/// Show that the desktop covers the whole scanout at the chosen scale. bwm draws the
/// desktop alone (no windows yet, and no pointer over the sampled pixels), then reads the
/// scanout itself back where the scale puts the edges of the taskbar and the app bar: the
/// top-left and bottom-right pixels of each bar, and the background pixel just past each.
/// The read-back's record must name a scanout of the composed size, the one MODES reports
/// bound, and the GPU core's current mode must have that size.
fn check_desktop_fills(gpu: &mut Compositor, desktop: &[u32], fb: &FrameBuf, mode_index: u32) -> Result<String, String> {
    gpu.compose(&[], None)?;
    let (w, h) = (fb.width as u32, fb.height as u32);
    let (tb, ab) = (px(TASKBAR_HEIGHT) as u32, px(APPBAR_HEIGHT) as u32);
    // (x, y, what the desktop buffer must hold there)
    let taskbar = |c: Color| c == TASKBAR_BG;
    let border = |c: Color| c == APPBAR_BORDER;
    let appbar = |c: Color| c == APPBAR_BG;
    let background = |c: Color| c != TASKBAR_BG && c != APPBAR_BORDER && c != APPBAR_BG;
    let points: [(u32, u32, &dyn Fn(Color) -> bool, &str); 6] = [
        (0, 0, &taskbar, "taskbar"),
        (w - 1, tb - 1, &taskbar, "taskbar"),
        (w - 1, tb, &background, "background"),
        (w - 1, h - ab - 1, &background, "background"),
        (0, h - ab, &border, "app bar border"),
        (w - 1, h - 1, &appbar, "app bar"),
    ];
    let mut samples = Vec::new();
    for (x, y, is, what) in points {
        if !is(fb.get_pixel(x as usize, y as usize)) {
            return Err(format!("bwm's desktop has no {} pixel at ({}, {})", what, x, y));
        }
        samples.push((x, y, desktop[(y * w + x) as usize]));
    }
    let shown = gpu.check_fills(&samples)?;
    let bound = compositor::scanout_resource()?;
    if bound != shown.resource {
        return Err(format!("MODES reports resource {} bound, the read-back showed {}", bound, shown.resource));
    }
    let (_, mode) = compositor::display()?;
    if (mode.width, mode.height) != (shown.width, shown.height) {
        return Err(format!("the GPU core's mode is {}x{}, the scanout {}x{}", mode.width, mode.height, shown.width, shown.height));
    }
    Ok(format!(
        "resource {} {}x{} bound (GPU core mode {}, {}x{}), composed {}x{}, UI scale {}; taskbar rows 0..{} at (0,0) and ({},{}), background at ({},{}); app bar rows {}..{} at (0,{}) and ({},{}), background at ({},{}); {} pixels exact",
        shown.resource, shown.width, shown.height, mode_index, mode.width, mode.height, w, h, ui().scale,
        tb, w - 1, tb - 1, w - 1, tb, h - ab, h, h - ab, w - 1, h - 1, w - 1, h - ab - 1, samples.len()
    ))
}

fn monotonic_ms() -> Option<u64> {
    libbreenix::time::now_monotonic()
        .ok()
        .map(|ts| ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000)
}

// ─── Main ────────────────────────────────────────────────────────────────────

fn main() {
    print!("[bwm] Breenix Window Manager starting... (v2-chromeless-skip)\n");

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

    // The desktop is the GPU core's display, whole: its current mode's size.
    let (mode_index, mode) = match compositor::display() {
        Ok(d) => d,
        Err(e) => {
            print!("[bwm] ERROR: GPU compositing required: {}\n", e);
            process::exit(1);
        }
    };
    let screen_w = mode.width as usize;
    let screen_h = mode.height as usize;
    let bpp = info.bytes_per_pixel as usize;
    let _ = UI.set(Ui::for_display(screen_w, screen_h));
    print!(
        "[bwm] UI scale {}: font {} px ({}x{} cells, text grid {}x{}), taskbar {} px, app bar {} px; the GPU core's mode {} ({}x{}) publishes scale {}\n",
        ui().scale, ui().font.val(), ui().cell_w, ui().cell_h, screen_w / ui().cell_w, screen_h / ui().cell_h,
        px(TASKBAR_HEIGHT), px(APPBAR_HEIGHT), mode_index, mode.width, mode.height, mode.scale
    );

    // The GPU core composes the desktop (`compositor`); there is no CPU-only fallback.
    let mut gpu = match Compositor::open(screen_w as u32, screen_h as u32, px(APPBAR_HEIGHT) as u32, ui().scale as u32) {
        Ok(c) if bpp == 4 => c,
        Ok(_) => {
            print!("[bwm] ERROR: GPU compositing required: the screen has {} bytes per pixel, not 4\n", bpp);
            process::exit(1);
        }
        Err(e) => {
            print!("[bwm] ERROR: GPU compositing required: {}\n", e);
            process::exit(1);
        }
    };
    print!("[bwm] GPU compositing mode (GPU core), display: {}x{}\n", screen_w, screen_h);

    // The desktop buffer: background, taskbar, window frames and app bar, drawn on the
    // CPU and uploaded to the compositor's desktop texture. BWM runs for the lifetime of
    // the OS, so the buffer is leaked into a &'static mut slice.
    let composite_buf: &'static mut [u32] = vec![0u32; screen_w * screen_h].leak();

    let mut fb = unsafe {
        FrameBuf::from_raw(
            composite_buf.as_mut_ptr() as *mut u8,
            screen_w, screen_h, screen_w * bpp, bpp, info.is_bgr(),
        )
    };

    // Redraws reach the screen through the compositor's upload of the desktop buffer
    // after they finish, so no shadow buffer is needed.
    let mut shadow_fb: Option<(&'static mut [u32], FrameBuf)> = None;

    // Avoid filesystem reads before the first ARM64 compositor submit. On
    // single-CPU Parallels, opening /etc/fonts.conf here can stall bwm before
    // it replaces the kernel proof draw. Bitmap chrome is enough for boot.
    #[cfg(target_arch = "aarch64")]
    let mut ui_font: Option<CachedFont> = {
        print!("[bwm] using bitmap font fallback for early boot\n");
        None
    };
    #[cfg(not(target_arch = "aarch64"))]
    let mut ui_font: Option<CachedFont> = {
        let config = FontConfig::load();
        let font = std::fs::read(&config.display_path).ok()
            .and_then(|data| Font::parse(&data).ok())
            .map(|f| CachedFont::new(f, 256));
        if font.is_some() {
            print!("[bwm] display font loaded: {}\n", config.display_path);
        } else {
            print!("[bwm] no display font, using bitmap fallback\n");
        }
        font
    };

    // Paint decorative background. ARM64 keeps no full-screen cache during
    // early boot; repainting avoids a multi-megabyte allocation before bwm's
    // first visible frame.
    paint_background(&mut fb);
    #[cfg(target_arch = "aarch64")]
    let bg_cache: &[u32] = &[];
    #[cfg(not(target_arch = "aarch64"))]
    let bg_cache_storage = composite_buf.to_vec();
    #[cfg(not(target_arch = "aarch64"))]
    let bg_cache: &[u32] = &bg_cache_storage;

    // Enter raw mode on stdin
    let mut orig_termios = libbreenix::termios::Termios::default();
    let _ = libbreenix::termios::tcgetattr(Fd::from_raw(0), &mut orig_termios);
    let mut raw = orig_termios;
    libbreenix::termios::cfmakeraw(&mut raw);
    let _ = libbreenix::termios::tcsetattr(Fd::from_raw(0), libbreenix::termios::TCSANOW, &raw);

    let mut windows: Vec<Window> = Vec::new();
    let mut focused_win: usize = 0;
    let mut input_parser = InputParser::new();
    let (mut mouse_x, mut mouse_y) = graphics::mouse_pos()
        .map(|(x, y)| (x as i32, y as i32))
        .unwrap_or((0, 0));
    let mut active_cursor_shape = graphics::cursor_shape::ARROW;
    let mut prev_buttons: u32 = 0;
    let mut dragging: Option<(usize, i32, i32)> = None;
    // Active resize: (win_idx, edge, anchor_x, anchor_y, orig_x, orig_y, orig_w, orig_h)
    let mut resizing: Option<(usize, ResizeEdge, i32, i32, i32, i32, usize, usize)> = None;
    let mut full_redraw = true;
    let mut content_dirty = false;
    let mut windows_dirty = false;
    let mut win_defaults: [WindowDefaults; MAX_DEFAULTS] = core::array::from_fn(|_| WindowDefaults {
        title: [0; 32], title_len: 0, x: 0, y: 0, width: 0, height: 0, valid: false,
    });

    // Clock state
    let mut last_clock_sec: i64 = -1;
    let mut clock_text = [0u8; 11];
    format_clock(0, &mut clock_text);
    let mut frame_counter: u32 = 0;
    let mut fps_frames_since_last: u32 = 0;
    let mut fps_last_ms = monotonic_ms().unwrap_or(0);
    let mut next_creation_order: u32 = 0;
    // Hotkey system: load config from /etc/hotkeys.conf
    let mut hotkey_mgr = HotkeyManager::new();
    #[cfg(target_arch = "aarch64")]
    {
        hotkey_mgr.load_defaults();
        print!("[bwm] hotkeys: using built-in defaults for early boot\n");
    }
    #[cfg(not(target_arch = "aarch64"))]
    hotkey_mgr.load_config("/etc/hotkeys.conf");

    // Initial composite: the desktop alone (background, taskbar and app bar), before any
    // client window is discovered. It is read back from the scanout and compared with the
    // desktop buffer (`compositor`).
    redraw_all_windows(&mut fb, &windows, focused_win, &clock_text, &mut ui_font);
    let first_pointer = Pointer { x: mouse_x, y: mouse_y, shape: active_cursor_shape };
    let first = gpu
        .upload_desktop(composite_buf, [0, 0, screen_w as u32, screen_h as u32])
        .and_then(|()| gpu.compose(&[], Some(first_pointer)))
        .and_then(|()| gpu.check(composite_buf, &[], Some(first_pointer)));
    match first {
        Ok(pixels) => print!("[bwm] first frame read back: {} pixels exact\n", pixels),
        Err(e) => print!("[bwm] first frame check FAILED: {}\n", e),
    }
    match check_desktop_fills(&mut gpu, composite_buf, &fb, mode_index) {
        Ok(line) => print!("[bwm] desktop fills the scanout: {}\n", line),
        Err(e) => print!("[bwm] desktop fill check FAILED: {}\n", e),
    }
    let mut client_frame_checked = false;
    let mut frame_failed = false;

    let mut read_buf = [0u8; 512];
    let mut poll_fds = [io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 }];

    // Registry generation tracking for compositor_wait
    let mut registry_gen: u32 = 0;

    // Initial window discovery (before entering event loop)
    if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order, &mut win_defaults) {
        focused_win = next_visible_window(&windows, 0);
        compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
        full_redraw = true;
    }

    loop {
        // ── 0. Block until something needs compositing ──
        // compositor_wait blocks in the kernel until: window dirty, mouse moved,
        // or registry changed. If bwm already has local work prepared (for
        // example the initial window discovery redraw), drain and present it
        // before entering the kernel wait.
        let ready = if full_redraw || content_dirty || windows_dirty {
            0
        } else {
            let (ready, new_reg_gen) =
                graphics::compositor_wait(0, registry_gen).unwrap_or((0, registry_gen));
            registry_gen = new_reg_gen;
            ready
        };

        // ── 0b. Poll modifier state and check hotkeys ──
        // Drain the kernel's latched Super press-edge count (op=31) every frame
        // — including frames where compositor_wait was skipped — so a tap that
        // completed between polls is fed into double-tap detection and the
        // keyboard-ready latch can't busy-loop compositor_wait.
        let super_taps = take_super_tap_count();
        let current_mods = graphics::poll_modifier_state() as u8;
        if let Some(action) = hotkey_mgr.update(current_mods, None, super_taps) {
            match &action {
                HotkeyAction::FocusNext => {
                    if !windows.is_empty() {
                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                        focused_win = next_visible_window(&windows, (focused_win + 1) % windows.len());
                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                        compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
                        full_redraw = true;
                    }
                }
                HotkeyAction::FocusPrev => {
                    if !windows.is_empty() {
                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                        focused_win = if focused_win == 0 { windows.len() - 1 } else { focused_win - 1 };
                        focused_win = next_visible_window(&windows, focused_win);
                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                        compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
                        full_redraw = true;
                    }
                }
                _ => execute_hotkey_action(&action, &mut windows, focused_win),
            }
        }

        // ── 1. Discover new/removed client windows (only when registry changed) ──
        if ready & graphics::COMPOSITOR_READY_REGISTRY != 0 {
            if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order, &mut win_defaults) {
                // Chromeless windows always float to top of z-order
                let chromeless_idx = windows.iter().position(|w| w.chromeless);
                if let Some(idx) = chromeless_idx {
                    if idx < windows.len() - 1 {
                        let win = windows.remove(idx);
                        windows.push(win);
                    }
                }
                update_kernel_z_order(&windows);
                focused_win = next_visible_window(&windows, 0);
                compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
                full_redraw = true;
            }

        }

        // ── 2. Poll stdin (non-blocking) — keyboard arrives via stdin, not kernel events ──
        poll_fds[0].revents = 0;
        let _ = io::poll(&mut poll_fds, 0);

        // ── 3. Process keyboard input ──
        if poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0 {
            if let Ok(n) = io::read(Fd::from_raw(0), &mut read_buf) {
                for i in 0..n {
                    if let Some(event) = input_parser.feed(read_buf[i]) {
                        match event {
                            InputEvent::FunctionKey(k) if (k as usize) <= windows.len() => {
                                let new_focus = (k - 1) as usize;
                                if new_focus < windows.len() && new_focus != focused_win {
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                    focused_win = new_focus;
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                                    compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
                                    full_redraw = true;
                                }
                            }
                            InputEvent::Key { ascii, keycode, modifiers } => {
                                if !windows.is_empty() {
                                    let win_event = WindowInputEvent {
                                        event_type: input_event_type::KEY_PRESS,
                                        keycode,
                                        mouse_x: ascii as i16,
                                        mouse_y: 0,
                                        modifiers,
                                        scroll_y: 0,
                                    };
                                    route_keyboard_to_focused(&windows, focused_win, &win_event);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Dirty rect tracking — initialized before mouse processing so drag
        // can expand the dirty region. Used by section 5 (client blit) and 6 (composite).
        let mut dirty_x0 = i32::MAX;
        let mut dirty_y0 = i32::MAX;
        let mut dirty_x1 = 0i32;
        let mut dirty_y1 = 0i32;
        let mut cursor_dirty = false;

        // ── 4. Process mouse input (only when mouse changed) ──
        let mut mouse_moved_this_frame = false;
        if ready & graphics::COMPOSITOR_READY_MOUSE != 0 {
            if let Ok((mx, my, buttons, scroll_delta)) = graphics::mouse_state_with_scroll() {
                let new_mx = mx as i32;
                let new_my = my as i32;
                let mouse_moved = new_mx != mouse_x || new_my != mouse_y;
                mouse_moved_this_frame = mouse_moved;

                if mouse_moved {
                    mouse_x = new_mx;
                    mouse_y = new_my;

                    if let Some((win_idx, off_x, off_y)) = dragging {
                        let new_x = mouse_x - off_x;
                        // Clamp drag to stay below taskbar
                        let new_y = (mouse_y - off_y).max(px(TASKBAR_HEIGHT) as i32);
                        if new_x != windows[win_idx].x || new_y != windows[win_idx].y {
                            // Capture old bounds before moving
                            let (ox0, oy0, ox1, oy1) = windows[win_idx].bounds();
                            windows[win_idx].x = new_x;
                            windows[win_idx].y = new_y;
                            // Update kernel window position for GPU compositing
                            if windows[win_idx].window_id != 0 {
                                let cx = windows[win_idx].content_x();
                                let cy = windows[win_idx].content_y();
                                let _ = graphics::set_window_position(windows[win_idx].window_id, cx, cy, win_idx as u32);
                            }
                            // Dirty region = union of old and new bounds
                            let (nx0, ny0, nx1, ny1) = windows[win_idx].bounds();
                            let dr_x0 = ox0.min(nx0).max(0) as usize;
                            let dr_y0 = oy0.min(ny0).max(0) as usize;
                            let dr_x1 = ox1.max(nx1) as usize;
                            let dr_y1 = oy1.max(ny1) as usize;
                            compose_partial_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, dr_x0, dr_y0, dr_x1, dr_y1, &mut ui_font);
                            // Use partial dirty rect instead of full_redraw
                            dirty_x0 = dirty_x0.min(dr_x0 as i32);
                            dirty_y0 = dirty_y0.min(dr_y0 as i32);
                            dirty_x1 = dirty_x1.max(dr_x1 as i32);
                            dirty_y1 = dirty_y1.max(dr_y1 as i32);
                            content_dirty = true;
                        }
                    } else if let Some((win_idx, edge, anchor_x, anchor_y, orig_x, orig_y, orig_w, orig_h)) = resizing {
                        let dx = mouse_x - anchor_x;
                        let dy = mouse_y - anchor_y;

                        let (mut new_x, mut new_y, mut new_w, mut new_h) =
                            (orig_x, orig_y, orig_w as i32, orig_h as i32);

                        match edge {
                            ResizeEdge::Right => { new_w += dx; }
                            ResizeEdge::Bottom => { new_h += dy; }
                            ResizeEdge::Left => { new_x += dx; new_w -= dx; }
                            ResizeEdge::Top => { new_y += dy; new_h -= dy; }
                            ResizeEdge::TopLeft => { new_x += dx; new_w -= dx; new_y += dy; new_h -= dy; }
                            ResizeEdge::TopRight => { new_w += dx; new_y += dy; new_h -= dy; }
                            ResizeEdge::BottomLeft => { new_x += dx; new_w -= dx; new_h += dy; }
                            ResizeEdge::BottomRight => { new_w += dx; new_h += dy; }
                        }

                        // Enforce minimums
                        if (new_w as usize) < px(MIN_WIN_WIDTH) {
                            if edge == ResizeEdge::Left || edge == ResizeEdge::TopLeft || edge == ResizeEdge::BottomLeft {
                                new_x = orig_x + orig_w as i32 - px(MIN_WIN_WIDTH) as i32;
                            }
                            new_w = px(MIN_WIN_WIDTH) as i32;
                        }
                        if (new_h as usize) < px(MIN_WIN_HEIGHT) {
                            if edge == ResizeEdge::Top || edge == ResizeEdge::TopLeft || edge == ResizeEdge::TopRight {
                                new_y = orig_y + orig_h as i32 - px(MIN_WIN_HEIGHT) as i32;
                            }
                            new_h = px(MIN_WIN_HEIGHT) as i32;
                        }

                        // Clamp top to taskbar
                        new_y = new_y.max(px(TASKBAR_HEIGHT) as i32);

                        let (ox0, oy0, ox1, oy1) = windows[win_idx].bounds();

                        // Update frame dimensions live (content stays at old size until release)
                        windows[win_idx].x = new_x;
                        windows[win_idx].y = new_y;
                        windows[win_idx].width = new_w as usize;
                        windows[win_idx].height = new_h as usize;

                        // Update kernel window position
                        if windows[win_idx].window_id != 0 {
                            let cx = windows[win_idx].content_x();
                            let cy = windows[win_idx].content_y();
                            let _ = graphics::set_window_position(
                                windows[win_idx].window_id, cx, cy, win_idx as u32,
                            );
                        }

                        // Send live resize event to client
                        let new_cw = windows[win_idx].content_width();
                        let new_ch = windows[win_idx].content_height();
                        if windows[win_idx].window_id != 0 {
                            let resize_event = WindowInputEvent {
                                event_type: input_event_type::WINDOW_RESIZED,
                                keycode: new_cw as u16,
                                mouse_x: new_ch as i16,
                                mouse_y: 0,
                                modifiers: 0,
                                scroll_y: 0,
                            };
                            let _ = graphics::write_window_input(
                                windows[win_idx].window_id, &resize_event,
                            );
                        }

                        // Dirty region = union of old and new bounds
                        let (nx0, ny0, nx1, ny1) = windows[win_idx].bounds();
                        let dr_x0 = ox0.min(nx0).max(0) as usize;
                        let dr_y0 = oy0.min(ny0).max(0) as usize;
                        let dr_x1 = ox1.max(nx1) as usize;
                        let dr_y1 = oy1.max(ny1) as usize;
                        compose_partial_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache,
                            &windows, focused_win, &clock_text, dr_x0, dr_y0, dr_x1, dr_y1, &mut ui_font);
                        dirty_x0 = dirty_x0.min(dr_x0 as i32);
                        dirty_y0 = dirty_y0.min(dr_y0 as i32);
                        dirty_x1 = dirty_x1.max(dr_x1 as i32);
                        dirty_y1 = dirty_y1.max(dr_y1 as i32);
                        content_dirty = true;
                    } else if !windows.is_empty() && focused_win < windows.len()
                        && !windows[focused_win].minimized
                        && windows[focused_win].hit_content(mouse_x, mouse_y)
                    {
                        let local_x = (mouse_x - windows[focused_win].content_x()) as i16;
                        let local_y = (mouse_y - windows[focused_win].content_y()) as i16;
                        route_mouse_move_to_focused(&windows, focused_win, local_x, local_y);
                    }

                    // Update cursor shape based on resize edge hover
                    if dragging.is_none() && resizing.is_none() {
                        let mut hover_shape = graphics::cursor_shape::ARROW;
                        for i in (0..windows.len()).rev() {
                            if windows[i].minimized { continue; }
                            if let Some(edge) = windows[i].hit_resize_edge(mouse_x, mouse_y) {
                                hover_shape = match edge {
                                    ResizeEdge::Top | ResizeEdge::Bottom => graphics::cursor_shape::RESIZE_NS,
                                    ResizeEdge::Left | ResizeEdge::Right => graphics::cursor_shape::RESIZE_EW,
                                    ResizeEdge::TopLeft | ResizeEdge::BottomRight => graphics::cursor_shape::RESIZE_NWSE,
                                    ResizeEdge::TopRight | ResizeEdge::BottomLeft => graphics::cursor_shape::RESIZE_NESW,
                                };
                                break;
                            }
                            if windows[i].hit_any(mouse_x, mouse_y) {
                                break; // Inside window but not on edge
                            }
                        }
                        cursor_dirty |= active_cursor_shape != hover_shape;
                        active_cursor_shape = hover_shape;
                    }
                }

                // Release: end drag or route release event.
                // Per-endpoint button tracking in the kernel prevents dual USB HID
                // endpoints from racing (one endpoint can't cancel the other's press).
                if (buttons & 1) == 0 && (prev_buttons & 1) != 0 {
                    if let Some((win_idx, _, _, _, _, _, orig_w, orig_h)) = resizing.take() {
                        // Reset cursor shape back to arrow
                        cursor_dirty |= active_cursor_shape != graphics::cursor_shape::ARROW;
                        active_cursor_shape = graphics::cursor_shape::ARROW;
                        // Resize ended — notify client of new content dimensions
                        let new_cw = windows[win_idx].content_width();
                        let new_ch = windows[win_idx].content_height();
                        let old_cw = orig_w.saturating_sub(px(BORDER_WIDTH) * 2);
                        let old_ch = orig_h.saturating_sub(px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2);
                        print!("[bwm] resize end: id={} old={}x{} new={}x{} frame={}x{}\n",
                            windows[win_idx].window_id, old_cw, old_ch, new_cw, new_ch,
                            windows[win_idx].width, windows[win_idx].height);
                        if (new_cw != old_cw || new_ch != old_ch)
                            && windows[win_idx].window_id != 0
                        {
                            let resize_event = WindowInputEvent {
                                event_type: input_event_type::WINDOW_RESIZED,
                                keycode: new_cw as u16,
                                mouse_x: new_ch as i16,
                                mouse_y: 0,
                                modifiers: 0,
                                scroll_y: 0,
                            };
                            let _ = graphics::write_window_input(
                                windows[win_idx].window_id, &resize_event,
                            );
                        }
                        save_window_defaults(&windows[win_idx], &mut win_defaults);
                    } else if dragging.is_some() {
                        let drag_idx = dragging.unwrap().0;
                        dragging = None;
                        save_window_defaults(&windows[drag_idx], &mut win_defaults);
                    } else if !windows.is_empty() && focused_win < windows.len()
                        && !windows[focused_win].minimized
                        && windows[focused_win].hit_content(mouse_x, mouse_y)
                    {
                        let local_x = (mouse_x - windows[focused_win].content_x()) as i16;
                        let local_y = (mouse_y - windows[focused_win].content_y()) as i16;
                        route_mouse_button_to_focused(&windows, focused_win, 1, false, local_x, local_y);
                    }
                }

                // Scroll wheel: forward to focused window content area
                if scroll_delta != 0 && !windows.is_empty() && focused_win < windows.len()
                    && !windows[focused_win].minimized
                    && windows[focused_win].window_id != 0
                {
                    let local_x = (mouse_x - windows[focused_win].content_x()) as i16;
                    let local_y = (mouse_y - windows[focused_win].content_y()) as i16;
                    route_mouse_scroll_to_focused(
                        &windows, focused_win, local_x, local_y, scroll_delta as i16,
                    );
                }

                // Click: focus change + drag or route click to client
                let new_click = (buttons & 1) != 0 && (prev_buttons & 1) == 0;
                prev_buttons = buttons;

                if new_click {
                    // Check taskbar area — ignore clicks
                    if mouse_y < px(TASKBAR_HEIGHT) as i32 {
                        // Taskbar click — no action
                    }
                    // Check appbar area
                    else if mouse_y >= (screen_h - px(APPBAR_HEIGHT)) as i32 {
                        if let Some(idx) = appbar_hit_test(&windows, screen_w, screen_h, mouse_x, mouse_y) {
                            if idx == focused_win && !windows[idx].minimized {
                                // Click focused window button → minimize
                                windows[idx].minimized = true;
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                focused_win = next_visible_window(&windows, focused_win);
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                            } else {
                                // Click unfocused/minimized → restore and focus
                                windows[idx].minimized = false;
                                // Bring to top of z-order
                                if idx < windows.len() - 1 {
                                    let win = windows.remove(idx);
                                    windows.push(win);
                                    update_kernel_z_order(&windows);
                                }
                                let top = windows.len() - 1;
                                if top != focused_win {
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                    focused_win = top;
                                    send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                                } else {
                                    focused_win = top;
                                }
                            }
                            compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
                            full_redraw = true;
                        }
                    }
                    // Check windows (existing logic with chrome button additions)
                    else if !windows.is_empty() {
                        let mut clicked_idx: Option<usize> = None;
                        let mut clicked_title = false;
                        let mut clicked_resize: Option<ResizeEdge> = None;
                        for i in (0..windows.len()).rev() {
                            if windows[i].minimized { continue; }
                            // Check resize edges first (they extend slightly outside)
                            if let Some(edge) = windows[i].hit_resize_edge(mouse_x, mouse_y) {
                                clicked_idx = Some(i);
                                clicked_resize = Some(edge);
                                break;
                            }
                            let ht = windows[i].hit_title(mouse_x, mouse_y);
                            let ha = windows[i].hit_any(mouse_x, mouse_y);
                            if ht || ha {
                                clicked_idx = Some(i);
                                clicked_title = ht;
                                break;
                            }
                        }
                        if let Some(ci) = clicked_idx {
                            let z_changed = ci < windows.len() - 1;
                            if z_changed {
                                let win = windows.remove(ci);
                                windows.push(win);
                                update_kernel_z_order(&windows);
                            }
                            let top = windows.len() - 1;
                            let focus_changed = top != focused_win;

                            if focus_changed {
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                focused_win = top;
                                send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                            } else {
                                focused_win = top;
                            }

                            if let Some(edge) = clicked_resize {
                                // Start resize — set cursor shape for the edge
                                let shape = match edge {
                                    ResizeEdge::Top | ResizeEdge::Bottom => graphics::cursor_shape::RESIZE_NS,
                                    ResizeEdge::Left | ResizeEdge::Right => graphics::cursor_shape::RESIZE_EW,
                                    ResizeEdge::TopLeft | ResizeEdge::BottomRight => graphics::cursor_shape::RESIZE_NWSE,
                                    ResizeEdge::TopRight | ResizeEdge::BottomLeft => graphics::cursor_shape::RESIZE_NESW,
                                };
                                cursor_dirty |= active_cursor_shape != shape;
                                active_cursor_shape = shape;
                                resizing = Some((
                                    top, edge,
                                    mouse_x, mouse_y,
                                    windows[top].x, windows[top].y,
                                    windows[top].width, windows[top].height,
                                ));
                            } else if clicked_title {
                                // Check chrome buttons before starting drag
                                if windows[top].hit_close_button(mouse_x, mouse_y) {
                                    let close_event = WindowInputEvent {
                                        event_type: input_event_type::CLOSE_REQUESTED,
                                        keycode: 0, mouse_x: 0, mouse_y: 0, modifiers: 0, scroll_y: 0,
                                    };
                                    let _ = graphics::write_window_input(windows[top].window_id, &close_event);
                                    if windows[top].owner_pid > 0 {
                                        let _ = signal::kill(windows[top].owner_pid as i32, signal::SIGTERM);
                                    }
                                } else if windows[top].hit_minimize_button(mouse_x, mouse_y) {
                                    windows[top].minimized = true;
                                    if focused_win == top {
                                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_LOST);
                                        focused_win = next_visible_window(&windows, focused_win);
                                        send_focus_event(&windows, focused_win, input_event_type::FOCUS_GAINED);
                                    }
                                    compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
                                    full_redraw = true;
                                } else {
                                    dragging = Some((top, mouse_x - windows[top].x, mouse_y - windows[top].y));
                                }
                            } else if windows[top].hit_content(mouse_x, mouse_y) {
                                let local_x = (mouse_x - windows[top].content_x()) as i16;
                                let local_y = (mouse_y - windows[top].content_y()) as i16;
                                route_mouse_button_to_focused(&windows, focused_win, 1, true, local_x, local_y);
                            }

                            // Full redraw for z-order or focus change
                            if !full_redraw && (z_changed || focus_changed) {
                                compose_full_redraw(composite_buf, &mut fb, &mut shadow_fb, &bg_cache, &windows, focused_win, &clock_text, &mut ui_font);
                                full_redraw = true;
                            }
                        }
                    }
                }
            }
        }

        // ── 5. Window content handled by GPU ──
        // Client frames are uploaded to per-window textures and drawn above their
        // frames in stacking order (section 6). No CPU blit needed. Mark windows_dirty
        // so a client frame is composed without re-uploading the desktop buffer
        // (which contains only the background and decorations).
        if ready & graphics::COMPOSITOR_READY_DIRTY != 0 {
            windows_dirty = true;
        }

        // ── 5b. Update clock (once per second) ──
        // Only check realtime every 30 frames (~5-6 checks/sec at 200 FPS)
        frame_counter = frame_counter.wrapping_add(1);
        fps_frames_since_last = fps_frames_since_last.wrapping_add(1);
        if let Some(now_ms) = monotonic_ms() {
            let elapsed_ms = now_ms.saturating_sub(fps_last_ms);
            if elapsed_ms >= 1000 {
                let instantaneous_fps =
                    fps_frames_since_last as u64 * 1000 / elapsed_ms.max(1);
                print!(
                    "[bwm-fps] frames_since_last={} elapsed_ms={} instantaneous_fps={}\n",
                    fps_frames_since_last, elapsed_ms, instantaneous_fps
                );
                fps_frames_since_last = 0;
                fps_last_ms = now_ms;
            }
        }
        if frame_counter % 30 == 0 {
            if let Ok(ts) = libbreenix::time::now_realtime() {
                if ts.tv_sec != last_clock_sec {
                    last_clock_sec = ts.tv_sec;
                    format_clock(ts.tv_sec, &mut clock_text);
                    draw_taskbar(&mut fb, &clock_text, &mut ui_font);
                    dirty_x0 = 0;
                    dirty_y0 = 0;
                    dirty_x1 = dirty_x1.max(screen_w as i32);
                    dirty_y1 = dirty_y1.max(px(TASKBAR_HEIGHT) as i32);
                    content_dirty = true;
                }
            }
        }

        // (hotkey cooldown is managed inside HotkeyManager::update)

        // ── 6. Composite on the GPU (only when something changed) ──
        if full_redraw || content_dirty || windows_dirty || mouse_moved_this_frame || cursor_dirty {
            let upload = if full_redraw {
                Some([0, 0, screen_w as u32, screen_h as u32])
            } else if content_dirty {
                let dx = dirty_x0.max(0);
                let dy = dirty_y0.max(0);
                let dw = (dirty_x1.min(screen_w as i32) - dx).max(0) as u32;
                let dh = (dirty_y1.min(screen_h as i32) - dy).max(0) as u32;
                Some([dx as u32, dy as u32, dw, dh])
            } else {
                None
            };
            let layers = frame_layers(&windows);
            let pointer = Some(Pointer { x: mouse_x, y: mouse_y, shape: active_cursor_shape });
            let frame = upload
                .map_or(Ok(()), |rect| gpu.upload_desktop(composite_buf, rect))
                .and_then(|()| gpu.refresh_windows())
                .and_then(|_| gpu.compose(&layers, pointer));
            full_redraw = false;
            content_dirty = false;
            windows_dirty = false;
            match frame {
                Ok(()) => {
                    frame_failed = false;
                    // The first frame that shows a client's pixels is read back once and
                    // compared with the frame bwm composed.
                    if !client_frame_checked {
                        if let Some(id) = gpu.shows_client(&layers) {
                            client_frame_checked = true;
                            match gpu.check(composite_buf, &layers, pointer) {
                                Ok(pixels) => print!(
                                    "[bwm] composited client frame: window {} on screen, {} pixels exact\n",
                                    id, pixels
                                ),
                                Err(e) => print!("[bwm] client frame check FAILED: window {}: {}\n", id, e),
                            }
                        }
                    }
                }
                Err(e) => {
                    // Start again with a fresh context and resources; the next frame
                    // uploads everything.
                    if !frame_failed {
                        print!("[bwm] GPU frame failed: {}\n", e);
                    }
                    frame_failed = true;
                    if let Err(e) = gpu.reopen() {
                        print!("[bwm] ERROR: GPU compositor could not be reopened: {}\n", e);
                        process::exit(1);
                    }
                    full_redraw = true;
                }
            }
        }
        // No sleep — compositor_wait handles blocking
    }
}
