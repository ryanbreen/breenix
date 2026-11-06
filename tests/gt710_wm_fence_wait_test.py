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
        code = item(source('kernel/src/gpu/mod.rs'), 'fn op_wait(')
        # G2/G26: the unmerged unbounded-spin assertion contradicted the shared
        # GPU wait-no-spin contract. Keep fast receipts and bound long-wait CPU.
        self.run_receipt_wait(self.policy(code, 'let outcome = wait_for'), r'''
fn main(){
 for delay in [80_000,50_000_000] {
  CLOCK.with(|c|c.set(0));SLEPT.with(|c|c.set(0));SLEEPS.with(|c|c.set(0));
  LAST_WINDOW.with(|c|c.set(0));MAX_MASKED.with(|c|c.set(0));
  IRQS.with(|c|c.set(false));PREEMPT.with(|c|c.set(1));
  CORE.with(|c|{*c.borrow_mut()=Core::new(0,delay);c.borrow_mut().backlog.clear();});
  assert_eq!(wait_for_traced(Some(200_000_000),true,|c|c.done.then_some(7),None,POLICY,None),Ok(7));
  let wall=now_ns();let cpu=wall-SLEPT.with(Cell::get);
  if delay<1_000_000 {assert_eq!(SLEEPS.with(Cell::get),0,"fast receipt acquired a timer delay");}
  else {assert!(wall>=20_000_000);assert!(cpu*100<=wall*5,"long client WAIT monopolized CPU: {cpu}/{wall}");}
 }
}
'''.replace('POLICY', self.policy(code, 'let outcome = wait_for')))

    def test_composite_latch_wait_does_not_add_a_timer_dispatch(self):
        code = item(source('kernel/src/gpu/display.rs'), 'pub(super) fn composite_present_time(')
        self.run_receipt_wait(self.policy(code, 'let id = wait_for'))


if __name__ == '__main__':
    unittest.main()
