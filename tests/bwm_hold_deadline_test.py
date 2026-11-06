"""Execute the response-hold wait and shared descriptor park against a tick model."""
import unittest
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing
source = timing.source

class HoldDeadline(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_hold_deadline_is_not_rounded_to_a_scheduler_tick(self):
        wake = source('userspace/programs/src/bwm/wake.rs').replace('//!', '//')
        precise = 'wait_ns' in wake
        call = 'wake::wait_ns(ms, now, Some(deadline))' if precise else 'Some(ms as u64 * 1_000_000)'
        fixture = '''
mod wake { WAKE }
#[test]fn hold_uses_exact_monotonic_remaining_time(){
 for offset in [0,1,999_999] {
  let now=1_000_000_000+offset;let deadline=now+1_200_000;
  let ms=wake::timeout(false,false,80_000_000_000,now,
   wake::Deadlines{presentation:Some(deadline),..Default::default()});
  assert_eq!(CALL,Some(1_200_000),"hold consumed dispatch guard through millisecond rounding");
 }
}
'''.replace('WAKE',wake).replace('CALL',call)
        self.run_fixture('#![allow(dead_code)]\n'+fixture)
        bwm = source('userspace/programs/src/bwm.rs')
        self.assertIn('io::poll_ns(self.fds', bwm)
        self.assertIn('wake::wait_ns(timeout, pacing_ns(), hold)', bwm)
        self.assertNotIn('let monotonic_ns = monotonic_ms()', bwm)

    def test_descriptor_park_arms_the_exact_deadline_and_accepts_early_readiness(self):
        park = item(source('kernel/src/syscall/multiplex.rs'), '    pub(super) fn park(')
        fixture = r'''
#![allow(dead_code)] // Park has an ARM-only address-space restore branch.
use std::cell::{Cell,RefCell};
thread_local!{static NOW:Cell<u64>=const{Cell::new(1_000_999_999)};static DUE:Cell<u64>=const{Cell::new(u64::MAX)};static SCHED:RefCell<task::scheduler::Scheduler>=RefCell::new(task::scheduler::Scheduler{thread:task::scheduler::Thread{state:task::thread::ThreadState::Ready,blocked_in_syscall:false,wake_time_ns:None}});}
mod time{pub mod wakeup{pub fn arm(d:u64)->bool{crate::DUE.with(|v|v.set(d));true}}}
mod task{
 pub mod thread{#[derive(PartialEq,Eq)]pub enum ThreadState{BlockedOnIO,Ready}}
 pub mod scheduler{
  pub struct Thread{pub state:super::thread::ThreadState,pub blocked_in_syscall:bool,pub wake_time_ns:Option<u64>}
  impl Thread{pub fn set_ready(&mut self){self.state=super::thread::ThreadState::Ready;}}
  pub struct Scheduler{pub thread:Thread}
  impl Scheduler{pub fn block_current_for_io_with_timeout(&mut self,d:Option<u64>){self.thread.state=super::thread::ThreadState::BlockedOnIO;self.thread.wake_time_ns=d;}
   pub fn wake_expired_timers(&mut self){if self.thread.wake_time_ns.is_some_and(|d|crate::NOW.with(|n|n.get())>=d){self.thread.state=super::thread::ThreadState::Ready;}}
   pub fn current_thread_mut(&mut self)->Option<&mut Thread>{Some(&mut self.thread)}}
  pub fn with_current<R>(f:impl FnOnce(&mut Scheduler)->R)->Option<R>{Some(crate::SCHED.with(|s|f(&mut s.borrow_mut())))}
  pub fn yield_current(){}
 }
}
mod per_cpu{pub fn preempt_enable(){}pub fn preempt_disable(){}}
fn arch_halt_while_blocked(){let due=DUE.with(Cell::get);NOW.with(|n|n.set(due.min(1_005_000_000)));}
fn check_signals_for_wait()->Option<()>{None}
mod handlers{pub fn poll_ensure_address_space(){}}
mod multiplex{
 fn now()->u64{crate::NOW.with(|n|n.get())}
 pub struct WaitSet{pub notified:bool}
 impl WaitSet{fn register(&self)->bool{true}fn unregister(&self){} PARK }
}
#[test]fn exact_wake_precedes_the_tick(){
 let deadline=1_002_199_999;
 multiplex::WaitSet{notified:true}.park(deadline,||false).unwrap();
 assert_eq!(NOW.with(Cell::get),deadline,"descriptor timeout waited for the 5ms tick");
}
#[test]fn readiness_does_not_sleep(){
 NOW.with(|n|n.set(1_000_999_999));
 multiplex::WaitSet{notified:true}.park(1_002_199_999,||true).unwrap();
 assert_eq!(NOW.with(Cell::get),1_000_999_999);
}
'''.replace('PARK',park)
        self.run_fixture(fixture)

if __name__ == '__main__':
    unittest.main()
