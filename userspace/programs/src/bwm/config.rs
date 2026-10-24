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
        let sizes = [
            (2, bitmap_font::RasterHeight::Size32),
            (1, bitmap_font::RasterHeight::Size16),
        ];
        let uis = sizes.map(|(scale, font)| {
            let m = bitmap_font::metrics_at(font);
            Ui {
                scale,
                font,
                cell_w: m.char_width,
                cell_h: m.line_height(),
            }
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
        Ok(pixels) => print!(
            "[bwm] composited client frame: window {} on screen, {} pixels exact\n",
            id, pixels
        ),
        Err(CheckError::Moved(e)) | Err(CheckError::Failed(e)) => {
            print!("[bwm] client frame check FAILED: window {}: {}\n", id, e)
        }
    }
}

pub(super) fn start() -> State {
    print!("[bwm] Breenix Window Manager starting... (v2-chromeless-skip)\n");

    if let Err(e) = session::take_over_display() {
        print!("[bwm] WARNING: take_over_display failed: {}\n", e);
    }

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
    let _ = UI.set(Ui::for_display(screen_w, screen_h));
    print!(
        "[bwm] UI scale {}: font {} px ({}x{} cells, text grid {}x{}), taskbar {} px, app bar {} px; the GPU core's mode {} ({}x{}) publishes scale {}\n",
        ui().scale, ui().font.val(), ui().cell_w, ui().cell_h, screen_w / ui().cell_w, screen_h / ui().cell_h,
        px(TASKBAR_HEIGHT), px(APPBAR_HEIGHT), mode_index, mode.width, mode.height, mode.scale
    );

    // bwm composes the windows as the compositor (window op 17's claim) and display owner.
    let claim_epoch = match session::claim() {
        Ok(epoch) => {
            print!("[bwm] compositor claim: epoch {}\n", epoch);
            epoch
        }
        Err(e) => {
            print!(
                "[bwm] ERROR: GPU compositing required: the compositor claim failed: {}\n",
                e
            );
            process::exit(1);
        }
    };
    let scene_fd = match session::scene_fd() {
        Ok(fd) => fd,
        Err(e) => {
            print!(
                "[bwm] ERROR: GPU compositing required: the scene descriptor failed: {}\n",
                e
            );
            process::exit(1);
        }
    };

    // The GPU core composes the desktop (`compositor`); there is no CPU-only fallback.
    let mut gpu = match Compositor::open(
        screen_w as u32,
        screen_h as u32,
        px(APPBAR_HEIGHT) as u32,
        ui().scale as u32,
    ) {
        Ok(c) => c,
        Err(e) => {
            print!("[bwm] ERROR: GPU compositing required: {}\n", e);
            process::exit(1);
        }
    };
    print!(
        "[bwm] GPU compositing mode (GPU core), display: {}x{}\n",
        screen_w, screen_h
    );

    let settings = Settings::load();
    let _ = CONFIG_ERROR.set(settings.error);
    let mut ui_font = settings.load_font();
    // Layout and painting use independent caches of the same configured font.
    let _ = FONT_METRICS.set(std::sync::Mutex::new(settings.load_font()));
    let mut clock = clock::Clock::new(settings.frozen, settings.valid_test);

    // Paint the decorative background once; each desktop repaint copies it.
    paint_background(&mut gpu.background);

    // Enter raw mode on stdin
    let mut orig_termios = libbreenix::termios::Termios::default();
    let _ = libbreenix::termios::tcgetattr(Fd::from_raw(0), &mut orig_termios);
    let mut raw = orig_termios;
    libbreenix::termios::cfmakeraw(&mut raw);
    let _ = libbreenix::termios::tcsetattr(Fd::from_raw(0), libbreenix::termios::TCSANOW, &raw);

    let mut windows: Vec<Window> = Vec::new();
    let mut focused_win: usize = 0;
    let input_parser = InputParser::new();
    let (mouse_x, mouse_y) = session::mouse_pos()
        .map(|(x, y)| (x as i32, y as i32))
        .unwrap_or((0, 0));
    let active_cursor_shape = cursor_shape::ARROW;
    let prev_buttons: u32 = 0;
    let dragging: Option<(usize, i32, i32)> = None;
    // Active resize: (win_idx, edge, anchor_x, anchor_y, orig_x, orig_y, orig_w, orig_h)
    let resizing: Option<(usize, ResizeEdge, i32, i32, i32, i32, usize, usize)> = None;
    let mut full_redraw = true;
    let content_dirty = false;
    let windows_dirty = false;
    let mut win_defaults: [WindowDefaults; MAX_DEFAULTS] =
        core::array::from_fn(|_| WindowDefaults {
            title: [0; 64],
            title_len: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            valid: false,
        });

    let last_clock_sec = clock.wall_sec();
    let mut clock_text = [0u8; 12];
    clock.format(&mut clock_text);
    let mut next_creation_order: u32 = 0;
    let mut hotkey_mgr = HotkeyManager::new();
    hotkey_mgr.load_config("/etc/hotkeys.conf");

    // Initial composite: the desktop alone (background, taskbar and app bar), before any
    // client window is discovered. It is read back from the scanout and compared with the
    // desktop target and the pointer's bitmap (`compositor`).
    compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
    let first_pointer = Pointer {
        x: mouse_x,
        y: mouse_y,
        shape: active_cursor_shape,
    };
    let first = gpu
        .compose(&[], Some(first_pointer), &[])
        .map_err(|e| CheckError::Failed(e.to_string()))
        .and_then(|()| gpu.check(0, &[], Some(first_pointer)));
    match first {
        Ok(pixels) => print!("[bwm] first frame read back: {} pixels exact\n", pixels),
        Err(CheckError::Moved(e)) | Err(CheckError::Failed(e)) => {
            print!("[bwm] first frame check FAILED: {}\n", e)
        }
    }
    let fills = check_desktop_fills(&mut gpu, screen_w, screen_h, mode_index);
    clock.presented(gpu.last_serial);
    clock.acknowledge(claim_epoch);
    match fills {
        Ok(line) => print!("[bwm] desktop fills the scanout: {}\n", line),
        Err(e) => print!("[bwm] desktop fill check FAILED: {}\n", e),
    }
    let frame_failed = false;
    // Another process holds the display: bwm composes again when it has it back.
    let display_lost = false;
    // The pointer the last composed frame drew, so a move repaints where it was.
    let drawn_pointer: Option<Pointer> = None;

    let read_buf = [0u8; 512];
    let poll_fds = [
        io::PollFd {
            fd: 0,
            events: io::poll_events::POLLIN as i16,
            revents: 0,
        },
        io::PollFd {
            fd: scene_fd,
            events: io::poll_events::POLLIN as i16,
            revents: 0,
        },
    ];

    // Initial window discovery (before entering event loop)
    if discover_windows(
        &mut windows,
        screen_w,
        screen_h,
        &mut next_creation_order,
        &mut win_defaults,
    ) {
        focused_win = next_visible_window(&windows, 0);
        compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
        full_redraw = true;
    }

    State {
        screen_w,
        screen_h,
        scene_fd,
        gpu,
        ui_font,
        windows,
        focused_win,
        input_parser,
        mouse_x,
        mouse_y,
        active_cursor_shape,
        prev_buttons,
        dragging,
        resizing,
        full_redraw,
        content_dirty,
        windows_dirty,
        win_defaults,
        last_clock_sec,
        clock_text,
        clock,
        claim_epoch,
        next_creation_order,
        hotkey_mgr,
        frame_failed,
        display_lost,
        drawn_pointer,
        read_buf,
        poll_fds,
    }
}

