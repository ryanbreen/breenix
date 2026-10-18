#!/usr/bin/env python3
"""scripts/vigil-record.sh passes a boot's processor count to Vigil only when it was set."""

import os
import platform
import subprocess
import tempfile
import unittest
from pathlib import Path
from vigil_record_fixture import isolated_commands

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
                'if [ "$#" -eq 1 ]; then echo "usage: vigil-cli breenix start [--cpus N]"; exit 64; fi\n'
                f'printf "%s\\n" "$@" > "{log}"\n'
                "echo 20261015T000000Z-qemu\n"
            )
            cli.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if k not in ("VIGIL_BOOT_ID", "BREENIX_VIGIL_CPUS")}
            env["VIGIL_CLI"] = str(cli)
            if cpus is not None:
                env["BREENIX_VIGIL_CPUS"] = cpus
            subprocess.run([str(SCRIPT), "start", "qemu", "suite", "processes", str(Path(tmp) / "serial.txt"), "", "", "", "0" * 40],
                           env=isolated_commands(env, tmp), check=True, capture_output=True, text=True)
            return log.read_text().split("\n")

    def test_a_set_count_reaches_vigil(self):
        args = self.start("1")
        self.assertEqual(args[args.index("--cpus") + 1], "1")

    def test_no_count_sends_none(self):
        self.assertNotIn("--cpus", self.start(None))
        self.assertNotIn("--cpus", self.start(""))


@unittest.skipUnless(platform.system() == "Darwin", "vigil-record.sh does nothing off macOS")
class VigilRecordOlderCliTest(unittest.TestCase):
    @staticmethod
    def cli(path, log, knows_cpus):
        usage = "usage: vigil-cli breenix start --platform P" + (" [--cpus N]" if knows_cpus else "")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            "#!/bin/sh\n"
            f'if [ "$#" -eq 1 ]; then echo "{usage}"; exit 64; fi\n'
            + ("" if knows_cpus else 'for a in "$@"; do [ "$a" = --cpus ] && exit 64; done\n')
            + f'printf "%s\\n" "$@" > "{log}"\n'
            "echo 20261015T000000Z-qemu\n"
        )
        path.chmod(0o755)

    def run_start(self, home, cpus):
        env = {k: v for k, v in os.environ.items() if k not in ("VIGIL_BOOT_ID", "VIGIL_CLI", "BREENIX_VIGIL_CPUS")}
        env["HOME"] = str(home)
        env["BREENIX_VIGIL_CPUS"] = cpus
        return subprocess.run([str(SCRIPT), "start", "qemu", "suite", "processes", str(home / "serial.txt"), "", "", "", "0" * 40],
                              env=isolated_commands(env, home), capture_output=True, text=True)

    def test_an_older_cli_is_passed_over_for_one_that_records_the_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            self.cli(home / ".local" / "bin" / "vigil-cli", home / "old-args", knows_cpus=False)
            self.cli(home / "fun" / "code" / "vigil" / ".build" / "release" / "vigil-cli", home / "new-args", knows_cpus=True)
            result = self.run_start(home, "1")
            self.assertEqual(result.returncode, 0)
            self.assertFalse((home / "old-args").exists())
            args = (home / "new-args").read_text().split("\n")
            self.assertEqual(args[args.index("--cpus") + 1], "1")

    def test_with_only_an_older_cli_the_boot_goes_on_unrecorded(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            self.cli(home / ".local" / "bin" / "vigil-cli", home / "old-args", knows_cpus=False)
            result = self.run_start(home, "1")
            self.assertEqual((result.returncode, result.stdout), (0, ""))
            self.assertFalse((home / "old-args").exists(), "never registered without its processor count")

    def test_a_refused_registration_never_fails_the_boot(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            cli = home / ".local" / "bin" / "vigil-cli"
            cli.parent.mkdir(parents=True)
            cli.write_text('#!/bin/sh\nif [ "$#" -eq 1 ]; then echo "usage: vigil-cli breenix start [--cpus N]"; fi\nexit 64\n')
            cli.chmod(0o755)
            result = self.run_start(home, "2")
            self.assertEqual((result.returncode, result.stdout), (0, ""))


class BootInteractiveCpusTest(unittest.TestCase):
    def test_only_a_non_default_count_is_recorded(self):
        text = (ROOT / "scripts" / "boot-interactive.sh").read_text()
        self.assertIn('[ "$CPUS" = 4 ] || VIGIL_CPUS=$CPUS', text)
        self.assertIn('BREENIX_VIGIL_CPUS="$VIGIL_CPUS" "$ROOT/scripts/vigil-record.sh" start qemu', text)


if __name__ == "__main__":
    unittest.main()
