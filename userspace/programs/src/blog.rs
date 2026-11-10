//! blog — Breenix Graphical Log Viewer
//!
//! A GUI log viewer built on the Breengel windowing library. Displays log
//! files in a tabbed interface with auto-scroll (follow mode) and keyboard
//! navigation. Uses TrueType fonts from system font configuration.
//!
//! Default tabs:
//!   Tab 0: "Kernel"  — tails /var/log/kernel.log
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

#[path = "blog/tail.rs"]
mod tail;
#[path = "blog/node.rs"]
mod node;
#[path = "blog/view.rs"]
mod view;
use view::{Command, View};
use tail::{Action, Tail};

use breengel::{CachedFont, Event, Handled, InputState, Rect, TabBar, Theme, Window};
use libbreenix::fs;
use libbreenix::io;
use libbreenix::time;
use libbreenix::types::Fd;

use libgfx::bitmap_font;
use libgfx::ttf_font;
use libgfx::color::Color;
use libgfx::shapes;
use libgfx::surface::Surface;


// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const WIN_W: u32 = 600;
const WIN_H: u32 = 400;
const TAB_BAR_H: i32 = 20;
const STATUS_BAR_H: i32 = 18;
const READ_CHUNK: usize = 4096;

// Colors
const BG: Color = Color::rgb(30, 30, 40);
const TEXT_NORMAL: Color = Color::rgb(204, 204, 204);
const TEXT_LINE_NUM: Color = Color::rgb(100, 100, 120);
const STATUS_BG: Color = Color::rgb(50, 50, 60);
const STATUS_FG: Color = Color::rgb(180, 180, 200);
const FOLLOW_COLOR: Color = Color::rgb(100, 200, 100);
const NOT_FOUND_COLOR: Color = Color::rgb(200, 100, 100);

// Linux key codes for arrow keys and navigation
const KEY_UP: u32 = breengel::keys::UP;
const KEY_DOWN: u32 = breengel::keys::DOWN;
const KEY_PAGE_UP: u32 = breengel::keys::PAGE_UP;
const KEY_PAGE_DOWN: u32 = breengel::keys::PAGE_DOWN;
const KEY_HOME: u32 = breengel::keys::HOME;
const KEY_END: u32 = breengel::keys::END;

// ---------------------------------------------------------------------------
// Log tab state
// ---------------------------------------------------------------------------

struct LogTab {
    path: String,
    tail: Tail,
    view: View,
    fd: Option<Fd>,
}

impl LogTab {
    fn new(path: String) -> Self {
        Self {
            path,
            tail: Tail::new(),
            view: View::new(),
            fd: None,
        }
    }

    fn open(&mut self) {
        let _ = self.poll_new_content();
    }

    fn poll_new_content(&mut self) -> bool {
        // Opening the path lets us detect rename-rotation without relying on
        // metadata from the old, still-open file descriptor.
        let candidate = match fs::open(&self.path, fs::O_RDONLY) {
            Ok(fd) => fd,
            Err(_) => return false,
        };
        let stat = match fs::fstat(candidate) {
            Ok(stat) => stat,
            Err(_) => { let _ = io::close(candidate); return false; }
        };
        let action = self.tail.observe(stat.st_ino, stat.st_size.max(0) as usize);
        if self.fd.is_none() || action == Action::Reopen {
            if let Some(old) = self.fd.replace(candidate) {
                let _ = io::close(old);
            }
        } else {
            let _ = io::close(candidate);
        }
        let fd = self.fd.unwrap();
        if fs::lseek(fd, self.tail.offset() as i64, fs::SEEK_SET).is_err() {
            return false;
        }
        let mut buf = [0u8; READ_CHUNK];
        while let Ok(n) = io::read(fd, &mut buf) {
            if n == 0 { break; }
            self.tail.feed(&buf[..n]);
        }
        self.view.offset = self.view.offset.min(self.tail.line_count().saturating_sub(1));
        action != Action::Read
    }