pub(super) struct State {
    pub(super) screen_w: usize,
    pub(super) screen_h: usize,
    pub(super) scene_fd: i32,
    pub(super) gpu: Compositor,
    pub(super) ui_font: Option<CachedFont>,
    pub(super) windows: Vec<Window>,
    pub(super) focused_win: usize,
    pub(super) input_parser: InputParser,
    pub(super) mouse_x: i32,
    pub(super) mouse_y: i32,
    pub(super) active_cursor_shape: u32,
    pub(super) prev_buttons: u32,
    pub(super) dragging: Option<(usize, i32, i32)>,
    pub(super) resizing: Option<(usize, ResizeEdge, i32, i32, i32, i32, usize, usize)>,
    pub(super) full_redraw: bool,
    pub(super) content_dirty: bool,
    pub(super) windows_dirty: bool,
    pub(super) win_defaults: [WindowDefaults; MAX_DEFAULTS],
    pub(super) last_clock_sec: i64,
    pub(super) clock_text: [u8; 12],
    pub(super) clock: clock::Clock,
    pub(super) claim_epoch: u64,
    pub(super) next_creation_order: u32,
    pub(super) hotkey_mgr: HotkeyManager,
    pub(super) frame_failed: bool,
    pub(super) display_lost: bool,
    pub(super) drawn_pointer: Option<Pointer>,
    pub(super) read_buf: [u8; 512],
    pub(super) poll_fds: [io::PollFd; 2],
}

