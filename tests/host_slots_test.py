#!/usr/bin/env python3
"""Exercise real flock contention and launcher cleanup without starting a VM."""
import importlib.util
import fcntl
import json
import os
import re
from pathlib import Path
import selectors
import signal
import shlex
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
# These tests model the gate VM's two build leases; build_slots() sizes them to the host it runs on.
slots.RESOURCES['x86-build'] = 2


class HostSlotsTest(unittest.TestCase):
    def test_vmware_names_are_reused_per_held_mac_boot_slot(self):
        # Use the launcher's actual naming expression against two real leases.
        source = (HELPER.parents[1] / 'run.sh').read_text()
        name = next(line.strip() for line in source.splitlines() if 'VM_NAME=' in line)
        script = (f'source {HELPER.with_suffix(".sh")}; '
                  + self.vmware_name_function(source) + '; host_slot_acquire mac-boot; ' + name + '; '
                  'echo READY:$VM_NAME; read -r release')
        first = self.launch(script, prelude="m.RESOURCES['mac-boot'] = 2")
        self.assertIn('READY:breenix-mac-boot-1', self.line_matching(first, 'READY:'))
        second = self.launch(script, prelude="m.RESOURCES['mac-boot'] = 2")
        self.assertIn('READY:breenix-mac-boot-2', self.line_matching(second, 'READY:'))
        first.communicate(input=b'finish\n', timeout=15)
        self.assertEqual(first.returncode, 0)
        again = self.launch(script, prelude="m.RESOURCES['mac-boot'] = 2")
        self.assertIn('READY:breenix-mac-boot-1', self.line_matching(again, 'READY:'))
        for process in (second, again):
            process.communicate(input=b'finish\n', timeout=15)
            self.assertEqual(process.returncode, 0)

    def vmware_name_function(self, source):
        body = re.search(r'(?ms)^vmware_boot_name\(\) \{\n(.*?)^\}', source)[1]
        return 'vmware_boot_name() {\n' + body + '\n}'

    def test_unqueued_vmware_names_do_not_require_or_reuse_a_slot(self):
        source = (HELPER.parents[1] / 'run.sh').read_text()
        name = next(line.strip() for line in source.splitlines() if 'VM_NAME=' in line)
        script = (f'source {HELPER.with_suffix(".sh")}; '
                  + self.vmware_name_function(source) + '; set -e; '
                  'host_slot_acquire mac-boot; ' + name + '; echo NAME:$VM_NAME; '
                  + name + '; echo NAME:$VM_NAME')
        process = self.launch(script, bypass=True)
        output = process.communicate(timeout=15)[0].decode()
        self.assertEqual(process.returncode, 0, output)
        names = [line for line in output.splitlines() if line.startswith('NAME:')]
        self.assertEqual(len(names), 2, output)
        self.assertTrue(all(n.startswith('NAME:breenix-unqueued-') for n in names), output)
        self.assertNotEqual(names[0], names[1], output)

    def test_slot_number_requires_a_held_lease(self):
        script = (self.request('number', 'mac-boot') + '; test $? != 0; '
                  + self.request('acquire', 'mac-boot') + '; '
                  + self.request('release', 'mac-boot') + '; '
                  + self.request('number', 'mac-boot') + '; test $? != 0')
        process = self.launch(script)
        output = process.communicate(timeout=15)[0].decode()
        self.assertEqual(process.returncode, 0, output)
        self.assertEqual(output.count('RuntimeError: resource lease is not held'), 2, output)

    def test_staged_classifier_stops_checkout_without_watchdog(self):
        bundle = self.root / 'watchdog'
        repo = HELPER.parents[1]
        for relative in ('scripts/boot_progress.py', 'docs/boot-path.json',
                         'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-aarch64.json',
                         'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
            destination = bundle / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            source = (repo / relative).read_text()
            destination.write_text(source.replace('STALL_SECONDS = 120', 'STALL_SECONDS = 0.4'))
        historical = self.root / 'historical'
        historical.mkdir()
        serial = self.root / 'serial_user.log'
        replay = f'''from pathlib import Path
import time
p = Path({str(serial)!r})
p.write_text('SIGKILL_TEARDOWN_TEST: FAIL\\n')
for i in range(100):
    with p.open('a') as stream: stream.write('[CPU_CENSUS:seq=%d]\\n' % i)
    time.sleep(.05)
'''
        fake = self.root / 'qemu-system-fake'
        fake.symlink_to(sys.executable)
        for enabled, halted in ((False, False), (True, True), (True, False)):
            with self.subTest(enabled=enabled, halted=halted):
                serial.unlink(missing_ok=True)
                Path(str(serial) + '.stall.json').unlink(missing_ok=True)
                bounded_replay = replay.replace('range(100)', 'range(40)')
                script = (f'{sys.executable} {HELPER} acquire mac-boot; '
                          f'{sys.executable} {HELPER} serial {serial}; '
                          f'{sys.executable} {HELPER} quiet; '
                          f'sleep 1; exec {fake} -c {shlex.quote(bounded_replay)} '
                          f'-serial file:{serial}' + (' -S' if halted else ''))
                with mock.patch.dict(os.environ, BREENIX_SLOT_WORKTREE=str(historical),
                                     BREENIX_SLOT_PROGRESS_HELPER=str(bundle / 'scripts/boot_progress.py'),
                                     BREENIX_QUIET='0', BREENIX_WATCH_HISTORICAL=str(int(enabled))):
                    process = self.launch(script, qemu_alias=str(fake))
                output = process.communicate(timeout=10)[0].decode()
                if enabled and not halted:
                    self.assertEqual(process.returncode, 124, output)
                    record = json.loads(Path(str(serial) + '.stall.json').read_text())
                    self.assertEqual(record['status'], 'stalled')
                    self.assertEqual(record['last_progress_line'], 'SIGKILL_TEARDOWN_TEST: FAIL')
                else:
                    self.assertEqual(process.returncode, 0, output)
                    self.assertFalse(Path(str(serial) + '.stall.json').exists(), output)

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

    def launch(self, script, bypass=False, miss_first_snapshot=False, temporary_directory=None, settings=None, qemu_alias=None,
               prelude=''):
        # Only tests inject a directory and disable observational VM discovery and
        # the Linux vCPU gap sampler, so the host's own guests and load can't decide them.
        # Production's fixed directory has no environment override.
        program = f'''import importlib.util, sys
spec = importlib.util.spec_from_file_location('host_slots', {str(HELPER)!r})
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
m.SLOT_DIR = m.Path({str(self.root / 'locks')!r})
m.RESOURCES['x86-build'] = 2
m.mac_vms = lambda: []
m.x86_vms = lambda: []
m.vcpu_gap_ms = lambda seconds: 0.0
m.WAIT_MESSAGE_SECONDS = 0.2
if {qemu_alias!r}:
    original_table = m.process_table
    def with_qemu_alias(*args):
        return {{pid: (*row[:3], {qemu_alias!r} + ' ' + row[3])
                 if ' -c from pathlib import Path' in row[3] and '-serial file:' in row[3]
                 else row for pid, row in original_table(*args).items()}}
    m.process_table = with_qemu_alias
for name, value in {settings or {}!r}.items():
    setattr(m, name, value)
if {miss_first_snapshot!r}:
    original_snapshot = m.Slots.snapshot
    def missed_snapshot(self):
        m.Slots.snapshot = original_snapshot
        return []
    m.Slots.snapshot = missed_snapshot
{prelude}
sys.exit(m.supervise(['bash', '-c', sys.argv[1]]))'''
        process = subprocess.Popen([sys.executable, '-u', '-c', program, script],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT,
                                   env=dict(os.environ, BREENIX_BOOT_NO_QUEUE='1' if bypass else '0',
                                            TMPDIR=str(temporary_directory or tempfile.gettempdir())))
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

    def test_build_slots_fit_the_machines_cpus_and_memory(self):
        for cpus, memory, expected in ((24, 8 * 1024 ** 3, 2), (24, 64 * 1024 ** 3, 6), (8, 64 * 1024 ** 3, 2), (2, 1024 ** 3, 1)):
            with self.subTest(cpus=cpus, memory=memory), \
                    mock.patch.object(slots.os, 'sched_getaffinity', lambda pid: set(range(cpus)), create=True), \
                    mock.patch.object(slots.Path, 'read_text', lambda self: str(memory)):
                self.assertEqual(slots.build_slots(), expected)

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
        self.line_matching(second, 'before x86-boot:')
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
        self.line_matching(second, 'before x86-boot:')
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
        self.line_matching(second, 'before x86-boot:')
        self.assertIn('held=', self.line_matching(second, 'before x86-boot:'))

    def test_supervisor_works_with_a_long_nested_tmpdir(self):
        directory = self.root / ('nested-' + 'x' * 100)
        directory.mkdir()
        process = self.launch(self.request('acquire', 'x86-boot') +
                              '; echo CONTROL=$BREENIX_SLOT_SESSION; exit 0',
                              temporary_directory=directory)
        output = self.line_matching(process, 'CONTROL=')
        control = output.split('CONTROL=', 1)[1].splitlines()[0]
        self.assertLess(len(os.fsencode(control)), 104)
        self.assertFalse(control.startswith(str(directory)))
        self.assertEqual(process.wait(timeout=10), 0)

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
        self.line_matching(second, 'before x86-boot:')
        first.stdin.write(b'release\n'); first.stdin.flush()
        self.assertEqual(first.wait(timeout=10), 0)
        self.assertEqual(second.wait(timeout=10), 0)
        context = json.loads(record.read_text())
        self.assertGreater(context['queue_wait_seconds'], 0)
        # The running boot holds every build lease, so those are what the second boot waited on.
        self.assertEqual(context['observed_running'][0]['resource'], 'x86-build')
        self.assertEqual(context['observed_running'][0]['reason'], 'x86-boot running')
        self.assertGreater(context['exclusive_wait_seconds'], 0)
        self.assertEqual(len(context['load_at_enqueue']), 3)
        self.assertEqual(len(context['load_at_acquire']), 3)

    def test_boot_holds_every_build_lease_until_it_ends(self):
        boot = self.launch(self.request('acquire', 'x86-boot') + '; echo READY; read -r release; '
                           + self.request('release', 'x86-boot') + '; echo RELEASED; read -r done')
        self.line_matching(boot, 'READY')
        build = self.launch(self.request('acquire', 'x86-build') + '; echo BUILDING; read -r done')
        self.assertIn('x86-boot running', self.line_matching(build, 'waiting for x86-build:'))
        boot.stdin.write(b'release\n'); boot.stdin.flush()
        self.line_matching(boot, 'RELEASED')
        self.line_matching(build, 'BUILDING')
        for process in (boot, build):
            process.stdin.write(b'done\n'); process.stdin.flush()
            self.assertEqual(process.wait(timeout=10), 0)

    def test_metal_build_runs_beside_an_x86_boot_and_its_builds(self):
        boot = self.launch(self.request('acquire', 'x86-boot') + '; echo BOOTING; read -r done')
        self.line_matching(boot, 'BOOTING')
        build = self.launch(self.request('acquire', 'metal-build') + '; echo BUILDING; read -r done')
        self.line_matching(build, 'BUILDING')
        for process in (boot, build):
            process.stdin.write(b'done\n'); process.stdin.flush()
            self.assertEqual(process.wait(timeout=10), 0)

    def test_boot_waits_for_running_build_then_holds_both(self):
        build = self.launch(self.request('acquire', 'x86-build') + '; echo BUILDING; read -r done')
        self.line_matching(build, 'BUILDING')
        boot = self.launch(self.request('acquire', 'x86-boot') + '; echo BOOTING; read -r done')
        self.line_matching(boot, 'waiting for every x86-build lease before x86-boot:')
        build.stdin.write(b'done\n'); build.stdin.flush()
        self.assertEqual(build.wait(timeout=10), 0)
        self.line_matching(boot, 'BOOTING')
        later = self.launch(self.request('acquire', 'x86-build') + '; echo LATER')
        self.line_matching(later, 'waiting for x86-build:')
        boot.stdin.write(b'done\n'); boot.stdin.flush()
        self.assertEqual(boot.wait(timeout=10), 0)
        self.line_matching(later, 'LATER')
        self.assertEqual(later.wait(timeout=10), 0)

    def test_session_holding_a_build_lease_can_take_the_boot(self):
        # Historical gates hold a build lease while they ask for the boot.
        session = self.launch(self.request('acquire', 'x86-build') + '; ' + self.request('acquire', 'x86-boot') + '; echo BOOTING')
        self.line_matching(session, 'BOOTING')
        self.assertEqual(session.wait(timeout=10), 0)

    def write_manifest(self, sensitive):
        manifest = self.root / 'suite.json'
        manifest.write_text(json.dumps({'id': 'suite', 'timingSensitive': sensitive}))
        return manifest

    def test_timing_sensitive_boot_waits_for_quiet_host(self):
        serial = self.root / 'serial.log'
        serial.write_text('')
        manifest = self.write_manifest(True)
        quiet = {'QUIET_WAITS_ON_THIS_HOST': True, 'QUIET_LOAD_FRACTION': 1000.0, 'QUIET_SAMPLES': 2, 'QUIET_SAMPLE_SECONDS': 0.2}
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                              f'{sys.executable} {HELPER} quiet {manifest}; '
                              f'{sys.executable} {HELPER} serial {serial}; echo BOOTING; '
                              f'echo "SUITE suite DONE passed=1 failed=0 skipped=0 total=1" >> {serial}; sleep 1.5; '
                              + self.request('release', 'mac-boot'), settings=quiet)
        self.line_matching(session, 'quiet host:')
        self.line_matching(session, 'BOOTING')
        self.assertEqual(session.wait(timeout=10), 0)
        record = json.loads(serial.with_name(serial.name + '.host-slots.jsonl').read_text())
        self.assertEqual(record['quiet']['required'], True)
        self.assertEqual(record['quiet']['met'], True)
        for key in ('load_at_acquire', 'load_at_start', 'load_at_verdict', 'load_at_release'):
            self.assertEqual(len(record[key]), 3, key)

    def test_busy_host_boots_after_the_cap_recorded_as_not_quiet(self):
        serial = self.root / 'serial.log'
        serial.write_text('')
        manifest = self.write_manifest(True)
        busy = {'QUIET_WAITS_ON_THIS_HOST': True, 'QUIET_LOAD_FRACTION': -1.0, 'QUIET_SAMPLE_SECONDS': 0.2, 'QUIET_MAX_WAIT_SECONDS': 1.0}
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                              f'{sys.executable} {HELPER} quiet {manifest}; '
                              f'{sys.executable} {HELPER} serial {serial}; echo BOOTING', settings=busy)
        self.line_matching(session, 'waiting for a quiet host')
        self.line_matching(session, 'host NOT quiet')
        self.line_matching(session, 'BOOTING')
        self.assertEqual(session.wait(timeout=10), 0)
        record = json.loads(serial.with_name(serial.name + '.host-slots.jsonl').read_text())
        self.assertEqual(record['quiet']['met'], False)
        self.assertGreaterEqual(record['quiet']['wait_seconds'], 1.0)
        self.assertEqual(len(record['load_at_release']), 3)
        self.assertNotIn('load_at_verdict', record)  # The serial printed no DONE line.

    def test_nested_acquire_shares_the_boot_record(self):
        serial = self.root / 'serial.log'
        serial.write_text('')
        manifest = self.write_manifest(True)
        quiet = {'QUIET_WAITS_ON_THIS_HOST': True, 'QUIET_LOAD_FRACTION': 1000.0, 'QUIET_SAMPLES': 1, 'QUIET_SAMPLE_SECONDS': 0.2}
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; {sys.executable} {HELPER} serial {serial}; '
                              f'{sys.executable} {HELPER} acquire mac-boot; {sys.executable} {HELPER} quiet {manifest}; echo BOOTING',
                              settings=quiet)
        self.line_matching(session, 'BOOTING')
        self.assertEqual(session.wait(timeout=10), 0)
        lines = serial.with_name(serial.name + '.host-slots.jsonl').read_text().splitlines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(json.loads(lines[0])['quiet']['met'], True)

    def test_a_host_that_does_not_wait_boots_a_timing_sensitive_suite_at_once(self):
        manifest = self.write_manifest(True)
        busy = {'QUIET_WAITS_ON_THIS_HOST': False, 'QUIET_LOAD_FRACTION': -1.0, 'QUIET_SAMPLE_SECONDS': 0.2,
                'QUIET_MAX_WAIT_SECONDS': 60.0}
        record = self.root / 'record'
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                              f'{sys.executable} {HELPER} quiet {manifest}; '
                              f'cp "$BREENIX_SLOT_RECORD" {record}; echo BOOTING', settings=busy)
        self.line_matching(session, 'BOOTING', timeout=5)
        self.assertEqual(session.wait(timeout=10), 0)
        self.assertEqual(json.loads(record.read_text())['quiet'], {'required': False})

    def test_no_host_waits_by_default(self):
        self.assertIs(slots.QUIET_WAITS_ON_THIS_HOST, False)

    def test_suite_that_is_not_timing_sensitive_does_not_wait(self):
        manifest = self.write_manifest(False)
        busy = {'QUIET_LOAD_FRACTION': -1.0, 'QUIET_SAMPLE_SECONDS': 0.2, 'QUIET_MAX_WAIT_SECONDS': 60.0}
        record = self.root / 'record'
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                              f'{sys.executable} {HELPER} quiet {manifest}; '
                              f'cp "$BREENIX_SLOT_RECORD" {record}; echo BOOTING', settings=busy)
        self.line_matching(session, 'BOOTING', timeout=5)
        self.assertEqual(session.wait(timeout=10), 0)
        self.assertEqual(json.loads(record.read_text())['quiet'], {'required': False})

    def test_two_sessions_holding_build_leases_both_take_the_boot(self):
        # Historical gates each hold one of the two build leases, then ask for the boot,
        # which needs both: each gives its own up while it waits.
        command = (self.request('acquire', 'x86-build') + '; echo HAVE; read -r go; '
                   + self.request('acquire', 'x86-boot') + '; echo BOOTING; read -r done')
        first, second = self.launch(command), self.launch(command)
        self.line_matching(first, 'HAVE')
        self.line_matching(second, 'HAVE')
        for process in (first, second):
            process.stdin.write(b'go\n'); process.stdin.flush()
        winner = None
        deadline = time.monotonic() + 15
        while winner is None and time.monotonic() < deadline:
            for process in (first, second):
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ)
                    if selector.select(timeout=0.1) and b'BOOTING' in os.read(process.stdout.fileno(), 65536):
                        winner = process
                        break
        self.assertIsNotNone(winner, 'neither session took the boot: deadlock')
        loser = second if winner is first else first
        winner.stdin.write(b'done\n'); winner.stdin.flush()
        self.assertEqual(winner.wait(timeout=10), 0)
        self.line_matching(loser, 'BOOTING', timeout=15)
        loser.stdin.write(b'done\n'); loser.stdin.flush()
        self.assertEqual(loser.wait(timeout=10), 0)

    def test_boot_requests_queue_for_builds_and_boot_in_one_order(self):
        # A-boot, B-boot, B-build, A-build queued separately let B hold every build lease
        # behind A's boot ticket. One enqueue per request orders both queues alike.
        first, second = slots.Slots(self.root / 'locks'), slots.Slots(self.root / 'locks')
        try:
            first.enqueue('x86-build', 'x86-boot')
            second.enqueue('x86-build', 'x86-boot')
            for resource in ('x86-build', 'x86-boot'):
                queue = sorted((self.root / 'locks').glob(f'{resource}-wait-*'))
                self.assertEqual(queue, [first.tickets[resource][0], second.tickets[resource][0]])
            identity = {'worktree': 'w', 'commit': 'c'}
            self.assertFalse(second.try_acquire_all('x86-build', identity, 'x86-boot running'))
            self.assertTrue(first.try_acquire_all('x86-build', identity, 'x86-boot running'))
            self.assertIsNotNone(first.try_acquire('x86-boot', identity))
            first.release('x86-boot')
            self.assertTrue(second.try_acquire_all('x86-build', identity, 'x86-boot running'))
            self.assertIsNotNone(second.try_acquire('x86-boot', identity))
        finally:
            first.close()
            second.close()

    def test_environment_overrides_the_quiet_policy(self):
        self.assertEqual(slots.quiet_policy({}), {})
        self.assertEqual(slots.quiet_policy({'BREENIX_QUIET': '0', 'BREENIX_QUIET_MAX_LOAD': '12',
                                             'BREENIX_QUIET_MAX_WAIT': '2700', 'BREENIX_QUIET_MAX_GAP_MS': '8'}),
                         {'required': False, 'max_load': 12.0, 'max_wait_seconds': 2700.0, 'max_gap_ms': 8.0})
        for bad in ({'BREENIX_QUIET': 'yes'}, {'BREENIX_QUIET_MAX_LOAD': '-1'}, {'BREENIX_QUIET_MAX_WAIT': 'nan'}):
            with self.assertRaises(ValueError):
                slots.quiet_policy(bad)

    def test_invocation_overrides_reach_the_wait(self):
        manifest = self.write_manifest(True)
        busy = {'QUIET_WAITS_ON_THIS_HOST': True, 'QUIET_LOAD_FRACTION': -1.0, 'QUIET_SAMPLES': 1, 'QUIET_SAMPLE_SECONDS': 0.2,
                'QUIET_MAX_WAIT_SECONDS': 60.0}
        record = self.root / 'record'
        skipped = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                              f'BREENIX_QUIET=0 {sys.executable} {HELPER} quiet {manifest}; '
                              f'cp "$BREENIX_SLOT_RECORD" {record}; echo BOOTING', settings=busy)
        self.line_matching(skipped, 'BOOTING', timeout=5)
        self.assertEqual(skipped.wait(timeout=10), 0)
        self.assertEqual(json.loads(record.read_text())['quiet'], {'required': False})
        loose = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                            f'BREENIX_QUIET_MAX_LOAD=100000 {sys.executable} {HELPER} quiet {manifest}; '
                            f'cp "$BREENIX_SLOT_RECORD" {record}; echo BOOTING', settings=busy)
        self.line_matching(loose, 'BOOTING', timeout=5)
        self.assertEqual(loose.wait(timeout=10), 0)
        quiet = json.loads(record.read_text())['quiet']
        self.assertEqual((quiet['met'], quiet['threshold']), (True, 100000.0))
        capped = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                             f'BREENIX_QUIET_MAX_WAIT=0.5 {sys.executable} {HELPER} quiet {manifest}; echo BOOTING', settings=busy)
        self.line_matching(capped, 'host NOT quiet', timeout=5)
        self.assertEqual(capped.wait(timeout=10), 0)

    def test_every_boot_records_its_start_and_verdict_before_release(self):
        serial = self.root / 'serial.log'
        serial.write_text('PROBE DONE passed=16 failed=0\n')  # An earlier boot's line.
        sidecar = serial.with_name(serial.name + '.host-slots.jsonl')
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; {sys.executable} {HELPER} serial {serial}; '
                              f'{sys.executable} {HELPER} quiet; sleep 1; '
                              f'echo "kernel::test_exec:     -> Userspace will emit PROBE DONE" >> {serial}; sleep 1; '
                              f'cp {sidecar} {self.root}/before-done; '
                              f'echo "PROBE DONE passed=16 failed=0" >> {serial}; echo SHOWN; read -r stop',
                              prelude="m.sys.platform = 'linux'\n"
                                      "def no_load_probe(seconds): raise AssertionError('boot admission ran a load probe')\n"
                                      "m.vcpu_gap_ms = no_load_probe")
        self.line_matching(session, 'SHOWN')
        before = json.loads((self.root / 'before-done').read_text())
        self.assertEqual(before['quiet'], {'required': False})
        self.assertEqual(len(before['load_at_start']), 3)
        self.assertNotIn('vcpu_gap_ms_at_start', before)
        self.assertNotIn('load_at_verdict', before)
        deadline = time.monotonic() + 10
        while 'load_at_verdict' not in sidecar.read_text() and time.monotonic() < deadline:
            time.sleep(0.1)
        record = json.loads(sidecar.read_text())
        self.assertEqual(len(record['load_at_verdict']), 3)
        self.assertNotIn('load_at_release', record)  # The launcher still shows the result.
        session.stdin.write(b'stop\n'); session.stdin.flush()
        self.assertEqual(session.wait(timeout=10), 0)
        self.assertEqual(len(json.loads(sidecar.read_text())['load_at_release']), 3)

    def test_quiet_under_an_older_supervisor_is_skipped_not_fatal(self):
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; '
                              f'BREENIX_SLOT_OPERATIONS= {sys.executable} {HELPER} quiet && echo BOOTING')
        self.line_matching(session, 'predates boot-start records')
        self.line_matching(session, 'BOOTING')
        self.assertEqual(session.wait(timeout=10), 0)

    def test_boot_without_a_start_request_records_no_start_load(self):
        record = self.root / 'record'
        session = self.launch(f'{sys.executable} {HELPER} acquire mac-boot; cp "$BREENIX_SLOT_RECORD" {record}')
        self.assertEqual(session.wait(timeout=10), 0)
        self.assertNotIn('load_at_start', json.loads(record.read_text()))

    def test_linux_quiet_needs_short_vcpu_gaps(self):
        with mock.patch.object(slots.sys, 'platform', 'linux'), \
                mock.patch.object(slots, 'vcpu_gap_ms', lambda seconds: 25.0), \
                mock.patch.object(slots.os, 'getloadavg', lambda: (0.0, 0.0, 0.0)):
            self.assertEqual(slots.host_sample({}), {'load': 0.0, 'vcpu_gap_ms': 25.0, 'quiet': False})
            self.assertTrue(slots.host_sample({'max_gap_ms': 30})['quiet'])
        with mock.patch.object(slots.sys, 'platform', 'darwin'):
            self.assertNotIn('vcpu_gap_ms', slots.host_sample({}))

    def test_vcpu_gap_sampler_reports_a_gap(self):
        if sys.platform.startswith('linux'):
            self.assertGreaterEqual(slots.vcpu_gap_ms(0.05), 0.0)
        else:
            # Without sched_setaffinity the sampler fails rather than report no gap.
            with self.assertRaises(AttributeError):
                slots.vcpu_gap_ms(0.05)

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
        slots.signal_process_group(child, signal.SIGKILL, slots.CLEANUP_PS_TIMEOUTS)
        process.stdin.write(b'reap\n'); process.stdin.flush()
        self.assertEqual(process.wait(timeout=10), 0)

    def test_cleanup_permission_failure_with_live_group_is_not_hidden(self):
        process = subprocess.Popen(['sleep', '60'], start_new_session=True,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.processes.append(process)
        with mock.patch.object(slots.os, 'killpg', side_effect=PermissionError(1, 'denied')):
            with self.assertRaises(PermissionError):
                slots.signal_process_group(process.pid, signal.SIGKILL, slots.CLEANUP_PS_TIMEOUTS)

    def test_process_table_survives_a_slow_ps(self):
        real = subprocess.check_output
        calls = []
        def slow_once(argv, **kwargs):
            calls.append(kwargs['timeout'])
            if len(calls) == 1:
                raise subprocess.TimeoutExpired(argv, kwargs['timeout'])
            return real(argv, **kwargs)
        with mock.patch.object(slots.subprocess, 'check_output', side_effect=slow_once):
            table = slots.process_table(slots.CLEANUP_PS_TIMEOUTS)
        self.assertEqual(calls, [10, 30])
        self.assertIn(os.getpid(), table)

    def slow_ps_prelude(self, log, slow_while=None):
        # A 10 s-bound ps "times out" after 0.2 s (while the flag file exists, if one is
        # given); the cleanup bounds take 2 s each, so a loop that used them would show.
        return f'''real = m.subprocess.check_output
import os as _os, time as _time
def slow(argv, **kwargs):
    if argv[0] == 'ps':
        with open({str(log)!r}, 'a') as stream:
            stream.write('%d\\n' % kwargs['timeout'])
        if kwargs['timeout'] == 10 and ({str(slow_while)!r} == 'None' or _os.path.exists({str(slow_while)!r})):
            _time.sleep(0.2)
            raise m.subprocess.TimeoutExpired(argv, kwargs['timeout'])
        if kwargs['timeout'] != 10:
            _time.sleep(2)
    return real(argv, **kwargs)
m.subprocess.check_output = slow'''

    def test_a_slow_ps_does_not_hold_the_supervisor_loop(self):
        # Every ps read bounded at 10 s times out. The loop keeps turning on that one
        # bound, forwards the interrupt to the launcher at once, and leaves the longer
        # bounds to cleanup after the launcher has exited.
        log = self.root / 'ps-timeouts'
        process = self.launch(self.request('acquire', 'mac-boot') +
                              f"; trap 'echo EXIT >> {log}; exit 0' INT; echo READY; while :; do sleep 0.1; done",
                              prelude=self.slow_ps_prelude(log))
        self.line_matching(process, 'READY')
        time.sleep(1)
        interrupted = time.monotonic()
        process.send_signal(signal.SIGINT)
        deadline = time.monotonic() + 5
        while 'EXIT' not in log.read_text().split() and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertLess(time.monotonic() - interrupted, 2)
        self.assertEqual(process.wait(timeout=20), 0)
        reads = log.read_text().split()
        in_loop = reads[:reads.index('EXIT')]
        self.assertGreater(len(in_loop), 2)
        self.assertEqual(set(in_loop), {'10'})
        self.assertIn('30', reads[reads.index('EXIT'):])

    def test_a_release_waits_out_a_slow_ps(self):
        # A release whose QEMU stop meets a slow ps is retried on later turns, not
        # raised and not served on the cleanup bounds; it completes once ps answers.
        log = self.root / 'ps-timeouts'
        flag = self.root / 'ps-slow'
        process = self.launch(self.request('acquire', 'mac-boot') + '; echo READY; read -r go; ' +
                              self.request('release', 'mac-boot') + '; echo RELEASED; read -r done',
                              prelude=self.slow_ps_prelude(log, slow_while=flag))
        self.line_matching(process, 'READY')
        flag.touch()
        process.stdin.write(b'go\n'); process.stdin.flush()
        time.sleep(1.5)
        self.assertNotIn('RELEASED', self.drain(process))
        self.assertEqual(set(log.read_text().split()), {'10'})
        flag.unlink()
        self.line_matching(process, 'RELEASED')
        process.stdin.write(b'done\n'); process.stdin.flush()
        self.assertEqual(process.wait(timeout=20), 0)

    def drain(self, process):
        collected = b''
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.select(timeout=0.1):
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    break
                collected += chunk
        return collected.decode()

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
