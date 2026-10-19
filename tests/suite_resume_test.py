#!/usr/bin/env python3
"""Replay interrupted suite boots through the host collector without a guest."""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch, Mock
from vigil_record_fixture import isolated_commands

ROOT = Path(__file__).resolve().parents[1]
SEQUENCE = runpy.run_path(str(ROOT / 'scripts/suite-sequence.py'))
DONE = runpy.run_path(str(ROOT / 'scripts/wait-boot-done.py'))


def terminal(suite, failed=0):
    return f'SUITE {suite} DONE passed={1-failed} failed={failed} skipped=0 total=1\n'


def load_metal(name):
    sys.path.insert(0, str(ROOT / 'scripts/metal'))
    loader = importlib.machinery.SourceFileLoader(name, str(ROOT / 'scripts/metal' / name))
    spec = importlib.util.spec_from_loader(name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class ResumeTest(unittest.TestCase):
    def test_failed_launch_or_incomplete_collection_is_not_a_guest_death(self):
        for status in (0, 1, 2, 75):
            calls = []
            def attempt(pending, number):
                calls.append(list(pending))
                return terminal('console'), status
            with self.subTest(status=status), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(SEQUENCE['collect'](['console', 'signals'], attempt), status or 1)
                self.assertEqual(calls, [['console', 'signals']])

    def test_cancel_waits_for_launcher_exit_trap_and_never_resumes(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            launcher = tmp / 'launcher.sh'
            launcher.write_text('''#!/bin/bash
echo attempt >> "$1/attempts"
trap 'sleep 0.8; echo cleaned > "$1/cleaned"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo 'SUITE console DONE passed=1 failed=0 skipped=0 total=1' > "$1/serial"
echo ready > "$1/ready"
while :; do sleep 0.1; done
''')
            for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGKILL):
                with self.subTest(signal=sig):
                    for name in ('ready', 'cleaned', 'attempts'):
                        (tmp / name).unlink(missing_ok=True)
                    # Kill the launcher itself in the SIGKILL leg; the collector must
                    # normalize the negative Popen status and treat it as cancellation.
                    command = ['bash', str(launcher), str(tmp)]
                    if sig == signal.SIGKILL:
                        launcher_text = launcher.read_text()
                        launcher.write_text(launcher_text.replace('echo ready >', 'kill -KILL $$\necho ready >'))
                    proc = subprocess.Popen([sys.executable, str(ROOT / 'scripts/suite-sequence.py'),
                                             'run', 'console,signals', str(tmp / 'serial'), *command],
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    try:
                        if sig != signal.SIGKILL:
                            deadline = time.monotonic() + 10
                            while not (tmp / 'ready').exists() and time.monotonic() < deadline:
                                time.sleep(0.02)
                            self.assertTrue((tmp / 'ready').exists())
                            proc.send_signal(sig)
                        out, err = proc.communicate(timeout=10)
                        self.assertEqual(proc.returncode, 128 + sig, out + err)
                        self.assertEqual((tmp / 'attempts').read_text(), 'attempt\n')
                        if sig != signal.SIGKILL:
                            self.assertEqual((tmp / 'cleaned').read_text(), 'cleaned\n')
                        self.assertNotIn('resuming unreached', out)
                    finally:
                        if proc.poll() is None:
                            proc.terminate()
                            proc.communicate(timeout=10)

    def test_dead_serial_reboots_only_unreached_in_order(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            calls = tmp / 'calls'
            replay = tmp / 'launcher.py'
            replay.write_text('''import json, os, sys
from pathlib import Path
args = sys.argv[2:]
suites = args[args.index('--suite')+1]
serial = Path(args[args.index('--serial-log')+1])
with Path(sys.argv[1]).open('a') as f: f.write(json.dumps(dict(args=args, owner=os.environ.get('VIGIL_BOOT_ID')))+'\\n')
if suites == 'console,signals,process,files-io':
    serial.write_text('SUITE console DONE passed=0 failed=1 skipped=0 total=1\\n'
                     'SUITE_SEQUENCE STALLED signals cases=0/1\\n'
                     'SUITE process START cases=1\\nKERNEL PANIC: replay death\\n')
    sys.exit(1)
serial.write_text(''.join('SUITE '+s+' DONE passed=1 failed=0 skipped=0 total=1\\n' for s in suites.split(','))
                 + 'SUITE_SEQUENCE DONE suites=2\\n')
''')
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/suite-sequence.py'), 'run',
                                     'console,signals,process,files-io', str(tmp / 'serial'), sys.executable,
                                     str(replay), str(calls)], env=dict(os.environ, VIGIL_BOOT_ID='initial'),
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            records = [json.loads(line) for line in calls.read_text().splitlines()]
            self.assertEqual([r['owner'] for r in records], ['initial', None])
            attempts = [r['args'] for r in records]
            self.assertEqual([a[a.index('--suite')+1] for a in attempts],
                             ['console,signals,process,files-io', 'process,files-io'])
            self.assertNotIn('--no-build', attempts[0])
            self.assertIn('--no-build', attempts[1])
            self.assertNotEqual(attempts[0][-1], attempts[1][attempts[1].index('--serial-log')+1])
            self.assertIn('KERNEL PANIC:', (tmp / 'serial').read_text())

    def test_failed_launch_does_not_retire_suites_from_old_serial(self):
        with tempfile.TemporaryDirectory() as tmp:
            serial = Path(tmp) / 'serial'
            serial.write_text(terminal('console') + terminal('files-io'))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                code = SEQUENCE['launch']('console,files-io', serial,
                                          [sys.executable, '-c', 'raise SystemExit(2)'])
            self.assertEqual(code, 2)
            self.assertEqual(serial.read_text(), '')
            self.assertIn('could not get past console; unreached: console,files-io', output.getvalue())

    def test_complete_records_and_no_progress_bound_retries(self):
        ids = ['console', 'signals', 'files-io']
        text = (terminal('console', 1) + 'SUITE_SEQUENCE STALLED signals cases=0/1\n'
                + 'SUITE files-io DONE passed=1 failed=0 skipped=0 total=1')
        self.assertEqual(SEQUENCE['unreached'](ids, text), ['files-io'])
        calls = []
        def attempt(pending, number):
            calls.append(list(pending))
            return (terminal('console', 1) + 'SUITE_SEQUENCE FAIL exec signals\nKERNEL PANIC: replay death\n' if number == 1 else 'lost serial\n', 1)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(SEQUENCE['collect'](ids, attempt), 1)
        self.assertEqual(calls, [ids, ['signals', 'files-io']])
        self.assertIn('could not get past signals; unreached: signals,files-io', output.getvalue())

    def test_failures_do_not_complete_a_sequence(self):
        for line in ('[BOOT_TESTS:FAIL:1]', '[TEST:example:FAIL]', '[console] tty1 shown: read back differs',
                     'SUITE_SEQUENCE RESET signals FAIL processes outlived SIGKILL', terminal('console', 1).strip(),
                     'SUITE smoke CASE heap/alloc FAIL ms=0 msg=allocation error:',
                     'SUITE smoke CASE kernel/check FAIL ms=0 msg=panicked at kernel/src/example.rs'):
            with self.subTest(line=line):
                self.assertIsNone(DONE['completion'](line+'\n', 'suite', 'console,signals'))
        self.assertEqual(DONE['completion']('SUITE_SEQUENCE DONE suites=2\n', 'suite', 'console,signals')[0], True)
        self.assertFalse(DONE['completion']('KERNEL PANIC: dead\n', 'suite', 'console,signals')[0])
        self.assertFalse(DONE['completion'](terminal('console').strip() + ' KERNEL PANIC: dead\n',
                                            'suite', 'console,signals')[0])

    def test_vm_waiter_does_not_score_on_a_failed_suite(self):
        with tempfile.TemporaryDirectory() as tmp:
            serial = Path(tmp) / 'serial'
            serial.write_text(f'[clock] UTC epoch={int(time.time())} source=PL031\n'
                              '[BOOT_TESTS:FAIL:1]\n' + terminal('smoke', 1))
            child = subprocess.Popen([sys.executable, str(ROOT / 'scripts/wait-boot-done.py'), str(serial),
                                      'suite', '--suite', 'smoke,crypto', '--timeout', '10'],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                time.sleep(1.2)
                self.assertIsNone(child.poll(), 'suite failure ended the VM waiter')
                with serial.open('a') as out:
                    out.write(terminal('crypto') + 'SUITE_SEQUENCE DONE suites=2\n')
                stdout, stderr = child.communicate(timeout=5)
                self.assertEqual(child.returncode, 0, stdout + stderr)
                self.assertIn('DONE: SUITE_SEQUENCE DONE suites=2', stdout)
            finally:
                if child.poll() is None:
                    child.terminate()
                    child.communicate(timeout=5)

    def test_metal_detached_collector_uses_the_same_pending_list(self):
        metal = load_metal('metal-run')
        args = SimpleNamespace(suite='console,signals,files-io')
        starts = []
        def boot(options):
            starts.append(options.suite)
            options.boot_id = 'second'
            return 0
        def remote(env, *command, **kwargs):
            path = command[-1]
            value = ''
            if path.endswith('/serial.txt'):
                value = terminal('console', 1) + 'KERNEL PANIC: replay death\n' if '/first/' in path else terminal('signals') + terminal('files-io')
            if path.endswith('/exit-status'):
                value = '1' if '/first/' in path else '0'
            return SimpleNamespace(returncode=0, stdout=value)
        with patch.object(metal, 'host_env', return_value={}), patch.object(metal, 'in_metal', side_effect=remote), \
             patch.object(metal, 'boot', side_effect=boot), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(metal.resume(args, 'first'), 1)
        self.assertEqual(starts, ['signals,files-io'])

    def test_metal_collects_failed_suites_until_exact_sequence_done(self):
        metal = load_metal('metal-boot')
        args = SimpleNamespace(id='20261018T000000Z-metal', mode='suite', suite='smoke,crypto',
                               payload='image', inject=None)
        with patch.object(metal.m, 'load_site', return_value={}):
            driver = metal.Driver(args)
        driver.event = driver.phase = Mock()
        driver.t0 = driver.kernel_started = 100
        driver.final_marker = ('SUITE_SEQUENCE DONE ', 'contains')
        driver.required_stages = [[('missing kernel stage', 'line')]]
        driver.lease_ceiling = 200
        driver.claimed = True
        with patch.object(metal.m, 'update_lease') as update:
            driver.on_line('SUITE smoke CASE heap/alloc FAIL ms=0 msg=allocation error:', observed_at=149)
            self.assertIsNone(driver.fatal_at)
            driver.on_line(terminal('smoke', 1).strip(), observed_at=150)
            driver.on_line('SUITE_SEQUENCE RESET crypto FAIL fixture', observed_at=151)
            driver.on_line('SUITE_SEQUENCE STALLED crypto cases=0/1', observed_at=152)
            driver.on_line('SUITE_SEQUENCE DONE suites=2 partial', observed_at=153)
            self.assertFalse(driver.final_seen)
            driver.on_line('SUITE_SEQUENCE DONE suites=2', observed_at=154)
            self.assertTrue(driver.final_seen)
            self.assertFalse(driver.completion_reported, 'missing definition stages must hold completion')
            driver.required_stages = [[]]
            with patch.object(driver, 'disk_evidence_seen', return_value=False):
                driver.on_line('still waiting for disk evidence', observed_at=154)
            self.assertFalse(driver.completion_reported, 'missing disk evidence must hold completion')
            with patch.object(driver, 'disk_evidence_seen', return_value=True):
                with self.assertRaises(metal.Finish) as finished:
                    driver.on_line('disk evidence complete', observed_at=154)
            self.assertEqual(finished.exception.status, metal.m.EXIT_DONE)
            update.assert_not_called()  # a sequence never extends the attempt's fixed lease ceiling
            driver.completion_reported = driver.final_seen = False
            driver.on_line('KERNEL PANIC!', observed_at=155)
            driver.on_line('SUITE_SEQUENCE DONE suites=2', observed_at=156)
            self.assertEqual(driver.fatal_at, 155)
            self.assertFalse(driver.completion_reported)

    def test_vigil_receives_one_ordered_boot_and_finishes_after_collection(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            cli = tmp / 'vigil-cli'
            calls = tmp / 'calls'
            cli.write_text('#!/bin/sh\n'
                           'if [ "$#" = 1 ]; then echo "usage: vigil-cli breenix start --suite a,b,c --cpus --arch"; exit 64; fi\n'
                           'printf "%s\n" "$*" >> "$CALLS"\n'
                           'if [ "$2" = start ]; then echo 20261018T000000Z-test; fi\n')
            cli.chmod(0o755)
            env = isolated_commands(dict(os.environ, VIGIL_CLI=str(cli), CALLS=str(calls)), tmp)
            uname = tmp / 'fixture-tools/uname'
            uname.write_text('#!/bin/sh\nprintf "Darwin\\n"\n')
            uname.chmod(0o755)
            env.pop('VIGIL_BOOT_ID', None)
            result = subprocess.run([str(ROOT / 'scripts/vigil-record.sh'), 'start', 'qemu', 'suite',
                                     'console,signals,files-io', str(tmp / 'serial')],
                                    env=env, capture_output=True, text=True, check=True)
            before = calls.read_text().splitlines()
            self.assertEqual(len(before), 1)
            self.assertIn('--suite console,signals,files-io', before[0])
            self.assertNotIn('finish', before[0])
            subprocess.run([str(ROOT / 'scripts/vigil-record.sh'), 'start', 'qemu', 'suite',
                            'files-io', str(tmp / 'retry-serial')], env=env, check=True, capture_output=True)
            starts = calls.read_text().splitlines()
            ids = [line.split('--id ')[1].split()[0] for line in starts]
            self.assertEqual(len(set(ids)), 2, 'resumed boots need distinct Vigil registrations')
            subprocess.run([str(ROOT / 'scripts/vigil-record.sh'), 'finish', result.stdout.strip(), '1'],
                           env=env, check=True)
            self.assertIn('breenix finish --id 20261018T000000Z-test --exit-status 1', calls.read_text())


if __name__ == '__main__':
    unittest.main()
