//! Windows-style menu bar widget with pull-down dropdown menus.

use alloc::vec::Vec;
use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

use crate::input::InputState;
use crate::rect::Rect;
use crate::shortcut::Shortcut;
use crate::text;
use crate::theme::Theme;

// ── Layout constants ──────────────────────────────────────────────────────────

/// Height of the menu bar strip.
pub const MENU_BAR_HEIGHT: i32 = 22;

/// Horizontal padding on each side of a top-level menu label.
const TOP_LABEL_PAD_X: i32 = 10;

/// Height of a normal dropdown item row.
const ITEM_HEIGHT: i32 = 22;

/// Height of a separator row.
const SEPARATOR_HEIGHT: i32 = 9;

/// Horizontal padding inside each dropdown item row (left of label, right of shortcut).
const ITEM_PAD_X: i32 = 24;

/// Extra right margin for the shortcut column.
const SHORTCUT_MARGIN_RIGHT: i32 = 8;

/// Minimum gap between item label and shortcut text.
const LABEL_SHORTCUT_GAP: i32 = 24;

/// Width reserved for the check-mark / bullet column.
const CHECK_AREA_WIDTH: i32 = 18;

/// Minimum width of any dropdown.
const MIN_DROPDOWN_WIDTH: i32 = 140;

// ── Public action type ────────────────────────────────────────────────────────

/// Opaque action ID returned when a menu item is activated.
pub type MenuAction = u16;

/// Sentinel for "no action" (separators and placeholders).
pub const NO_ACTION: MenuAction = 0;

// ── MenuItem ─────────────────────────────────────────────────────────────────

/// A single entry in a dropdown menu.
#[derive(Clone, Copy)]
pub struct MenuItem {
    pub label: &'static [u8],
    pub action: MenuAction,
    pub shortcut: Shortcut,
    pub enabled: bool,
    pub checked: bool,
    pub separator: bool,
}

impl MenuItem {
    /// Normal, enabled menu item.
    pub const fn new(
        label: &'static [u8],
        action: MenuAction,
        shortcut: Shortcut,
    ) -> Self {
        Self {
            label,
            action,
            shortcut,
            enabled: true,
            checked: false,
            separator: false,
        }
    }

    /// Disabled (greyed-out) menu item.
    pub const fn disabled(
        label: &'static [u8],
        action: MenuAction,
        shortcut: Shortcut,
    ) -> Self {
        Self {
            label,
            action,
            shortcut,
            enabled: false,
            checked: false,
            separator: false,
        }
    }

    /// Horizontal separator line.
    pub const fn separator() -> Self {
        Self {
            label: b"",
            action: NO_ACTION,
            shortcut: Shortcut::NONE,
            enabled: false,
            checked: false,
            separator: true,
        }
    }

    /// Checkable item (shows a tick when `checked == true`).
    pub const fn checkable(
        label: &'static [u8],
        action: MenuAction,
        shortcut: Shortcut,
        checked: bool,
    ) -> Self {
        Self {
            label,
            action,
            shortcut,
            enabled: true,
            checked,
            separator: false,
        }
    }
}

// ── Menu ─────────────────────────────────────────────────────────────────────

/// A top-level menu (label + list of items).
pub struct Menu {
    pub label: &'static [u8],
    pub items: Vec<MenuItem>,
}

// ── MenuEvent ─────────────────────────────────────────────────────────────────

/// Events returned by `MenuBar::update`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuEvent {
    /// Nothing happened this frame.
    None,
    /// A menu item was activated; payload is the item's `action` ID.
    Activated(MenuAction),
}

// ── Internal state machine ────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuState {
    /// No dropdown open, mouse is not over any header.
    Closed,
    /// No dropdown open, mouse is hovering a header (click-to-open mode).
    Idle,
    /// A dropdown is open.
    Open,
}

// ── MenuBar ───────────────────────────────────────────────────────────────────

/// Windows-style horizontal menu bar with pull-down dropdowns.
pub struct MenuBar {
    rect: Rect,
    menus: Vec<Menu>,
    state: MenuState,
    /// Which top-level menu is currently open (only valid when state == Open).
    open_index: Option<usize>,
    /// Which dropdown item is hovered (index into the open menu's items).
    hover_item: Option<usize>,
    /// Pre-computed bounding rects of each top-level label button.
    top_rects: Vec<Rect>,
    /// After a click opens the menu we enter "click mode": the first mouse
    /// *release* that occurs while still inside a header or item counts.
    /// Until then we ignore release events (drag-to-select is not supported).
    click_mode: bool,
}

