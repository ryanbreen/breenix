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
#[path = "bwm/input.rs"]
mod input;
#[path = "bwm/input_keys.rs"]
mod input_keys;
use input::{flush_input, send_to};
use libbreenix::input::{key, kind};

use std::process;

use compositor::{CheckError, Compositor, FrameError, Layer, Pointer};
use session::Input;
use libbreenix::window::abi::cursor_shape;

use libbreenix::fs;
use libbreenix::io;
use libbreenix::signal;

use breengel::FontConfig;
use libgfx::ttf_font::CachedFont;
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
#[path = "bwm/stack.rs"]
mod stack;
use stack::Stack;
#[path = "bwm/chrome.rs"]
mod chrome;
use chrome::*;
#[path = "bwm/launch.rs"]
mod launch;
use launch::*;

#[path = "bwm/clock.rs"]
mod clock;

fn main() {
    print!("[bwm] Breenix Window Manager starting... (v2-chromeless-skip)\n");

    if let Err(e) = libbreenix::graphics::take_over_display() {
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
            print!("[bwm] ERROR: GPU compositing required: the compositor claim failed: {}\n", e);
            process::exit(1);
        }
    };
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

    let mut windows: Vec<Window> = Vec::new();
    let mut stack = Stack::new();
    let mut stream = input::Stream::new();
    let mut consumed_keys = std::collections::BTreeSet::new();
    let (mut mouse_x, mut mouse_y) = (0, 0);
    let mut active_cursor_shape = cursor_shape::ARROW;
    let mut prev_buttons: u32 = 0;
    let mut dragging: Option<(usize, i32, i32)> = None;
    // Active resize: (win_idx, edge, anchor_x, anchor_y, orig_x, orig_y, orig_w, orig_h)
    let mut resizing: Option<(usize, ResizeEdge, i32, i32, i32, i32, usize, usize)> = None;
    let mut content_dirty = false;
    // Raw startup PRESENT has no window receipt. Schedule the initial window
    // composite even when there is no client, clock tick or later scene change.
    let mut windows_dirty = true;
    let mut win_defaults: [WindowDefaults; MAX_DEFAULTS] = core::array::from_fn(|_| WindowDefaults {
        title: [0; 32], title_len: 0, x: 0, y: 0, width: 0, height: 0, valid: false,
    });

    // Clock state
    let mut clock = clock::Clock::new(std::env::var_os("BREENIX_UI_CLOCK").is_some());
    let mut clock_text = [0u8; 11];
    format_clock(clock.wall_sec(), &mut clock_text);
    let mut next_creation_order: u32 = 0;
    // Hotkey system: load config from /etc/hotkeys.conf
    let mut hotkey_mgr = HotkeyManager::new();
    hotkey_mgr.load_config("/etc/hotkeys.conf");

    // Initial composite: the desktop alone (background, taskbar and app bar), before any
    // client window is discovered. It is read back from the scanout and compared with the
    // desktop target and the pointer's bitmap (`compositor`).
    compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
    let fills = check_desktop_fills(&mut gpu, screen_w, screen_h, mode_index);
    let first_pointer = Pointer { x: mouse_x, y: mouse_y, shape: active_cursor_shape };
    let first = gpu
        .present_startup_desktop(Some(first_pointer))
        .map_err(|e| CheckError::Failed(e.to_string()))
        .and_then(|()| measure_check(|| gpu.check(0, &[], Some(first_pointer))));
    let first_presented = first.is_ok();
    match first {
        Ok(pixels) => print!("[bwm] first frame read back: {} pixels exact\n", pixels),
        Err(CheckError::Moved(e)) | Err(CheckError::Failed(e)) => print!("[bwm] first frame check FAILED: {}\n", e),
    }
    match fills {
        Ok(line) => print!("[bwm] desktop fills the scanout: {}\n", line),
        Err(e) => print!("[bwm] desktop fill check FAILED: {}\n", e),
    }
    let mut frame_failed = false;
    // Another process holds the display: bwm composes again when it has it back.
    let mut display_lost = false;
    // The pointer the last composed frame drew, so a move repaints where it was.
    let mut drawn_pointer = first_presented.then_some(first_pointer);
    let mut full_redraw = !first_presented;
    let mut validation = ValidationQueue::new();

    let mut poll_fds = [
        io::PollFd { fd: stream.fd(), events: io::poll_events::POLLIN as i16, revents: 0 },
        io::PollFd { fd: scene_fd, events: io::poll_events::POLLIN as i16, revents: 0 },
    ];

    // Initial window discovery (before entering event loop)
    if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order, &mut win_defaults) {
        reconcile_stack(&windows, &mut stack);
        compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
        full_redraw = true;
    }

    loop {
        flush_input();
        // ── 0. Sleep until input or a scene change ──
        // Input and scene changes both wake the loop. Keep the existing bounded
        // sleep until descriptor-only pacing lands in 21.7.
        let timeout = if display_lost {
            DISPLAY_WAIT_MS
        } else if full_redraw || content_dirty || windows_dirty {
            0
        } else {
            POINTER_POLL_MS
        };
        for fd in poll_fds.iter_mut() {
            fd.revents = 0;
        }
        let _ = io::poll(&mut poll_fds, timeout);
        clock.begin_epoch();
        let input_ready = poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0;
        let scene_changed = poll_fds[1].revents & io::poll_events::POLLIN as i16 != 0;
        if scene_changed {
            session::drain_scene(scene_fd);
        }

        hotkey_mgr.cooldown = hotkey_mgr.cooldown.saturating_sub(1);

        // ── 1. Discover new/removed client windows, and see client frames ──
        if scene_changed {
            if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order, &mut win_defaults) {
                if reconcile_stack(&windows, &mut stack) {
                    // Removal can shift the metadata indices held by existing gestures.
                    dragging = None;
                    resizing = None;
                }
                compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                full_redraw = true;
            }
            // A committed client frame is composed without repainting the desktop.
            windows_dirty |= gpu.pending();
        }

        // Dirty rect tracking is shared by the records drained in this epoch.
        let mut dirty_x0 = i32::MAX;
        let mut dirty_y0 = i32::MAX;
        let mut dirty_x1 = 0i32;
        let mut dirty_y1 = 0i32;
        let mut cursor_dirty = false;
        let mut mouse_moved_this_frame = false;

        let records = if input_ready { stream.read() } else { Vec::new() };
        for (event, payload) in records {
            input::begin(event);
            let current_mods = event.flags as u8;
            let super_taps = u32::from(event.kind == kind::KEY
                && matches!(event.code, key::LEFT_SUPER | key::RIGHT_SUPER)
                && event.value == 1 && stream.modifiers & modifier::SUPER == 0);
            stream.modifiers = current_mods;
            let mut hotkey_actions = Vec::new();
            if event.kind == kind::KEY && event.code < key::BUTTON_LEFT {
                let slot = (event.device, event.code);
                let released_chord = event.value == 0 && consumed_keys.remove(&slot);
                let mut consumed = released_chord;
                if event.value != 0 {
                    if (key::F1..key::F1 + 5).contains(&event.code) {
                        if let Some(&id) = stack.order().get((event.code - key::F1) as usize) {
                            let transition = stack.raise(id);
                            publish_transition(&windows, &stack, transition);
                            compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                            full_redraw = true;
                        }
                        consumed = true;
                    } else if input_keys::chord_key(event.code) != 0 {
                        match hotkey_mgr.key_pressed(current_mods, current_mods,
                            input_keys::chord_key(event.code), Some(event.value == 2)) {
                            Chord::Fire(action) => { hotkey_actions.push(action); consumed = true; }
                            Chord::Consumed => consumed = true,
                            Chord::None => {}
                        }
                    }
                    if consumed { consumed_keys.insert(slot); }
                }
                if !consumed {
                    input::send_keys(&windows, &stack, event, &payload);
                }
            } else if event.kind == kind::TEXT {
                input::send_keys(&windows, &stack, event, &payload);
            }
            hotkey_actions.extend(hotkey_mgr.update(current_mods, super_taps));
            for action in &hotkey_actions {
                match action {
                    HotkeyAction::FocusNext | HotkeyAction::FocusPrev => {
                        if let Some(id) = cycle_target(&stack, matches!(action, HotkeyAction::FocusPrev)) {
                            let transition = stack.raise(id);
                            publish_transition(&windows, &stack, transition);
                            compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                            full_redraw = true;
                        }
                    }
                    _ => execute_hotkey_action(action, &windows, &stack),
                }
            }

            // ── Pointer records use today's routing policy ──
            if let Some((new_mx, new_my, buttons, scroll_delta)) = stream.pointer(event, screen_w, screen_h) {
                let mouse_moved = new_mx != mouse_x || new_my != mouse_y;
                mouse_moved_this_frame |= mouse_moved;

                let focused = focus_index(&windows, &stack);
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
                            update_kernel_z_order(&windows, &stack);
                            // Dirty region = union of old and new bounds
                            let (nx0, ny0, nx1, ny1) = windows[win_idx].bounds();
                            let dr_x0 = ox0.min(nx0).max(0) as usize;
                            let dr_y0 = oy0.min(ny0).max(0) as usize;
                            let dr_x1 = ox1.max(nx1) as usize;
                            let dr_y1 = oy1.max(ny1) as usize;
                            compose_partial_redraw(&mut gpu, &windows, &stack, &clock_text, dr_x0, dr_y0, dr_x1, dr_y1, &mut ui_font);
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
                    } else if !windows.is_empty() && focused < windows.len()
                        && !stack.minimized(windows[focused].window_id)
                        && windows[focused].hit_content(mouse_x, mouse_y)
                    {
                        let local_x = mouse_x - windows[focused].content_x();
                        let local_y = mouse_y - windows[focused].content_y();
                        send_to(&windows, focused, Input::Move { x: local_x, y: local_y });
                    }

                    // Update cursor shape based on resize edge hover
                    if dragging.is_none() && resizing.is_none() {
                        let mut hover_shape = cursor_shape::ARROW;
                        for id in stack.order().iter().rev() {
                            let Some(i) = windows.iter().position(|w| w.window_id == *id) else { continue; };
                            if stack.minimized(*id) { continue; }
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
                    } else if !windows.is_empty() && focused < windows.len()
                        && !stack.minimized(windows[focused].window_id)
                        && windows[focused].hit_content(mouse_x, mouse_y)
                    {
                        let local_x = mouse_x - windows[focused].content_x();
                        let local_y = mouse_y - windows[focused].content_y();
                        send_to(&windows, focused, Input::Button { button: 1, pressed: false, x: local_x, y: local_y });
                    }
                }

                // Scroll wheel: forward to focused window content area
                if scroll_delta != 0 && !windows.is_empty() && focused < windows.len()
                    && !stack.minimized(windows[focused].window_id)
                    && windows[focused].window_id != 0
                {
                    let local_x = mouse_x - windows[focused].content_x();
                    let local_y = mouse_y - windows[focused].content_y();
                    send_to(&windows, focused, Input::Scroll { x: local_x, y: local_y, delta_y: scroll_delta });
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
                            let id = windows[idx].window_id;
                            let transition = if stack.focus() == Some(id) {
                                stack.minimize(id)
                            } else {
                                stack.restore(id)
                            };
                            publish_transition(&windows, &stack, transition);
                            compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                            full_redraw = true;
                        }
                    }
                    // Check windows (existing logic with chrome button additions)
                    else if !windows.is_empty() {
                        let mut clicked_idx: Option<usize> = None;
                        let mut clicked_title = false;
                        let mut clicked_resize: Option<ResizeEdge> = None;
                        for id in stack.order().iter().rev() {
                            let Some(i) = windows.iter().position(|w| w.window_id == *id) else { continue; };
                            if stack.minimized(*id) { continue; }
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
                            let id = windows[ci].window_id;
                            let old_order = stack.order().to_vec();
                            let transition = stack.raise(id);
                            let z_changed = stack.order() != old_order;
                            let focus_changed = transition.0 != transition.1;
                            publish_transition(&windows, &stack, transition);
                            let top = ci;

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
                                    let transition = stack.minimize(windows[top].window_id);
                                    publish_transition(&windows, &stack, transition);
                                    compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                                    full_redraw = true;
                                } else {
                                    dragging = Some((top, mouse_x - windows[top].x, mouse_y - windows[top].y));
                                }
                            } else if windows[top].hit_content(mouse_x, mouse_y) {
                                let local_x = mouse_x - windows[top].content_x();
                                let local_y = mouse_y - windows[top].content_y();
                                send_to(&windows, top, Input::Button { button: 1, pressed: true, x: local_x, y: local_y });
                            }

                            // Full redraw for z-order or focus change
                            if !full_redraw && (z_changed || focus_changed) {
                                compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                                full_redraw = true;
                            }
                        }
                    }
                }
            }

            input::finish();
        }

        // ── 4. Update chrome on real UI-clock seconds ──
        if let Some(sec) = clock.update_clock() {
            format_clock(sec, &mut clock_text);
            draw_taskbar(&mut gpu.desktop, &clock_text, &mut ui_font);
            dirty_x0 = 0;
            dirty_y0 = 0;
            dirty_x1 = dirty_x1.max(screen_w as i32);
            dirty_y1 = dirty_y1.max(px(TASKBAR_HEIGHT) as i32);
            content_dirty = true;
        }

        validation.retain_clients(|id| windows.iter().any(|w| w.window_id == id && !w.frame_checked));
        for win in windows.iter().filter(|w| !w.frame_checked) {
            validation.first_frame(win.window_id);
        }

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
            let layers = frame_layers(&windows, &stack);
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
                    clock.presented(gpu.last_serial);
                    validation.presented(PresentedScene { layers, pointer });
                }
                Err(FrameError::NotOwner) => {
                    validation.invalidate_scene();
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
                    validation.invalidate_scene();
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
                    compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                    drawn_pointer = None;
                    full_redraw = true;
                }
            }
        }
        if !full_redraw && !content_dirty && !windows_dirty && !display_lost && !frame_failed {
            let mut ack = clock.acknowledgment(claim_epoch);
            if let Err(error) = libbreenix::window::call(
                libbreenix::window::abi::op::COMPOSITOR_ACK, &mut ack,
            ) {
                eprintln!("[bwm] compositor acknowledgment failed: {}", error);
                process::exit(1);
            }
            run_deferred_check(&mut validation, &mut gpu, &mut windows, &mut poll_fds, resizing.is_some());
        }
    }
}


