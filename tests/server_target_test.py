"""Host-only call parity for the server runner's target boundary."""
import copy
import json
from pathlib import Path
import runpy
import shlex
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
HOST = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))
BOOT = '20261101T120000Z-metal'
DIRECTORY = '/var/lib/breenix-metal/boots/' + BOOT


def reply(stdout='', returncode=0):
    return SimpleNamespace(returncode=returncode, stdout=stdout, stderr='')


class FakeTarget:
    def __init__(self):
        self.calls = []

    def console(self, user, password):
        self.calls.append(('console', user, password))
        return self

    def login(self):
        self.calls.append(('login',))

    def logout(self):
        self.calls.append(('logout',))

    def run(self, command, **kwargs):
        self.calls.append(('run', command, kwargs))
        return '1200 [init] bsshd started\ninit-log-read\n'

    def capture(self):
        self.calls.append(('capture',))
        return 'unrelated\n[init] fixture exited\n', [(1.0, 0), (2.0, 10)]


class ServerTarget(unittest.TestCase):
    def test_reboot_metal_operations_keep_existing_commands(self):
        calls = []
        def execute(args, **kwargs):
            calls.append((args, kwargs))
            if args[-1] == '--help': return reply('panic-reboot')
            return reply(BOOT)
        runner = dict(host_env=lambda: {}, in_metal=lambda *a, **kw: reply())
        with patch.object(HOST['runpy'], 'run_path', return_value=runner), \
             patch.object(HOST['subprocess'], 'run', side_effect=execute):
            target = HOST['MetalTarget'](BOOT)
            self.assertEqual(target.boot_start_marker, 'UEFI bootloader started')
            target.stop_for_fault('fixture', 'password')
            self.assertEqual(target.start_fault('server/arm-reboot', 'a' * 40), BOOT)
            self.assertEqual(target.start_persistent(), BOOT)
        command = ['python3', str(ROOT / 'scripts/metal/metal-run')]
        self.assertEqual(calls, [
            (command + ['boot', '--help'], dict(capture_output=True, text=True, check=True)),
            (command + ['stop', BOOT], dict(check=True)),
            (command + ['boot', '--branch', 'server/arm-reboot', '--sha', 'a' * 40,
                        '--inject', 'panic-reboot'], dict(capture_output=True, text=True, timeout=15000)),
            (command + ['boot', '--persistent'], dict(capture_output=True, text=True, timeout=15000)),
        ])

    def test_qemu_reboot_cli_dispatches_to_target_runner(self):
        argv = ['server-suite', '--target', 'qemu', '--run-dir', '/unused',
                '--boot', '20261101T120000Z-qemu', '--sha', 'a' * 40,
                '--milestone', 'server/reboot']
        called = []
        def run(api, args):
            called.append(args.target)
            return 0
        with patch.object(sys, 'argv', argv), \
             patch.object(HOST['runpy'], 'run_path', return_value={'run': run}):
            self.assertEqual(HOST['main'](), 0)
        self.assertEqual(called, ['qemu'])

    def test_qemu_reboot_scorer_requires_guest_successor_and_fault_identity(self):
        sha = 'a' * 40
        def boot(name, process='launch-one'):
            return dict(id=name, target='qemu', mode='persistent', bootedCommit=sha,
                        toolsCommit=sha, evidenceEligible=True, runId='private-run', qemuProcess=process)
        before, after = boot('before'), boot('after')
        final = boot('final', 'launch-two')
        serial = '[stdout] bsshd: host key ED25519 SHA256:fixture (loaded)\nbsshd: listening on 0.0.0.0:22\n'
        records = dict(boot=final['id'], commit=sha, milestone='server/reboot', cleanup=True,
                       cases=[dict(id='reboot/' + name, result='PASS') for name, _ in HOST['REBOOT']],
                       refusalRecovery=dict(serial='[shutdown] kernel refused transition: -13\n[shutdown] refused; resuming services'),
                       transitions=[], panicBoot=dict(id='fault', run=dict(boot('fault', 'fault-launch'), kind='fault-test'),
                           build=dict(commit=sha), serial='KERNEL PANIC: reboot fault test',
                           events=[dict(event='SHUTDOWN', data=dict(reason='guest-reset')),
                                   dict(event='panic-reset-observed')]))
        for action, old, fresh in [('reboot', before, after), ('poweroff', after, final)]:
            records['transitions'].append(dict(action=action, before=old['id'], after=fresh['id'], exit='0',
                beforeRun=old, afterRun=fresh,
                beforeSerial='[shutdown] home clean read-only\n[shutdown] root clean read-only',
                afterSerial=serial + '[storage] root prior state=clean\n[storage] home prior state=clean',
                events=[dict(event='requested-shutdown', action=action),
                        dict(event='RESET' if action == 'reboot' else 'SHUTDOWN',
                             data=dict(reason='guest-reset' if action == 'reboot' else 'guest-shutdown')),
                        dict(event='requested-reset-observed', successor=fresh['id'])]))
        manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        def score(value): return HOST['score'](manifest, path, value, serial, final, sha, [])
        self.assertTrue(score(records).startswith('PASS: server/reboot'))
        mutations = {
            'host reset': lambda r: r['transitions'][0]['events'][1]['data'].update(reason='host-qmp-system-reset'),
            'non-successor': lambda r: r['transitions'][0].update(after='other'),
            'missing receipt': lambda r: r['transitions'][0]['events'].pop(0),
            'missing poweroff receipt': lambda r: r['transitions'][1]['events'].pop(0),
            'wrong-commit fault': lambda r: r['panicBoot']['run'].update(bootedCommit='b' * 40),
            'other run': lambda r: r['transitions'][0]['afterRun'].update(runId='another-run'),
            'other process': lambda r: r['transitions'][0]['afterRun'].update(qemuProcess='another-launch'),
            'missing process': lambda r: r['transitions'][0]['beforeRun'].pop('qemuProcess'),
            'no SSH': lambda r: r['transitions'][0].update(afterSerial='[storage] root prior state=clean\n[storage] home prior state=clean'),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                bad = copy.deepcopy(records)
                mutate(bad)
                with self.assertRaises(ValueError): score(bad)

    def test_qemu_reboot_target_lifecycle_without_vm(self):
        with tempfile.TemporaryDirectory(dir='/tmp') as scratch:
            run = Path(scratch)
            (run / 'current').write_text('old')
            (run / 'mode').write_text('console')
            (run / 'build.json').write_text(json.dumps(dict(commit='a' * 40)))
            target = HOST['QemuTarget'](run, 'old')
            calls = []
            def launch(args, **kwargs):
                calls.append(args)
                (run / 'current').write_text('new-' + str(len(calls)))
                return SimpleNamespace(poll=lambda: None)
            with patch.object(HOST['subprocess'], 'Popen', side_effect=launch):
                self.assertEqual(target.start_persistent(), 'new-1')
                self.assertEqual(target.start_fault(None, 'a' * 40), 'new-2')
                with self.assertRaisesRegex(ValueError, 'this commit'):
                    target.start_fault(None, 'b' * 40)
            launcher = str(ROOT / 'scripts/vm-persistent.sh')
            self.assertEqual(calls, [
                [launcher, 'qemu', 'boot', '--run-dir', str(run), '--mode', 'console'],
                [launcher, 'qemu', 'fault-boot', '--run-dir', str(run)],
            ])
            console = unittest.mock.Mock()
            with patch.object(target, 'console', return_value=console):
                target.stop_for_fault('fixture', 'password')
                self.assertEqual(console.method_calls, [unittest.mock.call.login(),
                                                       unittest.mock.call.type_line('/sbin/poweroff')])
            self.assertEqual(target.boot_start_marker, 'Breenix ARM64 Kernel Starting')

    def test_qemu_target_waits_for_the_named_successor_at_this_commit(self):
        sha = 'a' * 40
        with tempfile.TemporaryDirectory(dir='/tmp') as scratch:
            run = Path(scratch)
            for name in ('old', 'next'):
                directory = run / 'boots' / name
                directory.mkdir(parents=True)
                (directory / 'run.json').write_text(json.dumps(dict(
                    bootedCommit=sha, toolsCommit=sha, runId='run', qemuProcess='launch')))
                (directory / 'serial.txt').write_text('')
            old, fresh = run / 'boots/old', run / 'boots/next'
            (old / 'events.jsonl').write_text(json.dumps(dict(event='requested-reset-observed', successor='next')) + '\n')
            (old / 'ended').touch()
            target = HOST['QemuTarget'](run, 'old')
            self.assertIsNone(target.reboot_successor(sha))
            (fresh / 'serial.txt').write_text('bsshd: listening on 0.0.0.0:22')
            self.assertEqual(target.reboot_successor(sha), 'next')
            with self.assertRaisesRegex(ValueError, 'different run, QEMU process or commit'):
                target.reboot_successor('b' * 40)
            (old / 'events.jsonl').write_text('')
            with self.assertRaisesRegex(ValueError, 'no authenticated guest reset'):
                target.reboot_successor(sha)

    def test_qemu_reboot_files_cases_and_judge_exit_in_vigil(self):
        helper = runpy.run_path(str(ROOT / 'scripts/metal/server_reboot.py'))
        with tempfile.TemporaryDirectory() as scratch:
            run = Path(scratch)
            (run / 'started').write_text('123.25')
            (run / 'qemu-profile').write_text('fixture-profile')
            records = dict(cases=[dict(id='reboot/' + name, result='PASS', ms=1, reason='')
                                  for name, _ in HOST['REBOOT']])
            args = SimpleNamespace(run_dir=run, sha='a' * 40)
            with patch.object(helper['subprocess'], 'run', return_value=reply()) as execute:
                helper['file_vigil'](HOST, args, SimpleNamespace(directory=run), records, 'FAIL: judge refused', 6)
            command = execute.call_args.args[0]
            self.assertEqual(command[1:5], ['record', 'qemu', 'persistent', 'server'])
            self.assertEqual(command[-4], args.sha)
            self.assertEqual(command[-3], '123.25')
            self.assertEqual(command[-1], '1')
            self.assertEqual(execute.call_args.kwargs['env']['BREENIX_VIGIL_ARCH'], 'aarch64')
            lines = (run / 'reboot-suite.txt').read_text().splitlines()
            self.assertEqual(len(lines), 8)
            self.assertEqual(lines[-1], 'SUITE server DONE passed=6 failed=0 skipped=0 total=6')

    def test_rtc_reboot_requires_the_boot_architecture_hardware_source(self):
        previous = {'corrected': True, 'skewSeconds': 172800}
        for arch, source, accepted in (
            ('x86_64', 'CMOS', True), ('aarch64', 'PL031', True),
            ('x86_64', 'PL031', False), ('aarch64', 'CMOS', False),
            ('x86_64', 'other', False), ('aarch64', 'other', False),
            ('unknown', 'CMOS', False),
        ):
            with self.subTest(arch=arch, source=source):
                serial = '[clock] UTC epoch=1000 source=' + source + '\n'
                case = HOST['TimeCases'](None, None, previous, lambda: (serial, [(1000, 0)]),
                                         {'BOOT_URL': 'http://fixture.invalid'}, arch)
                if accepted:
                    case.run_case('rtc-reboot')
                else:
                    with self.assertRaises(ValueError):
                        case.run_case('rtc-reboot')

    def test_check_manifest(self):
        for suite in ('server', 'users'):
            with self.subTest(suite=suite):
                result = subprocess.run([sys.executable, str(ROOT / 'scripts/metal/server-suite'),
                                         '--check-manifest', str(ROOT / 'docs/suites' / (suite + '.json'))],
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_probe_and_services_call_sequence(self):
        # These commands and options are the metal path before the extraction.
        target = FakeTarget()
        required = HOST['Services'].REQUIRED
        state = 'supervisor began=100 entropy=200 network-up=300\n' + ''.join(
            'service ' + name + ' state=running pid=10 starts=1 first=400 since=400 next=0 quick-failures=0\n'
            for name in required)
        init_command = 'sh -c ' + shlex.quote('cat /var/log/init.log; printf "init-log-%s\\n" read')

        def command(args, **kwargs):
            target.calls.append(('command', args, kwargs))
            if args[0] == 'ssh-keyscan':
                return reply('guest.invalid ssh-ed25519 fixture\n')
            if args[:2] == ['ssh-keygen', '-E']:
                return reply('256 SHA256:fixture guest.invalid (ED25519)\n')
            if args[0] == 'ssh':
                script = args[-1]
                if script == "sh -c 'cat /etc/breenix/services'":
                    return reply(''.join('[' + name + ']\n' for name in required))
                if script == "sh -c 'ls -ln /run/breenix/services /run/breenix/svc.sock'":
                    return reply('-rw-r--r-- 1 0 0 1 /run/breenix/services\nsrw-rw-rw- 1 0 0 0 /run/breenix/svc.sock\n')
                if script == "sh -c 'cat /run/breenix/services'":
                    return reply(state)
                if script == "sh -c 'svc status'":
                    return reply(''.join(name + ' running pid=10\n' for name in required))
                if args[args.index('-c') + 1] == 'aes256-gcm@openssh.com':
                    return reply('cipher-ok\n')
                self.fail('unexpected SSH command: ' + script)
            if args[0] != 'ssh-keygen':
                self.fail('unexpected host command: ' + repr(args))
            return reply()

        with tempfile.TemporaryDirectory() as scratch, patch.dict(HOST['Probe'].__init__.__globals__, command=command):
            directory = Path(scratch)
            probe = HOST['Probe']('guest.invalid', 'fixture-user', directory / 'identity', directory)
            console = target.console('fixture-root', 'fixture-password')
            console.login()
            probe.run_case('host-key', '[stdout] bsshd: host key ED25519 SHA256:fixture (loaded)\n')
            self.assertEqual(probe.known.read_text(), 'guest.invalid ssh-ed25519 fixture\n')
            probe.run_case('aes256-gcm', '')
            services = HOST['Services'](probe, console, target.capture)
            for name in ('table', 'entropy-first', 'network-first'):
                services.run_case(name)
            self.assertEqual(services.init_records(), [(1.2, '[init] bsshd started')])
            self.assertEqual(services.lines(10, 'fixture exited'), [(2.0, '[init] fixture exited')])
            services.cleanup()
            probe.cleanup()
            common = ['ssh', '-F', '/dev/null', '-p', '22', '-o', 'ConnectTimeout=10',
                      '-o', 'ConnectionAttempts=1', '-o', 'StrictHostKeyChecking=yes',
                      '-o', 'UserKnownHostsFile=' + str(directory / 'known_hosts'), '-o', 'GlobalKnownHostsFile=/dev/null',
                      '-o', 'HostKeyAlgorithms=ssh-ed25519', '-o', 'IdentitiesOnly=yes',
                      '-o', 'IdentityAgent=none', '-o', 'BatchMode=yes', '-i', str(directory / 'identity')]
            ssh_kwargs = dict(input=None, env=None, timeout=45, text=True)
            def ssh(script):
                return ('command', common + ['fixture-user@guest.invalid', 'sh -c ' + shlex.quote(script)], ssh_kwargs)
            self.assertEqual(target.calls, [
                *[('command', ['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C',
                               'breenix-server-suite-fixture', '-f', str(directory / name)], {})
                  for name in ('ed25519', 'wrong')],
                ('console', 'fixture-root', 'fixture-password'), ('login',),
                ('command', ['ssh-keyscan', '-T', '10', '-p', '22', '-t', 'ed25519', 'guest.invalid'], {}),
                ('command', ['ssh-keygen', '-E', 'sha256', '-lf', '-'], dict(input='guest.invalid ssh-ed25519 fixture\n')),
                ('command', common + ['-c', 'aes256-gcm@openssh.com', 'fixture-user@guest.invalid',
                                     'sh -c ' + shlex.quote("printf 'cipher-ok\\n'")], ssh_kwargs),
                ssh('cat /etc/breenix/services'), ssh('ls -ln /run/breenix/services /run/breenix/svc.sock'),
                ssh('cat /run/breenix/services'), ssh('svc status'),
                ssh('cat /run/breenix/services'), ssh('cat /run/breenix/services'),
                ('run', init_command, dict(timeout=60)), ('capture',), ('logout',),
            ])

    def test_qemu_target_local_run_and_forward(self):
        import socket
        import threading
        with tempfile.TemporaryDirectory(dir='/tmp') as scratch:
            run = Path(scratch)
            boot = '20261101T120000Z-qemu'
            directory = run / 'boots' / boot
            directory.mkdir(parents=True)
            (run / 'boot.lock').mkdir()
            (run / 'current').write_text(boot + '\n')
            (directory / 'run.json').write_text(json.dumps({'mode': 'persistent'}))
            (directory / 'serial.txt').write_text('captured\n')
            (directory / 'serial.ts').write_text('1.5\t0\n')
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(str(run / 'console.sock'))
            listener.listen()
            received = []
            def accept():
                conn, _ = listener.accept()
                with conn: received.append(conn.recv(100))
            thread = threading.Thread(target=accept, daemon=True)
            thread.start()
            target = HOST['QemuTarget'](run, boot)
            with patch.object(target, '_qemu_alive', return_value=True), patch.object(HOST['time'], 'sleep'):
                self.assertTrue(target.holds_lease())
                with patch.object(target, '_qemu_alive', return_value=False):
                    self.assertFalse(target.holds_lease())
                (run / 'boot.lock').rmdir()
                self.assertFalse(target.holds_lease())
                (run / 'boot.lock').mkdir()
                (run / 'current').write_text('another-boot\n')
                self.assertFalse(target.holds_lease())
                (run / 'current').write_text(boot + '\n')
                self.assertEqual(target.identity()['mode'], 'persistent')
                self.assertEqual(target.capture(), ('captured\n', [(1.5, 0)]))
                self.assertEqual(target.write('result.json', {'ok': True}).returncode, 0)
                console = target.console('fixture', 'password')
                self.assertEqual(console.size(), 9)
                self.assertEqual(console.since(0), 'captured\n')
                console.send('ok')
                (directory / 'ended').touch()
                self.assertFalse(target.holds_lease())
            thread.join(timeout=2)
            listener.close()
            self.assertEqual(received, [b'ok\r'])
            calls = []
            def fake_command(args, **kwargs):
                calls.append(args)
                return reply()
            with patch.dict(HOST['Probe'].__init__.__globals__, command=fake_command):
                probe = HOST['Probe']('127.0.0.1', 'fixture', run / 'identity', run, port=2222)
                probe.ssh('true')
                self.assertEqual(calls[-1][calls[-1].index('-p') + 1], '2222')
                services = HOST['Services'](probe, console, target.capture)
                self.assertEqual(services.lines(0, 'captured'), [(1.5, 'captured')])

    def test_qemu_cli_reaches_forwarded_probe_and_refuses_ended_boot(self):
        with tempfile.TemporaryDirectory(dir='/tmp') as scratch:
            run = Path(scratch)
            boot, sha = '20261101T120000Z-qemu', 'a' * 40
            directory = run / 'boots' / boot
            directory.mkdir(parents=True)
            (run / 'current').write_text(boot + '\n')
            (run / 'boot.lock').mkdir()
            (directory / 'run.json').write_text(json.dumps(dict(
                id=boot, mode='persistent', bootedCommit=sha, toolsCommit=sha, evidenceEligible=True)))
            (directory / 'serial.txt').write_text('fixture\n')
            for name in ('docs/suites/server.json', 'docs/efforts/path.json', 'docs/boot-path.json'):
                path = directory / 'definition' / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes((ROOT / name).read_bytes())
            (run / '.ssh').mkdir()
            (run / '.ssh/fixture.pub').write_text('ssh-ed25519 fixture')
            (run / '.ssh/fixture').touch()
            site = dict(SERVER_SSH_HOST='127.0.0.1', SERVER_SSH_PORT=2222,
                        OPERATOR_USER='fixture', OPERATOR_SSH_PUBLIC_KEY='ssh-ed25519 fixture')
            argv = ['server-suite', '--target', 'qemu', '--run-dir', str(run),
                    '--boot', boot, '--sha', sha, '--milestone', 'server/ssh-keys']
            with patch.object(sys, 'argv', argv), patch.object(Path, 'home', return_value=run), \
                 patch.object(HOST['QemuTarget'], '_qemu_alive', return_value=True), \
                 patch.object(HOST['QemuTarget'], 'site', return_value=site), \
                 patch.dict(HOST['main'].__globals__, Probe=unittest.mock.Mock(side_effect=RuntimeError('probe reached'))):
                with self.assertRaisesRegex(RuntimeError, 'probe reached'):
                    HOST['main']()
                self.assertEqual(HOST['main'].__globals__['Probe'].call_args.kwargs.get('port'), 2222)
                (directory / 'ended').touch()
                with self.assertRaisesRegex(ValueError, 'does not hold the machine lease'):
                    HOST['main']()

    def test_qemu_id_requires_qemu_and_run_directory(self):
        for extra in ([], ['--target', 'qemu']):
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/metal/server-suite'),
                                     '--boot', '20261101T120000Z-qemu', '--sha', 'a' * 40, *extra],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_qemu_does_not_dispatch_later_milestones_to_metal(self):
        import contextlib
        import io
        for milestone in ('server/watchdog', 'server/remote-upgrade',
                          'server/resting-state', 'users/operator-login'):
            argv = ['server-suite', '--target', 'qemu', '--run-dir', '/unused',
                    '--boot', '20261101T120000Z-qemu', '--sha', 'a' * 40,
                    '--milestone', milestone, '--branch', 'server/vm-runner']
            with self.subTest(milestone=milestone), patch.object(sys, 'argv', argv), \
                 patch.object(HOST['runpy'], 'run_path', side_effect=AssertionError('metal helper reached')), \
                 contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    HOST['main']()
                self.assertEqual(error.exception.code, 2)

    def test_forwarded_probe_checks_banner_and_pins_wrong_key_to_port(self):
        import socket
        with tempfile.TemporaryDirectory() as scratch:
            run = Path(scratch)
            def command(args, **kwargs):
                if args[0] == 'ssh' and args[-1] == "sh -c true":
                    self.assertEqual((run / 'known_hosts').read_text(),
                                     '[127.0.0.1]:2222 ssh-ed25519 wrong\n')
                    return SimpleNamespace(returncode=255, stdout='', stderr='Host key verification failed')
                if args[0] == 'ssh':
                    return reply('1000\n1000\nfixture\n/home/fixture\n/home/fixture\n')
                if args[0] == 'ssh-keyscan':
                    self.assertEqual(args[args.index('-p') + 1], '2222')
                    return reply('[127.0.0.1]:2222 ssh-ed25519 fixture\n')
                if args[:2] == ['ssh-keygen', '-E']:
                    return reply('256 SHA256:fixture fixture (ED25519)\n')
                return reply()
            with patch.dict(HOST['Probe'].__init__.__globals__, command=command), \
                 patch.object(socket, 'create_connection') as connect:
                stream = connect.return_value.__enter__.return_value
                stream.recv.return_value = b'SSH-2.0-fixture\r\n'
                probe = HOST['Probe']('127.0.0.1', 'fixture', run / 'identity', run, port=2222)
                probe.run_case('port-22', '')
                connect.assert_called_once_with(('127.0.0.1', 2222), timeout=10)
                probe.wrong.with_suffix('.pub').write_text('ssh-ed25519 wrong\n')
                probe.run_case('strict-login', '[stdout] bsshd: host key ED25519 SHA256:fixture (loaded)\n')

    def test_vigil_filing_for_scored_qemu_categories_only(self):
        import contextlib
        import io
        import os
        with tempfile.TemporaryDirectory(dir='/tmp') as scratch:
            run = Path(scratch)
            boot, sha = '20261101T120000Z-qemu', 'a' * 40
            directory = run / 'boots' / boot
            directory.mkdir(parents=True)
            (directory / 'started').write_text('1793534400.125\n')
            (directory / 'serial.txt').write_text('boot serial\n')
            (run / 'qemu-profile').write_text('fixture-profile\n')
            cli = run / 'vigil-cli'
            calls = run / 'calls'
            cli.write_text('#!/usr/bin/env python3\nimport json,os,sys\n'
                           'if len(sys.argv) == 2: print("breenix start"); sys.exit()\n'
                           'with open(os.environ["CALLS"], "a") as f: f.write(json.dumps(sys.argv[1:]) + "\\n")\n')
            cli.chmod(0o755)
            (run / '.ssh').mkdir()
            (run / '.ssh/key.pub').write_text('ssh-ed25519 fixture')
            (run / '.ssh/key').touch()
            site = dict(SERVER_SSH_HOST='127.0.0.1', OPERATOR_USER='fixture',
                        OPERATOR_SSH_PUBLIC_KEY='ssh-ed25519 fixture', CONSOLE_USER='fixture',
                        CONSOLE_PASSWORD='fixture')
            class Machine:
                def __init__(self, *args): self.directory = directory
                def identity(self): return dict(mode='persistent', bootedCommit=sha, toolsCommit=sha, evidenceEligible=True)
                def holds_lease(self): return True
                def read(self, name, **kwargs):
                    if name.startswith('definition/'): return (ROOT / name.removeprefix('definition/')).read_text()
                    if name == 'events.jsonl': return ''
                    return (directory / name).read_text()
                def site(self, fields): return site
                def console(self, *args): return Stub()
                def capture(self): return ('', [])
                def write(self, name, value): return reply()
            class Stub:
                transcript = []
                init_log = ''
                def __init__(self, *args, **kwargs): pass
                def service_log(self, *args): return ''
                def run_case(self, *args): pass
                def prepare(self): return {}
                def cleanup(self): pass
                def logout(self): pass
            env = dict(VIGIL_CLI=str(cli), CALLS=str(calls), VIGIL_BOOT_ID='', VIGIL_TEST_HOME=str(run))
            globals_ = HOST['main'].__globals__
            replacements = dict(QemuTarget=Machine, MetalTarget=Machine, Probe=Stub, ConsoleInput=Stub,
                                Services=Stub, Logs=Stub, TimeCases=Stub, score=lambda *a: 'PASS',
                                S27_SUSPENDS_PERSISTENT=False)
            with patch.dict(os.environ, env), patch.dict(globals_, replacements), \
                 patch.object(Path, 'home', return_value=run), contextlib.redirect_stdout(io.StringIO()):
                for category in ('ssh-keys', 'services', 'logs', 'time', 'console-input'):
                    argv = ['server-suite', '--target', 'qemu', '--run-dir', str(run), '--boot', boot,
                            '--sha', sha, '--milestone', 'server/' + category]
                    with patch.object(sys, 'argv', argv): self.assertEqual(HOST['main'](), 0)
                    rows = [json.loads(row) for row in calls.read_text().splitlines()]
                    args = rows[-1]
                    self.assertEqual(len(rows), ('ssh-keys', 'services', 'logs', 'time', 'console-input').index(category) + 1)
                    def arg(name): return args[args.index(name) + 1]
                    self.assertEqual((arg('--arch'), arg('--platform'), arg('--suite'), arg('--profile'),
                                      arg('--commit'), arg('--started')), ('aarch64', 'qemu', 'server',
                                      'fixture-profile', sha, '1793534400.125'))
                    self.assertEqual(arg('--serial'), str(directory / (category + '-suite.txt')))
                    self.assertEqual(arg('--serial-user'), str(directory / 'serial.txt'))
                    self.assertNotEqual(arg('--id'), boot)
                    self.assertEqual(args[:2], ['breenix', 'record'])
                    self.assertRegex(arg('--started'), r'^\d+(?:\.\d+)?$')
                    self.assertRegex(arg('--ended'), r'^\d+(?:\.\d+)?$')
                    self.assertGreaterEqual(float(arg('--ended')), float(arg('--started')))
                    self.assertEqual(arg('--exit-status'), '0')
                    lines = Path(arg('--serial')).read_text().splitlines()
                    self.assertEqual(lines[0], 'SUITE server START cases=' + str(len(HOST['CATEGORIES'][category])))
                    self.assertEqual(len(lines), len(HOST['CATEGORIES'][category]) + 2)
                    self.assertTrue(all(line.startswith('SUITE server CASE ' + category + '/') for line in lines[1:-1]))
                    self.assertTrue(lines[-1].startswith('SUITE server DONE '))
                ids = [row[row.index('--id') + 1] for row in rows]
                self.assertEqual(len(set(ids)), len(ids))
                for category, flag in (('logs', '--prepare-logs'), ('time', '--prepare-time')):
                    argv = ['server-suite', '--target', 'qemu', '--run-dir', str(run), '--boot', boot,
                            '--sha', sha, '--milestone', 'server/' + category, flag]
                    with patch.object(sys, 'argv', argv): self.assertEqual(HOST['main'](), 0)
                argv = ['server-suite', '--boot', '20261101T120000Z-metal', '--sha', sha,
                        '--milestone', 'server/console-input']
                with patch.object(sys, 'argv', argv): self.assertEqual(HOST['main'](), 0)
                self.assertEqual(len(calls.read_text().splitlines()), 5)

    def test_metal_target_keeps_boot_store_and_console_calls(self):
        calls = []
        identity = dict(mode='persistent', bootedCommit='a' * 40, toolsCommit='a' * 40, evidenceEligible=True)
        artifacts = {'run.json': json.dumps(identity), 'serial.txt': 'captured\n', 'serial.ts': '1.5\t0\n'}
        def in_metal(env, *args, **kwargs):
            calls.append((env, args, kwargs))
            if args == ('cat', '/var/lib/breenix-metal/lease.json'):
                return reply(json.dumps(dict(id=BOOT)))
            if args[0] == 'cat':
                return reply(artifacts[args[1].removeprefix(DIRECTORY + '/')])
            if args[0] == 'stat': return reply('9')
            if args[0] == 'tail': return reply(b'captured\n')
            return reply()
        env = dict(fixture='host-environment')
        runner = dict(host_env=lambda: env, in_metal=in_metal)
        with patch.object(HOST['runpy'], 'run_path', return_value=runner), patch.object(HOST['time'], 'sleep'):
            target = HOST['MetalTarget'](BOOT)
            self.assertEqual(target.identity(), identity)
            self.assertTrue(target.holds_lease())
            self.assertEqual(target.capture(), ('captured\n', [(1.5, 0)]))
            self.assertEqual(target.write('server-services.json', dict(cleanup=True)).returncode, 0)
            console = target.console('fixture-root', 'fixture-password')
            self.assertEqual(console.size(), 9)
            self.assertEqual(console.since(0), 'captured\n')
            console.send('ok')
        text = dict(capture_output=True, text=True)
        tmux = 'tmux -L metal-' + BOOT
        self.assertEqual(calls, [
            (env, ('cat', DIRECTORY + '/run.json'), text),
            (env, ('cat', '/var/lib/breenix-metal/lease.json'), text),
            (env, ('cat', DIRECTORY + '/serial.ts'), text),
            (env, ('cat', DIRECTORY + '/serial.txt'), text),
            (env, ('tee', DIRECTORY + '/server-services.json'), dict(stdin=None, input='{"cleanup": true}\n', **text)),
            (env, ('stat', '-c', '%s', DIRECTORY + '/serial.txt'), text),
            (env, ('tail', '-c', '+1', DIRECTORY + '/serial.txt'), dict(capture_output=True)),
            (env, ('sh', '-c', tmux + ' load-buffer - && ' + tmux + ' paste-buffer -d -t sol'),
             dict(stdin=None, input=b'ok\r', capture_output=True)),
        ])


if __name__ == '__main__':
    unittest.main()
