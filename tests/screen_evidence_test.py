"""Exercise the SCREEN sink and UART producer routing without a guest boot."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, signature):
    start = source.index(signature)
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class Evidence(unittest.TestCase):
    def test_screen_bytes_never_reach_console_or_producer_capture(self):
        devfs = (ROOT / 'kernel/src/fs/devfs/mod.rs').read_text()
        sink = function(devfs, 'pub fn screen_evidence_write(')
        wrote = function((ROOT / 'kernel/src/serial_line.rs').read_text(), 'fn wrote(writer:')
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertIn('screen::emit(&checkpoint)', suite)
        emitter = function((ROOT / 'libs/libbreenix/src/suite_screen.rs').read_text(), 'pub(super) fn emit(')
        self.assertIn('/dev/screen-evidence', emitter)
        self.assertNotIn('STDOUT', emitter)
        writer_source = (ROOT / 'kernel/src/serial_writer.rs').read_text()
        writer = function(writer_source, 'pub enum Writer')
        writer += '\nstatic OPEN: AtomicU8 = AtomicU8::new(0);\n'
        for signature in ['pub fn lead(', 'pub fn line_open(', 'pub fn wrote(']:
            writer += '\n' + function(writer_source, signature)
        fixture = r'''

use std::sync::Mutex;
static WIRE:Mutex<Vec<u8>>=Mutex::new(Vec::new());
static CAPTURE:Mutex<Vec<u8>>=Mutex::new(Vec::new());
static CONSOLE:Mutex<Vec<u8>>=Mutex::new(Vec::new());
static QUEUED:Mutex<Vec<u8>>=Mutex::new(Vec::new());
static DRAINS:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
use serial_writer::Writer;
mod graphics { pub mod boot_ribbon { pub fn byte(b:u8){crate::CAPTURE.lock().unwrap().push(b);} } }
mod serial_writer {use std::sync::atomic::{AtomicU8,Ordering}; #[derive(PartialEq,Eq,Clone,Copy)] WRITER }
mod gpu_abi { pub mod screen {
 pub const MAGIC:[u8;8]=*b"SCREENRB";
 pub struct Request;
 impl Request {pub fn decode(b: &[u8])->Option<Self>{(b.len()==32).then_some(Self)}}
} }
static READS:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
static ROOT:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(true);
static SUITE:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(true);
mod gpu {pub fn observe_screen(_:crate::gpu_abi::screen::Request)->Result<(),i64>{super::READS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);Ok(())}}
mod test_control {pub fn enabled()->bool{super::SUITE.load(std::sync::atomic::Ordering::Relaxed)}}
mod window {pub struct Caller;impl Caller {pub fn current()->Result<Self,i32>{Ok(Self)}pub fn is_root(&self)->bool{super::ROOT.load(std::sync::atomic::Ordering::Relaxed)}}}
mod serial {pub use crate::serial_writer::Writer;
pub fn drain_output(){super::DRAINS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);let bytes=std::mem::take(&mut *super::QUEUED.lock().unwrap());super::WIRE.lock().unwrap().extend_from_slice(&bytes);}
pub fn write_bytes_atomic(w:Writer,b:&[u8]){let lead=super::serial_writer::lead(w,b);for &v in lead.iter().chain(b) {super::QUEUED.lock().unwrap().push(v);super::wrote(w,v);}}}
WROTE
SINK
fn ordinary(w:Writer,b:&[u8]){CONSOLE.lock().unwrap().extend_from_slice(b);serial::write_bytes_atomic(w,b);}
fn run(on:bool)->(Vec<u8>,Vec<u8>,Vec<u8>){
 WIRE.lock().unwrap().clear();CAPTURE.lock().unwrap().clear();CONSOLE.lock().unwrap().clear();
 ordinary(Writer::Kernel,b"boot producer\n");
 ordinary(Writer::Tty,b"producer one\npartial");
 assert!(serial_writer::line_open());
 if on {let record=b"\nSUITE gpu SCREEN draw/one end error=readback-failed\n";assert_eq!(screen_evidence_write(record),Ok(record.len()));}
 ordinary(Writer::Console,b"producer two\n");
 serial::drain_output();
 (CONSOLE.lock().unwrap().clone(),CAPTURE.lock().unwrap().clone(),WIRE.lock().unwrap().clone())
}
#[test] fn isolation(){let off=run(false);let on=run(true);assert_eq!(off.0,on.0);assert_eq!(off.1,on.1);assert!(on.2.len()>off.2.len());
 WIRE.lock().unwrap().clear();
 QUEUED.lock().unwrap().extend_from_slice(b"\nSUITE gpu CASE draw/one PASS ms=1\n");
 let drains=DRAINS.load(std::sync::atomic::Ordering::Relaxed);
 screen_evidence_write(b"\nSUITE gpu SCREEN draw/one end error=readback-failed\n").unwrap();
 assert_eq!(DRAINS.load(std::sync::atomic::Ordering::Relaxed),drains,"SCREEN must queue without waiting for UART or XON");
 serial::drain_output();
 let wire=String::from_utf8(WIRE.lock().unwrap().clone()).unwrap();
 assert!(wire.find(" CASE ").unwrap()<wire.find(" SCREEN ").unwrap(),"SCREEN overtook queued CASE: {wire}");
 let mut request=[0u8;32];request[..8].copy_from_slice(b"SCREENRB");
 let wire=WIRE.lock().unwrap().len();
 assert_eq!(screen_evidence_write(&request),Ok(32));
 assert_eq!(WIRE.lock().unwrap().len(),wire,"binary requests cannot reach serial");
 assert_eq!(screen_evidence_write(&request[..31]),Err(22));
 ROOT.store(false,std::sync::atomic::Ordering::Relaxed);
 assert_eq!(screen_evidence_write(&request),Err(1));
 ROOT.store(true,std::sync::atomic::Ordering::Relaxed);
 SUITE.store(false,std::sync::atomic::Ordering::Relaxed);
 assert_eq!(screen_evidence_write(&request),Err(38));
 assert_eq!(READS.load(std::sync::atomic::Ordering::Relaxed),1);
}
'''.replace('WROTE', wrote).replace('SINK', sink).replace('WRITER', writer)
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)
            (p / 'test.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'test.rs'), '-o', str(p / 'test')], check=True)
            subprocess.run([str(p / 'test')], check=True)


if __name__ == '__main__':
    unittest.main()
