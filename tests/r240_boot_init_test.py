"""Keep disk-backed PID 1 loading out of x86 boot's non-parkable idle context."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BootInitTest(unittest.TestCase):
    def test_pid1_load_runs_in_a_thread_that_can_wait_for_the_commit_owner(self):
        source = (ROOT / "kernel/src/main.rs").read_text()
        source = re.sub(r"(?m)^\s*//.*$", "", source)
        boot = source.split("fn kernel_main_continue() -> ! {", 1)[1]
        boot = boot.split("fn idle_thread_fn()", 1)[0]
        # TID 1 is idle and holds two brakes until the boot tail. If a commit
        # worker owns slot 0 when the main thread reads, AHCI cannot admit it.
        self.assertNotIn("let mut image = read_x86_production_init()", boot,
                         "boot idle reads the image while the port can be owned")
        handoff = re.search(r"task::kthread::kthread_run_on_cpu\(\s*"
                            r"run_x86_production_init,\s*\"production_init\",\s*0,?\s*\)", boot)
        self.assertIsNotNone(handoff, "submit the loader as a schedulable thread on the boot CPU")
        tail = boot[handoff.end():].split("loop {", 1)[0]
        self.assertIn("kernel::per_cpu::preempt_enable();", tail)
        loader = source.split("fn run_x86_production_init() {", 1)[1].split("\n}", 1)[0]
        self.assertIn("let mut image = read_x86_production_init();", loader)
        self.assertIn("image = read_default_x86_init();", loader)
        self.assertNotIn("preempt_disable()", loader)


if __name__ == "__main__":
    unittest.main()
