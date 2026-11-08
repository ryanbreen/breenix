"""W23: execute production completion deadlines/waits on a deterministic clock."""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    rev = os.environ.get('BREENIX_FENCE_WAIT_REV')
    if rev:
        return subprocess.run(['git', 'show', f'{rev}:{path}'], cwd=ROOT,
                              check=True, capture_output=True, text=True).stdout
    return (ROOT / path).read_text()


class WorkerFenceWait(unittest.TestCase):
    def test_production_deadlines_and_bounded_waits(self):
        src = source('kernel/src/gpu/mod.rs')
        wait = item(src, 'fn wait_for_completion<')
        deadline = item(src, 'fn next_deadline(')
        block = item(src, 'fn reaper_block(')
        period = re.search(r'const POLL_NS: u64 = ([0-9_]+);', src if 'const POLL_NS:' in src else (ROOT / 'kernel/src/gpu/completion_timing.rs').read_text()).group(1)
        modern = 'enum CompletionWait' in src
        policy = (ROOT / 'kernel/src/gpu/completion_timing.rs').read_text().replace('//!', '//') if modern else ''
        mode = item(src, '#[derive(Clone, Copy, PartialEq)]\nenum CompletionWait') if modern else ''
        fence_mode = 'CompletionWait::Fence' if modern else 'false'
        worker_mode = 'CompletionWait::Worker' if modern else 'false'
        content_mode = 'CompletionWait::Presentation' if modern else 'true'
        fixture = r'''
#![allow(dead_code)] // Only the production wait/deadline dependencies are modelled.
use std::{cell::{Cell,RefCell},collections::BTreeMap,sync::atomic::{AtomicU64,Ordering}};
const POLL_NS:u64=POLL_VALUE;const EXAMINE_NS:u64=250_000_000;
mod completion_timing { POLICY }
@MODE@
static EVENTS:AtomicU64=AtomicU64::new(0);static SENT:AtomicU64=AtomicU64::new(0);
thread_local!{static CLOCK:Cell<u64>=const{Cell::new(0)};static CPU:Cell<u64>=const{Cell::new(0)};static WORK:Cell<u64>=const{Cell::new(0)};
static READY:Cell<u64>=const{Cell::new(0)};static WORKER:Cell<u64>=const{Cell::new(0)};static NEXT:Cell<u64>=const{Cell::new(0)};static ARMED:Cell<u64>=const{Cell::new(0)};
static LATE:Cell<bool>=const{Cell::new(false)};static DONE:Cell<bool>=const{Cell::new(false)};
static PRESENTATION:Cell<Option<u64>>=const{Cell::new(None)};static SIGNAL:Cell<bool>=const{Cell::new(false)};static VIRTUAL:Cell<bool>=const{Cell::new(false)};
static CORE:RefCell<Core>=RefCell::new(Core::new());}
fn now_ns()->u64{CLOCK.with(Cell::get)}
fn advance(ns:u64){CLOCK.with(|c|c.set(c.get()+ns));}
fn charge(ns:u64){CPU.with(|c|c.set(c.get()+ns));advance(ns);}
struct Req{published:bool,failed:bool,sent_ns:Option<u64>,progress_ns:Option<u64>}
struct Context{warm_catalogue:bool,state:CtxState,dropped:bool,warm:u64,warm_after:u64}
#[derive(PartialEq)]enum CtxState{Live}
fn catalogue_mask()->u64{0}
struct Device;impl Device{fn outstanding(&self)->usize{usize::from(!DONE.with(Cell::get))}}
struct Core{subs:BTreeMap<u64,Req>,syncs:BTreeMap<u64,Req>,contexts:BTreeMap<u64,Context>,quarantined_at:Option<u64>,last_progress:u64,dev:Option<Device>,stalled:bool}
impl Core{
 fn new()->Self{Self{subs:BTreeMap::from([(1,Req{published:false,failed:false,sent_ns:Some(0),progress_ns:None})]),syncs:BTreeMap::new(),contexts:BTreeMap::new(),quarantined_at:None,last_progress:0,dev:Some(Device),stalled:false}}
 DEADLINE
 fn progress_readback(&mut self){if VIRTUAL.with(Cell::get){DONE.with(|c|c.set(true));}}
 fn poll_traced(&mut self, #[cfg(target_arch = "x86_64")] _:Option<()>){charge(50_000);self.observe();}
 fn observe(&mut self){if now_ns()>=READY.with(Cell::get){DONE.with(|c|c.set(true));EVENTS.fetch_add(1,Ordering::Release);}}
}
mod abi {pub mod backend {pub const SVGA3D_DX:u32=2;pub const GK208:u32=6;}}
mod limits {pub const WATCHDOG_NS:u64=2_000_000_000;}
mod dev {pub fn backend()->u32{crate::abi::backend::GK208}
 pub fn presentation_probe(_:bool)->Option<u64>{crate::PRESENTATION.with(|p|p.get().map(|latch|if crate::now_ns()<latch+30_000 {latch+30_000}else{crate::now_ns()+100_000}))}}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{charge(3_000);CORE.with(|c|Some(f(&mut c.borrow_mut())))}
fn wake_reaper(){}
mod syscall {pub fn check_signals_for_wait()->Option<()>{crate::SIGNAL.with(|s|s.get().then_some(()))}}
mod per_cpu {pub fn preempt_count()->u32{1}pub fn preempt_disable(){}pub fn preempt_enable(){}}
mod time {pub mod wakeup {pub fn arm(at:u64)->bool{crate::ARMED.with(|c|c.set(at));true}}}
mod task {pub mod scheduler {
 pub struct Sched;impl Sched{pub fn block_current_for_io_with_timeout(&mut self,wake:Option<u64>){crate::NEXT.with(|n|n.set(wake.unwrap()));}pub fn unblock(&mut self,_:u64){}}
 pub fn with_scheduler<T>(f:impl FnOnce(&mut Sched)->T)->Option<T>{Some(f(&mut Sched))}
 pub fn yield_current(){crate::WORKER.with(|c|c.set(crate::NEXT.with(std::cell::Cell::get)));}
}}
fn arch_halt_while_blocked(){}
#[derive(Debug,PartialEq)]enum Stop{Timeout,Interrupted}
BLOCK
fn worker_deadline()->u64{CORE.with(|c|c.borrow().next_deadline(now_ns()).unwrap())}
fn reset(ready:u64,late:bool){PRESENTATION.with(|p|p.set(None));CLOCK.with(|c|c.set(0));CPU.with(|c|c.set(0));WORK.with(|c|c.set(0));DONE.with(|c|c.set(false));READY.with(|c|c.set(ready));LATE.with(|c|c.set(late));CORE.with(|c|*c.borrow_mut()=Core::new());WORKER.with(|c|c.set(worker_deadline()));}
fn wait_event_prepared(_:u64,wake:Option<u64>,_:bool)->Result<(),i32>{
 // Sleep until the caller deadline, a worker notification, or the worker's next
 // production deadline. A single delayed worker models the captured 9ms stall.
 loop{
  let worker=WORKER.with(Cell::get);let worker=if LATE.with(Cell::get){worker+9_000_000}else{worker};
  let at=worker.min(wake.unwrap_or(u64::MAX));CLOCK.with(|c|c.set(c.get().max(at)));
  if at==worker {
   LATE.with(|c|c.set(false));WORK.with(|c|c.set(c.get()+55_000));advance(55_000);
   CORE.with(|c|c.borrow_mut().observe());
   reaper_block(1,EVENTS.load(Ordering::Acquire),SENT.load(Ordering::Acquire),Some(worker_deadline()));
  }
  if DONE.with(Cell::get)||now_ns()>=wake.unwrap_or(u64::MAX){return Ok(());}
 }
}
WAIT
fn observe(mode:bool,ready:u64,late:bool)->(u64,u64,u64){
 reset(ready,late);
 let result=if mode {wait_for_completion(Some(200_000_000),true,FENCE,|_|DONE.with(Cell::get).then_some(7))}
 else {wait_for_completion(Some(200_000_000),true,WORKER_MODE,|_|DONE.with(Cell::get).then_some(7))};
 assert_eq!(result,Ok(7));(now_ns(),CPU.with(Cell::get),WORK.with(Cell::get))
}
#[test]fn t_a_real_worker_deadlines_observe_short_fences_within_250us(){
 for ready in [80_000,150_000]{let(at,_,_)=observe(false,ready,false);assert!(at<=ready+250_000,"ready={ready} observed={at}");}
}
fn publication(latch:u64)->u64{
 reset(u64::MAX,false);PRESENTATION.with(|p|p.set(Some(latch)));
 assert_eq!(wait_for_completion(Some(100_000_000),true,CONTENT_MODE,|_|(now_ns()>=latch).then_some(7)),Ok(7));now_ns()
}
#[test]fn predicted_latch_deadline_is_armed_beyond_the_poll_period(){
 reset(u64::MAX,false);let latch=16_661_185+30_000;
 reaper_block(1,EVENTS.load(Ordering::Acquire),SENT.load(Ordering::Acquire),Some(latch));
 assert_eq!(ARMED.with(Cell::get),latch,"physical deadline would round to the 5ms tick");
}
#[test]fn t_b_content_is_published_within_200us_of_latch(){
 for phase in [100u64,700,949]{
  let accept=phase*1688*1_000_000_000/108_000_000;
  // A latch 10us after the old grid forces almost a full late poll.
  let latch=accept+1_510_000;let at=publication(latch);
  assert!(at<=latch+200_000,"phase={phase} latch={latch} publication={at}");
 }
}
#[test]fn t_c_closed_loop_retains_present_latency_margin(){
 let paint=observe(true,150_000,false).0 as f64/1e6;
 let pub_delay=(publication(1_510_000)-1_510_000)as f64/1e6;
 let period=1688.0*1066.0/108000.0;let deadline=950.0*1688.0/108000.0;
 for jitter in [0.0,1.25,2.5]{let mut values=Vec::new();
  for seed in 1..=300u64 {let mut rng=seed;let mut random=||{rng=rng.wrapping_mul(6364136223846793005).wrapping_add(1);(rng>>11)as f64/(1u64<<53)as f64};
   let mut commit=random()*period;
   for i in 0..330 {let get=commit+3.74;let phase=get%period;
    let latch=((get/period).floor()+if phase<deadline{1.0}else{2.0})*period;
    if i>=30 {values.push(latch-commit);}
    let publication=latch+pub_delay;let mut sample=commit+0.05;
    while sample<publication {sample+=3.0+5.0+random()*jitter;}
    commit=sample+3.0+0.10+0.33+paint+0.01;
   }
  }
  values.sort_by(f64::total_cmp);let median=values[values.len()/2];
  println!("closed loop jitter={jitter} median={median:.3}ms paint={paint:.3}ms publication={pub_delay:.3}ms");
  assert!(median<=16.0,"jitter={jitter} median={median} paint={paint} pub={pub_delay}");
  assert!(*values.last().unwrap()<=33.0);
 }
}
#[test]fn long_wait_combined_cpu_stays_below_five_percent(){
 for ready in [50_000_000,112_000_000]{let(at,caller,worker)=observe(true,ready,false);
 println!("fence {ready}ns: caller={caller}ns worker={worker}ns wall={at}ns");
 assert!(caller*100<=at*5,"caller CPU {caller}/{at}");assert!((caller+worker)*100<=at*5,"combined CPU {caller}+{worker}/{at}");}
}
#[test]fn t_d_late_worker_cannot_hold_a_short_fence_past_the_next_frame_start(){
 let(at,_,_)=observe(true,150_000,true);
 // The measured good frame budget has 4.2ms for acquire/paint/commit;
 // the captured 9.52ms fence pushes admission past the 11.11ms cliff.
 assert!(at<=400_000,"late worker stalled short fence to {at}");
 let previous_edge=16_661_185u64;let commit=previous_edge+2_539_000+at;
 let get=commit+3_740_000;let phase=get%previous_edge;
 assert!(phase<14_848_148,"late GET misses the next display edge");
}
#[test]fn ready_timeout_signal_and_virtual_progress_are_preserved(){
 reset(u64::MAX,false);assert_eq!(wait_for_completion(Some(100_000),true,WORKER_MODE,|_|None::<u8>),Err(Stop::Timeout));
 assert_eq!(wait_for_completion(None,true,FENCE,|_|Some(7)),Ok(7));
 reset(u64::MAX,false);VIRTUAL.with(|c|c.set(true));
 assert_eq!(wait_for_completion(Some(1_000_000),true,WORKER_MODE,|_|DONE.with(Cell::get).then_some(7)),Ok(7));VIRTUAL.with(|c|c.set(false));
 SIGNAL.with(|c|c.set(true));assert_eq!(wait_for_completion(None,true,FENCE,|_|None::<u8>),Err(Stop::Interrupted));
}
'''.replace('POLL_VALUE',period).replace('POLICY',policy).replace('@MODE@',mode).replace('DEADLINE',deadline).replace('BLOCK',block).replace('WAIT',wait).replace('FENCE',fence_mode).replace('WORKER_MODE',worker_mode).replace('CONTENT_MODE',content_mode)
        with tempfile.TemporaryDirectory(prefix='worker-fence-') as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(fixture)
            command = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '--test', '-Dwarnings']
            subprocess.run(command + [str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check'), '--test-threads=1', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
