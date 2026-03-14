//! bfontpicker — System font picker for Breenix.
//!
//! Lists available monospace TTF fonts from /usr/share/fonts/ and lets the
//! user select one to set as the system default. Writes the selection to
//! /etc/fonts.conf.
//!
//! Controls:
//!   Up/Down       Navigate font list
//!   Enter/Click   Select font (single click highlights, double-click applies)
//!   Enter         Apply selected font and write config
//!   q/Escape      Quit without changes

use std::process;

use breengel::{Window, Event, Color, FrameBuf};
use libbreenix::fs;
use libbreenix::io;
use libbreenix::time;

use libfont::{Font, CachedFont};
use libgfx::bitmap_font;
use libgfx::ttf_font;
use libgfx::shapes;

// ─── Constants ──────────────────────────────────────────────────────────────

const WIN_W: u32 = 560;
const WIN_H: u32 = 500;
const FONT_DIR: &str = "/usr/share/fonts";
const CONFIG_PATH: &str = "/etc/fonts.conf";
const TITLE: &[u8] = b"System Font Picker";

const BG: Color = Color::rgb(30, 30, 40);
const FG: Color = Color::rgb(204, 204, 204);
const SELECTED_BG: Color = Color::rgb(60, 60, 100);
const ACTIVE_FG: Color = Color::rgb(100, 200, 100);
const HEADER_FG: Color = Color::rgb(140, 140, 180);
const STATUS_BG: Color = Color::rgb(50, 50, 60);
const PREVIEW_BG: Color = Color::rgb(38, 38, 50);
const PREVIEW_BORDER: Color = Color::rgb(60, 60, 80);
const PREVIEW_LABEL: Color = Color::rgb(120, 120, 160);
const PREVIEW_TEXT: Color = Color::rgb(220, 220, 230);

const KEY_UP: u16 = 0x52;
const KEY_DOWN: u16 = 0x51;
const KEY_ENTER: u16 = 0x28;
const KEY_ESCAPE: u16 = 0x29;

const PREVIEW_SAMPLE: &str = "The quick brown fox jumps over the lazy dog";
const PREVIEW_SAMPLE_2: &str = "0123456789 !@#$%^&*() ABCDEFG abcdefg";
const PREVIEW_FONT_SIZE: f32 = 18.0;

/// Height of the preview area at the bottom of the window.
const PREVIEW_HEIGHT: usize = 80;

/// Height of the font list area.
const LIST_TOP: usize = 30;

// ─── Font entry ─────────────────────────────────────────────────────────────

struct FontEntry {
    /// Full path, e.g. "/usr/share/fonts/DejaVuSansMono.ttf"
    path: String,
    /// Display name derived from filename
    name: String,
    /// Loaded font data (loaded lazily for preview)
    data: Option<Vec<u8>>,
}

