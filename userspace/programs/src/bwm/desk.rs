use super::*;

// ─── Window Defaults (Position/Size Memory) ─────────────────────────────────

pub(super) struct WindowDefaults {
    pub(super) title: [u8; 64],
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
    pub(super) title: [u8; 64],
    pub(super) title_len: usize,
    pub(super) window_id: u64,
    pub(super) owner_pid: u32,
    pub(super) minimized: bool,
    pub(super) resize_pending: Option<(u32, u32)>,
    pub(super) resize_desired: Option<(u32, u32)>,
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
            let title_len = (info.title_len as usize).min(64);
            if w.title_len != title_len || w.title[..title_len] != info.title[..title_len] {
                w.title = info.title;
                w.title_len = title_len;
                resized = true; // A title change repaints chrome and appbar too (W17).
            }
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
                w.resize_pending = None;
                w.width = width;
                w.height = height;
                w.frame_checked = false;
                w.check_moved = 0;
                w.check_after_ms = monotonic_ms().unwrap_or(0) + 150;
                if let Some((width, height)) = w.resize_desired.take().filter(|size| *size != (info.width, info.height)) {
                    w.resize_pending = Some((width, height));
                    send_notice(w.window_id, Input::Resize { width, height });
                }

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
        let mut title = [0u8; 64];
        let title_len = (info.title_len as usize).min(64);
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
        let published = info.flags & libbreenix::window::abi::win_flags::VISIBLE != 0;
        let (win_x, win_y) = if published && (info.x != 0 || info.y != 0 || info.z != 0) {
            (info.x - if chromeless {0} else {px(BORDER_WIDTH) as i32},
             info.y - if chromeless {0} else {(px(TITLE_BAR_HEIGHT)+px(BORDER_WIDTH)) as i32})
        } else { (win_x, win_y) };
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
            minimized: info.flags & libbreenix::window::abi::win_flags::MINIMIZED != 0,
            resize_pending: None,
            resize_desired: None,
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
pub(super) fn update_kernel_z_order(windows: &[Window], focused: usize) {
    for (i, win) in windows.iter().enumerate() {
        if win.window_id != 0 {
            let flags = if win.minimized { libbreenix::window::abi::geometry_flags::MINIMIZED }
                else { libbreenix::window::abi::geometry_flags::VISIBLE }
                | if i == focused && !win.minimized { libbreenix::window::abi::geometry_flags::FOCUSED } else { 0 };
            let _ = libbreenix::window::set_geometry(win.window_id, win.content_x(), win.content_y(), i as u32, flags);
        }
    }
}

/// Redraw the windows in z-order (index 0 = bottom), plus taskbar and app bar.
/// Window frames and decorations go into the desktop target; the compositor draws client
/// content above them from the windows' committed fronts.

impl State {
    pub(super) fn focus(&mut self, id: Option<u64>, raise: bool) {
        let old = self.windows.get(self.focused_win).map(|w| w.window_id);
        let selected = id.filter(|id| self.windows.iter().any(|w| w.window_id == *id && !w.minimized));
        if raise && old == selected && selected.is_some() {
            let last = self.windows.iter().rev().find(|w| !w.minimized && w.chromeless == self.windows[self.focused_win].chromeless).map(|w| w.window_id);
            if last == selected { return; }
        }

        if old != selected {
            if let Some(index) = old.and_then(|id| self.windows.iter().position(|w| w.window_id == id)) {
                send_to(&self.windows, index, Input::FocusLost);
            }
        }
        if raise {
            if let Some(index) = selected.and_then(|id| self.windows.iter().position(|w| w.window_id == id)) {
                let win = self.windows.remove(index);
                let top = if win.chromeless { self.windows.len() } else { self.windows.iter().position(|w| w.chromeless).unwrap_or(self.windows.len()) };
                self.windows.insert(top, win);
            }
        }
        self.focused_win = selected.and_then(|id| self.windows.iter().position(|w| w.window_id == id)).unwrap_or(usize::MAX);
        if old != selected { send_to(&self.windows, self.focused_win, Input::FocusGained); }
        update_kernel_z_order(&self.windows, self.focused_win);
        self.redraw_desk();
    }
    pub(super) fn redraw_desk(&mut self) {
        compose_full_redraw(&mut self.gpu, &self.windows, self.focused_win, &self.clock_text, &mut self.ui_font);
        self.full_redraw = true;
    }
    pub(super) fn minimize(&mut self, id: u64) {
        if let Some(i) = self.windows.iter().position(|w| w.window_id == id) {
            self.windows[i].minimized = true;
            let next = if self.windows.get(self.focused_win).is_some_and(|w| w.window_id == id) {
                self.windows.iter().rev().find(|w| !w.minimized).map(|w| w.window_id)
            } else { self.windows.get(self.focused_win).map(|w| w.window_id) };
            self.focus(next, false);
        }
    }
    pub(super) fn discover(&mut self, scene_changed: bool) {
        if !scene_changed { return; }
        let old = self.windows.get(self.focused_win).map(|w| w.window_id);
        let ids: Vec<_> = self.windows.iter().map(|w| w.window_id).collect();
        if discover_windows(&mut self.windows, self.screen_w, self.screen_h, &mut self.next_creation_order, &mut self.win_defaults) {
            retain_windows(&self.windows);
            if self.dragging.is_some_and(|(id,_,_)| !self.windows.iter().any(|w| w.window_id == id)) { self.dragging = None; }
            if self.resizing.is_some_and(|(id,_,_,_,_,_,_,_)| !self.windows.iter().any(|w| w.window_id == id)) { self.resizing = None; }
            let new = self.windows.iter().rev().find(|w| !ids.contains(&w.window_id) && !w.minimized).map(|w| w.window_id);
            let focus = new.or_else(||old.filter(|id| self.windows.iter().any(|w|w.window_id==*id&&!w.minimized)))
                .or_else(||self.windows.iter().rev().find(|w|!w.minimized).map(|w|w.window_id));
            self.focused_win = old.and_then(|id| self.windows.iter().position(|w| w.window_id == id)).unwrap_or(usize::MAX);
            self.focus(focus, false);
        }
        self.windows_dirty |= self.gpu.pending();
    }
}
