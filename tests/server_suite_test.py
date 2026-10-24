"""The host server scorer refuses incomplete or mismatched machine evidence."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import re
import socket
import struct
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing

ROOT = Path(__file__).resolve().parents[1]
host = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))


class RebootReadback(unittest.TestCase):
    def test_interleaved_console_keeps_exact_order_and_data_checks(self):
        verify = runpy.run_path(str(ROOT / 'scripts/metal/server_reboot.py'))['verify_root_readback']
        root, token = '/var/lib/reboot-fixture', 'fresh-nonce'
        def capture(order=b'reboot-child\nreboot-base\n', data=token.encode()):
            return ('[ INFO] exec\n' + hashlib.sha256(order).hexdigest() + '  ' + root + '/order\n'
                    '[ INFO] another exec\n' + hashlib.sha256(data).hexdigest() + '  ' + root + '/data\n')
        verify(root, token, capture())
        for output in [capture(order=b'reboot-base\nreboot-child\n'), capture(data=b'old-nonce'),
                       capture(data=token.encode() + b'\n'), capture().replace(root, '/var/lib/other-fixture')]:
            with self.assertRaises(ValueError):
                verify(root, token, output)


class TimeFixture(unittest.TestCase):
    def test_shifted_clock_keeps_reference_fresh(self):
        for offset in (0, 172800, -172800):
            fixture = host['SntpFixture'](offset=offset)
            try:
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                    client.settimeout(2)
                    client.sendto(bytes(48), ('127.0.0.1', fixture.port))
                    reply = client.recv(48)
                reference = struct.unpack('!Q', reply[16:24])[0]
                transmit = struct.unpack('!Q', reply[40:48])[0]
                self.assertLess(abs((transmit - reference) / (1 << 32) - 1), .1)
            finally:
                fixture.close()


class PersistentServerRecord(existing.StateTest):
    def test_persistent_session_record_can_supply_server_evidence(self):
        sha = 'a' * 40
        args = SimpleNamespace(id='20261021T000000Z-metal', mode='persistent', payload='persistent',
                               session=False, target='main', task='', commit=sha, inject=None, nmi=False)
        with patch.object(existing.m, 'load_site', return_value={}):
            driver = existing.boot.Driver(args)
        driver.create_record()
        record = existing.m.read_json(driver.dir / 'run.json')
        self.assertTrue(record['session'])
        self.assertTrue(record['evidenceEligible'])
        self.assertEqual(record['kind'], 'persistent')
        record.update(bootedCommit=sha, toolsCommit=sha)
        manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        records = dict(boot=record['id'], commit=sha, milestone='server/ssh-keys', cleanup=True,
                       cases=[dict(id='ssh-keys/' + name, result='PASS') for name, _ in host['CATEGORIES']['ssh-keys']])
        serial = "[stdout] bsshd: host key ED25519 SHA256:example (loaded)\nbsshd: refused method 'password'\n"
        self.assertTrue(host['score'](manifest, path, records, serial, record, sha, []).startswith('PASS: server/ssh-keys'))


class HostServerScore(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        self.path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        self.sha = 'a' * 40
        self.boot = dict(id='20261021T000000Z-metal', mode='persistent', bootedCommit=self.sha, toolsCommit=self.sha, evidenceEligible=True)
        self.records = dict(boot=self.boot['id'], commit=self.sha, milestone='server/ssh-keys', cleanup=True,
                            cases=[dict(id='ssh-keys/' + name, result='PASS') for name, _ in host['CATEGORIES']['ssh-keys']])
        self.serial = "[stdout] bsshd: host key ED25519 SHA256:example (loaded)\nbsshd: refused method 'password'\n"

    def test_panic_provenance_uses_validated_image_identity(self):
        records = dict(self.records, milestone='server/reboot',
                       cases=[dict(id='reboot/' + name, result='PASS') for name, _ in host['REBOOT']],
                       refusalRecovery=dict(serial='[shutdown] kernel refused transition: -13\n[shutdown] refused; resuming services'),
                       transitions=[], panicBoot=dict(run=dict(toolsCommit=self.sha, kind='fault-test'),
                            build=dict(commit=self.sha), serial='KERNEL PANIC: reboot fault test',
                            events=[dict(event='panic-reset-observed')]))
        for action in ('reboot', 'poweroff'):
            records['transitions'].append(dict(action=action, before='same', after='same', exit='0',
                beforeRun=self.boot, afterRun=self.boot,
                beforeSerial='[shutdown] home clean read-only\n[shutdown] root clean read-only',
                afterSerial='[storage] root prior state=clean\n[storage] home prior state=clean',
                events=[dict(event='requested-shutdown', action=action), dict(event='requested-reset-observed'),
                        dict(event='requested-reboot-ready', commit=self.sha)]))
        def score():
            return host['score'](self.manifest, self.path, records, self.serial, self.boot, self.sha, [])
        self.assertTrue(score().startswith('PASS:'))
        records['panicBoot']['build']['commit'] = 'b' * 40
        with self.assertRaisesRegex(ValueError, 'matching fault-test'):
            score()

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

    def test_watchdog_requires_distinct_boots_for_the_named_cases(self):
        records = dict(self.records, milestone='server/watchdog',
                       cases=[dict(id='watchdog/' + name, result='PASS') for name, _ in host['WATCHDOG']],
                       faultBoots=[dict(passed=True, commit=self.sha, boot=str(i), receipt=dict(case=name))
                                   for i, (name, _) in enumerate(host['WATCHDOG'])])
        def score():
            return host['score'](self.manifest, self.path, records, self.serial, self.boot, self.sha, [])
        self.assertTrue(score().startswith('PASS: server/watchdog'))
        records['faultBoots'][1]['receipt']['case'] = 'feeder-hang'
        with self.assertRaises(ValueError): score()
        records['faultBoots'][1]['receipt']['case'] = 'health-hang'
        records['faultBoots'][1]['boot'] = '0'
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
        records = dict(self.records, milestone='server/services', initLog=['[init] bsshd killed by signal 9'],
                       cases=[dict(id='services/' + name, result='PASS') for name, _ in host['CATEGORIES']['services']])
        serial = ('[stdout] bsshd: host key ED25519 SHA256:example (loaded)\n'
                  '1234 [init] bsshd killed by signal 9\n'
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
        # The root console read must capture the record, independently of runner metadata.
        self.assertTrue(score(dict(records, initLog=[])).startswith('PASS:'))
        with self.assertRaisesRegex(ValueError, 'init log'):
            score(serial=serial.replace('1234 [init] bsshd killed by signal 9\n', ''))
        failed = copy.deepcopy(records)
        failed['cases'][3]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score(failed)

    def test_fixtures_are_written_as_console_literals(self):
        with tempfile.TemporaryDirectory() as scratch:
            target = Path(scratch) / 'services'
            commands = []
            def console_run(command, timeout=30):
                commands.append(command)
                self.assertNotIn('\n', command)
                if command == 'svc reload': return 'svc: reloaded 6 added'
                # init's log, read before the fixtures load (#2282): one earlier record.
                if command.startswith("sh -c 'cat /var/log/init.log"): return '1234 [init] blogd started (PID 9)\ninit-log-read\n'
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
            self.assertEqual(len(commands), 3)
            self.assertEqual(commands[2], 'svc reload')
            self.assertEqual(services.init_start, 1)
            self.assertEqual(services.init_log, ['[init] blogd started (PID 9)'])

    def test_restart_matches_submitted_line_and_fresh_prompt_before_reconnect(self):
        for prefix, prompt in (('', 'bsh /> '),
                               ('bsh /> kill -9\nbsh /> \n', 'bsh /> '),
                               ('', 'bsh /> [init] ntp is crash-looping\n')):
            for prompt_delay in (1, 11):
                with self.subTest(prefix=prefix, prompt=prompt, prompt_delay=prompt_delay):
                    services = object.__new__(host['Services'])
                    clock, states, notices = [0.0], [0], []
                    stale = 'bsh /> \n'
                    submitted = stale + prefix + 'bsh /> kill -9 42\n'
                    def state(timeout=45):
                        states[0] += 1
                        if states[0] == 1: return {}, {'bsshd': dict(pid=42, starts=1)}
                        self.assertEqual(timeout, 10 - prompt_delay)
                        return {}, {'bsshd': dict(pid=43, starts=2, state='running', exit='signal:9')}
                    console = object.__new__(host['Console'])
                    console.login = lambda: None
                    console.size = lambda: len(stale.encode())
                    console.type_line = lambda line: self.assertEqual(line, 'kill -9 42')
                    # Replay cleaned capture through the real wait: the retry's
                    # cleared prompt precedes submission; completion arrives later.
                    def since(offset):
                        text = submitted + (prompt if clock[0] >= prompt_delay else '')
                        return text.encode()[offset:].decode()
                    console.since = since
                    def notice(*args, **kwargs):
                        self.assertEqual(states[0], 2)
                        notices.append(args[1])
                        clock[0] += 8  # Slow SOL log read must not consume restart time.
                    def ssh(script, timeout):
                        self.assertEqual(timeout, 10 - prompt_delay)
                        self.assertEqual(notices, [])
                        return SimpleNamespace(returncode=0, stdout='1000\n')
                    services.state = state
                    services.init_records = lambda: []
                    services.await_init_line = notice
                    services.console = console
                    services.probe = SimpleNamespace(ssh=ssh, transcript=[])
                    with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]), \
                            patch.object(host['time'], 'sleep', side_effect=lambda delay: clock.__setitem__(0, clock[0] + delay)):
                        if prompt_delay > 10:
                            with self.assertRaisesRegex(ValueError, 'shell prompt'):
                                services.case_kill_restart()
                            self.assertEqual(clock[0], 10)
                        else:
                            services.case_kill_restart()
                            self.assertEqual(len(notices), 2)
                            self.assertEqual(services.probe.transcript, [dict(reconnectSeconds=1)])

    def test_reconnect_rejects_success_after_deadline(self):
        services = object.__new__(host['Services'])
        services.state = lambda: ({}, {'bsshd': {'pid': 42}})
        services.console = SimpleNamespace(size=lambda: 0, login=lambda: None,
                                          type_line=lambda line: None, wait=lambda *args, **kwargs: ('kill\n', re.search('kill\n', 'kill\n')))
        clock = [0.0]
        timeouts = []
        def ssh(script, timeout):
            timeouts.append(timeout)
            clock[0] = 11.0
            return SimpleNamespace(returncode=0, stdout='1000\n')
        services.probe = SimpleNamespace(ssh=ssh, transcript=[])
        services.init_records = lambda: []
        services.await_init_line = lambda *args, **kwargs: [(0, '[init] bsshd killed by signal 9')]
        with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]):
            with self.assertRaisesRegex(ValueError, 'within 10 s'):
                services.case_kill_restart()
        self.assertEqual(timeouts, [10.0])
        self.assertEqual(services.probe.transcript, [])

    def test_restart_bounds_state_read_before_reading_reap_record(self):
        for state_delay in (0, 10):
            with self.subTest(state_delay=state_delay):
                services = object.__new__(host['Services'])
                clock, events, state_timeouts = [0.0], [], []
                before = dict(pid=42, starts=1)
                after = dict(pid=43, starts=2, state='running', exit='signal:9')
                def state(timeout=45):
                    state_timeouts.append(timeout)
                    if len(state_timeouts) == 1: return {}, {'bsshd': before}
                    clock[0] += state_delay
                    return {}, {'bsshd': after}
                def notice(*args, **kwargs):
                    events.append('reaped')
                    clock[0] += 2
                def ssh(script, timeout):
                    self.assertEqual(events, [])
                    self.assertEqual(timeout, 10)
                    clock[0] += 1
                    return SimpleNamespace(returncode=0, stdout='1000\n')
                services.state = state
                services.init_records = lambda: []
                services.await_init_line = notice
                services.console = SimpleNamespace(login=lambda: None, size=lambda: 0,
                                                   type_line=lambda line: None, wait=lambda *args, **kwargs: ('kill\n', re.search('kill\n', 'kill\n')))
                services.probe = SimpleNamespace(ssh=ssh, transcript=[])
                with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]):
                    if state_delay:
                        with self.assertRaisesRegex(ValueError, 'state.*within 10 s'):
                            services.case_kill_restart()
                    else:
                        services.case_kill_restart()
                self.assertEqual(state_timeouts, [45, 9])

    def test_operator_category_is_scored_against_the_users_manifest(self):
        users = json.loads((ROOT / 'docs/suites/users.json').read_text())
        host['validate_manifest'](users)
        records = dict(self.records, milestone='users/operator-login',
                       cases=[dict(id='operator/' + name, result='PASS') for name, _ in host['OPERATOR']])
        serial = self.serial + ''.join(line + '\n' for line in host['OPERATOR_CONSOLE_LINES'])
        def score(manifest=users, serial=serial):
            return host['score'](manifest, self.path, records, serial, self.boot, self.sha, ['KERNEL PANIC:'])
        self.assertTrue(score().startswith('PASS: users/operator-login categories=operator cases=6'))
        with self.assertRaisesRegex(ValueError, 'users manifest'):
            score(self.manifest)
        for line in host['OPERATOR_CONSOLE_LINES']:
            with self.subTest(line=line), self.assertRaisesRegex(ValueError, 'no line'):
                score(serial=serial.replace(line + '\n', ''))
        records['cases'][2]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score()
        # A host-run category whose cases drift from the runner's table is refused.
        drifted = copy.deepcopy(users)
        next(c for c in drifted['categories'] if c['id'] == 'operator')['cases'].pop()
        with self.assertRaises(ValueError): host['validate_manifest'](drifted)

    def test_boot_scorer_reads_operator_records_and_leaves_other_users_files(self):
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
            write('definition/docs/suites/users.json', json.loads((ROOT / 'docs/suites/users.json').read_text()))
            write('definition/docs/efforts/path.json', self.path)
            write('definition/docs/boot-path.json', dict(fatal=[]))
            source = directory / 'definition/scripts/metal/server-suite'
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text((ROOT / 'scripts/metal/server-suite').read_text())
            (directory / 'serial.txt').write_text(self.serial + ''.join(line + '\n' for line in host['OPERATOR_CONSOLE_LINES']))
            (directory / 'events.jsonl').write_text('')
            write('users-operator.json', self.records | dict(milestone='users/operator-login',
                  cases=[dict(id='operator/' + name, result='PASS') for name, _ in host['OPERATOR']]))
            write('users-operator-transcript.json', [])
            write('users-hostssh.json', dict(boot=self.boot['id'], requests=[]))
            verdict = score_boot(directory)
            self.assertEqual(list(verdict['milestones']), ['users/operator-login'])
            self.assertTrue(verdict['milestones']['users/operator-login']['passed'])

    def test_missing_category_and_manifest_drift_block_acceptance(self):
        self.manifest['categories'][0]['cases'] = []
        with self.assertRaises(ValueError):
            host['score'](self.manifest, self.path, self.records, self.serial, self.boot, self.sha, ['KERNEL PANIC:'])


class WatchdogRecovery(existing.StateTest):
    def test_fault_launch_remains_ram_root_in_detached_driver(self):
        for fault in existing.boot.WATCHDOG_FAULTS:
            def launch(args, extra):
                with patch.object(sys, 'argv', ['metal-boot', 'run', '--id', '20261023T000000Z-metal'] + extra), \
                     patch.object(existing.boot, 'Driver') as driver:
                    driver.return_value.run.return_value = 0
                    self.assertEqual(existing.boot.main(), 0)
                    self.assertIsNone(driver.call_args.args[0].disk)
                return 0
            with self.subTest(fault=fault), \
                 patch.object(sys, 'argv', ['metal-boot', 'launch', '--payload', 'image', '--inject', fault]), \
                 patch.object(existing.boot, 'launch', side_effect=launch):
                self.assertEqual(existing.boot.main(), 0)

    def setUp(self):
        super().setUp()
        self.sha = 'a' * 40
        args = SimpleNamespace(id='20261023T000000Z-metal', mode='default', payload='image',
                               target='main', task='', commit=self.sha, inject='watchdog-feeder-hang', nmi=False)
        with patch.object(existing.m, 'load_site', return_value={}):
            self.driver = existing.boot.Driver(args)
        d = self.driver
        d.create_record()
        d.watchdog_trigger = 100
        existing.m.write_json(d.dir / 'run.json', dict(toolsCommit=self.sha, watchdogRecoveredCommit=self.sha))
        existing.m.write_text(d.dir / 'events.jsonl', '')
        self.serial = ('[watchdog] armed seconds=120 action=hard-reset\n[watchdog] healthy feeds=6\n'
                       '[init] log boot-id=1-2-3\n[watchdog-fault] trigger feeder-hang\n'
                       '[storage] root partition: system synthetic\n[watchdog] previous-expired=1\n'
                       '[init] log boot-id=4-5-6\n')
        existing.m.write_text(d.dir / 'serial.txt', self.serial)
        self.receipt = dict(boot=d.id, commit=self.sha, case='feeder-hang', trigger=100, sshAt=450, bootId='4-5-6')

    def test_clean_return_waits_for_authenticated_shutdown_after_watchdog_proof(self):
        d = self.driver
        existing.m.write_json(d.dir / 'watchdog-ssh.json', dict(self.receipt, cleanShutdown=True))
        with patch.object(existing.m, 'now', return_value=451):
            d.check_deadlines()
        self.assertTrue(existing.m.read_json(d.dir / 'watchdog-verdict.json')['passed'])
        with patch.object(existing.m, 'now', return_value=571):
            with self.assertRaisesRegex(existing.boot.Finish, 'clean shutdown deadline'):
                d.check_deadlines()

    def test_trigger_round_trip_retains_exact_driver_timestamp(self):
        d = self.driver
        stamp = 100.123456
        d.watchdog_trigger = None
        with patch.object(existing.m, 'update_lease'):
            d.on_watchdog_line('[watchdog-fault] trigger feeder-hang', stamp)
        event = json.loads((d.dir / 'events.jsonl').read_text())
        self.assertNotEqual(event['t'], stamp)
        self.assertEqual(event['trigger'], stamp)
        self.assertTrue(d.watchdog_verdict(dict(self.receipt, trigger=event['trigger'])))
        self.assertFalse(d.watchdog_verdict(dict(self.receipt, trigger=event['t'])))

    def test_baseline_lease_loss_refuses_destructive_launch(self):
        remote = lambda *a, **k: SimpleNamespace(returncode=0, stdout=json.dumps(dict(id='other')))
        target = host['Watchdog'](None, None, remote, None, None, 'baseline', self.sha, 'fixture')
        with patch.dict(target.run_case.__globals__, command=lambda *a, **k: self.fail('launched after lease loss')):
            with self.assertRaisesRegex(ValueError, 'baseline lost'):
                target.run_case('feeder-hang')

    def test_completed_fault_waits_for_its_lease_release(self):
        boot = self.driver.id
        trigger = 100.123456
        receipt = dict(self.receipt, sshAt=200, trigger=trigger)
        saved = False
        leases = []
        def remote(*args, **kwargs):
            nonlocal saved
            if args[0] == 'tee':
                saved = True
                self.assertEqual(json.loads(kwargs['input']), receipt)
                return SimpleNamespace(returncode=0)
            leases.append(1)
            return SimpleNamespace(returncode=0 if len(leases) == 1 else 1,
                                   stdout=json.dumps(dict(id=boot)))
        probe = SimpleNamespace(ssh=lambda _: SimpleNamespace(returncode=0, stdout='4-5-6'))
        target = host['Watchdog'](probe, None, remote, None, None, 'baseline', self.sha, 'fixture')
        target.stopped = True
        def read(_, name):
            return {'events.jsonl': json.dumps(dict(event='watchdog-trigger', t=round(trigger, 3), trigger=trigger)),
                    'serial.txt': '[watchdog] previous-expired=1',
                    'run.json': json.dumps(dict(watchdogRecoveredCommit=self.sha)),
                    'ended': 'done' if saved else '',
                    'watchdog-verdict.json': json.dumps(dict(passed=True, boot=boot, commit=self.sha, receipt=receipt))}[name]
        target.read = read
        launch = lambda *a, **k: SimpleNamespace(returncode=0, stdout=json.dumps(dict(id=boot)))
        with patch.dict(target.run_case.__globals__, command=launch), \
             patch.object(host['time'], 'time', return_value=200), patch.object(host['time'], 'sleep'):
            target.run_case('feeder-hang')
        self.assertEqual(len(leases), 2)

    def test_late_watchdog_evidence_is_reannounced_to_the_importer(self):
        directory = self.driver.dir
        existing.m.write_text(existing.m.STATE / 'finished.jsonl', json.dumps(dict(id=directory.name, seq=8)) + '\n')
        verdict = dict(milestones={'server/watchdog': dict(passed=True)})
        with patch.object(sys, 'argv', ['-c', str(directory)]), \
             patch.dict(sys.modules, {'metal_score': SimpleNamespace(score=lambda _: verdict)}):
            exec(host['WATCHDOG_ARCHIVE'], {})
        entries = [json.loads(line) for line in (existing.m.STATE / 'finished.jsonl').read_text().splitlines()]
        self.assertEqual(entries[-1], dict(id=directory.name, seq=9))
        self.assertEqual(existing.m.read_json(directory / 'milestone-verdict.json'), verdict)

    def test_expiry_and_fresh_persistent_ssh_are_required(self):
        d = self.driver
        self.assertTrue(d.watchdog_verdict(self.receipt))
        for missing in ('[watchdog] previous-expired=1', '[watchdog] healthy feeds=6',
                        '[storage] root partition: system ', '[init] log boot-id=4-5-6'):
            existing.m.write_text(d.dir / 'serial.txt', self.serial.replace(missing, 'missing'))
            self.assertFalse(d.watchdog_verdict(self.receipt))
        existing.m.write_text(d.dir / 'serial.txt', self.serial)
        self.assertFalse(d.watchdog_verdict(dict(self.receipt, sshAt=701)))
        self.assertFalse(d.watchdog_verdict(dict(self.receipt, trigger=101)))
        self.assertFalse(d.watchdog_verdict(dict(self.receipt, bootId='1-2-3')))

    def test_lab_intervention_cannot_pass_and_observation_sends_no_power(self):
        d = self.driver
        for event in ('power-on', 'power-off', 'nmi', 'console-gap-start', 'nmi-capture-done'):
            existing.m.write_text(d.dir / 'events.jsonl', json.dumps(dict(event=event, t=200)) + '\n')
            self.assertFalse(d.watchdog_verdict(self.receipt))
        with patch.object(existing.m, 'rf') as power, patch.object(existing.m, 'now', return_value=300):
            d.check_deadlines()
            power.assert_not_called()


class ConsoleEchoTests(unittest.TestCase):
    def test_console_only_score_needs_no_ssh_record(self):
        sha = 'a' * 40
        boot = dict(id='20261023T000000Z-metal', mode='persistent', bootedCommit=sha,
                    toolsCommit=sha, evidenceEligible=True)
        records = dict(boot=boot['id'], commit=sha, milestone='server/console-input',
                       cases=[dict(id='console-input/long-line', result='PASS', ms=1)],
                       cleanup=True, consoleGaps=[])
        manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        effort = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        self.assertEqual(host['score'](manifest, effort, records, '', boot, sha, []),
                         'PASS: server/console-input categories=console-input cases=1')

    def test_kernel_records_between_echo_fragments_preserve_the_block(self):
        text = 'a' * 32 + '\n[DISPATCH_STRAND_CENSUS:tick=1]\n' + 'a' * 32
        self.assertEqual(host['canonical_echo'](text), 'a' * 64)

    def test_kernel_text_cannot_replace_missing_echo(self):
        text = 'a' * 32 + '\n[INFO] ' + 'a' * 64 + '\n'
        self.assertEqual(host['canonical_echo'](text), 'a' * 32)
        self.assertEqual(host['canonical_echo']('a\n[INFO] aaaa'), 'a')


if __name__ == '__main__':
    unittest.main()
