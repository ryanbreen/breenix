//! widgets, text-entry, icons and the `text/fontwatch-*` cases (contract sections 9 and
//! 14 to 16).

use crate::*;
use breengel::font::{FontChange, FontWatchError, FontWatcher};
use libbui::focus::FocusChain;
use libbui::widget::{file_picker::FilePickerResult, label::Label, text_field::TextField};
use libbui::{Key, KeyInput};

pub(crate) const FONTWATCH: &[Case] = &[
    pictured!("fontwatch-reload", "Changing the configured font makes later text use newly rasterized glyphs of the new font", text_fontwatch_reload),
    pictured!("fontwatch-size", "Changing the configured size changes the rasterized glyphs and their metrics", text_fontwatch_size),
    pictured!("fontwatch-missing", "A configured font file that is missing keeps the previous font and reports the error", text_fontwatch_missing),
    pictured!("fontwatch-malformed-config", "A malformed fonts.conf keeps the previous fonts and reports the error", text_fontwatch_malformed_config),
    pictured!("fontwatch-partial", "A fonts.conf replaced by rename is seen whole, and a truncated one is rejected", text_fontwatch_partial),
    pictured!("fontwatch-repeated", "Five changes in a row end with the last one in use", text_fontwatch_repeated),
    pictured!("fontwatch-independent", "Mono and display fonts change independently", text_fontwatch_independent),
    pictured!("fontwatch-idle", "A window that receives no events, with fonts.conf changed at UI time *t*, draws the new font in its first commit at or after the watcher's first 250 ms check after *t*", text_fontwatch_idle),
    pictured!("fontwatch-file-replaced", "Replacing the configured font file at the same path by rename, with fonts.conf unchanged, makes later text use newly rasterized glyphs of the new file by the next check", text_fontwatch_file_replaced),
];

pub(crate) const WIDGETS: &[Case] = &[
    pictured!(
        "button-draw",
        "A button draws its label and frame in the theme's literal colours and positions",
        widgets_button_draw,
    ),
    pictured!(
        "button-click",
        "Press and release inside a button report exactly one click",
        widgets_button_click,
    ),
    pictured!(
        "button-cancel",
        "Press inside and release outside report no click",
        widgets_button_cancel,
    ),
    pictured!(
        "button-disabled",
        "A disabled button draws dimmed and reports no click",
        widgets_button_disabled,
    ),
    pictured!(
        "checkbox-toggle",
        "Clicking a checkbox toggles it and redraws the mark",
        widgets_checkbox_toggle,
    ),
    pictured!(
        "checkbox-space",
        "Space toggles a focused checkbox",
        widgets_checkbox_space,
    ),
    pictured!(
        "slider-drag",
        "Dragging a slider sets the value the contract maps from the pointer x",
        widgets_slider_drag,
    ),
    pictured!(
        "slider-clamp",
        "Dragging past either end clamps to the minimum or maximum",
        widgets_slider_clamp,
    ),
    pictured!(
        "slider-keys",
        "Arrow keys step a focused slider by its step size",
        widgets_slider_keys,
    ),
    pictured!(
        "label-clip",
        "A label wider than its bounds is clipped at the bounds with an ellipsis",
        widgets_label_clip,
    ),
    pictured!(
        "tabbar-select",
        "Clicking a tab selects it and reports its index",
        widgets_tabbar_select,
    ),
    pictured!(
        "tabbar-overflow",
        "More tabs than fit scroll, and every tab stays reachable",
        widgets_tabbar_overflow,
    ),
    pictured!(
        "scrollbar-drag",
        "Dragging the thumb sets the offset proportionally",
        widgets_scrollbar_drag,
    ),
    pictured!(
        "scrollbar-wheel",
        "A wheel step moves the offset by the configured amount",
        widgets_scrollbar_wheel,
    ),
    pictured!(
        "scrollbar-thumb",
        "The thumb's length is proportional to visible over total",
        widgets_scrollbar_thumb,
    ),
    pictured!(
        "menubar-open",
        "Clicking a menu title opens its menu below it",
        widgets_menubar_open,
    ),
    pictured!(
        "menubar-shortcut",
        "match_shortcut returns the item bound to Ctrl+S",
        widgets_menubar_shortcut,
    ),
    pictured!(
        "menu-keys",
        "Arrows, Enter and Escape navigate, choose and close an open menu",
        widgets_menu_keys,
    ),
    pictured!(
        "menu-disabled",
        "A disabled item draws dimmed and cannot be chosen",
        widgets_menu_disabled,
    ),
    pictured!(
        "menu-dismiss",
        "A click outside an open menu closes it without choosing",
        widgets_menu_dismiss,
    ),
    pictured!(
        "layout-stack",
        "h_stack and v_stack place children at the contract's literal positions",
        widgets_layout_stack,
    ),
    pictured!(
        "focus-ring",
        "Each focusable widget draws the theme's focus ring when focused and none otherwise",
        widgets_focus_ring,
    ),
    pictured!(
        "no-change-ack",
        "An event that changes no widget state is reported as needing no redraw",
        widgets_no_change_ack,
    ),
    pictured!("scale-2", "Scale-2 widgets double logical geometry and rasterize TrueType labels at twice the size", widgets_scale_2),
];

pub(crate) const TEXT_ENTRY: &[Case] = &[
    pictured!(
        "insert",
        "Typed text is inserted at the cursor",
        text_entry_insert,
    ),
    pictured!(
        "delete",
        "Backspace and Delete remove the character before and after the cursor",
        text_entry_delete,
    ),
    pictured!(
        "move",
        "Left, Right, Home and End move the cursor as the contract says",
        text_entry_move,
    ),
    pictured!(
        "select",
        "Shift with an arrow selects, and typing replaces the selection",
        text_entry_select,
    ),
    pictured!(
        "utf8",
        "Multi-byte characters insert and delete as one character",
        text_entry_utf8,
    ),
    pictured!(
        "maxlen",
        "Input beyond the maximum length is refused",
        text_entry_maxlen,
    ),
    pictured!(
        "scroll-to-cursor",
        "Text wider than the field scrolls to keep the cursor visible",
        text_entry_scroll_to_cursor,
    ),
    pictured!(
        "selection-text",
        "The selected text is returned exactly for a copy",
        text_entry_selection_text,
    ),
    pictured!(
        "paste-insert",
        "Inserted paste text replaces the selection and moves the cursor past it",
        text_entry_paste_insert,
    ),
    pictured!(
        "focus-tab",
        "Tab moves focus through focusable widgets in order",
        text_entry_focus_tab,
    ),
    pictured!(
        "focus-shift-tab",
        "Shift+Tab moves focus in reverse",
        text_entry_focus_shift_tab,
    ),
    pictured!(
        "focus-skip-disabled",
        "Focus traversal skips disabled widgets",
        text_entry_focus_skip_disabled,
    ),
    pictured!(
        "focus-click",
        "Clicking a focusable widget focuses it",
        text_entry_focus_click,
    ),
    pictured!(
        "filepicker-list",
        "The file picker lists the given entries, directories first, in byte order",
        text_entry_filepicker_list,
    ),
    pictured!(
        "filepicker-keys",
        "Arrows move the selection and Enter opens it",
        text_entry_filepicker_keys,
    ),
    pictured!(
        "filepicker-path",
        "Typing a path into the picker's text field navigates to it",
        text_entry_filepicker_path,
    ),
    pictured!(
        "filepicker-mouse",
        "A click selects and a double click within the interval, by explicit timestamps, opens",
        text_entry_filepicker_mouse,
    ),
    pictured!(
        "filepicker-scroll",
        "Lists longer than the view scroll by keys and wheel",
        text_entry_filepicker_scroll,
    ),
    pictured!(
        "filepicker-errors",
        "A directory that cannot be read shows its error in the picker",
        text_entry_filepicker_errors,
    ),
];

pub(crate) const ICONS: &[Case] = &[
    pictured!("states", "An icon goes idle, hover, pressed and released on the corresponding pointer edges", icons_states),
    pictured!("spring-steps", "Spring positions after fixed time steps equal the contract's literal table", icons_spring_steps),
    pictured!("step-invariance", "One step of 1,000 ms, a thousand steps of 1 ms and steps of 16 ms give identical states at equal times", icons_step_invariance),
    pictured!("settles", "Every catalogue animation settles within 2,000 ms of UI time after its last input and never draws outside its box", icons_settles),
    pictured!("interrupt", "A hover interrupted by a press mid-animation follows the contract's state table", icons_interrupt),
    pictured!("reversal", "Leaving mid-hover reverses from the current position without a jump", icons_reversal),
    pictured!("reset", "Reset after a resize returns the icon to idle at its new geometry", icons_reset),
    pictured!("bounded", "Over 10,000 steps of seeded random pointer input, the trajectory stays inside its box", icons_bounded),
    pictured!("easing", "Each easing function matches the literal table at its sample points", icons_easing),
    pictured!("sin-approx", "sin_approx stays within 0.0012 of sin at 4,096 evenly spaced points of [−π, π]", icons_sin_approx),
    pictured!("particles-bounded", "Particle counts stay at or below the cap, and positions stay inside the box", icons_particles_bounded),
    pictured!("assets", "Each icon in each state draws to its reviewed reference", icons_assets),
];

fn text_fontwatch_reload() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    let old = watch_frame(&mut w, true);
    snapshot(&old)?;
    f.config("display.ttf", 16, "display.ttf", 24).cite("9.3")?;
    f.step(249_999_999).cite("9.1")?;
    ensure(w.poll() == Ok(None), "9.3", "watcher checked before 250ms")?;
    f.step(250_000_000).cite("9.1")?;
    ensure(
        w.poll()
            == Ok(Some(FontChange {
                mono: true,
                display: false,
            })),
        "9.3",
        "changed mono font was not applied at first check",
    )?;
    ensure(
        w.mono_font()
            .is_some_and(|font| font.cache_stats().entries == 0),
        "9.3",
        "changed font retained cached glyphs",
    )?;
    let new = watch_frame(&mut w, true);
    snapshot(&new)?;
    ensure(
        same_pixels(&old, &new).is_some(),
        "9.3",
        "changing mono font left all pixels unchanged",
    )?;
    f.assert_frame(&new, "display.ttf", 16.0)?;
    snapshot(&new)
}

fn text_fontwatch_size() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    let old = watch_frame(&mut w, true);
    snapshot(&old)?;
    f.config("mono.ttf", 32, "display.ttf", 24).cite("9.3")?;
    f.step(250_000_000).cite("9.1")?;
    ensure(
        w.poll()
            == Ok(Some(FontChange {
                mono: true,
                display: false,
            }))
            && w.mono_size() == 32.0,
        "9.3",
        "new mono size was not applied",
    )?;
    ensure(
        w.mono_font()
            .is_some_and(|font| font.cache_stats().entries == 0),
        "9.3",
        "size change retained old cache entries",
    )?;
    let new = watch_frame(&mut w, true);
    snapshot(&new)?;
    ensure(
        same_pixels(&old, &new).is_some(),
        "9.3",
        "size change did not change rasterized pixels",
    )?;
    f.assert_frame(&new, "mono.ttf", 32.0)?;
    snapshot(&new)
}

fn text_fontwatch_missing() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    let old = watch_frame(&mut w, true);
    snapshot(&old)?;
    let old_display = watch_frame(&mut w, false);
    snapshot(&old_display)?;
    f.config("display.ttf", 32, "absent.ttf", 40).cite("9.3")?;
    f.step(250_000_000).cite("9.1")?;
    ensure(
        w.poll() == Err(FontWatchError::Font(f.path("absent.ttf"))),
        "9.3",
        "missing second font did not report its path",
    )?;
    let actual_mono = watch_frame(&mut w, true);
    snapshot(&actual_mono)?;
    ensure(same_pixels(&old, &actual_mono).is_none(), "9.3", "missing font changed mono pixels")?;
    let actual_display = watch_frame(&mut w, false);
    snapshot(&actual_display)?;
    ensure(
        w.mono_size() == 16.0
            && w.display_size() == 24.0
            && same_pixels(&old_display, &actual_display).is_none(),
        "9.3",
        "missing font partly changed previous fonts",
    )
}

