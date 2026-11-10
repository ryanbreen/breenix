//! bfontpicker — System font picker for Breenix.
//!
//! Lists available TTF fonts from /usr/share/fonts/ organized into Monospace
//! and Display sections. Selecting a monospace font sets `mono.font` in the
//! system config; selecting a display font sets `display.font`.
//!
//! Controls:
//!   Up/Down       Navigate font list (skips section headers)
//!   Enter/Click   Apply selected font to appropriate config key
//!   +/=           Increase font size for the selected category
//!   -             Decrease font size for the selected category
//!   q/Escape      Quit without changes

use std::process;

use breengel::{Window, Event, Color, Surface, FontConfig, Handled};
#[path = "bfontpicker/conf.rs"] mod conf;
#[path = "bfontpicker/save.rs"]
mod save;
use save::SaveState;
#[path = "bfontpicker/meta.rs"] mod meta;
#[path = "bfontpicker/node.rs"] mod node;
use libbreenix::fs;
use libbreenix::io;
use libbreenix::time;

use libgfx::bitmap_font;
use libgfx::shapes;
use libgfx::ttf_font::{self, CachedFont, Font};

// ─── Constants ──────────────────────────────────────────────────────────────

const WIN_W: u32 = 560;
const WIN_H: u32 = 540;
const FONT_DIR: &str = "/usr/share/fonts";

const BG: Color = Color::rgb(30, 30, 40);
const FG: Color = Color::rgb(204, 204, 204);
const SELECTED_BG: Color = Color::rgb(60, 60, 100);
const ACTIVE_FG: Color = Color::rgb(100, 200, 100);
const HEADER_FG: Color = Color::rgb(140, 140, 180);
const SECTION_FG: Color = Color::rgb(180, 180, 220);
const SECTION_LINE: Color = Color::rgb(80, 80, 120);
const SIZE_FG: Color = Color::rgb(160, 160, 200);
const STATUS_BG: Color = Color::rgb(50, 50, 60);
const PREVIEW_BG: Color = Color::rgb(38, 38, 50);
const PREVIEW_BORDER: Color = Color::rgb(60, 60, 80);
const PREVIEW_LABEL: Color = Color::rgb(120, 120, 160);
const PREVIEW_TEXT: Color = Color::rgb(220, 220, 230);

const KEY_UP: u32 = breengel::keys::UP;
const KEY_DOWN: u32 = breengel::keys::DOWN;
const KEY_ENTER: u32 = breengel::keys::ENTER;
const KEY_ESCAPE: u32 = breengel::keys::ESC;

const PREVIEW_SAMPLE: &str = "The quick brown fox jumps over the lazy dog";
const PREVIEW_SAMPLE_2: &str = "0123456789 !@#$%^&*() ABCDEFG abcdefg";

const PREVIEW_HEIGHT: usize = 100;
const LIST_TOP: usize = 30;
const ITEM_H: usize = 22;

const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 48.0;
const SIZE_STEP: f32 = 1.0;

// ─── Data structures ────────────────────────────────────────────────────────

struct FontEntry {
    path: String,
    name: String,
    is_mono: bool,
    error: Option<String>,
    cached: Option<CachedFont>,
}

