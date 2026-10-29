"""Exercise the large-target viewing decision with fitting and oversized modes."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class LargeTargetView(unittest.TestCase):
    def test_r53_offscreen_answer_and_fitting_fence_are_both_required(self):
        source=(ROOT/'userspace/programs/src/suite_gpu/readback.rs').read_text()
        prefix=item(source,'pub fn large_target()').split('    expect(t, &scene_canvas')[0]+'    Ok(())\n}'
        helpers=item(source,'fn present_fence(')+'\n'+item(source,'fn present(')
        harness=r'''extern crate self as breenix_suite;
mod suite {pub type CaseError=String;}
type CaseResult=Result<(),String>;
use std::sync::atomic::{AtomicU32,AtomicBool,Ordering::SeqCst};
static WIDTH:AtomicU32=AtomicU32::new(0);static HEIGHT:AtomicU32=AtomicU32::new(0);
static VIEW:AtomicBool=AtomicBool::new(false);
fn fail_msg(s:String)->String{s}
fn ensure(ok:bool,why:impl FnOnce()->String)->CaseResult {if ok {Ok(())} else {Err(why())}}
fn begin()->Result<(),String>{Ok(())}
fn setup(_:u32,_:u32,_:u32)->Result<(u64,u64),String>{Ok((1,2))}
fn scene(_:u64,_:u32,_:&[([i32;4],u32)])->Vec<u32>{vec![]}
fn frame(_:u64,_:&[u32])->Result<u64,String>{Ok(1)}
fn errno(r:i64)->String{r.to_string()}
const A:u32=1;const B:u32=2;const C:u32=3;
struct Present {fence:u64}
fn present_view(_:u64,_:u64)->Result<Option<Present>,String>{Ok(VIEW.load(SeqCst).then_some(Present{fence:17}))}
mod format {pub const B8G8R8A8:u32=0;}
mod abi {
 pub mod op {pub const MODES:u32=1;}
 #[derive(Clone,Copy)] pub struct Mode {pub width:u32,pub height:u32}
 pub struct Modes {pub current:u32,pub count:u32,pub modes:[Mode;1]}
 impl Modes {pub const ZERO:Self=Self{current:0,count:0,modes:[Mode{width:0,height:0};1]};}
}
mod gpu {pub fn call_with(op:u32,m:&mut super::abi::Modes)->i64 {
 assert_eq!(op,super::abi::op::MODES);m.count=1;
 m.modes[0]=super::abi::Mode{width:super::WIDTH.load(super::SeqCst),height:super::HEIGHT.load(super::SeqCst)};0
}}
// HELPERS
// PREFIX
fn main(){
 for (w,h) in [(1280,1024),(2560,1600),(3840,2160),(3840,1024)] {
  for answer in [false,true] {
   WIDTH.store(w,SeqCst);HEIGHT.store(h,SeqCst);VIEW.store(answer,SeqCst);
   let expected=answer==(w>=2560 && h>=1600);
   assert_eq!(large_target().is_ok(),expected,"mode {w}x{h}, viewing fence {answer}");
  }
 }
}
'''.replace('// HELPERS',helpers).replace('// PREFIX',prefix)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'view.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','-Adead_code',str(p/'view.rs'),'-o',str(p/'view')],check=True)
            subprocess.run([str(p/'view')],check=True)
if __name__=='__main__': unittest.main()