/// Scan /usr/share/fonts/ for .ttf files.
fn scan_fonts() -> Vec<FontEntry> {
    let mut entries = Vec::new();
    let fd = match fs::open(FONT_DIR, fs::O_RDONLY) {
        Ok(f) => f,
        Err(_) => return entries,
    };

    let mut buf = [0u8; 2048];
    loop {
        let n = match fs::getdents64(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for ent in fs::DirentIter::new(&buf, n) {
            let name_bytes = unsafe { ent.name() };
            // Check for .ttf extension
            if name_bytes.len() >= 4 {
                let ext_start = name_bytes.len() - 4;
                let ext = &name_bytes[ext_start..];
                if (ext[0] == b'.' || ext[0] == b'.')
                    && (ext[1] == b't' || ext[1] == b'T')
                    && (ext[2] == b't' || ext[2] == b'T')
                    && (ext[3] == b'f' || ext[3] == b'F')
                {
                    let name_str = core::str::from_utf8(name_bytes).unwrap_or("?");
                    let display_name = name_str
                        .trim_end_matches(".ttf")
                        .trim_end_matches(".TTF")
                        .replace('-', " ")
                        .replace('_', " ");
                    entries.push(FontEntry {
                        path: format!("{}/{}", FONT_DIR, name_str),
                        name: display_name,
                        data: None,
                    });
                }
            }
        }
    }
    let _ = io::close(fd);

    // Sort by name
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Read current mono font path from /etc/fonts.conf.
fn current_font_path() -> String {
    match std::fs::read_to_string(CONFIG_PATH) {
        Ok(contents) => {
            for line in contents.lines() {
                let line = line.trim();
                if let Some((key, value)) = line.split_once('=') {
                    if key.trim() == "mono.font" {
                        return value.trim().to_string();
                    }
                }
            }
            String::new()
        }
        Err(_) => String::new(),
    }
}

/// Write a new /etc/fonts.conf with the selected font.
fn write_config(font_path: &str) {
    let content = format!(
        "# Breenix System Font Configuration\n\
         # Set by bfontpicker\n\
         #\n\
         mono.font={}\n\
         mono.size=14\n",
        font_path
    );
    let _ = std::fs::write(CONFIG_PATH, content);
}

/// Load font data for a FontEntry if not already loaded.
fn ensure_font_data(entry: &mut FontEntry) {
    if entry.data.is_none() {
        entry.data = std::fs::read(&entry.path).ok();
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

fn render(
    fb: &mut FrameBuf,
    fonts: &mut [FontEntry],
    selected: usize,
    current_path: &str,
    scroll: usize,
    status_msg: &str,
) {
    fb.clear(BG);

    // Title
    bitmap_font::draw_text(fb, TITLE, 10, 8, HEADER_FG);

    let item_h = 22;
    let list_bottom = WIN_H as usize - PREVIEW_HEIGHT - 24;
    let visible = (list_bottom - LIST_TOP) / item_h;

    for i in 0..visible {
        let idx = scroll + i;
        if idx >= fonts.len() {
            break;
        }

        let y = LIST_TOP + i * item_h;

        // Highlight selected entry
        if idx == selected {
            shapes::fill_rect(fb, 0, y as i32, WIN_W as i32, item_h as i32, SELECTED_BG);
        }

        // Show active marker for current font
        let is_active = fonts[idx].path == current_path;
        let name_color = if is_active { ACTIVE_FG } else { FG };

        // Font name
        let name_bytes = fonts[idx].name.as_bytes();
        bitmap_font::draw_text(fb, name_bytes, 10, y + 3, name_color);

        // Active indicator
        if is_active {
            bitmap_font::draw_text(fb, b"*", 2, y + 3, ACTIVE_FG);
        }
    }

    // ── Preview area ────────────────────────────────────────────────────────
    let preview_y = (WIN_H as usize - PREVIEW_HEIGHT - 20) as i32;

    // Border line
    shapes::fill_rect(fb, 0, preview_y, WIN_W as i32, 1, PREVIEW_BORDER);

    // Preview background
    shapes::fill_rect(
        fb,
        0,
        preview_y + 1,
        WIN_W as i32,
        PREVIEW_HEIGHT as i32,
        PREVIEW_BG,
    );

    // Preview label
    let label = format!("Preview: {}", fonts.get(selected).map(|f| f.name.as_str()).unwrap_or("?"));
    bitmap_font::draw_text(fb, label.as_bytes(), 10, (preview_y + 6) as usize, PREVIEW_LABEL);

    // Render sample text with the selected font's TTF data
    if selected < fonts.len() {
        ensure_font_data(&mut fonts[selected]);
        if let Some(ref data) = fonts[selected].data {
            if let Ok(parsed) = Font::parse(data) {
                let mut cached = CachedFont::new(parsed, 128);
                let sample_y = preview_y + 28;
                ttf_font::draw_text(
                    fb,
                    &mut cached,
                    PREVIEW_SAMPLE,
                    10,
                    sample_y,
                    PREVIEW_FONT_SIZE,
                    PREVIEW_TEXT,
                );
                let sample2_y = preview_y + 52;
                ttf_font::draw_text(
                    fb,
                    &mut cached,
                    PREVIEW_SAMPLE_2,
                    10,
                    sample2_y,
                    PREVIEW_FONT_SIZE,
                    PREVIEW_TEXT,
                );
            }
        }
    }

    // Status bar
    let status_y = WIN_H as i32 - 20;
    shapes::fill_rect(fb, 0, status_y, WIN_W as i32, 20, STATUS_BG);
    if !status_msg.is_empty() {
        bitmap_font::draw_text(fb, status_msg.as_bytes(), 8, status_y as usize + 3, ACTIVE_FG);
    } else {
        bitmap_font::draw_text(
            fb,
            b"Click/Enter: apply  Up/Down: navigate  q: quit",
            8,
            status_y as usize + 3,
            HEADER_FG,
        );
    }
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    println!("[bfontpicker] starting");

    let mut fonts = scan_fonts();
    if fonts.is_empty() {
        println!("[bfontpicker] no fonts found in {}", FONT_DIR);
        process::exit(1);
    }

    let current_path = current_font_path();
    let mut selected: usize = 0;
    // Start with current font selected
    for (i, f) in fonts.iter().enumerate() {
        if f.path == current_path {
            selected = i;
            break;
        }
    }

    let mut win = match Window::new(b"Font Picker", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("[bfontpicker] Window::new failed: {}", e);
            process::exit(1);
        }
    };

    let item_h = 22usize;
    let list_bottom = WIN_H as usize - PREVIEW_HEIGHT - 24;
    let visible = (list_bottom - LIST_TOP) / item_h;
    let mut scroll: usize = 0;
    let mut status_msg = String::new();
    let mut status_timer: u32 = 0;

    // Track last click time for double-click detection
    let mut last_click_ms: u64 = 0;
    let mut last_click_idx: usize = usize::MAX;

    // Ensure scroll brings current selection into view
    if selected >= visible {
        scroll = selected - visible + 1;
    }

    // Initial render
    {
        let fb = win.framebuf();
        render(fb, &mut fonts, selected, &current_path, scroll, &status_msg);
    }
    let _ = win.present();

    let mut current = current_path;

    loop {
        let events = win.poll_events();
        let mut needs_redraw = !events.is_empty();

        for event in events {
            match event {
                Event::KeyPress { ascii, keycode, .. } => {
                    match keycode {
                        KEY_UP => {
                            if selected > 0 {
                                selected -= 1;
                                if selected < scroll {
                                    scroll = selected;
                                }
                            }
                        }
                        KEY_DOWN => {
                            if selected + 1 < fonts.len() {
                                selected += 1;
                                if selected >= scroll + visible {
                                    scroll = selected - visible + 1;
                                }
                            }
                        }
                        KEY_ENTER => {
                            write_config(&fonts[selected].path);
                            current = fonts[selected].path.clone();
                            status_msg = format!("Set: {}", fonts[selected].name);
                            status_timer = 40;
                        }
                        KEY_ESCAPE => {
                            process::exit(0);
                        }
                        _ => {
                            // Also handle Enter by ASCII in case keycode
                            // mapping differs (e.g. '\r' or '\n')
                            if ascii == b'\r' || ascii == b'\n' {
                                write_config(&fonts[selected].path);
                                current = fonts[selected].path.clone();
                                status_msg = format!("Set: {}", fonts[selected].name);
                                status_timer = 40;
                            } else if ascii == b'q' || ascii == b'Q' {
                                process::exit(0);
                            }
                        }
                    }
                }
                Event::MouseButton { button: 1, pressed: true, x: _, y } => {
                    // Check if click is within the font list area
                    let ly = y as usize;
                    if ly >= LIST_TOP && ly < list_bottom {
                        let clicked_idx = scroll + (ly - LIST_TOP) / item_h;
                        if clicked_idx < fonts.len() {
                            // Double-click detection: apply font
                            let now_ms = time::now_monotonic()
                                .map(|ts| ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000)
                                .unwrap_or(0);
                            if clicked_idx == last_click_idx
                                && now_ms.saturating_sub(last_click_ms) < 400
                            {
                                // Double-click — apply the font
                                selected = clicked_idx;
                                write_config(&fonts[selected].path);
                                current = fonts[selected].path.clone();
                                status_msg = format!("Set: {}", fonts[selected].name);
                                status_timer = 40;
                                last_click_idx = usize::MAX;
                            } else {
                                // Single click — highlight
                                selected = clicked_idx;
                                last_click_ms = now_ms;
                                last_click_idx = clicked_idx;
                            }
                        }
                    }
                }
                Event::CloseRequested => {
                    process::exit(0);
                }
                _ => {}
            }
        }

        // Clear status message after timer expires
        if status_timer > 0 {
            status_timer -= 1;
            if status_timer == 0 {
                status_msg.clear();
                needs_redraw = true;
            }
        }

        if needs_redraw {
            let fb = win.framebuf();
            render(fb, &mut fonts, selected, &current, scroll, &status_msg);
            let _ = win.present();
        }

        let _ = time::sleep_ms(50);
    }
}