fn text_fontwatch_malformed_config() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    let old = watch_frame(&mut w, true);
    snapshot(&old)?;
    for (i, bad) in [
        "mono.font=/cut",
        "mono.font=x\nmono.size=16\ndisplay.font=x\ndisplay.size=NaN\n",
        "mono.font=x\nmono.size=16\ndisplay.font=x\ndisplay.size=24\nmono.size=18\n",
        "mono.font=x\nmono.size=73\ndisplay.font=x\ndisplay.size=24\n",
    ]
    .iter()
    .enumerate()
    {
        std::fs::write(f.root.join("etc/fonts.conf"), bad)
            .map_err(|e| format!("write config: {e}"))
            .cite("9.2")?;
        f.step((i as u64 + 1) * 250_000_000).cite("9.1")?;
        ensure(
            w.poll() == Err(FontWatchError::Config),
            "9.2",
            format!("malformed config {i} was not rejected"),
        )?;
        let actual = watch_frame(&mut w, true);
        snapshot(&actual)?;
        ensure(
            w.mono_size() == 16.0 && same_pixels(&old, &actual).is_none(),
            "9.3",
            "malformed configuration changed old pixels",
        )?;
    }
    Ok(())
}

fn text_fontwatch_partial() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    std::fs::write(f.root.join("etc/pending"), "mono.font=/cut")
        .map_err(|e| format!("write partial: {e}"))
        .cite("9.2")?;
    std::fs::rename(f.root.join("etc/pending"), f.root.join("etc/fonts.conf"))
        .map_err(|e| format!("rename config: {e}"))
        .cite("9.2")?;
    f.step(250_000_000).cite("9.1")?;
    ensure(
        w.poll() == Err(FontWatchError::Config) && w.mono_size() == 16.0,
        "9.3",
        "partial renamed config did not preserve previous size",
    )?;
    f.config("mono.ttf", 28, "display.ttf", 24).cite("9.2")?;
    f.step(500_000_000).cite("9.1")?;
    ensure(
        w.poll()
            == Ok(Some(FontChange {
                mono: true,
                display: false,
            }))
            && w.mono_size() == 28.0,
        "9.3",
        "complete renamed config was not applied",
    )
}

fn text_fontwatch_repeated() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    for i in 0..5 {
        f.config("mono.ttf", 20 + i, "display.ttf", 24)
            .cite("9.2")?;
        f.step((i as u64 + 1) * 250_000_000).cite("9.1")?;
        ensure(
            w.poll()
                == Ok(Some(FontChange {
                    mono: true,
                    display: false,
                }))
                && w.mono_size() == (20 + i) as f32,
            "9.3",
            format!("change {i} was not applied"),
        )?;
    }
    let frame = watch_frame(&mut w, true);
    snapshot(&frame)?;
    f.assert_frame(&frame, "mono.ttf", 24.0)?;
    snapshot(&frame)
}

fn text_fontwatch_independent() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    let mono = watch_frame(&mut w, true);
    snapshot(&mono)?;
    let display = watch_frame(&mut w, false);
    snapshot(&display)?;
    f.config("mono.ttf", 16, "mono.ttf", 24).cite("9.2")?;
    f.step(250_000_000).cite("9.1")?;
    ensure(
        w.poll()
            == Ok(Some(FontChange {
                mono: false,
                display: true,
            })),
        "9.3",
        "display change was not independent",
    )?;
    let actual_mono = watch_frame(&mut w, true);
    snapshot(&actual_mono)?;
    ensure(same_pixels(&mono, &actual_mono).is_none(), "9.3", "display reload changed mono pixels")?;
    let actual_display = watch_frame(&mut w, false);
    snapshot(&actual_display)?;
    ensure(
        same_pixels(&display, &actual_display).is_some(),
        "9.3",
        "display reload changed mono pixels or failed to change display pixels",
    )?;
    let held_mono = w.take_mono_font();
    f.config("display.ttf", 16, "mono.ttf", 24).cite("9.2")?;
    let display = watch_frame(&mut w, false);
    snapshot(&display)?;
    f.step(500_000_000).cite("9.1")?;
    ensure(
        w.poll()
            == Ok(Some(FontChange {
                mono: true,
                display: false,
            })),
        "9.3",
        "mono change was not independent",
    )?;
    w.put_mono_font(held_mono);
    let reloaded_mono = watch_frame(&mut w, true);
    snapshot(&reloaded_mono)?;
    f.assert_frame(&reloaded_mono, "display.ttf", 16.0)?;
    let actual_display = watch_frame(&mut w, false);
    snapshot(&actual_display)?;
    ensure(
        same_pixels(&display, &actual_display).is_none(),
        "9.3",
        "mono reload changed display pixels",
    )
}

fn text_fontwatch_idle() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    let mut win = breengel::Window::new(b"idle font reload", 200, 70)
        .map_err(|e| format!("create idle window: {e:?}")).cite("9.3")?;
    // Drain startup notifications before the idle interval: no input is injected.
    win.dispatch(|_| breengel::Handled::NoChange)
        .map_err(|e| format!("drain startup: {e:?}")).cite("9.3")?;
    f.config("display.ttf", 16, "display.ttf", 24).cite("9.3")?;
    for (ns, expected_font, changed) in [
        (249_999_999, "mono.ttf", false), (250_000_000, "display.ttf", true),
    ] {
        f.step(ns).cite("9.1")?;
        let mut font_events = 0;
        let mut input_events = 0;
        win.dispatch(|delivered| {
            if matches!(delivered.event, breengel::Event::FontChanged) { font_events += 1; }
            else { input_events += 1; }
            breengel::Handled::Redraw
        }).map_err(|e| format!("idle dispatch: {e:?}")).cite("9.3")?;
        ensure(input_events == 0 && font_events == usize::from(changed), "9.3",
            format!("idle epoch {ns}: font events={font_events}, input events={input_events}"))?;
        let mut font = win.take_mono_font().ok_or("idle window lost mono font").cite("9.3")?;
        let size = win.mono_size();
        let fb = win.framebuf();
        fb.clear(PAPER);
        ttf_font::draw_text(fb, &mut font, "Wi é A", 8, 40, size, Color::WHITE);
        win.put_mono_font(Some(font));
        let commit = win.commit(&[libbreenix::window::abi::Rect::new(0, 0, 200, 70)])
            .map_err(|e| format!("idle commit: {e:?}")).cite("9.3")?;
        let mut pixels = vec![0u8; 200 * 70 * 4];
        let capture = libbreenix::window::capture(win.id(), &mut pixels, None)
            .map_err(|e| format!("capture idle commit: {e:?}")).cite("9.3")?;
        ensure(capture.commit_id == commit.commit_id, "9.3", "capture is not the first idle epoch commit")?;
        let got = Guarded::bgrx(200, 70);
        // CAPTURE supplied exactly the checked frame's extent.
        unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), got.base, pixels.len()); }
        f.assert_frame(&got, expected_font, 16.0)?;
        snapshot(&got)?;
    }
    ensure(breengel::clock::real_clock_reads() == 0, "9.1", "idle reload read real time")
}

fn text_fontwatch_file_replaced() -> CaseResult {
    let f = FontFixture::new().cite("9.3")?;
    use std::os::unix::fs::MetadataExt;
    let mut original = std::fs::read(f.root.join("mono.ttf")).map_err(|e| e.to_string()).cite("9.3")?;
    let size = std::fs::metadata(f.root.join("display.ttf")).map_err(|e| e.to_string()).cite("9.3")?.len();
    original.resize(size as usize, 0);
    std::fs::write(f.root.join("mono.ttf"), original).map_err(|e| e.to_string()).cite("9.3")?;
    let before = std::fs::metadata(f.root.join("mono.ttf")).map_err(|e| e.to_string()).cite("9.3")?;
    let mut w = FontWatcher::try_new()
        .map_err(|e| format!("watcher startup: {e:?}"))
        .cite("9.3")?;
    let old = watch_frame(&mut w, true);
    snapshot(&old)?;
    let config = std::fs::read(f.root.join("etc/fonts.conf"))
        .map_err(|e| format!("read config: {e}"))
        .cite("9.3")?;
    std::fs::copy(f.root.join("display.ttf"), f.root.join("pending.ttf"))
        .map_err(|e| format!("copy replacement: {e}"))
        .cite("9.3")?;
    std::fs::File::options().write(true).open(f.root.join("pending.ttf"))
        .and_then(|file| file.set_times(std::fs::FileTimes::new().set_modified(before.modified()?)))
        .map_err(|e| format!("preserve replacement mtime: {e}")).cite("9.3")?;
    let replacement = std::fs::metadata(f.root.join("pending.ttf")).map_err(|e| e.to_string()).cite("9.3")?;
    ensure(before.ino() != replacement.ino() && before.len() == replacement.len()
        && before.mtime() == replacement.mtime() && before.mtime_nsec() == replacement.mtime_nsec(),
        "9.3", "replacement must differ only by inode in the watched metadata")?;
    std::fs::rename(f.root.join("pending.ttf"), f.root.join("mono.ttf"))
        .map_err(|e| format!("rename font: {e}"))
        .cite("9.3")?;
    f.step(250_000_000).cite("9.1")?;
    ensure(
        w.poll()
            == Ok(Some(FontChange {
                mono: true,
                display: false,
            })),
        "9.3",
        "same-path font replacement was not observed",
    )?;
    ensure(
        std::fs::read(f.root.join("etc/fonts.conf")).ok() == Some(config),
        "9.3",
        "fixture changed fonts.conf during same-path replacement",
    )?;
    let frame = watch_frame(&mut w, true);
    snapshot(&frame)?;
    ensure(
        same_pixels(&old, &frame).is_some(),
        "9.3",
        "same-path replacement left old glyphs cached",
    )?;
    f.assert_frame(&frame, "display.ttf", 16.0)?;
    snapshot(&frame)
}

// ---------------------------------------------------------------------------------------
// widgets (contract section 14)
// ---------------------------------------------------------------------------------------

fn input(x: i32, y: i32, down: bool, pressed: bool, released: bool) -> InputState {
    InputState {
        time_ms: 0,
        mouse_x: x,
        mouse_y: y,
        mouse_down: down,
        mouse_pressed: pressed,
        mouse_released: released,
    }
}
fn hover(x: i32, y: i32) -> InputState {
    input(x, y, false, false, false)
}
fn press(x: i32, y: i32) -> InputState {
    input(x, y, true, true, false)
}
fn hold(x: i32, y: i32) -> InputState {
    input(x, y, true, false, false)
}
fn release(x: i32, y: i32) -> InputState {
    input(x, y, false, false, true)
}

fn canonical(g:&Guarded)->Vec<u8> {
    let fb=g.fb();let mut pixels=Vec::with_capacity(g.width*g.height*4);
    for y in 0..g.height {for x in 0..g.width {let p=fb.get_pixel(x,y);pixels.extend_from_slice(&[p.b,p.g,p.r,255]);}}
    pixels
}
fn snapshot(g:&Guarded)->CaseResult {
    remember_frame(&canonical(g),g.width,g.height,g.reference.borrow().clone());Ok(())
}

/// Compare `got` with a surface drawn by `expect`, both on a `bg` background.
fn same_as(
    got: &Guarded,
    bg: Color,
    what: &str,
    section: &str,
    expect: impl FnOnce(&mut FrameBuf),
) -> CaseResult {
    let mut want = Guarded::bgrx(got.width, got.height);
    want.reset(bg);
    expect(&mut want.fb());
    let reference=canonical(&want);
    *got.reference.borrow_mut()=Some(reference.clone());
    remember_frame(&canonical(got),got.width,got.height,Some(reference));
    for y in 0..got.height {
        for x in 0..got.width {
            if got.color(x, y) != want.color(x, y) {
                return at(
                    section,
                    format!(
                        "{what}: pixel ({x},{y}) is {}; the contract's drawing gives {}",
                        hex(got.color(x, y)),
                        hex(want.color(x, y))
                    ),
                );
            }
        }
    }
    Ok(())
}

// gpu-text: R3 centers the certified DejaVu Sans 16 px label using 26.6 advances.
fn widgets_button_draw() -> CaseResult {
    let theme = Theme::dark();
    let mut g = Guarded::bgrx(100, 40);
    g.reset(PAPER);
    Button::new(Rect::new(10, 8, 60, 20), b"OK").draw(&mut g.fb(), &theme);
    snapshot(&g)?;
    same_as(
        &g,
        PAPER,
        "an idle 60x20 button labelled OK",
        "14.3",
        |fb| {
            shapes::fill_rect(fb, 10, 8, 60, 20, theme.widget_bg);
            shapes::draw_rect(fb, 10, 8, 60, 20, theme.border);
            // R3 uses font advances and a 16 px line box.
            sans_center(fb,b"OK",Rect::new(10,8,60,20),theme.text_primary);
        },
    )
}

