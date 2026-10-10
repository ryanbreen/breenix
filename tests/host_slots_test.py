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
from unittest import mock

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

    def launch(self, script, bypass=False, miss_first_snapshot=False, environment=None):
        # Only tests inject a directory and disable observational VM discovery.
        # Production's fixed directory has no environment override.
        program = f'''import importlib.util, sys
spec = importlib.util.spec_from_file_location('host_slots', {str(HELPER)!r})
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
m.SLOT_DIR = m.Path({str(self.root / 'locks')!r})
m.mac_vms = lambda: []
original_command_output = m.command_output
m.command_output = lambda argv: '' if argv[0] == 'pgrep' else original_command_output(argv)
m.WAIT_MESSAGE_SECONDS = 0.2
if {miss_first_snapshot!r}:
    original_snapshot = m.Slots.snapshot
    def missed_snapshot(self):
        m.Slots.snapshot = original_snapshot
        return []
    m.Slots.snapshot = missed_snapshot
sys.exit(m.supervise(['bash', '-c', sys.argv[1]]))'''
        process = subprocess.Popen([sys.executable, '-u', '-c', program, script],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT,
                                   env=dict(os.environ, BREENIX_BOOT_NO_QUEUE='1' if bypass else '0',
                                            **(environment or {})))
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

    @unittest.skipUnless(sys.platform == 'linux' and os.uname().machine == 'x86_64',
                         'Linux x86 CPU affinity')
    def test_guest_threads_use_boot_cpus_and_gate_work_keeps_work_cpus(self):
        allowed = sorted(os.sched_getaffinity(0))
        binary = self.root / 'qemu-system-x86_64'
        binary.write_text(f'''#!{sys.executable}
import json, os, threading
def vcpu():
    print('VCPU=' + json.dumps(sorted(os.sched_getaffinity(0))), flush=True)
threads = [threading.Thread(target=vcpu) for i in range(4)]
for thread in threads: thread.start()
for thread in threads: thread.join()
''')
        binary.chmod(0o755)
        work = f"{sys.executable} -c 'import json,os;print(\"WORK=\"+json.dumps(sorted(os.sched_getaffinity(0))))'"
        command = '; '.join([work, self.request('acquire', 'x86-build'), work,
                             self.request('release', 'x86-build'),
                             self.request('acquire', 'x86-boot'), 'qemu-system-x86_64', work,
                             self.request('release', 'x86-boot'), work])
        process = self.launch(command, environment={'PATH': str(self.root) + os.pathsep + os.environ['PATH']})
        output, _ = process.communicate(timeout=30)
        self.assertEqual(process.returncode, 0, output.decode())
        lines = output.decode().splitlines()
        self.assertEqual([json.loads(row[5:]) for row in lines if row.startswith('WORK=')], [allowed[4:]] * 4)
        self.assertEqual([json.loads(row[5:]) for row in lines if row.startswith('VCPU=')], [allowed[:4]] * 4)
        self.assertIn('[host-cpus] QEMU and inherited vCPU threads=', output.decode())

    @unittest.skipUnless(sys.platform == 'linux' and os.uname().machine == 'x86_64',
                         'Linux x86 CPU affinity')
    def test_guest_cannot_start_without_boot_lease(self):
        binary = self.root / 'qemu-system-x86_64'
        binary.write_text('#!/bin/sh\necho UNLEASED_GUEST\n')
        binary.chmod(0o755)
        process = self.launch('qemu-system-x86_64',
                              environment={'PATH': str(self.root) + os.pathsep + os.environ['PATH']})
        output, _ = process.communicate(timeout=30)
        self.assertNotEqual(process.returncode, 0)
        self.assertNotIn('UNLEASED_GUEST', output.decode())
        self.assertIn('QEMU requires the x86-boot lease', output.decode())

    def test_fresh_cargo_home_preserves_config_without_copying_live_cache(self):
        source = self.root / 'cargo'
        (source / 'registry' / 'index').mkdir(parents=True)
        (source / 'git').mkdir()
        cached = source / 'registry' / 'index' / 'entry'
        cached.write_text('shared')
        config = source / 'config.toml'
        config.write_text('[net]\nretry = 7\n')
        credentials = source / 'credentials.toml'
        credentials.write_text('test-only credential fixture')
        credentials.chmod(0o600)
        for name in ('.package-cache', '.package-cache-mutate', '.global-cache'):
            (source / name).write_text('not a seed')
        destination = slots.isolated_cargo_home(source, self.root / 'target')
        self.assertFalse((destination / 'registry').exists())
        self.assertFalse((destination / 'git').exists())
        self.assertEqual(cached.read_text(), 'shared')
        self.assertEqual((destination / 'config.toml').read_bytes(), config.read_bytes())
        self.assertEqual((destination / 'credentials.toml').stat().st_mode & 0o777, 0o600)
        for name in ('.package-cache', '.package-cache-mutate', '.global-cache'):
            self.assertFalse((destination / name).exists())

    def test_snapshot_waits_for_holder_metadata_publication(self):
        directory = self.root / 'locks'
        directory.mkdir()
        (directory / 'x86-boot-1.json').write_text(json.dumps({
            'worktree': 'previous', 'commit': 'previous', 'started': time.time() - 120}))
        setup = (f"import importlib.util; spec=importlib.util.spec_from_file_location('m',{str(HELPER)!r}); "
                 f"m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m); "
                 f"pool=m.Slots({str(directory)!r}); ")
        writer_code = setup + """import sys
original = m.json.dumps
def publish(holder):
    print('LOCKED', flush=True)
    sys.stdin.readline()
    return original(holder)
m.json.dumps = publish
pool.try_acquire('x86-boot', {'worktree':'current','commit':'current'})
print('PUBLISHED', flush=True)
sys.stdin.readline()
"""
        writer = subprocess.Popen([sys.executable, '-c', writer_code], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.processes.append(writer)
        self.line_matching(writer, 'LOCKED')
        reader = subprocess.Popen([sys.executable, '-c', setup + 'print(m.json.dumps(pool.snapshot()))'],
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.processes.append(reader)
        with selectors.DefaultSelector() as selector:
            selector.register(reader.stdout, selectors.EVENT_READ)
            self.assertFalse(selector.select(timeout=0.2), 'reader saw unpublished metadata')
        writer.stdin.write(b'publish\n'); writer.stdin.flush()
        output = reader.communicate(timeout=10)[0]
        self.assertEqual(reader.returncode, 0, output)
        self.assertEqual(json.loads(output)[0]['commit'], 'current')
        writer.stdin.write(b'release\n'); writer.stdin.flush()
        self.assertEqual(writer.wait(timeout=10), 0)

    def test_wait_message_refreshes_holder_after_acquisition_race(self):
        command = self.request('acquire', 'mac-boot') + '; echo READY; read -r release'
        first = self.launch(command)
        self.line_matching(first, 'READY')
        second = self.launch(command, miss_first_snapshot=True)
        waiting = self.line_matching(second, 'waiting for mac-boot:')
        self.assertIn('worktree=', waiting)
        self.assertIn('commit=', waiting)
        self.assertIn('held=', waiting)
        self.assertNotIn('publishing', waiting)
        first.stdin.write(b'release\n'); first.stdin.flush()
        self.assertEqual(first.wait(timeout=10), 0)
        self.line_matching(second, 'READY')

    def test_one_boot_slot_and_release_on_sigkill(self):
        child_pid = self.root / 'detached-pid'
        child_code = f"import os,time; from pathlib import Path; Path({str(child_pid)!r}).write_text(str(os.getpid())); time.sleep(60)"
        import shlex
        command = (self.request('acquire', 'x86-boot') + '; ' +
                   f'{sys.executable} -c ' + shlex.quote("import subprocess; subprocess.Popen([" + repr(sys.executable) + ", '-c', " + repr(child_code) + "], start_new_session=True)") +
                   '; echo READY; read -r release')
        first = self.launch(command)
        self.line_matching(first, 'READY')
        second = self.launch(self.request('acquire', 'x86-boot') + '; echo READY; read -r release')
        self.line_matching(second, 'waiting for x86-boot:')
        # Allow discovery of the detached stand-in, then kill the foreground handle.
        deadline = time.monotonic() + 5
        while not child_pid.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        pid = int(child_pid.read_text())
        time.sleep(0.6)
        first.kill()
        first.wait(timeout=10)
        self.line_matching(second, 'READY')
        state = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='], capture_output=True, text=True).stdout.strip()
        self.assertTrue(not state or state.startswith('Z'), state)
        second.stdin.write(b'release\n'); second.stdin.flush()
        self.assertEqual(second.wait(timeout=10), 0)
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
        context = json.loads(serial.with_name(serial.name + '.host-slots.jsonl').read_text())
        self.assertEqual(context['resource'], 'mac-boot')
        self.assertIn('load_at_acquire', context)
        self.assertEqual(serial.read_text(), 'PROBE DONE passed=16 failed=0\n')

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

    def test_interrupted_child_keeps_its_cleanup_status(self):
        process = self.launch(self.request('acquire', 'mac-boot') +
                              "; trap 'exit 0' INT; echo READY; while :; do sleep 1; done")
        self.line_matching(process, 'READY')
        process.send_signal(signal.SIGINT)
        self.assertEqual(process.wait(timeout=10), 0)

    def test_fifo_waiters_precede_a_reacquiring_holder(self):
        first = slots.Slots(self.root / 'fifo')
        second = slots.Slots(self.root / 'fifo')
        identity = {'worktree': 'fixture', 'commit': 'fixture'}
        first.enqueue('x86-boot')
        self.assertIsNotNone(first.try_acquire('x86-boot', identity))
        second.enqueue('x86-boot')
        first.release('x86-boot')
        first.enqueue('x86-boot')
        self.assertIsNone(first.try_acquire('x86-boot', identity))
        self.assertIsNotNone(second.try_acquire('x86-boot', identity))
        second.path('x86-boot', 1).with_suffix('.json').write_text('{}')
        self.assertEqual(first.snapshot()[0]['worktree'], 'unknown')
        first.close(); second.close()

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

    def test_zombie_only_process_group_cleanup_succeeds(self):
        # Keep the zombie's parent outside its group so the group persists.
        code = ("import os,sys; child=os.fork(); "
                "(os.setpgid(0,0),os._exit(0)) if child==0 else "
                "(print(child,flush=True),sys.stdin.readline(),os.waitpid(child,0))")
        process = subprocess.Popen([sys.executable, '-c', code], stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.processes.append(process)
        child = int(process.stdout.readline().strip())
        deadline = time.monotonic() + 5
        state = ''
        while time.monotonic() < deadline:
            state = subprocess.check_output(['ps', '-p', str(child), '-o', 'stat='], text=True).strip()
            if state.startswith('Z'):
                break
            time.sleep(0.01)
        self.assertTrue(state.startswith('Z'), state)
        slots.signal_process_group(child, signal.SIGKILL)
        process.stdin.write(b'reap\n'); process.stdin.flush()
        self.assertEqual(process.wait(timeout=10), 0)

    def test_cleanup_permission_failure_with_live_group_is_not_hidden(self):
        process = subprocess.Popen(['sleep', '60'], start_new_session=True,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.processes.append(process)
        with mock.patch.object(slots.os, 'killpg', side_effect=PermissionError(1, 'denied')):
            with self.assertRaises(PermissionError):
                slots.signal_process_group(process.pid, signal.SIGKILL)

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
