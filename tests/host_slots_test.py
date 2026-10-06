#!/usr/bin/env python3
"""Exercise real flock contention and launcher cleanup without starting a VM."""
import importlib.util
import fcntl
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import tempfile
import time
import unittest

HELPER = Path(__file__).resolve().parents[1] / 'scripts' / 'host-slots.py'
spec = importlib.util.spec_from_file_location('host_slots', HELPER)
slots = importlib.util.module_from_spec(spec)
spec.loader.exec_module(slots)


class HostSlotsTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.processes = []

    def tearDown(self):
        for process in self.processes:
            if process.poll() is None:
                process.terminate()
            try:
                process.communicate(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate()
        self.temporary.cleanup()

    def launch(self, script, bypass=False):
        # Only tests inject a directory and disable observational VM discovery.
        # Production's fixed directory has no environment override.
        program = f'''import importlib.util, sys
spec = importlib.util.spec_from_file_location('host_slots', {str(HELPER)!r})
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
m.SLOT_DIR = m.Path({str(self.root / 'locks')!r})
m.mac_vms = lambda: []
m.WAIT_MESSAGE_SECONDS = 0.2
sys.exit(m.supervise(['bash', '-c', sys.argv[1]]))'''
        process = subprocess.Popen([sys.executable, '-u', '-c', program, script],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT,
                                   env=dict(os.environ, BREENIX_BOOT_NO_QUEUE='1' if bypass else '0'))
        self.processes.append(process)
        return process

    def request(self, operation, resource):
        return f'{sys.executable} {HELPER} {operation} {resource}'

    def line_matching(self, process, text, timeout=10):
        # Read raw bytes: TextIO buffering can hide already-read lines from select.
        deadline = time.monotonic() + timeout
        collected = b''
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while time.monotonic() < deadline:
                if not selector.select(timeout=0.1):
                    continue
                chunk = os.read(process.stdout.fileno(), 4096)
                collected += chunk
                if text.encode() in collected:
                    return collected.decode()
                if not chunk:
                    break
        self.fail(f'missing {text!r}: {collected.decode()}')

    def test_two_builders_and_one_waiter(self):
        command = self.request('acquire', 'x86-build') + '; echo READY; read -r release'
        first, second = self.launch(command), self.launch(command)
        self.line_matching(first, 'READY')
        self.line_matching(second, 'READY')
        third = self.launch(command)
        waiting = self.line_matching(third, 'waiting for x86-build:')
        self.assertIn('worktree=', waiting)
        self.assertIn('commit=', waiting)
        self.assertIn('held=', waiting)
        self.assertIn('waited=', waiting)
        first.stdin.write(b'release\n'); first.stdin.flush()
        self.assertEqual(first.wait(timeout=10), 0)
        self.line_matching(third, 'READY')

    def test_one_boot_slot_and_release_on_sigkill(self):
        command = self.request('acquire', 'x86-boot') + '; echo READY; read -r release'
        first = self.launch(command)
        self.line_matching(first, 'READY')
        second = self.launch(command)
        self.line_matching(second, 'waiting for x86-boot:')
        # Kill the actual flock owner: no cleanup handler can release its lock.
        first.kill()
        first.wait(timeout=10)
        self.line_matching(second, 'READY')
        # The dead supervisor's shell was a read-only stand-in, not a VM.
        second.stdin.write(b'release\n'); second.stdin.flush()
        self.assertEqual(second.wait(timeout=10), 0)
        # EOF terminates first's read, leaving no stand-in running.
        first.stdin.close(); first.stdin = None

    def test_killed_launcher_stops_registered_vm_before_waiter(self):
        serial = self.root / 'serial.log'
        stopped = self.root / 'stopped'
        child_pid = self.root / 'child-pid'
        serial.write_bytes(b'PROBE DONE passed=16 failed=0\n')
        stop = f'{sys.executable} -c "from pathlib import Path; Path(\'{stopped}\').touch()"'
        command = (self.request('acquire', 'mac-boot') + '; '
                   f'{sys.executable} {HELPER} vm {serial} {stop}; '
                   f'echo $$ > {child_pid}; echo READY; read -r release')
        first = self.launch(command)
        self.line_matching(first, 'READY')
        second = self.launch(self.request('acquire', 'mac-boot') + '; echo READY; read -r release')
        self.line_matching(second, 'waiting for mac-boot:')
        os.kill(int(child_pid.read_text()), signal.SIGKILL)
        self.assertEqual(first.wait(timeout=10), 137)
        self.line_matching(second, 'READY')
        self.assertTrue(stopped.exists())
        header, guest = serial.read_text().split('\n', 1)
        context = json.loads(header.removeprefix('[host-slot] '))
        self.assertEqual(context['resource'], 'mac-boot')
        self.assertIn('load_at_acquire', context)
        self.assertEqual(guest, 'PROBE DONE passed=16 failed=0\n')

    def test_mac_bypass_does_not_bypass_x86_gate_slot(self):
        command = self.request('acquire', 'x86-boot') + '; echo READY; read -r release'
        first = self.launch(command)
        self.line_matching(first, 'READY')
        second = self.launch(command, bypass=True)
        self.line_matching(second, 'waiting for x86-boot:')
        mac = self.launch(self.request('acquire', 'mac-boot') + '; echo READY; read -r release')
        self.line_matching(mac, 'READY')
        bypass = self.launch(self.request('acquire', 'mac-boot') + '; echo READY', bypass=True)
        self.assertIn('BYPASS mac-boot', self.line_matching(bypass, 'READY'))
        self.assertEqual(bypass.wait(timeout=10), 0)

    def test_wait_messages_repeat_on_configured_cadence(self):
        self.assertEqual(slots.WAIT_MESSAGE_SECONDS, 60)
        command = self.request('acquire', 'x86-boot') + '; echo READY; read -r release'
        first = self.launch(command)
        self.line_matching(first, 'READY')
        second = self.launch(command)
        self.line_matching(second, 'waiting for x86-boot:')
        self.assertIn('held=', self.line_matching(second, 'waiting for x86-boot:'))

    def test_supervisor_preserves_command_exit_status(self):
        process = self.launch(self.request('acquire', 'x86-boot') + '; exit 23')
        self.assertEqual(process.wait(timeout=10), 23)

    def test_queue_context_contains_holder_and_wait(self):
        command = self.request('acquire', 'x86-boot') + '; echo READY; read -r release'
        first = self.launch(command)
        self.line_matching(first, 'READY')
        record = self.root / 'record'
        second = self.launch(self.request('acquire', 'x86-boot') + f'; cp "$BREENIX_SLOT_RECORD" {record}')
        self.line_matching(second, 'waiting for x86-boot:')
        first.stdin.write(b'release\n'); first.stdin.flush()
        self.assertEqual(first.wait(timeout=10), 0)
        self.assertEqual(second.wait(timeout=10), 0)
        context = json.loads(record.read_text())
        self.assertGreater(context['queue_wait_seconds'], 0)
        self.assertEqual(context['observed_running'][0]['resource'], 'x86-boot')
        self.assertEqual(len(context['load_at_enqueue']), 3)
        self.assertEqual(len(context['load_at_acquire']), 3)

    def test_parallel_observers_are_not_reported_as_running_builds(self):
        pool = slots.Slots(self.root / 'locks')
        # Hold the same shared probe another observer holds during snapshot().
        with pool.path('x86-build', 1).open('a+') as observer:
            fcntl.flock(observer, fcntl.LOCK_SH | fcntl.LOCK_NB)
            self.assertEqual(pool.snapshot(), [])
        pool.close()

    def test_explicit_release_keeps_lock_inode_and_ignores_stale_metadata(self):
        pool = slots.Slots(self.root / 'locks')
        identity = {'worktree': 'test-worktree', 'commit': 'test-commit'}
        pool.try_acquire('x86-boot', identity)
        inode = pool.path('x86-boot', 1).stat().st_ino
        self.assertEqual(pool.snapshot()[0]['commit'], 'test-commit')
        pool.release('x86-boot')
        self.assertEqual(pool.snapshot(), [])
        self.assertIsNotNone(pool.try_acquire('x86-boot', identity))
        self.assertEqual(pool.path('x86-boot', 1).stat().st_ino, inode)
        pool.close()


if __name__ == '__main__':
    unittest.main()
