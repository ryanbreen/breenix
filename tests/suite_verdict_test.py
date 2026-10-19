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
    "memory-leaked": (False, "memory not reclaimed: 100 pages short after the reap, allowance 64"),
}


class SuiteVerdictTests(unittest.TestCase):
    def test_category_selection_requires_its_complete_case_list(self):
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.txt"
            complete = ("SUITE fixture START cases=2 only=first\n"
                        "SUITE fixture CASE first/one PASS ms=1\n"
                        "SUITE fixture CASE first/two PASS ms=1\n"
                        "SUITE fixture DONE passed=2 failed=0 skipped=0 total=2 only=first\n")
            serial.write_text(complete)
            self.assertTrue(SCORER.verdict(MANIFEST, serial, [serial], only="first")[0])
            self.assertFalse(SCORER.verdict(MANIFEST, serial, [serial])[0])
            for broken in (complete.replace("SUITE fixture CASE first/two PASS ms=1\n", ""),
                           complete.replace("first/two PASS", "second/three PASS")):
                serial.write_text(broken)
                self.assertFalse(SCORER.verdict(MANIFEST, serial, [serial], only="first")[0])

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

    def test_memory_record_is_required_and_must_agree_with_itself(self):
        original = (FIXTURES / "all-pass.txt").read_text()
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.txt"
            for broken, reason in (
                ("".join(line for line in original.splitlines(keepends=True) if " MEMORY " not in line),
                 "found 0, 0 and 0"),
                (original.replace("free_after=990 allowance=64", "free_after=900 allowance=64"),
                 "does not repeat the free counts"),
                (original.replace("free_after=990 allowance=64", "free_after=990 allowance=5"),
                 "contradicts its own figures"),
            ):
                with self.subTest(reason=reason):
                    serial.write_text(broken)
                    ok, detail = SCORER.verdict(MANIFEST, serial, [serial])
                    self.assertFalse(ok)
                    self.assertIn(reason, detail)

    def test_hostkey_live_record(self):
        with tempfile.TemporaryDirectory() as temporary:
            serial = Path(temporary) / "serial"
            original = (FIXTURES / "all-pass.txt").read_text()
            request = "SUITE fixture HOSTKEY first/one seq=0 keys=alt-f2,alt-left\n"
            lines = original.splitlines(keepends=True)
            serial.write_text(lines[0] + request + ''.join(lines[1:]))
            ok, detail = SCORER.verdict(MANIFEST, serial, [serial])
            self.assertTrue(ok, detail)
            serial.write_text(original + request)
            ok, detail = SCORER.verdict(MANIFEST, serial, [serial])
            self.assertFalse(ok)
            self.assertIn("outside START..DONE", detail)

    def test_one_case_requires_explicit_selection_and_exact_records(self):
        with tempfile.TemporaryDirectory() as temporary:
            serial = Path(temporary) / "serial"
            partial = ("SUITE fixture START cases=1 only=first/two\n"
                       "SUITE fixture CASE first/two PASS ms=1\n"
                       "SUITE fixture DONE passed=1 failed=0 skipped=0 total=1 only=first/two\n")
            serial.write_text(partial)
            ok, reason = SCORER.verdict(MANIFEST, serial, [serial])
            self.assertFalse(ok)
            self.assertIn("not full-suite evidence", reason)
            self.assertFalse(WAIT.completion(partial, "suite", "fixture")[0])
            (Path(temporary) / "docs/suites").mkdir(parents=True)
            (Path(temporary) / "docs/suites/fixture.json").write_text(json.dumps(MANIFEST))
            with contextlib.redirect_stdout(io.StringIO()):
                result = SEQUENCE.record(Path(temporary), Path(temporary), "fixture", partial.encode(), b"", None, 0, 1)
            self.assertEqual(result["verdict"], "FAIL")
            self.assertIn("not full-suite evidence", result["reason"])
            self.assertTrue(WAIT.completion(partial, "suite", "fixture", "first/two")[0])
            self.assertTrue(SCORER.verdict(MANIFEST, serial, [serial], only="first/two")[0])
            self.assertFalse(SCORER.verdict(MANIFEST, serial, [serial], only="first/one")[0])
            # Even a manifest consisting of exactly one case cannot accept a watched run.
            one = dict(MANIFEST, categories=[dict(id="first", cases=[dict(id="two")])])
            self.assertFalse(SCORER.verdict(one, serial, [serial])[0])
            for extra in ("SUITE fixture CASE first/one PASS ms=1\n",
                          "SUITE fixture CASE first/two PASS ms=1\n"):
                serial.write_text(partial.replace("SUITE fixture DONE", extra + "SUITE fixture DONE"))
                self.assertFalse(SCORER.verdict(MANIFEST, serial, [serial], only="first/two")[0])
            serial.write_text(partial.replace("PASS ms=1", "SKIP msg=unavailable").replace("passed=1", "passed=0").replace("skipped=0", "skipped=1"))
            self.assertFalse(SCORER.verdict(MANIFEST, serial, [serial], only="first/two")[0])
            serial.write_text(partial + (FIXTURES / "all-pass.txt").read_text())
            self.assertFalse(SCORER.verdict(MANIFEST, serial, [serial])[0])
            unknown = "SUITE fixture DONE passed=0 failed=1 skipped=0 total=1 only=first/missing\n"
            self.assertEqual(WAIT.completion(unknown, "suite", "fixture", "first/missing"), (False, unknown.strip()))
            self.assertIn("unknown case first/missing", SCORER.verdict(MANIFEST, serial, [serial], only="first/missing")[1])

    def test_only_word_outside_start_and_done_is_not_a_one_case_run(self):
        with tempfile.TemporaryDirectory() as temporary:
            serial = Path(temporary) / "serial"
            text = (FIXTURES / "failed-case.txt").read_text().replace("msg=case failed", "msg=case failed only=first/two")
            serial.write_text(text)
            ok, reason = SCORER.verdict(MANIFEST, serial, [serial])
            self.assertFalse(ok)
            self.assertNotIn("not full-suite evidence", reason)
            self.assertIn("only=first/two", reason)

    def test_suite_hold_is_read_only_for_one_case_waits(self):
        with tempfile.TemporaryDirectory() as temporary:
            serial = Path(temporary) / "serial.txt"
            serial.write_text(f"[clock] UTC epoch={int(time.time())} source=PL031\nPROBE DONE passed=16 failed=0\n")
            output = io.StringIO()
            with patch.dict("os.environ", {"BREENIX_SUITE_HOLD": "soon"}), \
                 patch.object(sys, "argv", ["wait", str(serial), "probe"]), \
                 contextlib.redirect_stdout(output):
                self.assertEqual(WAIT.main(), 0, output.getvalue())
            with patch.dict("os.environ", {"BREENIX_SUITE_HOLD": "soon"}), \
                 patch.object(sys, "argv", ["wait", str(serial), "suite", "--suite", "fixture", "--case", "first/two"]), \
                 contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                WAIT.main()

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
