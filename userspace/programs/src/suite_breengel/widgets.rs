//! widgets, text-entry, icons and the `text/fontwatch-*` cases (contract sections 9 and
//! 14 to 16).

use crate::*;

pub(crate) const FONTWATCH: &[Case] = &[
    case("fontwatch-reload", "Changing the configured font makes later text use newly rasterized glyphs of the new font", text_fontwatch_reload),
    case("fontwatch-size", "Changing the configured size changes the rasterized glyphs and their metrics", text_fontwatch_size),
    case("fontwatch-missing", "A configured font file that is missing keeps the previous font and reports the error", text_fontwatch_missing),
    case("fontwatch-malformed-config", "A malformed fonts.conf keeps the previous fonts and reports the error", text_fontwatch_malformed_config),
    case("fontwatch-partial", "A fonts.conf replaced by rename is seen whole, and a truncated one is rejected", text_fontwatch_partial),
    case("fontwatch-repeated", "Five changes in a row end with the last one in use", text_fontwatch_repeated),
    case("fontwatch-independent", "Mono and display fonts change independently", text_fontwatch_independent),
    case("fontwatch-idle", "A window that receives no events, with fonts.conf changed at UI time *t*, draws the new font in its first commit at or after the watcher's first 250 ms check after *t*", text_fontwatch_idle),
    case("fontwatch-file-replaced", "Replacing the configured font file at the same path by rename, with fonts.conf unchanged, makes later text use newly rasterized glyphs of the new file by the next check", text_fontwatch_file_replaced),
];

pub(crate) const WIDGETS: &[Case] = &[
    case("button-draw", "A button draws its label and frame in the theme's literal colours and positions", widgets_button_draw),
    case("button-click", "Press and release inside a button report exactly one click", widgets_button_click),
    case("button-cancel", "Press inside and release outside report no click", widgets_button_cancel),
    case("button-disabled", "A disabled button draws dimmed and reports no click", widgets_button_disabled),
    case("checkbox-toggle", "Clicking a checkbox toggles it and redraws the mark", widgets_checkbox_toggle),
    case("checkbox-space", "Space toggles a focused checkbox", widgets_checkbox_space),
    case("slider-drag", "Dragging a slider sets the value the contract maps from the pointer x", widgets_slider_drag),
    case("slider-clamp", "Dragging past either end clamps to the minimum or maximum", widgets_slider_clamp),
    case("slider-keys", "Arrow keys step a focused slider by its step size", widgets_slider_keys),
    case("label-clip", "A label wider than its bounds is clipped at the bounds with an ellipsis", widgets_label_clip),
    case("tabbar-select", "Clicking a tab selects it and reports its index", widgets_tabbar_select),
    case("tabbar-overflow", "More tabs than fit scroll, and every tab stays reachable", widgets_tabbar_overflow),
    case("scrollbar-drag", "Dragging the thumb sets the offset proportionally", widgets_scrollbar_drag),
    case("scrollbar-wheel", "A wheel step moves the offset by the configured amount", widgets_scrollbar_wheel),
    case("scrollbar-thumb", "The thumb's length is proportional to visible over total", widgets_scrollbar_thumb),
    case("menubar-open", "Clicking a menu title opens its menu below it", widgets_menubar_open),
    case("menubar-shortcut", "match_shortcut returns the item bound to Ctrl+S", widgets_menubar_shortcut),
    case("menu-keys", "Arrows, Enter and Escape navigate, choose and close an open menu", widgets_menu_keys),
    case("menu-disabled", "A disabled item draws dimmed and cannot be chosen", widgets_menu_disabled),
    case("menu-dismiss", "A click outside an open menu closes it without choosing", widgets_menu_dismiss),
    case("layout-stack", "h_stack and v_stack place children at the contract's literal positions", widgets_layout_stack),
    case("focus-ring", "Each focusable widget draws the theme's focus ring when focused and none otherwise", widgets_focus_ring),
    case("no-change-ack", "An event that changes no widget state is reported as needing no redraw", widgets_no_change_ack),
];

