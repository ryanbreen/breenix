//! bterm — Standalone terminal emulator using Breengel windowing.
//!
//! Each tab spawns its own PTY + shell (bsh). Keyboard input flows to the
//! active tab's PTY master; PTY output is fed through a VT100/ANSI parser
//! and drawn into the Breengel window's GPU surface.
//!
//! Keyboard shortcuts:
//!   Ctrl+T       — open a new tab
//!   Ctrl+W       — close the active tab (exits if last tab)
//!   Ctrl+Plus/=  — increase font size
//!   Ctrl+Minus   — decrease font size
//!   Shift+PgUp/PgDn — scroll history by a page; wheel scrolls three rows

#[path = "bterm/selection.rs"]
mod selection;

#[path = "bterm/terminal_node.rs"]
mod terminal_node;

#[path = "bterm/exit_row.rs"]
mod exit_row;

#[path = "bterm/keys.rs"]
mod keys;

#[path = "bterm/session.rs"]
mod session;

#[path = "bterm/paint.rs"]
mod paint;

#[path = "bterm/cells.rs"]
mod cells;

use libbreenix::window::abi::frame_stage;
use std::process;

use breengel::{Window, Event, Handled, CachedFont, TabBar, Rect, Theme, Color, Surface};
use libbreenix::io;
use libbreenix::fs;
use libbreenix::process::{fork, execv, setsid, ForkResult};
use libbreenix::pty;
use libbreenix::termios;
use libbreenix::types::Fd;
use libbreenix::time;

use libgfx::bitmap_font;
use libgfx::shapes;
use libgfx::ttf_font;

use libbui::{InputState, WidgetEvent};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Min/max font size for Ctrl+/- resizing.
const MIN_FONT_SIZE: f32 = 8.0;
const MAX_FONT_SIZE: f32 = 32.0;
const FONT_SIZE_STEP: f32 = 1.0;

/// Tab bar height in logical UI units.
const TAB_BAR_HEIGHT: i32 = 24;

// ─── Window dimensions ──────────────────────────────────────────────────────

const WIN_WIDTH: u32 = 750;
const WIN_HEIGHT: u32 = 550;

