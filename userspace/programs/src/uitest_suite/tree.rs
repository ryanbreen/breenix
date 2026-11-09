//! Guest checks over committed widget trees, with independent literal expectations.
use libbui::tree::{Flag, Node, Role, Tree};
#[path = "widget_literals.rs"]
pub mod literals;

pub fn node<'a>(tree: &'a Tree, key: &str) -> Result<&'a Node, String> {
    let mut found = std::iter::once(&tree.root)
        .chain(&tree.children)
        .filter(|n| n.key == key);
    let n = found
        .next()
        .ok_or_else(|| format!("tree-mismatch node={key} field=key missing"))?;
    if found.next().is_some() {
        return Err(format!("tree-mismatch node={key} field=key duplicate"));
    }
    Ok(n)
}
fn equal<T: std::fmt::Debug + PartialEq>(
    key: &str,
    field: &str,
    got: T,
    want: T,
) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "tree-mismatch node={key} field={field} expected={want:?} got={got:?}"
        ))
    }
}
pub fn expect_roles_keys(tree: &Tree, scale: u32) -> Result<(), String> {
    use literals::*;
    let mut expected = vec![("root".to_string(), Role::Window)];
    for (key, role) in [
        ("entry", Role::Textfield),
        ("submit", Role::Button),
        ("disabled", Role::Button),
        ("accept", Role::Checkbox),
        ("amount", Role::Slider),
        ("status", Role::Label),
        ("menu", Role::Menubar),
        ("menu/0", Role::Menu),
        ("menu/0/0", Role::Menuitem),
        ("menu/0/1", Role::Menuitem),
        ("list", Role::List),
    ] {
        expected.push((key.into(), role));
    }
    for i in 0..LIST_ITEMS {
        expected.push((format!("list/item-{i:02}"), Role::Listitem));
    }
    expected.extend([
        ("tabs".into(), Role::Tablist),
        ("tabs/0".into(), Role::Tab),
        ("tabs/1".into(), Role::Tab),
        ("terminal".into(), Role::Terminal),
    ]);
    let got: Vec<_> = std::iter::once(&tree.root)
        .chain(&tree.children)
        .map(|n| (n.key.clone(), n.role))
        .collect();
    equal("root", "roles-keys", got, expected)?;
    equal(
        "root",
        "truncated",
        tree.root.flags.contains(&Flag::Truncated),
        false,
    )?;
    for (key, rect) in [
        ("root", [0, 0, WIDTH as i32, HEIGHT as i32]),
        ("entry", ENTRY),
        ("submit", SUBMIT),
        ("disabled", DISABLED),
        ("accept", CHECKBOX),
        ("amount", SLIDER),
        ("status", STATUS),
        ("menu", MENU),
        ("list", LIST),
        ("tabs", TABS),
        ("terminal", TERMINAL),
    ] {
        equal(
            key,
            "bounds",
            node(tree, key)?.bounds,
            rect.map(|v| i64::from(v) * i64::from(scale)),
        )?;
    }
    Ok(())
}
pub fn expect_names_values(tree: &Tree, scripted: bool) -> Result<(), String> {
    use serde_json::json;
    for (key, name, value) in [
        ("entry", "", json!(if scripted { "hi" } else { "" })),
        ("submit", "Submit", json!(null)),
        ("disabled", "Disabled", json!(null)),
        ("accept", "Accept", json!(scripted)),
        ("amount", "", json!(if scripted { 50.0 } else { 25.0 })),
        ("status", "PASS", json!(null)),
        ("menu/0", "Actions", json!(null)),
        ("menu/0/0", "Choose", json!(7)),
        ("menu/0/1", "Reset", json!(8)),
        ("tabs", "", json!(if scripted { 1 } else { 0 })),
        ("tabs/0", "First", json!(null)),
        ("tabs/1", "Second", json!(null)),
        (
            "list",
            "",
            if scripted {
                json!("item-01")
            } else {
                json!(null)
            },
        ),
    ] {
        let n = node(tree, key)?;
        equal(key, "name", n.name.as_str(), name)?;
        equal(key, "value", &n.value, &value)?;
    }
    for i in 0..literals::LIST_ITEMS {
        let key = format!("list/item-{i:02}");
        equal(
            &key,
            "name",
            node(tree, &key)?.name.clone(),
            format!("Item {i:02}"),
        )?;
        equal(&key, "value", &node(tree, &key)?.value, &json!(null))?;
    }
    if scripted {
        equal(
            "root",
            "value",
            &tree.root.value,
            &json!({"submissions":1,"menu_action":7}),
        )?;
    }
    Ok(())
}
pub fn expect_enabled(tree: &Tree) -> Result<(), String> {
    for key in ["entry", "submit", "accept", "amount", "list", "tabs"] {
        equal(
            key,
            "enabled",
            node(tree, key)?.flags.contains(&Flag::Enabled),
            true,
        )?;
    }
    equal(
        "disabled",
        "enabled",
        node(tree, "disabled")?.flags.contains(&Flag::Enabled),
        false,
    )
}
pub fn expect_hidden(tree: &Tree, offset: i32, scale: u32) -> Result<(), String> {
    use literals::*;
    for i in 0..LIST_ITEMS {
        let key = format!("list/item-{i:02}");
        let n = node(tree, &key)?;
        let y = LIST[1] + i as i32 * ROW_HEIGHT - offset;
        let top = y.max(LIST[1]);
        let height = (y + ROW_HEIGHT)
            .min(LIST[1] + LIST[3])
            .saturating_sub(top)
            .max(0);
        equal(
            &key,
            "bounds",
            n.bounds,
            [LIST[0], y, LIST[2], ROW_HEIGHT].map(|v| i64::from(v) * i64::from(scale)),
        )?;
        equal(
            &key,
            "clip",
            n.clip,
            [LIST[0], top, LIST[2], height].map(|v| i64::from(v) * i64::from(scale)),
        )?;
        equal(
            &key,
            "visible",
            n.flags.contains(&Flag::Visible),
            height > 0,
        )?;
    }
    // Closed menu entries remain discoverable, but are not drawn.
    for key in ["menu/0/0", "menu/0/1"] {
        equal(
            key,
            "visible",
            node(tree, key)?.flags.contains(&Flag::Visible),
            false,
        )?;
    }
    Ok(())
}
pub fn expect_terminal(tree: &Tree, scale: u32) -> Result<(), String> {
    let n = node(tree, "terminal")?;
    equal("terminal", "role", n.role, Role::Terminal)?;
    equal("terminal", "name", n.name.as_str(), "Fixed grid")?;
    equal(
        "terminal",
        "bounds",
        n.bounds,
        literals::TERMINAL.map(|v| i64::from(v) * i64::from(scale)),
    )?;
    equal(
        "terminal",
        "cell-size",
        [literals::CELL_WIDTH, literals::CELL_HEIGHT],
        [8, 16],
    )?;
    equal(
        "terminal",
        "value",
        &n.value,
        &serde_json::json!({
            "rows": [
                "PASS",
                "grid"
            ],
            "cols": 4,
            "cursor": {
                "row": 1,
                "col": 2,
                "visible": true
            },
            "cells": [
                [
                    {
                        "fg": 16777215,
                        "bg": 0,
                        "bold": true,
                        "inverse": false,
                        "underline": false
                    },
                    {
                        "fg": 16777215,
                        "bg": 0,
                        "bold": false,
                        "inverse": false,
                        "underline": false
                    },
                    {
                        "fg": 65280,
                        "bg": 0,
                        "bold": false,
                        "inverse": false,
                        "underline": false
                    },
                    {
                        "fg": 16777215,
                        "bg": 128,
                        "bold": false,
                        "inverse": false,
                        "underline": false
                    }
                ],
                [
                    {
                        "fg": 16777215,
                        "bg": 0,
                        "bold": false,
                        "inverse": true,
                        "underline": false
                    },
                    {
                        "fg": 16777215,
                        "bg": 0,
                        "bold": false,
                        "inverse": false,
                        "underline": true
                    },
                    {
                        "fg": 16777215,
                        "bg": 0,
                        "bold": false,
                        "inverse": false,
                        "underline": false
                    },
                    {
                        "fg": 16777215,
                        "bg": 0,
                        "bold": false,
                        "inverse": false,
                        "underline": false
                    }
                ]
            ],
            "pty-consumed": 0,
            "parse-errors": 0,
            "stream-digest": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "rows-retired": 0,
            "rows-digest": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        }),
    )
}

