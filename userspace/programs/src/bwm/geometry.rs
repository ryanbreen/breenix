use super::*;

// ─── Resize Edge ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Window {
    pub(super) fn content_x(&self) -> i32 {
        if self.chromeless {
            self.x
        } else {
            self.x + px(BORDER_WIDTH) as i32
        }
    }
    pub(super) fn content_y(&self) -> i32 {
        if self.chromeless {
            self.y
        } else {
            self.y + px(TITLE_BAR_HEIGHT) as i32 + px(BORDER_WIDTH) as i32
        }
    }
    pub(super) fn content_width(&self) -> usize {
        if self.chromeless {
            self.width
        } else {
            self.width.saturating_sub(px(BORDER_WIDTH) * 2)
        }
    }
    pub(super) fn content_height(&self) -> usize {
        if self.chromeless {
            self.height
        } else {
            self.height
                .saturating_sub(px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2)
        }
    }

    pub(super) fn total_height(&self) -> usize {
        self.height
    }

    pub(super) fn hit_title(&self, mx: i32, my: i32) -> bool {
        if self.chromeless {
            return false;
        }
        mx >= self.x
            && mx < self.x + self.width as i32
            && my >= self.y
            && my < self.y + px(TITLE_BAR_HEIGHT) as i32
    }

    pub(super) fn hit_any(&self, mx: i32, my: i32) -> bool {
        mx >= self.x
            && mx < self.x + self.width as i32
            && my >= self.y
            && my < self.y + self.total_height() as i32
    }

    pub(super) fn hit_content(&self, mx: i32, my: i32) -> bool {
        mx >= self.content_x()
            && mx < self.content_x() + self.content_width() as i32
            && my >= self.content_y()
            && my < self.content_y() + self.content_height() as i32
    }

    pub(super) fn bounds(&self) -> (i32, i32, i32, i32) {
        if self.chromeless {
            (
                self.x,
                self.y,
                self.x + self.width as i32,
                self.y + self.height as i32,
            )
        } else {
            // The drop shadow is drawn SHADOW_OFFSET right of and below the frame (draw_window_frame)
            let shadow = px(SHADOW_OFFSET) as i32;
            (
                self.x,
                self.y,
                self.x + self.width as i32 + shadow,
                self.y + self.total_height() as i32 + shadow,
            )
        }
    }

    pub(super) fn close_btn_rect(&self) -> (i32, i32, usize, usize) {
        let bx = self.x + self.width as i32
            - px(BORDER_WIDTH) as i32
            - px(CHROME_BTN_PAD) as i32
            - px(CHROME_BTN_SIZE) as i32;
        let by = self.y
            + px(BORDER_WIDTH) as i32
            + ((px(TITLE_BAR_HEIGHT) - px(BORDER_WIDTH) - px(CHROME_BTN_SIZE)) / 2) as i32;
        (bx, by, px(CHROME_BTN_SIZE), px(CHROME_BTN_SIZE))
    }

    pub(super) fn minimize_btn_rect(&self) -> (i32, i32, usize, usize) {
        let (cx, cy, _, _) = self.close_btn_rect();
        (
            cx - px(CHROME_BTN_PAD) as i32 - px(CHROME_BTN_SIZE) as i32,
            cy,
            px(CHROME_BTN_SIZE),
            px(CHROME_BTN_SIZE),
        )
    }

    pub(super) fn hit_close_button(&self, mx: i32, my: i32) -> bool {
        let (bx, by, bw, bh) = self.close_btn_rect();
        mx >= bx && mx < bx + bw as i32 && my >= by && my < by + bh as i32
    }

    pub(super) fn hit_minimize_button(&self, mx: i32, my: i32) -> bool {
        let (bx, by, bw, bh) = self.minimize_btn_rect();
        mx >= bx && mx < bx + bw as i32 && my >= by && my < by + bh as i32
    }

    pub(super) fn hit_resize_edge(&self, mx: i32, my: i32) -> Option<ResizeEdge> {
        if self.minimized || self.chromeless {
            return None;
        }

        let x0 = self.x;
        let y0 = self.y;
        let x1 = self.x + self.width as i32;
        let y1 = self.y + self.total_height() as i32;
        let g = px(RESIZE_GRAB) as i32;

        // Must be within or near the window bounds
        if mx < x0 - g || mx >= x1 + g || my < y0 - g || my >= y1 + g {
            return None;
        }

        let on_left = mx < x0 + g;
        let on_right = mx >= x1 - g;
        let on_top = my < y0 + g;
        let on_bottom = my >= y1 - g;

        match (on_left, on_right, on_top, on_bottom) {
            (true, _, true, _) => Some(ResizeEdge::TopLeft),
            (_, true, true, _) => Some(ResizeEdge::TopRight),
            (true, _, _, true) => Some(ResizeEdge::BottomLeft),
            (_, true, _, true) => Some(ResizeEdge::BottomRight),
            (true, _, _, _) => Some(ResizeEdge::Left),
            (_, true, _, _) => Some(ResizeEdge::Right),
            (_, _, true, _) => Some(ResizeEdge::Top),
            (_, _, _, true) => Some(ResizeEdge::Bottom),
            _ => None,
        }
    }
}

