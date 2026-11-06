"""Run the client WAIT and composite latch policies against delayed native receipts."""
from pathlib import Path
import re
import unittest
from gt710_gr_live_unwind_test import item
import gt710_readback_fence_wait_test as receipt

ROOT = Path(__file__).resolve().parents[1]


class WindowFenceWait(unittest.TestCase):
    run_receipt_wait = receipt.ReadbackFenceWait.run_receipt_wait

    def policy(self, code, marker):
        call = code.split(marker, 1)[1]
        call = call.replace('#[cfg(target_arch = "x86_64")]', '')
        policy = re.search(r'},\s*None,\s*(true|false),\s*None\)', call)
        return policy[1] if policy else 'false'

    def test_client_fence_wait_does_not_add_a_timer_dispatch(self):
        code = item((ROOT / 'kernel/src/gpu/mod.rs').read_text(), 'fn op_wait(')
        self.run_receipt_wait(self.policy(code, 'let outcome = wait_for'))

    def test_composite_latch_wait_does_not_add_a_timer_dispatch(self):
        code = item((ROOT / 'kernel/src/gpu/display.rs').read_text(), 'pub(super) fn composite_present_time(')
        self.run_receipt_wait(self.policy(code, 'let id = wait_for'))


if __name__ == '__main__':
    unittest.main()
