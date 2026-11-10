#!/usr/bin/env python3
"""Execute the production key-consumption predicate and clipboard fixture lifetime."""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def execute(source, *args):
    with tempfile.TemporaryDirectory(prefix="bterm-path-") as directory:
        path = Path(directory)
        (path / "main.rs").write_text(source)
        prefix = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
        subprocess.run(prefix + ["rustc", "--edition=2021", "-Dwarnings",
                        str(path / "main.rs"), "-o", str(path / "check")], check=True,
                       env=dict(os.environ, CARGO_BUILD_JOBS="4"))
        subprocess.run([str(path / "check"), *args], check=True)


class AcceptancePaths(unittest.TestCase):
    def test_function_keys_require_the_window_manager_modifier(self):
        source = (ROOT / "userspace/programs/src/bwm.rs").read_text()
        predicate = re.search(r"if ([^\n{]*\(key::F1\.\.key::F1 \+ 5\)\.contains\(&event.code\)) \{", source)[1]
        execute("""
mod key { pub const F1:u16=59; }
mod modifier { pub const SUPER:u8=8; }
struct Event { code:u16 }
fn main() {
    // Exercise the condition which consumes the actual source event.
    for code in 59..=63 {
        let event=Event { code };
        for current_mods in [0u8,1,2,4,8] {
            let consumed = {PREDICATE};
            assert_eq!(consumed, current_mods == modifier::SUPER,
                       "client function key code={code} modifiers={current_mods}");
        }
    }
}
""".replace("{PREDICATE}", predicate))

    def test_window_switches_cancel_super_taps(self):
        source = (ROOT / "userspace/programs/src/bwm.rs").read_text()
        dispatch = source.split("                if event.value != 0 {", 1)[1].split("                timing.key(event", 1)[0]
        route = (ROOT / "userspace/programs/src/bwm/route.rs").read_text()
        definitions = route[route.index("pub(super) mod modifier"):route.index("impl HotkeyManager")]
        methods = route[route.index("    pub(super) fn new()"):route.index("    /// Load hotkey config")]
        methods += route[route.index("    pub(super) fn load_defaults"):route.index("    pub(super) fn parse_line")]
        methods += route[route.index("    /// Match a core key press"):route.index("pub(super) fn trim")]
        execute(r"""
mod timing { pub fn within(a:u64,b:u64)->bool { b.saturating_sub(a)<=400_000_000 } }
mod route {
{DEFINITIONS}
impl HotkeyManager {
{METHODS}
}
use route::*;
mod key { pub const F1:u16=59; }
mod input_keys { pub fn chord_key(code:u16)->u8 { if code==30 {b'a'} else {0} } }
struct Event { code:u16, value:i32 }
struct Stack;
impl Stack { fn order(&self)->&[u64] { &[1,2,3,4,5] } fn raise(&mut self,_id:u64) {} }
fn publish_transition(_: &(),_: &Stack,_:()) {}
fn compose_full_redraw(_: &mut (),_: &(),_: &Stack,_: &(),_: &mut ()) {}
fn press(hotkey_mgr:&mut HotkeyManager, code:u16) {
    let event=Event {code,value:1}; let current_mods=modifier::SUPER;
    let mut stack=Stack; let windows=(); let mut gpu=(); let clock_text=(); let mut ui_font=();
    let mut full_redraw=false; let mut consumed=false; let slot=code;
    let mut consumed_keys=std::collections::HashSet::new(); let mut hotkey_actions=Vec::new();
    if event.value != 0 {
{DISPATCH}
    assert!(consumed && full_redraw && consumed_keys.contains(&slot));
    assert!(hotkey_actions.is_empty());
}
fn main() {
    let mut hotkeys=HotkeyManager::new(); hotkeys.load_defaults();
    for code in 59..=63 {
        assert!(hotkeys.update(8,1,10_000_000 * u64::from(code)).is_none(), "switch launched a modifier-only action");
        press(&mut hotkeys,code);
        assert!(hotkeys.update(0,0,10_000_000 * u64::from(code)+1).is_none());
    }
    assert!(hotkeys.update(8,1,700_000_000).is_none());
    assert!(hotkeys.update(0,0,710_000_000).is_none());
    assert!(matches!(hotkeys.update(8,1,720_000_000),Some(HotkeyAction::Exec(path)) if path==b"/usr/bin/blauncher"));
}
""".replace("{DEFINITIONS}",definitions).replace("{METHODS}",methods).replace("{DISPATCH}",dispatch))

    def test_type_line_survives_a_command_opening_another_window(self):
        app = (ROOT / "libs/libuitest/src/app.rs").read_text()
        methods = app[app.index("        pub fn type_line("):app.index("        pub fn tree(")]
        keyboard = (ROOT / "libs/libuitest/src/input.rs").read_text()
        chord = keyboard[keyboard.index("    pub fn chord("):keyboard.index("    pub fn type_text(")]
        execute("""
type Delivery = ();
mod key { pub const ENTER: u16 = 28; }
#[derive(PartialEq)]
struct Modifier;
impl Modifier { fn code(&self) -> u16 { 42 } }
struct Backend { focused: u64, bytes: Vec<u8>, open_window: bool }
impl Backend {
    fn text(&mut self, text: &str, window: u64) -> Result<(), String> {
        if self.focused != window { return Err("text reached another window".into()); }
        self.bytes.extend_from_slice(text.as_bytes());
        if text.ends_with('\\n') && self.open_window { self.focused = 15; }
        Ok(())
    }
    fn key(&mut self, code: u16, pressed: bool, window: u64) -> Result<(), String> {
        if self.focused != window {
            return Err(format!("key {code} pressed={pressed} reached {} instead of {window}", self.focused));
        }
        if pressed && code == key::ENTER {
            self.bytes.push(b'\\n');
            if self.open_window { self.focused = 15; }
        }
        Ok(())
    }
}
struct Keyboard { backend: Backend }
impl Keyboard {
    fn backend_mut(&mut self) -> &mut Backend { &mut self.backend }
{CHORD}
}
struct App { keys: Keyboard, window: u64 }
impl App {
{METHODS}
}
fn main() {
    for open_window in [false, true] {
        let mut app = App { window: 14, keys: Keyboard {
            backend: Backend { focused: 14, bytes: Vec::new(), open_window }
        }};
        app.type_line("launch-helper").expect("command submission must finish across a focus change");
        assert_eq!(app.keys.backend.bytes, b"launch-helper\\n");
        assert_eq!(app.keys.backend.focused, if open_window { 15 } else { 14 });
        // Keep the physical-key API compiled and exercise its stable-focus path.
        app.keys.backend.focused = 14;
        app.key(30, &[]).unwrap();
    }
}
""".replace("{CHORD}", chord).replace("{METHODS}", methods))

    def test_clipboard_ready_is_after_the_owner_window_is_destroyed(self):
        fixture = ROOT / "userspace/programs/src/clip_set_test.rs"
        execute("""
use std::sync::atomic::{AtomicBool, Ordering};
static ALIVE:AtomicBool=AtomicBool::new(false);
mod breengel {
    pub struct Window;
    impl Window {
        pub fn new_without_fonts(_: &[u8], _:u32, _:u32)->Result<Self,&'static str> {
            super::ALIVE.store(true,super::Ordering::SeqCst); Ok(Self)
        }
        pub fn id(&self)->u64 { 15 }
    }
    impl Drop for Window { fn drop(&mut self) {
        super::ALIVE.store(false,super::Ordering::SeqCst);
    }}
}
mod libclipboard {
    pub fn set(id:u64, text:&[u8])->Result<(), &'static str> {
        assert!(super::ALIVE.load(super::Ordering::SeqCst));
        assert_eq!(id,15); assert_eq!(text,b"echo pasted"); Ok(())
    }
}
macro_rules! println { ($message:literal) => {{
    assert!(!ALIVE.load(Ordering::SeqCst), "ready signalled while helper owns focus");
    std::println!($message);
}}; }
""" + f'include!({str(fixture)!r});'.replace("'", '"'), "echo pasted")


if __name__ == "__main__":
    unittest.main()