pub(crate) const TEXT_ENTRY: &[Case] = &[
    case("insert", "Typed text is inserted at the cursor", text_entry_insert),
    case("delete", "Backspace and Delete remove the character before and after the cursor", text_entry_delete),
    case("move", "Left, Right, Home and End move the cursor as the contract says", text_entry_move),
    case("select", "Shift with an arrow selects, and typing replaces the selection", text_entry_select),
    case("utf8", "Multi-byte characters insert and delete as one character", text_entry_utf8),
    case("maxlen", "Input beyond the maximum length is refused", text_entry_maxlen),
    case("scroll-to-cursor", "Text wider than the field scrolls to keep the cursor visible", text_entry_scroll_to_cursor),
    case("selection-text", "The selected text is returned exactly for a copy", text_entry_selection_text),
    case("paste-insert", "Inserted paste text replaces the selection and moves the cursor past it", text_entry_paste_insert),
    case("focus-tab", "Tab moves focus through focusable widgets in order", text_entry_focus_tab),
    case("focus-shift-tab", "Shift+Tab moves focus in reverse", text_entry_focus_shift_tab),
    case("focus-skip-disabled", "Focus traversal skips disabled widgets", text_entry_focus_skip_disabled),
    case("focus-click", "Clicking a focusable widget focuses it", text_entry_focus_click),
    case("filepicker-list", "The file picker lists the given entries, directories first, in byte order", text_entry_filepicker_list),
    case("filepicker-keys", "Arrows move the selection and Enter opens it", text_entry_filepicker_keys),
    case("filepicker-path", "Typing a path into the picker's text field navigates to it", text_entry_filepicker_path),
    case("filepicker-mouse", "A click selects and a double click within the interval, by explicit timestamps, opens", text_entry_filepicker_mouse),
    case("filepicker-scroll", "Lists longer than the view scroll by keys and wheel", text_entry_filepicker_scroll),
    case("filepicker-errors", "A directory that cannot be read shows its error in the picker", text_entry_filepicker_errors),
];

pub(crate) const ICONS: &[Case] = &[
    case("states", "An icon goes idle, hover, pressed and released on the corresponding pointer edges", icons_states),
    case("spring-steps", "Spring positions after fixed time steps equal the contract's literal table", icons_spring_steps),
    case("step-invariance", "One step of 1,000 ms, a thousand steps of 1 ms and steps of 16 ms give identical states at equal times", icons_step_invariance),
    case("settles", "Every catalogue animation settles within 2,000 ms of UI time after its last input and never draws outside its box", icons_settles),
    case("interrupt", "A hover interrupted by a press mid-animation follows the contract's state table", icons_interrupt),
    case("reversal", "Leaving mid-hover reverses from the current position without a jump", icons_reversal),
    case("reset", "Reset after a resize returns the icon to idle at its new geometry", icons_reset),
    case("bounded", "Over 10,000 steps of seeded random pointer input, the trajectory stays inside its box", icons_bounded),
    case("easing", "Each easing function matches the literal table at its sample points", icons_easing),
    case("sin-approx", "sin_approx stays within 0.0012 of sin at 4,096 evenly spaced points of [−π, π]", icons_sin_approx),
    case("particles-bounded", "Particle counts stay at or below the cap, and positions stay inside the box", icons_particles_bounded),
    case("assets", "Each icon in each state draws to its reviewed reference", icons_assets),
];

fn text_fontwatch_reload() -> CaseResult {
    missing("breengel::font::FontWatcher (poll on the UI clock, reload on change)", "9.3")
}

fn text_fontwatch_size() -> CaseResult {
    missing("breengel::font::FontWatcher (poll on the UI clock, reload on change)", "9.3")
}

fn text_fontwatch_missing() -> CaseResult {
    missing("breengel::font::FontWatcher with FontWatchError::Font", "9.3")
}

fn text_fontwatch_malformed_config() -> CaseResult {
    missing("breengel::font::FontWatcher with FontWatchError::Config", "9.2, 9.3")
}

fn text_fontwatch_partial() -> CaseResult {
    missing("breengel::font::FontWatcher (inode, size and mtime checks)", "9.2, 9.3")
}

fn text_fontwatch_repeated() -> CaseResult {
    missing("breengel::font::FontWatcher (poll on the UI clock, reload on change)", "9.3")
}

fn text_fontwatch_independent() -> CaseResult {
    missing("breengel::font::FontWatcher (mono and display fonts)", "9.3")
}

fn text_fontwatch_idle() -> CaseResult {
    missing("breengel::clock (the clock page named by BREENIX_UI_CLOCK) and the 250 ms FontWatcher", "9.1, 9.3")
}

fn text_fontwatch_file_replaced() -> CaseResult {
    missing("breengel::font::FontWatcher (font files compared by inode, size and mtime)", "9.3")
}

// ---------------------------------------------------------------------------------------
// widgets (contract section 14)
// ---------------------------------------------------------------------------------------

fn input(x: i32, y: i32, down: bool, pressed: bool, released: bool) -> InputState {
    InputState { mouse_x: x, mouse_y: y, mouse_down: down, mouse_pressed: pressed, mouse_released: released }
}
fn hover(x: i32, y: i32) -> InputState { input(x, y, false, false, false) }
fn press(x: i32, y: i32) -> InputState { input(x, y, true, true, false) }
fn hold(x: i32, y: i32) -> InputState { input(x, y, true, false, false) }
fn release(x: i32, y: i32) -> InputState { input(x, y, false, false, true) }