struct TermEmu { vt: libvt::TermEmu, selection: selection::Selection, dirty: bool, paint: paint::Paint }
impl TermEmu {
    fn new(cols: usize, rows: usize) -> Self { Self { vt: libvt::TermEmu::new(cols, rows), selection: selection::Selection::default(), dirty: true, paint: paint::Paint::default() } }
    fn invalidate(&mut self) { self.paint.invalidate(); self.dirty = true; }
    fn resize(&mut self, cols: usize, rows: usize) { self.vt.resize(cols, rows); self.selection.clear(); self.invalidate(); }
    fn feed(&mut self, bytes: &[u8], master_fd: Fd) {
        let generation = self.vt.scroll_generation();
        let screen = self.vt.screen_generation();
        let offset = self.vt.view_offset();
        self.vt.feed(bytes);
        if self.vt.scroll_generation() != generation || self.vt.screen_generation() != screen || self.vt.view_offset() != offset { self.selection.clear(); }
        self.dirty = true;
        self.flush_replies(master_fd);
    }
    fn flush_replies(&mut self, master_fd: Fd) {
        while !self.vt.replies.is_empty() {
            let (first, _) = self.vt.replies.as_slices();
            match io::write(master_fd, first) {
                Ok(0) | Err(_) => break,
                Ok(n) => { self.vt.replies.drain(..n); }
            }
        }
    }
    /// Paint changed cells: backgrounds as runs of one colour, then the characters
    /// at their cells' origins in as few draws as their colours allow, then the cursor's
    /// underline. A cell is drawn only when it lies inside the clip area, edge to edge.
    fn render(&mut self, fb: &mut Surface, x_off: usize, y_off: usize,
              clip_w: usize, clip_h: usize, cell_w: usize, cell_h: usize,
              font_size: f32, bitmap_height: bitmap_font::RasterHeight, ttf: Option<&mut CachedFont>) {
        if !self.dirty { return; }
        self.dirty = false;
        let max_x = (x_off + clip_w).min(fb.width());
        let max_y = (y_off + clip_h).min(fb.height());
        let rows = self.vt.size().1.min(max_y.saturating_sub(y_off) / cell_h.max(1));
        let cols = self.vt.size().0.min(max_x.saturating_sub(x_off) / cell_w.max(1));
        let changed = self.paint.changes(&self.vt, &self.selection,
            (x_off, y_off, cols, rows, cell_w, cell_h, font_size.to_bits()));
        if !changed.iter().any(|&cell| cell) { return; }
        let fg_of = |cell: &libvt::Cell, selected: bool| {
            let c = if cell.inverse ^ selected { cell.bg } else { cell.fg };
            if cell.bold { Color::rgb(c.r.saturating_add(40), c.g.saturating_add(40), c.b.saturating_add(40)) }
            else { Color::rgb(c.r, c.g, c.b) }
        };
        let bg_of = |cell: &libvt::Cell, selected: bool| if cell.inverse ^ selected { cell.fg } else { cell.bg };
        fb.batch(|fb| {
            for row in 0..rows {
                let py = y_off + row * cell_h;
                let mut col = 0;
                while col < cols {
                    if !changed[row * cols + col] { col += 1; continue; }
                    let bg = bg_of(self.vt.view_cell(col, row), self.selection.contains(col, row));
                    let start = col;
                    while col < cols && changed[row * cols + col] && bg_of(self.vt.view_cell(col, row), self.selection.contains(col, row)) == bg {
                        col += 1;
                    }
                    shapes::fill_rect(fb, (x_off + start * cell_w) as i32, py as i32,
                        ((col - start) * cell_w) as i32, cell_h as i32, Color::rgb(bg.r, bg.g, bg.b));
                }
            }
        });
        let glyphs: Vec<(usize, usize, Color)> = (0..rows)
            .flat_map(|row| (0..cols).map(move |col| (col, row)))
            .filter(|&(col, row)| changed[row * cols + col] && self.vt.view_cell(col, row).ch != ' ')
            .map(|(col, row)| (col, row, fg_of(self.vt.view_cell(col, row), self.selection.contains(col, row))))
            .collect();
        if let Some(font) = ttf {
            let cells: Vec<_> = glyphs.iter().map(|&(col, row, fg)| ttf_font::CellGlyph {
                ch: self.vt.view_cell(col, row).ch,
                x: (x_off + col * cell_w) as i32,
                y: (y_off + row * cell_h) as i32,
                fg,
            }).collect();
            fb.push_clip(libgfx::surface::Rect::new(x_off as i32, y_off as i32,
                (max_x - x_off) as i32, (max_y - y_off) as i32));
            ttf_font::draw_cells(fb, font, &cells, font_size, cell_w, cell_h);
            fb.pop_clip();
        } else {
            // The bitmap font's advance is the cell width: a row's characters of one colour
            // in a run are one draw.
            let mut i = 0;
            while i < glyphs.len() {
                let (col, row, fg) = glyphs[i];
                let mut text = vec![if self.vt.view_cell(col, row).ch.is_ascii() { self.vt.view_cell(col, row).ch as u8 } else { b'?' }];
                let mut j = i + 1;
                while j < glyphs.len() && glyphs[j].1 == row && glyphs[j].2 == fg
                    && glyphs[j].0 == col + text.len() && cell_w == bitmap_font::metrics_at(bitmap_height).char_width {
                    let ch = self.vt.view_cell(glyphs[j].0, row).ch;
                    text.push(if ch.is_ascii() { ch as u8 } else { b'?' });
                    j += 1;
                }
                bitmap_font::draw_text_at(fb, &text, x_off + col * cell_w, y_off + row * cell_h, fg, bitmap_height);
                i = j;
            }
        }
        fb.batch(|fb| {
            for row in 0..rows {
                for col in 0..cols {
                    let cell = self.vt.view_cell(col, row);
                    if changed[row * cols + col] && cell.underline {
                        shapes::fill_rect(fb, (x_off + col * cell_w) as i32,
                            (y_off + (row + 1) * cell_h - 1) as i32,
                            cell_w as i32, 1, fg_of(cell, self.selection.contains(col, row)));
                    }
                }
            }
        });
        // Cursor underline
        let (cursor_x, cursor_y) = self.vt.cursor();
        if self.vt.view_offset() == 0 && self.vt.cursor_visible() && cursor_y < rows && cursor_x < cols
            && changed[cursor_y * cols + cursor_x] {
            let cx = x_off + cursor_x * cell_w;
            let cy = y_off + cursor_y * cell_h + cell_h - 2;
            let w = cell_w.min(max_x.saturating_sub(cx));
            let h = 2usize.min(max_y.saturating_sub(cy));
            if w > 0 && h > 0 {
                shapes::fill_rect(fb, cx as i32, cy as i32, w as i32, h as i32, Color::WHITE);
            }
        }
    }
}