enum ListItem {
    SectionHeader { label: &'static str, is_mono: bool },
    Font(usize),
}

// ─── Font classification ────────────────────────────────────────────────────

/// Scan the font directory and classify readable TTF files by metadata.
fn scan_fonts(dir: &str) -> Vec<FontEntry> {
    let mut entries = Vec::new();
    let fd = match fs::open(dir, fs::O_RDONLY) {
        Ok(f) => f,
        Err(_) => return entries,
    };

    let mut buf = [0u8; 4096];
    loop {
        let n = match fs::getdents64(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for ent in fs::DirentIter::new(&buf, n) {
            let name_bytes = unsafe { ent.name() };
            if name_bytes.len() >= 4 {
                let ext_start = name_bytes.len() - 4;
                let ext = &name_bytes[ext_start..];
                if ext[0] == b'.'
                    && (ext[1] == b't' || ext[1] == b'T')
                    && (ext[2] == b't' || ext[2] == b'T')
                    && (ext[3] == b'f' || ext[3] == b'F')
                {
                    let name_str = core::str::from_utf8(name_bytes).unwrap_or("?");
                    let path = format!("{}/{}", dir, name_str);

                    let display_name = name_str
                        .trim_end_matches(".ttf")
                        .trim_end_matches(".TTF")
                        .replace('-', " ")
                        .replace('_', " ");

                    let result = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|data| {
                        let mono = meta::is_mono(&data).map_err(str::to_string)?;
                        Font::parse(&data).map_err(|e| format!("{:?}", e))?;
                        Ok(mono)
                    });
                    let is_mono = result.as_ref().copied().unwrap_or(false);

                    entries.push(FontEntry {
                        path,
                        name: display_name,
                        is_mono,
                        error: result.err(),
                        cached: None,
                    });
                }
            }
        }
    }
    let _ = io::close(fd);

    // Sort: mono first (sorted by name), then display (sorted by name)
    entries.sort_by(|a, b| {
        b.is_mono.cmp(&a.is_mono).then(a.name.cmp(&b.name))
    });
    entries
}

/// Build the visual list with section headers.
fn build_list(fonts: &[FontEntry]) -> Vec<ListItem> {
    let mut items = Vec::new();

    items.push(ListItem::SectionHeader { label: "Monospace", is_mono: true });
    for (i, f) in fonts.iter().enumerate() {
        if f.is_mono {
            items.push(ListItem::Font(i));
        }
    }

    items.push(ListItem::SectionHeader { label: "Display", is_mono: false });
    for (i, f) in fonts.iter().enumerate() {
        if !f.is_mono {
            items.push(ListItem::Font(i));
        }
    }

    items
}

// ─── List navigation (skip headers) ─────────────────────────────────────────

fn next_font_item(list: &[ListItem], current: usize) -> usize {
    let mut i = current + 1;
    while i < list.len() {
        if matches!(list[i], ListItem::Font(_)) {
            return i;
        }
        i += 1;
    }
    current
}

fn prev_font_item(list: &[ListItem], current: usize) -> usize {
    if current == 0 {
        return current;
    }
    let mut i = current - 1;
    loop {
        if matches!(list[i], ListItem::Font(_)) {
            return i;
        }
        if i == 0 {
            return current;
        }
        i -= 1;
    }
}

fn first_font_item(list: &[ListItem]) -> usize {
    for (i, item) in list.iter().enumerate() {
        if matches!(item, ListItem::Font(_)) {
            return i;
        }
    }
    0
}

/// Determine if the currently selected item is in the mono section.
fn selected_is_mono(fonts: &[FontEntry], list: &[ListItem], selected: usize) -> bool {
    match list.get(selected) {
        Some(ListItem::Font(idx)) => fonts[*idx].is_mono,
        _ => true,
    }
}

// ─── Config helpers ─────────────────────────────────────────────────────────

fn format_size(size: f32) -> String {
    let whole = size as u32;
    let frac = ((size - whole as f32) * 10.0 + 0.5) as u32;
    format!("{}.{}", whole, frac)
}

fn format_size_display(size: f32) -> String {
    let whole = size as u32;
    let frac = ((size - whole as f32) * 10.0 + 0.5) as u32;
    if frac == 0 {
        format!("{}px", whole)
    } else {
        format!("{}.{}px", whole, frac)
    }
}

/// Apply the selected font to the appropriate config key.
fn apply_font(
    fonts: &[FontEntry],
    list: &[ListItem],
    selected: usize,
    current_mono: &mut String,
    current_display: &mut String,
    state: &mut SaveState,
) -> Option<String> {
    if let ListItem::Font(idx) = list[selected] {
        let entry = &fonts[idx];
        if let Some(error) = &entry.error {
            state.save = Some("failed"); state.error = Some(error.clone()); return Some(state.message());
        }
        let key = if entry.is_mono { "mono.font" } else { "display.font" };
        if state.update(key, &entry.path) {
            if entry.is_mono { *current_mono = entry.path.clone(); } else { *current_display = entry.path.clone(); }
        }
        Some(state.message())
    } else {
        None
    }
}

/// Adjust font size for the selected category and write config.
fn adjust_size(
    fonts: &[FontEntry],
    list: &[ListItem],
    selected: usize,
    delta: f32,
    mono_size: &mut f32,
    display_size: &mut f32,
    state: &mut SaveState,
) -> Option<String> {
    let is_mono = selected_is_mono(fonts, list, selected);
    let size = if is_mono { mono_size } else { display_size };
    let next = (*size + delta).max(MIN_FONT_SIZE).min(MAX_FONT_SIZE);
    if state.update(if is_mono { "mono.size" } else { "display.size" }, &format_size(next)) { *size = next; }
    Some(state.message())
}

fn ensure_cached_font(entry: &mut FontEntry) {
    if entry.cached.is_none() {
        if let Ok(data) = std::fs::read(&entry.path) {
            if let Ok(parsed) = Font::parse(&data) {
                entry.cached = Some(CachedFont::new(parsed, 128));
            }
        }
    }
}

fn publish_tree(win: &mut Window, fonts: &[FontEntry], list: &[ListItem], selected: usize,
    mono: &str, display: &str, sizes: [f32; 2], state: &SaveState) {
    use breengel::tree::{Flag, Node, Role, Tree};
    let rows: Vec<_> = fonts.iter().map(|font| node::Row {
        name: &font.name, path: &font.path, section: if font.is_mono { "mono" } else { "display" },
        usable: font.error.is_none(), active: if font.is_mono { font.path == mono } else { font.path == display },
    }).collect();
    let selection = match list.get(selected) { Some(ListItem::Font(i)) => Some(*i), _ => None };
    let bounds = [0, 0, i64::from(win.width()), i64::from(win.height())];
    let flags = vec![Flag::Enabled, Flag::Visible];
    win.set_tree(Tree { root: Node {
        key: "fonts".into(), parent: None, role: Role::List, name: "Fonts".into(),
        value: node::fonts(&rows, selection), bounds, clip: bounds, flags: flags.clone(),
    }, children: vec![Node {
        key: "status".into(), parent: Some("fonts".into()), role: Role::Status, name: "Status".into(),
        value: node::status(mono, sizes[0], display, sizes[1], state.save, state.error.as_deref()),
        bounds, clip: bounds, flags,
    }] });
}

// ─── Rendering ──────────────────────────────────────────────────────────────

fn render(
    fb: &mut Surface,
    fonts: &mut [FontEntry],
    list: &[ListItem],
    selected: usize,
    current_mono: &str,
    current_display: &str,
    scroll: usize,
    status_msg: &str,
    sizes: &[f32; 2],  // [mono_size, display_size] — passed by ref to avoid f32 ABI issues
) {
    let mono_size = sizes[0];
    let display_size = sizes[1];
    fb.clear(BG);

    bitmap_font::draw_text(fb, b"System Font Picker", 10, 8, HEADER_FG);

    let list_bottom = WIN_H as usize - PREVIEW_HEIGHT - 24;
    let visible = (list_bottom - LIST_TOP) / ITEM_H;

    for i in 0..visible {
        let idx = scroll + i;
        if idx >= list.len() {
            break;
        }

        let y = LIST_TOP + i * ITEM_H;

        match &list[idx] {
            ListItem::SectionHeader { label, is_mono } => {
                let line_y = y as i32 + ITEM_H as i32 / 2;
                bitmap_font::draw_text(fb, label.as_bytes(), 10, y + 4, SECTION_FG);

                // Show current size and +/- controls
                let size = if *is_mono { mono_size } else { display_size };
                let size_str = format_size_display(size);
                let size_label = format!("  [- {} +]", size_str);
                let label_end = label.len() * 8 + 10;
                bitmap_font::draw_text(
                    fb, size_label.as_bytes(),
                    label_end + 8, y + 4, SIZE_FG,
                );

                let total_label_w = label_end + 8 + size_label.len() * 8 + 8;
                shapes::fill_rect(
                    fb,
                    total_label_w as i32,
                    line_y,
                    WIN_W as i32 - total_label_w as i32 - 4,
                    1,
                    SECTION_LINE,
                );
            }
            ListItem::Font(font_idx) => {
                let font_idx = *font_idx;

                if idx == selected {
                    shapes::fill_rect(
                        fb, 0, y as i32, WIN_W as i32, ITEM_H as i32, SELECTED_BG,
                    );
                }

                let is_active = if fonts[font_idx].is_mono {
                    fonts[font_idx].path == current_mono
                } else {
                    fonts[font_idx].path == current_display
                };
                let name_color = if is_active { ACTIVE_FG } else { FG };

                bitmap_font::draw_text(
                    fb, fonts[font_idx].name.as_bytes(), 20, y + 3, name_color,
                );
                if let Some(error) = &fonts[font_idx].error {
                    let label = format!(" - {}", error);
                    bitmap_font::draw_text(fb, label.as_bytes(), 20 + fonts[font_idx].name.len() * 8, y + 3, FG);
                }

                if is_active {
                    bitmap_font::draw_text(fb, b"*", 10, y + 3, ACTIVE_FG);
                }
            }
        }
    }

    // ── Preview area ────────────────────────────────────────────────────────
    let preview_y = (WIN_H as usize - PREVIEW_HEIGHT - 20) as i32;
    shapes::fill_rect(fb, 0, preview_y, WIN_W as i32, 1, PREVIEW_BORDER);
    shapes::fill_rect(
        fb, 0, preview_y + 1, WIN_W as i32, PREVIEW_HEIGHT as i32, PREVIEW_BG,
    );

    let selected_font_idx = match list.get(selected) {
        Some(ListItem::Font(idx)) => Some(*idx),
        _ => None,
    };

    if let Some(font_idx) = selected_font_idx {
        // Preview at the actual configured size for this category (enforce minimum)
        let preview_size = if fonts[font_idx].is_mono { mono_size } else { display_size };
        let preview_size = preview_size.max(MIN_FONT_SIZE);
        let label = format!("Preview: {} @ {}", fonts[font_idx].name, format_size_display(preview_size));
        bitmap_font::draw_text(
            fb, label.as_bytes(), 10, (preview_y + 6) as usize, PREVIEW_LABEL,
        );

        ensure_cached_font(&mut fonts[font_idx]);
        if let Some(ref mut cached) = fonts[font_idx].cached {
            let metrics = cached.metrics(preview_size);
            let line_h = (metrics.ascender + 0.99) as i32
                + ((-metrics.descender) + 0.99) as i32;
            // Both sample lines, each as `ttf_font::draw_text` draws it, sharing draws.
            let clip = libgfx::surface::Rect::new(0, 0, fb.width() as i32, fb.height() as i32);
            let runs = [
                ttf_font::TextRun { bytes: PREVIEW_SAMPLE.as_bytes(), x: 10, y: preview_y + 24,
                    size: preview_size, fg: PREVIEW_TEXT, clip },
                ttf_font::TextRun { bytes: PREVIEW_SAMPLE_2.as_bytes(), x: 10, y: preview_y + 24 + line_h + 4,
                    size: preview_size, fg: PREVIEW_TEXT, clip },
            ];
            ttf_font::draw_runs(fb, cached, &runs);
        }
    }

    // Status bar
    let status_y = WIN_H as i32 - 20;
    shapes::fill_rect(fb, 0, status_y, WIN_W as i32, 20, STATUS_BG);
    if !status_msg.is_empty() {
        bitmap_font::draw_text(
            fb, status_msg.as_bytes(), 8, status_y as usize + 3, ACTIVE_FG,
        );
    } else {
        bitmap_font::draw_text(
            fb,
            b"Enter: apply  +/-: size  Up/Down: navigate  q: quit",
            8,
            status_y as usize + 3,
            HEADER_FG,
        );
    }
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    println!("[bfontpicker] starting");

    let mut args = std::env::args().skip(1);
    let mut font_dir = FONT_DIR.to_string();
    while let Some(arg) = args.next() {
        if arg == "--fonts" {
            font_dir = args.next().unwrap_or_else(|| { eprintln!("--fonts requires a directory"); process::exit(2) });
        } else { eprintln!("Unknown argument: {}", arg); process::exit(2); }
    }
    let mut fonts = scan_fonts(&font_dir);
    if fonts.is_empty() {
        println!("[bfontpicker] no fonts found in {}", font_dir);
        process::exit(1);
    }

    let list = build_list(&fonts);
    let path = std::env::var_os("BREENIX_CONFIG_ROOT").map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/".into()).join("etc/fonts.conf");
    let mut state = SaveState { path, save: None, error: None };
    if let Err(e) = libbreenix::atomic_file::remove_stale(&state.path) { state.error = Some(e.to_string()); }
    let config = match std::fs::read_to_string(&state.path) {
        Ok(contents) => match FontConfig::parse(&contents) {
            Ok(c) => c,
            Err(e) => { state.error = Some(format!("Invalid font configuration: {:?}", e)); FontConfig::default() }
        },
        Err(e) => { state.error = Some(e.to_string()); FontConfig::default() }
    };
    println!("[bfontpicker] config: mono_path='{}' mono_size={} display_path='{}' display_size={}",
             config.mono_path, config.mono_size, config.display_path, config.display_size);
    let mut current_mono = config.mono_path;
    let mut current_display = config.display_path;
    let mut mono_size = config.mono_size.max(MIN_FONT_SIZE);
    let mut display_size = config.display_size.max(MIN_FONT_SIZE);

    // Start with current mono font selected
    let mut selected = first_font_item(&list);
    for (i, item) in list.iter().enumerate() {
        if let ListItem::Font(idx) = item {
            if fonts[*idx].path == current_mono {
                selected = i;
                break;
            }
        }
    }

    let mut win = match Window::new(b"Font Picker", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("[bfontpicker] Window::new failed: {}", e);
            process::exit(1);
        }
    };

