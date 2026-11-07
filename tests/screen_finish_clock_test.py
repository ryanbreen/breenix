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
fn timed<R,F:FnOnce()->R>(_:F)->R{panic!("observation timed without clock")}
type Snapshot=Result<([u8;32],(),()),String>;
static END:Mutex<End>=Mutex::new(End{result:None});
struct End{result:Option<Snapshot>}
static CAPTURE_PIXELS:Mutex<Option<Vec<u8>>>=Mutex::new(None);
static CRC:Mutex<Option<Vec<u8>>>=Mutex::new(None);
static END_CRC:Mutex<Option<Vec<u8>>>=Mutex::new(None);
fn format_crc_record(_: &str,_:&str,_:&str,_:[u8;32],_:(),_:(),_:&[u8])->Result<String,&'static str>{panic!("CRC without clock")}
fn queue_crc(_: &std::path::Path,_:&[u8])->Result<(),&'static str>{panic!("CRC without clock")}
fn crc_pending_path()->std::path::PathBuf{std::path::PathBuf::new()}
fn snapshot(_: &str)->Snapshot{panic!("observation without clock")}
fn snapshot_end()->Snapshot{snapshot("end")}
fn capture_enabled()->bool{false}
fn capture(_: &str,_:&str,_:&str,_:(),_:[u8;32],_:&[u8])->Result<(),&'static str>{Ok(())}
fn format_record(_: &str,_:&str,_:&str,result:Result<([u8;32],(),()),&str>)->String{format!("error={}",result.unwrap_err())}
APPEND
fn pack(s:&str)->Vec<u8>{s.as_bytes().to_vec()}
mod super_stub {pub static RECORD_NAMES:std::sync::OnceLock<(String,String)>=std::sync::OnceLock::new(); pub fn running_case()->Option<&'static str>{Some("case")} pub fn judged_path()->String{std::env::var("TEST_PATH").unwrap()}}
FINISH
fn main(){super_stub::RECORD_NAMES.set(("SUITE gpu ".into(),String::new())).unwrap();assert!(CLOCK.load(Ordering::Relaxed).is_null());finish();let text=std::fs::read_to_string(std::env::var("TEST_PATH").unwrap()+".screen").unwrap();assert!(text.contains("error=clock-allocation"));}
'''.replace('FINISH', function(source, 'pub(super) fn finish()').replace('super::judged_path()', 'super_stub::judged_path()').replace('pub(super) fn finish()', 'fn finish()')).replace('APPEND', function(source, 'fn append(').replace('super::', 'super_stub::'))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            import os
            subprocess.run([str(path / 'test')], env={**os.environ, 'TEST_PATH': str(path / 'handoff')}, check=True)

if __name__ == '__main__':
    unittest.main()
