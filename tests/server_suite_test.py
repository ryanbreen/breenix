"""The host server scorer refuses incomplete or mismatched machine evidence."""
import copy
import json
from pathlib import Path
import runpy
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
host = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))


class HostServerScore(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        self.path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        self.sha = 'a' * 40
        self.boot = dict(id='20261021T000000Z-metal', mode='persistent', bootedCommit=self.sha, toolsCommit=self.sha, evidenceEligible=True)
        self.records = dict(boot=self.boot['id'], commit=self.sha, milestone='server/ssh-keys', cleanup=True,
                            cases=[dict(id='ssh-keys/' + name, result='PASS') for name, _ in host['CATEGORIES']['ssh-keys']])
        self.serial = "[stdout] bsshd: host key ED25519 SHA256:example (loaded)\nbsshd: refused method 'password'\n"

    def test_complete_host_evidence_and_rejections(self):
        def score():
            return host['score'](self.manifest, self.path, self.records, self.serial, self.boot, self.sha, ['KERNEL PANIC:'])
        self.assertTrue(score().startswith('PASS: server/ssh-keys'))
        good = copy.deepcopy(self.records)
        for bad in [[], good['cases'][:-1], good['cases'] + good['cases'][:1], list(reversed(good['cases']))]:
            self.records['cases'] = bad
            with self.assertRaises(ValueError): score()
        self.records = copy.deepcopy(good)
        self.records['cases'][0]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score()
        self.records = copy.deepcopy(good)
        self.records['cleanup'] = False
        with self.assertRaises(ValueError): score()
        self.records = good
        self.boot['bootedCommit'] = 'b' * 40
        with self.assertRaises(ValueError): score()
        self.boot['bootedCommit'] = self.sha
        self.boot['toolsCommit'] = 'b' * 40
        with self.assertRaises(ValueError): score()
        self.boot['toolsCommit'] = self.sha
        serial = self.serial
        self.serial = '[stdout] bsshd: host key ED25519 SHA256:example (loaded)\n'
        with self.assertRaises(ValueError): score()
        self.serial = serial + 'KERNEL PANIC: example\n'
        with self.assertRaises(ValueError): score()

    def test_host_key_rejects_embedded_fingerprint_and_requires_complete_record(self):
        with tempfile.TemporaryDirectory() as scratch:
            probe = object.__new__(host['Probe'])
            probe.host = 'fixture'
            probe.known = Path(scratch) / 'known_hosts'
            replies = [SimpleNamespace(returncode=0, stdout='fixture ssh-ed25519 key\n'),
                       SimpleNamespace(returncode=0, stdout='256 SHA256:forged fixture (ED25519)\n')]
            forged = "[stdout] bsshd: refused method 'password' for 'bsshd: host key ED25519 SHA256:forged'\n"
            with patch.dict(host['Probe'].run_case.__globals__, command=lambda *a, **kw: replies.pop(0)):
                with self.assertRaisesRegex(ValueError, 'fingerprint differs'):
                    probe.run_case('host-key', forged)
            self.assertFalse(probe.known.exists())
            replies[:] = [SimpleNamespace(returncode=0, stdout='fixture ssh-ed25519 key\n'),
                          SimpleNamespace(returncode=0, stdout='256 SHA256:trusted fixture (ED25519)\n')]
            with patch.dict(host['Probe'].run_case.__globals__, command=lambda *a, **kw: replies.pop(0)):
                probe.run_case('host-key', forged + '[stdout] bsshd: host key ED25519 SHA256:trusted (loaded)\n')
            self.assertEqual(probe.known.read_text(), 'fixture ssh-ed25519 key\n')

    def test_logs_require_all_cases_and_prior_boot_at_same_commit(self):
        records = dict(self.records, milestone='server/logs',
                       cases=[dict(id='logs/' + name, result='PASS') for name, _ in host['LOGS']])
        def score():
            return host['score'](self.manifest, self.path, records, self.serial, self.boot, self.sha, [])
        with self.assertRaises(ValueError): score()
        records['previous'] = dict(commit='b' * 40)
        with self.assertRaises(ValueError): score()
        records['previous'] = dict(commit=self.sha)
        self.assertTrue(score().startswith('PASS: server/logs'))
        records['cases'][-1]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score()

    def test_cleanup_restores_exact_keys_without_guest_mktemp(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            ssh = root / '.ssh'
            ssh.mkdir()
            target = ssh / 'authorized_keys'
            original = '# retained comment\nprimary key\nsecond key without final newline'
            target.write_text(original + '\ntemporary probe key\n')
            binaries = root / 'bin'
            binaries.mkdir()
            for name in ('cat', 'mv'):
                (binaries / name).symlink_to(shutil.which(name))
            probe = object.__new__(host['Probe'])
            probe.original = original
            probe.known_guest = None
            probe.ssh = lambda script: subprocess.run(
                ['/bin/sh', '-c', script], capture_output=True, text=True,
                env=dict(os.environ, HOME=str(root), PATH=str(binaries)))
            probe.cleanup()
            self.assertEqual(target.read_text(), original)
            self.assertEqual(target.stat().st_mode & 0o777, 0o600)
            self.assertEqual(list(ssh.iterdir()), [target])

    def test_temporary_key_leftover_is_removed_by_marker_only(self):
        mark = host['FIXTURE_MARK']
        quote = host['shlex'].quote
        kept = 'primary key\nkey with ' + mark + ' inside its comment\n'
        for stored, original, appended in [
                (kept + 'ssh-ed25519 OLD ' + mark + '\n', kept, 'ssh-ed25519 NEW ' + mark + '\n'),
                ('last key', 'last key', '\nssh-ed25519 NEW ' + mark + '\n')]:
            scripts = []
            probe = object.__new__(host['Probe'])
            probe.edkey = Path('/nonexistent/ed25519')
            def ssh(script, identity=None):
                scripts.append(script)
                if script == 'cat "$HOME/.ssh/authorized_keys"':
                    return SimpleNamespace(returncode=0, stdout=stored)
                return SimpleNamespace(returncode=0, stdout='1000\n' if identity else '')
            probe.ssh = ssh
            with patch.object(Path, 'read_text', lambda self: 'ssh-ed25519 NEW ' + mark + '\n'):
                probe.run_case('ed25519-user', '')
            self.assertEqual(probe.original, original)
            rewrites = [script for script in scripts if '> "$tmp"' in script]
            self.assertEqual(len(rewrites), 0 if stored == original else 1)
            if rewrites: self.assertIn('printf %s ' + quote(original) + ' > "$tmp"', rewrites[0])
            self.assertIn('printf %s ' + quote(appended) + ' >> "$HOME/.ssh/authorized_keys"', scripts)

    def test_console_waits_for_slow_redraws_before_sending_more_or_enter(self):
        # A slow console finishes a redraw seven seconds after the sender
        # pauses. A bounded receive queue loses anything sent beyond it.
        console = object.__new__(host['Console'])
        clock, pending, visible, capture, entered = [0.0], [b''], [b''], [''], []
        due = [None]
        def sleep(seconds):
            clock[0] += seconds
            if due[0] is not None and clock[0] >= due[0]:
                visible[0] += pending[0]
                pending[0] = b''
                capture[0] += '\nbsh root> ' + visible[0].decode()
                due[0] = None
        def send(text, enter=True):
            if text == '\x15':
                visible[0] = pending[0] = b''
                capture[0] += '\nbsh root> '
                return
            pending[0] += text.encode()[:max(0, 32 - len(pending[0]))]
            if text: due[0] = clock[0] + 7
            if enter:
                entered.append(visible[0])
        console.send = send
        console.size = lambda: len(capture[0])
        console.since = lambda offset: capture[0][offset:]
        text = 'printf ' + 'x' * 673
        with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]), \
             patch.object(host['time'], 'sleep', side_effect=sleep):
            console.type_line(text)
        self.assertEqual(entered, [text.encode()])
        self.assertEqual(pending[0], b'')

    def test_console_echo_and_clear_allow_async_init_output(self):
        console = object.__new__(host['Console'])
        clock, capture, cleared, entered = [0.0], [''], [False], []
        def send(text, enter=True):
            if text == '\x15':
                cleared[0] = True
                capture[0] += '\nbsh root> [init] fixture exited (code 0)\n'
            elif enter:
                entered.append(text)
            elif cleared[0]:
                capture[0] += '\nbsh root> ' + text + '[init] fixture exited (code 0)\n'
        console.send = send
        console.size = lambda: len(capture[0])
        console.since = lambda offset: capture[0][offset:]
        def sleep(seconds):
            clock[0] += seconds
        with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]), \
             patch.object(host['time'], 'sleep', side_effect=sleep):
            console.type_line('printf ok', attempts=2)
        self.assertTrue(cleared[0])
        self.assertEqual(entered, [''])

    def test_console_refuses_oversized_or_editing_input_before_typing(self):
        console = object.__new__(host['Console'])
        console.send = lambda *a, **kw: self.fail('invalid line reached the console')
        console.size = lambda: 0
        for text in ('x' * 4096, 'echo a\necho b', 'echo \x15', 'é' * 2048):
            with self.subTest(text=text[:20]), self.assertRaises(ValueError):
                console.type_line(text)

    def test_console_collects_status_without_typing_into_a_running_command(self):
        console = object.__new__(host['Console'])
        console.step = 0
        console.login = lambda: None
        console.size = lambda: 0
        typed = []
        console.type_line = typed.append
        output = 'child output\nsvc-suite-step-1 3\nbsh root> '
        import re
        console.wait = lambda offset, pattern, what, timeout: (output, re.search(pattern, output))
        with self.assertRaisesRegex(ValueError, 'exited 3'):
            console.run("sh -c 'exit 3'", done=r'(?m)^logs-check-ok$')
        self.assertEqual(len(typed), 1)

    def test_console_recognizes_prompt_followed_by_async_service_output(self):
        # A prompt has no newline; init can write directly after it on SOL.
        console = object.__new__(host['Console'])
        console.step = 0
        console.login = lambda: None
        console.size = lambda: 0
        console.type_line = lambda text: None
        capture = 'child output\nsvc-suite-step-1 0\nbsh root> [init] logs-rotate exited (code 0)\n'
        console.since = lambda offset: capture[offset:]
        clock = [0.0]
        def sleep(seconds):
            clock[0] += seconds
        with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]), \
             patch.object(host['time'], 'sleep', side_effect=sleep):
            self.assertEqual(console.run('svc reload', timeout=1), 'child output\n')

    def test_kernel_gap_requires_this_boot_nonce_and_exact_cursor_across_rotation(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            bindir = root / 'bin'
            bindir.mkdir()
            svc = bindir / 'svc'
            svc.write_text('#!/bin/sh\necho "$*" >> "' + str(root / 'controls') + '"\n')
            svc.chmod(0o755)
            logs = object.__new__(host['Logs'])
            logs.boot_id = '1-2-3'
            def run(command, **kwargs):
                command = command.replace('/var/log/', str(root) + '/').replace('/dev/kmsg', str(root / 'kmsg'))
                command = command.replace('sleep 1', ':')
                return subprocess.run(['/bin/sh', '-c', command], capture_output=True, text=True,
                    env=dict(os.environ, PATH=str(bindir) + ':' + os.environ['PATH'])).stdout
            logs.console = SimpleNamespace(run=run)
            def header(boot='1-2-3'):
                return 'BREENIX-LOG 1 boot=' + boot + ' source=kernel limit=262144 archives=4\n'
            live = 'kernel-live-fresh'
            marker = '[kernel] [ INFO] audit: ' + live + '\n'
            gap = '[log] lost lines=unknown bytes=20000 reason=ring-overrun from=260647 to=280647\n'
            (root / 'kernel.log').write_text(header() + marker)
            archive = root / 'kernel.log.1'
            archive.write_text(header() + gap)
            logs.kernel_gap(live, 260647)
            self.assertEqual((root / 'controls').read_text().splitlines(), ['stop blogd', 'start blogd'])
            for content in (header('old-boot') + gap, header() + gap.replace('20000', '19999'),
                            header() + gap.replace('260647', '260648').replace('280647', '280648'), header()):
                archive.write_text(content)
                with self.assertRaisesRegex(ValueError, 'byte-exact ring overrun'):
                    logs.kernel_gap(live, 260647)
            archive.write_text(header() + gap)
            (root / 'kernel.log').write_text(header() + marker.replace('fresh', 'stale'))
            with self.assertRaisesRegex(ValueError, 'live kernel record'):
                logs.kernel_gap(live, 260647)
            (root / 'kernel.log').write_text(header('old-boot') + marker)
            with self.assertRaisesRegex(ValueError, 'root log assertion failed'):
                logs.kernel_gap(live, 260647)

    def test_service_log_header_must_name_the_boot_id_read_over_sol(self):
        console = object.__new__(host['Console'])
        console.boot_id = None
        header = 'BREENIX-LOG 1 boot={} source=bsshd limit=262144 archives=4\n'
        replies = ['boot-id=1-2-3\n' + header.format('9-9-9'), 'boot-id=1-2-3\n' + header.format('1-2-3')]
        console.run = lambda command, done=None: replies.pop(0)
        with self.assertRaisesRegex(ValueError, 'boot ID read over SOL'):
            console.service_log('bsshd')
        console.service_log('bsshd')
        self.assertEqual(console.boot_id, '1-2-3')

    def test_log_sweep_requires_ownership_besides_supervisor_header(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            tables, logs_dir = root / 'tables', root / 'logs'
            tables.mkdir()
            logs_dir.mkdir()
            def run(command, **kwargs):
                command = command.replace('/run/breenix/services.d', str(tables)).replace('/var/log', str(logs_dir))
                return subprocess.run(['/bin/sh', '-c', command], capture_output=True, text=True).stdout
            logs = object.__new__(host['Logs'])
            logs.console = SimpleNamespace(run=run)
            logs.state = lambda: {}
            mark = host['FIXTURE_MARK']
            owner = logs_dir / ('.logs-root.' + mark)
            paths = [logs_dir / ('logs-root.log' + suffix) for suffix in ('', '.1', '.4')]
            header = 'BREENIX-LOG 1 boot=1-2-3 source=logs-root limit=262144 archives=4\n'
            for path in paths: path.write_text(header + 'administrator record\n')
            with self.assertRaisesRegex(ValueError, 'root log assertion failed'):
                logs.sweep(['logs-root'])
            for path in paths: self.assertEqual(path.read_text(), header + 'administrator record\n')
            with self.assertRaises(ValueError): logs.claim('logs-root')
            self.assertFalse(owner.exists())
            for path in paths: path.unlink()
            logs.claim('logs-root')
            self.assertEqual(owner.read_text(), mark + '\n')
            for path in paths: path.write_text(header + 'fixture record\n')
            # Ownership survives a preparation boot and permits its retained log.
            logs.claim('logs-root')
            logs.sweep(['logs-root'])
            self.assertEqual(list(logs_dir.iterdir()), [])

    def test_log_claim_rejects_foreign_marker_and_dangling_archive(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            def run(command, **kwargs):
                return subprocess.run(['/bin/sh', '-c', command.replace('/var/log', str(root))],
                                      capture_output=True, text=True).stdout
            logs = object.__new__(host['Logs'])
            logs.console = SimpleNamespace(run=run)
            owner = root / ('.logs-root.' + host['FIXTURE_MARK'])
            owner.write_text('administrator file\n')
            with self.assertRaises(ValueError): logs.claim('logs-root')
            self.assertEqual(owner.read_text(), 'administrator file\n')
            owner.unlink()
            archive = root / 'logs-root.log.4'
            archive.symlink_to(root / 'absent')
            with self.assertRaises(ValueError): logs.claim('logs-root')
            self.assertTrue(archive.is_symlink())
            self.assertFalse(owner.exists())

    def test_malformed_records_fail_this_milestone_without_crashing_the_boot_scorer(self):
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        score_boot = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))['score']
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            def write(name, value):
                path = directory / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(value))
            write('run.json', self.boot | dict(disk='persistent'))
            write('definition/docs/metal/boot-path.json', dict(gateArch='x86_64', milestones=[]))
            write('definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json', dict(stages=[]))
            write('definition/docs/suites/server.json', self.manifest)
            write('definition/docs/efforts/path.json', self.path)
            write('definition/docs/boot-path.json', dict(fatal=[]))
            source = directory / 'definition/scripts/metal/server-suite'
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text((ROOT / 'scripts/metal/server-suite').read_text())
            (directory / 'serial.txt').write_text(self.serial)
            (directory / 'events.jsonl').write_text('')
            write('server-ssh-keys.json', self.records | dict(cases=None))
            write('server-services.json', self.records | dict(milestone='server/services', cases=None))
            write('server-services-transcript.json', [])
            verdict = score_boot(directory)
            self.assertFalse(verdict['milestones']['server/ssh-keys']['passed'])
            self.assertFalse(verdict['milestones']['server/services']['passed'])
            self.assertNotIn('server/services-transcript', verdict['milestones'])
            self.assertTrue(verdict['captureComplete'])

    def test_services_evidence_needs_its_cases_cleanup_and_console_lines(self):
        records = dict(self.records, milestone='server/services',
                       cases=[dict(id='services/' + name, result='PASS') for name, _ in host['CATEGORIES']['services']])
        serial = ('[stdout] bsshd: host key ED25519 SHA256:example (loaded)\n[init] bsshd killed by signal 9\n'
                  '[init] svc-suite-fail is crash-looping: 5 failures in a row, each within 10 s of starting\n')
        def score(records=records, serial=serial):
            return host['score'](self.manifest, self.path, records, serial, self.boot, self.sha, ['KERNEL PANIC:'])
        self.assertTrue(score().startswith('PASS: server/services categories=services'))
        # ssh-keys results do not complete the services milestone, nor the reverse.
        with self.assertRaises(ValueError): score(dict(records, cases=self.records['cases']))
        with self.assertRaises(ValueError): score(dict(self.records, milestone='server/services'))
        with self.assertRaises(ValueError): score(dict(records, milestone='server/logs'))
        with self.assertRaises(ValueError): score(dict(records, cleanup=False))
        for line in serial.splitlines()[1:]:
            with self.assertRaises(ValueError): score(serial=serial.replace(line + '\n', ''))
        failed = copy.deepcopy(records)
        failed['cases'][3]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score(failed)

    def test_fixtures_are_written_as_console_literals(self):
        with tempfile.TemporaryDirectory() as scratch:
            target = Path(scratch) / 'services'
            commands = []
            def console_run(command):
                commands.append(command)
                self.assertNotIn('\n', command)
                if command == 'svc reload': return 'svc: reloaded 6 added'
                subprocess.run(['/bin/sh', '-c', command], check=True)
                return ''
            probe = SimpleNamespace(user='service-user')
            console = SimpleNamespace(size=lambda: 0, run=console_run)
            services = host['Services'](probe, console, None)
            services.fixture_file = str(target)
            services.ensure_fixtures()
            table = target.read_text()
            self.assertEqual(table.count('user = service-user\n'), 5)
            self.assertIn('trap "" TERM; exec /bin/sleep 3600', table)
            self.assertIn('user = svc-suite-absent\n', table)
            self.assertEqual(len(commands), 2)

    def test_reconnect_rejects_success_after_deadline(self):
        services = object.__new__(host['Services'])
        services.state = lambda: ({}, {'bsshd': {'pid': 42}})
        services.console = SimpleNamespace(size=lambda: 0, login=lambda: None,
                                          type_line=lambda line: None, wait=lambda *args: None)
        clock = [0.0]
        timeouts = []
        def ssh(script, timeout):
            timeouts.append(timeout)
            clock[0] = 11.0
            return SimpleNamespace(returncode=0, stdout='1000\n')
        services.probe = SimpleNamespace(ssh=ssh, transcript=[])
        with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]):
            with self.assertRaisesRegex(ValueError, 'within 10 s'):
                services.case_kill_restart()
        self.assertEqual(timeouts, [10.0])
        self.assertEqual(services.probe.transcript, [])

    def test_missing_category_and_manifest_drift_block_acceptance(self):
        self.manifest['categories'][0]['cases'] = []
        with self.assertRaises(ValueError):
            host['score'](self.manifest, self.path, self.records, self.serial, self.boot, self.sha, ['KERNEL PANIC:'])


if __name__ == '__main__':
    unittest.main()
