"""Run the readback wait against engine receipts between x86's 5 ms ticks.

The u8 capture had 748 readbacks over 5 ms; all blocked and crossed a tick.
This executes the production wait with a bounded queue and an independent
receipt clock. Sleeping dispatches kgpu only on the next delivered tick.
"""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ReadbackFenceWait(unittest.TestCase):
    def test_readback_completes_on_receipt_without_a_tick_handoff(self):
        self.run_receipt_wait('true')

    def test_submit_admission_completes_on_receipt_without_a_tick_handoff(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        admitted = core.split('let admitted = wait_for_traced', 1)[1].split('let writer = match admitted', 1)[0]
        admitted = admitted.replace('#[cfg(target_arch = "x86_64")]', '')
        policy = re.search(r'},\s*None,\s*(true|false),\s*trace\);', admitted)
        self.assertIsNotNone(policy, 'run the production admission policy')
        self.run_receipt_wait(policy[1])

    def run_receipt_wait(self, selected_policy):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        wait = item(core, 'fn wait_for_traced<').replace('#[cfg(target_arch = "x86_64")]', '')
        # Accommodate the explicit readback policy argument without changing
        # the reproduction, device clock or any of the assertions.
        policy = ', ' + selected_policy if 'fence_driven: bool' in wait else ''
        if 'submit_trace:' in wait:
            policy += ', None'
        arch = (ROOT / 'kernel/src/gpu/arch.rs').read_text()
        pause = item(arch, 'pub(super) fn poll_pause(') if 'pub(super) fn poll_pause(' in arch else ''
        fixture = r'''
use std::{cell::{Cell,RefCell},collections::VecDeque,sync::atomic::{AtomicU64,Ordering}};
const SPIN_POLL_NS:u64=1_000_000;
const SPIN_POLL_PAUSE:u32=1;
const EXAMINE_NS:u64=250_000_000;
static EVENTS:AtomicU64=AtomicU64::new(0);
static REAPER:AtomicU64=AtomicU64::new(1);
static SENT:AtomicU64=AtomicU64::new(0);
fn wake_reaper(){}
mod abi {pub mod backend {pub const SVGA3D_DX:u32=5;pub const GK208:u32=6;}}
mod dev {pub fn backend()->u32{crate::BACKEND.with(std::cell::Cell::get)}}
mod per_cpu {
 pub fn preempt_count()->u32{crate::PREEMPT.with(std::cell::Cell::get)}
 pub fn preempt_disable(){crate::PREEMPT.with(|c|c.set(c.get()+1));}
 pub fn preempt_enable(){crate::PREEMPT.with(|c|{assert!(c.get()>0);c.set(c.get()-1);});crate::now_ns();}
}
mod arch {
 pub fn with_interrupts<R>(f:impl FnOnce()->R)->R {
  let was=crate::IRQS.with(std::cell::Cell::get);
  assert_eq!(crate::PREEMPT.with(std::cell::Cell::get),0,"pending IRQ delivered before releasing syscall preemption");
  crate::IRQS.with(|c|c.set(true));crate::now_ns();
  let result=f();crate::IRQS.with(|c|c.set(was));result
 }
 // PAUSE
}
mod syscall {pub fn check_signals_for_wait()->Option<()>{crate::SIGNAL.with(|s|s.get().then_some(()))}}
mod time {pub fn get_ticks()->u64{crate::now_ns()/5_000_000}}
mod gk208 {pub mod timing {
 pub struct Readback;
 impl Readback {
  pub fn phase(&self,_:bool){} pub fn poll(&self){} pub fn spin(&self){}
  pub fn blocked(&self,_:u64,_:u64,_:u64,_:u64){}
 }
}}
thread_local! {
 static IRQS:Cell<bool>=const{Cell::new(false)};
 static PREEMPT:Cell<u32>=const{Cell::new(1)};
 static LAST_WINDOW:Cell<u64>=const{Cell::new(0)};
 static MAX_MASKED:Cell<u64>=const{Cell::new(0)};
 static WINDOWS:Cell<u64>=const{Cell::new(0)};
 static BACKEND:Cell<u32>=const{Cell::new(6)};
 static CLOCK:Cell<u64>=const{Cell::new(0)};
 static SLEEPS:Cell<u32>=const{Cell::new(0)};
 static SIGNAL:Cell<bool>=const{Cell::new(false)};
 static CORE:RefCell<Core>=RefCell::new(Core::new(0,80_000));
}
#[derive(Debug,PartialEq)]enum Stop{Timeout,Interrupted}
enum SubmitPhase { AdmissionPoll, AdmissionInspect, AdmissionPause, AdmissionSleep }
struct SubmitTrace;
struct Scope;
impl SubmitTrace {fn scope(&self,_:SubmitPhase)->Scope{Scope}}
struct Device {outstanding:usize}
impl Device {fn outstanding(&self)->usize{self.outstanding}}
struct Core {stalled:bool,dev:Option<Device>,backlog:VecDeque<()>,receipt:u64,delay:u64,done:bool}
impl Core {
 fn new(start:u64,delay:u64)->Self{Self{stalled:false,dev:Some(Device{outstanding:1}),backlog:VecDeque::from([()]),receipt:start+delay,delay,done:false}}
 fn poll_traced(&mut self,_:Option<&SubmitTrace>){
  CLOCK.with(|c|c.set(c.get()+10_000));
  if self.stalled || now_ns()<self.receipt{return;}
  if self.backlog.pop_front().is_some(){self.receipt=now_ns()+self.delay;}
  else {self.dev.as_mut().unwrap().outstanding=0;self.done=true;}
 }
 fn progress_readback(&mut self){}
}
fn now_ns()->u64{
 let now=CLOCK.with(Cell::get);
 if IRQS.with(Cell::get) && PREEMPT.with(Cell::get)==0 {
  MAX_MASKED.with(|m|m.set(m.get().max(now-LAST_WINDOW.with(Cell::get))));
  LAST_WINDOW.with(|c|c.set(now));WINDOWS.with(|c|c.set(c.get()+1));
 }
 now
}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn wait_event(_:u64,_:Option<u64>)->Result<(),i32>{
 SLEEPS.with(|s|s.set(s.get()+1));
 // A deadline shorter than a tick cannot dispatch the interrupt-free worker.
 CLOCK.with(|c|c.set((c.get()/5_000_000+1)*5_000_000));Ok(())
}
// WAIT
fn main(){
 // Both queued continuations and a receipt beyond the old 1 ms spin window.
 // Diagnostic tracing is absent too: completion cannot depend on recording it.
 for delay in [80_000,1_100_000] {for phase in [0,1_000_000,4_000_000] {for traced in [false,true] {
  BACKEND.with(|b|b.set(6));CLOCK.with(|c|c.set(phase));SLEEPS.with(|s|s.set(0));
  IRQS.with(|c|c.set(false));PREEMPT.with(|c|c.set(1));
  LAST_WINDOW.with(|c|c.set(phase));MAX_MASKED.with(|c|c.set(0));WINDOWS.with(|c|c.set(0));
  CORE.with(|c|*c.borrow_mut()=Core::new(phase,delay));
  let trace=gk208::timing::Readback;
  assert_eq!(wait_for_traced(Some(phase+20_000_000),true,|c|c.done.then_some(7),traced.then_some((&trace,1))POLICY),Ok(7));
  assert!(WINDOWS.with(Cell::get)>0,"native waiter never allowed IRQ delivery and preemption");
  assert!(MAX_MASKED.with(Cell::get)<=500_000,"native poll masked IRQs beyond 500 us");
  assert!(!IRQS.with(Cell::get));assert_eq!(PREEMPT.with(Cell::get),1);
  assert_eq!(SLEEPS.with(Cell::get),0,"a native fence waiter handed an observable engine receipt to the tick-driven worker");
  assert!(now_ns()-phase<=2*delay+20_000,"readback completed on a tick instead of its engine receipt");
 }}}
 // A missing receipt must still obey the caller deadline and pending signal.
 CLOCK.with(|c|c.set(0));LAST_WINDOW.with(|c|c.set(0));CORE.with(|c|*c.borrow_mut()=Core::new(0,100_000_000));
 assert_eq!(wait_for_traced(Some(100_000),true,|c|c.done.then_some(7),NonePOLICY),Err(Stop::Timeout));
 SIGNAL.with(|s|s.set(true));
 assert_eq!(wait_for_traced(None,true,|c|c.done.then_some(7),NonePOLICY),Err(Stop::Interrupted));
 SIGNAL.with(|s|s.set(false));
 // Virtual backends retain the worker handoff. A stalled native queue must
 // return to its recovery worker rather than monopolize the caller CPU.
 for backend in [4,5,6] {
  BACKEND.with(|b|b.set(backend));CLOCK.with(|c|c.set(0));SLEEPS.with(|s|s.set(0));
  CORE.with(|c|{*c.borrow_mut()=Core::new(0,1_100_000);c.borrow_mut().stalled=backend==6;});
  assert_eq!(wait_for_traced(Some(6_000_000),true,|c|c.done.then_some(7),NonePOLICY),if backend==6{Err(Stop::Timeout)}else{Ok(7)});
  assert!(SLEEPS.with(Cell::get)>0,"virtual/recovery waits must retain their blocking path");
 }
}
'''.replace('// WAIT', wait).replace('// PAUSE', pause).replace('POLICY', policy)
        if not pause:
            fixture = fixture[:fixture.index('mod arch {')] + fixture[fixture.index('mod syscall {'):]
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-receipt-wait-') as directory:
            folder = Path(directory)
            source, binary = folder / 'wait.rs', folder / 'wait'
            source.write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(source), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == '__main__':
    unittest.main()
