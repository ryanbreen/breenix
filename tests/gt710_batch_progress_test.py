"""A bounded native queue must probe newly refilled work, without extending watchdogs."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class BatchProgress(unittest.TestCase):
    def test_short_chunk_refills_do_not_inherit_coarse_submission_age(self):
        path='kernel/src/gpu/mod.rs';old=os.environ.get('BREENIX_BATCH_PROGRESS_REV')
        source=subprocess.check_output(['git','show',f'{old}:{path}'],cwd=ROOT,text=True) if old else (ROOT/path).read_text()
        policy=(ROOT/'kernel/src/gpu/completion_timing.rs').read_text().replace('//!','//').split('/// Five caller')[0]
        run_rust(self,r'''
use std::{collections::BTreeMap,sync::atomic::{AtomicU64,Ordering}};
static CLOCK:AtomicU64=AtomicU64::new(0);
fn now_ns()->u64{CLOCK.load(Ordering::Relaxed)}
mod completion_timing{@POLICY@}
const POLL_NS:u64=completion_timing::POLL_NS;const EXAMINE_NS:u64=250_000_000;
mod abi{pub mod backend{pub const SVGA3D_DX:u32=5;pub const GK208:u32=6;}}
mod limits{pub const WATCHDOG_NS:u64=2_000_000_000;}
mod fence_status{pub const OK:u32=0;pub const DEVICE_ERROR:u32=1;pub const FENCE_MISMATCH:u32=2;}
mod test_point{pub const STALE_FENCE:u64=20;pub const WITHHOLD_FENCE:u64=31;pub const WITHHOLD_DEPTH:u64=1;pub const WITHHOLD_ALL:u64=2;}
mod test_control{pub fn take(_:u64,_:u64)->Option<u64>{None}pub fn take_if(_:u64,_:u64,f:impl FnOnce(u64)->bool)->Option<u64>{let _=f(0);None}}
mod dev{pub fn backend()->u32{crate::abi::backend::GK208}pub fn ok(answer:u32)->bool{answer==0}pub fn presentation_probe(_:bool)->Option<u64>{None}}
struct Device;impl Device{fn outstanding(&self)->usize{1}}
#[derive(PartialEq)]enum CtxState{Live}
struct Depth{kept:Vec<u64>}
struct Context{depth:Option<Depth>,warm_catalogue:bool,state:CtxState,dropped:bool,warm:u32,warm_after:u64}
fn catalogue_mask()->u32{0}
struct Sub{published:bool,failed:bool,sent_ns:Option<u64>,progress_ns:Option<u64>,requests:u32,error:Option<u32>,present:Option<()>,native_fence:u64,fence:u64,mismatch:Option<u32>,result:Option<(u32,u32)>,ctx:u32}
struct Done{answer:u32,native_fence:Option<u64>,fence:Option<u64>,echoed:u64}
impl Done{fn matches_fence(&self,f:u64)->bool{self.fence==Some(f)}}
struct Core{subs:BTreeMap<u64,Sub>,syncs:BTreeMap<u64,Sub>,contexts:BTreeMap<u32,Context>,quarantined_at:Option<u64>,last_progress:u64,dev:Option<Device>,stalled:bool}
impl Core{
 fn new(requests:u32)->Self{Self{subs:BTreeMap::from([(1,Sub{published:false,failed:false,sent_ns:Some(0),progress_ns:None,requests,error:None,present:None,native_fence:0,fence:17,mismatch:None,result:None,ctx:1})]),syncs:BTreeMap::new(),contexts:BTreeMap::from([(1,Context{depth:None,warm_catalogue:false,state:CtxState::Live,dropped:false,warm:0,warm_after:0})]),quarantined_at:None,last_progress:0,dev:Some(Device),stalled:false}}
 fn publish(&mut self,serial:u64,status:u32,answer:u32){assert_eq!((status,answer),(0,0));self.subs.get_mut(&serial).unwrap().published=true;}
 fn retire_if_done(&mut self,_:u64){}
 @DEADLINE@
 @RETURNED@
}
#[test]fn bounded_refills_watchdog_and_unrelated_progress(){
 let mut c=Core::new(132);CLOCK.store(0,Ordering::Relaxed);let mut left=132;
 while left!=0{
  let now=now_ns();let wake=c.next_deadline(now).unwrap();assert!(wake>now);
  // Four 100us chunks finish before this probe. Charge 50us to processing
  // completions and refilling; use the production return and deadline bodies.
  CLOCK.store(wake+50_000,Ordering::Relaxed);
  for _ in 0..left.min(4){left-=1;let last=left==0;c.submit_returned(1,last,Done{answer:0,native_fence:Some(17),fence:last.then_some(17),echoed:17});}
  assert_eq!(c.subs[&1].sent_ns,Some(0),"progress moved the watchdog clock");
 }
 assert!(now_ns()<10_000_000,"132 short chunks inherited coarse probes for {}ns",now_ns());
 assert!(c.subs[&1].published);assert_eq!(c.subs[&1].progress_ns,Some(now_ns()));
 let mut c=Core::new(2);CLOCK.store(1_999_950_000,Ordering::Relaxed);
 c.submit_returned(1,false,Done{answer:0,native_fence:None,fence:None,echoed:0});
 assert_eq!(c.next_deadline(now_ns()),Some(2_000_000_000),"recent progress postponed the original watchdog");
 let mut c=Core::new(2);
 for now in [10_000_000,10_100_000,11_000_000]{CLOCK.store(now,Ordering::Relaxed);c.last_progress=now;
  assert_eq!(c.next_deadline(now),Some(completion_timing::next_probe(0,now)),"unrelated traffic reset this submission's probes");}
}
'''.replace('@POLICY@',policy).replace('@DEADLINE@',item(source,'fn next_deadline(')).replace('@RETURNED@',item(source,'fn submit_returned(')))
if __name__=='__main__':unittest.main()