fn pointer_event<T: std::fmt::Debug>(target: &str, input: InputState, handle: impl FnOnce(&InputState) -> T) -> T {
    let result = handle(&input);
    let hit = format!("{} / {target}: {result:?}", libbreenix::suite::running_case().unwrap_or("widget"));
    libbreenix::suite::input_step(input.mouse_x, input.mouse_y, input.mouse_down, &hit);
    result
}

fn key_event<T: std::fmt::Debug>(target: &str, input: KeyInput<'_>, handle: impl FnOnce(&KeyInput<'_>) -> T) -> T {
    let result = handle(&input);
    libbreenix::suite::input_key(&format!("{} / {target}: {:?} => {result:?}", libbreenix::suite::running_case().unwrap_or("widget"), input.key));
    result
}

/// Feed `inputs` to `update` and collect the events that are not `None`.
fn events(
    mut update: impl FnMut(&InputState) -> WidgetEvent,
    inputs: &[InputState],
) -> Vec<WidgetEvent> {
    inputs
        .iter()
        .map(|i| {
            let event=update(i);
            let target=libbreenix::suite::running_case().unwrap_or("widget");
            let hit=if event==WidgetEvent::None {String::new()} else {format!("{target}: {event:?}")};
            libbreenix::suite::input_step(i.mouse_x,i.mouse_y,i.mouse_down,&hit);
            event
        })
        .filter(|e| *e != WidgetEvent::None)
        .collect()
}

fn widgets_button_click() -> CaseResult {
    let mut b = Button::new(Rect::new(10, 8, 60, 20), b"OK");
    let got = events(
        |i| b.update(i),
        &[
            hover(30, 15),
            press(30, 15),
            hold(31, 16),
            release(31, 16),
            hover(31, 16),
            hover(90, 30),
        ],
    );
    ensure(
        got == vec![WidgetEvent::Clicked],
        "14.3",
        format!(
            "press and release inside reported {got:?}; the contract gives exactly one Clicked"
        ),
    )
}

fn widgets_button_cancel() -> CaseResult {
    let mut b = Button::new(Rect::new(10, 8, 60, 20), b"OK");
    let got = events(
        |i| b.update(i),
        &[press(30, 15), hold(95, 35), release(95, 35), hover(30, 15)],
    );
    ensure(
        got.is_empty(),
        "14.3",
        format!("a press inside released outside reported {got:?}; the contract reports nothing"),
    )?;
    let got = events(
        |i| b.update(i),
        &[press(95, 35), hold(30, 15), release(30, 15)],
    );
    ensure(
        got.is_empty(),
        "14.3",
        format!("a press outside released inside reported {got:?}; the contract reports nothing"),
    )
}

// gpu-text: R3 changes only the label metrics and certified text pixels; disabled input and colours remain exact.
fn widgets_button_disabled() -> CaseResult {
    let theme = Theme::dark();
    let mut b = Button::new(Rect::new(10, 8, 60, 20), b"OK");
    b.set_enabled(false);
    ensure(
        events(
            |i| b.update(i),
            &[hover(30, 15), press(30, 15), release(30, 15)],
        )
        .is_empty(),
        "14.2",
        "disabled button emitted an event",
    )?;
    let mut g = Guarded::bgrx(100, 40);
    g.reset(PAPER);
    b.draw(&mut g.fb(), &theme);
    snapshot(&g)?;
    same_as(&g, PAPER, "disabled button", "14.2", |fb| {
        shapes::fill_rect(fb, 10, 8, 60, 20, theme.widget_bg);
        shapes::draw_rect(fb, 10, 8, 60, 20, theme.border);
        sans_center(fb,b"OK",Rect::new(10,8,60,20),theme.text_secondary);
    })?;
    snapshot(&g)
}

// gpu-text: R3 uses DejaVu Sans label pixels; both toggle events and checkbox geometry stay exact.
fn widgets_checkbox_toggle() -> CaseResult {
    let theme = Theme::dark();
    let mut c = Checkbox::new(Rect::new(10, 10, 120, 20), b"Opt");
    let got = events(
        |i| c.update(i),
        &[hover(15, 18), press(15, 18), release(15, 18)],
    );
    ensure(
        got == vec![WidgetEvent::Toggled(true)] && c.checked,
        "14.3",
        format!(
            "one click reported {got:?} and left checked = {}",
            c.checked
        ),
    )?;
    let mut g = Guarded::bgrx(140, 40);
    g.reset(PAPER);
    c.draw(&mut g.fb(), &theme);
    snapshot(&g)?;
    same_as(&g, PAPER, "a checked checkbox", "14.3", |fb| {
        let (bx, by) = (10, 13);
        shapes::fill_rect(fb, bx, by, 14, 14, theme.accent);
        shapes::draw_line(fb, bx + 3, by + 7, bx + 5, by + 10, theme.text_primary);
        shapes::draw_line(fb, bx + 5, by + 10, bx + 11, by + 3, theme.text_primary);
        shapes::draw_rect(fb, bx, by, 14, 14, theme.border);
        sans_draw(fb,b"Opt",28,12,theme.text_primary);
    })?;
    let got = events(|i| c.update(i), &[press(15, 18), release(15, 18)]);
    ensure(
        got == vec![WidgetEvent::Toggled(false)] && !c.checked,
        "14.3",
        format!(
            "a second click reported {got:?} and left checked = {}",
            c.checked
        ),
    )
}

fn widgets_checkbox_space() -> CaseResult {
    let mut c = Checkbox::new(Rect::new(10, 10, 100, 20), b"Opt");
    ensure(
        key_event("checkbox", KeyInput::new(Key::Space), |i| c.key(i)) == WidgetEvent::None && !c.checked,
        "14.3",
        "unfocused checkbox toggled",
    )?;
    c.set_focused(true);
    ensure(
        key_event("checkbox", KeyInput::new(Key::Space), |i| c.key(i)) == WidgetEvent::Toggled(true) && c.checked,
        "14.3",
        "Space did not toggle the focused checkbox",
    )?;
    ensure(
        key_event("checkbox", KeyInput::new(Key::Space), |i| c.key(i)) == WidgetEvent::Toggled(false) && !c.checked,
        "14.3",
        "second Space did not toggle back",
    )?;
    c.set_enabled(false);
    ensure(
        key_event("checkbox", KeyInput::new(Key::Space), |i| c.key(i)) == WidgetEvent::None && !c.checked,
        "14.2",
        "disabled checkbox toggled",
    )
}

/// The slider's value for a pointer at `x`, from the contract's formula (section 14.3).
fn slider_value(x: i32) -> f32 {
    let v = (x - 26) as f32 / 100.0;
    v.clamp(0.0, 1.0) * 100.0
}

fn widgets_slider_drag() -> CaseResult {
    let mut s = Slider::new(Rect::new(20, 10, 112, 16), 0.0, 100.0);
    let got = events(
        |i| s.update(i),
        &[press(51, 18), hold(106, 18), hold(77, 17), release(77, 17)],
    );
    let want = vec![
        WidgetEvent::ValueChanged(slider_value(51)),
        WidgetEvent::ValueChanged(slider_value(106)),
        WidgetEvent::ValueChanged(slider_value(77)),
    ];
    ensure(
        got == want,
        "14.3",
        format!("dragging to x = 51, 106, 77 reported {got:?}; the contract gives {want:?}"),
    )
}

fn widgets_slider_clamp() -> CaseResult {
    let mut s = Slider::new(Rect::new(20, 10, 112, 16), 0.0, 100.0);
    let got = events(
        |i| s.update(i),
        &[
            press(60, 18),
            hold(0, 18),
            hold(-500, 18),
            hold(500, 18),
            release(500, 18),
        ],
    );
    let want = vec![
        WidgetEvent::ValueChanged(slider_value(60)),
        WidgetEvent::ValueChanged(0.0),
        WidgetEvent::ValueChanged(100.0),
    ];
    ensure(
        got == want,
        "14.3",
        format!("dragging past both ends reported {got:?}; the contract clamps to {want:?}"),
    )
}

fn widgets_slider_keys() -> CaseResult {
    let mut slider = Slider::new(Rect::new(0, 0, 112, 20), 10.0, 20.0);
    slider.set_step(3.0);
    ensure(
        key_event("slider", KeyInput::new(Key::Right), |i| slider.key(i)) == WidgetEvent::None,
        "14.3",
        "unfocused slider responded to a key",
    )?;
    slider.set_focused(true);
    for (key, want, event) in [
        (Key::Right, 13.0, true),
        (Key::Right, 16.0, true),
        (Key::Right, 19.0, true),
        (Key::Right, 20.0, true),
        (Key::Right, 20.0, false),
        (Key::Left, 17.0, true),
        (Key::Left, 14.0, true),
        (Key::Left, 11.0, true),
        (Key::Left, 10.0, true),
        (Key::Left, 10.0, false),
    ] {
        let got = key_event("slider", KeyInput::new(key), |i| slider.key(i));
        ensure(
            slider.mapped_value() == want
                && got
                    == if event {
                        WidgetEvent::ValueChanged(want)
                    } else {
                        WidgetEvent::None
                    },
            "14.3",
            format!(
                "slider {key:?} gave {got:?}, value {}; expected {want}",
                slider.mapped_value()
            ),
        )?;
    }
    Ok(())
}

// gpu-text: R3 derives ellipsis prefixes from 26.6 DejaVu advances; clipping and empty areas remain exact.
fn widgets_label_clip() -> CaseResult {
    let theme = Theme::dark();
    let mut g = Guarded::bgrx(80, 40);
    g.reset(PAPER);
    Label::new(Rect::new(10, 8, 35, 16), b"ABCDEFGHIJKLM").draw(&mut g.fb(), &theme);
    snapshot(&g)?;
    same_as(&g, PAPER, "35-pixel clipped label", "14.3", |fb| {
        let mut prefix=Vec::new();
        for c in b"ABCDEFGHIJKLM" {let mut next=prefix.clone();next.push(*c);next.extend_from_slice(b"...");if sans_width(&next)>35 {break;}prefix.push(*c);}
        prefix.extend_from_slice(b"...");fb.push_clip(libgfx_ref::framebuf::Rect::new(10,8,35,16));
        sans_draw(fb,&prefix,10,8,theme.text_primary);fb.pop_clip();
    })?;
    let mut narrow = Guarded::bgrx(80, 40);
    narrow.reset(PAPER);
    Label::new(Rect::new(10, 8, 12, 16), b"ABCDEFGHIJKLM").draw(&mut narrow.fb(), &theme);
    snapshot(&narrow)?;
    for y in 0..40 {
        for x in 0..80 {
            if !in_rect(x as i64, y as i64, 10, 8, 12, 16) {
                ensure(
                    narrow.color(x, y) == PAPER,
                    "14.3",
                    format!("small label wrote outside bounds at {x},{y}"),
                )?;
            }
        }
    }
    ensure(
        drawn_pixels(&narrow, PAPER) > 0,
        "14.3",
        "small label drew no visible ellipsis",
    )?;
    // R3 uses the certified glyph pixels and clips the final dot at the physical bounds.
    same_as(&narrow, PAPER, "12x16 DejaVu ellipsis", "14.3", |fb| {
        fb.push_clip(libgfx_ref::framebuf::Rect::new(10,8,12,16));
        sans_draw(fb,b"...",10,8,theme.text_primary);fb.pop_clip();
    })?;
    narrow.reset(PAPER);
    Label::new(Rect::new(10, 8, 12, 3), b"ABCDEFGHIJKLM").draw(&mut narrow.fb(), &theme);
    snapshot(&narrow)?;
    ensure(
        drawn_pixels(&narrow, PAPER) == 0,
        "14.3",
        "clip above dot rows drew ink",
    )?;
    same_as(
        &narrow,
        PAPER,
        "12x3 clip above the dot rows",
        "14.3",
        |_| {},
    )?;
    snapshot(&g)
}