// ─── Tab ────────────────────────────────────────────────────────────────────

struct Tab {
    emu: TermEmu,
    master_fd: Fd,
    pending_input: Vec<u8>,
    child_pid: i64,
    session_pid: i64,
    show_exit: bool,
}

impl Drop for Tab {
    fn drop(&mut self) {
        if self.session_pid > 1 {
            let child = (self.child_pid > 0).then_some(self.child_pid as i32);
            if let Err(error) = session::hangup_and_reap(self.session_pid as i32, child, self.master_fd.raw() as i32) {
                eprintln!("[bterm] close tab: {error}");
            }
        } else {
            let _ = io::close(self.master_fd);
        }
    }
}

fn winsize(cols: usize, rows: usize, width: u32, height: u32) -> termios::Winsize {
    termios::Winsize { ws_row: rows as u16, ws_col: cols as u16,
        ws_xpixel: width as u16, ws_ypixel: height as u16 }
}

impl Tab {
    fn send_input(&mut self, bytes: &[u8]) {
        self.pending_input.extend_from_slice(bytes);
        self.flush_input();
    }
    fn flush_input(&mut self) {
        while !self.pending_input.is_empty() {
            match io::write(self.master_fd, &self.pending_input) {
                Ok(0) | Err(_) => break,
                Ok(n) => { self.pending_input.drain(..n); }
            }
        }
    }
    fn resize(&mut self, cols: usize, rows: usize, width: u32, height: u32) {
        self.emu.resize(cols, rows);
        if let Err(error) = termios::set_winsize(self.master_fd, &winsize(cols, rows, width, height)) {
            eprintln!("[bterm] PTY resize failed: {error}");
        }
    }
}

// ─── Process Spawning ───────────────────────────────────────────────────────

fn spawn_child(args: &[String], size: &termios::Winsize) -> (Fd, i64) {
    let strings: Vec<std::ffi::CString> = args.iter().map(|s| std::ffi::CString::new(s.as_bytes()).expect("argument contains NUL")).collect();
    let mut argv: Vec<*const u8> = strings.iter().map(|s| s.as_ptr().cast()).collect();
    argv.push(core::ptr::null());
    let path = strings[0].as_bytes_with_nul();
    let (master_fd, slave_name) = match pty::openpty() {
        Ok((m, s)) => (m, s),
        Err(_) => {
            print!("[bterm] openpty failed\n");
            return (Fd::from_raw(u64::MAX), -1);
        }
    };

    // Initialize the PTY before fork, so even a child that runs first sees its grid.
    if let Err(error) = termios::set_winsize(master_fd, size) {
        eprintln!("[bterm] initial PTY size: {error}");
    }

    // Build slave path on the STACK (not heap) to avoid CoW corruption after fork.
    // slave_name is [u8; 32] containing e.g. "/dev/pts/0\0..."
    let mut slave_path: [u8; 33] = [0; 33];
    let len = slave_name.iter().position(|&b| b == 0).unwrap_or(slave_name.len()).min(32);
    slave_path[..len].copy_from_slice(&slave_name[..len]);
    slave_path[len] = 0; // null terminator
    let slave_path_str = core::str::from_utf8(&slave_path[..=len]).unwrap_or("/dev/pts/0\0");

    match fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(master_fd);
            let _ = setsid();
            // Init ignores HUP while detaching its console. A new tab session
            // must receive HUP even when another tab still holds its master FD.
            if libbreenix::signal::sigaction(libbreenix::signal::SIGHUP,
                Some(&libbreenix::signal::Sigaction::default_action()), None).is_err() {
                libbreenix::process::exit(126);
            }
            let slave_fd = match fs::open(slave_path_str, fs::O_RDWR) {
                Ok(fd) => fd,
                Err(_) => { libbreenix::process::exit(126); }
            };
            let _ = libbreenix::termios::set_controlling_terminal(slave_fd);
            let _ = io::dup2(slave_fd, Fd::from_raw(0));
            let _ = io::dup2(slave_fd, Fd::from_raw(1));
            let _ = io::dup2(slave_fd, Fd::from_raw(2));
            if slave_fd.raw() > 2 { let _ = io::close(slave_fd); }
            let _ = execv(path, argv.as_ptr());
            libbreenix::process::exit(127);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            print!("[bterm] spawned child pid={}\n", child_pid.raw());
            (master_fd, child_pid.raw() as i64)
        }
        Err(_) => {
            print!("[bterm] fork failed\n");
            let _ = io::close(master_fd);
            (Fd::from_raw(u64::MAX), -1)
        }
    }
}

