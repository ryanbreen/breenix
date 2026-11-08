use super::*;

#[path = "chrome/validation.rs"]
mod validation;
pub(super) use validation::ValidationQueue;

pub(super) struct PresentedScene {
    pub layers: Vec<Layer>,
    pub pointer: Pointer,
}

impl Clone for PresentedScene {
    fn clone(&self) -> Self {
        Self { layers: self.layers.clone(), pointer: self.pointer }
    }
    fn clone_from(&mut self, source: &Self) {
        self.layers.clone_from(&source.layers);
        self.pointer = source.pointer;
    }
}

pub(super) fn measure_check<T>(check: impl FnOnce() -> T) -> T {
    let begin = std::time::Instant::now();
    let result = check();
    let record = format!("VALUE bwm-check-us {}\n", begin.elapsed().as_micros());
    print!("{record}");
    // Suite desktops redirect stdout into their private readiness pipe. The
    // shared root-only evidence device retains measurements on either arch.
    // The device admits records only after the suite latch is set; outside
    // that state it refuses writes and stdout remains the sink.
    if let Ok(mut evidence) = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence") {
        use std::io::Write;
        let _ = evidence.write_all(record.as_bytes());
    }
    result
}

fn validation_idle(poll_fds: &mut [io::PollFd]) -> bool {
    for fd in poll_fds.iter_mut() {
        fd.revents = 0;
    }
    matches!(io::poll(poll_fds, 0), Ok(0)) && poll_fds.iter().all(|fd| fd.revents == 0)
}

pub(super) fn run_deferred_check(
    queue: &mut ValidationQueue<PresentedScene>,
    gpu: &mut Compositor,
    windows: &mut [Window],
    poll_fds: &mut [io::PollFd],
    resizing: bool,
) {
    if resizing { return; }

    let now = monotonic_ms().unwrap_or(0);
    let shown = queue.scene().map(|scene| gpu.shown_clients(&scene.layers, Some(scene.pointer)))
        .unwrap_or_default();
    let Some((scene, unchecked)) = queue.next_if_idle(
        |id| shown.iter().any(|(shown, _)| *shown == id)
            && windows.iter().any(|w| w.window_id == id && now >= w.check_after_ms),
        || validation_idle(poll_fds),
    ) else { return };
    let next = unchecked.iter().copied()
        .min_by_key(|id| windows.iter().find(|w| w.window_id == *id).map_or(0, |w| w.check_moved));
    let id = next.unwrap_or(0);
    let result = measure_check(|| gpu.check(id, &scene.layers, Some(scene.pointer)));
    if result.is_ok() {
        // Harvested from 097962f6: the full-scene comparison includes the
        // shown sources, so its successful result also serves these clients.
        for checked in unchecked {
            if let Some(win) = windows.iter_mut().find(|w| w.window_id == checked) {
                win.frame_checked = true;
            }
            queue.complete(checked);
            report_check(checked, &result);
        }
    } else if next.is_some() {
        let retry = match (&result, windows.iter_mut().find(|w| w.window_id == id)) {
            (Err(CheckError::Moved(_)), Some(win)) => {
                win.check_moved += 1;
                win.check_moved < CHECK_MOVED_LIMIT
            }
            _ => false,
        };
        if !retry {
            if let Some(win) = windows.iter_mut().find(|w| w.window_id == id) {
                win.frame_checked = true;
            }
            queue.complete(id);
            report_check(id, &result);
        }
    } else if matches!(result, Err(CheckError::Failed(_))) {
        report_check(0, &result);
    }
}

// ─── Drawing Helpers ─────────────────────────────────────────────────────────

pub(super) fn fill_rect(fb: &mut Surface, x: i32, y: i32, w: usize, h: usize, color: Color) {
    libgfx::shapes::fill_rect(fb, x, y, w as i32, h as i32, color);
}