fn widgets_tabbar_select() -> CaseResult {
    let mut t = TabBar::new(
        Rect::new(0, 0, 300, 24),
        vec![&b"One"[..], &b"Two"[..], &b"Three"[..]], &Theme::dark());
    let got = events(|i| t.update(i), &[press(150, 12), release(150, 12)]);
    ensure(got == vec![WidgetEvent::ValueChanged(1.0)] && t.selected() == 1, "14.3",
        format!("clicking the second tab reported {got:?} and selected {}; the contract reports ValueChanged(1) and selects 1", t.selected()))?;
    let got = events(
        |i| t.update(i),
        &[
            press(250, 12),
            release(250, 12),
            press(260, 12),
            release(260, 12),
        ],
    );
    ensure(got == vec![WidgetEvent::ValueChanged(2.0)] && t.selected() == 2, "14.3",
        format!("clicking the third tab twice reported {got:?}; the contract selects on the first click only"))
}

fn widgets_tabbar_overflow() -> CaseResult {
    let mut tabs = TabBar::new(
        Rect::new(10, 8, 70, 24),
        vec![b"First", b"Second", b"Third", b"Fourth", b"Fifth"], &Theme::dark());
    for i in 0..5 {
        tabs.scroll_by(if i == 0 { 0 } else { 1 });
        pointer_event("tabs", press(20, 18), |i| tabs.update(i));
        let event = pointer_event("tabs", release(20, 18), |i| tabs.update(i));
        ensure(
            tabs.selected() == i
                && event
                    == if i == 0 {
                        WidgetEvent::None
                    } else {
                        WidgetEvent::ValueChanged(i as f32)
                    },
            "14.3",
            format!(
                "scrolled tab {i} was unreachable: selected {} event {event:?}",
                tabs.selected()
            ),
        )?;
    }
    tabs.scroll_by(-100);
    pointer_event("tabs", release(20, 18), |i| tabs.update(i));
    ensure(
        tabs.selected() == 0,
        "14.3",
        "scrolling back did not reach first tab",
    )?;
    let mut g = Guarded::bgrx(100, 50);
    g.reset(PAPER);
    tabs.draw(&mut g.fb(), &Theme::dark());
    snapshot(&g)?;
    ensure(g.color(12, 10) == Theme::dark().widget_bg_active
        && g.color(12, 30) == Theme::dark().accent, "14.3", "tab fill or selected highlight missing")?;
    for y in 0..50 {
        for x in 0..100 {
            if !in_rect(x as i64, y as i64, 10, 8, 70, 24) {
                ensure(
                    g.color(x, y) == PAPER,
                    "14.3",
                    format!("overflowing tab wrote outside bar at {x},{y}"),
                )?;
            }
        }
    }
    snapshot(&g)
}

fn widgets_scrollbar_drag() -> CaseResult {
    let mut s = ScrollBar::new(Rect::new(0, 0, 12, 200), 1000, 200);
    pointer_event("scrollbar", press(6, 10), |i| s.update(i));
    pointer_event("scrollbar", hold(6, 90), |i| s.update(i));
    // The thumb (40 px of a 160 px range) moved from 0 to 80: offset 80 / 160 × 800.
    ensure(
        s.offset() == 400,
        "14.3",
        format!(
            "dragging the thumb 80 px down set offset {}; the contract gives 400",
            s.offset()
        ),
    )?;
    pointer_event("scrollbar", hold(6, 500), |i| s.update(i));
    ensure(
        s.offset() == 800,
        "14.3",
        format!(
            "dragging past the end set offset {}; the contract clamps it to 800",
            s.offset()
        ),
    )
}

fn widgets_scrollbar_wheel() -> CaseResult {
    let mut s = ScrollBar::new(Rect::new(0, 0, 12, 200), 1000, 200);
    for (d, want) in [(-1, 40), (-1, 80), (2, 0), (-100, 800), (1, 760)] {
        s.scroll(d);
        ensure(
            s.offset() == want,
            "14.3",
            format!(
                "scroll({d}) left offset {}; the contract gives {want}",
                s.offset()
            ),
        )?;
    }
    Ok(())
}

fn widgets_scrollbar_thumb() -> CaseResult {
    for (content, view, thumb) in [(1000, 250, 50), (1000, 500, 100), (10_000, 100, 20)] {
        let s = ScrollBar::new(Rect::new(4, 0, 12, 200), content, view);
        let mut g = Guarded::bgrx(20, 200);
        g.reset(PAPER);
        s.draw(&mut g.fb(), &Theme::dark());
        snapshot(&g)?;
        let rows = (0..200)
            .filter(|&y| g.color(9, y) == Color::rgb(80, 85, 100))
            .count();
        ensure(rows == thumb - 2, "14.3", format!(
            "content {content} with view {view}: the thumb is drawn {rows} rows tall; a {thumb} px thumb draws {} rows", thumb - 2))?;
    }
    Ok(())
}

fn test_menus() -> Vec<Menu> {
    vec![
        Menu {
            label: b"File",
            items: vec![
                MenuItem::new(b"New", 1, Shortcut::ctrl(b'N')),
                MenuItem::disabled(b"Save", 2, Shortcut::ctrl(b'S')),
                MenuItem::new(b"Save As", 3, Shortcut::ctrl_shift(b'S')),
                MenuItem::separator(),
                MenuItem::new(b"Quit", 4, Shortcut::ctrl(b'Q')),
            ],
        },
        Menu {
            label: b"Edit",
            items: vec![
                MenuItem::new(b"Copy", 5, Shortcut::ctrl(b'C')),
                MenuItem::new(b"Paste", 6, Shortcut::ctrl(b'V')),
            ],
        },
    ]
}

/// A menu bar at (10, 5), 300 wide, with File open; the File title is (10, 5, 43, 22).
fn open_menu() -> (MenuBar, Theme) {
    let theme = Theme::dark();
    let mut m = MenuBar::new(Rect::new(10, 5, 300, 22), test_menus());
    m.layout(&theme);
    pointer_event("menu", press(30, 15), |i| m.update(i, &theme));
    pointer_event("menu", release(30, 15), |i| m.update(i, &theme));
    (m, theme)
}

// gpu-text: R3 derives menu label and shortcut widths from certified 26.6 advances.
fn widgets_menubar_open() -> CaseResult {
    let (m, theme) = open_menu();
    ensure(
        m.is_open(),
        "14.3",
        "a press and release on the File title left its menu closed",
    )?;
    let mut g = Guarded::bgrx(330, 140);
    g.reset(PAPER);
    m.draw(&mut g.fb(), &theme);
    snapshot(&g)?;
    let width=18+24+sans_width(b"Save As")+24+sans_width(b"Ctrl+Shift+S")+8;
    let (dx,dy,dw,dh)=(10usize,27usize,width as usize,97usize);
    for (x, y, want, what) in [
        (dx, dy + 30, theme.border, "its left border"),
        (dx + dw - 1, dy + 30, theme.border, "its right border"),
        (dx, dy + dh - 1, theme.border, "its bottom border"),
        (dx + dw, dy + 30, PAPER, "the pixel right of it"),
        (dx + 2, dy + dh - 3, theme.widget_bg, "its fill"),
        (dx + 2, dy + dh, PAPER, "the pixel below it"),
    ] {
        ensure(
            g.color(x, y) == want,
            "14.3",
            format!(
                "{what} at ({x},{y}) is {}, the contract gives {}",
                hex(g.color(x, y)),
                hex(want)
            ),
        )?;
    }
    let mut want = Guarded::bgrx(sans_width(b"New") as usize, 16);
    want.reset(theme.widget_bg);
    sans_draw(&mut want.fb(),b"New",0,0,theme.text_primary);
    for y in 0..16 {
        for x in 0..want.width {
            let got = g.color(dx + 18 + x, dy + 3 + y);
            ensure(
                got == want.color(x, y),
                "14.3",
                format!(
                    "the first item's label pixel ({},{}) is {}, the contract gives {}",
                    dx + 18 + x,
                    dy + 3 + y,
                    hex(got),
                    hex(want.color(x, y))
                ),
            )?;
        }
    }
    Ok(())
}

fn widgets_menubar_shortcut() -> CaseResult {
    let m = MenuBar::new(Rect::new(0, 0, 300, 22), test_menus());
    for (ascii, ctrl, shift, want) in [
        (b'N', true, false, Some(1u16)),
        (0x0E, true, false, Some(1)),
        (b'n', true, false, Some(1)),
        (b'S', true, true, Some(3)),
        (b'C', true, false, Some(5)),
        (b'S', true, false, None),
        (b'Z', true, false, None),
        (b'N', false, false, None),
    ] {
        let got = m.match_shortcut(ascii, ctrl, shift, false);
        ensure(got == want, "14.3", format!("match_shortcut({ascii:#04x}, ctrl {ctrl}, shift {shift}) gave {got:?}; the contract gives {want:?} (Save is disabled)"))?;
    }
    Ok(())
}

fn widgets_menu_keys() -> CaseResult {
    let (mut menu, _) = open_menu();
    for (key, want) in [
        (Key::Down, 0),
        (Key::Down, 2),
        (Key::Down, 4),
        (Key::Down, 0),
        (Key::Up, 4),
    ] {
        ensure(
            key_event("menu", KeyInput::new(key), |i| menu.key(i)) == MenuEvent::None && menu.highlighted() == Some(want),
            "14.3",
            format!("menu {key:?} did not highlight enabled item {want}"),
        )?;
    }
    ensure(
        key_event("menu", KeyInput::new(Key::Enter), |i| menu.key(i)) == MenuEvent::Activated(4) && !menu.is_open(),
        "14.3",
        "Enter did not activate Quit and close",
    )?;
    let (mut menu, _) = open_menu();
    ensure(
        key_event("menu", KeyInput::new(Key::Escape), |i| menu.key(i)) == MenuEvent::None && !menu.is_open(),
        "14.3",
        "Escape did not close without activation",
    )
}

// gpu-text: R3 changes disabled label metrics and pixels; disabled activation remains forbidden.
fn widgets_menu_disabled() -> CaseResult {
    let (mut m, theme) = open_menu();
    // Save, the disabled second item, is the row at y = 27 + 22 .. 27 + 44.
    let mut got = Vec::new();
    for i in [hover(40, 60), press(40, 60), release(40, 60)] {
        let e = pointer_event("menu", i, |i| m.update(i, &theme));
        if e != MenuEvent::None {
            got.push(e);
        }
    }
    ensure(got.is_empty(), "14.3", format!("clicking the disabled item reported {got:?}; the contract does not let a disabled item be chosen"))?;
    let mut g = Guarded::bgrx(330, 140);
    g.reset(PAPER);
    m.draw(&mut g.fb(), &theme);
    snapshot(&g)?;
    let mut want = Guarded::bgrx(sans_width(b"Save") as usize, 16);
    want.reset(theme.widget_bg);
    sans_draw(&mut want.fb(),b"Save",0,0,theme.text_secondary);
    for y in 0..16 {
        for x in 0..want.width {
            let got = g.color(28 + x, 27 + 22 + 3 + y);
            ensure(
                got == want.color(x, y),
                "14.3",
                format!(
                    "the disabled label's pixel ({},{}) is {}, the contract dims it to {}",
                    28 + x,
                    52 + y,
                    hex(got),
                    hex(want.color(x, y))
                ),
            )?;
        }
    }
    Ok(())
}

fn widgets_menu_dismiss() -> CaseResult {
    let (mut m, theme) = open_menu();
    let mut got = Vec::new();
    for i in [hover(250, 130), press(250, 130), release(250, 130)] {
        let e = pointer_event("menu", i, |i| m.update(i, &theme));
        if e != MenuEvent::None {
            got.push(e);
        }
    }
    ensure(got.is_empty() && !m.is_open(), "14.3", format!(
        "a click outside the open menu reported {got:?} and left it {}; the contract closes it without choosing", if m.is_open() { "open" } else { "closed" }))
}

fn widgets_layout_stack() -> CaseResult {
    let c = Rect::new(10, 20, 500, 30);
    let got = libbui::layout::h_stack(&c, 40, 4, 6);
    let want: Vec<Rect> = (0..4).map(|i| Rect::new(10 + i * 46, 20, 40, 30)).collect();
    ensure(
        got == want,
        "14.3",
        format!("h_stack gave {got:?}; the contract gives {want:?}"),
    )?;
    let c = Rect::new(3, 7, 90, 400);
    let got = libbui::layout::v_stack(&c, 25, 3, 2);
    let want: Vec<Rect> = (0..3).map(|i| Rect::new(3, 7 + i * 27, 90, 25)).collect();
    ensure(
        got == want,
        "14.3",
        format!("v_stack gave {got:?}; the contract gives {want:?}"),
    )
}

