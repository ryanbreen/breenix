"""Execute pending-composition readback admission before GPU observation."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class PendingReadback(unittest.TestCase):
 def test_pending_claim_does_not_pin_scanout(self):
  body=item((ROOT/'kernel/src/syscall/window.rs').read_text(),'fn readback(')
  run_rust(self,r'''
use std::sync::{Mutex,atomic::{AtomicUsize,Ordering}};
const EPERM:i32=1;const EINVAL:i32=22;const EAGAIN:i32=11;
static GPU_CALLS:AtomicUsize=AtomicUsize::new(0);static OUTPUTS:AtomicUsize=AtomicUsize::new(0);
#[derive(Clone,Copy)]struct Held{first_serial:u64}
struct Present{serial:u64,busy:bool}
struct Registry{claim:Option<Held>,present:Present}
impl Registry{fn is_compositor(&self,_:&Caller)->bool{false}}
static REGISTRY:Mutex<Registry>=Mutex::new(Registry{claim:Some(Held{first_serial:10}),present:Present{serial:10,busy:false}});
struct Caller{root:bool}impl Caller{fn is_root(&self)->bool{self.root}}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]struct Readback{reserved:u32,reserved2:u32,serial:u64,reason:u32,record_len:u32,dst:u64,dst_len:u64,record:u64}
mod gpu_abi{pub mod record{pub const SIZE:usize=1024;}pub mod errno{pub const EIO:i64=5;}}
mod gpu{pub fn observe_scanout(_:u64,_:u64,_:u64)->Result<u64,(i64,u32)>{crate::GPU_CALLS.fetch_add(1,crate::Ordering::Relaxed);Ok(99)}}
fn output(_:u64,_:&Readback)->Result<(),i32>{OUTPUTS.fetch_add(1,Ordering::Relaxed);Ok(())}
@BODY@
#[test]fn pending_and_completed_admission(){
 let caller=Caller{root:true};assert_eq!(REGISTRY.lock().unwrap().claim.unwrap().first_serial,10);
 let blank=Readback{reserved:0,reserved2:0,serial:0,reason:0,record_len:1024,dst:1,dst_len:4096,record:2};
 for busy in [false,true]{let mut reg=REGISTRY.lock().unwrap();reg.present.busy=busy;drop(reg);let mut r=blank;assert_eq!(readback(&caller,0,&mut r),Err(EAGAIN));assert_eq!(r,blank);assert_eq!(GPU_CALLS.load(Ordering::Relaxed),0);assert_eq!(OUTPUTS.load(Ordering::Relaxed),0);}
 {let mut reg=REGISTRY.lock().unwrap();reg.present.serial=11;reg.present.busy=false;}
 let mut r=blank;assert_eq!(readback(&caller,0,&mut r),Ok(0));assert_eq!(r.serial,99);assert_eq!(GPU_CALLS.load(Ordering::Relaxed),1);
 {let mut reg=REGISTRY.lock().unwrap();reg.present.busy=true;}
 let mut r=blank;assert_eq!(readback(&caller,0,&mut r),Err(EAGAIN));assert_eq!(GPU_CALLS.load(Ordering::Relaxed),1);
 let mut r=blank;r.reserved=1;assert_eq!(readback(&caller,0,&mut r),Err(EINVAL));
 assert_eq!(readback(&Caller{root:false},0,&mut blank.clone()),Err(EPERM));
 {let mut reg=REGISTRY.lock().unwrap();reg.claim=None;reg.present.busy=false;}
 assert_eq!(readback(&caller,0,&mut blank.clone()),Ok(0));
}
'''.replace('@BODY@',body).replace('REGISTRY.lock()', 'REGISTRY.lock().unwrap()').replace('.unwrap().unwrap()', '.unwrap()'))
if __name__=='__main__':unittest.main()