/// Compare `got` with a surface drawn by `expect`, both on a `bg` background.
fn same_as(got: &Guarded, bg: Color, what: &str, section: &str, expect: impl FnOnce(&mut FrameBuf)) -> CaseResult {
    let mut want = Guarded::bgrx(got.width, got.height);
    want.reset(bg);
    expect(&mut want.fb());
    for y in 0..got.height {
        for x in 0..got.width {
            if got.color(x, y) != want.color(x, y) {
                return at(section, format!("{what}: pixel ({x},{y}) is {}; the contract's drawing gives {}", hex(got.color(x, y)), hex(want.color(x, y))));
            }
        }
    }
    Ok(())
}

fn widgets_button_draw() -> CaseResult {
    let theme = Theme::dark();
    let mut g = Guarded::bgrx(100, 40);
    g.reset(PAPER);
    Button::new(Rect::new(10, 8, 60, 20), b"OK").draw(&mut g.fb(), &theme);
    same_as(&g, PAPER, "an idle 60x20 button labelled OK", "14.3", |fb| {
        shapes::fill_rect(fb, 10, 8, 60, 20, theme.widget_bg);
        shapes::draw_rect(fb, 10, 8, 60, 20, theme.border);
        // text_width("OK") = 11 and the glyphs are 7 high: (10 + 49 / 2, 8 + 13 / 2).
        font5x7::draw_text(fb, b"OK", 34, 14, theme.text_primary, 1);
    })
}

/// Feed `inputs` to `update` and collect the events that are not `None`.
fn events(mut update: impl FnMut(&InputState) -> WidgetEvent, inputs: &[InputState]) -> Vec<WidgetEvent> {
    inputs.iter().map(|i| update(i)).filter(|e| *e != WidgetEvent::None).collect()
}

fn widgets_button_click() -> CaseResult {
    let mut b = Button::new(Rect::new(10, 8, 60, 20), b"OK");
    let got = events(|i| b.update(i), &[hover(30, 15), press(30, 15), hold(31, 16), release(31, 16), hover(31, 16), hover(90, 30)]);
    ensure(got == vec![WidgetEvent::Clicked], "14.3", format!("press and release inside reported {got:?}; the contract gives exactly one Clicked"))
}

fn widgets_button_cancel() -> CaseResult {
    let mut b = Button::new(Rect::new(10, 8, 60, 20), b"OK");
    let got = events(|i| b.update(i), &[press(30, 15), hold(95, 35), release(95, 35), hover(30, 15)]);
    ensure(got.is_empty(), "14.3", format!("a press inside released outside reported {got:?}; the contract reports nothing"))?;
    let got = events(|i| b.update(i), &[press(95, 35), hold(30, 15), release(30, 15)]);
    ensure(got.is_empty(), "14.3", format!("a press outside released inside reported {got:?}; the contract reports nothing"))
}

fn widgets_button_disabled() -> CaseResult {
    missing("Button::set_enabled (a disabled state)", "14.2")
}

fn widgets_checkbox_toggle() -> CaseResult {
    let theme = Theme::dark();
    let mut c = Checkbox::new(Rect::new(10, 10, 120, 20), b"Opt");
    let got = events(|i| c.update(i), &[hover(15, 18), press(15, 18), release(15, 18)]);
    ensure(got == vec![WidgetEvent::Toggled(true)] && c.checked, "14.3", format!("one click reported {got:?} and left checked = {}", c.checked))?;
    let mut g = Guarded::bgrx(140, 40);
    g.reset(PAPER);
    c.draw(&mut g.fb(), &theme);
    same_as(&g, PAPER, "a checked checkbox", "14.3", |fb| {
        let (bx, by) = (10, 13);
        shapes::fill_rect(fb, bx, by, 14, 14, theme.accent);
        shapes::draw_line(fb, bx + 3, by + 7, bx + 5, by + 10, theme.text_primary);
        shapes::draw_line(fb, bx + 5, by + 10, bx + 11, by + 3, theme.text_primary);
        shapes::draw_rect(fb, bx, by, 14, 14, theme.border);
        font5x7::draw_text(fb, b"Opt", 28, 16, theme.text_primary, 1);
    })?;
    let got = events(|i| c.update(i), &[press(15, 18), release(15, 18)]);
    ensure(got == vec![WidgetEvent::Toggled(false)] && !c.checked, "14.3", format!("a second click reported {got:?} and left checked = {}", c.checked))
}

fn widgets_checkbox_space() -> CaseResult {
    missing("keyboard input to widgets (KeyInput and Checkbox::key)", "14.2, 14.3")
}

/// The slider's value for a pointer at `x`, from the contract's formula (section 14.3).
fn slider_value(x: i32) -> f32 {
    let v = (x - 26) as f32 / 100.0;
    v.clamp(0.0, 1.0) * 100.0
}

