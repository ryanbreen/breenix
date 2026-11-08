//! Breenix Window Manager (bwm) — Pure Compositor + Input Router
//!
//! Discovers Breengel client windows registered on the window interface (version 2,
//! `docs/gui/breengel.md` section 17), composes them with its own desktop on the GPU core
//! (`compositor`, ruling B13), and routes keyboard input by focus and pointer input by content hit test
//! as window events (section 19).
//!
//! BWM does not emulate terminals; init starts the session's programs, and bwm starts only
//! the ones its hotkeys name.

#[path = "bwm/compositor.rs"]
mod compositor;
#[path = "bwm/startup_prepare.rs"]
mod startup_prepare;
#[path = "bwm/placement.rs"]
mod placement;
#[path = "bwm/session.rs"]
mod session;
#[path = "bwm/input_backlog.rs"]
mod input_backlog;
#[path = "bwm/input.rs"]
mod input;
#[path = "bwm/timing.rs"]
mod timing;
#[path = "bwm/pace.rs"]
mod pace;
#[path = "bwm/input_keys.rs"]
mod input_keys;
#[path = "bwm/pointer_route.rs"]
mod pointer_route;
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
#[path = "bwm/gesture.rs"]
mod gesture;
#[path = "bwm/geometry.rs"]
mod geometry;
use geometry::*;
#[path = "bwm/adopt.rs"]
mod adopt;
#[path = "bwm/desk.rs"]
mod desk;
use desk::*;
#[path = "bwm/stack.rs"]
mod stack;
use stack::Stack;
#[path = "bwm/chrome.rs"]
mod chrome;
use chrome::*;
#[path = "bwm/children.rs"]
mod children;
#[path = "bwm/launch.rs"]
mod launch;
use launch::*;

#[path = "bwm/clipboard.rs"]
mod clipboard;
#[path = "bwm/clipboard_server.rs"]
mod clipboard_server;
#[path = "bwm/clipboard_socket.rs"]
mod clipboard_socket;

#[path = "bwm/clock.rs"]
mod clock;
#[path = "bwm/clock_wake.rs"]
mod clock_wake;
#[path = "bwm/close.rs"]
mod close;
#[path = "bwm/wake.rs"]
mod wake;
#[path = "bwm/shutdown.rs"]
mod shutdown;
#[path = "bwm/signal_wake.rs"]
mod signal_wake;

