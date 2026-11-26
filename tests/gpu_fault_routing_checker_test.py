"""Reject incomplete routing evidence and a recovered fatal kernel thread."""
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[1]
check = runpy.run_path(str(ROOT / "scripts/check-gpu-fault-routing.py"))["check"]
# Excerpt of ARM64 boot 20261126T204534Z-qemu-35291: the probe relays the
# program's piped stdout, so the op 2 marker lands inside the abort's dump.
CAPTURED = (ROOT / "tests/fixtures/gpu_fault_acceptance_relayed_marker_serial.txt").read_text()
SYSCALL_ABORT = (
    "[DATA_ABORT] FAR=0x700000000000 ELR=0xffff000040200000 ESR=0x96000047 from_el0=0\n"
    "[FATAL_REGS] label=DATA_ABORT cpu=1 esr=0x96000047 far=0x700000000000\n"
    "  x19=0x0 sp=0x0 tid=23 name=/usr/bin/gpu_fault_acceptance_main\n"
    "[FATAL_POSTMORTEM] cpu=1 label=DATA_ABORT\n"
    "[DATA_ABORT] deferring process cleanup\n"
    "[DATA_ABORT] deferred_tid=23 queued=1\n")
KERNEL_ABORT = (
    "[DATA_ABORT] FAR=0x700000001000 ELR=0xffff000040200000 ESR=0x96000007 from_el0=0\n"
    "[FATAL_REGS] label=DATA_ABORT cpu=3 esr=0x96000007 far=0x700000001000\n"
    "  x19=0x0 sp=0x0 tid=25 name=gpu-fault-test\n"
    "[FATAL_POSTMORTEM] cpu=3 label=DATA_ABORT\n")
MARKER = "GPU_FAULT_TEST user syscall=0x7fff0000 handoff=1\n"
RECOVERED = "GPU_FAULT_TEST recovered signal=11 kernel=running handoff=1\n"
PRIVILEGE = "[GPU_FAULT_TEST kernel privilege=kernel handoff=1]\n"
SCREEN = "[GPU_PANIC_SCREEN shown=1 observed=1 pixels=4096000 mismatches=0]\n"
VALID = MARKER + SYSCALL_ABORT + RECOVERED + PRIVILEGE + KERNEL_ABORT + SCREEN
UNRELATED_ABORT = (
    "[DATA_ABORT] FAR=0x700000000000 ELR=0xffff000040300000 ESR=0x96000047 from_el0=0\n"
    "[FATAL_REGS] label=DATA_ABORT cpu=2 esr=0x96000047 far=0x700000000000\n"
    "  x19=0x0 sp=0x0 tid=9 name=kworker\n"
    "[FATAL_POSTMORTEM] cpu=2 label=DATA_ABORT\n")


class FaultRoutingCheckerTest(unittest.TestCase):
    def test_ordered_routing_and_screen(self):
        self.assertTrue(check(VALID)[0])

    def test_relayed_program_lines_may_trail_kernel_records(self):
        # The program's lines reach serial only when the probe relays its pipe.
        for relayed in (
            SYSCALL_ABORT + MARKER + RECOVERED + PRIVILEGE + KERNEL_ABORT + SCREEN,
            SYSCALL_ABORT + MARKER + PRIVILEGE + RECOVERED + KERNEL_ABORT + SCREEN,
            SYSCALL_ABORT + PRIVILEGE + MARKER + KERNEL_ABORT + RECOVERED + SCREEN,
        ):
            with self.subTest(serial=relayed):
                self.assertEqual(check(relayed), (True, check(VALID)[1]))

    def test_captured_relayed_marker_boot(self):
        self.assertTrue(check(CAPTURED)[0], check(CAPTURED)[1])

    def test_reject_unrelated_abort_before_op2(self):
        for bad in (
            UNRELATED_ABORT + VALID,
            UNRELATED_ABORT.replace("0x700000000000", "0x700000002000") + VALID,
            MARKER + UNRELATED_ABORT + SYSCALL_ABORT + RECOVERED + PRIVILEGE + KERNEL_ABORT + SCREEN,
            # The program's op 2 fault is absent and an unrelated thread faults at its address.
            MARKER + UNRELATED_ABORT + SYSCALL_ABORT.split("[DATA_ABORT] deferring")[0].replace(
                "[DATA_ABORT] FAR=0x700000000000", "announcement FAR=0x700000000000")
            + "[DATA_ABORT] deferring process cleanup\n[DATA_ABORT] deferred_tid=9 queued=1\n"
            + RECOVERED + PRIVILEGE + KERNEL_ABORT + SCREEN,
            VALID.replace("name=/usr/bin/gpu_fault_acceptance_main", "name=kworker"),
            CAPTURED.replace("name=/usr/bin/gpu_fault_acceptance_main", "name=probe_main"),
        ):
            with self.subTest(serial=bad):
                self.assertFalse(check(bad)[0])

    def test_reject_missing_fault_or_incorrect_routing(self):
        for bad in (
            VALID.replace("[DATA_ABORT] FAR=0x700000000000", "announcement FAR=0x700000000000"),
            VALID.replace("from_el0=0", "from_el0=1", 1),
            VALID.replace("ESR=0x96000047", "ESR=0x92000047"),
            VALID.replace("ESR=0x96000047", "ESR=0x96000007"),
            VALID.replace("ESR=0x96000007", "ESR=0x96000047"),
            VALID.replace("deferred_tid=23", "deferred_tid=24"),
            VALID.replace("[DATA_ABORT] deferred_tid=23 queued=1\n", ""),
            VALID.replace("name=gpu-fault-test", "name=kworker"),
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

    def test_reject_orders_the_code_guarantees(self):
        for bad in (
            # One pipe: the child's marker is relayed before the parent's recovery.
            RECOVERED + SYSCALL_ABORT + MARKER + PRIVILEGE + KERNEL_ABORT + SCREEN,
            # The parent writes its recovery only after reaping the faulted child.
            MARKER + RECOVERED + SYSCALL_ABORT + PRIVILEGE + KERNEL_ABORT + SCREEN,
            # Kernel records follow the program's control flow.
            MARKER + PRIVILEGE + SYSCALL_ABORT + RECOVERED + KERNEL_ABORT + SCREEN,
            MARKER + KERNEL_ABORT + SYSCALL_ABORT + RECOVERED + PRIVILEGE + SCREEN,
            MARKER + SYSCALL_ABORT + RECOVERED + KERNEL_ABORT + PRIVILEGE + SCREEN,
            MARKER + SYSCALL_ABORT + RECOVERED + PRIVILEGE + SCREEN + KERNEL_ABORT,
        ):
            with self.subTest(serial=bad):
                self.assertFalse(check(bad)[0])


if __name__ == "__main__":
    unittest.main()
