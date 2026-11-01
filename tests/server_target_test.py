"""Host-only call parity for the server runner's target boundary."""
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
        with tempfile.TemporaryDirectory() as scratch:
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
