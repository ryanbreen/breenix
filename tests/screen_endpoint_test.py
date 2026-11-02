"""Exercise the production screen endpoint's error publication without a GPU."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function

ROOT = Path(__file__).resolve().parents[1]

class Endpoint(unittest.TestCase):
    def test_errno_reason_and_invalidating_submission_survive(self):
        endpoint = function((ROOT / 'kernel/src/gpu/mod.rs').read_text(), 'pub fn observe_screen(')
        fixture = r'''
#![allow(dead_code)]
mod readback_reason {pub const TARGET_INVALID:u32=4;}
const ENODEV:i64=19; const EIO:i64=5; const EFAULT:i64=14;
mod abi { pub mod screen { ABI } pub struct Readback {pub pixels:u64,pub pixels_len:u64,pub record:u64}
 impl Readback {pub const ZERO:Self=Self{pixels:0,pixels_len:0,record:0};}}
mod console {pub fn await_repaint(){}}
mod syscall {pub mod userptr {pub fn write_user_bytes(p:u64,b:*const u8,n:usize)->Result<(),()>{unsafe{std::ptr::copy_nonoverlapping(b,p as *mut u8,n)};Ok(())}}}
struct GpuPath; impl GpuPath{fn enter()->Self{Self}}
#[derive(Clone,Copy)] struct Binding{id:u32}
struct Res{invalid:bool,invalidated_by:u64,deliberate_invalidation:u64}
struct Display{binding:Option<Binding>}
struct Core{display:Display,resources:std::collections::BTreeMap<u32,Res>} impl Core{fn admission(&self)->Result<(),()>{Ok(())}}
static DELIBERATE:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
fn with_core<T>(f:impl FnOnce(&Core)->T)->Option<T>{
 let mut resources=std::collections::BTreeMap::new();
 resources.insert(7,Res{invalid:true,invalidated_by:1234,deliberate_invalidation:DELIBERATE.load(std::sync::atomic::Ordering::Relaxed)});
 Some(f(&Core{display:Display{binding:Some(Binding{id:7})},resources}))
}
fn probe_writable(_:u64,_:usize)->Result<(),i64>{Ok(())}
fn caller()->Result<u64,i64>{Ok(42)}
enum Refused{Errno(i64),Eio(u32,u64)} enum Observer{Screen}
static WHY:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(4);
fn read_back(_:u64,_:&mut abi::Readback,_:Observer,_:Option<u64>)->Result<u64,Refused>{
 let why=WHY.load(std::sync::atomic::Ordering::Relaxed);
 if why==0 {Err(Refused::Errno(110))} else {Err(Refused::Eio(why,1234))}
}
ENDPOINT
#[test] fn exact_failure(){
 for (why,deliberate) in [(4,0),(4,1234),(4,1233),(5,1234),(7,0),(8,0),(10,0),(0,0)] {
 DELIBERATE.store(deliberate,std::sync::atomic::Ordering::Relaxed);
 WHY.store(why,std::sync::atomic::Ordering::Relaxed);
 let mut record=[0x77u8;1024];
 let result=observe_screen(abi::screen::Request{pixels:0,len:0,record:record.as_mut_ptr() as u64});
 assert_eq!(result,Err(if why==0{110}else{5}));
 assert_eq!(&record[..8],b"SCREENER","endpoint erased refusal {why}");
 assert_eq!(i64::from_le_bytes(record[8..16].try_into().unwrap()),if why==0{110}else{5});
 assert_eq!(u32::from_le_bytes(record[16..20].try_into().unwrap()),why);
 assert_eq!(u32::from_le_bytes(record[20..24].try_into().unwrap()),u32::from(why==4 && deliberate==1234));
 assert_eq!(u64::from_le_bytes(record[24..32].try_into().unwrap()),if why==0{0}else{1234});
 }
}
'''.replace('ABI', (ROOT / 'libs/gpu-abi/src/screen.rs').read_text()).replace('ENDPOINT', endpoint)
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'test.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'test.rs'), '-o', str(p / 'test')], check=True)
            subprocess.run([str(p / 'test')], check=True)

if __name__ == '__main__': unittest.main()