pub(super) fn draw_text_at(fb: &mut Surface, text: &[u8], x: i32, y: i32, color: Color,
                ttf: Option<&mut CachedFont>) {
    if y < 0 || y >= fb.height() as i32 { return; }
    if let Some(font) = ttf {
        // Use LCD subpixel rendering for crisp window chrome text
        let s = unsafe { core::str::from_utf8_unchecked(text) };
        ttf_font::draw_text_subpixel(fb, font, s, x, y, ttf_font_size() * ui().scale as f32, color);
    } else {
        // Only the characters whose whole cell lies across the surface's width are drawn.
        let cell_w = ui().cell_w as i32;
        let shown = |i: usize| {
            let cx = x + i as i32 * cell_w;
            cx >= 0 && cx + cell_w <= fb.width() as i32
        };
        let Some(first) = (0..text.len()).find(|&i| shown(i)) else { return };
        let last = (first..text.len()).take_while(|&i| shown(i)).last().unwrap_or(first);
        let cx = x + first as i32 * cell_w;
        bitmap_font::draw_text_at(fb, &text[first..=last], cx as usize, y as usize, color, ui().font);
    }
}

/// Draw a floating window frame (border + title bar + chrome buttons + content bg)
pub(super) fn draw_window_frame(fb: &mut Surface, win: &Window, focused: bool,
                     ui_font: &mut Option<CachedFont>) {
    let border_color = if focused { WIN_BORDER_FOCUSED } else { WIN_BORDER_COLOR };
    let title_bg = if focused { TITLE_FOCUSED_BG } else { TITLE_UNFOCUSED_BG };
    let title_fg = if focused { TITLE_FOCUSED_TEXT } else { TITLE_TEXT };
    let bw = px(BORDER_WIDTH);

    let shadow = Color::rgb(8, 10, 18);
    let offset = px(SHADOW_OFFSET) as i32;
    fill_rect(fb, win.x + offset, win.y + offset, win.width, win.total_height(), shadow);
    fill_rect(fb, win.x, win.y, win.width, win.total_height(), border_color);
    let content_bg = Color::rgb(12, 14, 20);
    fill_rect(fb, win.content_x(), win.content_y(),
              win.content_width(), win.content_height(), content_bg);
    fill_rect(fb, win.x + bw as i32, win.y + bw as i32,
              win.width - bw * 2, px(TITLE_BAR_HEIGHT) - bw, title_bg);

    let max_title_w = win.width.saturating_sub(bw * 2 + px(8) + px(CHROME_RESERVED));
    let max_chars = max_title_w / ui().cell_w;
    let text_len = win.title_len.min(max_chars);
    let text_y = win.y + bw as i32 + ((px(TITLE_BAR_HEIGHT) - bw - ui().cell_h) / 2) as i32;
    draw_text_at(fb, &win.title[..text_len], win.x + pxi(8), text_y, title_fg,
                 ui_font.as_mut());

    let (cbx, cby, cbw, cbh) = win.close_btn_rect();
    fill_rect(fb, cbx, cby, cbw, cbh, CLOSE_BTN_BG);
    let cx = cbx + (cbw as i32 - ui().cell_w as i32) / 2;
    let cy = cby + (cbh as i32 - ui().cell_h as i32) / 2;
    draw_text_at(fb, b"x", cx, cy, CLOSE_BTN_TEXT, ui_font.as_mut());

    let (mbx, mby, mbw, mbh) = win.minimize_btn_rect();
    fill_rect(fb, mbx, mby, mbw, mbh, MINIMIZE_BTN_BG);
    let mx = mbx + (mbw as i32 - ui().cell_w as i32) / 2;
    let my = mby + (mbh as i32 - ui().cell_h as i32) / 2;
    draw_text_at(fb, b"-", mx, my, MINIMIZE_BTN_TEXT, ui_font.as_mut());
}

/// The decorative desktop background at `(x, y)` of a `w` x `h` screen: a vertical gradient
/// whose colour steps with the row, with a grid line each 64 logical units.
pub(super) fn background_pixel(x: usize, y: usize, h: usize) -> Color {
    let (r, g, b) = background_row(y, h);
    let step = px(64);
    let grid = (x % step == 0 || y % step == 0) as u8;
    Color::rgb(r.saturating_add(grid * 6), g.saturating_add(grid * 8), b.saturating_add(grid * 12))
}

/// The gradient's colour on row `y`, before the grid.
pub(super) fn background_row(y: usize, h: usize) -> (u8, u8, u8) {
    let t = (y * 255 / h) as u8;
    (12u8.saturating_add(t / 12), 16u8.saturating_add(t / 20), 38u8.saturating_add(t / 6))
}

