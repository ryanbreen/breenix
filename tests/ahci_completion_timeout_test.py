#!/usr/bin/env python3
"""Drive the slot-0 driver with AHCI 1.3.1 and Intel LAPIC register doubles."""

from pathlib import Path
import re
import subprocess
import tempfile
import unittest

from ahci_poller_race_test import function
from suite_verdict_test import FIXTURES, MANIFEST, SCORER

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "kernel/src/drivers/ahci/mod.rs"


class AhciCompletion(unittest.TestCase):
    def test_interrupt_and_timeout(self):
        source = SOURCE.read_text()
        registers = re.findall(
            r"(?m)^const (?:HBA_IS|PORT_(?:IS|CI|TFD|CMD)|PORT_(?:IRQ|TFD|CMD)_[A-Z0-9_]+|"
            r"AHCI_(?:CI_COMPLETION_LOOP_LIMIT|TRACKED_SLOT_MASK|TIMEOUT_SECS)): [^=]+=[^;]+;",
            source,
        )
        names = ["ack_port_interrupt", "detect_completed_slots", "handle_interrupt", "wait_cmd_slot0"]
        # The baseline has an inline timeout error. The new helper, when present,
        # must run its production body against the very same device and tests.
        if "fn resolve_wait_timeout(" in source:
            names.append("resolve_wait_timeout")
        if "struct TimeoutRegisters " in source:
            registers.append(re.search(r"struct TimeoutRegisters \{[^}]+\}", source)[0])
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
                ["rustc", "--edition=2021", "--test", str(path / "host.rs"), "-o", str(path / "host")],
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
            serial.write_text(
                passing + "AHCI_LOST_NOTIFICATION port=0 token=51849 CI=0x0 IS=0x21 TFD=0x50 HBA_IS=0x1\n"
            )
            ok, detail = SCORER.verdict(MANIFEST, serial, [serial])
            self.assertFalse(ok, detail)
            self.assertIn("AHCI_LOST_NOTIFICATION", detail)


if __name__ == "__main__":
    unittest.main()
