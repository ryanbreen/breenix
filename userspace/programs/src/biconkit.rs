//! biconkit — Breenix Icon Kit showcase app.
//!
//! Displays all animated icons from the libicon crate in an interactive
//! two-section layout:
//!
//!   Top: grid of all icons at small (40px) size — hover and click each one.
//!   Bottom: large (80px) interactive preview of the selected icon, showing
//!           its current state name. Keyboard shortcuts cycle through icons.
//!
//! Controls:
//!   Mouse         Hover / click any icon (grid or preview)
//!   [,] or [<]    Select previous icon
//!   [.] or [>]    Select next icon
//!   Q             Quit

use std::process;

use breengel::{Window, Event};
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::shapes;

use libicon::icon::{Icon, IconMouse, IconState};
use libicon::icons::home::HomeIcon;
use libicon::icons::arrow::ArrowIcon;
use libicon::icons::search::SearchIcon;
use libicon::icons::save::SaveIcon;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const WIN_W: u32 = 600;
const WIN_H: u32 = 480;

const BG: Color = Color::rgb(30, 30, 40);
const SECTION_TITLE: Color = Color::rgb(180, 180, 200);
const SEPARATOR: Color = Color::rgb(60, 60, 80);
const LABEL_COLOR: Color = Color::rgb(140, 140, 160);
const SELECTED_BORDER: Color = Color::rgb(80, 140, 220);
const STATE_COLOR: Color = Color::rgb(120, 200, 120);
const HINT_COLOR: Color = Color::rgb(100, 100, 120);
const NAME_COLOR: Color = Color::WHITE;

// Grid section
const GRID_START_X: i32 = 60;
const GRID_START_Y: i32 = 55;
const GRID_SIZE: i32 = 40;
const GRID_SPACING_X: i32 = 120;
const GRID_SPACING_Y: i32 = 80;
const ICONS_PER_ROW: usize = 3;

// ---------------------------------------------------------------------------
// Monotonic clock helper
// ---------------------------------------------------------------------------

