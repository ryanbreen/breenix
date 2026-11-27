#!/usr/bin/env python3
"""Drive the slot-0 driver with AHCI 1.3.1 and Intel LAPIC register doubles."""

from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from ahci_poller_race_test import function
from suite_verdict_test import FIXTURES, MANIFEST, SCORER

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "kernel/src/drivers/ahci/mod.rs"
LOST_NOTIFICATION = "AHCI_LOST_NOTIFICATION port=0 token=51849 CI=0x0 IS=0x21 TFD=0x50 HBA_IS=0x1\n"


class AhciCompletion(unittest.TestCase):
    def test_interrupt_and_timeout(self):
        source = SOURCE.read_text()
        registers = re.findall(
            r"(?m)^const (?:HBA_IS|PORT_(?:IS|IE|CI|TFD|CMD|SERR|SSTS|SCTL|SACT)|S[CS]TS_[A-Z0-9_]+|SCTL_[A-Z0-9_]+|PORT_(?:IRQ|TFD|CMD)_[A-Z0-9_]+|"
            r"AHCI_(?:CI_COMPLETION_LOOP_LIMIT|TRACKED_SLOT_MASK|TIMEOUT_SECS)|PORT_(?:READY|STOPPING|RESET_REQUIRED|OFFLINE|RECOVERY_QUEUED)): [^=]+=[^;]+;",
            source,
        )
        names = ["ack_port_interrupt", "detect_completed_slots", "handle_interrupt", "wait_cmd_slot0",
                 "wait_cmd_slot0_inner", "slot0_can_retire", "save_command_errors",
                 "command_receipt", "port_quiescent", "link_ready", "wait_for_recovery",
                 "ensure_port_ready", "recover_stopped_port", "recover_failed_command"]
        receipt = re.search(r"struct CommandReceipt \{[^}]+\}", source)[0]
        registers.append(receipt + "\nimpl CommandReceipt { " + function(source, "validate") + " }")
        # The baseline has an inline timeout error. The new helper, when present,
        # must run its production body against the very same device and tests.
        if "fn resolve_wait_timeout(" in source:
            names.extend(["resolve_wait_timeout", "record_lost_notification"])
            registers.append("#[derive(Clone, Copy)]\n" +
                             re.search(r"struct TimeoutRegisters \{[^}]+\}", source)[0])
        dump = function(source, "dump_timeout_state_free").split("{", 1)[0]
        registers.append(dump + "{ MODEL.with_borrow_mut(|device| device.timeouts += 1); }")
        harness = (ROOT / "tests/fixtures/ahci_completion_timeout_model.rs").read_text()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            driver = "\n".join(registers + [function(source, name) for name in names])
            # Exercise the driver's x86 cfg branches even on an ARM build host.
            driver = driver.replace('target_arch = "aarch64"', 'feature = "model_arm"')
            driver = driver.replace('target_arch = "x86_64"', 'not(feature = "model_arm")')
            (path / "driver.rs").write_text(driver)
            (path / "host.rs").write_text(harness)
            subprocess.run(
                ["rustc", "--edition=2021", "-A", "unused_variables", "--test",
                 str(path / "host.rs"), "-o", str(path / "host")],
                check=True,
                timeout=30,
            )
            subprocess.run([str(path / "host"), "--test-threads=1"], check=True, timeout=10)

    def test_lost_notification_refuses_an_otherwise_passing_suite(self):
        passing = (FIXTURES / "all-pass.txt").read_text()
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.txt"
            serial.write_text(passing)
            self.assertTrue(SCORER.verdict(MANIFEST, serial, [serial])[0])
            serial.write_text(passing + LOST_NOTIFICATION)
            ok, detail = SCORER.verdict(MANIFEST, serial, [serial])
            self.assertFalse(ok, detail)
            self.assertIn("AHCI_LOST_NOTIFICATION", detail)

    def test_lost_notification_refuses_a_testing_kernel_capture(self):
        import json

        with patch.object(sys, "path", [str(ROOT / "scripts/metal"), *sys.path]):
            import metal_score

        def read(name):
            return json.loads((ROOT / name.removeprefix("definition/")).read_text())

        with tempfile.TemporaryDirectory() as directory, patch.object(
            metal_score, "tests_verdict", return_value=(True, "complete test report")
        ):
            capture = Path(directory)
            (capture / "events.jsonl").write_text("")
            serial = capture / "serial.txt"
            start = "[init] boot\n"
            done = "USERSPACE TEST REPORT DONE\n"
            serial.write_text(start + done)
            verdict = metal_score.score_tests(capture, {}, read)
            self.assertTrue(verdict["testReport"]["passed"])
            serial.write_text(start + LOST_NOTIFICATION + done)
            verdict = metal_score.score_tests(capture, {}, read)
            self.assertEqual(
                verdict["testReport"], {"passed": False, "reason": "fatal kernel output"}
            )


if __name__ == "__main__":
    unittest.main()
