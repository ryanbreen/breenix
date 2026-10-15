#!/usr/bin/env python3
"""Exercise suite acceptance and its caller boundaries using serial fixtures."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests/fixtures/suite-verdict"
MANIFEST = json.loads((FIXTURES / "manifest.json").read_text())


def load(name, script):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SCORER = load("suite_verdict", "suite-verdict.py")
WAIT = load("wait_boot_done", "wait-boot-done.py")
SEQUENCE = load("x86_suite_boot", "x86-suite-boot.py")

EXPECTATIONS = {
    "all-pass": (True, "SUITE fixture DONE passed=3 failed=0 skipped=0 total=3"),
    "one-skip": (False, "suite acceptance requires skipped=0 and passed=total=3"),
    "all-skip": (False, "suite acceptance requires skipped=0 and passed=total=3"),
    "missing-case": (False, "no CASE line for second/three"),
    "out-of-order": (False, "CASE line 1 is first/two; the manifest's case 1 is first/one"),
    "wrong-total": (False, "disagrees with its CASE lines (3 passed, 0 failed, 0 skipped of 3)"),
    "failed-case": (False, "CASE first/two FAIL ms=1 msg=case failed"),
}


class SuiteVerdictTests(unittest.TestCase):
    def test_fixture_verdicts_and_cli_status(self):
        for name, (expected, reason) in EXPECTATIONS.items():
            with self.subTest(fixture=name):
                serial = FIXTURES / (name + ".txt")
                ok, detail = SCORER.verdict(MANIFEST, serial, [serial])
                self.assertEqual(ok, expected, detail)
                self.assertIn(reason, detail)
                result = subprocess.run(
                    [sys.executable, str(ROOT / "scripts/suite-verdict.py"),
                     str(FIXTURES / "manifest.json"), str(serial)],
                    capture_output=True, text=True, check=False,
                )
                self.assertEqual(result.returncode, 0 if expected else 1, result.stdout + result.stderr)
                self.assertIn(("PASS: " if expected else "FAIL: ") + detail, result.stdout)

    def test_native_vm_waiter_propagates_verdict(self):
        for name, (expected, reason) in EXPECTATIONS.items():
            with self.subTest(fixture=name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                manifests = root / "docs/suites"
                manifests.mkdir(parents=True)
                (manifests / "fixture.json").write_text(json.dumps(MANIFEST))
                (root / "scripts").symlink_to(ROOT / "scripts", target_is_directory=True)
                serial = root / "serial.txt"
                serial.write_text(f"[clock] UTC epoch={int(time.time())} source=PL031\n" +
                                  (FIXTURES / (name + ".txt")).read_text())
                output = io.StringIO()
                with patch.object(WAIT, "ROOT", root), \
                     patch.object(sys, "argv", ["wait", str(serial), "suite", "--suite", "fixture"]), \
                     contextlib.redirect_stdout(output):
                    status = WAIT.main()
                self.assertEqual(status, 0 if expected else 1, output.getvalue())
                # Failed CASEs are rejected at the completion check before scoring.
                if name != "failed-case":
                    self.assertIn(reason, output.getvalue())

    def test_x86_boundary_persists_verdict(self):
        for name, (expected, reason) in EXPECTATIONS.items():
            with self.subTest(fixture=name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                manifests = root / "docs/suites"
                manifests.mkdir(parents=True)
                (manifests / "fixture.json").write_text(json.dumps(MANIFEST))
                with contextlib.redirect_stdout(io.StringIO()):
                    result = SEQUENCE.record(root, root, "fixture",
                                             (FIXTURES / (name + ".txt")).read_bytes(),
                                             b"", None, 0, 1)
                saved = json.loads((root / "suite-fixture/result.json").read_text())
                self.assertEqual(saved, result)
                self.assertEqual(saved["verdict"], "PASS" if expected else "FAIL")
                self.assertIn(reason, saved["reason"])


if __name__ == "__main__":
    unittest.main()
