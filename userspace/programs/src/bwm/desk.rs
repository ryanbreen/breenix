use super::*;

// ─── Window Defaults (Position/Size Memory) ─────────────────────────────────

pub(super) struct WindowDefaults {
    pub(super) title: [u8; 32],
    pub(super) title_len: usize,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) valid: bool,
}

pub(super) fn save_window_defaults(win: &Window, defaults: &mut [WindowDefaults; MAX_DEFAULTS]) {
    let mut slot = None;
    for i in 0..MAX_DEFAULTS {
        if defaults[i].valid
            && defaults[i].title_len == win.title_len
            && defaults[i].title[..win.title_len] == win.title[..win.title_len]
        {
            slot = Some(i);
            break;
        }
    }
    if slot.is_none() {
        for i in 0..MAX_DEFAULTS {
            if !defaults[i].valid {
                slot = Some(i);
                break;
            }
        }
    }
    if let Some(i) = slot {
        defaults[i].title = win.title;
        defaults[i].title_len = win.title_len;
        defaults[i].x = win.x;
        defaults[i].y = win.y;
        defaults[i].width = win.width;
        defaults[i].height = win.height;
        defaults[i].valid = true;
    }
}

pub(super) fn find_window_defaults(
    title: &[u8],
    title_len: usize,
    defaults: &[WindowDefaults; MAX_DEFAULTS],
) -> Option<(i32, i32, usize, usize)> {
    for d in defaults.iter() {
        if d.valid && d.title_len == title_len && d.title[..d.title_len] == title[..title_len] {
            return Some((d.x, d.y, d.width, d.height));
        }
    }
    None
}

// ─── Window ─────────────────────────────────────────────────────────────────

pub(super) struct Window {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) title: [u8; 32],
    pub(super) title_len: usize,
    pub(super) window_id: u64,
    pub(super) owner_pid: u32,
    pub(super) minimized: bool,
    /// The first composed frame showing this client has been checked.
    pub(super) frame_checked: bool,
    /// Delay resize validation until input and the owner have settled.
    pub(super) check_after_ms: u64,
    /// Fixed far edges for owner-mediated left/top resizes.
    pub(super) resize_right: Option<i32>,
    pub(super) resize_bottom: Option<i32>,
    /// Composed frames whose check found this client had committed again since the frame
    /// sampled it.
    pub(super) check_moved: u32,
    /// Stable ordering for appbar (assigned at discovery time)
    pub(super) creation_order: u32,
    /// Chromeless windows have no title bar, border, or chrome buttons.
    /// Detected by title prefix \x01.
    pub(super) chromeless: bool,
}