fn clock_ms() -> u64 {
    time::now_monotonic()
        .map(|ts| ts.tv_sec as u64 * 1_000 + ts.tv_nsec as u64 / 1_000_000)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Icon factory — one set for the grid, one for the large preview.
// We need two independent instances so hover state doesn't bleed between them.
// ---------------------------------------------------------------------------

fn make_icons() -> Vec<Box<dyn Icon>> {
    vec![
        Box::new(HomeIcon::new()),
        Box::new(ArrowIcon::new(false)),  // back
        Box::new(ArrowIcon::new(true)),   // forward
        Box::new(SearchIcon::new()),
        Box::new(SaveIcon::new()),
    ]
}

// ---------------------------------------------------------------------------
// Grid geometry helpers
// ---------------------------------------------------------------------------

/// Center point of icon i in the grid.
fn grid_center(i: usize) -> (i32, i32) {
    let col = i % ICONS_PER_ROW;
    let row = i / ICONS_PER_ROW;
    let cx = GRID_START_X + col as i32 * GRID_SPACING_X + GRID_SIZE / 2;
    let cy = GRID_START_Y + row as i32 * GRID_SPACING_Y + GRID_SIZE / 2;
    (cx, cy)
}

/// How many pixel rows the grid occupies (including label text below each icon).
fn grid_height(icon_count: usize) -> i32 {
    let rows = (icon_count + ICONS_PER_ROW - 1) / ICONS_PER_ROW;
    rows as i32 * GRID_SPACING_Y
}

// ---------------------------------------------------------------------------
// IconMouse constructor from global mouse state relative to a center point
// ---------------------------------------------------------------------------

fn icon_mouse_for(
    mx: i32, my: i32,
    mouse_down: bool, just_clicked: bool, just_released: bool,
    cx: i32, cy: i32, hover_radius: i32,
) -> IconMouse {
    let dx = mx - cx;
    let dy = my - cy;
    let dist_sq = dx * dx + dy * dy;
    let hr_sq = hover_radius * hover_radius;
    let hovering = dist_sq < hr_sq;

    IconMouse {
        hovering,
        pressed: hovering && mouse_down,
        just_clicked: hovering && just_clicked,
        just_released: hovering && just_released,
        rel_x: if hovering && hover_radius > 0 { dx as f32 / hover_radius as f32 } else { 0.0 },
        rel_y: if hovering && hover_radius > 0 { dy as f32 / hover_radius as f32 } else { 0.0 },
    }
}

// ---------------------------------------------------------------------------
// State name string (no alloc)
// ---------------------------------------------------------------------------

fn state_label(state: IconState) -> &'static [u8] {
    match state {
        IconState::Idle     => b"State: Idle",
        IconState::HoverIn  => b"State: Hover In",
        IconState::Hovering => b"State: Hovering",
        IconState::HoverOut => b"State: Hover Out",
        IconState::Pressed  => b"State: Pressed",
        IconState::Clicked  => b"State: Clicked",
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    println!("[biconkit] starting");

    let mut win = match Window::new(b"Breenix Icon Kit", WIN_W, WIN_H) {
        Ok(w) => w,
        Err(e) => {
            println!("[biconkit] Window::new failed: {}", e);
            process::exit(1);
        }
    };
    println!("[biconkit] window {} ({}x{})", win.id(), WIN_W, WIN_H);

    // Two independent icon sets so grid and preview animate independently.
    let mut grid_icons = make_icons();
    let mut preview_icons = make_icons();

    let icon_count = grid_icons.len();

    // Icon display names (must match make_icons() order).
    const NAMES: &[&[u8]] = &[b"Home", b"Back", b"Forward", b"Search", b"Save"];

    let mut selected_idx: usize = 0;
    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;
    let mut mouse_down = false;

    let mut last_ms: u64 = 0;

    loop {
        let now_ms = clock_ms();
        let dt_ms: u32 = if last_ms == 0 {
            16
        } else {
            (now_ms.saturating_sub(last_ms)).min(50) as u32
        };
        last_ms = now_ms;

        let prev_mouse_down = mouse_down;

        // ---------------------------------------------------------------
        // Event processing
        // ---------------------------------------------------------------
        for event in win.poll_events() {
            match event {
                Event::MouseMove { x, y } => {
                    mouse_x = x;
                    mouse_y = y;
                }
                Event::MouseButton { button, pressed, x, y } => {
                    if button == 1 {
                        mouse_down = pressed;
                        mouse_x = x;
                        mouse_y = y;
                    }
                }
                Event::KeyPress { ascii, .. } => {
                    match ascii {
                        b',' | b'<' => {
                            if selected_idx > 0 {
                                selected_idx -= 1;
                                preview_icons[selected_idx].reset();
                            }
                        }
                        b'.' | b'>' => {
                            if selected_idx + 1 < icon_count {
                                selected_idx += 1;
                                preview_icons[selected_idx].reset();
                            }
                        }
                        b'q' | b'Q' => process::exit(0),
                        _ => {}
                    }
                }
                Event::CloseRequested => process::exit(0),
                _ => {}
            }
        }

        let just_clicked  = mouse_down && !prev_mouse_down;
        let just_released = !mouse_down && prev_mouse_down;

        // ---------------------------------------------------------------
        // Update grid icons
        // ---------------------------------------------------------------
        for (i, icon) in grid_icons.iter_mut().enumerate() {
            let (cx, cy) = grid_center(i);
            let hover_radius = GRID_SIZE / 2 + 10;
            let im = icon_mouse_for(
                mouse_x, mouse_y,
                mouse_down, just_clicked, just_released,
                cx, cy, hover_radius,
            );

            // Clicking a grid icon selects it for the preview.
            if im.just_clicked {
                selected_idx = i;
                preview_icons[i].reset();
            }

            icon.update(dt_ms, im);
        }

        // ---------------------------------------------------------------
        // Update preview icon
        // ---------------------------------------------------------------
        let grid_h = grid_height(icon_count);
        let sep_y = GRID_START_Y + grid_h + 10;
        // Preview section: centered horizontally, vertically in the remaining space.
        let preview_section_top = sep_y + 30; // below separator + title
        let preview_cy = preview_section_top + 70;
        let preview_cx = WIN_W as i32 / 2;
        let preview_size: i32 = 80;
        let preview_hover_radius = preview_size / 2 + 20;

        {
            let im = icon_mouse_for(
                mouse_x, mouse_y,
                mouse_down, just_clicked, just_released,
                preview_cx, preview_cy, preview_hover_radius,
            );
            preview_icons[selected_idx].update(dt_ms, im);
        }

        // ---------------------------------------------------------------
        // Render
        // ---------------------------------------------------------------
        let fb = win.framebuf();
        let w = fb.width as i32;

        // Background
        shapes::fill_rect(fb, 0, 0, w, WIN_H as i32, BG);

        // --- Section title: ICON LIBRARY ---
        font::draw_text(fb, b"ICON LIBRARY", 20, 15, SECTION_TITLE, 2);
        shapes::fill_rect(fb, 20, 40, w - 40, 1, SEPARATOR);

        // --- Grid icons ---
        for (i, icon) in grid_icons.iter().enumerate() {
            let (cx, cy) = grid_center(i);

            // Highlight box around the selected icon.
            if i == selected_idx {
                shapes::draw_rect(
                    fb,
                    cx - GRID_SIZE / 2 - 5,
                    cy - GRID_SIZE / 2 - 5,
                    GRID_SIZE + 10,
                    GRID_SIZE + 10,
                    SELECTED_BORDER,
                );
            }

            icon.draw(fb, cx, cy, GRID_SIZE);

            // Label centered below the icon.
            let name = NAMES[i];
            let label_w = font::text_width(name, 1) as i32;
            let label_x = cx - label_w / 2;
            let label_y = cy + GRID_SIZE / 2 + 8;
            font::draw_text(fb, name, label_x as usize, label_y as usize, LABEL_COLOR, 1);
        }

        // --- Separator before preview ---
        shapes::fill_rect(fb, 20, sep_y, w - 40, 1, SEPARATOR);

        // --- Section title: INTERACTIVE PREVIEW ---
        font::draw_text(fb, b"INTERACTIVE PREVIEW", 20, (sep_y + 10) as usize, SECTION_TITLE, 2);

        // Icon name centered above the preview.
        let preview_name = NAMES[selected_idx];
        let name_w = font::text_width(preview_name, 2) as i32;
        let name_x = preview_cx - name_w / 2;
        let name_y = preview_cy - preview_size / 2 - 24;
        font::draw_text(fb, preview_name, name_x as usize, name_y as usize, NAME_COLOR, 2);

        // Large preview icon.
        preview_icons[selected_idx].draw(fb, preview_cx, preview_cy, preview_size);

        // State label below the preview icon.
        let state = preview_icons[selected_idx].state();
        let state_text = state_label(state);
        let state_w = font::text_width(state_text, 1) as i32;
        let state_x = preview_cx - state_w / 2;
        let state_y = preview_cy + preview_size / 2 + 14;
        font::draw_text(fb, state_text, state_x as usize, state_y as usize, STATE_COLOR, 1);

        // Navigation / quit hint at the bottom.
        let hint = b"[,] Prev    [.] Next    [Q] Quit";
        let hint_w = font::text_width(hint, 1) as i32;
        let hint_x = (w - hint_w) / 2;
        let hint_y = WIN_H as i32 - 18;
        font::draw_text(fb, hint, hint_x as usize, hint_y as usize, HINT_COLOR, 1);

        let _ = win.present();
        let _ = time::sleep_ms(16);
    }
}