    fn line_bytes(&self, line: usize) -> &[u8] { self.tail.line(line) }

    fn line_count(&self) -> usize { self.tail.line_count() }

    fn scroll_to_bottom(&mut self, visible_lines: usize) {
        self.view.bottom(self.tail.line_count(), visible_lines);
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

/// A frame's text, drawn after its fills, which it does not overlap: each line as
/// `ttf_font::draw_text` draws it, with the lines' glyphs sharing draws.
struct Text {
    lines: Vec<(Vec<u8>, usize, usize, Color)>,
}

impl Text {
    fn draw_text(&mut self, text: &[u8], x: usize, y: usize, color: Color) {
        // TrueType draws invalid UTF-8 as "?", as it did line by line.
        let bytes = if core::str::from_utf8(text).is_ok() { text.to_vec() } else { b"?".to_vec() };
        self.lines.push((bytes, x, y, color));
    }

    fn finish(self, fb: &mut Surface, fonts: &mut FontState) {
        if let Some(ref mut font) = fonts.ttf {
            let clip = libgfx::surface::Rect::new(0, 0, fb.width() as i32, fb.height() as i32);
            let runs: Vec<ttf_font::TextRun<'_>> = self.lines.iter().map(|(bytes, x, y, fg)| ttf_font::TextRun {
                bytes, x: *x as i32, y: *y as i32, size: fonts.size, fg: *fg, clip,
            }).collect();
            ttf_font::draw_runs(fb, font, &runs);
        } else {
            for (bytes, x, y, fg) in &self.lines {
                bitmap_font::draw_text(fb, bytes, *x, *y, *fg);
            }
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
    let fb = match win.surface() {
        Ok(fb) => fb,
        Err(e) => {
            println!("[blog] ERROR: no window surface: {} -- exiting", e);
            process::exit(1);
        }
    };
    let w = fb.width() as i32;
    let h = fb.height() as i32;
    let mut text = Text { lines: Vec::new() };

    fb.clear(BG);
    tab_bar.draw(fb, theme);

    let line_h = fonts.line_h;
    let char_w = fonts.char_w;

    let content_y = TAB_BAR_H * fb.ui_scale() as i32;
    let content_h = h - content_y - STATUS_BAR_H;
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
        let msg = format!("Waiting for {}", tab.path);
        let msg = msg.as_bytes();
        let msg_x = (w as usize).saturating_sub(msg.len() * char_w) / 2;
        let msg_y = content_y as usize + content_h as usize / 2;
        text.draw_text(msg, msg_x, msg_y, NOT_FOUND_COLOR);
    } else if tab.line_count() == 0 {
        let msg = b"(empty)";
        let msg_x = (w as usize).saturating_sub(msg.len() * char_w) / 2;
        let msg_y = content_y as usize + content_h as usize / 2;
        text.draw_text(msg, msg_x, msg_y, TEXT_LINE_NUM);
    } else {
        for i in 0..visible_lines {
            let line_idx = tab.view.offset + i;
            if line_idx >= tab.line_count() {
                break;
            }
            let y = content_y as usize + i * line_h;

            // Line number
            let mut num_buf = [b' '; 5];
            format_line_number(&mut num_buf, tab.tail.line_number(line_idx));
            text.draw_text(&num_buf, 2, y, TEXT_LINE_NUM);

            // Line content, truncated to fit
            if tab.view.matched == Some(line_idx) { shapes::fill_rect(fb, 0, y as i32, w, line_h as i32, Color::rgb(80, 70, 35)); }
            let line = tab.line_bytes(line_idx);
            let max_chars = ((w as usize).saturating_sub(gutter_w)) / char_w;
            let display_len = line.len().min(max_chars);
            if display_len > 0 {
                text.draw_text(&line[..display_len], gutter_w, y, TEXT_NORMAL);
            }
        }
    }

    // Status bar
    let status_y = h - STATUS_BAR_H;
    shapes::fill_rect(fb, 0, status_y, w, STATUS_BAR_H, STATUS_BG);

    let search_text = format!("/{}", tab.view.draft());
    let path_bytes = if tab.view.editing { search_text.as_bytes() } else { tab.path.as_bytes() };
    text.draw_text(path_bytes, 4, status_y as usize + 2, STATUS_FG);

    let mut info_buf = [0u8; 40];
    let info_len = format_line_info(
        &mut info_buf,
        tab.tail.line_number(tab.view.offset),
        tab.tail.line_number(tab.line_count()).saturating_sub(1),
    );
    let info_x = (w as usize / 2).saturating_sub(info_len * char_w / 2);
    text.draw_text(&info_buf[..info_len], info_x, status_y as usize + 2, STATUS_FG);

    if tab.view.follow {
        let label = b"[FOLLOW]";
        let label_x = w as usize - label.len() * char_w - 4;
        text.draw_text(label, label_x, status_y as usize + 2, FOLLOW_COLOR);
    }
    text.finish(fb, fonts);
}

fn publish_tree(win: &mut Window, tabs: &[LogTab], selected: usize, visible: usize) {
    use breengel::tree::{Flag, Node, Role, Tree};
    let tab = &tabs[selected];
    let bounds = [0, 0, i64::from(win.width()), i64::from(win.height())];
    let flags = vec![Flag::Enabled, Flag::Visible];
    win.set_tree(Tree { root: Node {
        key: "lines".into(), parent: None, role: Role::List, name: "Lines".into(),
        value: node::lines_view(&tab.tail, tab.view.offset, visible, tab.view.matched), bounds, clip: bounds, flags: flags.clone(),
    }, children: vec![Node {
        key: "status".into(), parent: Some("lines".into()), role: Role::Status, name: "Status".into(),
        value: node::status_view(selected, &tab.path, tab.view.follow, tab.is_open(), &tab.view.query, tab.view.matched, tab.view.matches, &tab.tail), bounds, clip: bounds, flags,
    }] });
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

    // Create window (loads system font automatically from /etc/fonts.conf)
    let mut win = match Window::new(b"Log Viewer", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("[blog] Window::new failed: {} -- exiting", e);
            process::exit(1);
        }
    };
    println!("[blog] Window {} ({}x{})", win.id(), WIN_W, WIN_H);