pub(super) fn sorted_by_creation(windows: &[Window]) -> ([usize; 16], usize) {
    let n = windows.len().min(16);
    let mut idx = [0usize; 16];
    for i in 0..n {
        idx[i] = i;
    }
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

pub(super) fn next_visible_window(windows: &[Window], current: usize) -> usize {
    for i in (0..windows.len()).rev() {
        if !windows[i].minimized {
            return i;
        }
    }
    current
}

// ─── Input Routing ──────────────────────────────────────────────────────────

pub(super) fn discover_windows(
    windows: &mut Vec<Window>,
    screen_w: usize,
    screen_h: usize,
    next_order: &mut u32,
    win_defaults: &mut [WindowDefaults; MAX_DEFAULTS],
) -> bool {
    let mut win_infos = Vec::new();
    if session::list_windows(&mut win_infos).is_err() {
        return false;
    }

    INPUT_BACKLOG
        .lock()
        .unwrap()
        .retain_windows(|id| win_infos.iter().any(|info| info.id == id));

    // Save defaults for windows about to be removed
    for w in windows.iter() {
        if !win_infos.iter().any(|info| info.id == w.window_id) {
            save_window_defaults(w, win_defaults);
        }
    }

    let before = windows.len();
    windows.retain(|w| win_infos.iter().any(|info| info.id == w.window_id));
    let removed = before > windows.len();

    let mut added = false;
    let mut resized = false;
    for info in &win_infos {
        if let Some((index, w)) = windows
            .iter_mut()
            .enumerate()
            .find(|(_, w)| w.window_id == info.id)
        {
            let width = info.width as usize
                + if w.chromeless {
                    0
                } else {
                    px(BORDER_WIDTH) * 2
                };
            let height = info.height as usize
                + if w.chromeless {
                    0
                } else {
                    px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2
                };
            if (w.width, w.height) != (width, height) {
                if let Some(right) = w.resize_right {
                    w.x = right - width as i32;
                }
                if let Some(bottom) = w.resize_bottom {
                    w.y = bottom - height as i32;
                }
                w.width = width;
                w.height = height;
                w.frame_checked = false;
                w.check_moved = 0;
                w.check_after_ms = monotonic_ms().unwrap_or(0) + 150;
                let _ = session::set_geometry(
                    w.window_id,
                    w.content_x(),
                    w.content_y(),
                    index as u32,
                    true,
                );
                resized = true;
            }
            continue;
        }

        let n = windows.len();
        let mut title = [0u8; 32];
        let title_len = (info.title_len as usize).min(32);
        title[..title_len].copy_from_slice(&info.title[..title_len]);

        // Chromeless windows (title prefix \x01) have no frame/chrome
        let chromeless = title_len > 0 && title[0] == 1;

        let total_w = if chromeless {
            info.width as usize
        } else {
            info.width as usize + px(BORDER_WIDTH) * 2
        };
        let total_h = if chromeless {
            info.height as usize
        } else {
            info.height as usize + px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2
        };

        // Use saved defaults if available, otherwise cascade (or origin for fullscreen chromeless)
        let (win_x, win_y, win_w, win_h) =
            match find_window_defaults(&title[..title_len], title_len, win_defaults) {
                Some((dx, dy, _, _)) => (dx, dy, total_w, total_h),
                None => {
                    if chromeless && total_w >= screen_w && total_h >= screen_h {
                        (0i32, 0i32, total_w, total_h)
                    } else if chromeless {
                        // Center small chromeless windows on screen
                        let cx = (screen_w as i32 - total_w as i32) / 2;
                        let cy = screen_h as i32 / 5; // Spotlight-style: 1/5 from top
                        (cx.max(0), cy.max(0), total_w, total_h)
                    } else {
                        let (x, y) = placement::cascade_position(
                            [
                                0,
                                px(TASKBAR_HEIGHT),
                                screen_w,
                                screen_h.saturating_sub(px(TASKBAR_HEIGHT) + px(APPBAR_HEIGHT)),
                            ],
                            [total_w, total_h],
                            ui().scale,
                            n,
                        );
                        (x, y, total_w, total_h)
                    }
                }
            };

        print!(
            "[bwm] Discovered window '{}' (id={}, {}x{}{}) at ({},{})\n",
            core::str::from_utf8(&title[..title_len]).unwrap_or("?"),
            info.id,
            info.width,
            info.height,
            if chromeless { " chromeless" } else { "" },
            win_x,
            win_y
        );

        let (content_x, content_y) = if chromeless {
            (win_x, win_y)
        } else {
            (
                win_x + px(BORDER_WIDTH) as i32,
                win_y + px(TITLE_BAR_HEIGHT) as i32 + px(BORDER_WIDTH) as i32,
            )
        };
        let z_order = windows.len() as u32;
        let _ = session::set_geometry(info.id, content_x, content_y, z_order, true);

        let order = *next_order;
        *next_order += 1;
        windows.push(Window {
            x: win_x,
            y: win_y,
            width: win_w,
            height: win_h,
            title,
            title_len,
            window_id: info.id,
            owner_pid: info.owner_pid,
            minimized: false,
            frame_checked: false,
            check_after_ms: 0,
            resize_right: None,
            resize_bottom: None,
            check_moved: 0,
            creation_order: order,
            chromeless,
        });
        added = true;
    }

    removed || added || resized
}

/// Publish each window's place: its content position, its stacking (z from the bottom) and
/// whether it is shown or minimized. Called after any change to them.
pub(super) fn update_kernel_z_order(windows: &[Window]) {
    for (i, win) in windows.iter().enumerate() {
        if win.window_id != 0 {
            let _ = session::set_geometry(
                win.window_id,
                win.content_x(),
                win.content_y(),
                i as u32,
                !win.minimized,
            );
        }
    }
}

/// Redraw the windows in z-order (index 0 = bottom), plus taskbar and app bar.
/// Window frames and decorations go into the desktop target; the compositor draws client
/// content above them from the windows' committed fronts.

impl State {
    pub(super) fn discover(&mut self, scene_changed: bool) {
        // ── 1. Discover new/removed client windows, and see client frames ──
        if scene_changed {
            if discover_windows(
                &mut self.windows,
                self.screen_w,
                self.screen_h,
                &mut self.next_creation_order,
                &mut self.win_defaults,
            ) {
                // Chromeless windows float to top of z-order
                let chromeless_idx = self.windows.iter().position(|w| w.chromeless);
                if let Some(idx) = chromeless_idx {
                    if idx < self.windows.len() - 1 {
                        let win = self.windows.remove(idx);
                        self.windows.push(win);
                    }
                }
                update_kernel_z_order(&self.windows);
                self.focused_win = next_visible_window(&self.windows, 0);
                compose_full_redraw(
                    &mut self.gpu,
                    &self.windows,
                    self.focused_win,
                    &self.clock_text,
                    &mut self.ui_font,
                );
                self.full_redraw = true;
            }
            // A committed client frame is composed without repainting the desktop.
            self.windows_dirty |= self.gpu.pending();
        }
    }
}
