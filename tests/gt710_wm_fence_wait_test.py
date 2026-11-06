"""Run the client WAIT and composite latch policies against delayed native receipts."""
from pathlib import Path
import re
import unittest
from gt710_gr_live_unwind_test import item
from gt710_receipt_timing_test import source
import gt710_readback_fence_wait_test as receipt

ROOT = Path(__file__).resolve().parents[1]


class WindowFenceWait(unittest.TestCase):
    run_receipt_wait = receipt.ReadbackFenceWait.run_receipt_wait

    def policy(self, code, marker):
        call = code.split(marker, 1)[1]
        call = call.replace('#[cfg(target_arch = "x86_64")]', '')
        if call.startswith('('):
            return 'false'  # The production wait_for wrapper selects bounded polling.
        self.assertTrue(call.startswith('_traced('), 'unknown production wait entry')
        policy = re.search(r'},\s*None,\s*(true|false),\s*None\)', call)
        self.assertIsNotNone(policy, 'missing production wait policy')
        return policy[1]

    def test_client_fence_wait_bounds_cpu_and_keeps_fast_receipts(self):
        core = source('kernel/src/gpu/mod.rs')
        code = item(core, 'fn op_wait(')
        self.assertIn('let outcome = wait_for_fine(', code)
        self.run_fine_wait(core)

    def run_fine_wait(self, core):
        fine = item(core, 'fn wait_for_fine<') if 'fn wait_for_fine<' in core else '''
fn wait_for_fine<T>(deadline:Option<u64>,interruptible:bool,f:impl FnMut(&mut Core)->Option<T>)->Result<T,Stop>{
 wait_for_traced(deadline,interruptible,f,None,false,None)
}
'''
        # W22 requires a sleeping fine wake, including for short receipts.
        self.run_receipt_wait('false', r'''
pub fn wait_event_fine(_:u64,wake:u64)->Result<(),i32>{
 SLEEPS.with(|s|s.set(s.get()+1));
 CLOCK.with(|c|{SLEPT.with(|s|s.set(s.get()+wake.saturating_sub(c.get())));c.set(wake)});Ok(())
}
fn main(){
 for delay in [80_000,1_100_000,50_000_000] {
  CLOCK.with(|c|c.set(0));SLEPT.with(|c|c.set(0));SLEEPS.with(|c|c.set(0));
  LAST_WINDOW.with(|c|c.set(0));MAX_MASKED.with(|c|c.set(0));
  IRQS.with(|c|c.set(false));PREEMPT.with(|c|c.set(1));
  CORE.with(|c|{*c.borrow_mut()=Core::new(0,delay);c.borrow_mut().backlog.clear();});
  assert_eq!(wait_for_fine(Some(200_000_000),true,|c|c.done.then_some(7)),Ok(7));
  let wall=now_ns();let cpu=wall-SLEPT.with(Cell::get);
  assert!(SLEEPS.with(Cell::get)>0,"fence wait spun instead of sleeping");
  assert!(wall<=delay+500_000,"fine wait fell back to a 5ms tick: {wall}/{delay}");
  if wall>=20_000_000 {assert!(cpu*100<=wall*5,"long client WAIT monopolized CPU: {cpu}/{wall}");}
 }
 CORE.with(|c|c.borrow_mut().done=true);
 assert_eq!(wait_for_fine(None,true,|_|Some(7)),Ok(7));
 assert_eq!(wait_for_traced(None,true,|_|Some(7),None,false,None),Ok(7));
 CLOCK.with(|c|c.set(0));CORE.with(|c|*c.borrow_mut()=Core::new(0,100_000_000));
 assert_eq!(wait_for_fine(Some(100_000),true,|c|c.done.then_some(7)),Err(Stop::Timeout));
 SIGNAL.with(|s|s.set(true));
 assert_eq!(wait_for_fine(None,true,|c|c.done.then_some(7)),Err(Stop::Interrupted));
}
''', extra_wait=fine)

    def test_composite_latch_wait_sleeps_on_a_fine_wake(self):
        code = item(source('kernel/src/gpu/display.rs'), 'pub(super) fn composite_present_time(')
        self.assertIn('let id = wait_for_fine(', code)
        self.assertIn('return wait_for_fine(', code)
        self.run_fine_wait(source('kernel/src/gpu/mod.rs'))


if __name__ == '__main__':
    unittest.main()
