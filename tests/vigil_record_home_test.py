#!/usr/bin/env python3
"""scripts/vigil-record.sh record finds an already published boot where vigil-cli keeps it, not under $HOME."""

import os
import platform
import subprocess
import tempfile
import unittest
import uuid
from pathlib import Path
from vigil_record_fixture import isolated_commands

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "vigil-record.sh"
BOOTS = "Library/Application Support/Vigil/breenix/boots"


@unittest.skipUnless(platform.system() == "Darwin", "vigil-record.sh does nothing off macOS")
class VigilRecordHomeTest(unittest.TestCase):
    def record(self, tmp, boot_id, published_under, vigil_home):
        """Runs record with a published run.json under published_under; returns vigil-cli's subcommand."""
        log = tmp / "args"
        cli = tmp / "vigil-cli"
        cli.write_text(
            "#!/bin/sh\n"
            'if [ "$#" -eq 1 ]; then echo "usage: vigil-cli breenix start"; exit 64; fi\n'
            f'printf "%s\\n" "$@" > "{log}"\n'
        )
        cli.chmod(0o755)
        run = published_under / BOOTS / boot_id / "run.json"
        run.parent.mkdir(parents=True)
        run.write_text("{}")
        home = tmp / "home"
        home.mkdir(exist_ok=True)
        env = {k: v for k, v in os.environ.items() if k not in ("VIGIL_BOOT_ID", "VIGIL_TEST_HOME", "BREENIX_VIGIL_CPUS")}
        env.update(VIGIL_CLI=str(cli), HOME=str(home))
        if vigil_home is not None:
            env["VIGIL_TEST_HOME"] = str(vigil_home)
        subprocess.run([str(SCRIPT), "record", "qemu", "tests", "", str(tmp / "serial.txt"), "", "", boot_id,
                        "0" * 40, "1", "2", "0"], env=isolated_commands(env, tmp), check=True, capture_output=True, text=True)
        return log.read_text().split("\n")[1]

    def test_a_boot_published_under_vigils_home_is_finished(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            self.assertEqual(self.record(tmp, "boot-1", tmp / "vh", tmp / "vh"), "finish")

    def test_a_run_json_under_home_alone_is_not_vigils(self):
        # Without VIGIL_TEST_HOME, vigil-cli uses the account's home, which has no boot with this id.
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            self.assertEqual(self.record(tmp, f"boot-{uuid.uuid4().hex}", tmp / "home", None), "record")


if __name__ == "__main__":
    unittest.main()