    let list_bottom = WIN_H as usize - PREVIEW_HEIGHT - 24;
    let visible = (list_bottom - LIST_TOP) / ITEM_H;
    let mut scroll: usize = 0;
    let mut status_msg = state.error.clone().unwrap_or_default();
    let mut status_timer: u32 = 0;
    let mut last_click_ms: u64 = 0;
    let mut last_click_idx: usize = usize::MAX;

    if selected >= visible {
        scroll = selected - visible + 1;
    }

    match win.surface() {
        Ok(fb) => render(
            fb, &mut fonts, &list, selected,
            &current_mono, &current_display, scroll, &status_msg,
            &[mono_size, display_size],
        ),
        Err(e) => {
            println!("[bfontpicker] ERROR: no window surface: {}", e);
            process::exit(1);
        }
    }
    publish_tree(&mut win, &fonts, &list, selected, &current_mono, &current_display, [mono_size, display_size], &state);
    match win.present() {
        Ok(()) => println!("[bfontpicker] first frame presented: {}x{}", win.width(), win.height()),
        Err(e) => println!("[bfontpicker] first frame present failed: {}", e),
    }

    loop {
        let event_count = win.dispatch(|delivered| {
            match delivered.event.clone() {
                Event::KeyPress { ascii, keycode, .. } => {
                    match keycode {
                        KEY_UP => {
                            selected = prev_font_item(&list, selected);
                            if selected < scroll {
                                scroll = selected;
                            }
                        }
                        KEY_DOWN => {
                            selected = next_font_item(&list, selected);
                            if selected >= scroll + visible {
                                scroll = selected - visible + 1;
                            }
                        }
                        KEY_ENTER => {
                            if let Some(msg) = apply_font(
                                &fonts, &list, selected,
                                &mut current_mono, &mut current_display,
                                &mut state,
                            ) {
                                status_msg = msg;
                                status_timer = 40;
                            }
                        }
                        KEY_ESCAPE => {
                            process::exit(0);
                        }
                        _ => {
                            if ascii == b'\r' || ascii == b'\n' {
                                if let Some(msg) = apply_font(
                                    &fonts, &list, selected,
                                    &mut current_mono, &mut current_display,
                                    &mut state,
                                ) {
                                    status_msg = msg;
                                    status_timer = 40;
                                }
                            } else if ascii == b'+' || ascii == b'=' {
                                if let Some(msg) = adjust_size(
                                    &fonts, &list, selected, SIZE_STEP,
                                    &mut mono_size, &mut display_size, &mut state,
                                ) {
                                    status_msg = msg;
                                    status_timer = 40;
                                }
                            } else if ascii == b'-' {
                                if let Some(msg) = adjust_size(
                                    &fonts, &list, selected, -SIZE_STEP,
                                    &mut mono_size, &mut display_size, &mut state,
                                ) {
                                    status_msg = msg;
                                    status_timer = 40;
                                }
                            } else if ascii == b'q' || ascii == b'Q' {
                                process::exit(0);
                            }
                        }
                    }
                }
                Event::MouseButton { button: breengel::buttons::LEFT, pressed: true, x: _, y } => {
                    let ly = y as usize;
                    if ly >= LIST_TOP && ly < list_bottom {
                        let clicked_list_idx = scroll + (ly - LIST_TOP) / ITEM_H;
                        if clicked_list_idx < list.len() {
                            if matches!(list[clicked_list_idx], ListItem::Font(_)) {
                                let now_ms = time::now_monotonic()
                                    .map(|ts| {
                                        ts.tv_sec as u64 * 1000
                                            + ts.tv_nsec as u64 / 1_000_000
                                    })
                                    .unwrap_or(0);
                                if clicked_list_idx == last_click_idx
                                    && now_ms.saturating_sub(last_click_ms) < 400
                                {
                                    selected = clicked_list_idx;
                                    if let Some(msg) = apply_font(
                                        &fonts, &list, selected,
                                        &mut current_mono, &mut current_display,
                                        &mut state,
                                    ) {
                                        status_msg = msg;
                                        status_timer = 40;
                                    }
                                    last_click_idx = usize::MAX;
                                } else {
                                    selected = clicked_list_idx;
                                    last_click_ms = now_ms;
                                    last_click_idx = clicked_list_idx;
                                }
                            }
                        }
                    }
                }
                Event::Resized { .. } => {}
                Event::CloseRequested => {
                    process::exit(0);
                }
                _ => {}
            }
            Handled::Redraw
        }).unwrap_or_else(|e| { eprintln!("[bfontpicker] dispatch: {}", e); process::exit(1) });
        let mut needs_redraw = event_count != 0;

        if status_timer > 0 {
            status_timer -= 1;
            if status_timer == 0 {
                status_msg = state.error.clone().unwrap_or_default();
                needs_redraw = true;
            }
        }

        if needs_redraw {
            if let Ok(fb) = win.surface() {
                render(
                    fb, &mut fonts, &list, selected,
                    &current_mono, &current_display, scroll, &status_msg,
                    &[mono_size, display_size],
                );
                publish_tree(&mut win, &fonts, &list, selected, &current_mono, &current_display, [mono_size, display_size], &state);
                let _ = win.present();
            }
        }

        let _ = time::sleep_ms(50);
    }
}