/// Paint the decorative desktop background — gradient with grid — into `fb`, the compositor's
/// background target, which each desktop repaint copies. Rows of one gradient colour form a
/// band: each band is one rectangle, its vertical grid lines another draw, and each grid row
/// a full-width line, so the GPU draws `background_pixel`'s colours exactly.
pub(super) fn paint_background(fb: &mut Surface) {
    let (w, h) = (fb.width(), fb.height());
    let step = px(64);
    fb.batch(|fb| {
        let mut y0 = 0;
        while y0 < h {
            let colour = background_row(y0, h);
            let mut y1 = y0 + 1;
            while y1 < h && background_row(y1, h) == colour {
                y1 += 1;
            }
            let (r, g, b) = colour;
            fill_rect(fb, 0, y0 as i32, w, y1 - y0, Color::rgb(r, g, b));
            let line = Color::rgb(r.saturating_add(6), g.saturating_add(8), b.saturating_add(12));
            for x in (0..w).step_by(step) {
                fill_rect(fb, x as i32, y0 as i32, 1, y1 - y0, line);
            }
            for y in (y0..y1).filter(|y| y % step == 0) {
                fill_rect(fb, 0, y as i32, w, 1, line);
            }
            y0 = y1;
        }
    });
}

// ─── Taskbar & App Bar ──────────────────────────────────────────────────────