// gpu-text: R3 adds certified DejaVu label pixels to the unchanged five-widget focus ring checks.
fn widgets_focus_ring() -> CaseResult {
    let rect = Rect::new(20, 20, 100, 24);
    let theme = Theme::dark();
    ensure(
        theme.focus_ring == Color::rgb(255, 200, 60),
        "14.1",
        "focus ring colour differs from literal theme",
    )?;
    let mut b = Button::new(rect, b"OK");
    let mut c = Checkbox::new(rect, b"Opt");
    let mut slider = Slider::new(rect, 0.0, 1.0);
    let mut tabs = TabBar::new(rect, vec![b"One", b"Two"], &Theme::dark());
    let mut field = TextField::new(rect, 30);
    let mut g = Guarded::bgrx(150, 70);
    for focused in [false, true] {
        b.set_focused(focused);
        c.set_focused(focused);
        slider.set_focused(focused);
        tabs.set_focused(focused);
        field.set_focused(focused);
        for kind in 0..5 {
            g.reset(PAPER);
            match kind {
                0 => b.draw(&mut g.fb(), &theme),
                1 => c.draw(&mut g.fb(), &theme),
                2 => slider.draw(&mut g.fb(), &theme),
                3 => tabs.draw(&mut g.fb(), &theme),
                _ => field.draw(&mut g.fb(), &theme),
            };
    snapshot(&g)?;
            for y in 18..46 {
                for x in 18..122 {
                    if x == 18 || x == 121 || y == 18 || y == 45 {
                        let want = if focused { theme.focus_ring } else { PAPER };
                        ensure(
                            g.color(x, y) == want,
                            "14.3",
                            format!("widget {kind} focused={focused} ring differs at {x},{y}"),
                        )?;
                    }
                }
            }
        }
    }
    g.reset(PAPER);b.set_focused(true);b.draw(&mut g.fb(),&theme);
    same_as(&g,PAPER,"focused TrueType button","14.3",|fb|{
        shapes::fill_rect(fb,20,20,100,24,theme.widget_bg);shapes::draw_rect(fb,20,20,100,24,theme.border);
        sans_center(fb,b"OK",rect,theme.text_primary);shapes::draw_rect(fb,18,18,104,28,theme.focus_ring);
    })?;
    snapshot(&g)
}

fn widgets_no_change_ack() -> CaseResult {
    let mut b = Button::new(Rect::new(10, 10, 80, 24), b"OK");
    pointer_event("button OK", hover(0, 0), |i| b.update(i));
    ensure(
        !b.last_update_redraw(),
        "14.2",
        "unchanged outside pointer requested redraw",
    )?;
    pointer_event("button OK", hover(20, 20), |i| b.update(i));
    ensure(
        b.last_update_redraw(),
        "14.2",
        "entering hover did not request redraw",
    )?;
    pointer_event("button OK", hover(21, 21), |i| b.update(i));
    ensure(
        !b.last_update_redraw(),
        "14.2",
        "moving within hover requested redraw",
    )?;
    pointer_event("button OK", press(21, 21), |i| b.update(i));
    ensure(
        b.last_update_redraw(),
        "14.2",
        "press did not request redraw",
    )?;
    pointer_event("button OK", hold(22, 22), |i| b.update(i));
    ensure(
        !b.last_update_redraw(),
        "14.2",
        "unchanged held state requested redraw",
    )?;
    let mut c = Checkbox::new(Rect::new(10, 10, 80, 24), b"Opt");
    c.set_focused(true);
    key_event("checkbox", KeyInput::new(Key::Left), |i| c.key(i));
    ensure(
        !c.last_update_redraw(),
        "14.2",
        "irrelevant checkbox key requested redraw",
    )?;
    let mut f = entry_field(10);
    key_event("text field", KeyInput::new(Key::Left), |i| f.key(i));
    ensure(
        !f.last_update_redraw(),
        "14.2",
        "cursor clamped at start requested redraw",
    )?;
    let mut bar = ScrollBar::new(Rect::new(0, 0, 12, 200), 1000, 200);
    pointer_event("scrollbar", hover(50, 50), |i| bar.update(i));
    ensure(
        !bar.last_update_redraw(),
        "14.2",
        "unchanged scrollbar pointer requested redraw",
    )?;
    pointer_event("scrollbar", press(6, 10), |i| bar.update(i));
    ensure(
        bar.last_update_redraw(),
        "14.2",
        "scrollbar drag colour did not request redraw",
    )?;
    pointer_event("scrollbar", hold(6, 10), |i| bar.update(i));
    ensure(
        !bar.last_update_redraw(),
        "14.2",
        "stationary thumb drag requested redraw",
    )?;
    let mut picker = picker_fixture(5);
    pointer_event("picker", hover(500, 500), |i| picker.update(i));
    ensure(
        !picker.last_update_redraw(),
        "14.2",
        "unchanged picker requested redraw",
    )?;
    pointer_event("picker", press(50, 30), |i| picker.update(i));
    ensure(
        picker.last_update_redraw(),
        "14.2",
        "picker selection did not request redraw",
    )?;
    pointer_event("picker", release(50, 30), |i| picker.update(i));
    ensure(
        !picker.last_update_redraw(),
        "14.2",
        "picker release without drawing change requested redraw",
    )?;
    let (mut menu, theme) = open_menu();
    pointer_event("menu", hover(500, 500), |i| menu.update(i, &theme));
    pointer_event("menu", hover(501, 501), |i| menu.update(i, &theme));
    ensure(
        !menu.last_update_redraw(),
        "14.2",
        "unchanged menu requested redraw",
    )?;
    let mut tabs = TabBar::new(Rect::new(10, 10, 200, 24), vec![b"One", b"Two"], &Theme::dark());
    pointer_event("tabs", hover(20, 20), |i| tabs.update(i));
    pointer_event("tabs", hover(21, 20), |i| tabs.update(i));
    ensure(
        !tabs.last_update_redraw(),
        "14.2",
        "pointer motion inside one tab requested redraw",
    )
}

// ---------------------------------------------------------------------------------------
// text-entry: text fields, focus & file picker (contract section 15)
// ---------------------------------------------------------------------------------------

fn text_entry_insert() -> CaseResult {
    let mut f = entry_field(40);
    key_event("text field", KeyInput::text("abcd"), |i| f.key(i));
    key_event("text field", KeyInput::new(Key::Left), |i| f.key(i));
    key_event("text field", KeyInput::text("XY"), |i| f.key(i));
    ensure(
        f.text() == "abcXYd" && f.cursor() == 5,
        "15",
        format!("insertion produced {:?} cursor {}", f.text(), f.cursor()),
    )
}
fn text_entry_delete() -> CaseResult {
    let mut f = entry_field(40);
    f.insert_str("abcde");
    key_event("text field", KeyInput::new(Key::Left), |i| f.key(i));
    key_event("text field", KeyInput::new(Key::Backspace), |i| f.key(i));
    ensure(
        f.text() == "abce" && f.cursor() == 3,
        "15",
        "Backspace did not remove d as one scalar",
    )?;
    key_event("text field", KeyInput::new(Key::Delete), |i| f.key(i));
    ensure(
        f.text() == "abc" && f.cursor() == 3,
        "15",
        "Delete did not remove e",
    )?;
    key_event("text field", KeyInput::new(Key::End), |i| f.key(i));
    key_event("text field", KeyInput::new(Key::Delete), |i| f.key(i));
    key_event("text field", KeyInput::new(Key::Home), |i| f.key(i));
    key_event("text field", KeyInput::new(Key::Backspace), |i| f.key(i));
    ensure(
        f.text() == "abc" && f.cursor() == 0,
        "15",
        "delete at ends changed the string",
    )
}
fn text_entry_move() -> CaseResult {
    let mut f = entry_field(40);
    f.insert_str("aé中🦀");
    for (key, want) in [
        (Key::Home, 0),
        (Key::Left, 0),
        (Key::Right, 1),
        (Key::Right, 2),
        (Key::End, 4),
        (Key::Right, 4),
        (Key::Left, 3),
    ] {
        key_event("text field", KeyInput::new(key), |i| f.key(i));
        ensure(
            f.cursor() == want,
            "15",
            format!("{key:?} cursor {} expected {want}", f.cursor()),
        )?;
    }
    Ok(())
}
fn text_entry_select() -> CaseResult {
    let mut f = entry_field(40);
    f.insert_str("abcdef");
    key_event("text field", shift_key(Key::Left), |i| f.key(i));
    key_event("text field", shift_key(Key::Left), |i| f.key(i));
    ensure(
        f.selection() == Some((4, 6)),
        "15",
        "Shift+Left did not select ef",
    )?;
    key_event("text field", KeyInput::text("Q"), |i| f.key(i));
    ensure(
        f.text() == "abcdQ" && f.cursor() == 5 && f.selection().is_none(),
        "15",
        "typing did not replace ef and collapse selection",
    )
}
fn text_entry_utf8() -> CaseResult {
    let mut f = entry_field(40);
    f.insert_str("aé中🦀");
    ensure(f.cursor() == 4, "15", "cursor counted UTF-8 bytes")?;
    key_event("text field", KeyInput::new(Key::Backspace), |i| f.key(i));
    key_event("text field", KeyInput::new(Key::Left), |i| f.key(i));
    key_event("text field", KeyInput::new(Key::Delete), |i| f.key(i));
    ensure(
        f.text() == "aé" && f.cursor() == 2,
        "15",
        format!(
            "Unicode scalar deletion produced {:?}, cursor {}",
            f.text(),
            f.cursor()
        ),
    )
}
fn text_entry_maxlen() -> CaseResult {
    let mut f = entry_field(4);
    ensure(
        f.insert_str("é中🦀"),
        "15",
        "valid three-scalar paste refused",
    )?;
    ensure(
        !f.insert_str("xy") && f.text() == "é中🦀" && f.cursor() == 3,
        "15",
        "overflowing paste was partly inserted",
    )?;
    key_event("text field", shift_key(Key::Left), |i| f.key(i));
    ensure(
        !f.insert_str("abc") && f.selected_text() == "🦀",
        "15",
        "refused replacement changed selection",
    )?;
    ensure(
        f.insert_str("ab") && f.text() == "é中ab" && f.cursor() == 4,
        "15",
        "replacement did not count removed selection",
    )
}
// gpu-text: R3 derives visible cursor position from DejaVu Sans advances, preserving Home and clipping checks.
fn text_entry_scroll_to_cursor() -> CaseResult {
    let mut f = entry_field(100);
    f.insert_str("abcdefghijklmnopqrstuvwxyz");
    let visible:String=f.text().chars().enumerate().filter_map(|(i,c)| (i>=f.scroll_offset()).then_some(c)).collect();
    ensure(f.cursor_x()==14+sans_width(visible.as_bytes()),"15",format!("gpu-text: caret x {} disagrees with the visible substring advance {}",f.cursor_x(),sans_width(visible.as_bytes())))?;
    ensure(
        f.scroll_offset() > 0 && f.cursor_x() >= 14 && f.cursor_x() < 70,
        "15",
        "end cursor not visible in 60px field",
    )?;
    key_event("text field", KeyInput::new(Key::Home), |i| f.key(i));
    ensure(
        f.scroll_offset() == 0 && f.cursor_x() == 14,
        "15",
        "Home did not reveal the first column",
    )?;
    let mut g = Guarded::bgrx(100, 50);
    g.reset(PAPER);
    f.draw(&mut g.fb(), &Theme::dark());
    snapshot(&g)?;
    ensure(g.color(10, 10) == Theme::dark().border
        && g.color(14, 16) == Color::WHITE
        && g.color(65, 12) == Theme::dark().widget_bg, "15", "focused field border or interior drawing missing")?;
    for y in 0..50 {
        for x in 0..100 {
            if !in_rect(x as i64, y as i64, 8, 8, 64, 24) {
                ensure(
                    g.color(x, y) == PAPER,
                    "15",
                    "scrolling field wrote outside bounds and focus ring",
                )?;
            }
        }
    }
    snapshot(&g)
}
fn text_entry_selection_text() -> CaseResult {
    let mut f = entry_field(40);
    f.insert_str("aé中🦀z");
    key_event("text field", KeyInput::new(Key::Left), |i| f.key(i));
    for _ in 0..3 {
        key_event("text field", shift_key(Key::Left), |i| f.key(i));
    }
    ensure(
        f.selection() == Some((1, 4)) && f.selected_text() == "é中🦀",
        "15",
        format!(
            "selected {:?}, range {:?}",
            f.selected_text(),
            f.selection()
        ),
    )
}
fn text_entry_paste_insert() -> CaseResult {
    let mut f = entry_field(40);
    f.insert_str("abcde");
    key_event("text field", KeyInput::new(Key::Left), |i| f.key(i));
    key_event("text field", shift_key(Key::Left), |i| f.key(i));
    key_event("text field", shift_key(Key::Left), |i| f.key(i));
    ensure(
        f.insert_str("é🦀中")
            && f.text() == "abé🦀中e"
            && f.cursor() == 5
            && f.selection().is_none(),
        "15",
        "paste did not replace cd with three scalars",
    )
}
fn text_entry_focus_tab() -> CaseResult {
    let mut f = focus_fixture();
    for want in [0, 1, 2, 0, 1] {
        ensure(
            key_event("focus manager", KeyInput::new(Key::Tab), |i| f.key(i)) == Some(want),
            "15",
            format!("Tab did not focus {want}"),
        )?;
    }
    Ok(())
}
fn text_entry_focus_shift_tab() -> CaseResult {
    let mut f = focus_fixture();
    for want in [2, 1, 0, 2] {
        ensure(
            key_event("focus manager", shift_key(Key::Tab), |i| f.key(i)) == Some(want),
            "15",
            format!("Shift+Tab did not focus {want}"),
        )?;
    }
    Ok(())
}
fn text_entry_focus_skip_disabled() -> CaseResult {
    let mut f = focus_fixture();
    f.set_enabled(1, false);
    for want in [0, 2, 0, 2] {
        ensure(
            key_event("focus manager", KeyInput::new(Key::Tab), |i| f.key(i)) == Some(want),
            "15",
            format!("Tab did not skip disabled 1 to {want}"),
        )?;
    }
    f.set_enabled(0, false);
    f.set_enabled(2, false);
    ensure(
        key_event("focus manager", KeyInput::new(Key::Tab), |i| f.key(i)).is_none(),
        "15",
        "all-disabled chain retained a focus",
    )
}
fn text_entry_focus_click() -> CaseResult {
    let mut f = focus_fixture();
    ensure(
        pointer_event("focus manager", press(65, 15), |i| f.update(i)) == Some(2),
        "15",
        "press did not focus third widget",
    )?;
    f.set_enabled(1, false);
    ensure(
        pointer_event("focus manager", press(35, 15), |i| f.update(i)) == Some(2),
        "15",
        "disabled widget stole focus",
    )?;
    ensure(
        pointer_event("focus manager", release(5, 15), |i| f.update(i)) == Some(2),
        "15",
        "release moved focus without a press",
    )
}