fn main() {
    libuitest::startup::mark("compositor-exec");
    let paths = settings::Paths::environment();
    let loaded = settings::Settings::load(&paths.bwm).unwrap_or_else(|e| { eprintln!("[bwm] config: {e}"); process::exit(1); });
    let _ = SETTINGS.set(loaded);
    libuitest::startup::mark("config-ready");
    print!("[bwm] settings font.size={} clock.visible={}\n", settings().font_size, settings().clock_visible);
    let signal_wake = initialize_shutdown();
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

    libuitest::startup::mark("scene-ready");
    // Clock state
    let mut clock = clock::Clock::new(std::env::var_os("BREENIX_UI_CLOCK").is_some());
    let mut clock_wake = clock_wake::open().expect("UI clock wake setup failed");
    let mut clock_text = [0u8; 11];
    format_clock(clock.wall_sec(), &mut clock_text);
    // The GPU core composes the desktop (`compositor`); there is no CPU-only fallback.
    // Font parsing and cold taskbar atlas coverage do not touch these targets.
    // Prepare the same clock snapshot that draw_taskbar will use.
    let (opened, mut ui_font) = std::thread::scope(|scope| {
        let font = scope.spawn(|| {
            libuitest::startup::mark("startup-font-begin");
            let config = FontConfig::load();
            print!("[bwm] fonts {config:?}\n");
            let mut font = std::fs::read(&config.display_path).ok()
                .and_then(|data| Font::parse(&data).ok())
                .map(|f| {
                    let mut font = CachedFont::new(f, 256);
                    font.observe_phases(libuitest::startup::mark);
                    font
                });
            if font.is_some() {
                print!("[bwm] display font loaded: {}\n", config.display_path);
            } else {
                print!("[bwm] no display font, using bitmap fallback\n");
            }
            if let Some(font) = font.as_mut() {
                if let Err(error) = font.prepare_subpixel(&[b"Breenix", &clock_text], ttf_font_size() * ui().scale as f32) {
                    eprintln!("[bwm] taskbar coverage warm-up unavailable: {error}");
                }
            }
            libuitest::startup::mark("fonts-ready");
            font
        });
        let opened = Compositor::open(screen_w as u32, screen_h as u32, px(APPBAR_HEIGHT) as u32, ui().scale as u32);
        libuitest::startup::mark("targets-ready");
        (opened, font.join().expect("startup font worker panicked"))
    });
    let mut gpu = match opened {
        Ok(c) => c,
        Err(e) => {
            print!("[bwm] ERROR: GPU compositing required: {}\n", e);
            process::exit(1);
        }
    };
    print!("[bwm] GPU compositing mode (GPU core), display: {}x{}\n", screen_w, screen_h);

    // Paint the decorative background once; each desktop repaint copies it.
    paint_background(&mut gpu.background);
    libuitest::startup::mark("background-ready");

    let mut windows: Vec<Window> = Vec::new();
    let mut stack = Stack::new();
    let mut stream = input::Stream::new();
    let mut timing = timing::Timing::new();
    let mut children = children::Children::default();
    let mut closing = close::Close::for_process(libbreenix::process::getpid().map_or(0, |pid| pid.raw() as u32));
    let mut clicks = timing::Clicks::new();
    let mut consumed_keys = std::collections::BTreeSet::new();
    let (mut mouse_x, mut mouse_y) = (0, 0);
    let mut active_cursor_shape = cursor_shape::ARROW;
    let mut pointer_routes = pointer_route::Router::new();
    let mut chrome_device = None;
    let mut dragging: Option<gesture::Drag> = None;
    let mut resizing: Option<gesture::Resize> = None;
    let mut content_dirty = false;
    // Raw startup PRESENT has no window receipt. Schedule the initial window
    // composite even when there is no client, clock tick or later scene change.
    let mut windows_dirty = true;
    let mut pacing = pace::Pacer::default();
    let mut win_defaults: [WindowDefaults; MAX_DEFAULTS] = core::array::from_fn(|_| WindowDefaults {
        title: [0; 32], title_len: 0, x: 0, y: 0, width: 0, height: 0, valid: false,
    });

    let mut taskbar_prepare_ns = None;
    let mut clock_pace = pace::Maintenance::default();
    let mut clock_dirty = false;
    let mut next_creation_order: u32 = 0;
    // Resolve hotkeys through the same root as bwm.conf.
    let mut hotkey_mgr = HotkeyManager::new();
    hotkey_mgr.load_config(paths.hotkeys.to_str().expect("UTF-8 hotkey path"));

    // Initial composite: the desktop alone (background, taskbar and app bar), before any
    // client window is discovered. It is read back from the scanout and compared with the
    // desktop target and the pointer's bitmap (`compositor`).
    compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
    libuitest::startup::mark("chrome-ready");
    let strict_startup = std::env::var_os("BREENIX_INPUT_DEVICES").is_some()
        || std::env::var_os("BREENIX_CONFIG_ROOT").is_some();
    let mut reported_startup_error = false;
    let mut fills = startup_prepare::recover(
        check_desktop_fills(&mut gpu, screen_w, screen_h, mode_index), strict_startup, |error| {
            if !reported_startup_error {
                eprintln!("[bwm] initial presentation unavailable: {error}; recovering");
                reported_startup_error = true;
            }
            startup_retry_wait();
            if let Err(error) = gpu.reopen() {
                eprintln!("[bwm] ERROR: GPU compositor could not be reopened: {error}");
                process::exit(1);
            }
            paint_background(&mut gpu.background);
            compose_full_redraw(&mut gpu, &[], &stack, &clock_text, &mut ui_font);
            check_desktop_fills(&mut gpu, screen_w, screen_h, mode_index)
        }).unwrap_or_else(|error| {
            eprintln!("[bwm] initial presentation FAILED: {error}");
            process::exit(1);
        });
    if let Err(error) = &fills {
        eprintln!("[bwm] desktop fill check FAILED: {error}");
        if strict_startup { process::exit(1); }
    }
    libuitest::startup::mark("fills-ready");
    // Discover metadata while the bare desktop remains unchanged. Coverage
    // preparation writes only the font atlas, independently of the pixel check.
    let initial_windows = discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order, &mut win_defaults);
    if initial_windows { reconcile_stack(&windows, &mut stack, true); }
    libuitest::startup::mark("initial-windows-ready");
    let first_pointer = Pointer { x: mouse_x, y: mouse_y, shape: active_cursor_shape };
    let (prepared, presented) = startup_prepare::during(|| {
        libuitest::startup::mark("title-plan-begin");
        let result = if let Some(font) = ui_font.as_mut() {
            let titles: Vec<&[u8]> = ordered_windows(&windows, &stack)
                .filter(|win| !stack.minimized(win.window_id) && !win.chromeless)
                .map(|win| &win.title[..win.title_len]).collect();
            font.prepare_subpixel(&titles, ttf_font_size() * ui().scale as f32)
                .map_err(|error| format!("initial title coverage: {error:?}"))
        } else { Ok(()) };
        libuitest::startup::mark("title-plan-return");
        result
    }, || {
        gpu.present_startup_desktop(Some(first_pointer))
            .map(|()| {
                libuitest::startup::mark("pointer-presented");
                measure_check(|| gpu.check(0, &[], Some(first_pointer)))
            })
    });
    if let Err(error) = prepared {
        eprintln!("[bwm] title coverage warm-up unavailable: {error}");
    }
    // Use the normal fresh-context recovery before composing clients, then
    // perform the same whole-frame and fill checks on the recovered desktop.
    let first = startup_prepare::recover(presented, strict_startup, |error| {
        if !reported_startup_error {
            eprintln!("[bwm] initial presentation unavailable: {error}; recovering");
            reported_startup_error = true;
        }
        startup_retry_wait();
        if let Err(error) = gpu.reopen() {
            eprintln!("[bwm] ERROR: GPU compositor could not be reopened: {error}");
            process::exit(1);
        }
        paint_background(&mut gpu.background);
        compose_full_redraw(&mut gpu, &[], &stack, &clock_text, &mut ui_font);
        fills = match check_desktop_fills(&mut gpu, screen_w, screen_h, mode_index) {
            Ok(checked) => checked,
            Err(error) => return Err(error),
        };
        if let Err(error) = &fills {
            eprintln!("[bwm] desktop fill check FAILED: {error}");
            if strict_startup { process::exit(1); }
        }
        gpu.present_startup_desktop(Some(first_pointer)).map(|()| {
            measure_check(|| gpu.check(0, &[], Some(first_pointer)))
        })
    }).unwrap_or_else(|error| {
        eprintln!("[bwm] initial presentation FAILED: {error}");
        process::exit(1);
    });
    libuitest::startup::mark("checks-ready");
    match first {
        Ok(pixels) => print!("[bwm] first frame read back: {} pixels exact\n", pixels),
        Err(CheckError::Moved(e)) | Err(CheckError::Failed(e)) => {
            print!("[bwm] first frame check FAILED: {}\n", e);
            if strict_startup { process::exit(1); }
        },
    }
    match fills {
        Ok(line) => print!("[bwm] desktop fills the scanout: {}\n", line),
        Err(e) => {
            print!("[bwm] desktop fill check FAILED: {}\n", e);
            if strict_startup { process::exit(1); }
        },
    }
    libuitest::startup::mark("desktop-ready");
    let mut frame_failed = false;
    // Another process holds the display: bwm composes again when it has it back.
    let mut display_lost = false;
    // The pointer the last composed frame drew, so a move repaints where it was.
    let mut drawn_pointer = Some(first_pointer);
    let mut full_redraw = false;
    let mut validation: ValidationQueue<PresentedScene> = ValidationQueue::new();
    let mut working_scene = PresentedScene { layers: Vec::new(), pointer: first_pointer };

    // The desktop outlives a clipboard that cannot open; clipboard clients then see ENOENT.
    let mut clipboard = clipboard_socket::Server::open().map_err(|e| eprintln!("[bwm] clipboard socket: {e}")).ok();
    let mut poll_fds = vec![
        io::PollFd { fd: stream.fd(), events: io::poll_events::POLLIN as i16, revents: 0 },
        io::PollFd { fd: scene_fd, events: io::poll_events::POLLIN as i16, revents: 0 },
    ];

    // The complete bare-desktop comparison and title preparation have both
    // finished before a client can receive its first COMPOSITE receipt.
    if initial_windows {
        compose_initial_windows(&mut gpu, &windows, &stack, &mut ui_font);
        full_redraw = true;
    }
    libuitest::startup::mark("client-chrome-ready");

    loop {
        flush_input();
        // ── 0. Sleep until input or a scene change ──
        clock.begin_epoch();
        let now = libbreenix::ui_clock::now_ns();
        let monotonic_ns = pacing_ns();
        let wall = clock.acknowledgment(claim_epoch).next_deadline_ns;
        let shown = validation.scene().map(|scene| gpu.shown_clients(&scene.layers, Some(scene.pointer)))
            .unwrap_or_default();
        refresh_pacing(&mut pacing);
        let hold = match pacing.decision(pacing_ns()) {
            pace::Decision::WaitUntil(deadline) => Some(deadline),
            pace::Decision::Compose => None,
        };
        let timeout = wake::timeout(
            (full_redraw || content_dirty || windows_dirty) && hold.is_none(), display_lost, now, monotonic_ns,
            wake::Deadlines {
                presentation: hold,
                closing: closing.deadline(),
                clipboard: clipboard.as_ref().and_then(|c| c.deadline()),
                repeat: timing.deadline(),
                launch: children.deadline(),
                check_after_ms: if resizing.is_none() {
                    windows.iter().filter(|w| validation.pending(w.window_id)
                        && shown.iter().any(|(id, _)| *id == w.window_id))
                        .map(|w| w.check_after_ms).min()
                } else { None },
                wall: (wall != 0).then_some(wall),
            });
        let timeout = closing.timeout_ms(now, timeout);
        let hold = if display_lost { None } else { hold };
        let clock_hold = hold.into_iter().chain(clock_pace.deadline()).min();
        let timeout = wake::wait_ns(timeout, pacing_ns(), clock_hold);
        poll_fds.truncate(2);
        if let Some(wake) = &clock_wake {
            poll_fds.push(io::PollFd { fd: wake.fd(), events: io::poll_events::POLLIN as i16, revents: 0 });
        }
        poll_fds.push(io::PollFd { fd: signal_wake.fd(), events: io::poll_events::POLLIN as i16, revents: 0 });
        if let Some(clipboard) = &clipboard { clipboard.append_poll(&mut poll_fds); }
        for fd in poll_fds.iter_mut() {
            fd.revents = 0;
        }
        let mut wait = ShutdownWait { fds: &mut poll_fds, timeout, failed: false };
        if shutdown::boundary(&mut wait).is_none() { return; }
        signal_wake.drain();
        if let Some(clipboard) = &mut clipboard { clipboard.service(&poll_fds, libbreenix::ui_clock::now_ns()); }
        if let Some(wake) = &clock_wake {
            if poll_fds[2].revents != 0 {
                if let Err(error) = wake.drain() {
                    eprintln!("[bwm] UI clock wake stopped: {error}");
                    clock_wake = None;
                }
            }
        }
        clock.begin_epoch();
        launch::reap(&mut children);
        launch::poll_errors(&mut children);
        let input_ready = poll_fds[0].revents & io::poll_events::POLLIN as i16 != 0;
        let scene_changed = poll_fds[1].revents & io::poll_events::POLLIN as i16 != 0;
        if scene_changed {
            session::drain_scene(scene_fd);
        }

        hotkey_mgr.cooldown = hotkey_mgr.cooldown.saturating_sub(1);

        // ── 1. Discover new/removed client windows, and see client frames ──
        if scene_changed {
            if discover_windows(&mut windows, screen_w, screen_h, &mut next_creation_order, &mut win_defaults) {
                reconcile_stack(&windows, &mut stack, false);
                if dragging.as_ref().and_then(|d| d.id()).is_some_and(|id| !windows.iter().any(|w| w.window_id == id))
                    || resizing.as_ref().is_some_and(|r| !windows.iter().any(|w| w.window_id == r.id)) {
                    dragging = None;
                    resizing = None;
                    chrome_device = None;
                }
                compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                full_redraw = true;
            }
            // A committed client frame is composed without repainting the desktop.
            windows_dirty |= gpu.pending();
        }

        closing.retain(|id, owner| windows.iter().any(|w| w.window_id == id && w.owner_pid == owner));
        let close_now = libbreenix::ui_clock::now_ns();
        if closing.has_due(close_now) {
            let mut listed = Vec::new();
            if session::list_windows(&mut listed).is_ok() {
                closing.retain(|id, owner| listed.iter().any(|w| w.id == id && w.owner_pid == owner));
                closing.due(close_now, |pid, _| {
                    let _ = signal::kill(pid as i32, signal::SIGKILL);
                });
            }
        }

        send_pointer_notices(pointer_routes.reconcile(&stack, &pointer_contents(&windows)));

        // Dirty rect tracking is shared by the records drained in this epoch.
        let mut dirty_x0 = i32::MAX;
        let mut dirty_y0 = i32::MAX;
        let mut dirty_x1 = 0i32;
        let mut dirty_y1 = 0i32;
        let mut cursor_dirty = false;
        let mut mouse_moved_this_frame = false;

        let records = if input_ready { stream.read() } else { Vec::new() };
        let positions = stream.positions(&records, screen_w, screen_h);
        for ((event, payload), position) in records.into_iter().zip(positions) {
            input::begin(event);
            if event.kind == kind::SYN && event.code == libbreenix::input::syn::DROPPED {
                timing.key(event, stack.focus(), false, libbreenix::ui_clock::now_ns());
            }
            if event.kind == kind::KEY && event.value == 2
                && !timing.key(event, stack.focus(), false, libbreenix::ui_clock::now_ns()) {
                input::finish();
                continue;
            }
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
                timing.key(event, stack.focus(), consumed, libbreenix::ui_clock::now_ns());
                if !consumed {
                    input::send_keys(&windows, &stack, event, &payload);
                }
            } else if event.kind == kind::TEXT {
                input::send_keys(&windows, &stack, event, &payload);
            }
            hotkey_actions.extend(hotkey_mgr.update(current_mods, super_taps, event.time_ns));
            for action in &hotkey_actions {
                match action {
                    HotkeyAction::FocusNext | HotkeyAction::FocusPrev => {
                        if let Some(id) = stack.cycle(matches!(action, HotkeyAction::FocusPrev)) {
                            let transition = stack.raise(id);
                            publish_transition(&windows, &stack, transition);
                            compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                            full_redraw = true;
                        }
                    }
                    _ => execute_hotkey_action(action, &windows, &stack, &mut children),
                }
            }

            // Content policy and chrome share one normalized pointer snapshot.
            if let Some((new_mx, new_my)) = stream.pointer(event, position) {
                let mouse_moved = new_mx != mouse_x || new_my != mouse_y;
                mouse_moved_this_frame |= mouse_moved;

                use pointer_route::Action;
                use libbreenix::input::{axis, syn};
                let action = match (event.kind, event.code) {
                    (kind::KEY, _) => Action::Button { code: event.code, value: event.value },
                    (kind::REL, axis::WHEEL) => Action::Wheel(event.value),
                    (kind::SYN, syn::DROPPED) => Action::Cancel,
                    _ => pointer_route::motion_action((mouse_x, mouse_y), (new_mx, new_my)),
                };
                let chrome_active = chrome_device == Some(event.device)
                    && (dragging.is_some() || resizing.is_some());
                let enabled = !chrome_active && pointer_content_enabled(
                    &windows, &stack, (new_mx, new_my), screen_h,
                );
                let was_grabbed = pointer_routes.grabbed(event.device);
                let routed = pointer_routes.route(&mut stack, &pointer_contents(&windows),
                    event.device, (new_mx, new_my), enabled, action);
                if let Some(transition) = routed.focus {
                    publish_transition(&windows, &stack, transition);
                    compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                    full_redraw = true;
                }
                let new_click = routed.press_edge && event.code == key::BUTTON_LEFT
                    && !pointer_routes.grabbed(event.device);
                let release_chrome = routed.release_edge && event.code == key::BUTTON_LEFT
                    && chrome_device == Some(event.device);
                send_timed_pointer_notices(routed.notices, &mut clicks, event, (new_mx, new_my));
                if mouse_moved {
                    mouse_x = new_mx;
                    mouse_y = new_my;

                    if let Some((win_idx, (x, y))) = dragging.as_mut().filter(|_| chrome_active)
                        .and_then(|d| d.motion(&windows.iter().map(|w| w.window_id).collect::<Vec<_>>(), (mouse_x, mouse_y))) {
                        let new_x = x.clamp(px(32) as i32 - windows[win_idx].width as i32, screen_w as i32 - px(32) as i32);
                        let new_y = y.max(px(TASKBAR_HEIGHT) as i32);
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
                    } else if let Some(resize) = resizing.as_ref().filter(|_| chrome_active) {
                        if let Some(win_idx) = windows.iter().position(|w| w.window_id == resize.id) {
                            let candidate = resize.size((mouse_x, mouse_y), ui().scale as u32, screen_w, screen_h);
                            let current = (windows[win_idx].content_width() as u32, windows[win_idx].content_height() as u32);
                            if let Some((width, height)) = windows[win_idx].resize_queue.motion(current, candidate) {
                                send_to(&windows, win_idx, Input::Resize { width, height });
                            }
                        } else {
                            resizing = None;
                            chrome_device = None;
                        }
                    }


                }

                // Release: end drag or route release event.
                // Per-endpoint button tracking in the kernel prevents dual USB HID
                // endpoints from racing (one endpoint can't cancel the other's press).
                if release_chrome {
                    chrome_device = None;
                    if let Some(resize) = resizing.take() {
                        cursor_dirty |= active_cursor_shape != cursor_shape::ARROW;
                        active_cursor_shape = cursor_shape::ARROW;
                        if let Some(win) = windows.iter().find(|w| w.window_id == resize.id) {
                            save_window_defaults(win, &mut win_defaults);
                        }
                    } else if let Some(drag) = dragging.take() {
                        if let Some(win) = windows.iter().find(|w| Some(w.window_id) == drag.id()) {
                            save_window_defaults(win, &mut win_defaults);
                        }
                    }
                }
                if matches!(action, Action::Cancel) && chrome_device == Some(event.device) {
                    chrome_device = None;
                    dragging = None;
                    resizing = None;
                }

                // A content grab owns further presses from its device, including over chrome.
                if new_click && !was_grabbed {
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
                                windows[top].resize_queue.begin();
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
                                chrome_device = Some(event.device);
                                resizing = Some(gesture::Resize::new(id, edge, (mouse_x, mouse_y),
                                    (windows[top].x, windows[top].y, windows[top].width, windows[top].height)));
                            } else if clicked_title {
                                // Check chrome buttons before starting drag
                                if windows[top].hit_close_button(mouse_x, mouse_y) {
                                    match closing.request(id, windows[top].owner_pid, libbreenix::ui_clock::now_ns()) {
                                        close::Action::Request => send_to(&windows, top, Input::Close),
                                        close::Action::Term(pid) => { let _ = signal::kill(pid as i32, signal::SIGTERM); }
                                        _ => {}
                                    }
                                } else if windows[top].hit_minimize_button(mouse_x, mouse_y) {
                                    let transition = stack.minimize(windows[top].window_id);
                                    publish_transition(&windows, &stack, transition);
                                    compose_full_redraw(&mut gpu, &windows, &stack, &clock_text, &mut ui_font);
                                    full_redraw = true;
                                } else {
                                    chrome_device = Some(event.device);
                                    dragging = Some(gesture::Drag::new(id, (mouse_x, mouse_y), (windows[top].x, windows[top].y)));
                                }
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

            send_pointer_notices(pointer_routes.reconcile(&stack, &pointer_contents(&windows)));
            input::finish();
            timing.focus(stack.focus());
        }

        // Device retirement may follow a release evicted by reader pressure.
        // Run after the batch so queued clicks keep their source-time ordering.
        if input_ready {
            timing::retire_unregistered(&mut clicks, &mut consumed_keys, |device| stream.registered(device));
        }

        for repeat in timing.due(libbreenix::ui_clock::now_ns(), stack.focus()) {
            input::send_repeat(repeat);
        }

        // f48a2564: scene geometry can change under a stationary pointer.
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

        // ── 4. Update chrome on real UI-clock seconds ──
        if clock.due() && clock_pace.ready(pacing_ns(),
            full_redraw || content_dirty || windows_dirty || mouse_moved_this_frame || cursor_dirty, &pacing) {
            let sec = clock.update_clock().expect("due wall-clock update");
            format_clock(sec, &mut clock_text);
            let began = pacing_ns();
            draw_taskbar(&mut gpu.desktop, &clock_text, &mut ui_font);
            let draw_ns = pacing_ns().saturating_sub(began);
            taskbar_prepare_ns = Some(draw_ns);
            clock_pace.reset();
            clock_dirty = true;
        }

        validation.retain_clients(|id| windows.iter().any(|w| w.window_id == id && !w.frame_checked));
        for win in windows.iter().filter(|w| !w.frame_checked) {
            validation.first_frame(win.window_id);
        }

        // ── 5. Composite on the GPU (only when something changed) ──
        let urgent_frame = full_redraw || content_dirty || windows_dirty || mouse_moved_this_frame || cursor_dirty;
        if urgent_frame || clock_dirty {
            refresh_pacing(&mut pacing);
            let clock_ready = !clock_dirty || clock_pace.ready(pacing_ns(), urgent_frame, &pacing);
            if clock_ready && matches!(pacing.decision(pacing_ns()), pace::Decision::WaitUntil(_)) {
                // Carry cursor/window dirt through descriptor wakes while the client handles input.
                windows_dirty = true;
                clock_pace.reset();
                if content_dirty { full_redraw = true; }
            } else if clock_ready {
                let pointer = Pointer { x: mouse_x, y: mouse_y, shape: active_cursor_shape };
                let mut damage = Vec::new();
                if full_redraw {
                    damage.push([0, 0, screen_w as i32, screen_h as i32]);
                } else if content_dirty {
                    damage.push([dirty_x0.max(0), dirty_y0.max(0), dirty_x1.min(screen_w as i32), dirty_y1.min(screen_h as i32)]);
                }
                if clock_dirty {
                    damage.push([0, 0, screen_w as i32, px(TASKBAR_HEIGHT) as i32]);
                }
                // The pointer where the last frame drew it and where this one draws it.
                if drawn_pointer.is_none_or(|p| (p.x, p.y, p.shape) != (pointer.x, pointer.y, pointer.shape)) {
                    damage.extend(drawn_pointer.map(|p| compositor::pointer_footprint(p, ui().scale as i32)));
                    damage.push(compositor::pointer_footprint(pointer, ui().scale as i32));
                }
                frame_layers(&windows, &stack, &mut working_scene.layers);
                working_scene.pointer = pointer;
                let frame = gpu.compose(&working_scene.layers, Some(pointer), &damage);
                full_redraw = false;
                content_dirty = false;
                windows_dirty = false;
                clock_dirty = false;
                clock_pace.reset();
                match frame {
                    Ok(()) => {
                        if let Some(draw_ns) = taskbar_prepare_ns.take() {
                            let prepare_ns = gpu.last_timing.prepare_ns;
                            let period_ns = gpu.last_timing.period_ns;
                            if period_ns != 0 && draw_ns.saturating_add(prepare_ns) >= period_ns {
                                use std::io::Write;
                                if let Ok(mut evidence) = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence") {
                                    let _ = writeln!(evidence, "VALUE bwm-taskbar-overrun draw_ns={} prepare_ns={} period_ns={} serial={}", draw_ns, prepare_ns, period_ns, gpu.last_serial);
                                }
                            }
                        }
                        pacing.presented(gpu.last_timing);
                        frame_failed = false;
                        if display_lost {
                            display_lost = false;
                            print!("[bwm] display back: composing again\n");
                        }
                        drawn_pointer = Some(pointer);
                        launch::acknowledge(&mut children, gpu.last_serial);
                        clock.presented(gpu.last_serial);
                        working_scene = validation.presented(working_scene);
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
        }
        if !full_redraw && !content_dirty && !windows_dirty && !clock_dirty && !display_lost && !frame_failed {
            let mut ack = clock.acknowledgment(claim_epoch);
            if let Some(deadline) = timing.deadline() {
                if ack.next_deadline_ns == 0 || deadline < ack.next_deadline_ns { ack.next_deadline_ns = deadline; }
            }
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


fn pacing_ns() -> u64 {
    libbreenix::time::now_monotonic().map_or(0, |time| time.as_nanos() as u64)
}
fn refresh_pacing(pacing: &mut pace::Pacer) {
    for (id, serial) in input::take_delivered() { pacing.delivered(id,input::serial_key(serial)); }
    pacing.refresh(|id, wanted| match libbreenix::window::read_meta(id) {
        Ok((_, meta)) => Some(input::serial_key(meta.processed_serial)),
        Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOENT)) => Some(wanted),
        Err(_) => None,
    });
}

/// Publish focus notices around the kernel stack update.
fn publish_transition(windows: &[Window], stack: &Stack, transition: (Option<u64>, Option<u64>)) {
    stack::publish_focus(transition, |notice| {
        let (id, input) = match notice {
            stack::FocusNotice::Lost(id) => (id, Input::FocusLost),
            stack::FocusNotice::Gained(id) => (id, Input::FocusGained),
        };
        input::send_id(id, input);
    }, || update_kernel_z_order(windows, stack));
}

/// Reconcile membership without disturbing surviving ids' policy order.
fn reconcile_stack(windows: &[Window], stack: &mut Stack, initial: bool) -> bool {
    if initial {
        let adopted: Vec<_> = windows.iter().filter_map(|win| {
            win.published_stack.map(|(z, minimized, focused)|
                (win.window_id, z, minimized, focused, win.chromeless))
        }).collect();
        stack.adopt(&adopted);
    }
    let removed: Vec<_> = stack.order().iter().copied()
        .filter(|id| !windows.iter().any(|w| w.window_id == *id)).collect();
    for &id in &removed {
        let transition = stack.remove(id);
        publish_transition(windows, stack, transition);
    }
    for win in windows {
        if !stack.order().contains(&win.window_id) {
            let transition = stack.add(win.window_id, win.chromeless);
            if initial {
                stack::publish_focus(transition, |notice| {
                    let (id, input) = match notice {
                        stack::FocusNotice::Lost(id) => (id, Input::FocusLost),
                        stack::FocusNotice::Gained(id) => (id, Input::FocusGained),
                    };
                    input::send_id(id, input);
                }, || {});
            } else {
                publish_transition(windows, stack, transition);
            }
        }
    }
    // LIST can also change dimensions and therefore the content origin.
    update_kernel_z_order(windows, stack);
    !removed.is_empty()
}

fn pointer_contents(windows: &[Window]) -> Vec<pointer_route::Content> {
    windows.iter().map(|w| pointer_route::Content {
        id: w.window_id, x: w.content_x(), y: w.content_y(),
        width: w.content_width(), height: w.content_height(),
    }).collect()
}

fn pointer_content_enabled(windows: &[Window], stack: &Stack, (x, y): (i32, i32), screen_h: usize) -> bool {
    if y < px(TASKBAR_HEIGHT) as i32 || y >= screen_h.saturating_sub(px(APPBAR_HEIGHT)) as i32 {
        return false;
    }
    // Chrome and its resize margin occlude lower content as in the existing hit path.
    ordered_windows(windows, stack).rev().filter(|w| !stack.minimized(w.window_id))
        .find(|w| w.hit_any(x, y) || w.hit_resize_edge(x, y).is_some())
        .is_some_and(|w| w.hit_content(x, y) && w.hit_resize_edge(x, y).is_none())
}

fn send_pointer_notices(notices: Vec<pointer_route::Notice>) {
    use pointer_route::NoticeKind;
    for notice in notices {
        let (x, y) = (notice.x, notice.y);
        let input = match notice.kind {
            NoticeKind::Move => Input::Move { x, y },
            NoticeKind::Button { code, pressed } => Input::Button { button: (code - 0x110 + 1) as u32, pressed, x, y, clicks: 1 },
            NoticeKind::Wheel(delta_y) => Input::Scroll { x, y, delta_y },
            NoticeKind::Enter => Input::Enter { x, y },
            NoticeKind::Leave => Input::Leave { x, y },
            NoticeKind::Cancel => Input::Cancel { x, y },
        };
        input::send_id(notice.id, input);
    }
}

fn send_timed_pointer_notices(notices: Vec<pointer_route::Notice>, clicks: &mut timing::Clicks, event: libbreenix::input::Event, point: (i32, i32)) {
    for notice in notices {
        if let pointer_route::NoticeKind::Button { code, pressed } = notice.kind {
            let count = if pressed { clicks.press(event.device, code, notice.id, point, event.time_ns) } else { clicks.release(event.device, code) };
            input::send_id(notice.id, Input::Button { button: (code - 0x110 + 1) as u32, pressed, x: notice.x, y: notice.y, clicks: count });
        } else { send_pointer_notices(vec![notice]); }
    }
}

static TERMINATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn terminate(_: i32) {
    TERMINATE.store(true, std::sync::atomic::Ordering::Relaxed);
    signal_wake::notify();
}

fn term_mask(how: i32) {
    let mask = (1u64 << (signal::SIGTERM - 1)) | (1u64 << (signal::SIGCHLD - 1));
    if let Err(error) = signal::sigprocmask(how, Some(&mask), None) {
        eprintln!("[bwm] cannot set shutdown signal mask: {error}");
        process::exit(1);
    }
}

// Startup has not reached the poll loop that normally delivers blocked signals.
// Deliver a pending SIGTERM during the retry delay, before allocating targets.
fn startup_retry_wait() {
    term_mask(signal::SIG_UNBLOCK);
    let _ = libbreenix::time::sleep_ms(16);
    term_mask(signal::SIG_BLOCK);
    if TERMINATE.load(std::sync::atomic::Ordering::Relaxed) {
        process::exit(0);
    }
}

extern "C" fn child_exited(_: i32) { signal_wake::notify(); }

fn initialize_shutdown() -> signal_wake::SignalWake {
    term_mask(signal::SIG_BLOCK);
    let wake = signal_wake::SignalWake::open().unwrap_or_else(|error| {
        eprintln!("[bwm] cannot create signal wake pipe: {error}");
        process::exit(1);
    });
    if let Err(error) = signal::sigaction(signal::SIGCHLD, Some(&signal::Sigaction::new(child_exited)), None) {
        eprintln!("[bwm] cannot install child handler: {error}");
        process::exit(1);
    }
    if let Err(error) = signal::sigaction(signal::SIGTERM, Some(&signal::Sigaction::new(terminate)), None) {
        eprintln!("[bwm] cannot install shutdown handler: {error}");
        process::exit(1);
    }
    wake
}

struct ShutdownWait<'a> {
    fds: &'a mut [io::PollFd],
    timeout: Option<u64>,
    failed: bool,
}

impl shutdown::Runtime for ShutdownWait<'_> {
    type Ready = ();
    fn unblock(&mut self) { term_mask(signal::SIG_UNBLOCK); }
    fn wait(&mut self) { let _ = io::poll_ns(self.fds, self.timeout); }
    fn block(&mut self) { term_mask(signal::SIG_BLOCK); }
    fn terminated(&self) -> bool { TERMINATE.load(std::sync::atomic::Ordering::Relaxed) }
    fn release(&mut self) {
        if let Err(error) = libbreenix::window::release_compositor() {
            eprintln!("[bwm] cannot release compositor: {error}");
            self.failed = true;
        }
    }
    fn restore(&mut self) {
        if let Err(error) = libbreenix::graphics::give_back_display() {
            eprintln!("[bwm] cannot restore display: {error}");
            self.failed = true;
        }
    }
    fn exit(&mut self, status: i32) { process::exit(if self.failed { 1 } else { status }); }
}
