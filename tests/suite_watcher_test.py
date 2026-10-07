#!/usr/bin/env python3
"""Exercise watcher lifecycle and scoring with synthetic serial, without a VM.

Run directly or with unittest discovery; guest behavior is checked by real boots.
"""

import json
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = json.loads((ROOT / "docs/suites/directories.json").read_text())
CASES = [f"{category['id']}/{case['id']}" for category in MANIFEST["categories"]
         for case in category["cases"]]
RECORDING = (f"SUITE directories START cases={len(CASES)}\n" +
             "".join(f"SUITE directories CASE {case} PASS ms=0\n" for case in CASES) +
             f"SUITE directories DONE passed={len(CASES)} failed=0 skipped=0 total={len(CASES)}\n").encode()

# The stand-in writes serial in pieces and can append a separate late record.
# Its TERM handler writes a sentinel so each test checks that it was reaped.

REPLAY = """
from pathlib import Path
import signal, sys, time
source, serial, stopped = map(Path, sys.argv[1:4])
delay = float(sys.argv[4])
late = Path(sys.argv[5]).read_bytes()
def finish(signum, frame):
    stopped.write_text('stopped')
    raise SystemExit(0)
signal.signal(signal.SIGTERM, finish)
data = source.read_bytes()
with serial.open('wb') as output:
    cut = len(data) - 3 if delay else len(data)
    output.write(data[:cut]); output.flush()
    time.sleep(delay)
    output.write(data[cut:]); output.flush()
    if late:
        time.sleep(0.6)
        output.write(late); output.flush()
while True:
    time.sleep(1)
"""