// ─── Tab helpers ────────────────────────────────────────────────────────────

/// Generate a label for a new tab.
/// We use a static counter to keep labels unique.
static mut TAB_COUNTER: usize = 0;

/// Tab label storage — we need 'static lifetimes for TabBar labels.
fn make_tab_label() -> &'static [u8] {
    let n = unsafe {
        TAB_COUNTER += 1;
        TAB_COUNTER
    };
    let s = format!("shell {}", n);
    let boxed: Box<[u8]> = s.into_bytes().into_boxed_slice();
    Box::leak(boxed)
}

fn spawn_tab(cols: usize, rows: usize, width: u32, height: u32) -> Tab {
    spawn_tab_cmd(cols, rows, width, height, &["/bin/bsh".into()], false)
}

fn spawn_tab_cmd(cols: usize, rows: usize, width: u32, height: u32, args: &[String], show_exit: bool) -> Tab {
    let (master_fd, child_pid) = spawn_child(args, &winsize(cols, rows, width, height));
    let _ = io::fcntl_setfl(master_fd, io::status_flags::O_NONBLOCK);
    Tab {
        emu: TermEmu::new(cols, rows),
        master_fd,
        pending_input: Vec::new(),
        child_pid,
        session_pid: child_pid,
        show_exit,
    }
}