    let font_size = if win.mono_size() >= 6.0 { win.mono_size() } else { 14.0 };
    println!("[blog] config: mono_size={} font_size={} path='{}'", win.mono_size(), font_size, win.mono_path());

    // Take font for local use (needed alongside the window's surface for rendering)
    let ttf_font: Option<CachedFont> = win.take_mono_font();

    let mut fonts = if let Some(font) = ttf_font {
        let metrics = font.metrics(font_size);
        let glyph_m = font.glyph_index('M');
        let advance = font.advance_width(glyph_m, font_size);
        let char_w = (advance + 0.5) as i32;
        let line_h = (metrics.ascender + 0.99) as i32 + ((-metrics.descender) + 0.99) as i32;
        println!("[blog] font loaded, char_w={} line_h={}", char_w.max(1), line_h.max(1));
        FontState {
            ttf: Some(font),
            size: font_size,
            char_w: char_w.max(1) as usize,
            line_h: line_h.max(1) as usize,
        }
    } else {
        println!("[blog] TTF font NOT loaded, using bitmap fallback");
        let fm = bitmap_font::metrics();
        FontState {
            ttf: None,
            size: font_size,
            char_w: fm.char_width,
            line_h: fm.line_height(),
        }
    };

    let mut theme = Theme::dark();
    theme.use_bitmap_font = true;

