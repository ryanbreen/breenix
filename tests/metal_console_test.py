"""Scoped console staging and SOL protocol tests. No controller is contacted."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
import metal_root

boot = existing.boot

class ConsoleTest(unittest.TestCase):
    def driver(self):
        driver = object.__new__(boot.Driver)
        driver.args = SimpleNamespace(mode='console')
        driver.site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345', 'CONSOLE_IDENTITY_KEY': 'ab' * 32}
        driver.fatal_at = None
        driver.delivery = {}
        driver.console_sudo_answered = False
        driver.console_step = 0
        driver.console_done = False
        driver.final_seen = True
        driver.required_stages = []
        driver.console_intent = None
        driver.seen = set()
        driver.event = Mock()
        driver.console_send = Mock()
        return driver

    def test_suite_replay_chunks_fit_the_uart_without_interactive_delays(self):
        driver = self.driver()
        driver.args.mode = 'suite'
        driver.tmux = ['tmux', '-L', 'fixture']
        line = boot.hostkeys.ReplayKeys.line((), 'v')
        with patch.object(boot.subprocess, 'run') as run, patch.object(boot.time, 'sleep') as pause:
            boot.Driver.console_send(driver, line, paced=True)
        loads = [call.kwargs['input'] for call in run.call_args_list if 'load-buffer' in call.args[0]]
        self.assertEqual(b''.join(loads), (line + '\r').encode())
        self.assertTrue(all(len(chunk) == 8 for chunk in loads[:-1]))
        self.assertTrue(all(call.args == (0.01,) for call in pause.call_args_list))

    def test_suite_replay_uses_the_paced_sol_transport(self):
        driver = self.driver()
        driver.args.mode = 'suite'
        driver.kernel_started = True
        driver.hostkey_watcher = None
        with patch.object(boot.hostkeys.time, 'sleep'):
            driver.on_hostkey_line('SUITE console HOSTKEY getty/independent-sessions seq=1 keys=v,t,enter')
        self.assertEqual(driver.console_send.call_count, 3)
        for call in driver.console_send.call_args_list:
            self.assertTrue(call.args[0].startswith('BXKEY '))
            self.assertEqual(call.kwargs, {'paced': True})

    def test_wrong_password_right_password_computation_and_reacquisition_are_required(self):
        driver = self.driver()
        # A prompt or an echoed command cannot stand in for the calculation.
        for text in ['Breenix login: ', 'Password: ', 'bsh />', 'metal-login-ok 42']:
            driver.challenge_console(text)
        self.assertEqual(driver.console_step, 2)
        for text in ['Login incorrect', 'Breenix login: ', 'Password: ', 'bsh /root>',
                     'print("metal-login-ok", 6*7)', 'metal-login-ok 42', 'bsh /root> ']:
            driver.challenge_console(text)
        self.assertEqual(driver.console_send.call_args.args[0], 'cat /proc/fbcon/check')
        # Logout waits for the check's line and then the prompt after its rows.
        driver.challenge_console('fbcon check: shadow and screen identical, 7 glyph cells match the font', complete=True)
        for text in ['fbcon row: metal-login-ok 42', 'bsh /root> ',
                     '[init] console session ended; restarting getty', '[getty] console acquired']:
            driver.challenge_console(text)
        self.assertFalse(driver.console_done)
        with self.assertRaises(boot.Finish) as result:
            driver.challenge_console('Breenix login: ')
        self.assertEqual(result.exception.status, 0)
        self.assertTrue(driver.console_done)
        self.assertEqual([call.args[0] for call in driver.console_send.call_args_list],
                         ['root', 'deliberately-wrong-password', 'root', driver.site['CONSOLE_PASSWORD'],
                          'print("metal-login-ok", 6*7)', 'cat /proc/fbcon/check', 'exit'])

    def test_fixture_account_logs_in_and_reads_the_screen_check_through_sudo(self):
        driver = self.driver()
        driver.delivery = {'diskRoot': {'consoleAccount': 'uw'}}
        password = driver.site['CONSOLE_PASSWORD']
        for text in ['Breenix login: ', 'Password: ', 'Login incorrect', 'Breenix login: ', 'Password: ',
                     'bsh /etc/breenix/test-users/uw>', 'metal-login-ok 42']:
            driver.challenge_console(text)
        self.assertEqual(driver.console_send.call_args.args[0], 'sudo cat /proc/fbcon/check')
        # sudo's prompt is answered once, however often SOL repeats the partial line.
        driver.challenge_console('[sudo] password for uw: ')
        driver.challenge_console('[sudo] password for uw: ')
        driver.challenge_console('fbcon check: shadow and screen identical, 7 glyph cells match the font', complete=True)
        self.assertEqual(driver.console_step, 8)
        self.assertEqual([call.args[0] for call in driver.console_send.call_args_list],
                         ['uw', 'deliberately-wrong-password', 'uw', password,
                          'print("metal-login-ok", 6*7)', 'sudo cat /proc/fbcon/check', password])

    def test_console_account_is_the_fixture_only_when_the_image_carries_it(self):
        fixture = 'uw:x:2003:2003:Users suite fixture:/etc/breenix/test-users/uw:/bin/bsh\n'
        self.assertEqual(metal_root.console_account('root:x:0:0:root:/root:/bin/bsh\n' + fixture), 'uw')
        self.assertEqual(metal_root.console_account('root:x:0:0:root:/root:/bin/bsh\n'), 'root')
        self.assertEqual(metal_root.console_account('uw:x:2003:2003::/home/uw:/bin/bsh\n'), 'root')

    def test_console_shadow_sets_root_and_the_account_and_keeps_other_lines(self):
        with tempfile.TemporaryDirectory() as scratch:
            hasher = Path(scratch) / 'hasher'
            hasher.write_text('#!/usr/bin/env python3\nimport sys\nsalt = sys.stdin.buffer.read().split(b\'\\0\')[1].decode()\n'
                              'sys.stdout.write(salt + \'$\' + \'A\' * 86)\n')
            hasher.chmod(0o755)
            site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345', 'CONSOLE_IDENTITY_KEY': 'ab' * 32}
            text = metal_root.console_shadow(site, hasher, 'root:!:::::::\nnobody:*:::::::\nuw:!:::::::\n', 'uw')
        lines = text.splitlines()
        self.assertEqual([line.split(':')[0] for line in lines], ['root', 'nobody', 'uw'])
        self.assertEqual(lines[1], 'nobody:*:::::::')
        for line in (lines[0], lines[2]):
            self.assertRegex(line, r'^(root|uw):\$6\$[./A-Za-z0-9]{16}\$A{86}:\d+:0:99999:7:::$')
        self.assertNotEqual(lines[0].split(':')[1], lines[2].split(':')[1])

    def test_console_completion_requires_disk_evidence(self):
        driver = self.driver()
        driver.args.disk = 'root'
        driver.seen = set()
        driver.console_step = 11
        driver.challenge_console('Breenix login: ')
        self.assertTrue(driver.console_done)
        driver.console_step = 11
        driver.seen.add('disk-root')
        with self.assertRaises(boot.Finish) as result:
            driver.challenge_console('Breenix login: ')
        self.assertEqual(result.exception.status, 0)
        # A disk-root console boot also needs the kernel's account of its intent.
        driver.console_intent = 'measure'
        driver.console_step = 11
        driver.challenge_console('Breenix login: ')
        driver.console_step = 11
        driver.seen.add('console-intent')
        with self.assertRaises(boot.Finish) as result:
            driver.challenge_console('Breenix login: ')
        self.assertEqual(result.exception.status, 0)

    def test_disk_root_console_intent_must_come_from_the_staged_payload(self):
        line = '[ INFO] [console] input preparation: replug {} by {}'
        for staged, is_session, replug, source, honored in [
                ('measure', False, 'requested', 'intent', True),
                ('session', True, 'not requested', 'intent', True),
                ('measure', False, 'requested', 'target', False),
                ('measure', False, 'not requested', 'intent', False),
                ('session', True, 'requested', 'target', False)]:
            driver = self.driver()
            driver.args.session, driver.console_intent, driver.seen = is_session, staged, set()
            if honored or is_session:
                driver.on_intent_line(line.format(replug, source), 1.0)
            else:
                with self.assertRaises(boot.Finish) as result:
                    driver.on_intent_line(line.format(replug, source), 1.0)
                self.assertEqual(result.exception.status, existing.m.EXIT_SHORT)
            self.assertIn('console-intent', driver.seen)
            self.assertEqual(driver.event.call_args.kwargs['honored'], honored)
        # A RAM-disk console boot stages no intent and does not check the line.
        driver = self.driver()
        driver.seen = set()
        driver.on_intent_line(line.format('requested', 'target'), 1.0)
        self.assertEqual(driver.seen, set())

    def test_console_prompt_cannot_override_a_failing_serial_chunk(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            driver = fixture.driver
            driver.args.mode = 'console'
            driver.console_step = 11
            driver.final_seen = True
            driver.required_stages = []
            driver.on_line = Mock(side_effect=boot.Finish(existing.m.EXIT_SHORT, 'disk mismatch'))
            (driver.dir / 'serial.raw').write_bytes(b'disk mismatch\nBreenix login: ')
            with self.assertRaises(boot.Finish) as result:
                driver.pump_serial()
            self.assertEqual(result.exception.status, existing.m.EXIT_SHORT)
        finally:
            fixture.tearDown()

    def test_identity_is_keyed_and_tracks_password_changes(self):
        site = self.driver().site
        identity = metal_root.console_identity(site)
        self.assertNotEqual(identity, metal_root.console_identity(dict(site, CONSOLE_IDENTITY_KEY='cd' * 32)))
        self.assertNotEqual(identity, metal_root.console_identity(dict(site, CONSOLE_PASSWORD='changed_fixture_password_12345')))
        with self.assertRaises(ValueError):
            metal_root.console_identity(dict(site, CONSOLE_IDENTITY_KEY=''))

    def test_console_stages_keep_screen_readback_and_exact_line_matching(self):
        self.assertFalse(boot.marker_matches(('answer', 'line'), 'echo answer'))
        self.assertTrue(boot.marker_matches(('answer', 'line'), 'answer'))
        definition = existing.json.loads((existing.TOOLS.parents[1] / 'docs/metal/boot-path.json').read_text())
        catalog = existing.json.loads((existing.TOOLS.parents[1] /
                                       'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json').read_text())
        groups = boot.measured_stages(definition, 'console', catalog, 'ramdisk')
        self.assertIn(('fbcon row: metal-login-ok 42', 'contains'), [stage for group in groups for stage in group])
        self.assertEqual(boot.final_marker(definition, 'console', catalog, 'ramdisk'),
                         ('[getty] console acquired', 'line'))

    def test_console_input_options_are_validated_before_host_access(self):
        runner = existing.load_script('metal-run')
        for mode, input_kind in [('console', None), ('default', 'sol')]:
            with self.assertRaisesRegex(SystemExit, 'console mode requires'):
                runner.boot(SimpleNamespace(mode=mode, input=input_kind, suite=None))

    def test_password_is_sent_on_stdin_and_redacted(self):
        driver = self.driver()
        driver.tmux = ['tmux', '-L', 'fixture']
        secret = driver.site['CONSOLE_PASSWORD']
        with patch.object(boot.subprocess, 'run') as run:
            boot.Driver.console_send(driver, secret)
        self.assertNotIn(secret, repr(run.call_args_list[0].args))
        self.assertEqual(run.call_args_list[0].kwargs['input'], (secret + '\r').encode())
        # Paced, the secret goes on stdin a few bytes at a time and no piece of it reaches argv.
        with patch.object(boot.subprocess, 'run') as run, patch.object(boot.time, 'sleep'):
            boot.Driver.console_send(driver, secret, paced=True)
        loads = [call for call in run.call_args_list if 'load-buffer' in call.args[0]]
        self.assertEqual(b''.join(call.kwargs['input'] for call in loads), (secret + '\r').encode())
        self.assertTrue(all(len(call.kwargs['input']) <= 4 for call in loads))
        argv = {tuple(call.args[0]) for call in run.call_args_list}
        self.assertEqual(argv, {tuple(driver.tmux + ['load-buffer', '-']),
                                tuple(driver.tmux + ['paste-buffer', '-d', '-t', 'sol'])})
        self.assertNotIn(secret, existing.m.Redactor(driver.site)('Password: ' + secret))

    def test_disk_root_console_carries_current_boot_intent(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.disk, d.args.commit = 'console', 'root', 'a' * 40
            d.args.ramdisk = False
            d.site = self.driver().site
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            existing.m.write_json(inputs / 'build.json', {'commit': d.args.commit,
                                  'profile': 'production', 'features': []})
            existing.m.write_json(existing.m.STATE / 'disk.json', {'commit': d.args.commit,
                                  'mode': 'console', 'consoleIdentity': metal_root.console_identity(d.site)})
            for is_session, expected in [(True, b'BREENIX-BOOT 1\nconsole idle=30\n'),
                                         (False, b'BREENIX-BOOT 1\nconsole replug\n')]:
                d.args.session = is_session
                d.args.idle_minutes = 30
                with patch.object(boot.metal_disk, 'stage', return_value=({}, {})), \
                     patch.object(boot.metal_root, 'stage_root') as stage, \
                     patch.object(boot.metal_image, 'extract_esp', return_value={}), \
                     patch.object(boot.metal_image, 'assemble', side_effect=RuntimeError('assembly captured')) as assemble:
                    with self.assertRaisesRegex(RuntimeError, 'assembly captured'):
                        d.prepare_payload()
                stage.assert_not_called()
                files = assemble.call_args.args[1]
                self.assertEqual(files['ramdisk'].read_bytes(), expected)
                self.assertEqual(d.delivery['consoleIntent'], 'session' if is_session else 'measure')
            # The assembled image records the intent, not a RAM-disk root.
            with patch.object(boot.metal_disk, 'stage', return_value=({}, {})), \
                 patch.object(boot.metal_image, 'extract_esp', return_value={}), \
                 patch.object(boot.metal_image, 'assemble', side_effect=lambda path, files: (
                     path.write_bytes(b''), {'size': 1, 'sha256': '0' * 64, 'components': []})[1]), \
                 patch.object(boot.metal_image, 'stub_efi', return_value={'size': 1, 'sha256': '0' * 64, 'stub': {}}), \
                 patch.object(boot.Driver, 'load_definition'):
                d.prepare_payload()
            assembled = [c for c in d.event.call_args_list if c.args[0] == 'image-assembled'][-1].kwargs
            self.assertEqual((assembled['ramdisk'], assembled['consoleIntent']), (False, 'measure'))
        finally:
            fixture.tearDown()

    def test_only_measured_boots_stage_the_replug_exercise(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.commit = 'console', 'a' * 40
            d.site = self.driver().site
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            existing.m.write_json(inputs / 'build.json', {'commit': d.args.commit,
                                  'profile': 'production', 'features': []})
            for is_session, input_kind in [(False, 'sol'), (True, None)]:
                d.args.session, d.args.input = is_session, input_kind
                d.args.idle_minutes = 30
                with patch.object(boot.metal_root, 'stage_root', side_effect=RuntimeError('staging captured')) as stage:
                    with self.assertRaisesRegex(RuntimeError, 'staging captured'):
                        d.prepare_payload()
                self.assertEqual(stage.call_args.kwargs['replug'], not is_session)
                self.assertEqual(stage.call_args.kwargs['idle_minutes'], 30 if is_session else None)
        finally:
            fixture.tearDown()

    def test_staged_shadow_permissions_and_target_and_malformed_hash_refusal(self):
        helper = os.environ.get('CONSOLE_HASH_HELPER', str(Path(__file__).resolve().parents[1] / 'target/host-test-tools/sha512-crypt'))
        prep_message = ('Missing or stale console hash helper: run scripts/build-host-test-tools.sh '
                        '(CONSOLE_HASH_HELPER may select an existing executable)')
        self.assertTrue(helper and Path(helper).is_file() and os.access(helper, os.X_OK),
                        prep_message)
        hasher = Path(helper)
        if 'CONSOLE_HASH_HELPER' not in os.environ:
            source = Path(__file__).resolve().parents[1] / 'scripts/metal/sha512-crypt.rs'
            self.assertGreaterEqual(hasher.stat().st_mtime_ns, source.stat().st_mtime_ns,
                                    prep_message)
        prefix = Path('/opt/homebrew/opt/e2fsprogs/sbin')
        debugfs = shutil.which('debugfs') or str(prefix / 'debugfs')
        mke2fs = shutil.which('mke2fs') or str(prefix / 'mke2fs')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tree = root / 'tree'
            for name in ('etc/breenix', 'sbin', 'bin'): (tree / name).mkdir(parents=True)
            for name in ('sbin/init', 'bin/getty', 'bin/login'): (tree / name).write_bytes(b'\x7fELFfixture')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '4096'], check=True, capture_output=True)
            site = self.driver().site
            facts = metal_root.stage_root(image, 'console', None, site, hasher=hasher)
            self.assertEqual(facts['mode'], 'console')
            def read(command):
                return subprocess.run([debugfs, '-R', command, str(image)], check=True, capture_output=True).stdout
            self.assertEqual(read('cat /etc/breenix/boot-target'), b'console\n')
            metal_root.stage_root(image, 'console', None, site, hasher=hasher, replug=True)
            self.assertEqual(read('cat /etc/breenix/boot-target'), b'console replug\n')
            metal_root.stage_root(image, 'console', None, site, hasher=hasher, idle_minutes=2)
            self.assertEqual(read('cat /etc/breenix/boot-target'), b'console idle=2\n')
            with self.assertRaisesRegex(ValueError, 'measured console'):
                metal_root.stage_root(image, 'console', None, site, hasher=hasher, idle_minutes=2, replug=True)

            for fault in ('reset-device', 'short-read', None):
                metal_root.stage_root(image, 'console', None, site, hasher=hasher, xhci_fault=fault)
                self.assertEqual(read('cat /etc/breenix/xhci-fault'),
                                 b'' if fault is None else (fault + '\n').encode())
            with self.assertRaisesRegex(ValueError, 'fault-test'):
                metal_root.stage_root(image, 'console', None, site, hasher=hasher,
                                      xhci_fault='short-read', replug=True)

            shadow = read('cat /etc/shadow')
            self.assertRegex(shadow, rb'^root:\$6\$[./A-Za-z0-9]{16}\$[./A-Za-z0-9]{86}:')
            self.assertNotIn(site['CONSOLE_PASSWORD'].encode(), shadow)
            self.assertIn(b'Mode:  0600', read('stat /etc/shadow'))
            home = read('stat /root')
            self.assertIn(b'Type: directory', home)
            self.assertIn(b'Mode:  0700', home)
            before = image.read_bytes()
            with patch.object(metal_root.subprocess, 'run', return_value=SimpleNamespace(stdout=b'$6$malformed')):
                with self.assertRaisesRegex(ValueError, 'malformed'):
                    metal_root.stage_root(image, 'console', None, site, hasher=hasher)
            self.assertEqual(before, image.read_bytes())
            with self.assertRaises(ValueError): metal_root.console_identity({'CONSOLE_USER': 'root'})

if __name__ == '__main__': unittest.main()
