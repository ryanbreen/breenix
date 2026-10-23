"""Execute the gate's actual PCI-count pipeline against serial containing a NUL."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class SerialNulTests(unittest.TestCase):
    def test_gate_counts_every_pci_line_after_a_nul(self):
        gate = (ROOT / "docker/qemu/run-x86-gate.sh").read_text()
        command = re.search(r'observed=\$\((grep[^\n]*\n[^\n]*\| wc -l)\)', gate)
        self.assertIsNotNone(command, "cannot find the gate's PCI-count pipeline")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "serial_kernel.log"
            path.write_bytes(b"name=tablet\x00\n"
                             b"PCI_FN 00:01.0 1af4:1001 class=01/00\n"
                             b"PCI_FN 00:02.0 1af4:1001 class=01/00\n")
            result = subprocess.run(["bash", "-c", command[1]], env={**os.environ, "OUTDIR": directory, "pci_id": "1af4:1001"}, capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), b"2", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
