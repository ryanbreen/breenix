"""Reject incomplete routing evidence and a recovered fatal kernel thread."""
from pathlib import Path
import runpy
import unittest

check = runpy.run_path(str(Path(__file__).resolve().parents[1] / "scripts/check-gpu-fault-routing.py"))["check"]
VALID = ("GPU_FAULT_TEST user syscall=500 handoff=1\n"
         "[DATA_ABORT] FAR=0x700000000000 ELR=0xffff000040200000 ESR=0x96000047 from_el0=0\n"
         "GPU_FAULT_TEST recovered signal=11 kernel=running handoff=1\n"
         "[GPU_FAULT_TEST kernel privilege=kernel handoff=1]\n"
         "[DATA_ABORT] FAR=0x700000001000 ELR=0xffff000040200000 ESR=0x96000007 from_el0=0\n"
         "[GPU_PANIC_SCREEN shown=1 observed=1 pixels=4096000 mismatches=0]\n")


class FaultRoutingCheckerTest(unittest.TestCase):
    def test_ordered_routing_and_screen(self):
        self.assertTrue(check(VALID)[0])

    def test_reject_missing_fault_or_incorrect_routing(self):
        for bad in (
            VALID.replace("[DATA_ABORT] FAR=0x700000000000", "announcement FAR=0x700000000000"),
            VALID.replace("from_el0=0", "from_el0=1", 1),
            VALID.replace("ESR=0x96000047", "ESR=0x92000047"),
            VALID.replace("signal=11", "signal=9"),
            VALID.replace("privilege=kernel", "privilege=user"),
            VALID.replace("mismatches=0", "mismatches=1"),
            VALID + "[DATA_ABORT] deferring process cleanup\n",
            VALID + "[GPU_FAULT_TEST kernel unexpectedly resumed]\n",
            VALID + "[BXCAP:BEGIN edge=LOCKUP]\n",
            VALID + "[BXCAP:BEGIN edge=PANIC]\n",
            VALID + "[DATA_ABORT] FAR=0x1234 from_el0=0\n",
            VALID + VALID,
            "\n".join(reversed(VALID.splitlines())),
        ):
            with self.subTest(serial=bad):
                self.assertFalse(check(bad)[0])


if __name__ == "__main__":
    unittest.main()
