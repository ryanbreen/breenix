"""Native callers wait for measured reset requalification, not fallback CAPS."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class ResetAdmissionWait(unittest.TestCase):
    def test_decided_and_running_reset_wait_but_virtual_backends_do_not(self):
        helper=item((ROOT/'kernel/src/gpu/mod.rs').read_text(),'fn native_reset_barrier(')
        source=r'''
use std::cell::Cell;
const EINTR:i64=4;
mod abi {pub mod backend {pub const GK208:u32=6;}}
mod dev {pub fn backend()->u32{crate::BACKEND.with(Cell::get)}use std::cell::Cell;}
thread_local! {static BACKEND:Cell<u32>=const{Cell::new(6)};static CALLS:Cell<u32>=const{Cell::new(0)};static INTERRUPT:Cell<bool>=const{Cell::new(false)};}
struct Core {reset_wanted:bool,resetting:bool}
mod arch {pub fn with_interrupts<R>(f:impl FnOnce()->R)->R {f()}}
fn wait_for<T>(_:Option<u64>,interruptible:bool,mut f:impl FnMut(&mut Core)->Option<T>)->Result<T,()>{
 assert!(interruptible);CALLS.with(|x|x.set(x.get()+1));
 if INTERRUPT.with(Cell::get){return Err(());}
 for (reset_wanted,resetting) in [(true,false),(false,true),(false,false)] {
  let result=f(&mut Core{reset_wanted,resetting});
  if reset_wanted||resetting {assert!(result.is_none(),"published admission before requalification");}
  else {return Ok(result.expect("stable reset must release callers"));}
 }
 unreachable!()
}
// HELPER
fn main(){
 assert_eq!(native_reset_barrier(),Ok(()));assert_eq!(CALLS.with(Cell::get),1);
 INTERRUPT.with(|x|x.set(true));assert_eq!(native_reset_barrier(),Err(EINTR));
 BACKEND.with(|x|x.set(4));assert_eq!(native_reset_barrier(),Ok(()));
 assert_eq!(CALLS.with(Cell::get),2);
}
'''.replace('// HELPER',helper)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'wait.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'wait.rs'),'-o',str(p/'wait')],check=True)
            subprocess.run([str(p/'wait')],check=True)
if __name__=='__main__':unittest.main()
