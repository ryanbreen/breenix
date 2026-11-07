"""Run the CRC endpoint and proc access boundaries with modeled GPU readback."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function

ROOT = Path(__file__).resolve().parents[1]


class CrcEndpoint(unittest.TestCase):
    def test_authorization_proof_gate_and_refusal_details(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        endpoint = function(source, 'pub fn observe_screen_crc(')
        summary = function(source, 'pub fn display_crc_summary(')
        devfs = function((ROOT / 'kernel/src/fs/devfs/mod.rs').read_text(), 'pub fn screen_evidence_write(')
        procfs = (ROOT / 'kernel/src/fs/procfs/mod.rs').read_text()
        branch = function(procfs, '        ProcEntryType::DisplayCrc => {')
        proc_read = 'fn read_display_crc() -> Result<Vec<u8>, i32> ' + branch[branch.index('{'):]
        code = r'''
extern crate alloc;
extern crate self as gpu_abi;
#[path="ABI/screen.rs"] pub mod screen;
#[path="ABI/screen_crc.rs"] pub mod screen_crc;
use std::sync::{Mutex,MutexGuard,atomic::{AtomicBool,AtomicU32,AtomicU64,Ordering}};
static ROOT_CALLER:AtomicBool=AtomicBool::new(true);
mod test_control {pub static ENABLED:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(true);
 pub fn enabled()->bool{ENABLED.load(std::sync::atomic::Ordering::Relaxed)}}
mod window {pub struct Caller; impl Caller {
 pub fn current()->Result<Self,i32>{Ok(Self)}
 pub fn is_root(&self)->bool{super::ROOT_CALLER.load(std::sync::atomic::Ordering::Relaxed)}
}}
mod serial {pub enum Writer{Evidence} pub fn drain_output(){} pub fn write_bytes_atomic(_:Writer,_:&[u8]){panic!("binary request reached serial")}}
mod screen_capture {pub const REQUEST_MAGIC:&[u8]=b"SCR1WR01";pub fn write(_:&[u8])->Result<usize,i32>{panic!("wrong request variant")}}
mod console {pub fn await_repaint(){}}
mod syscall {pub mod userptr {pub fn write_user_bytes(p:u64,b:*const u8,n:usize)->Result<(),()>{unsafe{std::ptr::copy_nonoverlapping(b,p as *mut u8,n)};Ok(())}}}
DEVFS
PROC_READ
mod gpu {
use super::*;
pub fn observe_screen(_: crate::screen::Request)->Result<(),i64>{panic!("CRC dispatched as SCREENRB")}
const ENODEV:i64=19; const EIO:i64=5; const EFAULT:i64=14;
mod abi {pub use crate::{screen,screen_crc};pub struct Readback {pub pixels:u64,pub pixels_len:u64,pub record:u64}
 impl Readback {pub const ZERO:Self=Self{pixels:0,pixels_len:0,record:0};} pub mod backend {pub const GK208:u32=6;}}
mod readback_reason {pub const TARGET_INVALID:u32=4;}
pub static PROOF:AtomicBool=AtomicBool::new(true);
pub static BACKEND:AtomicU32=AtomicU32::new(6);
pub static WHY:AtomicU32=AtomicU32::new(0);
pub static DELIBERATE:AtomicU64=AtomicU64::new(0);
fn screen_proof_enabled()->bool{assert_eq!(BACKEND.load(Ordering::Relaxed),6,"virtual backend reached firmware configuration");PROOF.load(Ordering::Relaxed)}
mod dev {pub fn backend()->u32{super::BACKEND.load(super::Ordering::Relaxed)}}
struct GpuPath;impl GpuPath{fn enter()->Self{Self}}
#[derive(Clone,Copy)]struct Binding{id:u32}
struct Res{invalid:bool,invalidated_by:u64,deliberate_invalidation:u64}
struct Display{binding:Option<Binding>}
struct Core{display:Display,resources:std::collections::BTreeMap<u32,Res>}impl Core{fn admission(&self)->Result<(),()>{Ok(())}}
fn with_core<T>(f:impl FnOnce(&Core)->T)->Option<T>{
 let mut resources=std::collections::BTreeMap::new();
 resources.insert(7,Res{invalid:true,invalidated_by:9,deliberate_invalidation:DELIBERATE.load(Ordering::Relaxed)});
 Some(f(&Core{display:Display{binding:Some(Binding{id:7})},resources}))
}
fn probe_writable(_:u64,_:usize)->Result<(),i64>{Ok(())}
fn caller()->Result<u64,i64>{Ok(42)}
enum Refused{Errno(i64),Eio(u32,u64)}impl From<i64> for Refused{fn from(e:i64)->Self{Self::Errno(e)}}
enum Observer{Screen}
fn read_back_impl(tgid:u64,a:&mut abi::Readback,_:Observer,arg:Option<u64>,crc:Option<screen::CrcRequest<'_>>)->Result<u64,Refused>{
 assert_eq!(tgid,42);assert!(arg.is_none());let crc=crc.unwrap();
 assert_eq!((crc.suite,crc.case,crc.checkpoint),("gpu","draw/one","end"));
 assert_eq!((a.pixels,a.pixels_len,a.record),(crc.pixels,crc.len,crc.observation));
 match WHY.load(Ordering::Relaxed) {
  0=>{unsafe{std::ptr::copy_nonoverlapping(b"SCRCRC01".as_ptr(),crc.record as *mut u8,8)};Ok(123)},
  61=>Err(Refused::Errno(61)),110=>Err(Refused::Errno(110)),n=>Err(Refused::Eio(n,9)),
 }
}
struct Summary(Mutex<Option<String>>);impl Summary{fn lock(&self)->MutexGuard<'_,Option<String>>{self.0.lock().unwrap()}}
static DISPLAY_CRC_SUMMARY:Summary=Summary(Mutex::new(None));
ENDPOINT
SUMMARY
pub fn set_summary(){*DISPLAY_CRC_SUMMARY.lock()=Some("last checkpoint\n".into());}
}
#[test]fn crc_request_and_proc_boundaries(){
 let mut observation=[0x77;1024];let mut record=[0x88;4096];
 let request=screen::CrcRequest{pixels:0x1000,len:4096,observation:observation.as_mut_ptr() as u64,
  record:record.as_mut_ptr() as u64,suite:"gpu",case:"draw/one",checkpoint:"end"}.encode().unwrap();
 test_control::ENABLED.store(false,Ordering::Relaxed);
 assert_eq!(screen_evidence_write(&request),Err(38));assert_eq!(read_display_crc(),Err(-38));
 test_control::ENABLED.store(true,Ordering::Relaxed);ROOT_CALLER.store(false,Ordering::Relaxed);
 assert_eq!(screen_evidence_write(&request),Err(1));assert_eq!(read_display_crc(),Err(-1));
 ROOT_CALLER.store(true,Ordering::Relaxed);gpu::PROOF.store(false,Ordering::Relaxed);
 assert_eq!(screen_evidence_write(&request),Err(38));assert_eq!(screen::Failure::decode(&observation).unwrap().errno,38);
 assert_eq!(record,[0x88;4096]);gpu::PROOF.store(true,Ordering::Relaxed);
 for backend in [4,5] {gpu::BACKEND.store(backend,Ordering::Relaxed);
  assert_eq!(screen_evidence_write(&request),Err(19));assert_eq!(read_display_crc(),Err(-19));}
 gpu::BACKEND.store(6,Ordering::Relaxed);assert_eq!(read_display_crc(),Err(-61));
 assert_eq!(screen_evidence_write(&request),Ok(request.len()));assert_eq!(&record[..8],b"SCRCRC01");
 for why in [4,10,61,110] {gpu::WHY.store(why,Ordering::Relaxed);gpu::DELIBERATE.store(9,Ordering::Relaxed);
  assert_eq!(screen_evidence_write(&request),Err(if why>=61{why as i32}else{5}));
  let failure=screen::Failure::decode(&observation).unwrap();
  assert_eq!(failure.reason,if why>=61{0}else{why});
  assert_eq!(failure.failed_submission,if why>=61{0}else{9});
  assert_eq!(failure.declared,u32::from(why==4));
 }
 gpu::set_summary();assert_eq!(read_display_crc().unwrap(),b"last checkpoint\n");
}
'''
        code = code.replace('ABI', str(ROOT / 'libs/gpu-abi/src'))
        code = code.replace('ENDPOINT', endpoint).replace('\nSUMMARY\n', '\n' + summary + '\n')
        code = code.replace('DEVFS', devfs).replace('PROC_READ', proc_read)
        with tempfile.TemporaryDirectory(prefix='crc-endpoint-') as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(code)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            subprocess.run([str(path / 'test')], check=True)


if __name__ == '__main__':
    unittest.main()