pub(super) struct Damage {
    pub(super) dirty_x0: i32,
    pub(super) dirty_y0: i32,
    pub(super) dirty_x1: i32,
    pub(super) dirty_y1: i32,
    pub(super) cursor_dirty: bool,
    pub(super) mouse_moved_this_frame: bool,
}
impl Damage {
    pub(super) fn new() -> Self {
        Self {
            dirty_x0: i32::MAX,
            dirty_y0: i32::MAX,
            dirty_x1: 0,
            dirty_y1: 0,
            cursor_dirty: false,
            mouse_moved_this_frame: false,
        }
    }
}

impl State {
    pub(super) fn drag_resize(&mut self, damage: &mut Damage) {
                if let Some((win_idx, off_x, off_y)) = self.dragging {
                    let new_x = self.mouse_x - off_x;
                    // Clamp drag to stay below taskbar
                    let new_y = (self.mouse_y - off_y).max(px(TASKBAR_HEIGHT) as i32);
                    if new_x != self.windows[win_idx].x || new_y != self.windows[win_idx].y {
                        // Capture old bounds before moving
                        let (ox0, oy0, ox1, oy1) = self.windows[win_idx].bounds();
                        self.windows[win_idx].resize_right = None;
                        self.windows[win_idx].resize_bottom = None;
                        self.windows[win_idx].x = new_x;
                        self.windows[win_idx].y = new_y;
                        // Publish the window's new place for composition
                        if self.windows[win_idx].window_id != 0 {
                            let cx = self.windows[win_idx].content_x();
                            let cy = self.windows[win_idx].content_y();
                            let _ = session::set_geometry(
                                self.windows[win_idx].window_id,
                                cx,
                                cy,
                                win_idx as u32,
                                true,
                            );
                        }
                        // Dirty region = union of old and new bounds
                        let (nx0, ny0, nx1, ny1) = self.windows[win_idx].bounds();
                        let dr_x0 = ox0.min(nx0).max(0) as usize;
                        let dr_y0 = oy0.min(ny0).max(0) as usize;
                        let dr_x1 = ox1.max(nx1) as usize;
                        let dr_y1 = oy1.max(ny1) as usize;
                        compose_partial_redraw(
                            &mut self.gpu,
                            &self.windows,
                            self.focused_win,
                            &self.clock_text,
                            dr_x0,
                            dr_y0,
                            dr_x1,
                            dr_y1,
                            &mut self.ui_font,
                        );
                        damage.dirty_x0 = damage.dirty_x0.min(dr_x0 as i32);
                        damage.dirty_y0 = damage.dirty_y0.min(dr_y0 as i32);
                        damage.dirty_x1 = damage.dirty_x1.max(dr_x1 as i32);
                        damage.dirty_y1 = damage.dirty_y1.max(dr_y1 as i32);
                        self.content_dirty = true;
                    }
                } else if let Some((win_idx, edge, anchor_x, anchor_y, _, orig_y, orig_w, orig_h)) =
                    self.resizing
                {
                    let dx = self.mouse_x - anchor_x;
                    let dy = self.mouse_y - anchor_y;

                    let mut new_w = orig_w as i32;
                    let mut new_h = orig_h as i32;
                    match edge {
                        ResizeEdge::Right => {
                            new_w += dx;
                        }
                        ResizeEdge::Bottom => {
                            new_h += dy;
                        }
                        ResizeEdge::Left => {
                            new_w -= dx;
                        }
                        ResizeEdge::Top => {
                            new_h -= dy;
                        }
                        ResizeEdge::TopLeft => {
                            new_w -= dx;
                            new_h -= dy;
                        }
                        ResizeEdge::TopRight => {
                            new_w += dx;
                            new_h -= dy;
                        }
                        ResizeEdge::BottomLeft => {
                            new_w -= dx;
                            new_h += dy;
                        }
                        ResizeEdge::BottomRight => {
                            new_w += dx;
                            new_h += dy;
                        }
                    }
                    new_w = new_w.max(px(MIN_WIN_WIDTH) as i32);
                    new_h = new_h.max(px(MIN_WIN_HEIGHT) as i32);
                    if self.windows[win_idx].resize_bottom.is_some() {
                        new_h = new_h.min(orig_y + orig_h as i32 - px(TASKBAR_HEIGHT) as i32);
                    }
                    // B41: neither position nor dimensions change until LIST reports
                    // the owner's answer. LIST preserves the opposite edge atomically.
                    let width = new_w
                        .saturating_sub((px(BORDER_WIDTH) * 2) as i32)
                        .clamp(1, 4096) as u32;
                    let height = new_h
                        .saturating_sub((px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2) as i32)
                        .clamp(1, 4096) as u32;
                    send_to(&self.windows, win_idx, Input::Resize { width, height });
                }

    }
}
