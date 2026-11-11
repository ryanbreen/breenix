"""Execute shared wait/deadline selection for an interrupt-free native device."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class PolledWait(unittest.TestCase):
 def test_native_poll_refills_after_reaping_before_waiting(self):
  source=item((ROOT/'kernel/src/gpu/mod.rs').read_text(),'    fn poll_traced(&mut self').replace('#[cfg(target_arch = "x86_64")]', '')
  fixture=r'''
use std::{cell::Cell,sync::atomic::{AtomicU64,Ordering}};
static REAPER:AtomicU64=AtomicU64::new(1);
thread_local!{static BACKEND:Cell<u32>=const{Cell::new(6)};}
mod abi{pub mod backend{pub const GK208:u32=6;}}
mod dev{pub fn backend()->u32{crate::BACKEND.with(Cell::get)}use std::cell::Cell;}
fn now_ns()->u64{123}
struct SubmitTrace;
struct Core{events:Vec<&'static str>,queued:usize,published:usize}
impl Core{
 fn reap_all(&mut self){self.events.push("reap");}
 fn watchdog(&mut self,now:u64){assert_eq!(now,123);self.events.push("watchdog");}
 fn flush_traced(&mut self,_:Option<&SubmitTrace>){self.events.push("flush");let count=self.queued.min(4);self.queued-=count;self.published+=count;}
 POLL
}
fn main(){for reaper in [0,1]{for backend in [4,5,abi::backend::GK208]{
 REAPER.store(reaper,Ordering::Release);BACKEND.with(|b|b.set(backend));assert_eq!(dev::backend(),backend);
 let mut c=Core{events:Vec::new(),queued:9,published:0};c.poll_traced(None);
 let refilled=reaper==0||backend==6;
 assert_eq!(c.published,if refilled{4}else{0},"native completion polling must refill available slots");
 assert_eq!(c.queued,if refilled{5}else{9});
 assert_eq!(c.events,if refilled{vec!["reap","watchdog","flush"]}else{vec!["reap","watchdog"]});
}}}
'''.replace('POLL',source)
  priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'poll.rs').write_text(fixture)
   subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'poll.rs'),'-o',str(p/'poll')],check=True)
   subprocess.run([str(p/'poll')],check=True)

 def test_native_waits_poll_without_a_quarter_second_sleep(self):
  core=(ROOT/'kernel/src/gpu/mod.rs').read_text()
  wait=(item(core,'fn wait_for<')+'\n'+item(core,'fn wait_for_traced<')).replace('#[cfg(target_arch = "x86_64")]', '')
  deadline=item(core,'    fn next_deadline(')
  fixture=r'''
#![allow(dead_code)] // Minimal model for the extracted scheduler functions.
use std::{cell::{Cell,RefCell},collections::{BTreeMap,VecDeque},sync::atomic::{AtomicU64,Ordering}};
mod tracing {pub mod providers {pub mod gpu {
 pub struct Counter;impl Counter {pub fn increment(&self){}}
 pub static GPU_WAIT_LONG:Counter=Counter;
 pub static GPU_WAIT_SILENT:Counter=Counter;
 pub static GPU_WAIT_SILENT_FOUND:Counter=Counter;
 pub static GPU_WAIT_SILENT_POLLED:Counter=Counter;
}}}
const EXAMINE_NS:u64=250_000_000;const POLL_NS:u64=250_000;const SPIN_POLL_NS:u64=1_000_000;const SPIN_POLL_PAUSE:u32=1;
static EVENTS:AtomicU64=AtomicU64::new(0);static REAPER:AtomicU64=AtomicU64::new(1);
static SENT:AtomicU64=AtomicU64::new(0);static WAKES:AtomicU64=AtomicU64::new(0);
fn wake_reaper(){WAKES.fetch_add(1,Ordering::AcqRel);}
mod abi {pub mod backend {pub const SVGA3D_DX:u32=5;pub const GK208:u32=6;}}
mod limits {pub const WATCHDOG_NS:u64=2_000_000_000;}
thread_local! {static BACKEND:Cell<u32>=const{Cell::new(6)};static CLOCK:Cell<u64>=const{Cell::new(0)};static SLEEPS:Cell<u32>=const{Cell::new(0)};static POLLS:Cell<u32>=const{Cell::new(0)};static CORE:RefCell<Core>=RefCell::new(core());}
mod dev {pub fn backend()->u32{crate::BACKEND.with(std::cell::Cell::get)} pub fn is_svga()->bool{backend()==5}}
mod syscall {pub fn check_signals_for_wait()->Option<()>{None}}
mod time {pub fn get_ticks()->u64{crate::now_ns()/5_000_000}}
enum SubmitPhase { AdmissionPoll, AdmissionInspect, AdmissionPause, AdmissionSleep }
struct SubmitTrace;
struct Scope;
impl SubmitTrace {fn scope(&self,_:SubmitPhase)->Scope{Scope}}
mod gk208 {pub mod timing {
 pub struct Readback;
 impl Readback {pub fn phase(&self,_:bool){} pub fn poll(&self){} pub fn spin(&self){}
 pub fn blocked(&self,_:u64,_:u64,_:u64,_:u64){}}
}}
mod arch {
 pub fn with_interrupts<R>(f:impl FnOnce()->R)->R{f()}
 // NATIVE_PAUSE
}
mod per_cpu {pub fn preempt_count()->u32{1} pub fn preempt_disable(){} pub fn preempt_enable(){}}
#[derive(Debug,PartialEq)]enum Stop{Timeout,Interrupted}
struct Device;impl Device{fn outstanding(&self)->usize{1}}
struct Sub{published:bool,sent_ns:Option<u64>,progress_ns:Option<u64>}struct Sync{failed:bool,sent_ns:Option<u64>}
#[derive(PartialEq)]enum CtxState{Live}
struct Ctx{warm_catalogue:bool,state:CtxState,dropped:bool,warm:u32,warm_after:u64}
struct Core{stalled:bool,dev:Option<Device>,subs:BTreeMap<u64,Sub>,syncs:BTreeMap<u64,Sync>,quarantined_at:Option<u64>,last_progress:u64,contexts:BTreeMap<u64,Ctx>,backlog:VecDeque<()>}
fn catalogue_mask()->u32{31}
fn core()->Core{Core{stalled:false,dev:Some(Device),subs:BTreeMap::new(),syncs:BTreeMap::new(),quarantined_at:None,last_progress:0,contexts:BTreeMap::new(),backlog:VecDeque::new()}}
impl Core {
 fn poll_traced(&mut self,_:Option<&SubmitTrace>){POLLS.with(|v|v.set(v.get()+1));}
 fn progress_readback(&mut self){}
 // DEADLINE
}
fn now_ns()->u64{CLOCK.with(Cell::get)}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn wait_event(_:u64,wake:Option<u64>)->Result<(),i32>{SLEEPS.with(|v|v.set(v.get()+1));CLOCK.with(|c|c.set(wake.unwrap()));Ok(())}
// WAIT
fn main(){
 for backend in [5,6,4] {
  BACKEND.with(|b|b.set(backend));CLOCK.with(|c|c.set(0));SLEEPS.with(|c|c.set(0));POLLS.with(|c|c.set(0));
  assert_eq!(wait_for(None,true,|_|(POLLS.with(Cell::get)>=2).then_some(7)),Ok(7));
  assert_eq!(SLEEPS.with(Cell::get),u32::from(backend==4),"backend {backend} slept awaiting an interrupt it cannot receive");
  let expected=if backend==4 {None} else {Some(123+POLL_NS)};
  assert_eq!(with_core(|c|c.next_deadline(123)).unwrap(),expected);
  with_core(|c|c.stalled=true);assert_eq!(with_core(|c|c.next_deadline(123)).unwrap(),None);
  CORE.with(|c|*c.borrow_mut()=core());
 }
 for backend in [5,6] {
  BACKEND.with(|b|b.set(backend));CLOCK.with(|c|c.set(0));SLEEPS.with(|c|c.set(0));POLLS.with(|c|c.set(0));
  SENT.store(0,Ordering::Release);WAKES.store(0,Ordering::Release);
  with_core(|c|c.backlog.push_back(()));
  assert_eq!(wait_for(None,true,|_|(POLLS.with(Cell::get)>=2).then_some(7)),Ok(7));
  assert_eq!(SLEEPS.with(Cell::get),u32::from(backend==6),"native queued work needs the worker, not a one-millisecond waiter spin");
  assert_eq!(SENT.load(Ordering::Acquire),u64::from(backend==6),"handoff must invalidate a worker deadline chosen before the queue was drained");
  assert_eq!(WAKES.load(Ordering::Acquire),u64::from(backend==6));
  CORE.with(|c|*c.borrow_mut()=core());
 }
 REAPER.store(0,Ordering::Release);BACKEND.with(|b|b.set(6));CLOCK.with(|c|c.set(0));
 SLEEPS.with(|c|c.set(0));POLLS.with(|c|c.set(0));SENT.store(0,Ordering::Release);WAKES.store(0,Ordering::Release);
 with_core(|c|c.backlog.push_back(()));
 assert_eq!(wait_for(None,true,|_|(POLLS.with(Cell::get)>=2).then_some(7)),Ok(7));
 assert_eq!(SLEEPS.with(Cell::get),0,"bootstrap must keep progressing when kgpu is absent");
 assert_eq!(SENT.load(Ordering::Acquire),0);assert_eq!(WAKES.load(Ordering::Acquire),0);
}
'''.replace('// DEADLINE',deadline).replace('// WAIT',wait).replace('// NATIVE_PAUSE',item((ROOT/'kernel/src/gpu/arch.rs').read_text(),'pub(super) fn poll_pause('))
  priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'wait.rs').write_text(fixture)
   subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'wait.rs'),'-o',str(p/'wait')],check=True)
   subprocess.run([str(p/'wait')],check=True)
if __name__=='__main__':unittest.main()
