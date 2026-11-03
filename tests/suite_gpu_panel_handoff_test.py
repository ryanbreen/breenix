"""Execute the runner's final handoff decisions against display/exec boundaries."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class PanelHandoff(unittest.TestCase):
    def test_text_panel_releases_before_next_exec_and_last_panel_stays(self):
        source = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        tail = source.split('        // A one-case boot stops here, before a sequence\'s next suite.\n', 1)[1]
        tail = tail.split('\n    }\n}\n\n#[derive(Default)]', 1)[0]
        helper = item(source, 'fn hand_off_text_panel(') if 'fn hand_off_text_panel(' in source else ''
        fixture = r'''
use std::sync::{Mutex, atomic::{AtomicBool, Ordering::SeqCst}};
static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());
static OWNED: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicBool = AtomicBool::new(false);
static REFUSE: AtomicBool = AtomicBool::new(false);
pub struct Screen { pub fb: Option<u64> }
pub struct Feed;
struct Runner { id: &'static str }
pub mod gpu_screen {
    pub fn await_console() -> Result<(), String> {
        super::EVENTS.lock().unwrap().push("release");
        if super::REFUSE.load(super::SeqCst) { return Err("restore failed".into()); }
        super::OWNED.store(false, super::SeqCst);
        Ok(())
    }
}
mod sequence {
    pub fn finish() {
        if super::NEXT.load(super::SeqCst) {
            assert!(!super::OWNED.load(super::SeqCst), "exec while the text panel owns the display");
            super::EVENTS.lock().unwrap().push("exec");
            panic!("finished");
        }
        super::EVENTS.lock().unwrap().push("sequence-done");
    }
}
fn emit(line: &str) { EVENTS.lock().unwrap().push(if line.contains("ERROR") {"error"} else {"ready"}); }
fn idle(_: &mut Screen, _: &Feed) -> ! { EVENTS.lock().unwrap().push("idle"); panic!("finished"); }
HELPER
impl Runner {
    fn finish(&self, mut screen: Screen, feed: Feed, place: Option<(Vec<String>, usize)>, only: Option<&str>) -> ! {
TAIL
    }
}
#[test]
fn ownership_is_resolved_at_the_exec_boundary() {
    for (panel, next, only, refuse, expected) in [
        (true, true, false, false, vec!["release", "ready", "exec"]),
        (true, false, false, false, vec!["ready", "sequence-done", "idle"]),
        (false, true, false, false, vec!["ready", "exec"]),
        (true, true, true, false, vec!["idle"]),
        (true, true, false, true, vec!["release", "error", "idle"]),
    ] {
        EVENTS.lock().unwrap().clear();
        OWNED.store(panel, SeqCst); NEXT.store(next, SeqCst); REFUSE.store(refuse, SeqCst);
        let ids = if next {vec!["console".into(), "time".into()]} else {vec!["time".into()]};
        let _ = std::panic::catch_unwind(|| Runner {id:"time"}.finish(
            Screen {fb:panel.then_some(1)}, Feed, Some((ids, 0)), only.then_some("clock")));
        assert_eq!(*EVENTS.lock().unwrap(), expected);
    }
}
'''
        fixture = fixture.replace('HELPER', helper).replace('TAIL', tail)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(fixture)
            target = 'x86_64-apple-darwin' if sys.platform == 'darwin' else 'x86_64-unknown-linux-gnu'
            command = ['rustc', '--edition=2021', '--target', target, '--test', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin': command = ['taskpolicy', '-b', *command]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__': unittest.main()