    let ui_scale = win.surface().expect("log surface").ui_scale() as i32;
    let tab_pixels = TAB_BAR_H * ui_scale;
    let tab_rect = Rect::new(0, 0, WIN_W as i32 / ui_scale, TAB_BAR_H);
    let paths: Vec<String> = {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() { vec!["/var/log/kernel.log".into()] } else { args }
    };
    // TabBar retains labels for the lifetime of the process.
    let labels: Vec<&'static [u8]> = paths.iter().map(|path| &*Box::leak(path.clone().into_bytes().into_boxed_slice())).collect();
    let mut tab_bar = TabBar::new(tab_rect, labels, &theme);
    let mut tabs: Vec<LogTab> = paths.into_iter().map(LogTab::new).collect();

    for tab in tabs.iter_mut() {
        tab.open();
    }

    // Diagnostic: show log file status
    for (i, tab) in tabs.iter().enumerate() {
        println!("[blog] tab[{}] '{}': open={} lines={} bytes={}",
                 i, tab.path, tab.is_open(), tab.line_count(), tab.tail.offset());
    }

    let content_h = WIN_H as i32 - tab_pixels - STATUS_BAR_H;
    let mut visible_lines = if content_h > 0 && fonts.line_h > 0 {
        content_h as usize / fonts.line_h
    } else {
        1
    };

    for tab in tabs.iter_mut() {
        if tab.view.follow {
            tab.scroll_to_bottom(visible_lines);
        }
    }

    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut mouse_buttons: u32 = 0;


    render(&mut win, &tabs, &tab_bar, &theme, &mut fonts);
    publish_tree(&mut win, &tabs, tab_bar.selected(), visible_lines);
    match win.present() {
        Ok(()) => println!("[blog] first frame presented: {}x{}", win.width(), win.height()),
        Err(e) => println!("[blog] first frame present failed: {}", e),
    }

