#!/usr/bin/env python3
"""Exercise watcher lifecycle and scoring with synthetic serial, without a VM.

Run directly or with unittest discovery; guest behavior is checked by real boots.
"""

import json
import re
import runpy
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = json.loads((ROOT / "docs/suites/directories.json").read_text())
CASES = [f"{category['id']}/{case['id']}" for category in MANIFEST["categories"]
         for case in category["cases"]]
RECORDING = (f"SUITE directories START cases={len(CASES)}\n" +
             "".join(f"SUITE directories CASE {case} PASS ms=0\n" for case in CASES) +
             f"SUITE directories DONE passed={len(CASES)} failed=0 skipped=0 total={len(CASES)}\n").encode()

CANONICAL_COUNT = len(re.findall(r'^    "([^"]+)",',
    (ROOT / "kernel/src/boot/test_list.rs").read_text(), re.MULTILINE))
LOADER = f"[test] Loaded {CANONICAL_COUNT}/{CANONICAL_COUNT} test binaries (0 failed, 0 not found)\n".encode()

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
data = ('[clock] UTC epoch=%d source=PL031\\n' % (int(time.time()) + (int(sys.argv[6]) if len(sys.argv) > 6 else 0))).encode() + source.read_bytes()
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
    def test_case_progress_cannot_extend_suite_done_deadlines(self):
        for monitor in ('watch-qemu.py', 'wait-boot-done.py'):
            with self.subTest(monitor=monitor), tempfile.TemporaryDirectory() as tmp:
                serial = Path(tmp) / 'serial'
                serial.write_text('SUITE directories START cases=117\n')
                elapsed = [0.0]
                def tick(seconds):
                    elapsed[0] += seconds
                    with serial.open('a') as stream:
                        stream.write('SUITE directories CASE progress PASS ms=1\n')
                module = runpy.run_path(str(ROOT / 'scripts' / monitor))
                entry = module['watch'] if monitor == 'watch-qemu.py' else module['main']
                with patch.object(time, 'monotonic', side_effect=lambda: elapsed[0]), \
                     patch.object(time, 'sleep', side_effect=tick):
                    if monitor == 'watch-qemu.py':
                        self.assertEqual(entry(Mock(poll=lambda: None), serial, 'suite',
                                               'directories,smoke', 0, 1, 0, Mock()), 'deadline')
                    else:
                        with patch.object(sys, 'argv', [monitor, str(serial), 'suite',
                                                       '--suite', 'directories,smoke', '--timeout', '1']):
                            self.assertEqual(entry(), 124)
                self.assertLess(elapsed[0], 2)

    def test_partial_sequence_timeout_has_the_retryable_timeout_status(self):
        result, _ = self.replay(b'SUITE directories DONE passed=1 failed=0 skipped=0 total=1\n'
                                b'SUITE smoke START cases=5\n', suite='directories,smoke',
                                split=False, idle=0.4)
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)

    def test_quitting_or_signalling_qemu_cancels_a_partial_sequence(self):
        for action, status in (("raise SystemExit(0)", 130),
                               ("os.kill(os.getpid(), signal.SIGTERM)", 143)):
            with self.subTest(action=action), tempfile.TemporaryDirectory() as tmp:
                serial = Path(tmp) / 'serial'
                replay = ("import os, signal, sys\nfrom pathlib import Path\n"
                          "Path(sys.argv[1]).write_text('SUITE directories DONE passed=1 failed=0 skipped=0 total=1\\n')\n"
                          + action)
                result = subprocess.run([sys.executable, str(ROOT / 'scripts/watch-qemu.py'),
                                         '--serial', str(serial), '--mode', 'suite',
                                         '--suite', 'directories,smoke', '--', sys.executable,
                                         '-c', replay, str(serial)],
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
                self.assertIn('operator cancellation', result.stdout)

    def replay(self, data, idle=2, mode="suite", suite="directories", split=True, late=b"", deadline=30, clock_offset=0, only=""):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source, serial, stopped, late_source = (directory / name for name in ("source", "serial", "stopped", "late"))
            if mode == "tests" and b"[test] Loaded " not in data:
                data = LOADER + data
            source.write_bytes(data)
            late_source.write_bytes(late)
            started = time.monotonic()
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/watch-qemu.py"),
                 "--serial", str(serial), "--mode", mode, "--suite", suite, "--case", only,
                 "--idle-exit", str(idle), "--gate-timeout", str(deadline),
                 "--suite-hold", "0", "--", sys.executable, "-c", REPLAY,
                 str(source), str(serial), str(stopped), "0.4" if split else "0", str(late_source), str(clock_offset)],
                capture_output=True, text=True, timeout=10,
            )
            elapsed = time.monotonic() - started
            self.assertTrue(stopped.exists(), result.stdout + result.stderr)
            recorded = serial.read_bytes()
            clock_line, body = recorded.split(b"\n", 1)
            self.assertRegex(clock_line, rb"^\[clock\] UTC epoch=\d+ source=PL031$")
            self.assertEqual(body, data + late)
            return result, elapsed

    def test_sequence_keeps_collecting_after_failed_check_and_suite(self):
        data = (b'[BOOT_TESTS:FAIL:1]\n[TEST:example:FAIL]\n'
                b'SUITE directories START cases=1\n'
                b'SUITE directories DONE passed=0 failed=1 skipped=0 total=1\n'
                b'SUITE_SEQUENCE RESET time removed=0 changed=0 tcp=0 tcpwait=0\n'
                b'SUITE time START cases=1\n')
        late = (b'SUITE_SEQUENCE STALLED time cases=0/1\n'
                b'SUITE_SEQUENCE DONE suites=2\n')
        result, _ = self.replay(data, suite="directories,time", late=late)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("completed; stopping", result.stdout)
        self.assertIn("FAIL directories", result.stdout)
        self.assertIn("STALLED time", result.stdout)

    def test_one_case_completion_is_scored_only_when_requested(self):
        name = CASES[0]
        partial = (f"SUITE directories START cases=1 only={name}\n"
                   f"SUITE directories CASE {name} PASS ms=0\n"
                   f"SUITE directories DONE passed=1 failed=0 skipped=0 total=1 only={name}\n").encode()
        result, _ = self.replay(partial, only=name)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result, _ = self.replay(partial)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not full-suite evidence", result.stdout)

    def test_done_stops_before_idle_and_keeps_complete_serial(self):
        result, elapsed = self.replay(RECORDING, idle=5)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS: SUITE directories DONE", result.stdout)
        self.assertIn("completed; stopping", result.stdout)
        self.assertLess(elapsed, 4)
        self.assertGreaterEqual(elapsed, 2.4)

    def test_completed_suite_with_local_clock_fails(self):
        result, _ = self.replay(RECORDING, clock_offset=-3600)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("UTC clock FAIL", result.stdout)
        self.assertIn("PASS: SUITE directories DONE", result.stdout)

    def test_skipped_cases_fail_the_qemu_watcher(self):
        for count in (1, len(CASES)):
            with self.subTest(skipped=count):
                data = RECORDING
                for case in CASES[:count]:
                    data = data.replace(f"CASE {case} PASS ms=0".encode(),
                                        f"CASE {case} SKIP msg=unavailable".encode())
                data = data.replace(f"passed={len(CASES)} failed=0 skipped=0".encode(),
                                    f"passed={len(CASES) - count} failed=0 skipped={count}".encode())
                result, _ = self.replay(data)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("suite acceptance requires skipped=0", result.stdout)
                self.assertIn("completed; stopping", result.stdout)

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

    def test_interactive_modes_idle_without_a_clock_verdict(self):
        for mode in ("shell", "desktop"):
            result, _ = self.replay(b"interactive output\n", idle=0.2, mode=mode, split=False)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn("UTC clock", result.stdout)

    def test_program_requires_done_and_scores_actual_guest_path(self):
        missing, _ = self.replay(b"RUN /usr/bin/true START\n", idle=.2, mode="program", split=False)
        self.assertEqual(missing.returncode, 1, missing.stdout + missing.stderr)
        self.assertIn("no completion record", missing.stdout)
        for state, expected in (("PASS exit 0; no FAIL output", 0), ("FAIL exit 1", 1)):
            result, _ = self.replay(f"RUN /usr/bin/true DONE {state}\n".encode(), mode="program")
            self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
            self.assertIn("completed; stopping", result.stdout)

    def test_bad_clock_preserves_probe_and_testing_verdicts(self):
        for mode, data, expected in (
            ("probe", b"PROBE DONE passed=16 failed=0\n", "PASS: PROBE DONE passed=16 failed=0"),
            ("tests", b"TEST_TALLY: exited=1 nonzero=0 failed=[] started=1\nUSERSPACE TEST REPORT DONE\n", "PASS: TEST_TALLY:"),
        ):
            result, _ = self.replay(data, mode=mode, clock_offset=-3600)
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn(expected, result.stdout)
            self.assertIn("UTC clock FAIL", result.stdout)

    def test_kernel_failures_override_testing_report(self):
        done = (LOADER.decode() +
                "TEST_TALLY: exited=1 nonzero=0 failed=[] started=1\n"
                "USERSPACE TEST REPORT DONE\n")
        completion = runpy.run_path(str(ROOT / "scripts/wait-boot-done.py"))["completion"]
        for marker in ("[BOOT_TESTS:FAIL:1]", "[BOOT_TESTS:FAIL:12]",
                       "[TEST:system:tty_foreground_pgrp:FAIL:foreground pgrp missing]",
                       "[TEST:system:example:FAIL]"):
            for serial in (marker + "\n" + done, done + marker + "\n"):
                with self.subTest(marker=marker, serial=serial):
                    result = completion(serial, "tests", "")
                    self.assertFalse(result[0], result)
                    self.assertIn(marker, result[1])
        passed = "[BOOT_TESTS:FAIL:0]\n[TEST:system:example:PASS]\n" + done
        self.assertTrue(completion(passed, "tests", "")[0])

    def test_kernel_failure_stops_testing_watcher(self):
        data = (b"[TEST:system:tty_foreground_pgrp:FAIL:foreground pgrp missing]\n"
                b"[BOOT_TESTS:FAIL:1]\n"
                b"TEST_TALLY: exited=1 nonzero=0 failed=[] started=1\n"
                b"USERSPACE TEST REPORT DONE\n")
        result, _ = self.replay(data, mode="tests", split=False)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("fatal kernel output", result.stdout)

    def test_testing_completion_rejects_a_failed_canonical_program(self):
        data = b"TEST_TALLY: exited=2 nonzero=1 failed=[clonevm_exec_test:1] started=2\nUSERSPACE TEST REPORT DONE\n"
        result, _ = self.replay(data, mode="tests")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("canonical test failed: clonevm_exec_test:1", result.stdout)

    def test_testing_completion_rejects_unknown_child_exit(self):
        data = b"TEST_TALLY: exited=2 nonzero=1 failed=[dns_test_child_18:-9] started=2\nUSERSPACE TEST REPORT DONE\n"
        result, _ = self.replay(data, mode="tests")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unexpected process exit", result.stdout)

    def test_testing_completion_requires_exit_accounting(self):
        result, _ = self.replay(b"USERSPACE TEST REPORT DONE\n", mode="tests")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("no completed exit accounting", result.stdout)

    def test_testing_completion_preserves_expected_child_deaths(self):
        data = b"TEST_TALLY: exited=2 nonzero=1 failed=[sigkill_teardown_test_child_18:-9] started=2\nUSERSPACE TEST REPORT DONE\n"
        result, _ = self.replay(data, mode="tests")
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_testing_completion_rejects_skips_and_unloaded_programs(self):
        for line in (b"DNS_TEST: local_resolve SKIP\n",
                     b"[test] Loaded 73/78 test binaries (0 failed, 5 not found)\n"):
            result, _ = self.replay(line + b"USERSPACE TEST REPORT DONE\n", mode="tests")
            self.assertEqual(result.returncode, 1, result.stdout)

    def test_testing_loader_summary_is_mandatory_and_unique(self):
        completion = runpy.run_path(str(ROOT / "scripts/wait-boot-done.py"))["completion"]
        done = "TEST_TALLY: exited=1 nonzero=0 failed=[] started=1\nUSERSPACE TEST REPORT DONE\n"
        for loader in ("", LOADER.decode() * 2,
                       f"[test] Loaded {CANONICAL_COUNT-1}/{CANONICAL_COUNT-1} test binaries (0 failed, 0 not found)\n"):
            with self.subTest(loader=loader):
                result = completion(loader + done, "tests", "")
                self.assertIsNotNone(result)
                self.assertFalse(result[0], result)

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

    def test_probe_done_stops_before_idle(self):
        result, elapsed = self.replay(b"PROBE DONE passed=16 failed=0\n", idle=5, mode="probe")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS: PROBE DONE passed=16 failed=0", result.stdout)
        self.assertIn("completed; stopping", result.stdout)
        self.assertNotIn("No serial output", result.stdout)
        self.assertGreaterEqual(elapsed, 2.4)
        self.assertLess(elapsed, 4)

    def test_probe_and_testing_missing_done_idle_and_fail(self):
        for mode in ("probe", "tests"):
            with self.subTest(mode=mode):
                result, _ = self.replay(b"unfinished boot\n", idle=1, mode=mode)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("No serial output", result.stdout)
                self.assertIn("FAIL: no completion record", result.stdout)
                self.assertNotIn("PASS:", result.stdout)


if __name__ == "__main__":
    unittest.main()
