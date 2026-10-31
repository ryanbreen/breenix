"""An owner may delay the boot readback, but must not consume its request."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class IdleReadback(unittest.TestCase):
    def test_unavailable_pass_defers_idle_but_answers_proc(self):
        source = (ROOT / 'kernel/src/console/mod.rs').read_text()
        harness = r'''
use std::{fmt, sync::{Mutex, atomic::{AtomicBool, AtomicU32, Ordering}}};
static IDLE_CHECK: AtomicBool = AtomicBool::new(false);
static PROC_CHECK: AtomicU32 = AtomicU32::new(0);
struct ProcResult(Mutex<Option<Result<Check, Absent>>>);
impl ProcResult {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Result<Check, Absent>>> {
        self.0.lock().unwrap()
    }
}
static PROC_RESULT: ProcResult = ProcResult(Mutex::new(None));
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
const TAG: &str = "[fbcon]";
struct Completion(AtomicU32);
impl Completion { fn complete(&self, token: u32) { self.0.store(token, Ordering::Release); } }
static PROC_DONE: Completion = Completion(AtomicU32::new(0));
#[derive(Debug)] enum Absent { NotYet, Taken }
struct Screen;
struct Check {
    source: Result<&'static str, &'static str>, complete: bool, pixel_lines: usize,
    glyph_cells: usize, sha256: [u8; 32], differing_lines: usize, font_mismatches: usize,
}
impl Screen {
    fn check(&self, fg: usize) -> Check {
        assert_eq!(fg, 2);
        Check { source: Ok("device readback"), complete: true, pixel_lines: 1600,
            glyph_cells: 8554, sha256: [0x12; 32], differing_lines: 0, font_mismatches: 0 }
    }
}
impl Check { fn identical(&self) -> bool {
    self.complete && self.source.is_ok() && self.differing_lines == 0 && self.font_mismatches == 0
} }
HEX
macro_rules! say { ($level:ident, $($args:tt)*) => { LOG.lock().unwrap().push(format!($($args)*)) }; }
ANSWER
#[test] fn owner_and_splash_preserve_idle_until_a_real_readback() {
    for absent in [Absent::NotYet, Absent::Taken] {
        IDLE_CHECK.store(true, Ordering::Release);
        PROC_CHECK.store(7, Ordering::Release);
        answer_checks(Err(absent), true, 7);
        assert!(IDLE_CHECK.load(Ordering::Acquire), "unavailable display consumed boot readback");
        assert_eq!(PROC_CHECK.load(Ordering::Acquire), 0);
        assert_eq!(PROC_DONE.0.load(Ordering::Acquire), 7);
        assert!(PROC_RESULT.lock().take().unwrap().is_err(), "proc caller stranded");
        assert!(LOG.lock().unwrap().is_empty(), "unavailable pass reported an idle verdict");
        answer_checks(Ok((&Screen, 2)), false, 0);
        assert!(IDLE_CHECK.load(Ordering::Acquire));
        answer_checks(Ok((&Screen, 2)), true, 0);
        assert!(!IDLE_CHECK.load(Ordering::Acquire));
        let lines = LOG.lock().unwrap().drain(..).collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("screen read back identical: 1600 pixel rows, 8554 glyph cells match the font, sha256 "));
        assert!(lines[0].contains(&"12".repeat(32)));
        answer_checks(Ok((&Screen, 2)), true, 0);
        assert!(LOG.lock().unwrap().is_empty(), "boot readback answered twice");
    }
}
'''.replace('HEX', "struct Hex<'a>(&'a [u8]);\n" + item(source, "impl fmt::Display for Hex<'_>"))
        harness = harness.replace('ANSWER', item(source, 'fn answer_checks('))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            path = directory / 'idle.rs'
            path.write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path), '-o', str(directory / 'idle')], check=True)
            subprocess.run([str(directory / 'idle'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
