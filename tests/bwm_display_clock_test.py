"""Exercise the display case's live clock receipt with host proc-file boundaries."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]

class DisplayClock(unittest.TestCase):
    def test_native_rows_are_retained_and_absent_virtual_proc_is_explicit(self):
        source = item((ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text(), 'pub fn display_clocks(')
        harness = r'''
use std::{cell::RefCell, io};
type CaseResult = Result<(), String>;
thread_local! {
    static PROC: RefCell<io::Result<String>> = RefCell::new(Ok(String::new()));
    static OUTPUT: RefCell<Vec<(String, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}
mod fs { pub fn read_to_string(path: &str) -> std::io::Result<String> {
    assert_eq!(path, "/proc/gt710-pixels");
    super::PROC.with(|p| std::mem::replace(&mut *p.borrow_mut(), Ok(String::new())))
}}
// suite.rs::silence_output sends a case's stdout to /dev/null.
macro_rules! println { ($($arg:tt)*) => { let _ = format!($($arg)*); }; }
pub mod breenix_suite { pub mod suite {
    pub fn upload_diagnostic(suite: &str, label: &str, data: &[u8]) -> Result<String, String> {
        assert_eq!(suite, "window-manager");
        super::super::OUTPUT.with(|o| o.borrow_mut().push((label.into(), data.into())));
        Ok("retained".into())
    }
}}
SOURCE
#[test] fn receipts() {
    println!("case stdout is silenced");
    for phase in ["before", "after"] {
        PROC.with(|p| *p.borrow_mut() = Ok("unrelated pixels\nGT710_CLOCK_V1 {live registers}\n".into()));
        display_clocks(phase).unwrap();
        assert_eq!(OUTPUT.with(|o| std::mem::take(&mut *o.borrow_mut())),
            vec![(format!("display-clocks-{phase}"), b"GT710_CLOCK_V1 {live registers}".to_vec())]);
    }
    PROC.with(|p| *p.borrow_mut() = Err(io::Error::from(io::ErrorKind::NotFound)));
    display_clocks("before").unwrap();
    assert!(OUTPUT.with(|o| o.borrow().is_empty()));
    PROC.with(|p| *p.borrow_mut() = Ok("missing clock row".into()));
    assert!(display_clocks("after").is_err());
    PROC.with(|p| *p.borrow_mut() = Err(io::Error::from(io::ErrorKind::PermissionDenied)));
    assert!(display_clocks("after").is_err());
}
'''.replace('SOURCE', source.replace('std::fs::read_to_string', 'fs::read_to_string'))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(harness)
            for command in [
                ['taskpolicy', '-b', 'rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')],
                [str(path / 'test')],
            ]:
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

if __name__ == '__main__': unittest.main()
