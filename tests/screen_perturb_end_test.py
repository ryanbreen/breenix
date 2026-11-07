"""A phase's end reference must not consume the ordinary end checkpoint."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function

ROOT = Path(__file__).resolve().parents[1]


class EndIdentity(unittest.TestCase):
    def test_phase_reference_keeps_ordinary_end(self):
        append = function((ROOT / 'libs/libbreenix/src/suite_screen.rs').read_text(), 'fn append(').replace('super::', 'outer::')
        fixture = r'''
use std::sync::{Mutex,atomic::{AtomicPtr,Ordering}};
type Snapshot=Result<([u8;32],(),()),String>;
static CLOCK:AtomicPtr<u8>=AtomicPtr::new(std::ptr::dangling_mut());
static LINES:Mutex<String>=Mutex::new(String::new());
static CRC:Mutex<Option<Vec<u8>>>=Mutex::new(None);
static END_CRC:Mutex<Option<Vec<u8>>>=Mutex::new(None);
static CAPTURE_PIXELS:Mutex<Option<Vec<u8>>>=Mutex::new(None);
struct End{result:Option<Snapshot>}
static END:Mutex<End>=Mutex::new(End{result:None});
mod outer {pub static RECORD_NAMES:std::sync::OnceLock<(String,String)>=std::sync::OnceLock::new();pub fn running_case()->Option<&'static str>{Some("draw/one")}}
fn snapshot_end()->Snapshot{Ok(([3;32],(),()))}
fn snapshot(_: &str)->Snapshot{Ok(([3;32],(),()))}
fn timed<R>(f:impl FnOnce()->R)->R{f()}
fn capture_enabled()->bool{false}
fn capture(_:&str,_:&str,_:&str,_:(),_:[u8;32],_:&[u8])->Result<(),&'static str>{panic!("capture disabled")}
fn format_crc_record(_:&str,_:&str,_:&str,_:[u8;32],_:(),_:(),_:&[u8])->Result<String,&'static str>{panic!("no raw record")}
fn queue_crc(_: &std::path::Path,_:&[u8])->Result<(),&'static str>{panic!("no raw record")}
fn crc_pending_path()->std::path::PathBuf{std::path::PathBuf::new()}
fn format_record(s:&str,c:&str,n:&str,result:Result<([u8;32],(),()),&str>)->String{assert!(result.is_ok());format!("SUITE {s} SCREEN {c} {n}\n")}
APPEND
fn main(){
 outer::RECORD_NAMES.set(("SUITE gpu ".into(),String::new())).unwrap();
 LINES.lock().unwrap().push_str("SUITE gpu SCREENPERTURB draw/one end phase=1\n");
 append("end",true).expect("phase reference consumed ordinary end identity");
 assert!(LINES.lock().unwrap().contains("SUITE gpu SCREEN draw/one end\n"));
}
'''.replace('APPEND', append)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); source = path / 'test.rs'; source.write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(source), '-o', str(path/'test')], check=True)
            subprocess.run([str(path/'test')], check=True)


if __name__ == '__main__': unittest.main()
