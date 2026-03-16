//! blog — Breenix Graphical Log Viewer
//!
//! A GUI log viewer built on the Breengel windowing library. Displays log
//! files in a tabbed interface with auto-scroll (follow mode) and keyboard
//! navigation. Uses TrueType fonts from system font configuration.
//!
//! Default tabs:
//!   Tab 0: "Kernel"  — tails /var/log/kernel.log
//!   Tab 1: "Serial"  — tails /var/log/serial.log
//!
//! Navigation:
//!   Up/k       Scroll up one line
//!   Down/j     Scroll down one line
//!   PgUp       Page up
//!   PgDn/Space Page down
//!   g/Home     Go to top
//!   G/End      Go to bottom, enable follow
//!   f          Toggle follow mode
//!   Tab        Next tab
//!   q          Quit

use std::process;

use breengel::{Event, InputState, Rect, TabBar, Theme, Window};
use libbreenix::fs;
use libbreenix::io;
use libbreenix::time;
use libbreenix::types::Fd;

use libfont::CachedFont;
use libgfx::bitmap_font;
use libgfx::ttf_font;
use libgfx::color::Color;
use libgfx::shapes;


// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const WIN_W: u32 = 600;
const WIN_H: u32 = 400;
const TAB_BAR_H: i32 = 20;
const STATUS_BAR_H: i32 = 18;
const READ_CHUNK: usize = 4096;
const MAX_FILE_SIZE: usize = 256 * 1024;

// Colors
const BG: Color = Color::rgb(30, 30, 40);
const TEXT_NORMAL: Color = Color::rgb(204, 204, 204);
const TEXT_LINE_NUM: Color = Color::rgb(100, 100, 120);
const STATUS_BG: Color = Color::rgb(50, 50, 60);
const STATUS_FG: Color = Color::rgb(180, 180, 200);
const FOLLOW_COLOR: Color = Color::rgb(100, 200, 100);
const NOT_FOUND_COLOR: Color = Color::rgb(200, 100, 100);

// USB HID keycodes for arrow keys and navigation
const KEY_UP: u16 = 0x52;
const KEY_DOWN: u16 = 0x51;
const KEY_PAGE_UP: u16 = 0x4B;
const KEY_PAGE_DOWN: u16 = 0x4E;
const KEY_HOME: u16 = 0x4A;
const KEY_END: u16 = 0x4D;

// ---------------------------------------------------------------------------
// Log tab state
// ---------------------------------------------------------------------------

struct LogTab {
    path: &'static str,
    content: Vec<u8>,
    lines: Vec<(usize, usize)>,
    scroll_offset: usize,
    follow: bool,
    fd: Option<Fd>,
    last_size: usize,
}

impl LogTab {
    fn new(path: &'static str) -> Self {
        Self {
            path,
            content: Vec::new(),
            lines: Vec::new(),
            scroll_offset: 0,
            follow: true,
            fd: None,
            last_size: 0,
        }
    }

    fn open(&mut self) {
        match fs::open(self.path, fs::O_RDONLY) {
            Ok(fd) => {
                self.fd = Some(fd);
                self.load_all(fd);
            }
            Err(_) => {
                self.fd = None;
            }
        }
    }

    fn load_all(&mut self, fd: Fd) {
        let mut buf = [0u8; READ_CHUNK];
        loop {
            match io::read(fd, &mut buf) {
                Ok(n) if n > 0 => {
                    if self.content.len() + n > MAX_FILE_SIZE {
                        let drain = (self.content.len() + n) - MAX_FILE_SIZE;
                        if drain < self.content.len() {
                            self.content.drain(..drain);
                        } else {
                            self.content.clear();
                        }
                    }
                    self.content.extend_from_slice(&buf[..n]);
                }
                _ => break,
            }
        }
        self.last_size = self.content.len();
        self.rebuild_lines();
    }