fn make_static_label(s: &str) -> &'static [u8] {
    let boxed: Box<[u8]> = s.as_bytes().to_vec().into_boxed_slice();
    Box::leak(boxed)
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    print!("[bterm] starting\n");
    // Create window (loads system font automatically from /etc/fonts.conf)
    let scale = libbreenix::gpu::current_mode().expect("Terminal display mode").scale.max(1);
    let mut win = match Window::new(b"Terminal", WIN_WIDTH * scale, WIN_HEIGHT * scale) {
        Ok(w) => w,
        Err(e) => {
            print!("[bterm] ERROR: failed to create window: {}\n", e);
            process::exit(1);
        }
    };
    win.set_font_poll_interval(100); // ~1s at 10ms sleep

    let mut font_size = if win.mono_size() >= 6.0 { win.mono_size() } else { 14.0 };
    print!("[bterm] config: mono_size={} font_size={}\n", win.mono_size(), font_size);

    // Take TrueType font for local use (needed alongside the window's surface)
    let mut ttf_font: Option<CachedFont> = win.take_mono_font();

    // Widgets use logical UI coordinates; the terminal grid uses device pixels.
    let mut ui_scale = win.surface().expect("terminal surface").ui_scale() as i32;
    let (mut device_font_size, mut bitmap_height) = libbui::text::scaled_font(font_size, ui_scale as u32);
    let (mut cell_w, mut cell_h) = cells::cell_dims(ttf_font.as_mut(), font_size, ui_scale as u32);
    let mut tab_bar_pixels = (TAB_BAR_HEIGHT * ui_scale) as u32;
    // Calculate content area (below tab bar)
    let mut content_w = win.width();
    let mut content_h = win.height().saturating_sub(tab_bar_pixels);
    let mut cols = (content_w as usize / cell_w).max(1);
    let mut rows = (content_h as usize / cell_h).max(1);

    // A normal window starts at a shell; -e runs the requested argv directly.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exec_args = if args.first().map(String::as_str) == Some("-e") {
        if args.len() < 2 { eprintln!("bterm: -e requires a program"); process::exit(2); }
        let mut command = args[1..].to_vec();
        if !command[0].contains('/') {
            if let Some(path) = std::env::var_os("PATH") {
                if let Some(found) = std::env::split_paths(&path).map(|dir| dir.join(&command[0])).find(|p| p.is_file()) {
                    command[0] = found.to_string_lossy().into_owned();
                }
            }
        }
        Some(command)
    } else { None };
    let theme = Theme::dark();
    let label = if let Some(ref cmd) = exec_args { make_static_label(&cmd[0]) } else { make_tab_label() };
    let mut tab_bar = TabBar::new(
        Rect::new(0, 0, win.width() as i32 / ui_scale, TAB_BAR_HEIGHT),
        vec![label], &theme);
    let mut tabs = vec![match exec_args {
        Some(ref cmd) => spawn_tab_cmd(cols, rows, content_w, content_h, cmd, true),
        None => spawn_tab(cols, rows, content_w, content_h),
    }];

    // Mouse state for InputState edge detection
    let mut prev_buttons: u32 = 0;

    // Read buffer for PTY output
    let mut pty_buf = [0u8; 4096];

    // Sleep duration for the main loop (10ms)
    let sleep_ts = libbreenix::types::Timespec { tv_sec: 0, tv_nsec: 10_000_000 };
    let mut first_frame = true;
    let mut first_present = true;
    let mut last_key_serial = 0;
    let mut frame_id = 0u64;
    let mut had_tree_readers = false;

    loop {
        // Dispatch input before the rendered commit publishes its watermark.
        let window_id = win.id();
        let mut close_requested = false;
        let mut reload_font = false;
        let mut resize_requested = None;
        let mut resized = None;
        let mut mouse_x: i32 = 0;
        let mut mouse_y: i32 = 0;
        let mut buttons: u32 = prev_buttons;
        let mut font_changed = false;

        let event_count = win.dispatch(|delivered| {
            match &delivered.event {
                Event::CloseRequested => {
                    close_requested = true;
                    return Handled::Redraw;
                }
                Event::KeyPress { ascii, keycode, modifiers, text, .. } => {
                    last_key_serial = delivered.serial;
                    if modifiers.shift && (*keycode == breengel::keys::PAGE_UP || *keycode == breengel::keys::PAGE_DOWN) {
                        if let Some(tab) = tabs.get_mut(tab_bar.selected()) {
                            let page = tab.emu.vt.size().1 as isize;
                            tab.emu.selection.clear();
                            tab.emu.vt.scroll_view(if *keycode == breengel::keys::PAGE_UP { page } else { -page });
                            tab.emu.dirty = true;
                        }
                        return Handled::Redraw;
                    }
                    // Debug: log key events for +/- and ctrl
                    if *ascii == b'+' || *ascii == b'=' || *ascii == b'-'
                       || *keycode == breengel::keys::MINUS || *keycode == breengel::keys::EQUAL
                    {
                        print!("[bterm] key: ascii={} keycode=0x{:02X} ctrl={}\n",
                               *ascii, *keycode, modifiers.ctrl);
                    }
                    if modifiers.ctrl && modifiers.shift && *keycode == 46 {
                        if let Some(tab) = tabs.get(tab_bar.selected()) {
                            if let Some(text) = tab.emu.selection.text(&tab.emu.vt) { let _ = libclipboard::set(window_id, text.as_bytes()); }
                        }
                        return Handled::Redraw;
                    }
                    if modifiers.ctrl && modifiers.shift && *keycode == libbreenix::input::key::V as u32 {
                        if let Some(tab) = tabs.get_mut(tab_bar.selected()) {
                            if let Ok(data) = libclipboard::get(window_id) {
                                let mut bytes = Vec::new();
                                if tab.emu.vt.bracketed_paste() { bytes.extend_from_slice(b"\x1b[200~"); }
                                bytes.extend_from_slice(&data);
                                if tab.emu.vt.bracketed_paste() { bytes.extend_from_slice(b"\x1b[201~"); }
                                tab.send_input(&bytes);
                            }
                        }
                        return Handled::Redraw;
                    }
                    if modifiers.ctrl {
                        // Ctrl+T: new tab
                        if *ascii == b't' - b'a' + 1 || *ascii == b'T' - b'A' + 1
                           || *keycode == breengel::keys::T
                        {
                            let label = make_tab_label();
                            let idx = tab_bar.add_tab(label);
                            tabs.push(spawn_tab(cols, rows, content_w, content_h));
                            tab_bar.set_selected(idx);
                            if let Some(tab) = tabs.get_mut(idx) {
                                tab.emu.dirty = true;
                            }
                            return Handled::Redraw;
                        }
                        // Ctrl+W: close tab
                        if *ascii == b'w' - b'a' + 1 || *ascii == b'W' - b'A' + 1
                           || *keycode == breengel::keys::W
                        {
                            let sel = tab_bar.selected();
                            if tabs.len() <= 1 {
                                close_requested = true;
                                return Handled::Redraw;
                            }
                            tabs.remove(sel);
                            tab_bar.remove_tab(sel);
                            let new_sel = tab_bar.selected();
                            if let Some(tab) = tabs.get_mut(new_sel) {
                                tab.emu.invalidate();
                            }
                            return Handled::Redraw;
                        }
                        // Ctrl+C: send interrupt to PTY
                        if text.as_bytes() == [3] {
                            let sel = tab_bar.selected();
                            if let Some(tab) = tabs.get_mut(sel) {
                                tab.send_input(&[3]);
                            }
                            return Handled::Redraw;
                        }
                        // Ctrl+= or Ctrl++: increase font size
                        if *ascii == b'+' || *ascii == b'=' || *keycode == breengel::keys::EQUAL {
                            if ttf_font.is_some() {
                                let new_size = (font_size + FONT_SIZE_STEP).min(MAX_FONT_SIZE);
                                if new_size != font_size {
                                    print!("[bterm] font size: {} -> {}\n", font_size, new_size);
                                    font_size = new_size;
                                    font_changed = true;
                                }
                            }
                            return Handled::Redraw;
                        }
                        // Ctrl+-: decrease font size
                        if *ascii == b'-' || *keycode == breengel::keys::MINUS {
                            if ttf_font.is_some() {
                                let new_size = (font_size - FONT_SIZE_STEP).max(MIN_FONT_SIZE);
                                if new_size != font_size {
                                    print!("[bterm] font size: {} -> {}\n", font_size, new_size);
                                    font_size = new_size;
                                    font_changed = true;
                                }
                            }
                            return Handled::Redraw;
                        }
                    }

                    let sel = tab_bar.selected();
                    if let Some(tab) = tabs.get_mut(sel) {
                        let bytes = keys::encode(*keycode, modifiers.ctrl, text, tab.emu.vt.application_cursor_keys());
                        if !bytes.is_empty() { tab.send_input(&bytes); }
                    }
                }
                Event::Text { text } => {
                    if let Some(tab) = tabs.get_mut(tab_bar.selected()) {
                        let bytes = keys::encode_text(text,
                            libbreenix::input::abi::derived_source(delivered.serial).map(|(source, _)| source),
                            last_key_serial);
                        if !bytes.is_empty() { tab.send_input(&bytes); }
                    }
                }
                Event::Scroll { delta_y, .. } => {
                    if let Some(tab) = tabs.get_mut(tab_bar.selected()) {
                        tab.emu.selection.clear();
                        tab.emu.vt.scroll_view((*delta_y as isize).saturating_mul(3));
                        tab.emu.dirty = true;
                    }
                }
                Event::MouseMove { x, y } => {
                    mouse_x = *x;
                    mouse_y = *y;
                    if buttons & 1 != 0 {
                        if let Some(tab) = tabs.get_mut(tab_bar.selected()) {
                            tab.emu.selection.update(*x, *y, selection::Geometry { x: 0, y: tab_bar_pixels as i32, w: cell_w, h: cell_h }, tab.emu.vt.size());
                            tab.emu.dirty = true;
                        }
                    }
                }
                Event::MouseButton { button, pressed, x, y } => {
                    mouse_x = *x;
                    mouse_y = *y;
                    if *button == breengel::buttons::LEFT {
                        if let Some(tab) = tabs.get_mut(tab_bar.selected()) {
                            let g = selection::Geometry { x: 0, y: tab_bar_pixels as i32, w: cell_w, h: cell_h };
                            if *pressed && *y >= g.y { tab.emu.selection.press(*x, *y, g, tab.emu.vt.size()); }
                            if !*pressed { tab.emu.selection.release(*x, *y, g, tab.emu.vt.size()); }
                            tab.emu.dirty = true;
                        }
                        if *pressed {
                            buttons |= 1;
                        } else {
                            buttons &= !1;
                        }
                    }
                }
                Event::FontChanged => {
                    reload_font = true;
                    font_changed = true;
                }
                Event::ResizeRequested { width, height } => {
                    resize_requested = Some((*width, *height));
                }
                Event::Resized { width, height } => {
                    resized = Some((*width, *height));
                }
                _ => {}
            }
            Handled::Redraw
        }).unwrap_or_else(|error| {
            eprintln!("[bterm] event dispatch failed: {error}");
            process::exit(1);
        });
        if close_requested { break; }
        // These operations borrow Window, so finish them before acquiring a frame.
        if reload_font {
            win.put_mono_font(ttf_font.take());
            ttf_font = win.take_mono_font();
            font_size = win.mono_size();
        }
        if let Some((width, height)) = resize_requested {
            if let Err(error) = win.apply_resize(width, height) {
                eprintln!("[bterm] resize refused: {width}x{height}: {error}");
            }
        }

        // Refresh from the current surface after dispatch has applied its resize.
        // Keep the configured font logical; measurement, painting and selection
        // share the resulting device cell dimensions.
        if font_changed || resized.is_some() {
            if font_changed {
                if let Some(font) = ttf_font.as_mut() { font.clear_cache(); }
            }
            ui_scale = win.surface().expect("terminal surface").ui_scale() as i32;
            (device_font_size, bitmap_height) = libbui::text::scaled_font(font_size, ui_scale as u32);
            (cell_w, cell_h) = cells::cell_dims(ttf_font.as_mut(), font_size, ui_scale as u32);
            tab_bar_pixels = (TAB_BAR_HEIGHT * ui_scale) as u32;
            content_w = win.width();
            content_h = win.height().saturating_sub(tab_bar_pixels);
            let new_cols = (content_w as usize / cell_w).max(1);
            let new_rows = (content_h as usize / cell_h).max(1);
            for tab in &mut tabs {
                if resized.is_some() || new_cols != cols || new_rows != rows {
                    tab.resize(new_cols, new_rows, content_w, content_h);
                } else {
                    tab.emu.invalidate();
                }
            }
            cols = new_cols;
            rows = new_rows;
            tab_bar.set_rect(Rect::new(0, 0, content_w as i32 / ui_scale, TAB_BAR_HEIGHT));
        }

        // Pass mouse state to TabBar for tab switching
        let input = InputState::from_raw(mouse_x.div_euclid(ui_scale), mouse_y.div_euclid(ui_scale), buttons, prev_buttons, breengel::clock::now_ns() / 1_000_000);
        if let WidgetEvent::ValueChanged(_) = tab_bar.update(&input) {
            let sel = tab_bar.selected();
            if let Some(tab) = tabs.get_mut(sel) {
                tab.emu.invalidate();
            }
        }
        prev_buttons = buttons;

        // ── 2. Read PTY output for ALL tabs (non-blocking) ──────────
        for tab in tabs.iter_mut() {
            tab.flush_input();
            loop {
                match io::read(tab.master_fd, &mut pty_buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        tab.emu.feed(&pty_buf[..n], tab.master_fd);
                    }
                    Err(_) => break, // EAGAIN or error
                }
            }
            tab.emu.flush_replies(tab.master_fd);
            if tab.child_pid > 0 {
                let mut status = 0;
                if let Ok(pid) = libbreenix::process::waitpid(tab.child_pid as i32, &mut status, libbreenix::process::WNOHANG) {
                    if pid.raw() != 0 {
                        tab.child_pid = -1;
                        if tab.show_exit {
                            // The child can write and exit between the earlier read and waitpid.
                            while let Ok(n) = io::read(tab.master_fd, &mut pty_buf) {
                                if n == 0 { break; }
                                tab.emu.feed(&pty_buf[..n], tab.master_fd);
                            }
                            let row = format!("\r\n{}", exit_row::exit_row(status));
                            tab.emu.feed(row.as_bytes(), tab.master_fd);
                        }
                    }
                }
            }
        }

        // ── 3. Render ───────────────────────────────────────────────
        let sel = tab_bar.selected();
        let any_dirty = tabs.get(sel).map_or(false, |t| t.emu.dirty);
        // MAP_TREE is passive. Publish semantic state even when a reader attaches
        // after the shell's last output and there is no input to redraw the grid.
        let tree_readers = win.tree_reader_count() != 0;
        let tree_reader_attached = tree_readers && !had_tree_readers;
        had_tree_readers = tree_readers;
        let need_redraw = any_dirty || event_count != 0 || font_changed || first_frame || tree_reader_attached;
        first_frame = false;

        if need_redraw {
            frame_id = frame_id.wrapping_add(1);
            let trace = |stage| { let _ = libbreenix::window::frame_stage(window_id, frame_id, stage); };
            trace(frame_stage::BEGIN);
            let fb = match win.surface() {
                Ok(fb) => fb,
                Err(e) => {
                    print!("[bterm] ERROR: no window surface: {}\n", e);
                    tabs.clear();
                    process::exit(1);
                }
            };

            trace(frame_stage::ACQUIRED);

            // Draw tab bar
            tab_bar.draw(fb, &theme);
            trace(frame_stage::TABS);

            // Render active tab's terminal emulator into content area. The surface holds
            // the newest committed frame, so a tab whose grid is unchanged is not redrawn.
            if let Some(tab) = tabs.get_mut(sel) {
                tab.emu.render(
                    fb,
                    0,
                    tab_bar_pixels as usize,
                    content_w as usize,
                    content_h as usize,
                    cell_w,
                    cell_h,
                    device_font_size,
                    bitmap_height,
                    ttf_font.as_mut(),
                );
            }

            trace(frame_stage::TERMINAL);

            if let Some(tab) = tabs.get(sel) {
                let bounds = [0, 0, i64::from(win.width()), i64::from(win.height())];
                let mut value = terminal_node::terminal_value(&tab.emu.vt, sel, tabs.len(), win.mono_path(), font_size);
                let fields = terminal_node::selection_fields(&tab.emu.vt, &tab.emu.selection, selection::Geometry { x: 0, y: tab_bar_pixels as i32, w: cell_w, h: cell_h });
                value.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
                win.set_tree(breengel::tree::Tree { root: breengel::tree::Node {
                    key: "grid".into(), parent: None, role: breengel::tree::Role::Terminal,
                    name: "Terminal".into(),
                    value,
                    bounds, clip: bounds, flags: vec![breengel::tree::Flag::Enabled, breengel::tree::Flag::Visible],
                }, children: Vec::new() });
            }
            trace(frame_stage::TREE);
            trace(frame_stage::PRESENT_BEGIN);
            let presented = win.present();
            trace(frame_stage::PRESENT_END);
            match presented {
                Err(e) => print!("[bterm] present failed: {}\n", e),
                Ok(()) if first_present => {
                    first_present = false;
                    print!("[bterm] first frame presented: {}x{}\n", win.width(), win.height());
                    // Observe the selected tab's lower highlight in the committed GPU
                    // front: section 14.3's accent at the scaled logical tab height.
                    let mut pixels = vec![0u8; win.width() as usize * win.height() as usize * 4];
                    match libbreenix::window::capture(win.id(), &mut pixels, None) {
                        Ok(_) => {
                            let (x, y) = (4 * ui_scale as usize, (TAB_BAR_HEIGHT as usize - 1) * ui_scale as usize);
                            let at = (y * win.width() as usize + x) * 4;
                            if pixels[at..at + 3] == [220, 130, 70] {
                                print!("[bterm] tabs read back: bounds=0,0,{},{} window={}x{} scale={} highlight exact\n",
                                    win.width(), tab_bar_pixels, win.width(), win.height(), ui_scale);
                            } else {
                                print!("[bterm] tab layout FAILED: highlight at ({},{}) is {:?}\n", x, y, &pixels[at..at+3]);
                            }
                        }
                        Err(e) => print!("[bterm] tab layout FAILED: capture {}\n", e),
                    }
                }
                Ok(()) => {}
            }
        } else {
            // Nothing to draw — sleep briefly to avoid busy-waiting
            let _ = time::nanosleep(&sleep_ts);
        }
    }
}
