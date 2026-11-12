"""Replay icon-sized calls through the production admission and BGL caller.

The engine model delivers receipts independently of scheduler ticks. This is a
call-count check, not a claim about GPU pixels or elapsed hardware time.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class IconsAdmission(unittest.TestCase):
    def test_one_admission_per_step_across_the_measured_icon_prefix(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        admission = core[core.index('    let mut waited = false;', core.index('fn submit_stream(')):
                         core.index('    let admitted_ids = named_ids.clone();')]
        admission = admission[:admission.index('    #[cfg(target_arch = "x86_64")]\n    { drop(phase.take());')]
        admission = admission.replace('#[cfg(target_arch = "x86_64")]', '')
        wait = item(core, 'fn wait_for_traced<').replace('#[cfg(target_arch = "x86_64")]', '')
        caller = item((ROOT / 'libs/bgl/src/lib.rs').read_text(), '    pub fn submit(&mut self)')
        fixture = r'''
use std::{cell::{Cell,RefCell},collections::{BTreeMap,VecDeque},sync::{Arc,atomic::{AtomicU64,Ordering}}};
const SPIN_POLL_NS:u64=1_000_000;
const SPIN_POLL_PAUSE:u32=1;
const EXAMINE_NS:u64=250_000_000;
const EINVAL:i64=-22; const EIO:i64=-5; const EINTR:i64=-4;
static EVENTS:AtomicU64=AtomicU64::new(0);
static REAPER:AtomicU64=AtomicU64::new(1);
static SENT:AtomicU64=AtomicU64::new(0);
fn wake_reaper(){}
mod abi {pub mod backend {pub const SVGA3D_DX:u32=5;pub const GK208:u32=6;}}
mod dev {pub fn backend()->u32{6}}
mod limits {pub const IN_FLIGHT:usize=4;}
mod breengel_abi {pub mod errno {pub const EBUSY:i32=16;}}
mod arch {
 pub fn with_interrupts<R>(f:impl FnOnce()->R)->R{f()}
 // NATIVE_PAUSE
}
mod per_cpu {pub fn preempt_count()->u32{1} pub fn preempt_disable(){} pub fn preempt_enable(){}}
mod syscall {pub fn check_signals_for_wait()->Option<()>{None}}
mod time {pub fn get_ticks()->u64{crate::now_ns()/5_000_000}}
mod gk208 {pub mod timing {
 pub struct Readback;
 impl Readback {
  pub fn phase(&self,_:bool){} pub fn poll(&self){} pub fn spin(&self){}
  pub fn blocked(&self,_:u64,_:u64,_:u64,_:u64){}
 }
}}
mod window_targets {
 pub struct Writer {pub n:u64,pub serial:u64,pub fence:u64}
 impl Writer {pub fn new(_:u32,n:u64,serial:u64,fence:u64)->Self{Self{n,serial,fence}}
 pub fn ended(&self)->bool{false}}
}
mod display {pub fn write_blocked(_: &mut crate::Core,_:&[u32])->bool{false}}
#[derive(Debug,PartialEq)]enum Stop{Timeout,Interrupted}
#[derive(PartialEq)]enum CtxState{Live}
enum SubmitPhase { AdmissionPoll, AdmissionInspect, AdmissionPause, AdmissionSleep }
struct SubmitTrace;
struct Scope;
impl SubmitTrace {fn scope(&self,_:SubmitPhase)->Scope{Scope} fn wait_on(&self,_:Option<(usize,u64,u64)>){}}
struct Device {outstanding:usize}
impl Device {fn outstanding(&self)->usize{self.outstanding}}
struct ContextState {state:CtxState,dropped:bool,inflight:VecDeque<(u64,u64)>,depth:Option<()>,busy:bool,issued:u64}
struct Resource {dropped:bool,lost:bool,window_sealed:bool,reading:bool,progress_context:Option<()>,holds:u32,writers:u32,accepted:Vec<Arc<window_targets::Writer>>}
struct Core {stalled:bool,dev:Option<Device>,backlog:VecDeque<()>,contexts:BTreeMap<u32,ContextState>,resources:BTreeMap<u32,Resource>,serial:u64,doorbells:u64}
impl Core {
 fn trace_admission(&self,_:u32,_:usize,_:Option<&SubmitTrace>){}
 fn new()->Self{Self{stalled:false,dev:Some(Device{outstanding:0}),backlog:VecDeque::new(),
  contexts:BTreeMap::from([(1,ContextState{state:CtxState::Live,dropped:false,inflight:VecDeque::new(),depth:None,busy:false,issued:0})]),
  resources:BTreeMap::from([(7,Resource{dropped:false,lost:false,window_sealed:false,reading:false,progress_context:None,holds:0,writers:0,accepted:Vec::new()})]),serial:0,doorbells:0}}
 fn serial(&mut self)->u64{self.serial+=1;self.serial}
 fn device_fence(&self)->u64{self.serial+1}
 fn release_depth(&mut self,id:u32){self.contexts.get_mut(&id).unwrap().depth=None;}
 fn poll_traced(&mut self,_:Option<&SubmitTrace>){
  CLOCK.with(|c|c.set(c.get()+10_000));
  let ctx=self.contexts.get_mut(&1).unwrap();
  while ctx.inflight.front().is_some_and(|&(_,due)|due<=now_ns()) {
   ctx.inflight.pop_front();self.dev.as_mut().unwrap().outstanding-=1;
  }
 }
 fn progress_readback(&mut self){}
}
thread_local! {
 static CLOCK:Cell<u64>=const{Cell::new(0)};
 static SLEEPS:Cell<u32>=const{Cell::new(0)};
 static CALLS:Cell<u64>=const{Cell::new(0)};
 static CORE:RefCell<Core>=RefCell::new(Core::new());
}
fn now_ns()->u64{CLOCK.with(Cell::get)}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn wait_event(_:u64,_:Option<u64>)->Result<(),i32>{
 SLEEPS.with(|s|s.set(s.get()+1));
 CLOCK.with(|c|c.set((c.get()/5_000_000+1)*5_000_000));Ok(())
}
// WAIT
fn admit()->Result<(Arc<window_targets::Writer>,bool),i64>{
 let id=1;let written=vec![7];let named_ids=vec![7];let alias=|_:&u32|false;
 let trace=None;
 // ADMISSION
 Ok((writer,waited))
}
struct Answer{fence:u64}
mod gpu {
 pub fn submit(handle:u64,words:&[u32])->(i64,crate::Answer){
  assert_eq!(handle,1);assert!(!words.is_empty());
  crate::CALLS.with(|c|c.set(c.get()+1));
  let (writer,_waited)=match crate::admit(){Ok(w)=>w,Err(e)=>return(e,crate::Answer{fence:0})};
  crate::with_core(|c|{
   let ctx=c.contexts.get_mut(&1).unwrap();
   assert_eq!(writer.n,ctx.issued);
   ctx.inflight.push_back((writer.serial,crate::now_ns()+1_100_000));
   ctx.busy=false;c.dev.as_mut().unwrap().outstanding+=1;
   // Substitute the device send only: a successful admission publishes new work.
   c.doorbells+=1;
   let r=c.resources.get_mut(&7).unwrap();r.holds-=1;r.writers-=1;r.accepted.clear();
  });
  (0,crate::Answer{fence:writer.fence})
 }
}
struct Fence{context:u64,handle:u64}
struct BglContext{handle:u64,words:Vec<u32>,handed:Cell<bool>}
impl BglContext {
 // CALLER
}
fn main(){
 let mut context=BglContext{handle:1,words:Vec::new(),handed:Cell::new(false)};
 // Two clears and 10,000 batched draws per icon. Replay all four icons;
 // checkpoint the two captures' 10,002 and 22,582 call prefixes too.
 for step in 1..=40_008 {
  context.words.push(step);
  let fence=context.submit().expect("admission must wait, not return a retry or short result");
  assert_eq!(fence.context,1);assert!(fence.handle>0);assert!(context.words.is_empty());
  assert_eq!(CALLS.with(Cell::get),step as u64,"BGL retried an admitted step");
  with_core(|c|{assert_eq!(c.contexts[&1].issued,step as u64);assert_eq!(c.doorbells,step as u64);});
  if [10_002,22_582,40_008].contains(&step){println!("prefix={step} calls={step} admitted={step}");}
 }
 // Polling the existing receipts without submitting must publish no new work.
 let before=CORE.with(|c|c.borrow().doorbells);
 for _ in 0..200 {with_core(|c|c.poll_traced(None));}
 assert_eq!(CORE.with(|c|c.borrow().doorbells),before);
 assert_eq!(SLEEPS.with(Cell::get),0,"native admission handed its receipt to the tick worker");
}
'''.replace('// WAIT', wait).replace('// NATIVE_PAUSE',item((ROOT/'kernel/src/gpu/arch.rs').read_text(),'pub(super) fn poll_pause(')).replace('// ADMISSION', admission).replace('// CALLER', caller)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-icons-admission-') as directory:
            folder = Path(directory)
            source, binary = folder / 'replay.rs', folder / 'replay'
            source.write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(source), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == '__main__':
    unittest.main()
