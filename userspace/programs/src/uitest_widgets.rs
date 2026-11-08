//! Retained widget scene shared by the app and its host routing checks.
use crate::uitest_widget_layout as layout;
use libbui::shortcut::Shortcut;
use libbui::tree::{Flag, Node, Role, Tree};
use libbui::widget::{
    button::Button,
    checkbox::Checkbox,
    label::Label,
    list::{List, ListItem},
    menu_bar::{Menu, MenuBar, MenuEvent, MenuItem},
    slider::Slider,
    tab_bar::TabBar,
    text_field::TextField,
    Widget,
};
use libbui::{
    focus::FocusChain, input::PointerCapture, InputState, Key, KeyInput, Rect, Theme, WidgetEvent,
};
#[cfg(target_os = "breenix")]
use libgfx::{color::Color, surface::Surface};
use serde_json::json;

fn rect([x, y, w, h]: [i32; 4]) -> Rect {
    Rect::new(x, y, w, h)
}
const ENTRY: u64 = 0;
const SUBMIT: u64 = 1;
const CHECKBOX: u64 = 3;
const SLIDER: u64 = 4;
const LIST: u64 = 5;
const TABS: u64 = 6;
const MENU: u64 = 7;

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    fg: u32,
    bg: u32,
    bold: bool,
    inverse: bool,
    underline: bool,
}
struct Terminal {
    cells: [[Cell; 4]; 2],
    cursor: [usize; 2],
    cursor_visible: bool,
}
impl Terminal {
    fn new() -> Self {
        let cell = |ch| Cell {
            ch,
            fg: 0xffffff,
            bg: 0,
            bold: false,
            inverse: false,
            underline: false,
        };
        let mut cells = [*b"PASS", *b"grid"].map(|row| row.map(cell));
        cells[0][0].bold = true;
        cells[0][2].fg = 0x00ff00;
        cells[0][3].bg = 0x000080;
        cells[1][0].inverse = true;
        cells[1][1].underline = true;
        Self {
            cells,
            cursor: [1, 2],
            cursor_visible: true,
        }
    }
    fn value(&self) -> serde_json::Value {
        let rows: Vec<_> = self
            .cells
            .iter()
            .map(|row| String::from_utf8(row.iter().map(|c| c.ch).collect()).expect("ASCII grid"))
            .collect();
        let cells: Vec<Vec<_>> = self
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| json!({"fg":c.fg,"bg":c.bg,"bold":c.bold,"inverse":c.inverse,"underline":c.underline}))
                    .collect()
            })
            .collect();
        // This scene has no PTY or parser and retires no rows.
        const EMPTY_SHA256: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        json!({"rows":rows,"cols":4,"cursor":{"row":self.cursor[0],"col":self.cursor[1],"visible":self.cursor_visible},"cells":cells,
            "pty-consumed":0,"parse-errors":0,"stream-digest":EMPTY_SHA256,"rows-retired":0,"rows-digest":EMPTY_SHA256})
    }
    fn bounds(&self) -> Rect {
        Rect::new(
            layout::TERMINAL[0],
            layout::TERMINAL[1],
            self.cells[0].len() as i32 * layout::CELL_WIDTH,
            self.cells.len() as i32 * layout::CELL_HEIGHT,
        )
    }
    #[cfg(target_os = "breenix")]
    fn draw(&self, surface: &mut Surface) {
        let scale = surface.ui_scale() as i32;
        let color = |rgb: u32| Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
        for (row, cells) in self.cells.iter().enumerate() {
            for (col, c) in cells.iter().enumerate() {
                let (fg, bg) = if c.inverse {
                    (c.bg, c.fg)
                } else {
                    (c.fg, c.bg)
                };
                let x = layout::TERMINAL[0] + col as i32 * layout::CELL_WIDTH;
                let y = layout::TERMINAL[1] + row as i32 * layout::CELL_HEIGHT;
                libgfx::shapes::fill_rect(
                    surface,
                    x * scale,
                    y * scale,
                    layout::CELL_WIDTH * scale,
                    layout::CELL_HEIGHT * scale,
                    color(bg),
                );
                libgfx::font::draw_char(
                    surface,
                    c.ch,
                    (x * scale) as usize,
                    ((y + 4) * scale) as usize,
                    color(fg),
                    scale as usize,
                );
                if c.bold {
                    libgfx::font::draw_char(
                        surface,
                        c.ch,
                        ((x + 1) * scale) as usize,
                        ((y + 4) * scale) as usize,
                        color(fg),
                        scale as usize,
                    );
                }
                if c.underline {
                    libgfx::shapes::fill_rect(
                        surface,
                        x * scale,
                        (y + layout::CELL_HEIGHT - 3) * scale,
                        layout::CELL_WIDTH * scale,
                        scale,
                        color(fg),
                    );
                }
                if self.cursor_visible && self.cursor == [row, col] {
                    libgfx::shapes::fill_rect(
                        surface,
                        x * scale,
                        (y + layout::CELL_HEIGHT - 1) * scale,
                        layout::CELL_WIDTH * scale,
                        scale,
                        color(fg),
                    );
                }
            }
        }
    }
}

