//! blog — Breenix Graphical Log Viewer
//!
//! A GUI log viewer built on the Breengel windowing library. Displays log
//! files in a tabbed interface with auto-scroll (follow mode) and keyboard
//! navigation.
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

use libgfx::bitmap_font;
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

    /// Open the file and load initial content.
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

    /// Read all available content from the file descriptor.
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

    /// Check for new content appended to the file.
    fn poll_new_content(&mut self) {
        if self.fd.is_none() {
            // Try opening file if it didn't exist before
            match fs::open(self.path, fs::O_RDONLY) {
                Ok(fd) => {
                    self.fd = Some(fd);
                    self.load_all(fd);
                    return;
                }
                Err(_) => return,
            }
        }

        // Check file size via fstat
        let fd = self.fd.unwrap();
        let current_size = match fs::fstat(fd) {
            Ok(stat) => stat.st_size as usize,
            Err(_) => return,
        };

        if current_size > self.last_size {
            // Seek to where we left off and read new content
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
            // File was truncated/rotated; reload from scratch
            let _ = fs::lseek(fd, 0, fs::SEEK_SET);
            self.content.clear();
            self.load_all(fd);
        }
    }

    /// Build the line index from content bytes.
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
        // Handle trailing content without newline
        if start < self.content.len() {
            self.lines.push((start, self.content.len()));
        }
    }

    /// Get bytes for a given line index.
    fn line_bytes(&self, line: usize) -> &[u8] {
        if line >= self.lines.len() {
            return b"";
        }
        let (start, end) = self.lines[line];
        &self.content[start..end]
    }

    /// Total number of lines.
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Scroll to bottom.
    fn scroll_to_bottom(&mut self, visible_lines: usize) {
        if self.line_count() > visible_lines {
            self.scroll_offset = self.line_count() - visible_lines;
        } else {
            self.scroll_offset = 0;
        }
    }

    /// Returns true if the file was found.
    fn is_open(&self) -> bool {
        self.fd.is_some()
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
) {
    let fb = win.framebuf();
    let w = fb.width as i32;
    let h = fb.height as i32;

    // Clear entire framebuffer
    fb.clear(BG);

    // Draw tab bar
    tab_bar.draw(fb, theme);

    // Get font metrics
    let fm = bitmap_font::metrics();
    let line_h = fm.line_height();
    let char_w = fm.char_width;

    // Content area: below tab bar, above status bar
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
        // Show "File not found" message
        let msg = b"File not found";
        let msg_x = (w as usize - msg.len() * char_w) / 2;
        let msg_y = content_y as usize + content_h as usize / 2;
        bitmap_font::draw_text(fb, msg, msg_x, msg_y, NOT_FOUND_COLOR);
    } else if tab.line_count() == 0 {
        let msg = b"(empty)";
        let msg_x = (w as usize - msg.len() * char_w) / 2;
        let msg_y = content_y as usize + content_h as usize / 2;
        bitmap_font::draw_text(fb, msg, msg_x, msg_y, TEXT_LINE_NUM);
    } else {
        // Render visible lines
        for i in 0..visible_lines {
            let line_idx = tab.scroll_offset + i;
            if line_idx >= tab.line_count() {
                break;
            }
            let y = content_y as usize + i * line_h;

            // Line number
            let mut num_buf = [b' '; 5];
            format_line_number(&mut num_buf, line_idx + 1);
            bitmap_font::draw_text(fb, &num_buf, 2, y, TEXT_LINE_NUM);

            // Line content, truncated to fit
            let line = tab.line_bytes(line_idx);
            let max_chars = ((w as usize).saturating_sub(gutter_w)) / char_w;
            let display_len = line.len().min(max_chars);
            if display_len > 0 {
                bitmap_font::draw_text(fb, &line[..display_len], gutter_w, y, TEXT_NORMAL);
            }
        }
    }

    // Status bar
    let status_y = h - STATUS_BAR_H;
    shapes::fill_rect(fb, 0, status_y, w, STATUS_BAR_H, STATUS_BG);

    // Left: filename
    let path_bytes = tab.path.as_bytes();
    bitmap_font::draw_text(fb, path_bytes, 4, status_y as usize + 2, STATUS_FG);

    // Middle: line info
    let mut info_buf = [0u8; 40];
    let info_len = format_line_info(
        &mut info_buf,
        tab.scroll_offset + 1,
        tab.line_count(),
    );
    let info_x = (w as usize / 2).saturating_sub(info_len * char_w / 2);
    bitmap_font::draw_text(fb, &info_buf[..info_len], info_x, status_y as usize + 2, STATUS_FG);

    // Right: follow indicator
    if tab.follow {
        let label = b"[FOLLOW]";
        let label_x = w as usize - label.len() * char_w - 4;
        bitmap_font::draw_text(fb, label, label_x, status_y as usize + 2, FOLLOW_COLOR);
    }
}

/// Format a line number right-aligned into a 5-byte buffer.
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

/// Format "L:offset/total" into buffer, return length.
fn format_line_info(buf: &mut [u8], offset: usize, total: usize) -> usize {
    let mut pos = 0;
    buf[pos] = b'L'; pos += 1;
    buf[pos] = b':'; pos += 1;
    pos += write_usize(&mut buf[pos..], offset);
    buf[pos] = b'/'; pos += 1;
    pos += write_usize(&mut buf[pos..], total);
    pos
}

/// Write a usize as ASCII digits, return bytes written.
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

    // Create window
    let mut win = match Window::new(b"Log Viewer", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("[blog] Window::new failed: {} -- exiting", e);
            process::exit(1);
        }
    };
    println!("[blog] Window {} ({}x{})", win.id(), WIN_W, WIN_H);

    // Set up theme (use bitmap font for anti-aliased text)
    let mut theme = Theme::dark();
    theme.use_bitmap_font = true;

    // Create tab bar
    let tab_rect = Rect::new(0, 0, WIN_W as i32, TAB_BAR_H);
    let mut tab_bar = TabBar::new(tab_rect, vec![b"Kernel", b"Serial"]);

    // Create log tabs
    let mut tabs = vec![
        LogTab::new("/var/log/kernel.log"),
        LogTab::new("/var/log/serial.log"),
    ];

    // Open files
    for tab in tabs.iter_mut() {
        tab.open();
    }

    // Get font metrics for visible line calculations
    let fm = bitmap_font::metrics();
    let line_h = fm.line_height();
    let content_h = WIN_H as i32 - TAB_BAR_H - STATUS_BAR_H;
    let visible_lines = if content_h > 0 && line_h > 0 {
        content_h as usize / line_h
    } else {
        1
    };

    // Start in follow mode: scroll to bottom
    for tab in tabs.iter_mut() {
        if tab.follow {
            tab.scroll_to_bottom(visible_lines);
        }
    }

    // Mouse state for tab bar interaction
    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut mouse_buttons: u32 = 0;
    let mut poll_counter: u32 = 0;

    // Initial render
    render(&mut win, &tabs, &tab_bar, &theme);
    let _ = win.present();

    loop {
        // Poll Breengel events
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
                            // Handle ASCII keys
                            match ascii {
                                b'q' | b'Q' => {
                                    // Close file descriptors
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
                                    // Tab key: switch to next tab
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

        // Periodic file content polling (every ~50ms * 10 = 500ms)
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

        // Render and present if anything changed
        if needs_redraw {
            render(&mut win, &tabs, &tab_bar, &theme);
            let _ = win.present();
        }

        // Sleep 50ms between polls
        let _ = time::sleep_ms(50);
    }
}
