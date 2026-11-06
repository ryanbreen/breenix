"""Exercise the worker-owned completion wait with costly interrupt-free polling."""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class WorkerFenceWait(unittest.TestCase):
    def test_worker_fine_wakes_keep_polling_cost_off_the_sleeping_caller(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        wait_source = subprocess.run(['git', 'show', os.environ['BREENIX_FENCE_WAIT_REV'] + ':kernel/src/gpu/mod.rs'], cwd=ROOT, check=True, capture_output=True, text=True).stdout if os.environ.get('BREENIX_FENCE_WAIT_REV') else source
        name = 'wait_for_completion' if 'fn wait_for_completion<' in wait_source else 'wait_for_fine'
        wait = item(wait_source, 'fn ' + name + '<').replace(name, 'wait_for_completion')
        if name == 'wait_for_fine':
            wait = wait.replace('interruptible: bool,', 'interruptible: bool, _fine: bool,')
        block = item(source, 'fn reaper_block(')
        period = re.search(r'const POLL_NS: u64 = ([0-9_]+);', source).group(1)
        fixture = r'''
#![allow(dead_code)] // Driver fixture exposes the old and new wait dependencies.
use std::{cell::Cell,sync::atomic::{AtomicU64,Ordering}};
const POLL_NS:u64=POLL_VALUE;
static EVENTS:AtomicU64=AtomicU64::new(0);static SENT:AtomicU64=AtomicU64::new(0);
thread_local!{static CLOCK:Cell<u64>=const{Cell::new(0)};static CPU:Cell<u64>=const{Cell::new(0)};static WORK:Cell<u64>=const{Cell::new(0)};
static READY:Cell<u64>=const{Cell::new(50_000_000)};static NEXT:Cell<u64>=const{Cell::new(0)};
static ARMED:Cell<u32>=const{Cell::new(0)};static DONE:Cell<bool>=const{Cell::new(false)};static SIGNAL:Cell<bool>=const{Cell::new(false)};}
fn now_ns()->u64{CLOCK.with(Cell::get)}
fn charge(ns:u64){CPU.with(|c|c.set(c.get()+ns));CLOCK.with(|c|c.set(c.get()+ns));}
struct Core;
impl Core{fn progress_readback(&mut self){}}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{charge(3_000);Some(f(&mut Core))}
fn wake_reaper(){}
mod arch {pub fn poll(_: &mut crate::Core){crate::charge(50_000);crate::DONE.with(|d|d.set(crate::now_ns()>=crate::READY.with(std::cell::Cell::get)));}}
mod syscall {pub fn check_signals_for_wait()->Option<()>{crate::SIGNAL.with(|s|s.get().then_some(()))}}
mod per_cpu {pub fn preempt_count()->u32{1}pub fn preempt_disable(){}pub fn preempt_enable(){}}
mod time {pub mod wakeup {pub fn arm(at:u64)->bool{assert!(at<=crate::now_ns()+crate::POLL_NS);crate::ARMED.with(|n|n.set(n.get()+1));true}}}
mod task {pub mod scheduler {
 pub struct Sched;impl Sched{pub fn block_current_for_io_with_timeout(&mut self,wake:Option<u64>){crate::NEXT.with(|n|n.set(wake.unwrap()));}pub fn unblock(&mut self,_:u64){}}
 pub fn with_scheduler<T>(f:impl FnOnce(&mut Sched)->T)->Option<T>{Some(f(&mut Sched))}
 pub fn yield_current(){
  // Worker polling is real work on kgpu, not CPU time on the sleeping caller.
  crate::WORK.with(|n|n.set(n.get()+55_000));
  crate::CLOCK.with(|n|n.set(crate::NEXT.with(std::cell::Cell::get)+55_000));
  if crate::now_ns()>=crate::READY.with(std::cell::Cell::get){crate::DONE.with(|d|d.set(true));crate::EVENTS.fetch_add(1,std::sync::atomic::Ordering::Release);}
 }
}}
fn arch_halt_while_blocked(){}
#[derive(Debug,PartialEq)]enum Stop{Timeout,Interrupted}
BLOCK
fn wait_event(_:u64,deadline:Option<u64>)->Result<(),i32>{
 // kgpu's next_deadline requests 1.5ms while the device holds a request.
 // Keep the caller parked while that worker takes successive fine wakes.
 while !DONE.with(Cell::get)&&now_ns()<deadline.unwrap_or(u64::MAX){
  reaper_block(1,EVENTS.load(Ordering::Acquire),SENT.load(Ordering::Acquire),Some(now_ns()+POLL_NS));
  assert!(ARMED.with(Cell::get)>0,"worker deadline still uses the 5ms tick");
 }
 Ok(())
}
fn wait_event_fine(seen:u64,wake:u64)->Result<(),i32>{wait_event(seen,Some(wake))}
fn wait_event_prepared(seen:u64,wake:Option<u64>,_:bool)->Result<(),i32>{wait_event(seen,wake)}
WAIT
fn main(){
 for fine in [false,true] {for delay in [80_000,1_100_000,50_000_000]{
  CLOCK.with(|c|c.set(0));CPU.with(|c|c.set(0));WORK.with(|c|c.set(0));ARMED.with(|c|c.set(0));DONE.with(|c|c.set(false));READY.with(|c|c.set(delay));
  assert_eq!(wait_for_completion(Some(100_000_000),true,fine,|_|DONE.with(Cell::get).then_some(7)),Ok(7));
  assert!(now_ns()<=delay+1_600_000);let cpu=CPU.with(Cell::get);
  if delay>=20_000_000{assert!((cpu+WORK.with(Cell::get))*100<=now_ns()*5,"combined caller and worker CPU exceeded bound");assert!(cpu*100<=now_ns()*5,"caller did worker polling: {cpu}/{}",now_ns());}
 }}
 DONE.with(|c|c.set(false));READY.with(|c|c.set(u64::MAX));CLOCK.with(|c|c.set(0));
 assert_eq!(wait_for_completion(Some(100_000),true,false,|_|None::<u8>),Err(Stop::Timeout));
 let armed=ARMED.with(Cell::get);assert_eq!(wait_for_completion(None,true,false,|_|Some(7)),Ok(7));assert_eq!(ARMED.with(Cell::get),armed,"ready completion slept");
 SIGNAL.with(|s|s.set(true));assert_eq!(wait_for_completion(None,true,false,|_|None::<u8>),Err(Stop::Interrupted));
}
'''.replace('POLL_VALUE', period).replace('BLOCK', block).replace('WAIT', wait)
        with tempfile.TemporaryDirectory(prefix='worker-fence-') as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(fixture)
            command = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(command + [str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
