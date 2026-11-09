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
