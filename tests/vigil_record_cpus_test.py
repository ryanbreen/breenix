#!/usr/bin/env python3
"""scripts/vigil-record.sh passes a boot's processor count to Vigil only when it was set."""

import os
import platform
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "vigil-record.sh"


@unittest.skipUnless(platform.system() == "Darwin", "vigil-record.sh does nothing off macOS")
class VigilRecordCpusTest(unittest.TestCase):
    def start(self, cpus):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "args"
            cli = Path(tmp) / "vigil-cli"
            cli.write_text(
                "#!/bin/sh\n"
                'if [ "$#" -eq 1 ]; then echo "usage: vigil-cli breenix start"; exit 64; fi\n'
                f'printf "%s\\n" "$@" > "{log}"\n'
                "echo 20261015T000000Z-qemu\n"
            )
            cli.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if k not in ("VIGIL_BOOT_ID", "BREENIX_VIGIL_CPUS")}
            env["VIGIL_CLI"] = str(cli)
            if cpus is not None:
                env["BREENIX_VIGIL_CPUS"] = cpus
            subprocess.run([str(SCRIPT), "start", "qemu", "suite", "processes", str(Path(tmp) / "serial.txt"), "", "", "", "0" * 40],
                           env=env, check=True, capture_output=True, text=True)
            return log.read_text().split("\n")

    def test_a_set_count_reaches_vigil(self):
        args = self.start("1")
        self.assertEqual(args[args.index("--cpus") + 1], "1")

    def test_no_count_sends_none(self):
        self.assertNotIn("--cpus", self.start(None))
        self.assertNotIn("--cpus", self.start(""))


class BootInteractiveCpusTest(unittest.TestCase):
    def test_only_a_non_default_count_is_recorded(self):
        text = (ROOT / "scripts" / "boot-interactive.sh").read_text()
        self.assertIn('[ "$CPUS" = 4 ] || VIGIL_CPUS=$CPUS', text)
        self.assertIn('BREENIX_VIGIL_CPUS="$VIGIL_CPUS" "$ROOT/scripts/vigil-record.sh" start qemu', text)


if __name__ == "__main__":
    unittest.main()
