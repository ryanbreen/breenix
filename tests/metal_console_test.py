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
        driver.console_step = 0
        driver.console_done = False
        driver.final_seen = True
        driver.required_stages = []
        driver.console_intent = None
        driver.seen = set()
        driver.event = Mock()
        driver.console_send = Mock()
        return driver

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

    def test_keyboard_stage_rejects_sol_input(self):
        stage = ('metal-login-ok 42', 'line', 'keyboard')
        self.assertFalse(boot.marker_matches(stage, 'metal-login-ok 42', 'sol'))
        self.assertTrue(boot.marker_matches(stage, 'metal-login-ok 42', 'keyboard'))
        definition = existing.json.loads((existing.TOOLS.parents[1] / 'docs/metal/boot-path.json').read_text())
        login = next(m for m in definition['milestones'] if m['id'] == 'console-login')
        self.assertIn('The answer is on the screen', [s['name'] for s in login['stages']['console']])
        # A SOL boot waits for no keyboard-tagged stage, so it can finish; it still waits for the screen check.
        catalog = existing.json.loads((existing.TOOLS.parents[1] /
                                       'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json').read_text())
        groups = boot.measured_stages(definition, 'console', catalog, 'ramdisk', 'sol')
        self.assertFalse([stage for group in groups for stage in group if len(stage) == 3])
        self.assertIn(('fbcon row: metal-login-ok 42', 'contains'), [stage for group in groups for stage in group])
        self.assertEqual(len(boot.final_marker(definition, 'console', catalog, 'ramdisk', 'sol')), 2)
        keyboard = boot.measured_stages(definition, 'console', catalog, 'ramdisk', 'keyboard')
        self.assertIn(('[console] first input byte input=keyboard', 'line', 'keyboard'),
                      [stage for group in keyboard for stage in group])

    def test_console_input_options_are_validated_before_host_access(self):
        runner = existing.load_script('metal-run')
        for mode, input_kind in [('console', None), ('default', 'sol')]:
            with self.assertRaisesRegex(SystemExit, 'console mode requires'):
                runner.boot(SimpleNamespace(mode=mode, input=input_kind))

    def test_password_is_sent_on_stdin_and_redacted(self):
        driver = self.driver()
        driver.tmux = ['tmux', '-L', 'fixture']
        secret = driver.site['CONSOLE_PASSWORD']
        with patch.object(boot.subprocess, 'run') as run:
            boot.Driver.console_send(driver, secret)
        self.assertNotIn(secret, repr(run.call_args_list[0].args))
        self.assertEqual(run.call_args_list[0].kwargs['input'], (secret + '\r').encode())
        self.assertNotIn(secret, existing.m.Redactor(driver.site)('Password: ' + secret))

    def test_disk_root_refuses_an_install_with_a_different_target_or_credential(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            driver = fixture.driver
            driver.args.mode, driver.args.disk, driver.args.commit = 'console', 'root', 'a' * 40
            driver.site = self.driver().site
            inputs = driver.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            existing.m.write_json(inputs / 'build.json', {'commit': driver.args.commit, 'profile': 'production', 'features': []})
            for mode, identity in [('probe', metal_root.console_identity(driver.site)), ('console', 'wrong-identity')]:
                existing.m.write_json(existing.m.STATE / 'disk.json', {'commit': driver.args.commit,
                                      'mode': mode, 'consoleIdentity': identity})
                with self.assertRaises(boot.Finish) as result:
                    driver.prepare_payload()
                self.assertEqual(result.exception.status, existing.m.EXIT_CANCELLED)
                self.assertNotIn('diskRoot', driver.delivery)
        finally:
            fixture.tearDown()

    def test_disk_root_console_carries_per_boot_intent_without_restaging_root(self):
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
            for is_session, expected in [(True, b'BREENIX-CONSOLE 1\nsession\n'),
                                         (False, b'BREENIX-CONSOLE 1\nmeasure\n')]:
                d.args.session = is_session
                with patch.object(boot.metal_root, 'stage_root') as stage, \
                     patch.object(boot.metal_image, 'extract_esp', return_value={}), \
                     patch.object(boot.metal_image, 'assemble', side_effect=RuntimeError('assembly captured')) as assemble:
                    with self.assertRaisesRegex(RuntimeError, 'assembly captured'):
                        d.prepare_payload()
                stage.assert_not_called()
                files = assemble.call_args.args[1]
                self.assertEqual(files['ramdisk'].read_bytes(), expected)
                self.assertEqual(d.delivery['consoleIntent'], expected.split(b'\n')[1].decode())
            # The assembled image records the intent, not a RAM-disk root.
            with patch.object(boot.metal_image, 'extract_esp', return_value={}), \
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
            for is_session, is_attended, input_kind in [(False, False, 'sol'), (False, True, None),
                                                        (True, False, None)]:
                d.args.session, d.args.attended, d.args.input = is_session, is_attended, input_kind
                d.args.idle_minutes = 30
                with patch.object(boot.metal_root, 'stage_root', side_effect=RuntimeError('staging captured')) as stage:
                    with self.assertRaisesRegex(RuntimeError, 'staging captured'):
                        d.prepare_payload()
                self.assertEqual(stage.call_args.kwargs['replug'], not is_session)
                self.assertEqual(stage.call_args.kwargs['idle_minutes'], 30 if is_session else None)
        finally:
            fixture.tearDown()

    def test_staged_shadow_permissions_and_target_and_malformed_hash_refusal(self):
        hasher = Path(os.environ['CONSOLE_HASH_HELPER'])
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