fn widgets_slider_drag() -> CaseResult {
    let mut s = Slider::new(Rect::new(20, 10, 112, 16), 0.0, 100.0);
    let got = events(|i| s.update(i), &[press(51, 18), hold(106, 18), hold(77, 17), release(77, 17)]);
    let want = vec![WidgetEvent::ValueChanged(slider_value(51)), WidgetEvent::ValueChanged(slider_value(106)), WidgetEvent::ValueChanged(slider_value(77))];
    ensure(got == want, "14.3", format!("dragging to x = 51, 106, 77 reported {got:?}; the contract gives {want:?}"))
}

fn widgets_slider_clamp() -> CaseResult {
    let mut s = Slider::new(Rect::new(20, 10, 112, 16), 0.0, 100.0);
    let got = events(|i| s.update(i), &[press(60, 18), hold(0, 18), hold(-500, 18), hold(500, 18), release(500, 18)]);
    let want = vec![WidgetEvent::ValueChanged(slider_value(60)), WidgetEvent::ValueChanged(0.0), WidgetEvent::ValueChanged(100.0)];
    ensure(got == want, "14.3", format!("dragging past both ends reported {got:?}; the contract clamps to {want:?}"))
}

fn widgets_slider_keys() -> CaseResult {
    missing("keyboard input to widgets (KeyInput and Slider::key with a step size)", "14.2, 14.3")
}

fn widgets_label_clip() -> CaseResult {
    missing("libbui Label clipping with an ellipsis", "14.3")
}

fn widgets_tabbar_select() -> CaseResult {
    let mut t = TabBar::new(Rect::new(0, 0, 300, 24), vec![&b"One"[..], &b"Two"[..], &b"Three"[..]]);
    let got = events(|i| t.update(i), &[press(150, 12), release(150, 12)]);
    ensure(got == vec![WidgetEvent::ValueChanged(1.0)] && t.selected() == 1, "14.3",
        format!("clicking the second tab reported {got:?} and selected {}; the contract reports ValueChanged(1) and selects 1", t.selected()))?;
    let got = events(|i| t.update(i), &[press(250, 12), release(250, 12), press(260, 12), release(260, 12)]);
    ensure(got == vec![WidgetEvent::ValueChanged(2.0)] && t.selected() == 2, "14.3",
        format!("clicking the third tab twice reported {got:?}; the contract selects on the first click only"))
}

fn widgets_tabbar_overflow() -> CaseResult {
    missing("TabBar scrolling (scroll_by) when tabs do not fit", "14.3")
}

fn widgets_scrollbar_drag() -> CaseResult {
    let mut s = ScrollBar::new(Rect::new(0, 0, 12, 200), 1000, 200);
    s.update(&press(6, 10));
    s.update(&hold(6, 90));
    // The thumb (40 px of a 160 px range) moved from 0 to 80: offset 80 / 160 × 800.
    ensure(s.offset() == 400, "14.3", format!("dragging the thumb 80 px down set offset {}; the contract gives 400", s.offset()))?;
    s.update(&hold(6, 500));
    ensure(s.offset() == 800, "14.3", format!("dragging past the end set offset {}; the contract clamps it to 800", s.offset()))
}

fn widgets_scrollbar_wheel() -> CaseResult {
    let mut s = ScrollBar::new(Rect::new(0, 0, 12, 200), 1000, 200);
    for (d, want) in [(-1, 40), (-1, 80), (2, 0), (-100, 800), (1, 760)] {
        s.scroll(d);
        ensure(s.offset() == want, "14.3", format!("scroll({d}) left offset {}; the contract gives {want}", s.offset()))?;
    }
    Ok(())
}

fn widgets_scrollbar_thumb() -> CaseResult {
    for (content, view, thumb) in [(1000, 250, 50), (1000, 500, 100), (10_000, 100, 20)] {
        let s = ScrollBar::new(Rect::new(4, 0, 12, 200), content, view);
        let mut g = Guarded::bgrx(20, 200);
        g.reset(PAPER);
        s.draw(&mut g.fb(), &Theme::dark());
        let rows = (0..200).filter(|&y| g.color(9, y) == Color::rgb(80, 85, 100)).count();
        ensure(rows == thumb - 2, "14.3", format!(
            "content {content} with view {view}: the thumb is drawn {rows} rows tall; a {thumb} px thumb draws {} rows", thumb - 2))?;
    }
    Ok(())
}

fn test_menus() -> Vec<Menu> {
    vec![
        Menu { label: b"File", items: vec![
            MenuItem::new(b"New", 1, Shortcut::ctrl(b'N')),
            MenuItem::disabled(b"Save", 2, Shortcut::ctrl(b'S')),
            MenuItem::new(b"Save As", 3, Shortcut::ctrl_shift(b'S')),
            MenuItem::separator(),
            MenuItem::new(b"Quit", 4, Shortcut::ctrl(b'Q')),
        ] },
        Menu { label: b"Edit", items: vec![MenuItem::new(b"Copy", 5, Shortcut::ctrl(b'C')), MenuItem::new(b"Paste", 6, Shortcut::ctrl(b'V'))] },
    ]
}

