//! Breenix Window Manager (bwm) — Pure Compositor + Input Router
//!
//! Discovers Breengel client windows registered on the window interface (version 2,
//! `docs/gui/breengel.md` section 17), composes them with its own desktop on the GPU core
//! (`compositor`, ruling B13), and routes keyboard and pointer input to the focused window
//! as window events (section 19).
//!
//! BWM does not emulate terminals; init starts the session's programs, and bwm starts only
//! the ones its hotkeys name.

#[path = "bwm/compositor.rs"]
mod compositor;
#[path = "bwm/placement.rs"]
mod placement;
#[path = "bwm/session.rs"]
mod session;
#[path = "bwm/input_backlog.rs"]
mod input_backlog;

use std::process;
use std::io::Write;

use compositor::{CheckError, Compositor, FrameError, Layer, Pointer};
use session::{cursor_shape, Input};

#[cfg(not(target_arch = "aarch64"))]
use libbreenix::fs;
use libbreenix::io;
use libbreenix::signal;
use libbreenix::types::Fd;

#[cfg(not(target_arch = "aarch64"))]
use breengel::FontConfig;
use libgfx::ttf_font::CachedFont;
#[cfg(not(target_arch = "aarch64"))]
use libgfx::ttf_font::Font;
use libgfx::bitmap_font;
use libgfx::ttf_font;
use libgfx::color::Color;
use libgfx::surface::{Rect as Clip, Surface};

#[path = "bwm/config.rs"]
mod config;
use config::*;
#[path = "bwm/route.rs"]
mod route;
use route::*;
#[path = "bwm/geometry.rs"]
mod geometry;
use geometry::*;
#[path = "bwm/desk.rs"]
mod desk;
use desk::*;
#[path = "bwm/chrome.rs"]
mod chrome;
use chrome::*;
#[path = "bwm/launch.rs"]
mod launch;
use launch::*;