#[cfg(target_os = "breenix")]
mod guest {
    use super::*;
    use breenix_suite::suite::CaseResult;
    use libuitest::{
        arena::{Arena, Slot, SlotRead},
        capture, Session,
    };
    use std::time::{Duration, Instant};
    struct App {
        session: Session,
        id: u64,
        scale: u32,
        tree_baseline: u64,
        _reader: crate::tree_cases::TreeReader,
    }
    impl App {
        fn launch() -> Result<Self, String> {
            let mut session = Session::desktop()?;
            session.launch_app(
                &[
                    b"/usr/libexec/breenix/uitest-app_test",
                    b"widget tree",
                    b"--widgets",
                ],
                0,
            )?;
            let ready = session.wait_window("widget tree")?;
            let reader =
                crate::tree_cases::TreeReader::attach(ready.id).map_err(|e| e.to_string())?;
            let tree_baseline = libbreenix::window::read_meta(ready.id)
                .map_err(|e| e.to_string())?.1.tree_serializations;
            let scale = libbreenix::gpu::current_mode()
                .map_err(|e| e.to_string())?
                .scale;
            let mut app = Self {
                session,
                id: ready.id,
                scale,
                tree_baseline,
                _reader: reader,
            };
            // A real pointer event after attachment forces the first serialized commit.
            app.motion(30, 55)?;
            Ok(app)
        }
        fn motion(&mut self, x: i32, y: i32) -> Result<(), String> {
            let mut infos = vec![libbreenix::window::abi::WinInfo::default(); 64];
            let count = libbreenix::window::list(&mut infos).map_err(|e| e.to_string())?;
            let info = infos
                .get(..count)
                .ok_or("window registry overflow")?
                .iter()
                .find(|w| w.id == self.id)
                .ok_or("widget window missing")?;
            let (sx, sy) = (
                info.x + x * self.scale as i32,
                info.y + y * self.scale as i32,
            );
            self.session.raw_motion(i32::MIN / 2, i32::MIN / 2)?;
            self.session.raw_motion(sx, sy)
        }
        fn click(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.motion(x, y)?;
            self.session.raw_press(0x110)?;
            self.session.raw_release(0x110)
        }
        fn snapshot(&mut self) -> Result<Tree, String> {
            let path = std::path::Path::new(self.session.config_root()).join("widget-tree.arena");
            let result = (|| {
                let mut arena = Arena::create(&path, self.session.arena_mut().layout())?;
                let record = capture::window(&mut arena, "widget-tree", self.id)?;
                equal(
                    "root",
                    "capture-size",
                    (record.width, record.height),
                    (literals::WIDTH * self.scale, literals::HEIGHT * self.scale),
                )?;
                match arena.read(Slot::Tree)? {
                    SlotRead::Final { record, bytes } if !record.oversize => Tree::parse(&bytes),
                    other => Err(format!("incomplete widget tree: {other:?}")),
                }
            })();
            let remove = std::fs::remove_file(path).map_err(|e| e.to_string());
            let tree = result?;
            remove?;
            Ok(tree)
        }
        fn check_pixels(&mut self, text: bool) -> Result<(), String> {
            let path = std::path::Path::new(self.session.config_root()).join("widget-pixels.arena");
            let mut arena = Arena::create(&path, self.session.arena_mut().layout())?;
            let result = (|| {
                let record = capture::window(&mut arena, "widget-pixels", self.id)?;
                let tree = match arena.read(Slot::Tree)? {
                    SlotRead::Final { bytes, .. } => Tree::parse(&bytes)?,
                    other => return Err(format!("incomplete tree: {other:?}")),
                };
                let pixels = match arena.read(Slot::Frame)? {
                    SlotRead::Final { bytes, .. } => bytes,
                    other => return Err(format!("incomplete frame: {other:?}")),
                };
                let scale = self.scale as usize;
                if text {
                    equal("status", "name", node(&tree, "status")?.name.as_str(), "PASS")?;
                    let mut surface = libgfx::surface::Surface::new(record.width as usize, record.height as usize, libgfx::surface::Format::BGRX).map_err(|e| format!("surface: {e:?}"))?;
                    let theme = libbui::Theme::dark();
                    surface.clear(theme.panel_bg);
                    libbui::text::draw_text(&mut surface, b"PASS", literals::STATUS[0], literals::STATUS[1], theme.text_primary, &theme);
                    let mut expected = vec![0; pixels.len()];
                    surface.read(&mut expected, record.width as usize * 4).map_err(|e| format!("reference read: {e}"))?;
                    let [x,y,w,h] = node(&tree,"status")?.bounds.map(|v| v as usize);
                    for row in y..y+h {
                        let start = (row * record.width as usize + x) * 4;
                        equal("status", "pixels", &pixels[start..start+w*4], &expected[start..start+w*4])?;
                    }
                } else {
                    focus_is(&tree, "submit")?;
                    let [x,y,w,h] = node(&tree,"submit")?.bounds.map(|v| v as usize);
                    let (left, top, right, bottom) = (x-2*scale, y-2*scale, x+w+2*scale-1, y+h+2*scale-1);
                    let color = libbui::Theme::dark().focus_ring;
                    for yy in top..=bottom { for xx in left..=right {
                        if xx < left+scale || xx > right-scale || yy < top+scale || yy > bottom-scale {
                            let at = (yy * record.width as usize + xx) * 4;
                            equal("submit", "focus-ring", &pixels[at..at+3], &[color.b, color.g, color.r])?;
                        }
                    }}
                }
                Ok(())
            })();
            drop(arena);
            let cleanup = std::fs::remove_file(path).map_err(|e| e.to_string());
            result?; cleanup
        }
        fn wait(&mut self, judge: impl Fn(&Tree) -> Result<(), String>) -> Result<(), String> {
            let end = Instant::now() + Duration::from_secs(2);
            loop {
                let (_, meta) =
                    libbreenix::window::read_meta(self.id).map_err(|e| e.to_string())?;
                if libuitest::input::tree_ready_after_attach(meta.tree_serializations, self.tree_baseline) {
                    let tree = self.snapshot()?;
                    match judge(&tree) {
                        Ok(()) => return Ok(()),
                        Err(e) if Instant::now() >= end => return Err(e),
                        Err(_) => {}
                    }
                } else if Instant::now() >= end {
                    return Err("no widget commit after reader attachment".into());
                }
                libbreenix::time::sleep_ms(1).map_err(|e| e.to_string())?;
            }
        }
    }
    fn state(tree: &Tree, key: &str, flag: Flag, expected: bool) -> Result<(), String> {
        equal(key, "flag", node(tree, key)?.flags.contains(&flag), expected)
    }
    fn focus_is(tree: &Tree, key: &str) -> Result<(), String> {
        for name in ["entry", "submit", "accept", "amount", "list", "tabs"] {
            state(tree, name, Flag::Focused, name == key)?;
        }
        Ok(())
    }
    fn run(which: u8) -> Result<(), String> {
        let mut app = App::launch()?;
        let result = (|| {
            let scale = app.scale;
            app.wait(|tree| expect_roles_keys(tree, scale))?;
            match which {
                0 => Ok(()),
                1 => {
                    app.wait(|tree| expect_names_values(tree, false))?;
                    app.click(30, 55)?;
                    for key in [35, 23] {
                        app.session.raw_press(key)?;
                        app.session.raw_release(key)?;
                    }
                    app.click(25, 100)?;
                    app.click(340, 102)?;
                    app.click(30, 180)?;
                    app.click(485, 162)?;
                    app.click(25, 10)?;
                    app.click(30, 33)?;
                    app.click(330, 55)?;
                    app.wait(|tree| expect_names_values(tree, true))
                }
                2 => {
                    app.wait(expect_enabled)?;
                    app.click(450, 55)?;
                    app.wait(|tree| {
                        expect_enabled(tree)?;
                        equal(
                            "root",
                            "value",
                            &tree.root.value,
                            &serde_json::json!({"submissions":0,"menu_action":0}),
                        )
                    })
                }
                3 => {
                    app.wait(|tree| expect_hidden(tree, 0, scale))?;
                    app.motion(30, 180)?;
                    app.session.raw_wheel(-1)?;
                    app.wait(|tree| expect_hidden(tree, 40, scale))
                }
                4 => app.wait(|tree| expect_terminal(tree, scale)),
                5 => {
                    app.wait(|t| focus_is(t, "entry"))?;
                    app.session.raw_press(15)?;
                    app.session.raw_release(15)?;
                    app.wait(|t| focus_is(t, "submit"))
                }
                6 => {
                    app.wait(|t| focus_is(t, "entry"))?;
                    app.session.raw_press(15)?;
                    app.session.raw_release(15)?;
                    app.wait(|t| focus_is(t, "submit"))?;
                    app.check_pixels(false)
                }
                7 => {
                    app.wait(|t| state(t, "accept", Flag::Checked, false))?;
                    app.click(25, 100)?;
                    app.wait(|t| state(t, "accept", Flag::Checked, true))?;
                    app.wait(|t| state(t, "list/item-01", Flag::Selected, false))?;
                    app.click(30, 180)?;
                    app.wait(|t| state(t, "list/item-01", Flag::Selected, true))?;
                    app.motion(330, 55)?;
                    app.session.raw_press(0x110)?;
                    app.wait(|t| state(t, "submit", Flag::Pressed, true))?;
                    app.session.raw_release(0x110)?;
                    app.wait(|t| state(t, "submit", Flag::Pressed, false))
                }
                8 => {
                    app.wait(|t| equal("entry", "value", &node(t, "entry")?.value, &serde_json::json!("")))?;
                    app.click(30, 55)?;
                    for key in [35, 23] { app.session.raw_press(key)?; app.session.raw_release(key)?; }
                    app.wait(|t| equal("entry", "value", &node(t, "entry")?.value, &serde_json::json!("hi")))
                }
                9 => {
                    app.wait(|t| expect_hidden(t, 0, scale))?;
                    app.motion(30, 180)?;
                    app.session.raw_wheel(-1)?;
                    app.wait(|t| expect_hidden(t, 40, scale))
                }
                10 => {
                    app.wait(|t| equal("status", "name", node(t, "status")?.name.as_str(), "PASS"))?;
                    app.check_pixels(true)
                }
                _ => unreachable!(),
            }
        })();
        let teardown = app.session.teardown();
        result?;
        teardown?;
        Ok(())
    }
    pub fn focus_flag() -> CaseResult { run(5).map_err(Into::into) }
    pub fn focus_ring_pixels() -> CaseResult { run(6).map_err(Into::into) }
    pub fn state_flags() -> CaseResult { run(7).map_err(Into::into) }
    pub fn textfield_value() -> CaseResult { run(8).map_err(Into::into) }
    pub fn list_scroll() -> CaseResult { run(9).map_err(Into::into) }
    pub fn text_pixels() -> CaseResult { run(10).map_err(Into::into) }
    pub fn roles_keys() -> CaseResult {
        run(0).map_err(Into::into)
    }
    pub fn names_values() -> CaseResult {
        run(1).map_err(Into::into)
    }
    pub fn enabled_flag() -> CaseResult {
        run(2).map_err(Into::into)
    }
    pub fn hidden_nodes() -> CaseResult {
        run(3).map_err(Into::into)
    }
    pub fn terminal_grid() -> CaseResult {
        run(4).map_err(Into::into)
    }
}
#[cfg(target_os = "breenix")]
pub use guest::{enabled_flag, hidden_nodes, names_values, roles_keys, terminal_grid, focus_flag, focus_ring_pixels, state_flags, textfield_value, list_scroll, text_pixels};