class SuiteWatcherTest(unittest.TestCase):
    def replay(self, data, idle=2, mode="suite", suite="directories", split=True, late=b"", deadline=30):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source, serial, stopped, late_source = (directory / name for name in ("source", "serial", "stopped", "late"))
            source.write_bytes(data)
            late_source.write_bytes(late)
            started = time.monotonic()
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/watch-qemu.py"),
                 "--serial", str(serial), "--mode", mode, "--suite", suite,
                 "--idle-exit", str(idle), "--gate-timeout", str(deadline),
                 "--suite-hold", "0", "--", sys.executable, "-c", REPLAY,
                 str(source), str(serial), str(stopped), "0.4" if split else "0", str(late_source)],
                capture_output=True, text=True, timeout=10,
            )
            elapsed = time.monotonic() - started
            self.assertTrue(stopped.exists(), result.stdout + result.stderr)
            self.assertEqual(serial.read_bytes(), data + late)
            return result, elapsed

    def test_done_stops_before_idle_and_keeps_complete_serial(self):
        result, elapsed = self.replay(RECORDING, idle=5)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS: SUITE directories DONE", result.stdout)
        self.assertIn("completed; stopping", result.stdout)
        self.assertLess(elapsed, 4)
        self.assertGreaterEqual(elapsed, 2.4)

    def test_missing_done_idles_and_fails(self):
        data = b"\n".join(line for line in RECORDING.split(b"\n")
                          if not line.startswith(b"SUITE directories DONE "))
        result, elapsed = self.replay(data, idle=1)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("No serial output", result.stdout)
        self.assertIn("FAIL:", result.stdout)
        self.assertNotIn("PASS:", result.stdout)
        self.assertGreaterEqual(elapsed, 1)

    def test_announcement_does_not_finish_a_suite(self):
        data = b"kernel::test_exec: Userspace will emit SUITE directories DONE passed=117 failed=0 skipped=0 total=117\n"
        result, _ = self.replay(data, idle=1)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("No serial output", result.stdout)

    def test_unterminated_done_idles_and_fails(self):
        result, _ = self.replay(RECORDING.rstrip(b"\r\n"), idle=1)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("No serial output", result.stdout)
        self.assertIn("no complete SUITE directories DONE", result.stdout)

    def test_failing_done_stops_and_fails(self):
        data = RECORDING.replace(b"DONE passed=117 failed=0", b"DONE passed=116 failed=1")
        result, elapsed = self.replay(data, idle=5)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("completed; stopping", result.stdout)
        self.assertNotIn("PASS:", result.stdout)
        self.assertLess(elapsed, 4)

    def test_fatal_overrides_done(self):
        result, elapsed = self.replay(RECORDING + b"KERNEL PANIC: replayed fault\n", idle=5, split=False)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("fatal kernel output", result.stdout)
        self.assertLess(elapsed, 4)

    def test_late_fatal_overrides_done(self):
        result, elapsed = self.replay(RECORDING, idle=5, split=False,
                                      late=b"KERNEL PANIC: late fault\n")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("fatal kernel output", result.stdout)
        self.assertGreaterEqual(elapsed, 0.6)
        self.assertLess(elapsed, 2)

    def test_duplicate_done_in_later_write_fails(self):
        done = RECORDING.splitlines(keepends=True)[-1]
        result, _ = self.replay(RECORDING, late=done)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("duplicate completion records", result.stdout)

    def test_malformed_done_fails(self):
        data = RECORDING.replace(b"failed=0 skipped=0", b"failed=no skipped=0")
        result, _ = self.replay(data)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("malformed completion record", result.stdout)

    def test_lone_cr_is_not_a_complete_done(self):
        result, _ = self.replay(RECORDING.rstrip(b"\n") + b"\r", idle=1)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("no complete SUITE directories DONE", result.stdout)

    def test_crlf_done_passes(self):
        result, _ = self.replay(RECORDING.replace(b"\n", b"\r\n"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_deadline_with_idle_disabled_fails(self):
        result, elapsed = self.replay(b"SUITE directories START cases=117\n",
                                      idle=0, deadline=1)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("before deadline", result.stdout)
        self.assertGreaterEqual(elapsed, 1)
        self.assertLess(elapsed, 2)

    def test_shell_fatal_retains_idle_exit(self):
        result, elapsed = self.replay(b"!!! SOFT LOCKUP DETECTED !!! synthetic fault\n", idle=1, mode="shell")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("No serial output", result.stdout)
        self.assertNotIn("fatal kernel output", result.stdout)
        self.assertGreaterEqual(elapsed, 1)

    def test_done_with_missing_cases_is_failure(self):
        data = b"SUITE directories START cases=117\nSUITE directories DONE passed=117 failed=0 skipped=0 total=117\n"
        result, _ = self.replay(data)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("no CASE line", result.stdout)

    def test_other_suite_done_does_not_stop_selected_suite(self):
        result, _ = self.replay(RECORDING, idle=1, suite="processes")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("No serial output", result.stdout)

    def test_term_reaps_child(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source, serial, stopped, late = (directory / name for name in ("source", "serial", "stopped", "late"))
            source.write_bytes(b"still running\n")
            late.write_bytes(b"")
            process = subprocess.Popen(
                [sys.executable, str(ROOT / "scripts/watch-qemu.py"),
                 "--serial", str(serial), "--mode", "shell", "--idle-exit", "0",
                 "--", sys.executable, "-c", REPLAY, str(source), str(serial),
                 str(stopped), "0", str(late)], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                while not serial.exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                self.assertTrue(serial.exists())
                process.send_signal(signal.SIGTERM)
                stdout, stderr = process.communicate(timeout=5)
                self.assertEqual(process.returncode, 143, (stdout, stderr))
                self.assertTrue(stopped.exists())
            finally:
                if process.poll() is None:
                    process.kill()
                process.wait()

    def test_probe_retains_idle_exit(self):
        result, elapsed = self.replay(b"PROBE DONE passed=16 failed=0\n", idle=1, mode="probe")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("No serial output", result.stdout)
        self.assertGreaterEqual(elapsed, 1)


if __name__ == "__main__":
    unittest.main()