/// US Eastern Time offset from UTC in seconds.
/// DST (EDT, UTC-4) runs from 2nd Sunday of March to 1st Sunday of November.
/// Standard (EST, UTC-5) applies the rest of the year.
pub(super) fn eastern_offset_secs(utc_secs: i64) -> i64 {
    // Compute day-of-year, month, day-of-week from Unix timestamp
    let days = (utc_secs / 86400) as i64;

    // Compute year/month/day
    let mut y = 1970i64;
    let mut rem = days;
    loop {
        let ydays = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if rem < ydays { break; }
        rem -= ydays;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays: [i64; 12] = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0u8;
    for i in 0..12 {
        if rem < mdays[i] { month = i as u8 + 1; break; }
        rem -= mdays[i];
    }
    let day = rem as u8 + 1;

    // 2nd Sunday of March: find first Sunday in March, add 7
    // 1st Sunday of November: find first Sunday in November
    // Check month boundaries for DST transitions
    // DST starts: 2nd Sunday of March at 2:00 AM EST (7:00 AM UTC)
    // DST ends: 1st Sunday of November at 2:00 AM EDT (6:00 AM UTC)
    let hour_utc = ((utc_secs % 86400) / 3600) as u8;

    // March: find day-of-week of March 1
    let jan1_dow = {
        let mut d = 0i64;
        for yr in 1970..y {
            d += if yr % 4 == 0 && (yr % 100 != 0 || yr % 400 == 0) { 366 } else { 365 };
        }
        ((d % 7 + 4) % 7) as u8 // 0=Sun
    };
    // Day of week of March 1 = (jan1_dow + 31 + feb_days) % 7
    let feb = if leap { 29 } else { 28 };
    let mar1_dow = (jan1_dow + 31 + feb) % 7;
    let dst_start_day = if mar1_dow == 0 { 8u8 } else { (14 - mar1_dow + 1) as u8 }; // 2nd Sunday
    // Day of week of Nov 1
    let days_to_nov1: u16 = [31u16, feb as u16, 31, 30, 31, 30, 31, 31, 30, 31].iter().sum();
    let nov1_dow = (jan1_dow as u16 + days_to_nov1) % 7;
    let dst_end_day = if nov1_dow == 0 { 1u8 } else { 7 - nov1_dow as u8 + 1 };

    let is_dst = match month {
        1..=2 => false,
        4..=10 => true,
        3 => day > dst_start_day || (day == dst_start_day && hour_utc >= 7),
        11 => day < dst_end_day || (day == dst_end_day && hour_utc < 6),
        12 => false,
        _ => false,
    };

    if is_dst { -4 * 3600 } else { -5 * 3600 }
}

pub(super) fn format_clock(utc_secs: i64, buf: &mut [u8; 11]) {
    if !settings().clock_visible { buf.fill(b' '); return; }
    let offset = eastern_offset_secs(utc_secs);
    let local = utc_secs + offset;
    // Handle day wrap
    let t = ((local % 86400) + 86400) % 86400;
    let s = (t % 60) as u8;
    let m = ((t / 60) % 60) as u8;
    let h = ((t / 3600) % 24) as u8;
    buf[0] = b'0' + h / 10;
    buf[1] = b'0' + h % 10;
    buf[2] = b':';
    buf[3] = b'0' + m / 10;
    buf[4] = b'0' + m % 10;
    buf[5] = b':';
    buf[6] = b'0' + s / 10;
    buf[7] = b'0' + s % 10;
    buf[8] = b' ';
    buf[9] = b'E';
    buf[10] = b'T';
}

pub(super) fn draw_taskbar(fb: &mut Surface, clock_text: &[u8], ui_font: &mut Option<CachedFont>) {
    let w = fb.width();
    fill_rect(fb, 0, 0, w, px(TASKBAR_HEIGHT), TASKBAR_BG);
    let label_y = ((px(TASKBAR_HEIGHT) - ui().cell_h) / 2 + px(1)) as i32;
    let clock_x = w as i32 - (clock_text.len() as i32 * ui().cell_w as i32) - pxi(8);
    if let Some(font) = ui_font.as_mut() {
        let clip = fb.clip();
        let runs = [
            ttf_font::TextRun { bytes: b"Breenix", x: pxi(8), y: label_y,
                size: ttf_font_size() * ui().scale as f32, fg: TASKBAR_TEXT, clip },
            ttf_font::TextRun { bytes: clock_text, x: clock_x, y: label_y,
                size: ttf_font_size() * ui().scale as f32, fg: TASKBAR_TEXT, clip },
        ];
        ttf_font::draw_runs_subpixel(fb, font, &runs);
    } else {
        draw_text_at(fb, b"Breenix", pxi(8), label_y, TASKBAR_TEXT, None);
        if !clock_text.is_empty() {
            draw_text_at(fb, clock_text, clock_x, label_y, TASKBAR_TEXT, None);
        }
    }
}

pub(super) fn appbar_button_width(title_len: usize) -> usize {
    let text_w = title_len * ui().cell_w + px(16);
    text_w.max(px(60)).min(px(180))
}

/// Build indices sorted by creation_order (stable appbar layout).
pub(super) fn sorted_by_creation(windows: &[Window]) -> ([usize; 16], usize) {
    let n = windows.len().min(16);
    let mut idx = [0usize; 16];
    for i in 0..n { idx[i] = i; }
    // Insertion sort (at most 16 elements)
    for i in 1..n {
        let mut j = i;
        while j > 0 && windows[idx[j]].creation_order < windows[idx[j - 1]].creation_order {
            idx.swap(j, j - 1);
            j -= 1;
        }
    }
    (idx, n)
}

pub(super) fn draw_appbar(fb: &mut Surface, windows: &[Window], stack: &Stack,
               ui_font: &mut Option<CachedFont>) {
    let screen_h = fb.height();
    let screen_w = fb.width();
    let bar_y = (screen_h - px(APPBAR_HEIGHT)) as i32;

    fill_rect(fb, 0, bar_y, screen_w, px(APPBAR_HEIGHT), APPBAR_BG);
    fill_rect(fb, 0, bar_y, screen_w, px(1), APPBAR_BORDER);

    let (sorted, n) = sorted_by_creation(windows);
    let mut btn_x: i32 = pxi(4);
    let btn_h: usize = px(APPBAR_HEIGHT) - px(8);
    let btn_y = bar_y + pxi(4);

    for k in 0..n {
        let i = sorted[k];
        let win = &windows[i];
        if win.chromeless { continue; }
        let btn_w = appbar_button_width(win.title_len);

        let bg = if stack.focus() == Some(win.window_id) && !stack.minimized(win.window_id) {
            APPBAR_BTN_FOCUSED
        } else if stack.minimized(win.window_id) {
            APPBAR_BTN_MINIMIZED
        } else {
            APPBAR_BTN_BG
        };

        fill_rect(fb, btn_x, btn_y, btn_w, btn_h, bg);

        let max_chars = (btn_w.saturating_sub(px(12))) / ui().cell_w;
        let text_len = win.title_len.min(max_chars);
        let text_y = btn_y + ((btn_h - ui().cell_h) / 2 + px(1)) as i32;
        draw_text_at(fb, &win.title[..text_len], btn_x + pxi(6), text_y, APPBAR_BTN_TEXT,
                     ui_font.as_mut());

        btn_x += btn_w as i32 + pxi(2);
        if btn_x >= screen_w as i32 - pxi(4) { break; }
    }
}

pub(super) fn appbar_hit_test(windows: &[Window], screen_w: usize, screen_h: usize, mx: i32, my: i32) -> Option<usize> {
    let bar_y = (screen_h - px(APPBAR_HEIGHT)) as i32;
    if my < bar_y || my >= screen_h as i32 { return None; }

    let (sorted, n) = sorted_by_creation(windows);
    let mut btn_x: i32 = pxi(4);
    let btn_h = (px(APPBAR_HEIGHT) - px(8)) as i32;
    let btn_y = bar_y + pxi(4);

    for k in 0..n {
        let i = sorted[k];
        if windows[i].chromeless { continue; }
        let btn_w = appbar_button_width(windows[i].title_len) as i32;
        if mx >= btn_x && mx < btn_x + btn_w
            && my >= btn_y && my < btn_y + btn_h
        {
            return Some(i); // Returns the actual Vec index, not the sorted position
        }
        btn_x += btn_w + pxi(2);
        if btn_x >= screen_w as i32 - pxi(4) { break; }
    }
    None
}

/// Redraw the windows in stack order, plus taskbar and app bar.
/// Window frames and decorations go into the desktop target; the compositor draws client
/// content above them from the windows' committed fronts.
pub(super) fn redraw_all_windows(fb: &mut Surface, windows: &[Window], stack: &Stack,
                      clock_text: &[u8], ui_font: &mut Option<CachedFont>) {
    draw_taskbar(fb, clock_text, ui_font);
    libuitest::startup::mark("chrome-taskbar-ready");
    for win in ordered_windows(windows, stack) {
        if stack.minimized(win.window_id) || win.chromeless { continue; }
        draw_window_frame(fb, win, stack.focus() == Some(win.window_id), ui_font);
    }
    draw_appbar(fb, windows, stack, ui_font);
    libuitest::startup::mark("chrome-appbar-ready");
}

/// Repaint the whole desktop target: the background, then the chrome.
pub(super) fn compose_full_redraw(gpu: &mut Compositor, windows: &[Window], stack: &Stack, clock: &[u8],
                       ui_font: &mut Option<CachedFont>) {
    let Compositor { desktop, background, .. } = gpu;
    libgfx::blit::blit(desktop, background, 0, 0);
    libuitest::startup::mark("chrome-blit-ready");
    redraw_all_windows(desktop, windows, stack, clock, ui_font);
}

/// Partial redraw: repaint only the dirty sub-region `(dx0, dy0)..(dx1, dy1)` of the desktop
/// target, under a clip, with the bars and the frames that meet it.
pub(super) fn compose_partial_redraw(
    gpu: &mut Compositor,
    windows: &[Window],
    stack: &Stack,
    clock: &[u8],
    dx0: usize, dy0: usize, dx1: usize, dy1: usize,
    ui_font: &mut Option<CachedFont>,
) {
    let Compositor { desktop: fb, background, .. } = gpu;
    let screen_w = fb.width();
    let screen_h = fb.height();
    let dx1 = dx1.min(screen_w);
    let dy1 = dy1.min(screen_h);
    if dx0 >= dx1 || dy0 >= dy1 { return; }
    fb.push_clip(Clip::new(dx0 as i32, dy0 as i32, (dx1 - dx0) as i32, (dy1 - dy0) as i32));
    libgfx::blit::blit(fb, background, 0, 0);
    if dy0 < px(TASKBAR_HEIGHT) {
        draw_taskbar(fb, clock, ui_font);
    }
    for win in ordered_windows(windows, stack) {
        if stack.minimized(win.window_id) || win.chromeless { continue; }
        let (wx0, wy0, wx1, wy1) = win.bounds();
        if (wx1 as usize) > dx0 && (wx0 as usize) < dx1
            && (wy1 as usize) > dy0 && (wy0 as usize) < dy1
        {
            draw_window_frame(fb, win, stack.focus() == Some(win.window_id), ui_font);
        }
    }
    if dy1 > screen_h - px(APPBAR_HEIGHT) {
        draw_appbar(fb, windows, stack, ui_font);
    }
    fb.pop_clip();
}

/// The visible client windows as the compositor stacks them, bottom to top: each one's
/// frame and drop shadow (as `draw_window_frame` paints them) beneath its content.
pub(super) fn frame_layers(windows: &[Window], stack: &Stack, layers: &mut Vec<Layer>) {
    layers.clear();
    layers.extend(ordered_windows(windows, stack)
        .filter(|w| !stack.minimized(w.window_id))
        .map(|w| {
            let (mut frame, mut frame_len) = ([[0i32; 4]; 3], 0);
            if !w.chromeless {
                let (x0, y0) = (w.x, w.y);
                let (x1, y1) = (w.x + w.width as i32, w.y + w.total_height() as i32);
                let d = px(SHADOW_OFFSET) as i32;
                frame = [[x0, y0, x1, y1], [x1, y0 + d, x1 + d, y1 + d], [x0 + d, y1, x1, y1 + d]];
                frame_len = 3;
            }
            Layer {
                window_id: w.window_id,
                frame,
                frame_len,
                content: [w.content_x(), w.content_y(), w.content_width() as i32, w.content_height() as i32],
            }
        })
    );
}

/// Show that the desktop covers the whole scanout at the chosen scale. bwm composes the
/// desktop alone (no windows yet, and no pointer over the sampled pixels), then reads the
/// scanout itself back where the scale puts the edges of the taskbar and the app bar: the
/// top-left and bottom-right pixels of each bar, and the background pixel just past each.
/// Each must be the colour the desktop's definition gives there (the bar colours, and
/// `background_pixel`'s formula). The read-back's record must name a scanout of the composed
/// size, the one MODES reports bound, and the GPU core's current mode must have that size.
pub(super) fn check_desktop_fills(gpu: &mut Compositor, screen_w: usize, screen_h: usize, mode_index: u32) -> Result<String, String> {
    measure_check(|| {
        // The whole frame, so no pointer the last frame drew remains in it.
        gpu.present_startup_desktop(None).map_err(|e| e.to_string())?;
        let (w, h) = (screen_w as u32, screen_h as u32);
        let (tb, ab) = (px(TASKBAR_HEIGHT) as u32, px(APPBAR_HEIGHT) as u32);
        let word = |c: Color| 0xFF00_0000 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32;
        let background = |x: u32, y: u32| word(background_pixel(x as usize, y as usize, screen_h));
        // (x, y, the desktop's colour there)
        let samples = [
            (0, 0, word(TASKBAR_BG)),
            (w - 1, tb - 1, word(TASKBAR_BG)),
            (w - 1, tb, background(w - 1, tb)),
            (w - 1, h - ab - 1, background(w - 1, h - ab - 1)),
            (0, h - ab, word(APPBAR_BORDER)),
            (w - 1, h - 1, word(APPBAR_BG)),
        ];
        let shown = gpu.check_fills(&samples)?;
        let bound = compositor::scanout_resource()?;
        if bound != shown.resource {
            return Err(format!("MODES reports resource {} bound, the read-back showed {}", bound, shown.resource));
        }
        let (_, mode) = compositor::display()?;
        if (mode.width, mode.height) != (shown.width, shown.height) {
            return Err(format!("the GPU core's mode is {}x{}, the scanout {}x{}", mode.width, mode.height, shown.width, shown.height));
        }
        Ok(format!(
            "resource {} {}x{} bound (GPU core mode {}, {}x{}), composed {}x{}, UI scale {}; taskbar rows 0..{} at (0,0) and ({},{}), background at ({},{}); app bar rows {}..{} at (0,{}) and ({},{}), background at ({},{}); {} pixels exact",
            shown.resource, shown.width, shown.height, mode_index, mode.width, mode.height, w, h, ui().scale,
            tb, w - 1, tb - 1, w - 1, tb, h - ab, h, h - ab, w - 1, h - 1, w - 1, h - ab - 1, samples.len()
        ))
    })
}
