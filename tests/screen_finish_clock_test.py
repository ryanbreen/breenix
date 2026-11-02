"""Exercise finish when the shared checkpoint clock cannot be allocated."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function

ROOT = Path(__file__).resolve().parents[1]

class FinishClock(unittest.TestCase):
    def test_null_clock_reports_error_without_observation(self):
        source = (ROOT / 'libs/libbreenix/src/suite_screen.rs').read_text()
        fixture = r'''
use std::sync::{Mutex, atomic::{AtomicBool,AtomicPtr,Ordering}};
static ENABLED:AtomicBool=AtomicBool::new(true);
static CLOCK:AtomicPtr<u8>=AtomicPtr::new(std::ptr::null_mut());
static LINES:Mutex<String>=Mutex::new(String::new());
fn in_case()->bool{true}
fn timed<F:FnOnce()>(_:F){panic!("observation timed without clock")}
fn append(_: &str,_:bool)->Result<(),()>{panic!("observation without clock")}
fn pack(s:&str)->Vec<u8>{s.as_bytes().to_vec()}
mod super_stub {pub fn judged_path()->String{std::env::var("TEST_PATH").unwrap()}}
FINISH
fn main(){assert!(CLOCK.load(Ordering::Relaxed).is_null());finish();let text=std::fs::read_to_string(std::env::var("TEST_PATH").unwrap()+".screen").unwrap();assert!(text.contains("error=clock-allocation"));}
'''.replace('FINISH', function(source, 'pub(super) fn finish()').replace('super::judged_path()', 'super_stub::judged_path()').replace('pub(super) fn finish()', 'fn finish()'))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            import os
            subprocess.run([str(path / 'test')], env={**os.environ, 'TEST_PATH': str(path / 'handoff')}, check=True)

if __name__ == '__main__':
    unittest.main()