    loop {
        let mut resize_requested = None;
        let mut reload_font = false;
        let event_count = win.dispatch(|delivered| {
            match delivered.event.clone() {
                Event::KeyPress { ascii, keycode, .. } => {
                    let sel = tab_bar.selected();
                    let command = if tabs[sel].view.editing {
                        match keycode {
                            breengel::keys::ENTER => Some(Command::Enter),
                            breengel::keys::ESC => Some(Command::Escape),
                            breengel::keys::BACKSPACE => Some(Command::Backspace),
                            _ if ascii != 0 => Some(Command::Character(ascii)),
                            _ => None,
                        }
                    } else {
                        match keycode {
                            KEY_UP => Some(Command::Up), KEY_DOWN => Some(Command::Down),
                            KEY_PAGE_UP => Some(Command::PageUp), KEY_PAGE_DOWN => Some(Command::PageDown),
                            KEY_HOME => Some(Command::Top), KEY_END => Some(Command::Bottom),
                            _ => match ascii {
                                b'k' => Some(Command::Up), b'j' => Some(Command::Down),
                                b' ' => Some(Command::PageDown), b'g' => Some(Command::Top),
                                b'G' => Some(Command::Bottom), b'f' => Some(Command::ToggleFollow),
                                b'/' => Some(Command::Search), b'n' => Some(Command::Next), b'N' => Some(Command::Previous),
                                b'\t' => { tab_bar.set_selected((sel + 1) % tabs.len()); None },
                                b'q' | b'Q' => { for tab in &tabs { if let Some(fd) = tab.fd { let _ = io::close(fd); } } process::exit(0); },
                                _ => None,
                            }
                        }
                    };
                    if let Some(command) = command {
                        let tab = &mut tabs[sel];
                        let lines: Vec<&[u8]> = (0..tab.tail.line_count()).map(|i| tab.tail.line(i)).collect();
                        tab.view.apply(command, &lines, visible_lines);
                    }
                }
                Event::Scroll { delta_y, .. } => {
                    let tab = &mut tabs[tab_bar.selected()];
                    let lines: Vec<&[u8]> = (0..tab.tail.line_count()).map(|i| tab.tail.line(i)).collect();
                    tab.view.apply(Command::Wheel(delta_y as i32), &lines, visible_lines);
                }
                Event::MouseMove { x, y } => {
                    mouse_x = x;
                    mouse_y = y;
                }
                Event::MouseButton { button: breengel::buttons::LEFT, pressed, .. } => {
                    let old_buttons = mouse_buttons;
                    if pressed {
                        mouse_buttons |= 1;
                    } else {
                        mouse_buttons &= !1;
                    }
                    let input = InputState::from_raw(
                        mouse_x / ui_scale,
                        mouse_y / ui_scale,
                        mouse_buttons,
                        old_buttons,
                        breengel::clock::now_ns() / 1_000_000,
                    );
                    tab_bar.update(&input);
                }
                Event::ResizeRequested { width, height } => {
                    resize_requested = Some((width, height));
                }
                Event::Resized { width: w, height: h } => {
                    let new_content_h = h as i32 - tab_pixels - STATUS_BAR_H;
                    visible_lines = if new_content_h > 0 && fonts.line_h > 0 {
                        new_content_h as usize / fonts.line_h
                    } else {
                        1
                    };
                    tab_bar.set_rect(Rect::new(0, 0, w as i32 / ui_scale, TAB_BAR_H));
                }
                Event::CloseRequested => {
                    for tab in &tabs {
                        if let Some(fd) = tab.fd {
                            let _ = io::close(fd);
                        }
                    }
                    process::exit(0);
                }
                Event::FontChanged => { reload_font = true; }
                _ => {}
            }
            Handled::Redraw
        }).unwrap_or_else(|e| { eprintln!("[blog] dispatch: {e}"); process::exit(1) });
        let mut needs_redraw = event_count != 0;
        if let Some((width, height)) = resize_requested {
            if let Err(e) = win.apply_resize(width, height) {
                println!("[blog] resize refused: {}x{}: {}", width, height, e);
            }
        }
        if reload_font {
                    win.put_mono_font(fonts.ttf.take());
                    let new_font = win.take_mono_font();
                    let new_size = win.mono_size();
                    if let Some(ref f) = new_font {
                        let metrics = f.metrics(new_size);
                        let glyph_m = f.glyph_index('M');
                        let advance = f.advance_width(glyph_m, new_size);
                        let advance_rounded = (advance + 0.5) as i32;
                        let new_char_w = advance_rounded.max(1) as usize;
                        let asc_ceil = (metrics.ascender + 0.99) as i32;
                        let desc_ceil = ((-metrics.descender) + 0.99) as i32;
                        let new_line_h = (asc_ceil + desc_ceil).max(1) as usize;
                        fonts.char_w = new_char_w;
                        fonts.line_h = new_line_h;
                        fonts.size = new_size;
                        visible_lines = if content_h > 0 && fonts.line_h > 0 {
                            content_h as usize / fonts.line_h
                        } else {
                            1
                        };
                        for tab in tabs.iter_mut() {
                            if tab.view.follow {
                                tab.scroll_to_bottom(visible_lines);
                            }
                        }
                    }
                    fonts.ttf = new_font;

            needs_redraw = true;
        }
        {
            for tab in tabs.iter_mut() {
                let was_open = tab.is_open();
                let prev_count = tab.line_count();
                let prev_offset = tab.tail.offset();
                let reset = tab.poll_new_content();
                if reset || tab.is_open() != was_open || tab.line_count() != prev_count || tab.tail.offset() != prev_offset {
                    needs_redraw = true;
                    let lines: Vec<&[u8]> = (0..tab.tail.line_count()).map(|i| tab.tail.line(i)).collect();
                    tab.view.refresh(&lines, visible_lines);
                    if tab.view.follow {
                        tab.scroll_to_bottom(visible_lines);
                    }
                }
            }
        }

        if needs_redraw {
            render(&mut win, &tabs, &tab_bar, &theme, &mut fonts);
            publish_tree(&mut win, &tabs, tab_bar.selected(), visible_lines);
            let _ = win.present();
        }

        if !needs_redraw { let _ = time::sleep_ms(10); }
    }
}