/// A menu bar at (10, 5), 300 wide, with File open; the File title is (10, 5, 43, 22).
fn open_menu() -> (MenuBar, Theme) {
    let theme = Theme::dark();
    let mut m = MenuBar::new(Rect::new(10, 5, 300, 22), test_menus());
    m.layout(&theme);
    m.update(&press(30, 15), &theme);
    m.update(&release(30, 15), &theme);
    (m, theme)
}

fn widgets_menubar_open() -> CaseResult {
    let (m, theme) = open_menu();
    ensure(m.is_open(), "14.3", "a press and release on the File title left its menu closed")?;
    let mut g = Guarded::bgrx(330, 140);
    g.reset(PAPER);
    m.draw(&mut g.fb(), &theme);
    // The dropdown is under the File title: its widest row, Save As with Ctrl+Shift+S, is
    // 18 + 24 + 41 + 24 + 71 + 8 = 186 wide, and four items and a separator are 97 tall.
    let (dx, dy, dw, dh) = (10usize, 27usize, 186usize, 97usize);
    for (x, y, want, what) in [
        (dx, dy + 30, theme.border, "its left border"), (dx + dw - 1, dy + 30, theme.border, "its right border"),
        (dx, dy + dh - 1, theme.border, "its bottom border"), (dx + dw, dy + 30, PAPER, "the pixel right of it"),
        (dx + 2, dy + dh - 3, theme.widget_bg, "its fill"), (dx + 2, dy + dh, PAPER, "the pixel below it"),
    ] {
        ensure(g.color(x, y) == want, "14.3", format!("{what} at ({x},{y}) is {}, the contract gives {}", hex(g.color(x, y)), hex(want)))?;
    }
    let mut want = Guarded::bgrx(17, 7);
    want.reset(theme.widget_bg);
    font5x7::draw_text(&mut want.fb(), b"New", 0, 0, theme.text_primary, 1);
    for y in 0..7 {
        for x in 0..17 {
            let got = g.color(dx + 18 + x, dy + 7 + y);
            ensure(got == want.color(x, y), "14.3", format!("the first item's label pixel ({},{}) is {}, the contract gives {}",
                dx + 18 + x, dy + 7 + y, hex(got), hex(want.color(x, y))))?;
        }
    }
    Ok(())
}

fn widgets_menubar_shortcut() -> CaseResult {
    let m = MenuBar::new(Rect::new(0, 0, 300, 22), test_menus());
    for (ascii, ctrl, shift, want) in [(b'N', true, false, Some(1u16)), (0x0E, true, false, Some(1)), (b'n', true, false, Some(1)),
        (b'S', true, true, Some(3)), (b'C', true, false, Some(5)), (b'S', true, false, None), (b'Z', true, false, None), (b'N', false, false, None)] {
        let got = m.match_shortcut(ascii, ctrl, shift, false);
        ensure(got == want, "14.3", format!("match_shortcut({ascii:#04x}, ctrl {ctrl}, shift {shift}) gave {got:?}; the contract gives {want:?} (Save is disabled)"))?;
    }
    Ok(())
}

fn widgets_menu_keys() -> CaseResult {
    missing("keyboard navigation of an open menu (MenuBar::key)", "14.3")
}

fn widgets_menu_disabled() -> CaseResult {
    let (mut m, theme) = open_menu();
    // Save, the disabled second item, is the row at y = 27 + 22 .. 27 + 44.
    let mut got = Vec::new();
    for i in [hover(40, 60), press(40, 60), release(40, 60)] {
        let e = m.update(&i, &theme);
        if e != MenuEvent::None { got.push(e); }
    }
    ensure(got.is_empty(), "14.3", format!("clicking the disabled item reported {got:?}; the contract does not let a disabled item be chosen"))?;
    let mut g = Guarded::bgrx(330, 140);
    g.reset(PAPER);
    m.draw(&mut g.fb(), &theme);
    let mut want = Guarded::bgrx(23, 7);
    want.reset(theme.widget_bg);
    font5x7::draw_text(&mut want.fb(), b"Save", 0, 0, theme.text_secondary, 1);
    for y in 0..7 {
        for x in 0..23 {
            let got = g.color(28 + x, 27 + 22 + 7 + y);
            ensure(got == want.color(x, y), "14.3", format!("the disabled label's pixel ({},{}) is {}, the contract dims it to {}",
                28 + x, 56 + y, hex(got), hex(want.color(x, y))))?;
        }
    }
    Ok(())
}

fn widgets_menu_dismiss() -> CaseResult {
    let (mut m, theme) = open_menu();
    let mut got = Vec::new();
    for i in [hover(250, 130), press(250, 130), release(250, 130)] {
        let e = m.update(&i, &theme);
        if e != MenuEvent::None { got.push(e); }
    }
    ensure(got.is_empty() && !m.is_open(), "14.3", format!(
        "a click outside the open menu reported {got:?} and left it {}; the contract closes it without choosing", if m.is_open() { "open" } else { "closed" }))
}

