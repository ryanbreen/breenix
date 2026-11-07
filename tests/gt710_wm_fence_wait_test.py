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

    def test_client_fence_wait_uses_worker_notification(self):
        core = source('kernel/src/gpu/mod.rs')
        code = item(core, 'fn op_wait(')
        self.assertIn('let outcome = wait_for_completion(', code)
        # The worker-owned caller CPU oracle is gt710_worker_fence_wait_test.py.

    def test_composite_latch_wait_sleeps_on_a_fine_wake(self):
        code = item(source('kernel/src/gpu/display.rs'), 'pub(super) fn composite_present_time(')
        self.assertIn('let id = wait_for_completion(None, true, false,', code)
        self.assertIn('return wait_for_completion(None, true, true,', code)
        # The production loop's CPU/deadline/signal/ready checks, for both
        # modes, run in gt710_worker_fence_wait_test.py.


if __name__ == '__main__':
    unittest.main()