impl MenuBar {
    /// Construct a new `MenuBar`.
    ///
    /// `top_rects` are approximated immediately using 6 px per character so
    /// the widget is usable before `layout()` is called with a real theme.
    pub fn new(rect: Rect, menus: Vec<Menu>) -> Self {
        let top_rects = Self::compute_top_rects_approx(&rect, &menus);
        Self {
            rect,
            menus,
            state: MenuState::Closed,
            open_index: None,
            hover_item: None,
            top_rects,
            click_mode: false,
        }
    }

    /// Recompute top-level label rects using actual font metrics from `theme`.
    /// Call this once after construction (and after any font change).
    pub fn layout(&mut self, theme: &Theme) {
        self.top_rects = Self::compute_top_rects_exact(&self.rect, &self.menus, theme);
    }

    /// Update the bar's bounding rectangle (e.g. after a window resize).
    pub fn set_rect(&mut self, rect: Rect) {
        self.rect = rect;
        self.top_rects = Self::compute_top_rects_approx(&rect, &self.menus);
    }

    /// Fixed height of the menu bar strip.
    pub fn height() -> i32 {
        MENU_BAR_HEIGHT
    }

    /// Returns `true` if a dropdown is currently visible.
    pub fn is_open(&self) -> bool {
        self.state == MenuState::Open
    }

    /// Force-close any open dropdown.
    pub fn close(&mut self) {
        self.state = MenuState::Closed;
        self.open_index = None;
        self.hover_item = None;
        self.click_mode = false;
    }

    /// Set the enabled state of the item with the given action ID.
    pub fn set_enabled(&mut self, action: MenuAction, enabled: bool) {
        for menu in &mut self.menus {
            for item in &mut menu.items {
                if item.action == action {
                    item.enabled = enabled;
                    return;
                }
            }
        }
    }

    /// Set the checked state of the item with the given action ID.
    pub fn set_checked(&mut self, action: MenuAction, checked: bool) {
        for menu in &mut self.menus {
            for item in &mut menu.items {
                if item.action == action {
                    item.checked = checked;
                    return;
                }
            }
        }
    }