    fn poll_new_content(&mut self) {
        if self.fd.is_none() {
            match fs::open(self.path, fs::O_RDONLY) {
                Ok(fd) => {
                    self.fd = Some(fd);
                    self.load_all(fd);
                    return;
                }
                Err(_) => return,
            }
        }

        let fd = self.fd.unwrap();
        let current_size = match fs::fstat(fd) {
            Ok(stat) => stat.st_size as usize,
            Err(_) => return,
        };

        if current_size > self.last_size {
            let _ = fs::lseek(fd, self.last_size as i64, fs::SEEK_SET);
            let mut buf = [0u8; READ_CHUNK];
            loop {
                match io::read(fd, &mut buf) {
                    Ok(n) if n > 0 => {
                        if self.content.len() + n > MAX_FILE_SIZE {
                            let drain = (self.content.len() + n) - MAX_FILE_SIZE;
                            if drain < self.content.len() {
                                self.content.drain(..drain);
                            } else {
                                self.content.clear();
                            }
                        }
                        self.content.extend_from_slice(&buf[..n]);
                    }
                    _ => break,
                }
            }
            self.last_size = current_size;
            self.rebuild_lines();
        } else if current_size < self.last_size {
            let _ = fs::lseek(fd, 0, fs::SEEK_SET);
            self.content.clear();
            self.load_all(fd);
        }
    }

    fn rebuild_lines(&mut self) {
        self.lines.clear();
        if self.content.is_empty() {
            return;
        }
        let mut start = 0;
        for (i, &b) in self.content.iter().enumerate() {
            if b == b'\n' {
                self.lines.push((start, i));
                start = i + 1;
            }
        }
        if start < self.content.len() {
            self.lines.push((start, self.content.len()));
        }
    }

    fn line_bytes(&self, line: usize) -> &[u8] {
        if line >= self.lines.len() {
            return b"";
        }
        let (start, end) = self.lines[line];
        &self.content[start..end]
    }

    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn scroll_to_bottom(&mut self, visible_lines: usize) {
        if self.line_count() > visible_lines {
            self.scroll_offset = self.line_count() - visible_lines;
        } else {
            self.scroll_offset = 0;
        }
    }

    fn is_open(&self) -> bool {
        self.fd.is_some()
    }
}

// ---------------------------------------------------------------------------
// Font state
// ---------------------------------------------------------------------------

struct FontState {
    ttf: Option<CachedFont>,
    size: f32,
    char_w: usize,
    line_h: usize,
}