fn text_entry_filepicker_list() -> CaseResult {
    let theme = Theme::dark();
    let entry = |name: &[u8], is_dir: bool| FileEntry {
        name: name.to_vec(),
        is_dir,
        size: 10,
    };
    let mut p = FilePicker::new(
        Rect::new(0, 0, 300, 240),
        b"/data".to_vec(),
        vec![
            entry(b"zeta", false),
            entry(b"Beta", true),
            entry(b"alpha", false),
            entry(b"Alpha", true),
            entry(b"beta", false),
        ],
        &theme,
    );
    // The list area is (1, 25, 298, 182) with 11-pixel rows.
    for (row, want) in ["Alpha", "Beta", "alpha", "beta", "zeta"]
        .iter()
        .enumerate()
    {
        let y = 25 + 11 * row as i32 + 5;
        pointer_event("file picker", press(50, y), |i| p.update(i));
        pointer_event("file picker", release(50, y), |i| p.update(i));
        let got = p
            .selected_entry()
            .map(|e| String::from_utf8_lossy(&e.name).into_owned());
        ensure(got.as_deref() == Some(*want), "15", format!("row {row} lists {got:?}; directories first, then files, each in byte order, give {want:?}"))?;
    }
    Ok(())
}

fn text_entry_filepicker_keys() -> CaseResult {
    let mut p = picker_fixture(5);
    key_event("file picker", KeyInput::new(Key::Down), |i| p.key(i));
    ensure(
        p.selected_entry()
            .is_some_and(|e| e.is_dir && e.name == b"Dir"),
        "15",
        "Down did not select first directory",
    )?;
    ensure(
        key_event("file picker", KeyInput::new(Key::Enter), |i| p.key(i)) == FilePickerResult::NavigateDir(0),
        "15",
        "Enter did not open directory",
    )?;
    key_event("file picker", KeyInput::new(Key::Down), |i| p.key(i));
    ensure(
        key_event("file picker", KeyInput::new(Key::Enter), |i| p.key(i)) == FilePickerResult::Selected(1),
        "15",
        "Enter did not open first file",
    )?;
    key_event("file picker", KeyInput::new(Key::Up), |i| p.key(i));
    ensure(
        p.selected_entry().is_some_and(|e| e.name == b"Dir"),
        "15",
        "Up did not return to directory",
    )
}

fn text_entry_filepicker_path() -> CaseResult {
    let mut p = picker_fixture(5);
    key_event("file picker", KeyInput::new(Key::Tab), |i| p.key(i));
    key_event("file picker", KeyInput::new(Key::Home), |i| p.key(i));
    key_event("file picker", shift_key(Key::End), |i| p.key(i));
    key_event("file picker", KeyInput::text("/new/é"), |i| p.key(i));
    ensure(
        key_event("file picker", KeyInput::new(Key::Enter), |i| p.key(i))
            == FilePickerResult::NavigatePath("/new/é".as_bytes().to_vec()),
        "15",
        "typed path did not emit exact navigation request",
    )
}

fn text_entry_filepicker_mouse() -> CaseResult {
    let mut p = picker_fixture(5);
    for (time, x, want) in [
        (1000, 50, FilePickerResult::Active),
        (1401, 50, FilePickerResult::Active),
        (1801, 55, FilePickerResult::Active),
        (2201, 59, FilePickerResult::NavigateDir(0)),
    ] {
        let got = pointer_event("file picker", press(x, 30).with_time(time), |i| p.update(i));
        ensure(
            got == want,
            "15",
            format!("press at {time}ms x={x} gave {got:?}, expected {want:?}"),
        )?;
        pointer_event("file picker", release(x, 30).with_time(time), |i| p.update(i));
    }
    Ok(())
}

fn text_entry_filepicker_scroll() -> CaseResult {
    let mut p = picker_fixture(60);
    key_event("file picker", KeyInput::new(Key::End), |i| p.key(i));
    ensure(
        p.selected_entry().is_some_and(|e| e.name == b"file059") && p.scroll_offset() > 0,
        "15",
        "End did not reveal final row",
    )?;
    key_event("file picker", KeyInput::new(Key::Home), |i| p.key(i));
    ensure(p.scroll_offset() == 0, "15", "Home did not scroll to start")?;
    p.scroll(-1);
    ensure(
        p.scroll_offset() == 3,
        "15",
        "wheel did not scroll three rows",
    )?;
    p.scroll(1);
    ensure(p.scroll_offset() == 0, "15", "wheel up did not scroll back")?;
    key_event("file picker", KeyInput::new(Key::PageDown), |i| p.key(i));
    ensure(
        p.scroll_offset() > 0,
        "15",
        "PageDown did not reveal selected row beyond first page",
    )?;
    key_event("file picker", KeyInput::new(Key::PageUp), |i| p.key(i));
    ensure(
        p.scroll_offset() == 0,
        "15",
        "PageUp did not reveal top row",
    )
}

fn text_entry_filepicker_errors() -> CaseResult {
    let mut p = picker_fixture(5);
    p.show_error(b"Permission denied");
    ensure(
        p.error() == Some(&b"Permission denied"[..]) && p.selected_entry().is_none(),
        "15",
        "picker did not retain error and remove entries",
    )?;
    let mut g = Guarded::bgrx(320, 250);
    g.reset(PAPER);
    p.draw(&mut g.fb(), &Theme::dark());
    snapshot(&g)?;
    let mut expected = Guarded::bgrx(101, 7);
    expected.reset(Theme::dark().panel_bg);
    font5x7::draw_text(
        &mut expected.fb(),
        b"Permission denied",
        0,
        0,
        Color::WHITE,
        1,
    );
    for y in 0..7 {
        for x in 0..101 {
            ensure(
                g.color(x + 5, y + 29) == expected.color(x, y),
                "15",
                format!("error text differs at {x},{y}"),
            )?;
        }
    }
    ensure(
        key_event("file picker", KeyInput::new(Key::Enter), |i| p.key(i)) == FilePickerResult::Active,
        "15",
        "Enter activated a stale entry under error",
    )?;
    snapshot(&g)
}

// ---------------------------------------------------------------------------------------
// icons: animated icons (contract section 16)
// ---------------------------------------------------------------------------------------

fn mouse(hovering: bool, pressed: bool, clicked: bool, released: bool) -> IconMouse {
    IconMouse {
        hovering,
        pressed,
        just_clicked: clicked,
        just_released: released,
        rel_x: 0.2,
        rel_y: -0.1,
    }
}

fn icons_states() -> CaseResult {
    let mut b = IconBase::new();
    let steps: [(u32, IconMouse, IconState); 9] = [
        (1_000, mouse(true, false, false, false), IconState::HoverIn),
        (
            249_000,
            mouse(true, false, false, false),
            IconState::HoverIn,
        ),
        (1_000, mouse(true, false, false, false), IconState::Hovering),
        (1_000, mouse(true, true, true, false), IconState::Pressed),
        (50_000, mouse(true, true, false, false), IconState::Pressed),
        (1_000, mouse(true, false, false, true), IconState::Clicked),
        (
            500_000,
            mouse(true, false, false, false),
            IconState::Hovering,
        ),
        (
            1_000,
            mouse(false, false, false, false),
            IconState::HoverOut,
        ),
        (200_000, mouse(false, false, false, false), IconState::Idle),
    ];
    for (n, (dt, m, want)) in steps.into_iter().enumerate() {
        b.update(dt, &m);
        ensure(
            b.state == want,
            "16",
            format!(
                "step {n} ({dt} us): the state is {:?}; the contract's table gives {want:?}",
                b.state
            ),
        )?;
    }
    Ok(())
}

fn icons_spring_steps() -> CaseResult {
    let mut s = Spring::new(0.0, 300.0, 20.0);
    s.set_target(1.0);
    let mut table = SPRING_TABLE.iter().peekable();
    for ms in 1..=1000u32 {
        s.update(0.001);
        if let Some(&&(at_ms, value, velocity)) = table.peek() {
            if at_ms == ms {
                table.next();
                ensure((s.value as f64 - value).abs() <= 1e-4 && (s.velocity as f64 - velocity).abs() <= 1e-4, "16", format!(
                    "after {ms} ms the spring is at {} moving {}; the contract's table gives {value} and {velocity}", s.value, s.velocity))?;
            }
        }
    }
    Ok(())
}

fn catalogue() -> Vec<(&'static str, fn() -> Box<dyn Icon>)> {
    vec![
        ("home", || Box::new(HomeIcon::new())),
        ("search", || Box::new(SearchIcon::new())),
        ("save", || Box::new(SaveIcon::new())),
        ("arrow", || Box::new(ArrowIcon::new(true))),
    ]
}

fn drawn(icon: &dyn Icon, size: i32) -> Guarded {
    let mut g = Guarded::bgrx(256, 256);
    g.reset(PAPER);
    icon.draw(&mut g.fb(), 128, 128, size);
    g
}

fn same_pixels(a: &Guarded, b: &Guarded) -> Option<(usize, usize)> {
    (0..a.height)
        .flat_map(|y| (0..a.width).map(move |x| (x, y)))
        .find(|&(x, y)| a.color(x, y) != b.color(x, y))
}