    /// Match a keyboard event against all enabled items' shortcuts.
    ///
    /// When `ctrl == true` the terminal may deliver control bytes (1-26) for
    /// Ctrl+A…Ctrl+Z instead of the raw letter.  We normalise those back to
    /// uppercase letters before comparing.
    pub fn match_shortcut(
        &self,
        ascii: u8,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> Option<MenuAction> {
        // Normalise Ctrl+letter control bytes.
        let key = if ctrl && ascii >= 1 && ascii <= 26 {
            ascii + b'A' - 1
        } else if ascii >= b'a' && ascii <= b'z' {
            ascii - 32
        } else {
            ascii
        };

        for menu in &self.menus {
            for item in &menu.items {
                if !item.enabled || !item.shortcut.is_some() {
                    continue;
                }
                let sc = &item.shortcut;
                let sc_key = if sc.key >= b'a' && sc.key <= b'z' {
                    sc.key - 32
                } else {
                    sc.key
                };
                if sc_key == key && sc.ctrl == ctrl && sc.shift == shift && sc.alt == alt {
                    return Some(item.action);
                }
            }
        }
        None
    }

    /// Process mouse input.  Call every frame.
    pub fn update(&mut self, input: &InputState, theme: &Theme) -> MenuEvent {
        let mx = input.mouse_x;
        let my = input.mouse_y;

        match self.state {
            // ── Closed / Idle ─────────────────────────────────────────────
            MenuState::Closed | MenuState::Idle => {
                let top_hit = self.hit_top(mx, my);
                self.state = if top_hit.is_some() {
                    MenuState::Idle
                } else {
                    MenuState::Closed
                };

                if input.mouse_pressed {
                    if let Some(idx) = top_hit {
                        self.open_index = Some(idx);
                        self.hover_item = None;
                        self.state = MenuState::Open;
                        self.click_mode = true;
                    }
                }
            }

            // ── Open ─────────────────────────────────────────────────────
            MenuState::Open => {
                let menu_idx = match self.open_index {
                    Some(i) => i,
                    None => {
                        self.close();
                        return MenuEvent::None;
                    }
                };

                let top_hit = self.hit_top(mx, my);
                let item_hit = self.hit_dropdown_item(menu_idx, mx, my, theme);

                // Hover tracking inside the dropdown.
                self.hover_item = item_hit;

                // Hover over a different top-level menu: switch to it.
                if let Some(new_idx) = top_hit {
                    if new_idx != menu_idx && input.mouse_down {
                        self.open_index = Some(new_idx);
                        self.hover_item = None;
                        // Remain in click_mode.
                        return MenuEvent::None;
                    }
                    // Mouse is still over a header: switch on press.
                    if input.mouse_pressed && new_idx != menu_idx {
                        self.open_index = Some(new_idx);
                        self.hover_item = None;
                        return MenuEvent::None;
                    }
                }

                // Click outside everything: close.
                let dd_rect = self.dropdown_rect(menu_idx, theme);
                let over_top = self.rect.contains(mx, my);
                let over_dd = dd_rect.contains(mx, my);

                if input.mouse_pressed && !over_top && !over_dd {
                    self.close();
                    return MenuEvent::None;
                }

                // Activate item on release (after click_mode settles).
                if input.mouse_released {
                    // In click_mode the first release just arms the widget
                    // (unless the cursor moved onto a real item already).
                    if self.click_mode {
                        self.click_mode = false;
                        // If released on an item immediately, activate it.
                        if let Some(item_idx) = item_hit {
                            return self.activate_item(menu_idx, item_idx);
                        }
                        // Released on a header or outside: stay open.
                        return MenuEvent::None;
                    }

                    if let Some(item_idx) = item_hit {
                        return self.activate_item(menu_idx, item_idx);
                    }

                    // Released outside: close.
                    if !over_top && !over_dd {
                        self.close();
                    }
                }
            }
        }

        MenuEvent::None
    }

    /// Draw the menu bar and any open dropdown.
    pub fn draw(&self, fb: &mut FrameBuf, theme: &Theme) {
        // ── Bar strip ─────────────────────────────────────────────────────
        shapes::fill_rect(
            fb,
            self.rect.x,
            self.rect.y,
            self.rect.w,
            self.rect.h,
            theme.panel_bg,
        );
        // 1 px bottom border.
        shapes::fill_rect(
            fb,
            self.rect.x,
            self.rect.y + self.rect.h - 1,
            self.rect.w,
            1,
            theme.border,
        );

        // ── Top-level labels ──────────────────────────────────────────────
        for (i, menu) in self.menus.iter().enumerate() {
            if i >= self.top_rects.len() {
                break;
            }
            let r = &self.top_rects[i];
            let is_open = self.open_index == Some(i) && self.state == MenuState::Open;
            let is_hover = self.state == MenuState::Idle
                && self.hit_top(r.x + r.w / 2, r.y + r.h / 2) == Some(i);

            let bg = if is_open {
                theme.widget_bg_active
            } else if is_hover {
                theme.widget_bg_hover
            } else {
                theme.panel_bg
            };
            shapes::fill_rect(fb, r.x, r.y, r.w, r.h, bg);

            let text_color = if is_open {
                Color::WHITE
            } else {
                theme.text_primary
            };
            let tw = text::text_width(menu.label, theme);
            let th = text::text_height(theme);
            let tx = r.x + (r.w - tw) / 2;
            let ty = r.y + (r.h - th) / 2;
            text::draw_text(fb, menu.label, tx, ty, text_color, theme);
        }

        // ── Dropdown ──────────────────────────────────────────────────────
        if self.state == MenuState::Open {
            if let Some(menu_idx) = self.open_index {
                if menu_idx < self.menus.len() {
                    self.draw_dropdown(fb, menu_idx, theme);
                }
            }
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn compute_top_rects_approx(rect: &Rect, menus: &[Menu]) -> Vec<Rect> {
        let mut x = rect.x;
        let mut rects = Vec::with_capacity(menus.len());
        for menu in menus {
            let label_w = menu.label.len() as i32 * 6;
            let w = label_w + TOP_LABEL_PAD_X * 2;
            rects.push(Rect::new(x, rect.y, w, MENU_BAR_HEIGHT));
            x += w;
        }
        rects
    }

    fn compute_top_rects_exact(rect: &Rect, menus: &[Menu], theme: &Theme) -> Vec<Rect> {
        let mut x = rect.x;
        let mut rects = Vec::with_capacity(menus.len());
        for menu in menus {
            let label_w = text::text_width(menu.label, theme);
            let w = label_w + TOP_LABEL_PAD_X * 2;
            rects.push(Rect::new(x, rect.y, w, MENU_BAR_HEIGHT));
            x += w;
        }
        rects
    }

    /// Compute the required width of the dropdown for the given menu.
    fn compute_dropdown_width(&self, menu_index: usize, theme: &Theme) -> i32 {
        let menu = &self.menus[menu_index];
        let mut max_w = MIN_DROPDOWN_WIDTH;
        for item in &menu.items {
            if item.separator {
                continue;
            }
            let label_w = text::text_width(item.label, theme);
            let mut row_w = CHECK_AREA_WIDTH + ITEM_PAD_X + label_w;
            if item.shortcut.is_some() {
                let mut buf = [0u8; 24];
                let n = item.shortcut.format(&mut buf);
                let sc_w = text::text_width(&buf[..n], theme);
                row_w += LABEL_SHORTCUT_GAP + sc_w + SHORTCUT_MARGIN_RIGHT;
            } else {
                row_w += ITEM_PAD_X;
            }
            if row_w > max_w {
                max_w = row_w;
            }
        }
        max_w
    }

    /// Bounding rect of the dropdown panel for the given menu.
    fn dropdown_rect(&self, menu_index: usize, theme: &Theme) -> Rect {
        let menu = &self.menus[menu_index];
        let w = self.compute_dropdown_width(menu_index, theme);
        let h: i32 = menu
            .items
            .iter()
            .map(|it| if it.separator { SEPARATOR_HEIGHT } else { ITEM_HEIGHT })
            .sum();

        let anchor = if menu_index < self.top_rects.len() {
            &self.top_rects[menu_index]
        } else {
            &self.rect
        };
        Rect::new(anchor.x, self.rect.y + MENU_BAR_HEIGHT, w, h)
    }

    /// Return the top-level menu index that the point (px, py) hits, or None.
    fn hit_top(&self, px: i32, py: i32) -> Option<usize> {
        if !self.rect.contains(px, py) {
            return None;
        }
        for (i, r) in self.top_rects.iter().enumerate() {
            if r.contains(px, py) {
                return Some(i);
            }
        }
        None
    }

    /// Return the dropdown item index hit by (px, py), or None.
    fn hit_dropdown_item(
        &self,
        menu_index: usize,
        px: i32,
        py: i32,
        theme: &Theme,
    ) -> Option<usize> {
        let dd = self.dropdown_rect(menu_index, theme);
        if !dd.contains(px, py) {
            return None;
        }
        let menu = &self.menus[menu_index];
        let mut y = dd.y;
        for (i, item) in menu.items.iter().enumerate() {
            let row_h = if item.separator { SEPARATOR_HEIGHT } else { ITEM_HEIGHT };
            let row = Rect::new(dd.x, y, dd.w, row_h);
            if row.contains(px, py) {
                if item.separator || !item.enabled {
                    return None;
                }
                return Some(i);
            }
            y += row_h;
        }
        None
    }

    /// Activate item at `item_idx` inside the menu at `menu_idx`. Closes the
    /// dropdown and returns the appropriate `MenuEvent`.
    fn activate_item(&mut self, menu_idx: usize, item_idx: usize) -> MenuEvent {
        let action = self.menus[menu_idx].items[item_idx].action;
        self.close();
        if action != NO_ACTION {
            MenuEvent::Activated(action)
        } else {
            MenuEvent::None
        }
    }

    /// Draw the open dropdown panel.
    fn draw_dropdown(&self, fb: &mut FrameBuf, menu_idx: usize, theme: &Theme) {
        let dd = self.dropdown_rect(menu_idx, theme);
        let menu = &self.menus[menu_idx];

        // Background fill.
        shapes::fill_rect(fb, dd.x, dd.y, dd.w, dd.h, theme.widget_bg);

        // 1 px border around the whole dropdown.
        shapes::draw_rect(fb, dd.x, dd.y, dd.w, dd.h, theme.border);

        let mut y = dd.y;
        for (i, item) in menu.items.iter().enumerate() {
            if item.separator {
                // Draw a 1 px line vertically centred in SEPARATOR_HEIGHT.
                let sep_y = y + SEPARATOR_HEIGHT / 2;
                shapes::fill_rect(
                    fb,
                    dd.x + 1,
                    sep_y,
                    dd.w - 2,
                    1,
                    theme.border,
                );
                y += SEPARATOR_HEIGHT;
                continue;
            }

            let row = Rect::new(dd.x, y, dd.w, ITEM_HEIGHT);
            let is_hover = self.hover_item == Some(i);

            // Row background.
            if is_hover && item.enabled {
                shapes::fill_rect(fb, row.x, row.y, row.w, row.h, theme.widget_bg_active);
            }

            let text_color = if !item.enabled {
                theme.text_secondary
            } else if is_hover {
                Color::WHITE
            } else {
                theme.text_primary
            };

            let th = text::text_height(theme);
            let ty = row.y + (ITEM_HEIGHT - th) / 2;

            // Check mark.
            if item.checked {
                let cx = dd.x + (CHECK_AREA_WIDTH - text::text_width(b"*", theme)) / 2;
                text::draw_text(fb, b"*", cx, ty, text_color, theme);
            }

            // Item label.
            let label_x = dd.x + CHECK_AREA_WIDTH;
            text::draw_text(fb, item.label, label_x, ty, text_color, theme);

            // Shortcut (right-aligned).
            if item.shortcut.is_some() {
                let mut buf = [0u8; 24];
                let n = item.shortcut.format(&mut buf);
                let sc_slice = &buf[..n];
                let sc_w = text::text_width(sc_slice, theme);
                let sc_x = dd.x + dd.w - sc_w - SHORTCUT_MARGIN_RIGHT;
                text::draw_text(fb, sc_slice, sc_x, ty, text_color, theme);
            }

            y += ITEM_HEIGHT;
        }
    }
}