/// Send the current focus notices after a policy operation; u4 owns pre-raise ordering.
fn publish_transition(windows: &[Window], stack: &Stack, transition: (Option<u64>, Option<u64>)) {
    if transition.0 != transition.1 {
        for (id, gained) in [(transition.0, false), (transition.1, true)] {
            if let Some(index) = id.and_then(|id| windows.iter().position(|w| w.window_id == id)) {
                send_focus_event(windows, index, gained);
            }
        }
    }
    update_kernel_z_order(windows, stack);
}

/// Reconcile membership without disturbing surviving ids' policy order.
fn reconcile_stack(windows: &[Window], stack: &mut Stack) -> bool {
    let removed: Vec<_> = stack.order().iter().copied()
        .filter(|id| !windows.iter().any(|w| w.window_id == *id)).collect();
    for &id in &removed {
        let transition = stack.remove(id);
        publish_transition(windows, stack, transition);
    }
    for win in windows {
        if !stack.order().contains(&win.window_id) {
            let transition = stack.add(win.window_id, win.chromeless);
            publish_transition(windows, stack, transition);
        }
    }
    // LIST can also change dimensions and therefore the content origin.
    update_kernel_z_order(windows, stack);
    !removed.is_empty()
}

fn cycle_target(stack: &Stack, reverse: bool) -> Option<u64> {
    let visible: Vec<_> = stack.order().iter().copied().filter(|id| !stack.minimized(*id)).collect();
    if visible.is_empty() { return None; }
    let current = visible.iter().position(|id| Some(*id) == stack.focus());
    let index = match current {
        Some(i) if reverse => (i + visible.len() - 1) % visible.len(),
        Some(i) => (i + 1) % visible.len(),
        None if reverse => visible.len() - 1,
        None => 0,
    };
    Some(visible[index])
}