impl FontState {
    fn draw_text(&mut self, fb: &mut breengel::FrameBuf, text: &[u8], x: usize, y: usize, color: Color) {
        if let Some(ref mut font) = self.ttf {
            let s = core::str::from_utf8(text).unwrap_or("?");
            ttf_font::draw_text(fb, font, s, x as i32, y as i32, self.size, color);
        } else {
            bitmap_font::draw_text(fb, text, x, y, color);
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render(
    win: &mut Window,
    tabs: &[LogTab],
    tab_bar: &TabBar,
    theme: &Theme,
    fonts: &mut FontState,
) {
    let fb = win.framebuf();
    let w = fb.width as i32;
    let h = fb.height as i32;

    fb.clear(BG);
    tab_bar.draw(fb, theme);

    let line_h = fonts.line_h;
    let char_w = fonts.char_w;

    let content_y = TAB_BAR_H;
    let content_h = h - TAB_BAR_H - STATUS_BAR_H;
    let visible_lines = if content_h > 0 {
        content_h as usize / line_h
    } else {
        0
    };

    // Line number gutter width: 5 chars + 1 space
    let gutter_w = 6 * char_w;

    let selected = tab_bar.selected();
    let tab = &tabs[selected];

    if !tab.is_open() {
        let msg = b"File not found";
        let msg_x = (w as usize - msg.len() * char_w) / 2;
        let msg_y = content_y as usize + content_h as usize / 2;
        fonts.draw_text(fb, msg, msg_x, msg_y, NOT_FOUND_COLOR);
    } else if tab.line_count() == 0 {
        let msg = b"(empty)";
        let msg_x = (w as usize - msg.len() * char_w) / 2;
        let msg_y = content_y as usize + content_h as usize / 2;
        fonts.draw_text(fb, msg, msg_x, msg_y, TEXT_LINE_NUM);
    } else {
        for i in 0..visible_lines {
            let line_idx = tab.scroll_offset + i;
            if line_idx >= tab.line_count() {
                break;
            }
            let y = content_y as usize + i * line_h;

            // Line number
            let mut num_buf = [b' '; 5];
            format_line_number(&mut num_buf, line_idx + 1);
            fonts.draw_text(fb, &num_buf, 2, y, TEXT_LINE_NUM);

            // Line content, truncated to fit
            let line = tab.line_bytes(line_idx);
            let max_chars = ((w as usize).saturating_sub(gutter_w)) / char_w;
            let display_len = line.len().min(max_chars);
            if display_len > 0 {
                fonts.draw_text(fb, &line[..display_len], gutter_w, y, TEXT_NORMAL);
            }
        }
    }

    // Status bar
    let status_y = h - STATUS_BAR_H;
    shapes::fill_rect(fb, 0, status_y, w, STATUS_BAR_H, STATUS_BG);

    let path_bytes = tab.path.as_bytes();
    fonts.draw_text(fb, path_bytes, 4, status_y as usize + 2, STATUS_FG);

    let mut info_buf = [0u8; 40];
    let info_len = format_line_info(
        &mut info_buf,
        tab.scroll_offset + 1,
        tab.line_count(),
    );
    let info_x = (w as usize / 2).saturating_sub(info_len * char_w / 2);
    fonts.draw_text(fb, &info_buf[..info_len], info_x, status_y as usize + 2, STATUS_FG);

    if tab.follow {
        let label = b"[FOLLOW]";
        let label_x = w as usize - label.len() * char_w - 4;
        fonts.draw_text(fb, label, label_x, status_y as usize + 2, FOLLOW_COLOR);
    }
}

fn format_line_number(buf: &mut [u8; 5], n: usize) {
    let mut val = n;
    let mut i = 4;
    loop {
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
        if val == 0 || i == 0 {
            break;
        }
        i -= 1;
    }
}

fn format_line_info(buf: &mut [u8], offset: usize, total: usize) -> usize {
    let mut pos = 0;
    buf[pos] = b'L'; pos += 1;
    buf[pos] = b':'; pos += 1;
    pos += write_usize(&mut buf[pos..], offset);
    buf[pos] = b'/'; pos += 1;
    pos += write_usize(&mut buf[pos..], total);
    pos
}

fn write_usize(buf: &mut [u8], mut n: usize) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut count = 0;
    while n > 0 {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }
    for i in 0..count {
        buf[i] = digits[count - 1 - i];
    }
    count
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    println!("[blog] Breenix Log Viewer starting");

    // Load system font configuration via FontWatcher (handles hot-reload)
    let mut font_watcher = breengel::FontWatcher::new();
    let font_size = if font_watcher.mono_size() >= 6.0 { font_watcher.mono_size() } else { 14.0 };
    println!("[blog] config: mono_size={} font_size={} path='{}'", font_watcher.mono_size(), font_size, font_watcher.mono_path());

    // Load TrueType font (Font owns its data, no lifetime gymnastics needed)
    let ttf_font: Option<CachedFont> = font_watcher.load_font();

    // Compute font metrics
    let mut fonts = if let Some(font) = ttf_font {
        let metrics = font.metrics(font_size);
        let glyph_m = font.glyph_index('M');
        let advance = font.advance_width(glyph_m, font_size);
        let char_w = (advance + 0.5) as i32;
        let line_h = (metrics.ascender + 0.99) as i32 + ((-metrics.descender) + 0.99) as i32;
        FontState {
            ttf: Some(font),
            size: font_size,
            char_w: char_w.max(1) as usize,
            line_h: line_h.max(1) as usize,
        }
    } else {
        let fm = bitmap_font::metrics();
        FontState {
            ttf: None,
            size: font_size,
            char_w: fm.char_width,
            line_h: fm.line_height(),
        }
    };

    // Diagnostic: test TTF rasterization
    if let Some(ref mut f) = fonts.ttf {
        let gi = f.glyph_index('A');
        println!("[blog] font loaded, char_w={} line_h={}", fonts.char_w, fonts.line_h);
        match f.font().debug_rasterize(gi, fonts.size) {
            Ok(d) => {
                println!("[blog] dbg: px={} upm={} scale={}", d.pixel_size, d.units_per_em, d.scale);
                println!("[blog] dbg: glyph_bbox=({},{},{},{})", d.glyph_x_min, d.glyph_y_min, d.glyph_x_max, d.glyph_y_max);
                println!("[blog] dbg: scaled=({},{},{},{})", d.x_min_scaled, d.y_min_scaled, d.x_max_scaled, d.y_max_scaled);
                println!("[blog] dbg: bmp={}x{} off=({},{}) baseline={}", d.bmp_width, d.bmp_height, d.bmp_x_offset, d.bmp_y_offset, d.baseline);
                println!("[blog] dbg: contours={} pts={} segs={} nz={}", d.num_contours, d.num_points, d.num_segments, d.nonzero_coverage);
            }
            Err(e) => println!("[blog] debug_rasterize FAILED: {}", e),
        }
        // Test actual rasterize
        f.clear_cache();
        match f.rasterize_glyph(gi, fonts.size) {
            Ok(bmp) => {
                let nz = bmp.coverage.iter().filter(|&&v| v > 0).count();
                println!("[blog] raster: {}x{} off=({},{}) nz={}", bmp.width, bmp.height, bmp.x_offset, bmp.y_offset, nz);
            }
            Err(e) => println!("[blog] raster FAIL: {}", e),
        }
    } else {
        println!("[blog] TTF font NOT loaded, using bitmap fallback");
    }

    // Create window
    let mut win = match Window::new(b"Log Viewer", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("[blog] Window::new failed: {} -- exiting", e);
            process::exit(1);
        }
    };
    println!("[blog] Window {} ({}x{})", win.id(), WIN_W, WIN_H);

    let mut theme = Theme::dark();
    theme.use_bitmap_font = true;

    let tab_rect = Rect::new(0, 0, WIN_W as i32, TAB_BAR_H);
    let mut tab_bar = TabBar::new(tab_rect, vec![b"Kernel", b"Serial"]);

    let mut tabs = vec![
        LogTab::new("/var/log/kernel.log"),
        LogTab::new("/var/log/serial.log"),
    ];

    for tab in tabs.iter_mut() {
        tab.open();
    }

    // Diagnostic: show log file status
    for (i, tab) in tabs.iter().enumerate() {
        println!("[blog] tab[{}] '{}': open={} lines={} bytes={}",
                 i, tab.path, tab.is_open(), tab.line_count(), tab.content.len());
    }

    let content_h = WIN_H as i32 - TAB_BAR_H - STATUS_BAR_H;
    let mut visible_lines = if content_h > 0 && fonts.line_h > 0 {
        content_h as usize / fonts.line_h
    } else {
        1
    };

    for tab in tabs.iter_mut() {
        if tab.follow {
            tab.scroll_to_bottom(visible_lines);
        }
    }

    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut mouse_buttons: u32 = 0;
    let mut poll_counter: u32 = 0;

    render(&mut win, &tabs, &tab_bar, &theme, &mut fonts);
    let _ = win.present();

    loop {
        let events = win.poll_events();
        let mut needs_redraw = !events.is_empty();

        for event in events {
            match event {
                Event::KeyPress { ascii, keycode, .. } => {
                    let sel = tab_bar.selected();
                    match keycode {
                        KEY_UP => {
                            if tabs[sel].scroll_offset > 0 {
                                tabs[sel].scroll_offset -= 1;
                                tabs[sel].follow = false;
                            }
                        }
                        KEY_DOWN => {
                            if tabs[sel].scroll_offset + visible_lines < tabs[sel].line_count() {
                                tabs[sel].scroll_offset += 1;
                            }
                        }
                        KEY_PAGE_UP => {
                            tabs[sel].scroll_offset =
                                tabs[sel].scroll_offset.saturating_sub(visible_lines);
                            tabs[sel].follow = false;
                        }
                        KEY_PAGE_DOWN => {
                            let max = tabs[sel].line_count().saturating_sub(visible_lines);
                            tabs[sel].scroll_offset =
                                (tabs[sel].scroll_offset + visible_lines).min(max);
                        }
                        KEY_HOME => {
                            tabs[sel].scroll_offset = 0;
                            tabs[sel].follow = false;
                        }
                        KEY_END => {
                            tabs[sel].scroll_to_bottom(visible_lines);
                            tabs[sel].follow = true;
                        }
                        _ => {
                            match ascii {
                                b'q' | b'Q' => {
                                    for tab in &tabs {
                                        if let Some(fd) = tab.fd {
                                            let _ = io::close(fd);
                                        }
                                    }
                                    process::exit(0);
                                }
                                b'k' => {
                                    if tabs[sel].scroll_offset > 0 {
                                        tabs[sel].scroll_offset -= 1;
                                        tabs[sel].follow = false;
                                    }
                                }
                                b'j' => {
                                    if tabs[sel].scroll_offset + visible_lines
                                        < tabs[sel].line_count()
                                    {
                                        tabs[sel].scroll_offset += 1;
                                    }
                                }
                                b' ' => {
                                    let max =
                                        tabs[sel].line_count().saturating_sub(visible_lines);
                                    tabs[sel].scroll_offset =
                                        (tabs[sel].scroll_offset + visible_lines).min(max);
                                }
                                b'g' => {
                                    tabs[sel].scroll_offset = 0;
                                    tabs[sel].follow = false;
                                }
                                b'G' => {
                                    tabs[sel].scroll_to_bottom(visible_lines);
                                    tabs[sel].follow = true;
                                }
                                b'f' => {
                                    tabs[sel].follow = !tabs[sel].follow;
                                    if tabs[sel].follow {
                                        tabs[sel].scroll_to_bottom(visible_lines);
                                    }
                                }
                                b'\t' => {
                                    let next = (tab_bar.selected() + 1) % tabs.len();
                                    tab_bar.set_selected(next);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Event::MouseMove { x, y } => {
                    mouse_x = x;
                    mouse_y = y;
                }
                Event::MouseButton { button: 1, pressed, .. } => {
                    let old_buttons = mouse_buttons;
                    if pressed {
                        mouse_buttons |= 1;
                    } else {
                        mouse_buttons &= !1;
                    }
                    let input = InputState::from_raw(
                        mouse_x,
                        mouse_y,
                        mouse_buttons,
                        old_buttons,
                    );
                    tab_bar.update(&input);
                }
                Event::Resized { width: w, height: h } => {
                    let new_content_h = h as i32 - TAB_BAR_H - STATUS_BAR_H;
                    visible_lines = if new_content_h > 0 && fonts.line_h > 0 {
                        new_content_h as usize / fonts.line_h
                    } else {
                        1
                    };
                    tab_bar.set_rect(Rect::new(0, 0, w as i32, TAB_BAR_H));
                    needs_redraw = true;
                }
                Event::CloseRequested => {
                    for tab in &tabs {
                        if let Some(fd) = tab.fd {
                            let _ = io::close(fd);
                        }
                    }
                    process::exit(0);
                }
                _ => {}
            }
        }

        poll_counter += 1;
        if poll_counter % 10 == 0 {
            for tab in tabs.iter_mut() {
                let prev_count = tab.line_count();
                tab.poll_new_content();
                if tab.line_count() != prev_count {
                    needs_redraw = true;
                    if tab.follow {
                        tab.scroll_to_bottom(visible_lines);
                    }
                }
            }
        }

        // Font config hot-reload via FontWatcher
        if let Some(new_font) = font_watcher.poll() {
            let new_size = font_watcher.mono_size();
            println!("[blog] font config changed: {} size={} (bits=0x{:08x})",
                     font_watcher.mono_path(), new_size, new_size.to_bits());
            let metrics = new_font.metrics(new_size);
            let glyph_m = new_font.glyph_index('M');
            let advance = new_font.advance_width(glyph_m, new_size);
            // Use i32 intermediate to avoid potential f32-to-usize codegen issues
            let advance_rounded = (advance + 0.5) as i32;
            let new_char_w = advance_rounded.max(1) as usize;
            let asc_ceil = (metrics.ascender + 0.99) as i32;
            let desc_ceil = ((-metrics.descender) + 0.99) as i32;
            let new_line_h = (asc_ceil + desc_ceil).max(1) as usize;
            println!("[blog] metrics: asc={} desc={} adv={} adv_bits=0x{:08x} cw={} lh={}",
                     metrics.ascender, metrics.descender, advance, advance.to_bits(),
                     new_char_w, new_line_h);
            fonts.char_w = new_char_w;
            fonts.line_h = new_line_h;
            fonts.size = new_size;
            fonts.ttf = Some(new_font);
            // Update visible_lines for scroll calculations
            visible_lines = if content_h > 0 && fonts.line_h > 0 {
                content_h as usize / fonts.line_h
            } else {
                1
            };
            // Re-scroll follow-mode tabs for new line height
            for tab in tabs.iter_mut() {
                if tab.follow {
                    tab.scroll_to_bottom(visible_lines);
                }
            }
            needs_redraw = true;
        }

        if needs_redraw {
            render(&mut win, &tabs, &tab_bar, &theme, &mut fonts);
            let _ = win.present();
        }

        let _ = time::sleep_ms(50);
    }
}