fn icons_step_invariance() -> CaseResult {
    for (name, make) in catalogue() {
        let schedules: [(&str, &[(u32, u32)]); 3] = [
            ("one 1,000 ms step", &[(1, 1_000_000)]),
            ("1,000 steps of 1 ms", &[(1_000, 1_000)]),
            (
                "62 steps of 16 ms and one of 8 ms",
                &[(62, 16_000), (1, 8_000)],
            ),
        ];
        let mut results = Vec::new();
        for (label, schedule) in schedules {
            let mut icon = make();
            for &(n, dt) in schedule {
                for _ in 0..n {
                    icon.update(dt, mouse(true, false, false, false));
                }
            }
            let frame = drawn(icon.as_ref(), 48);
            snapshot(&frame)?;
            results.push((label, icon.state(), frame));
        }
        snapshot(&results[0].2)?;
        ensure(
            drawn_pixels(&results[0].2, PAPER) > 0,
            "16",
            format!("{name} drew nothing after {}", results[0].0),
        )?;
        for (label, state, g) in &results[1..] {
            snapshot(&g)?;
            ensure(
                *state == results[0].1,
                "16",
                format!(
                    "{name}: after {label} the state is {state:?}, after {} it is {:?}",
                    results[0].0, results[0].1
                ),
            )?;
            if let Some((x, y)) = same_pixels(g, &results[0].2) {
                return at("16", format!("{name}: after {label} pixel ({x},{y}) differs from the drawing after {}; the contract's 1 ms quanta make them equal", results[0].0));
            }
        }
    }
    Ok(())
}