impl State {
    pub(super) fn wait(&mut self) -> (bool, bool) {
        flush_input();
        // ── 0. Sleep until input or a scene change ──
        // The scene descriptor is readable when a window registered, committed, moved or
        // went away. The pointer has no descriptor, so the sleep is bounded and the
        // pointer is read after it. Local work already prepared (the initial discovery's
        // redraw) is presented before sleeping.
        let timeout = if self.display_lost {
            DISPLAY_WAIT_MS
        } else if self.full_redraw || self.content_dirty || self.windows_dirty {
            0
        } else if self.input_parser.state == InputState::Escape {
            (ESCAPE_TIMEOUT_MS as i32).min(POINTER_POLL_MS)
        } else {
            POINTER_POLL_MS
        };
        for fd in self.poll_fds.iter_mut() {
            fd.revents = 0;
        }
        let _ = io::poll(&mut self.poll_fds, timeout);
        let stdin_ready = self.poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0;
        let scene_changed = self.poll_fds[1].revents & io::poll_events::POLLIN as i16 != 0;
        if scene_changed {
            session::drain_scene(self.scene_fd);
        }

        (stdin_ready, scene_changed)
    }
}

// W17: loading and parsing are shared across architectures; matching stays in route.
impl HotkeyManager {
    pub(super) fn load_config(&mut self, path: &str) {
        let path = config_path(path);
        let path = path.to_str().expect("configuration path is UTF-8");
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
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
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

    pub(super) fn parse_line(line: &[u8]) -> Option<Hotkey> {
        // Split on '='
        let eq_pos = line.iter().position(|&b| b == b'=')?;
        let lhs = trim(&line[..eq_pos]);
        let rhs = trim(&line[eq_pos + 1..]);
        if lhs.is_empty() || rhs.is_empty() {
            return None;
        }

        // Parse left side: modifier+modifier+key
        let mut modifiers = 0u8;
        let mut key = 0u8;
        let mut taps = 1u8;

        // Count how many times each modifier appears (for multi-tap)
        let mut super_count = 0u8;

        let parts: Vec<&[u8]> = lhs.split(|&b| b == b'+').collect();
        for part in &parts {
            let part = trim(part);
            if part.is_empty() {
                continue;
            }
            match &part.to_ascii_lowercase()[..] {
                b"super" | b"cmd" | b"command" | b"win" => {
                    modifiers |= modifier::SUPER;
                    super_count += 1;
                }
                b"alt" => {
                    modifiers |= modifier::ALT;
                }
                b"ctrl" | b"control" => {
                    modifiers |= modifier::CTRL;
                }
                b"shift" => {
                    modifiers |= modifier::SHIFT;
                }
                b"return" | b"enter" => {
                    key = b'\r';
                }
                b"tab" => {
                    key = b'\t';
                }
                b"space" => {
                    key = b' ';
                }
                b"backspace" => {
                    key = 0x08;
                }
                b"escape" | b"esc" => {
                    key = 0x1b;
                }
                k if k.len() == 1 => {
                    key = k[0].to_ascii_lowercase();
                }
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
            if path.is_empty() {
                return None;
            }
            HotkeyAction::Exec(path.to_vec())
        } else {
            match &rhs.to_ascii_lowercase()[..] {
                b"close" => HotkeyAction::Close,
                b"focus_next" => HotkeyAction::FocusNext,
                b"focus_prev" => HotkeyAction::FocusPrev,
                _ => return None,
            }
        };

        Some(Hotkey {
            modifiers,
            key,
            taps,
            action,
        })
    }
}
pub(super) fn trim(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&b| b != b' ' && b != b'\t' && b != b'\r')
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&b| b != b' ' && b != b'\t' && b != b'\r')
        .map(|i| i + 1)
        .unwrap_or(start);
    &s[start..end]
}