fn widgets_layout_stack() -> CaseResult {
    let c = Rect::new(10, 20, 500, 30);
    let got = libbui::layout::h_stack(&c, 40, 4, 6);
    let want: Vec<Rect> = (0..4).map(|i| Rect::new(10 + i * 46, 20, 40, 30)).collect();
    ensure(got == want, "14.3", format!("h_stack gave {got:?}; the contract gives {want:?}"))?;
    let c = Rect::new(3, 7, 90, 400);
    let got = libbui::layout::v_stack(&c, 25, 3, 2);
    let want: Vec<Rect> = (0..3).map(|i| Rect::new(3, 7 + i * 27, 90, 25)).collect();
    ensure(got == want, "14.3", format!("v_stack gave {got:?}; the contract gives {want:?}"))
}

fn widgets_focus_ring() -> CaseResult {
    missing("widget focus and the theme's focus ring (Theme::focus_ring)", "14.1, 14.3")
}

fn widgets_no_change_ack() -> CaseResult {
    missing("Widget::last_update_redraw (whether an update changed what the widget draws)", "14.2")
}

// ---------------------------------------------------------------------------------------
// text-entry: text fields, focus & file picker (contract section 15)
// ---------------------------------------------------------------------------------------

fn no_text_field() -> CaseResult {
    missing("libbui::widget::text_field::TextField", "15")
}

fn no_focus_chain() -> CaseResult {
    missing("libbui::focus::FocusChain", "15")
}

fn text_entry_insert() -> CaseResult { no_text_field() }
fn text_entry_delete() -> CaseResult { no_text_field() }
fn text_entry_move() -> CaseResult { no_text_field() }
fn text_entry_select() -> CaseResult { no_text_field() }
fn text_entry_utf8() -> CaseResult { no_text_field() }
fn text_entry_maxlen() -> CaseResult { no_text_field() }
fn text_entry_scroll_to_cursor() -> CaseResult { no_text_field() }
fn text_entry_selection_text() -> CaseResult { no_text_field() }
fn text_entry_paste_insert() -> CaseResult { no_text_field() }
fn text_entry_focus_tab() -> CaseResult { no_focus_chain() }
fn text_entry_focus_shift_tab() -> CaseResult { no_focus_chain() }
fn text_entry_focus_skip_disabled() -> CaseResult { no_focus_chain() }
fn text_entry_focus_click() -> CaseResult { no_focus_chain() }

fn text_entry_filepicker_list() -> CaseResult {
    let theme = Theme::dark();
    let entry = |name: &[u8], is_dir: bool| FileEntry { name: name.to_vec(), is_dir, size: 10 };
    let mut p = FilePicker::new(Rect::new(0, 0, 300, 240), b"/data".to_vec(),
        vec![entry(b"zeta", false), entry(b"Beta", true), entry(b"alpha", false), entry(b"Alpha", true), entry(b"beta", false)], &theme);
    // The list area is (1, 25, 298, 182) with 11-pixel rows.
    for (row, want) in ["Alpha", "Beta", "alpha", "beta", "zeta"].iter().enumerate() {
        let y = 25 + 11 * row as i32 + 5;
        p.update(&press(50, y));
        p.update(&release(50, y));
        let got = p.selected_entry().map(|e| String::from_utf8_lossy(&e.name).into_owned());
        ensure(got.as_deref() == Some(*want), "15", format!("row {row} lists {got:?}; directories first, then files, each in byte order, give {want:?}"))?;
    }
    Ok(())
}

fn text_entry_filepicker_keys() -> CaseResult {
    missing("keyboard input to the file picker (FilePicker::key)", "15")
}

fn text_entry_filepicker_path() -> CaseResult {
    missing("the file picker's path field", "15")
}

fn text_entry_filepicker_mouse() -> CaseResult {
    missing("InputState timestamps (time_ms) for the file picker's double click", "15")
}

fn text_entry_filepicker_scroll() -> CaseResult {
    missing("file picker scrolling by keys and wheel", "15")
}

fn text_entry_filepicker_errors() -> CaseResult {
    missing("FilePicker::show_error (a directory's read error in the list)", "15")
}

// ---------------------------------------------------------------------------------------
// icons: animated icons (contract section 16)
// ---------------------------------------------------------------------------------------

fn mouse(hovering: bool, pressed: bool, clicked: bool, released: bool) -> IconMouse {
    IconMouse { hovering, pressed, just_clicked: clicked, just_released: released, rel_x: 0.2, rel_y: -0.1 }
}