fn main() {
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
    match session::claim() {
        Ok(epoch) => print!("[bwm] compositor claim: epoch {}\n", epoch),
        Err(e) => {
            print!("[bwm] ERROR: GPU compositing required: the compositor claim failed: {}\n", e);
            process::exit(1);
        }
    }
    let scene_fd = match session::scene_fd() {
        Ok(fd) => fd,
        Err(e) => {
            print!("[bwm] ERROR: GPU compositing required: the scene descriptor failed: {}\n", e);
            process::exit(1);
        }
    };

    // The GPU core composes the desktop (`compositor`); there is no CPU-only fallback.
    let mut gpu = match Compositor::open(screen_w as u32, screen_h as u32, px(APPBAR_HEIGHT) as u32, ui().scale as u32) {
        Ok(c) => c,
        Err(e) => {
            print!("[bwm] ERROR: GPU compositing required: {}\n", e);
            process::exit(1);
        }
    };
    print!("[bwm] GPU compositing mode (GPU core), display: {}x{}\n", screen_w, screen_h);

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
    let mut input_parser = InputParser::new();
    let (mut mouse_x, mut mouse_y) = session::mouse_pos()
        .map(|(x, y)| (x as i32, y as i32))
        .unwrap_or((0, 0));
    let mut active_cursor_shape = cursor_shape::ARROW;
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
    // desktop target and the pointer's bitmap (`compositor`).
    compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
    let first_pointer = Pointer { x: mouse_x, y: mouse_y, shape: active_cursor_shape };
    let first = gpu
        .compose(&[], Some(first_pointer), &[])
        .map_err(|e| CheckError::Failed(e.to_string()))
        .and_then(|()| gpu.check(0, &[], Some(first_pointer)));
    match first {
        Ok(pixels) => print!("[bwm] first frame read back: {} pixels exact\n", pixels),
        Err(CheckError::Moved(e)) | Err(CheckError::Failed(e)) => print!("[bwm] first frame check FAILED: {}\n", e),
    }
    match check_desktop_fills(&mut gpu, screen_w, screen_h, mode_index) {
        Ok(line) => print!("[bwm] desktop fills the scanout: {}\n", line),
        Err(e) => print!("[bwm] desktop fill check FAILED: {}\n", e),
    }
    let mut frame_failed = false;
    // Another process holds the display: bwm composes again when it has it back.
    let mut display_lost = false;
    // The pointer the last composed frame drew, so a move repaints where it was.
    let mut drawn_pointer: Option<Pointer> = None;

    let mut read_buf = [0u8; 512];
    let mut poll_fds = [
        io::PollFd { fd: 0, events: io::poll_events::POLLIN as i16, revents: 0 },
        io::PollFd { fd: scene_fd, events: io::poll_events::POLLIN as i16, revents: 0 },
    ];

    // Initial window discovery (before entering event loop)
    if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order, &mut win_defaults) {
        focused_win = next_visible_window(&windows, 0);
        compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
        full_redraw = true;
    }

    loop {
        flush_input();
        // ── 0. Sleep until input or a scene change ──
        // The scene descriptor is readable when a window registered, committed, moved or
        // went away. The pointer has no descriptor, so the sleep is bounded and the
        // pointer is read after it. Local work already prepared (the initial discovery's
        // redraw) is presented before sleeping.
        let timeout = if display_lost {
            DISPLAY_WAIT_MS
        } else if full_redraw || content_dirty || windows_dirty {
            0
        } else if input_parser.state == InputState::Escape {
            (ESCAPE_TIMEOUT_MS as i32).min(POINTER_POLL_MS)
        } else {
            POINTER_POLL_MS
        };
        for fd in poll_fds.iter_mut() {
            fd.revents = 0;
        }
        let _ = io::poll(&mut poll_fds, timeout);
        let stdin_ready = poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0;
        let scene_changed = poll_fds[1].revents & io::poll_events::POLLIN as i16 != 0;
        if scene_changed {
            session::drain_scene(scene_fd);
        }

        // ── 0b. Poll modifier state and check hotkeys ──
        // Drain the kernel's latched Super press-edge count each frame (syscall 413's
        // key state) so a tap that completed between polls is fed into double-tap
        // detection.
        let keys = session::key_state().unwrap_or_default();
        let super_taps = keys.super_taps;
        let current_mods = keys.modifiers as u8;
        hotkey_mgr.cooldown = hotkey_mgr.cooldown.saturating_sub(1);
        // The chords recognized this frame, in the order their keys arrived.
        let mut hotkey_actions = Vec::new();

        // ── 1. Discover new/removed client windows, and see client frames ──
        if scene_changed {
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
                compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
                full_redraw = true;
            }
            // A committed client frame is composed without repainting the desktop.
            windows_dirty |= gpu.pending();
        }

        // ── 2. Process keyboard input ──
        let now_ms = monotonic_ms().expect("bwm input requires the monotonic clock");
        let mut input_events: Vec<InputEvent> = input_parser.expire_escape(now_ms).into_iter().collect();
        if stdin_ready {
            if let Ok(n) = io::read(Fd::from_raw(0), &mut read_buf) {
                for i in 0..n {
                    if let Some(event) = input_parser.feed(read_buf[i], now_ms) {
                        input_events.push(event);
                    }
                }
            }
        }
        for event in input_events {
            match event {
                InputEvent::FunctionKey(k) if (k as usize) <= windows.len() => {
                    let new_focus = (k - 1) as usize;
                    if new_focus < windows.len() && new_focus != focused_win {
                        send_focus_event(&windows, focused_win, false);
                        focused_win = new_focus;
                        send_focus_event(&windows, focused_win, true);
                        compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
                        full_redraw = true;
                    }
                }
                InputEvent::Key { ascii, keycode, modifiers, repeat } => {
                    match hotkey_mgr.key_pressed(modifiers as u8, current_mods, ascii, repeat) {
                        Chord::Fire(action) => {
                            hotkey_actions.push(action);
                            continue;
                        }
                        Chord::Consumed => continue,
                        Chord::None => {}
                    }
                    if !windows.is_empty() {
                        send_to(&windows, focused_win, Input::Key { ascii, keycode, modifiers, repeat: repeat == Some(true) });
                    }
                }
                _ => {}
            }
        }

        hotkey_actions.extend(hotkey_mgr.update(current_mods, super_taps as u32));
        for action in &hotkey_actions {
            match action {
                HotkeyAction::FocusNext => {
                    if !windows.is_empty() {
                        send_focus_event(&windows, focused_win, false);
                        focused_win = next_visible_window(&windows, (focused_win + 1) % windows.len());
                        send_focus_event(&windows, focused_win, true);
                        compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
                        full_redraw = true;
                    }
                }
                HotkeyAction::FocusPrev => {
                    if !windows.is_empty() {
                        send_focus_event(&windows, focused_win, false);
                        focused_win = if focused_win == 0 { windows.len() - 1 } else { focused_win - 1 };
                        focused_win = next_visible_window(&windows, focused_win);
                        send_focus_event(&windows, focused_win, true);
                        compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
                        full_redraw = true;
                    }
                }
                _ => execute_hotkey_action(action, &mut windows, focused_win),
            }
        }

        // Dirty rect tracking — initialized before mouse processing so drag
        // can expand the dirty region.
        let mut dirty_x0 = i32::MAX;
        let mut dirty_y0 = i32::MAX;
        let mut dirty_x1 = 0i32;
        let mut dirty_y1 = 0i32;
        let mut cursor_dirty = false;

        // ── 3. Process mouse input ──
        let mut mouse_moved_this_frame = false;
        if let Ok((mx, my, buttons, scroll_delta)) = session::mouse_state_with_scroll() {
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
                        windows[win_idx].resize_right = None;
                        windows[win_idx].resize_bottom = None;
                        windows[win_idx].x = new_x;
                        windows[win_idx].y = new_y;
                        // Publish the window's new place for composition
                        if windows[win_idx].window_id != 0 {
                            let cx = windows[win_idx].content_x();
                            let cy = windows[win_idx].content_y();
                            let _ = session::set_geometry(windows[win_idx].window_id, cx, cy, win_idx as u32, true);
                        }
                        // Dirty region = union of old and new bounds
                        let (nx0, ny0, nx1, ny1) = windows[win_idx].bounds();
                        let dr_x0 = ox0.min(nx0).max(0) as usize;
                        let dr_y0 = oy0.min(ny0).max(0) as usize;
                        let dr_x1 = ox1.max(nx1) as usize;
                        let dr_y1 = oy1.max(ny1) as usize;
                        compose_partial_redraw(&mut gpu, &windows, focused_win, &clock_text, dr_x0, dr_y0, dr_x1, dr_y1, &mut ui_font);
                        dirty_x0 = dirty_x0.min(dr_x0 as i32);
                        dirty_y0 = dirty_y0.min(dr_y0 as i32);
                        dirty_x1 = dirty_x1.max(dr_x1 as i32);
                        dirty_y1 = dirty_y1.max(dr_y1 as i32);
                        content_dirty = true;
                    }
                } else if let Some((win_idx, edge, anchor_x, anchor_y, _, orig_y, orig_w, orig_h)) = resizing {
                    let dx = mouse_x - anchor_x;
                    let dy = mouse_y - anchor_y;

                    let mut new_w = orig_w as i32;
                    let mut new_h = orig_h as i32;
                    match edge {
                        ResizeEdge::Right => { new_w += dx; }
                        ResizeEdge::Bottom => { new_h += dy; }
                        ResizeEdge::Left => { new_w -= dx; }
                        ResizeEdge::Top => { new_h -= dy; }
                        ResizeEdge::TopLeft => { new_w -= dx; new_h -= dy; }
                        ResizeEdge::TopRight => { new_w += dx; new_h -= dy; }
                        ResizeEdge::BottomLeft => { new_w -= dx; new_h += dy; }
                        ResizeEdge::BottomRight => { new_w += dx; new_h += dy; }
                    }
                    new_w = new_w.max(px(MIN_WIN_WIDTH) as i32);
                    new_h = new_h.max(px(MIN_WIN_HEIGHT) as i32);
                    if windows[win_idx].resize_bottom.is_some() {
                        new_h = new_h.min(orig_y + orig_h as i32 - px(TASKBAR_HEIGHT) as i32);
                    }
                    // B41: neither position nor dimensions change until LIST reports
                    // the owner's answer. LIST preserves the opposite edge atomically.
                    let width = new_w.saturating_sub((px(BORDER_WIDTH) * 2) as i32).clamp(1, 4096) as u32;
                    let height = new_h.saturating_sub((px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2) as i32).clamp(1, 4096) as u32;
                    send_to(&windows, win_idx, Input::Resize { width, height });
                } else if !windows.is_empty() && focused_win < windows.len()
                    && !windows[focused_win].minimized
                    && windows[focused_win].hit_content(mouse_x, mouse_y)
                {
                    let local_x = mouse_x - windows[focused_win].content_x();
                    let local_y = mouse_y - windows[focused_win].content_y();
                    send_to(&windows, focused_win, Input::Move { x: local_x, y: local_y });
                }

                // Update cursor shape based on resize edge hover
                if dragging.is_none() && resizing.is_none() {
                    let mut hover_shape = cursor_shape::ARROW;
                    for i in (0..windows.len()).rev() {
                        if windows[i].minimized { continue; }
                        if let Some(edge) = windows[i].hit_resize_edge(mouse_x, mouse_y) {
                            hover_shape = match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => cursor_shape::RESIZE_NS,
                                ResizeEdge::Left | ResizeEdge::Right => cursor_shape::RESIZE_EW,
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => cursor_shape::RESIZE_NWSE,
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => cursor_shape::RESIZE_NESW,
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
                    cursor_dirty |= active_cursor_shape != cursor_shape::ARROW;
                    active_cursor_shape = cursor_shape::ARROW;
                    let new_cw = windows[win_idx].content_width();
                    let new_ch = windows[win_idx].content_height();
                    let old_cw = orig_w.saturating_sub(px(BORDER_WIDTH) * 2);
                    let old_ch = orig_h.saturating_sub(px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2);
                    print!("[bwm] resize end: id={} old={}x{} new={}x{} frame={}x{}\n",
                        windows[win_idx].window_id, old_cw, old_ch, new_cw, new_ch,
                        windows[win_idx].width, windows[win_idx].height);
                    save_window_defaults(&windows[win_idx], &mut win_defaults);
                } else if dragging.is_some() {
                    let drag_idx = dragging.unwrap().0;
                    dragging = None;
                    save_window_defaults(&windows[drag_idx], &mut win_defaults);
                } else if !windows.is_empty() && focused_win < windows.len()
                    && !windows[focused_win].minimized
                    && windows[focused_win].hit_content(mouse_x, mouse_y)
                {
                    let local_x = mouse_x - windows[focused_win].content_x();
                    let local_y = mouse_y - windows[focused_win].content_y();
                    send_to(&windows, focused_win, Input::Button { button: 1, pressed: false, x: local_x, y: local_y });
                }
            }

            // Scroll wheel: forward to focused window content area
            if scroll_delta != 0 && !windows.is_empty() && focused_win < windows.len()
                && !windows[focused_win].minimized
                && windows[focused_win].window_id != 0
            {
                let local_x = mouse_x - windows[focused_win].content_x();
                let local_y = mouse_y - windows[focused_win].content_y();
                send_to(&windows, focused_win, Input::Scroll { x: local_x, y: local_y, delta_y: scroll_delta });
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
                            send_focus_event(&windows, focused_win, false);
                            focused_win = next_visible_window(&windows, focused_win);
                            send_focus_event(&windows, focused_win, true);
                        } else {
                            // Click unfocused/minimized → restore and focus
                            windows[idx].minimized = false;
                            // Bring to top of z-order
                            if idx < windows.len() - 1 {
                                let win = windows.remove(idx);
                                windows.push(win);
                            }
                            let top = windows.len() - 1;
                            if top != focused_win {
                                send_focus_event(&windows, focused_win, false);
                                focused_win = top;
                                send_focus_event(&windows, focused_win, true);
                            } else {
                                focused_win = top;
                            }
                        }
                        update_kernel_z_order(&windows);
                        compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
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
                            send_focus_event(&windows, focused_win, false);
                            focused_win = top;
                            send_focus_event(&windows, focused_win, true);
                        } else {
                            focused_win = top;
                        }

                        if let Some(edge) = clicked_resize {
                            // Start resize — set cursor shape for the edge
                            let shape = match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => cursor_shape::RESIZE_NS,
                                ResizeEdge::Left | ResizeEdge::Right => cursor_shape::RESIZE_EW,
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => cursor_shape::RESIZE_NWSE,
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => cursor_shape::RESIZE_NESW,
                            };
                            cursor_dirty |= active_cursor_shape != shape;
                            active_cursor_shape = shape;
                            windows[top].resize_right = matches!(edge,
                                ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft)
                                .then_some(windows[top].x + windows[top].width as i32);
                            windows[top].resize_bottom = matches!(edge,
                                ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight)
                                .then_some(windows[top].y + windows[top].height as i32);
                            resizing = Some((
                                top, edge,
                                mouse_x, mouse_y,
                                windows[top].x, windows[top].y,
                                windows[top].width, windows[top].height,
                            ));
                        } else if clicked_title {
                            // Check chrome buttons before starting drag
                            if windows[top].hit_close_button(mouse_x, mouse_y) {
                                send_to(&windows, top, Input::Close);
                                if windows[top].owner_pid > 0 {
                                    let _ = signal::kill(windows[top].owner_pid as i32, signal::SIGTERM);
                                }
                            } else if windows[top].hit_minimize_button(mouse_x, mouse_y) {
                                windows[top].minimized = true;
                                if focused_win == top {
                                    send_focus_event(&windows, focused_win, false);
                                    focused_win = next_visible_window(&windows, focused_win);
                                    send_focus_event(&windows, focused_win, true);
                                }
                                update_kernel_z_order(&windows);
                                compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
                                full_redraw = true;
                            } else {
                                dragging = Some((top, mouse_x - windows[top].x, mouse_y - windows[top].y));
                            }
                        } else if windows[top].hit_content(mouse_x, mouse_y) {
                            let local_x = mouse_x - windows[top].content_x();
                            let local_y = mouse_y - windows[top].content_y();
                            send_to(&windows, focused_win, Input::Button { button: 1, pressed: true, x: local_x, y: local_y });
                        }

                        // Full redraw for z-order or focus change
                        if !full_redraw && (z_changed || focus_changed) {
                            compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
                            full_redraw = true;
                        }
                    }
                }
            }
        }

        // ── 4. Update clock (once per second) and the frame-rate line ──
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
        if let Ok(ts) = libbreenix::time::now_realtime() {
            if ts.tv_sec != last_clock_sec {
                last_clock_sec = ts.tv_sec;
                format_clock(ts.tv_sec, &mut clock_text);
                draw_taskbar(&mut gpu.desktop, &clock_text, &mut ui_font);
                dirty_x0 = 0;
                dirty_y0 = 0;
                dirty_x1 = dirty_x1.max(screen_w as i32);
                dirty_y1 = dirty_y1.max(px(TASKBAR_HEIGHT) as i32);
                content_dirty = true;
            }
        }

        // Keep complete pixel validation off the interactive drag path. A settled
        // resize still gets the same whole-scanout comparison on the next redraw
        // (the clock also redraws each second if no client commits after release).
        let check_ready = resizing.is_none() && windows.iter().any(|w|
            !w.frame_checked && monotonic_ms().unwrap_or(0) >= w.check_after_ms);
        // ── 5. Composite on the GPU (only when something changed) ──
        if full_redraw || content_dirty || windows_dirty || mouse_moved_this_frame || cursor_dirty {
            let pointer = Pointer { x: mouse_x, y: mouse_y, shape: active_cursor_shape };
            let mut damage = Vec::new();
            if full_redraw {
                damage.push([0, 0, screen_w as i32, screen_h as i32]);
            } else if content_dirty {
                damage.push([dirty_x0.max(0), dirty_y0.max(0), dirty_x1.min(screen_w as i32), dirty_y1.min(screen_h as i32)]);
            }
            // The pointer where the last frame drew it and where this one draws it.
            if drawn_pointer.is_none_or(|p| (p.x, p.y, p.shape) != (pointer.x, pointer.y, pointer.shape)) {
                damage.extend(drawn_pointer.map(|p| compositor::pointer_footprint(p, ui().scale as i32)));
                damage.push(compositor::pointer_footprint(pointer, ui().scale as i32));
            }
            let layers = frame_layers(&windows);
            let frame = gpu.compose(&layers, Some(pointer), &damage);
            full_redraw = false;
            content_dirty = false;
            windows_dirty = false;
            match frame {
                Ok(()) => {
                    frame_failed = false;
                    if display_lost {
                        display_lost = false;
                        print!("[bwm] display back: composing again\n");
                    }
                    drawn_pointer = Some(pointer);
                    fps_frames_since_last = fps_frames_since_last.wrapping_add(1);
                    // Each client window's first shown frame is read back and compared.
                    // Check the whole scanout for each newly shown or resized client,
                    // including other clients that keep animating beside it.
                    // A client that committed
                    // again after the frame sampled it is checked on a later frame. Any
                    // failure prints the line docs/boot-path.json lists as fatal.
                    let unchecked: Vec<(u64, [u32; 4])> = if check_ready {
                        gpu.shown_clients(&layers, Some(pointer))
                            .into_iter()
                            .filter(|(id, _)| windows.iter().any(|win| win.window_id == *id && !win.frame_checked
                                && monotonic_ms().unwrap_or(0) >= win.check_after_ms))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    // One check a frame: the unchecked window tried the fewest times.
                    let next = unchecked.iter()
                        .min_by_key(|(id, _)| windows.iter().find(|w| w.window_id == *id).map_or(0, |w| w.check_moved))
                        .copied();
                    if let Some((id, _)) = next {
                        let result = gpu.check(id, &layers, Some(pointer));
                        let retry = match (&result, windows.iter_mut().find(|win| win.window_id == id)) {
                            (Err(CheckError::Moved(_)), Some(win)) => {
                                win.check_moved += 1;
                                win.check_moved < CHECK_MOVED_LIMIT
                            }
                            _ => false,
                        };
                        if !retry {
                            if let Some(win) = windows.iter_mut().find(|win| win.window_id == id) {
                                win.frame_checked = true;
                            }
                            report_check(id, &result);
                        }
                    }
                }
                Err(FrameError::NotOwner) => {
                    // Another process took the display (probe, say): wait for it to give the
                    // display back, or exit, and then repaint everything.
                    if !display_lost {
                        print!("[bwm] display held by another process: waiting for it\n");
                    }
                    display_lost = true;
                    drawn_pointer = None;
                    full_redraw = true;
                }
                Err(FrameError::Failed(e)) => {
                    // Start again with a fresh context and targets; the next frame
                    // repaints everything.
                    if !frame_failed {
                        print!("[bwm] GPU frame failed: {}\n", e);
                    }
                    frame_failed = true;
                    if let Err(e) = gpu.reopen() {
                        print!("[bwm] ERROR: GPU compositor could not be reopened: {}\n", e);
                        process::exit(1);
                    }
                    paint_background(&mut gpu.background);
                    compose_full_redraw(&mut gpu, &windows, focused_win, &clock_text, &mut ui_font);
                    drawn_pointer = None;
                    full_redraw = true;
                }
            }
        }
    }
}