pub(super) static FONT_METRICS: std::sync::OnceLock<std::sync::Mutex<Option<CachedFont>>> =
    std::sync::OnceLock::new();
pub(super) static CONFIG_ERROR: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn config_path(path: &str) -> std::path::PathBuf {
    std::env::var_os("BREENIX_CONFIG_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/".into())
        .join(path.trim_start_matches('/'))
}

struct Settings {
    font: Option<String>,
    frozen: bool,
    valid_test: bool,
    error: bool,
}
impl Settings {
    fn defaults() -> Self {
        Self {
            font: Some(FontConfig::load().display_path),
            frozen: false,
            valid_test: false,
            error: false,
        }
    }
    fn parse(bytes: &[u8]) -> Result<Self, ()> {
        let text = std::str::from_utf8(bytes).map_err(|_| ())?;
        let mut settings = Self::defaults();
        let mut seen = [false; 3];
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or(())?;
            let index = match (key, value) {
                ("font", "bitmap") => {
                    settings.font = None;
                    0
                }
                ("font", path) if path.starts_with('/') && !path.contains('\0') => {
                    settings.font = Some(path.into());
                    0
                }
                ("clock", "frozen") => {
                    settings.frozen = true;
                    1
                }
                ("fps", "off") => 2,
                _ => return Err(()),
            };
            if seen[index] {
                return Err(());
            }
            seen[index] = true;
        }
        if !seen.iter().any(|s| *s) {
            return Err(());
        }
        if let Some(path) = &settings.font {
            let bytes = std::fs::read(path).map_err(|_| ())?;
            Font::parse(&bytes).map_err(|_| ())?;
        }
        settings.valid_test = true;
        Ok(settings)
    }
    fn load() -> Self {
        match std::fs::read(config_path("/etc/bwm.conf")) {
            Ok(bytes) => match Self::parse(&bytes) {
                Ok(s) => s,
                Err(()) => {
                    print!("[bwm] bwm.conf error: invalid settings; using defaults\n");
                    Self {
                        error: true,
                        ..Self::defaults()
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::defaults(),
            Err(_) => {
                print!("[bwm] bwm.conf error: cannot read settings; using defaults\n");
                Self {
                    error: true,
                    ..Self::defaults()
                }
            }
        }
    }
    fn load_font(&self) -> Option<CachedFont> {
        let path = self.font.as_ref()?;
        let font = std::fs::read(path)
            .ok()
            .and_then(|data| Font::parse(&data).ok())
            .map(|font| CachedFont::new(font, 256));
        if font.is_some() {
            print!("[bwm] display font loaded: {}\n", path);
        } else {
            print!("[bwm] no display font, using bitmap fallback\n");
        }
        font
    }
}