fn icons_states() -> CaseResult {
    let mut b = IconBase::new();
    let steps: [(u32, IconMouse, IconState); 9] = [
        (1_000, mouse(true, false, false, false), IconState::HoverIn),
        (249_000, mouse(true, false, false, false), IconState::HoverIn),
        (1_000, mouse(true, false, false, false), IconState::Hovering),
        (1_000, mouse(true, true, true, false), IconState::Pressed),
        (50_000, mouse(true, true, false, false), IconState::Pressed),
        (1_000, mouse(true, false, false, true), IconState::Clicked),
        (500_000, mouse(true, false, false, false), IconState::Hovering),
        (1_000, mouse(false, false, false, false), IconState::HoverOut),
        (200_000, mouse(false, false, false, false), IconState::Idle),
    ];
    for (n, (dt, m, want)) in steps.into_iter().enumerate() {
        b.update(dt, &m);
        ensure(b.state == want, "16", format!("step {n} ({dt} us): the state is {:?}; the contract's table gives {want:?}", b.state))?;
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
    let mut g = Guarded::bgrx(160, 160);
    g.reset(PAPER);
    icon.draw(&mut g.fb(), 80, 80, size);
    g
}

fn same_pixels(a: &Guarded, b: &Guarded) -> Option<(usize, usize)> {
    (0..a.height).flat_map(|y| (0..a.width).map(move |x| (x, y))).find(|&(x, y)| a.color(x, y) != b.color(x, y))
}

fn icons_step_invariance() -> CaseResult {
    for (name, make) in catalogue() {
        let schedules: [(&str, &[(u32, u32)]); 3] = [("one 1,000 ms step", &[(1, 1_000_000)]), ("1,000 steps of 1 ms", &[(1_000, 1_000)]),
            ("62 steps of 16 ms and one of 8 ms", &[(62, 16_000), (1, 8_000)])];
        let mut results = Vec::new();
        for (label, schedule) in schedules {
            let mut icon = make();
            for &(n, dt) in schedule {
                for _ in 0..n {
                    icon.update(dt, mouse(true, false, false, false));
                }
            }
            results.push((label, icon.state(), drawn(icon.as_ref(), 48)));
        }
        ensure(drawn_pixels(&results[0].2, PAPER) > 0, "16", format!("{name} drew nothing after {}", results[0].0))?;
        for (label, state, g) in &results[1..] {
            ensure(*state == results[0].1, "16", format!("{name}: after {label} the state is {state:?}, after {} it is {:?}", results[0].0, results[0].1))?;
            if let Some((x, y)) = same_pixels(g, &results[0].2) {
                return at("16", format!("{name}: after {label} pixel ({x},{y}) differs from the drawing after {}; the contract's 1 ms quanta make them equal", results[0].0));
            }
        }
    }
    Ok(())
}

fn icons_settles() -> CaseResult {
    missing("Icon::settled", "16")
}

fn icons_interrupt() -> CaseResult {
    let mut b = IconBase::new();
    b.update(1_000, &mouse(true, false, false, false));
    b.update(100_000, &mouse(true, false, false, false));
    ensure(b.state == IconState::HoverIn, "16", format!("100 ms after entering the state is {:?}, not HoverIn", b.state))?;
    b.update(1_000, &mouse(true, true, true, false));
    ensure(b.state == IconState::Pressed, "16", format!("a press during HoverIn gave {:?}; the table gives Pressed", b.state))?;
    let mut b = IconBase::new();
    b.update(1_000, &mouse(true, false, false, false));
    b.update(1_000, &mouse(false, false, false, false));
    b.update(50_000, &mouse(false, false, false, false));
    ensure(b.state == IconState::HoverOut, "16", format!("leaving during HoverIn gave {:?}; the table gives HoverOut", b.state))?;
    b.update(1_000, &mouse(true, false, false, false));
    ensure(b.state == IconState::HoverIn, "16", format!("entering again during HoverOut gave {:?}; the table gives HoverIn", b.state))
}

fn icons_reversal() -> CaseResult {
    missing("Icon::hover_amount (a continuous hover position)", "16")
}

fn icons_reset() -> CaseResult {
    for (name, make) in catalogue() {
        let mut icon = make();
        for (dt, m) in [(16_000, mouse(true, false, false, false)), (300_000, mouse(true, false, false, false)), (16_000, mouse(true, true, true, false)),
            (16_000, mouse(true, false, false, true)), (120_000, mouse(true, false, false, false))] {
            icon.update(dt, m);
        }
        icon.reset();
        let fresh = make();
        let fresh_drawing = drawn(fresh.as_ref(), 56);
        ensure(drawn_pixels(&fresh_drawing, PAPER) > 0, "16", format!("a new {name} icon drew nothing at size 56"))?;
        if let Some((x, y)) = same_pixels(&drawn(icon.as_ref(), 56), &fresh_drawing) {
            return at("16", format!("{name}: after reset pixel ({x},{y}) differs from a new icon drawn at the new size"));
        }
    }
    Ok(())
}

fn icons_bounded() -> CaseResult {
    for (name, make) in catalogue() {
        step("16", isolated(&format!("{name} under 10,000 steps of seeded input"), 6_000, || {
            let mut icon = make();
            let size = 32;
            let mut g = Guarded::bgrx(160, 160);
            g.reset(PAPER);
            let mut rng = Seeded(0x1C0_5EED ^ name.len() as u64);
            let mut held = false;
            for _ in 0..10_000 {
                let hovering = rng.next() % 4 != 0;
                let press = !held && rng.next() % 9 == 0;
                let release = held && rng.next() % 5 == 0;
                held = (held || press) && !release;
                icon.update(1_000 + (rng.next() % 32_000) as u32, IconMouse { hovering, pressed: held, just_clicked: press,
                    just_released: release, rel_x: (rng.next() % 200) as f32 / 100.0 - 1.0, rel_y: (rng.next() % 200) as f32 / 100.0 - 1.0 });
                icon.draw(&mut g.fb(), 80, 80, size);
            }
            if drawn_pixels(&g, PAPER) == 0 {
                return Err(format!("{name} drew nothing in 10,000 steps"));
            }
            let half = size / 2 + icon.bounds_overflow();
            for y in 0..160i64 {
                for x in 0..160i64 {
                    if !in_rect(x, y, 80 - half as i64, 80 - half as i64, 2 * half as i64, 2 * half as i64) && g.color(x as usize, y as usize) != PAPER {
                        return Err(format!("{name} drew pixel ({x},{y}), outside its box of half-width {half} around (80,80)"));
                    }
                }
            }
            g.sentinels()
        }))?;
    }
    Ok(())
}

fn icons_easing() -> CaseResult {
    use libicon::easing;
    let functions: [(&str, fn(f32) -> f32, f64); 7] = [("linear", easing::linear, 1e-5), ("ease_out_cubic", easing::ease_out_cubic, 1e-5),
        ("ease_in_cubic", easing::ease_in_cubic, 1e-5), ("ease_in_out_cubic", easing::ease_in_out_cubic, 1e-5),
        ("ease_out_elastic", easing::ease_out_elastic, 0.002), ("ease_out_bounce", easing::ease_out_bounce, 1e-5),
        ("ease_out_back", easing::ease_out_back, 1e-5)];
    for (name, f, tolerance) in functions {
        let row = EASING_TABLE.iter().find(|(n, _)| *n == name).map(|(_, r)| r).ok_or_else(|| format!("no table row for {name}")).cite("16")?;
        for (i, &want) in row.iter().enumerate() {
            let t = i as f32 / 10.0;
            let got = f(t) as f64;
            ensure((got - want).abs() <= tolerance, "16", format!("{name}({t}) is {got:.6}; the contract's table gives {want} within {tolerance}"))?;
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
        if err > worst.0 { worst = (err, x as f64); }
    }
    value("worst-error-ppm", (worst.0 * 1e6) as i64, "", Some((0, 1200)));
    ensure(worst.0 <= 0.0012, "16", format!("sin_approx is {:.6} from sin at x = {:.5}; the bound is 0.0012", worst.0, worst.1))
}

fn icons_particles_bounded() -> CaseResult {
    let mut pool = ParticlePool::new(8);
    for i in 0..20 {
        pool.emit(Particle { x: 0.0, y: 0.0, vx: i as f32 * 0.1, vy: -0.2, life: 1.0, max_life: 1.0, size: 2.0,
            color: Color::rgb(200, 200, 200), gravity: 0.5, friction: 0.1 });
        ensure(pool.active_count() <= 8, "16", format!("after {} emits a pool of 8 holds {} live particles", i + 1, pool.active_count()))?;
    }
    missing("ParticlePool::with_bounds (the count stays within the cap; positions are not confined)", "16")
}

fn icons_assets() -> CaseResult {
    reference("icons/assets.png")
}
const EASING_TABLE: &[(&str, [f64; 11])] = &[
    ("linear", [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0]),
    ("ease_out_cubic", [0.0, 0.271, 0.488, 0.657, 0.784, 0.875, 0.936, 0.973, 0.992, 0.999, 1.0]),
    ("ease_in_cubic", [0.0, 0.001, 0.008, 0.027, 0.064, 0.125, 0.216, 0.343, 0.512, 0.729, 1.0]),
    ("ease_in_out_cubic", [0.0, 0.004, 0.032, 0.108, 0.256, 0.5, 0.744, 0.892, 0.968, 0.996, 1.0]),
    ("ease_out_elastic", [0.0, 1.25, 1.125, 0.875, 1.03125, 1.015625, 0.984375, 1.003906, 1.001953, 0.998047, 1.0]),
    ("ease_out_bounce", [0.0, 0.075625, 0.3025, 0.680625, 0.91, 0.765625, 0.7725, 0.930625, 0.94, 0.988125, 1.0]),
    ("ease_out_back", [0.0, 0.408828, 0.705802, 0.907132, 1.029028, 1.087697, 1.099352, 1.0802, 1.046451, 1.014314, 1.0]),
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
