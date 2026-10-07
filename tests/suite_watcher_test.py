#!/usr/bin/env python3
"""Replay a recorded directories serial through the production watcher, without a VM.

Usage: python3 tests/suite_watcher_test.py /path/to/directories-serial.txt
No recorded logs are stored in the repository.
"""

from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
RECORDING = Path(sys.argv.pop(1)).read_bytes()
# Replay through the actual completion line, excluding later idle output.
end = next(line for line in RECORDING.splitlines(keepends=True)
           if line.startswith(b"SUITE directories DONE "))
RECORDING = RECORDING[:RECORDING.index(end) + len(end)]

# The stand-in writes the recording in two pieces, like QEMU's logfile, then
# stays alive until the watcher sends TERM. Its final write proves cleanup ran.
REPLAY = """
from pathlib import Path
import signal, sys, time
source, serial, stopped = map(Path, sys.argv[1:4])
delay = float(sys.argv[4])
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
while True:
    time.sleep(1)
"""


class SuiteWatcherTest(unittest.TestCase):
    def replay(self, data, idle=2, mode="suite", suite="directories", split=True):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source, serial, stopped = (directory / name for name in ("source", "serial", "stopped"))
            source.write_bytes(data)
            started = time.monotonic()
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/watch-qemu.py"),
                 "--serial", str(serial), "--mode", mode, "--suite", suite,
                 "--idle-exit", str(idle), "--", sys.executable, "-c", REPLAY,
                 str(source), str(serial), str(stopped), "0.4" if split else "0"],
                capture_output=True, text=True, timeout=10,
            )
            elapsed = time.monotonic() - started
            self.assertTrue(stopped.exists(), result.stdout + result.stderr)
            self.assertEqual(serial.read_bytes(), data)
            return result, elapsed

    def test_done_stops_before_idle_and_keeps_complete_serial(self):
        result, elapsed = self.replay(RECORDING, idle=5)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS: SUITE directories DONE", result.stdout)
        self.assertIn("completed; stopping", result.stdout)
        self.assertLess(elapsed, 3)
        self.assertGreaterEqual(elapsed, 0.4)

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
        self.assertLess(elapsed, 3)

    def test_fatal_overrides_done(self):
        result, elapsed = self.replay(RECORDING + b"KERNEL PANIC: replayed fault\n", idle=5, split=False)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("fatal kernel output", result.stdout)
        self.assertLess(elapsed, 3)

    def test_done_with_missing_cases_is_failure(self):
        data = b"SUITE directories START cases=117\nSUITE directories DONE passed=117 failed=0 skipped=0 total=117\n"
        result, _ = self.replay(data)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("no CASE line", result.stdout)

    def test_other_suite_done_does_not_stop_selected_suite(self):
        result, _ = self.replay(RECORDING, idle=1, suite="processes")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("No serial output", result.stdout)

    def test_probe_retains_idle_exit(self):
        result, elapsed = self.replay(b"PROBE DONE passed=16 failed=0\n", idle=1, mode="probe")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("No serial output", result.stdout)
        self.assertGreaterEqual(elapsed, 1)


if __name__ == "__main__":
    unittest.main()
