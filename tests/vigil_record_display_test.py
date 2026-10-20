"""The recorder stores window selection at launch and retains this boot's attestation."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
RECORDER = ROOT / "scripts/vigil-record.sh"
BOOT_ID = "20261017T120000Z-qemu"


class VigilRecordDisplayTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.folder = self.root / "Library/Application Support/Vigil/breenix/boots" / BOOT_ID
        self.serial = self.root / "run/serial.txt"
        self.serial.parent.mkdir()
        self.serial.write_text("SUITE gpu DONE passed=53 failed=117 skipped=0 total=170\n")
        self.renderer = self.serial.parent / "host-renderer.json"
        self.renderer.write_bytes(b'{"format":1,"records":[{"event":"first-submit-3d"}]}\n')
        cli = self.root / "vigil-cli"
        cli.write_text("#!" + sys.executable + "\n" + '''
import json, os, pathlib, sys
args = sys.argv[1:]
if args == ["breenix"]:
    print("usage: vigil-cli breenix start [--cpus N]")
    sys.exit(64)
opts = dict(zip(args[2::2], args[3::2]))
folder = pathlib.Path(os.environ["VIGIL_TEST_HOME"]) / "Library/Application Support/Vigil/breenix/boots" / "20261017T120000Z-qemu"
if args[1] == "start":
    folder.mkdir(parents=True)
    (folder / "run.json").write_text(json.dumps({"id": "20261017T120000Z-qemu", "checkout": opts["--checkout"], "display": False, "mode": "suite", "suite": opts["--suite"]}))
    (folder / "commit").write_text(opts["--commit"])
    (folder / "serial.txt").symlink_to(opts["--serial"])
    print("20261017T120000Z-qemu")
else:
    (folder / "exit-status").write_text(opts["--exit-status"])
''')
        cli.chmod(0o755)
        self.env = {k: v for k, v in os.environ.items() if k not in
                    ("VIGIL_BOOT_ID", "BREENIX_VIGIL_CPUS", "BREENIX_VIGIL_DISPLAY", "BREENIX_HOST_RENDERER_JSON", "BREENIX_GPU_TIME_JSONL")}
        # Provenance must not stash the active checkout: concurrent git writers
        # can make stash create fail, which makes the best-effort recorder return
        # success without an ID before it ever calls the fixture CLI.
        tools = self.root / "fixture-tools"
        tools.mkdir()
        git = tools / "git"
        git.write_text('#!/bin/sh\ncase "$3" in\n'
                       '  status|merge-base) exit 0 ;;\n'
                       '  *) echo "unexpected fixture git command" >&2; exit 1 ;;\n'
                       'esac\n')
        git.chmod(0o755)
        self.env.update(VIGIL_CLI=str(cli), VIGIL_TEST_HOME=str(self.root),
                        PATH=str(tools) + os.pathsep + self.env.get("PATH", ""))

    def start(self, display):
        if display is not None:
            self.env["BREENIX_VIGIL_DISPLAY"] = display
        result = subprocess.run([str(RECORDER), "start", "qemu", "suite", "gpu", str(self.serial), "", "", "", "a" * 40],
                                env=self.env, capture_output=True, text=True, check=True)
        self.assertEqual(result.stdout.strip(), BOOT_ID)
        self.assertEqual(result.stderr, "")
        return json.loads((self.folder / "run.json").read_text())

    def test_visible_boot_is_recorded_before_the_vm_starts(self):
        run = self.start("true")
        self.assertIs(run["display"], True)
        self.assertEqual(run["suite"], "gpu")
        self.assertEqual((self.folder / "commit").read_text(), "a" * 40)
        self.assertFalse((self.folder / "exit-status").exists())

    def test_no_window_and_unknown_are_never_recorded_as_visible(self):
        run = self.start("false")
        self.assertIs(run["display"], False)

    def test_unknown_display_keeps_the_registration_unchanged(self):
        run = self.start(None)
        self.assertIs(run["display"], False)

    def test_finish_archives_the_exact_renderer_without_relabelling_display(self):
        self.start("true")
        original = (self.folder / "run.json").read_bytes()
        self.env.update(BREENIX_VIGIL_DISPLAY="false", BREENIX_HOST_RENDERER_JSON=str(self.renderer))
        result = subprocess.run([str(RECORDER), "finish", BOOT_ID, "1"], env=self.env, capture_output=True, text=True, check=True)
        self.assertEqual(result.stderr, "")
        self.assertEqual((self.folder / "run.json").read_bytes(), original)
        self.assertEqual((self.folder / "host-renderer.json").read_bytes(), self.renderer.read_bytes())
        self.assertEqual((self.folder / "exit-status").read_text(), "1")

    def test_finish_retains_gpu_time_even_for_a_refused_run(self):
        self.start("true")
        record = Path(str(self.serial) + ".gpu-time.jsonl")
        record.write_bytes(b'{"event":"sample","gpu_ns":0}\n')
        self.env["BREENIX_GPU_TIME_JSONL"] = str(record)
        result = subprocess.run([str(RECORDER), "finish", BOOT_ID, "1"],
                                env=self.env, capture_output=True, text=True, check=True)
        self.assertEqual(result.stderr, "")
        self.assertEqual((self.folder / "gpu-time.jsonl").read_bytes(), record.read_bytes())
        self.assertEqual((self.folder / "exit-status").read_text(), "1")


if __name__ == "__main__":
    unittest.main()