fn icons_settles() -> CaseResult {
    for hovering in [false, true] {
        for (name, make) in catalogue() {
            let mut icon = make();
            icon.update(251_000, mouse(true, false, false, false));
            icon.update(1_000, mouse(true, true, true, false));
            icon.update(50_000, mouse(true, true, false, false));
            icon.update(1_000, mouse(true, false, false, true));
            icon.update(120_000, mouse(true, false, false, false));
            icon.update(2_000_000, mouse(hovering, false, false, false));
            let frame = drawn(&*icon, 48);
            snapshot(&frame)?;
            ensure(
                icon.settled(),
                "16",
                format!("{name} did not settle within 2000ms of final input"),
            )?;
            icon.update(100_000, mouse(hovering, false, false, false));
            let later = drawn(&*icon, 48);
            snapshot(&later)?;
            ensure(
                same_pixels(&frame, &later).is_none(),
                "16",
                format!("{name} kept drawing animation after reporting settled"),
            )?;
            snapshot(&frame)?;
            let half = 24 + icon.bounds_overflow();
            for y in 0..256 {
                for x in 0..256 {
                    if !in_rect(
                        x as i64,
                        y as i64,
                        (128 - half) as i64,
                        (128 - half) as i64,
                        (2 * half) as i64,
                        (2 * half) as i64,
                    ) {
                        if frame.color(x, y) != PAPER {
                            return at("16", format!("settled {name} wrote outside its box at {x},{y}"));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn icons_interrupt() -> CaseResult {
    let mut b = IconBase::new();
    b.update(1_000, &mouse(true, false, false, false));
    b.update(100_000, &mouse(true, false, false, false));
    ensure(
        b.state == IconState::HoverIn,
        "16",
        format!(
            "100 ms after entering the state is {:?}, not HoverIn",
            b.state
        ),
    )?;
    b.update(1_000, &mouse(true, true, true, false));
    ensure(
        b.state == IconState::Pressed,
        "16",
        format!(
            "a press during HoverIn gave {:?}; the table gives Pressed",
            b.state
        ),
    )?;
    let mut b = IconBase::new();
    b.update(1_000, &mouse(true, false, false, false));
    b.update(1_000, &mouse(false, false, false, false));
    b.update(50_000, &mouse(false, false, false, false));
    ensure(
        b.state == IconState::HoverOut,
        "16",
        format!(
            "leaving during HoverIn gave {:?}; the table gives HoverOut",
            b.state
        ),
    )?;
    b.update(1_000, &mouse(true, false, false, false));
    ensure(
        b.state == IconState::HoverIn,
        "16",
        format!(
            "entering again during HoverOut gave {:?}; the table gives HoverIn",
            b.state
        ),
    )
}

fn icons_reversal() -> CaseResult {
    for (name, make) in catalogue() {
        let mut icon = make();
        icon.update(101_000, mouse(true, false, false, false));
        let previous = icon.hover_amount();
        ensure(
            previous > 0.0 && previous < 1.0,
            "16",
            format!("{name} has no intermediate hover position: {previous}"),
        )?;
        icon.update(0, IconMouse::none());
        ensure(
            icon.hover_amount() == previous,
            "16",
            format!("{name} jumped on zero-time reversal"),
        )?;
        icon.update(1_000, IconMouse::none());
        let after = icon.hover_amount();
        ensure(
            after >= 0.0 && after < previous && previous - after <= 0.0051,
            "16",
            format!("{name} reversal jumped from {previous} to {after}"),
        )?;
        icon.update(50_000, IconMouse::none());
        let before = icon.hover_amount();
        icon.update(0, mouse(true, false, false, false));
        ensure(
            icon.hover_amount() == before,
            "16",
            format!("{name} jumped on second reversal"),
        )?;
    }
    Ok(())
}

fn icons_reset() -> CaseResult {
    for (name, make) in catalogue() {
        let mut icon = make();
        for (dt, m) in [
            (16_000, mouse(true, false, false, false)),
            (300_000, mouse(true, false, false, false)),
            (16_000, mouse(true, true, true, false)),
            (16_000, mouse(true, false, false, true)),
            (120_000, mouse(true, false, false, false)),
        ] {
            icon.update(dt, m);
        }
        icon.reset();
        let fresh = make();
        let fresh_drawing = drawn(fresh.as_ref(), 56);
        snapshot(&fresh_drawing)?;
        ensure(
            drawn_pixels(&fresh_drawing, PAPER) > 0,
            "16",
            format!("a new {name} icon drew nothing at size 56"),
        )?;
        let actual = drawn(icon.as_ref(), 56);
        snapshot(&actual)?;
        if let Some((x, y)) = same_pixels(&actual, &fresh_drawing) {
            return at("16", format!("{name}: after reset pixel ({x},{y}) differs from a new icon drawn at the new size"));
        }
    }
    Ok(())
}

fn icons_bounded() -> CaseResult {
    for (name, make) in catalogue() {
        step(
            "16",
            isolated(
                &format!("{name} under 10,000 steps of seeded input"),
                6_000,
                || {
                    let mut icon = make();
                    let size = 32;
                    let mut g = Guarded::bgrx(256, 256);
                    g.reset(PAPER);
                    let mut rng = Seeded(0x1C0_5EED ^ name.len() as u64);
                    let mut held = false;
                    for _ in 0..10_000 {
                        let hovering = rng.next() % 4 != 0;
                        let press = !held && rng.next() % 9 == 0;
                        let release = held && rng.next() % 5 == 0;
                        held = (held || press) && !release;
                        icon.update(
                            1_000 + (rng.next() % 32_000) as u32,
                            IconMouse {
                                hovering,
                                pressed: held,
                                just_clicked: press,
                                just_released: release,
                                rel_x: (rng.next() % 200) as f32 / 100.0 - 1.0,
                                rel_y: (rng.next() % 200) as f32 / 100.0 - 1.0,
                            },
                        );
                        icon.draw(&mut g.fb(), 128, 128, size);
                    }
                    snapshot(&g).map_err(|e| format!("judged icon: {e:?}"))?;
                    if drawn_pixels(&g, PAPER) == 0 {
                        return Err(format!("{name} drew nothing in 10,000 steps"));
                    }
                    let half = size / 2 + icon.bounds_overflow();
                    for y in 0..256i64 {
                        for x in 0..256i64 {
                            if !in_rect(
                                x,
                                y,
                                128 - half as i64,
                                128 - half as i64,
                                2 * half as i64,
                                2 * half as i64,
                            ) && g.color(x as usize, y as usize) != PAPER
                            {
                                return Err(format!("{name} drew pixel ({x},{y}), outside its box of half-width {half} around (128,128)"));
                            }
                        }
                    }
                    g.sentinels()
                },
            ),
        )?;
    }
    Ok(())
}

fn icons_easing() -> CaseResult {
    use libicon::easing;
    let functions: [(&str, fn(f32) -> f32, f64); 7] = [
        ("linear", easing::linear, 1e-5),
        ("ease_out_cubic", easing::ease_out_cubic, 1e-5),
        ("ease_in_cubic", easing::ease_in_cubic, 1e-5),
        ("ease_in_out_cubic", easing::ease_in_out_cubic, 1e-5),
        ("ease_out_elastic", easing::ease_out_elastic, 0.002),
        ("ease_out_bounce", easing::ease_out_bounce, 1e-5),
        ("ease_out_back", easing::ease_out_back, 1e-5),
    ];
    for (name, f, tolerance) in functions {
        let row = EASING_TABLE
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, r)| r)
            .ok_or_else(|| format!("no table row for {name}"))
            .cite("16")?;
        for (i, &want) in row.iter().enumerate() {
            let t = i as f32 / 10.0;
            let got = f(t) as f64;
            ensure(
                (got - want).abs() <= tolerance,
                "16",
                format!(
                    "{name}({t}) is {got:.6}; the contract's table gives {want} within {tolerance}"
                ),
            )?;
        }
    }
    Ok(())
}

/// sin by its Taylor series in f64, for |x| ≤ π (independent of libicon and the C library).
fn taylor_sin(x: f64) -> f64 {
    let (mut term, mut sum) = (x, x);
    for n in 1..30 {
        term *= -x * x / ((2 * n) as f64 * (2 * n + 1) as f64);
        sum += term;
    }
    sum
}

fn icons_sin_approx() -> CaseResult {
    let pi = std::f64::consts::PI;
    let mut worst = (0.0f64, 0.0f64);
    for i in 0..4096 {
        let x = (-pi + 2.0 * pi * i as f64 / 4095.0) as f32;
        let err = (libicon::easing::sin_approx(x) as f64 - taylor_sin(x as f64)).abs();
        if err > worst.0 {
            worst = (err, x as f64);
        }
    }
    value(
        "worst-error-ppm",
        (worst.0 * 1e6) as i64,
        "",
        Some((0, 1200)),
    );
    ensure(
        worst.0 <= 0.0012,
        "16",
        format!(
            "sin_approx is {:.6} from sin at x = {:.5}; the bound is 0.0012",
            worst.0, worst.1
        ),
    )
}

fn icons_particles_bounded() -> CaseResult {
    let mut pool = ParticlePool::with_bounds(8, -12.0, -10.0, 12.0, 10.0);
    for i in 0..20 {
        pool.emit(Particle {
            x: 0.0,
            y: 0.0,
            vx: 100.0 + i as f32,
            vy: -120.0,
            life: 1.0,
            max_life: 1.0,
            size: 2.0,
            color: Color::WHITE,
            gravity: 1000.0,
            friction: 0.1,
        });
        ensure(
            pool.active_count() <= 8,
            "16",
            "particle count exceeded capacity",
        )?;
    }
    let mut g = Guarded::bgrx(80, 80);
    g.reset(PAPER);
    for _ in 0..1000 {
        pool.update(0.001);
        for p in pool.particles() {
            if p.alive() {
                ensure(
                    p.x >= -11.0 && p.x <= 11.0 && p.y >= -9.0 && p.y <= 9.0,
                    "16",
                    format!("particle escaped bounds at {},{}", p.x, p.y),
                )?;
            }
        }
        pool.draw(&mut g.fb(), 40, 40);
    }
    snapshot(&g)?;
    ensure(
        drawn_pixels(&g, PAPER) > 0,
        "16",
        "bounded pool drew nothing",
    )?;
    for y in 0..80 {
        for x in 0..80 {
            if !in_rect(x as i64, y as i64, 28, 30, 24, 20) {
                ensure(
                    g.color(x, y) == PAPER,
                    "16",
                    format!("particle footprint escaped at {x},{y}"),
                )?;
            }
        }
    }
    ensure(
        pool.active_count() == 0,
        "16",
        "expired particles remained live",
    )?;
    for edge in [0.0, -0.25, -3.0] {
        let mut negative = ParticlePool::with_bounds(1, edge - 4.0, edge - 4.0, edge, edge);
        negative.emit(Particle { x: edge, y: edge, vx: 0.0, vy: 0.0, life: 1.0,
            max_life: 1.0, size: 1.0, color: Color::WHITE, gravity: 0.0, friction: 0.0 });
        let mut frame = Guarded::bgrx(20, 20);
        frame.reset(PAPER);
        negative.draw(&mut frame.fb(), 10, 10);
        snapshot(&frame)?;
        ensure(drawn_pixels(&frame, PAPER) > 0, "16", "negative bounded pool drew nothing")?;
        for y in 0..20 { for x in 0..20 {
            if x as f32 >= edge + 10.0 || y as f32 >= edge + 10.0
                || (x as f32) < edge + 6.0 || (y as f32) < edge + 6.0 {
                ensure(frame.color(x,y) == PAPER, "16", "negative-edge particle escaped box")?;
            }
        }}
    }
    snapshot(&g)
}

fn icons_assets() -> CaseResult {
    let path = "/usr/local/test/share/breengel/frames/icons/assets.png";
    let bytes = std::fs::read(path)
        .map_err(|e| format!("reviewed icon reference is absent: {e}"))
        .cite("8.6,16")?;
    let reference = libimg::png::decode(&bytes)
        .map_err(|e| format!("reviewed icon PNG cannot be decoded: {e:?}"))
        .cite("8.6,16")?;
    ensure(
        reference.width == 640 && reference.height == 512,
        "8.6,16",
        "reviewed icon scene is not 640x512",
    )?;
    let mut g = Guarded::bgrx(640, 512);
    g.reset(Color::rgb(12, 19, 31));
    // Drive the contract's schedules here, independently of the authoring tool.
    let schedules: &[&[(u32, bool, bool)]] = &[
        &[], &[(101_000, false, false)], &[(251_000, false, false)],
        &[(251_000, false, false), (1_000, true, true), (50_000, true, false)],
        &[(251_000, false, false), (1_000, true, true), (50_000, true, false),
          (1_000, false, true), (120_000, false, false)],
    ];
    for (row, (_, make)) in catalogue().into_iter().enumerate() {
        for (col, schedule) in schedules.iter().enumerate() {
            let mut icon = make();
            for &(dt, pressed, edge) in *schedule {
                icon.update(dt, IconMouse { hovering: true, pressed,
                    just_clicked: edge && pressed, just_released: edge && !pressed,
                    rel_x: 0.0, rel_y: 0.0 });
            }
            icon.draw(&mut g.fb(), 64 + col as i32 * 128, 64 + row as i32 * 128, 48);
        }
    }
    snapshot(&g)?;
    for y in 0..512 {
        for x in 0..640 {
            let i = (y * 640 + x) * 4;
            let want = Color::rgb(
                reference.pixels[i + 2],
                reference.pixels[i + 1],
                reference.pixels[i],
            );
            if g.color(x, y) != want {
                return at("8.6,16", format!(
                    "catalogue differs from reviewed scene at {x},{y}: actual {} expected {}",
                    hex(g.color(x, y)), hex(want)));
            }
        }
    }
    snapshot(&g)
}
const EASING_TABLE: &[(&str, [f64; 11])] = &[
    (
        "linear",
        [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
    ),
    (
        "ease_out_cubic",
        [
            0.0, 0.271, 0.488, 0.657, 0.784, 0.875, 0.936, 0.973, 0.992, 0.999, 1.0,
        ],
    ),
    (
        "ease_in_cubic",
        [
            0.0, 0.001, 0.008, 0.027, 0.064, 0.125, 0.216, 0.343, 0.512, 0.729, 1.0,
        ],
    ),
    (
        "ease_in_out_cubic",
        [
            0.0, 0.004, 0.032, 0.108, 0.256, 0.5, 0.744, 0.892, 0.968, 0.996, 1.0,
        ],
    ),
    (
        "ease_out_elastic",
        [
            0.0, 1.25, 1.125, 0.875, 1.03125, 1.015625, 0.984375, 1.003906, 1.001953, 0.998047, 1.0,
        ],
    ),
    (
        "ease_out_bounce",
        [
            0.0, 0.075625, 0.3025, 0.680625, 0.91, 0.765625, 0.7725, 0.930625, 0.94, 0.988125, 1.0,
        ],
    ),
    (
        "ease_out_back",
        [
            0.0, 0.408828, 0.705802, 0.907132, 1.029028, 1.087697, 1.099352, 1.0802, 1.046451,
            1.014314, 1.0,
        ],
    ),
];
const SPRING_TABLE: &[(u32, f64, f64)] = &[
    (1, 0.0003, 0.3),
    (10, 0.015507, 2.730217),
    (20, 0.055123, 4.886276),
    (50, 0.266207, 8.397326),
    (100, 0.691898, 7.693301),
    (200, 1.098417, 0.854046),
    (500, 0.992322, 0.098756),
    (1000, 0.999973, 0.000876),
];

fn entry_field(max: usize) -> TextField {
    let mut f = TextField::new(Rect::new(10, 10, 60, 20), max);
    f.set_focused(true);
    f
}
fn shift_key(key: Key) -> KeyInput<'static> {
    KeyInput {
        shift: true,
        ..KeyInput::new(key)
    }
}
fn focus_fixture() -> FocusChain {
    let mut f = FocusChain::new();
    for i in 0..3 {
        f.add(Rect::new(i * 30, 10, 20, 20));
    }
    f
}
fn picker_fixture(files: usize) -> FilePicker {
    let mut entries = vec![FileEntry {
        name: b"Dir".to_vec(),
        is_dir: true,
        size: 0,
    }];
    for i in (0..files).rev() {
        entries.push(FileEntry {
            name: format!("file{i:03}").into_bytes(),
            is_dir: false,
            size: 10,
        });
    }
    FilePicker::new(
        Rect::new(0, 0, 300, 240),
        b"/data".to_vec(),
        entries,
        &Theme::dark(),
    )
}

struct FontFixture {
    root: std::path::PathBuf,
    old_root: Option<std::ffi::OsString>,
    old_clock: Option<std::ffi::OsString>,
}
impl FontFixture {
    fn new() -> Result<Self, String> {
        let root = std::path::PathBuf::from(format!(
            "/tmp/breengel-font-{}",
            process::getpid().map_err(|e| format!("getpid: {e}"))?.raw()
        ));
        std::fs::create_dir_all(root.join("etc"))
            .map_err(|e| format!("create font fixture: {e}"))?;
        std::fs::copy("/usr/share/fonts/DejaVuSansMono.ttf", root.join("mono.ttf"))
            .map_err(|e| format!("copy mono: {e}"))?;
        std::fs::copy("/usr/share/fonts/DejaVuSans.ttf", root.join("display.ttf"))
            .map_err(|e| format!("copy display: {e}"))?;
        let mut page = [0u8; 4096];
        page[..8].copy_from_slice(b"BGCLK001");
        page[24..32].copy_from_slice(&1_767_225_600_000_000_000u64.to_le_bytes());
        std::fs::write(root.join("clock"), page).map_err(|e| format!("write clock page: {e}"))?;
        let f = Self {
            root,
            old_root: std::env::var_os("BREENIX_CONFIG_ROOT"),
            old_clock: std::env::var_os("BREENIX_UI_CLOCK"),
        };
        f.config("mono.ttf", 16, "display.ttf", 24)?;
        std::env::set_var("BREENIX_CONFIG_ROOT", &f.root);
        std::env::set_var("BREENIX_UI_CLOCK", f.root.join("clock"));
        Ok(f)
    }
    fn path(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().into_owned()
    }
    fn config(&self, mono: &str, ms: u32, display: &str, ds: u32) -> Result<(), String> {
        let contents = format!(
            "mono.font={}\nmono.size={ms}\ndisplay.font={}\ndisplay.size={ds}\n",
            self.path(mono),
            self.path(display)
        );
        std::fs::write(self.root.join("etc/pending"), contents)
            .map_err(|e| format!("write fonts.conf: {e}"))?;
        std::fs::rename(
            self.root.join("etc/pending"),
            self.root.join("etc/fonts.conf"),
        )
        .map_err(|e| format!("replace fonts.conf: {e}"))
    }
    fn step(&self, ns: u64) -> Result<(), String> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(self.root.join("clock"))
            .map_err(|e| format!("open clock: {e}"))?;
        file.seek(SeekFrom::Start(8))
            .map_err(|e| format!("clock seek: {e}"))?;
        file.write_all(&(ns / 250_000_000).to_le_bytes())
            .map_err(|e| format!("write clock epoch: {e}"))?;
        file.write_all(&ns.to_le_bytes())
            .map_err(|e| format!("write UI time: {e}"))
    }
    fn assert_frame(&self, got: &Guarded, path: &str, size: f32) -> CaseResult {
        let bytes = std::fs::read(self.root.join(path))
            .map_err(|e| format!("read expected font: {e}"))
            .cite("9.3")?;
        let mut font = CachedFont::new(
            Font::parse(&bytes)
                .map_err(|e| format!("parse expected font: {e}"))
                .cite("9.3")?,
            256,
        );
        same_as(got, PAPER, "reloaded font pixels", "9.3", |fb| {
            ttf_font::draw_text(fb, &mut font, "Wi é A", 8, 40, size, Color::WHITE);
        })
    }
}
impl Drop for FontFixture {
    fn drop(&mut self) {
        if let Some(v) = &self.old_root {
            std::env::set_var("BREENIX_CONFIG_ROOT", v);
        } else {
            std::env::remove_var("BREENIX_CONFIG_ROOT");
        }
        if let Some(v) = &self.old_clock {
            std::env::set_var("BREENIX_UI_CLOCK", v);
        } else {
            std::env::remove_var("BREENIX_UI_CLOCK");
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn watch_frame(w: &mut FontWatcher, mono: bool) -> Guarded {
    let mut g = Guarded::bgrx(200, 70);
    g.reset(PAPER);
    let size = if mono {
        w.mono_size()
    } else {
        w.display_size()
    };
    let font = if mono {
        w.mono_font()
    } else {
        w.display_font()
    }
    .expect("loaded watcher font");
    ttf_font::draw_text(&mut g.fb(), font, "Wi é A", 8, 40, size, Color::WHITE);
    g
}

/// R3 lower-layer oracle: the pinned font and §8.3's already-certified 26.6 metrics.
fn sans_width(text:&[u8])->i32 {
    let bytes=std::fs::read("/usr/share/fonts/DejaVuSans.ttf").expect("gpu-text: pinned DejaVu Sans is missing");
    let font=Font::parse(&bytes).expect("gpu-text: pinned DejaVu Sans is invalid");
    let glyphs:Vec<_>=ttf_font::decode_utf8(text).map(|cp|font.glyph_for(cp)).collect();
    let pen:i32=glyphs.iter().enumerate().map(|(i,&g)|font.advance26(g,1024)+glyphs.get(i+1).map_or(0,|&next|font.kern26(g,next,1024))).sum();
    (pen+32)/64
}
fn sans_draw(fb:&mut FrameBuf,text:&[u8],x:i32,y:i32,colour:Color) {
    let bytes=std::fs::read("/usr/share/fonts/DejaVuSans.ttf").expect("gpu-text: pinned DejaVu Sans is missing");
    let mut font=CachedFont::new(Font::parse(&bytes).expect("gpu-text: pinned DejaVu Sans is invalid"),256);
    ttf_font::draw_text_bytes(fb,&mut font,text,x,y,16.0,colour);
}
fn sans_center(fb:&mut FrameBuf,text:&[u8],rect:Rect,colour:Color) {
    sans_draw(fb,text,rect.x+(rect.w-sans_width(text))/2,rect.y+(rect.h-16)/2,colour)
}

// claim-lint:ok: docs/gui/breengel.md specifies this assertion; the GPU lanes supply its implementation.
// gpu-text: R4 doubles logical geometry and uses DejaVu Sans at 32 px, never replicated 16 px pixels.
fn widgets_scale_2()->CaseResult {
    let modes=bgl::modes().map_err(|r|format!("gpu-display: MODES {}",outcome(r))).cite("14")?;
    ensure(modes.modes[modes.current as usize].scale==2,"14","gpu-display: widget case is not in scale-2 mode")?;
    let theme=Theme::dark();let mut g=Guarded::bgrx(400,160);g.reset(PAPER);
    Button::new(Rect::new(20,16,120,40),b"OK").draw(&mut g.fb(),&theme);
    let bytes=std::fs::read("/usr/share/fonts/DejaVuSans.ttf").map_err(|e|e.to_string())?;
    let mut font=CachedFont::new(Font::parse(&bytes).cite("14")?,256);
    let width=ttf_font::text_width(&mut font,"OK",32.0);
    same_as(&g,PAPER,"scale-2 TrueType button","14",|fb|{
        shapes::fill_rect(fb,40,32,240,80,theme.widget_bg);
        for inset in 0..2 {shapes::draw_rect(fb,40+inset,32+inset,240-2*inset,80-2*inset,theme.border);}
        ttf_font::draw_text(fb,&mut font,"OK",40+(240-width)/2,56,32.0,theme.text_primary);
    })
}