pub struct Widgets {
    theme: Theme,
    entry: TextField,
    submit: Button,
    disabled: Button,
    accept: Checkbox,
    amount: Slider,
    status: Label,
    menu: MenuBar,
    list: List,
    tabs: TabBar,
    terminal: Terminal,
    focus: FocusChain,
    capture: PointerCapture,
    pointer: (i32, i32),
    submissions: u64,
    menu_action: u16,
}
impl Widgets {
    pub fn new() -> Self {
        let theme = Theme::dark();
        let mut disabled = Button::new(rect(layout::DISABLED), b"Disabled");
        disabled.set_enabled(false);
        let mut amount = Slider::new(rect(layout::SLIDER), 0.0, 100.0);
        amount.value = 0.25;
        let mut menu = MenuBar::new(
            rect(layout::MENU),
            vec![Menu {
                label: b"Actions",
                items: vec![
                    MenuItem::new(b"Choose", 7, Shortcut::NONE),
                    MenuItem::new(b"Reset", 8, Shortcut::NONE),
                ],
            }],
        );
        menu.layout(&theme);
        let mut tabs = TabBar::new(rect(layout::TABS), vec![b"First", b"Second"], &theme);
        tabs.layout(&theme);
        let mut focus = FocusChain::new();
        for bounds in [
            layout::ENTRY,
            layout::SUBMIT,
            layout::DISABLED,
            layout::CHECKBOX,
            layout::SLIDER,
            layout::LIST,
            layout::TABS,
        ] {
            focus.add(rect(bounds));
        }
        focus.set_enabled(2, false);
        focus.focus(ENTRY as usize);
        let mut scene = Self {
            theme,
            entry: TextField::new(rect(layout::ENTRY), 256),
            submit: Button::new(rect(layout::SUBMIT), b"Submit"),
            disabled,
            accept: Checkbox::new(rect(layout::CHECKBOX), b"Accept"),
            amount,
            status: Label::new(rect(layout::STATUS), b"PASS"),
            menu,
            list: List::new(
                rect(layout::LIST),
                layout::ROW_HEIGHT,
                (0..layout::LIST_ITEMS)
                    .map(|i| ListItem::new(&format!("item-{i:02}"), &format!("Item {i:02}")))
                    .collect(),
            ),
            tabs,
            terminal: Terminal::new(),
            focus,
            capture: PointerCapture::default(),
            pointer: (-1, -1),
            submissions: 0,
            menu_action: 0,
        };
        scene.sync_focus();
        scene
    }
    fn sync_focus(&mut self) {
        let f = self.focus.focused();
        self.entry.set_focused(f == Some(ENTRY as usize));
        self.submit.set_focused(f == Some(SUBMIT as usize));
        self.accept.set_focused(f == Some(CHECKBOX as usize));
        self.amount.set_focused(f == Some(SLIDER as usize));
        self.list.set_focused(f == Some(LIST as usize));
        self.tabs.set_focused(f == Some(TABS as usize));
    }
    fn menu_event(&mut self, event: MenuEvent) {
        if let MenuEvent::Activated(action) = event {
            self.menu_action = action;
        }
    }
    pub fn pointer(&mut self, input: &InputState) {
        self.pointer = (input.mouse_x, input.mouse_y);
        let hit =
            if self.menu.is_open() || rect(layout::MENU).contains(input.mouse_x, input.mouse_y) {
                Some(MENU)
            } else {
                [
                    (ENTRY, layout::ENTRY),
                    (SUBMIT, layout::SUBMIT),
                    (CHECKBOX, layout::CHECKBOX),
                    (SLIDER, layout::SLIDER),
                    (LIST, layout::LIST),
                    (TABS, layout::TABS),
                ]
                .into_iter()
                .find(|(_, r)| rect(*r).contains(input.mouse_x, input.mouse_y))
                .map(|(id, _)| id)
            };
        let target = self.capture.route(input, hit);
        if input.mouse_pressed && target != Some(MENU) {
            self.focus.update(input);
            self.sync_focus();
        }
        // Clear a previous hover without forwarding this press/release to a sibling.
        let passive = InputState::from_raw(i32::MIN, i32::MIN, 0, 0, input.time_ms);
        if target != Some(SUBMIT) {
            self.submit.update(&passive);
        }
        if target != Some(CHECKBOX) {
            self.accept.update(&passive);
        }
        if target != Some(TABS) {
            self.tabs.update(&passive);
        }
        match target {
            Some(ENTRY) => {
                self.entry.update(input);
            }
            Some(SUBMIT) => {
                if self.submit.update(input) == WidgetEvent::Clicked {
                    self.submissions += 1;
                }
            }
            Some(CHECKBOX) => {
                self.accept.update(input);
            }
            Some(SLIDER) => {
                self.amount.update(input);
            }
            Some(LIST) => {
                self.list.update(input);
            }
            Some(TABS) => {
                self.tabs.update(input);
            }
            Some(MENU) => {
                let event = self.menu.update(input, &self.theme);
                self.menu_event(event);
            }
            _ => {}
        }
    }
    pub fn cancel_pointer(&mut self) {
        let input = InputState::from_raw(i32::MIN, i32::MIN, 0, 1, 0);
        self.pointer(&input);
        self.capture.cancel();
        self.menu.close();
    }
    pub fn scroll(&mut self, delta_y: i32) {
        if !self.menu.is_open() && rect(layout::LIST).contains(self.pointer.0, self.pointer.1) {
            self.list.scroll(delta_y);
        }
    }
    pub fn key(&mut self, input: &KeyInput<'_>) {
        if self.menu.is_open() {
            let event = self.menu.key(input);
            self.menu_event(event);
            return;
        }
        if input.key == Key::Tab {
            self.focus.key(input);
            self.sync_focus();
            return;
        }
        match self.focus.focused().map(|id| id as u64) {
            Some(ENTRY) => {
                if input.ctrl && input.text.eq_ignore_ascii_case("a") {
                    self.entry.key(&KeyInput::new(Key::Home));
                    self.entry.key(&KeyInput {
                        key: Key::End,
                        shift: true,
                        ..KeyInput::new(Key::End)
                    });
                } else {
                    self.entry.key(input);
                }
            }
            Some(SUBMIT) if matches!(input.key, Key::Enter | Key::Space) => self.submissions += 1,
            Some(CHECKBOX) => {
                self.accept.key(input);
            }
            Some(SLIDER) => {
                self.amount.key(input);
            }
            Some(LIST) => {
                self.list.key(input);
            }
            Some(TABS) => {
                let i = self.tabs.selected();
                match input.key {
                    Key::Left => {
                        self.tabs.set_selected(i.saturating_sub(1));
                    }
                    Key::Right => {
                        self.tabs.set_selected((i + 1).min(self.tabs.count() - 1));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    pub fn tree(&self) -> Tree {
        let clip = Rect::new(0, 0, layout::WIDTH as i32, layout::HEIGHT as i32);
        let root = Node {
            key: "root".into(),
            parent: None,
            role: Role::Window,
            name: "Widgets".into(),
            value: json!({"submissions":self.submissions,"menu_action":self.menu_action}),
            bounds: clip.as_tree_rect(),
            clip: clip.as_tree_rect(),
            flags: vec![Flag::Enabled, Flag::Visible],
        };
        let mut children = vec![
            self.entry.tree_node("entry", Some("root"), clip),
            self.submit.tree_node("submit", Some("root"), clip),
            self.disabled.tree_node("disabled", Some("root"), clip),
            self.accept.tree_node("accept", Some("root"), clip),
            self.amount.tree_node("amount", Some("root"), clip),
            self.status.tree_node("status", Some("root"), clip),
        ];
        children.extend(
            self.menu
                .tree_nodes("menu", Some("root"), clip, &self.theme),
        );
        children.extend(
            self.list
                .tree_nodes("list", Some("root"), clip, &self.theme),
        );
        children.extend(
            self.tabs
                .tree_nodes("tabs", Some("root"), clip, &self.theme),
        );
        children.push(Node {
            key: "terminal".into(),
            parent: Some("root".into()),
            role: Role::Terminal,
            name: "Fixed grid".into(),
            value: self.terminal.value(),
            bounds: self.terminal.bounds().as_tree_rect(),
            clip: self.terminal.bounds().intersection(clip).as_tree_rect(),
            flags: vec![Flag::Enabled, Flag::Visible],
        });
        Tree { root, children }
    }
    /// Window tree coordinates use device pixels, as do BreenGL pointer events.
    pub fn committed_tree(&self, scale: u32) -> Tree {
        let mut tree = self.tree();
        for n in std::iter::once(&mut tree.root).chain(&mut tree.children) {
            n.bounds = n.bounds.map(|v| v * i64::from(scale));
            n.clip = n.clip.map(|v| v * i64::from(scale));
        }
        tree
    }
    #[cfg(target_os = "breenix")]
    pub fn draw(&self, surface: &mut Surface) {
        surface.clear(self.theme.panel_bg);
        self.entry.draw(surface, &self.theme);
        self.submit.draw(surface, &self.theme);
        self.disabled.draw(surface, &self.theme);
        self.accept.draw(surface, &self.theme);
        self.amount.draw(surface, &self.theme);
        self.list.draw(surface, &self.theme);
        self.tabs.draw(surface, &self.theme);
        self.status.draw(surface, &self.theme);
        self.terminal.draw(surface);
        self.menu.draw(surface, &self.theme);
    }
}
