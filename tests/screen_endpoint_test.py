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
const ENODEV:i64=19; const EIO:i64=5; const EFAULT:i64=14;
mod abi { pub mod screen { ABI } pub struct Readback {pub pixels:u64,pub pixels_len:u64,pub record:u64}
 impl Readback {pub const ZERO:Self=Self{pixels:0,pixels_len:0,record:0};}}
mod console {pub fn await_repaint(){}}
mod syscall {pub mod userptr {pub fn write_user_bytes(p:u64,b:*const u8,n:usize)->Result<(),()>{unsafe{std::ptr::copy_nonoverlapping(b,p as *mut u8,n)};Ok(())}}}
struct GpuPath; impl GpuPath{fn enter()->Self{Self}}
struct Core; impl Core{fn admission(&self)->Result<(),()>{Ok(())}}
fn with_core<T>(f:impl FnOnce(&Core)->T)->Option<T>{Some(f(&Core))}
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
 for why in [4,5,7,8,10,0] {
 WHY.store(why,std::sync::atomic::Ordering::Relaxed);
 let mut record=[0x77u8;1024];
 let result=observe_screen(abi::screen::Request{pixels:0,len:0,record:record.as_mut_ptr() as u64});
 assert_eq!(result,Err(if why==0{110}else{5}));
 assert_eq!(&record[..8],b"SCREENER","endpoint erased refusal {why}");
 assert_eq!(i64::from_le_bytes(record[8..16].try_into().unwrap()),if why==0{110}else{5});
 assert_eq!(u32::from_le_bytes(record[16..20].try_into().unwrap()),why);
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
