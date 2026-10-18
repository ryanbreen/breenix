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
        driver.site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345'}
        driver.fatal_at = None
        driver.console_step = 0
        driver.console_done = False
        driver.final_seen = True
        driver.required_stages = []
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
                     'print("metal-login-ok", 6*7)', 'metal-login-ok 42',
                     '[init] console session ended; restarting getty', '[getty] console acquired']:
            driver.challenge_console(text)
        self.assertFalse(driver.console_done)
        with self.assertRaises(boot.Finish) as result:
            driver.challenge_console('Breenix login: ')
        self.assertEqual(result.exception.status, 0)
        self.assertTrue(driver.console_done)
        self.assertEqual([call.args[0] for call in driver.console_send.call_args_list],
                         ['root', 'deliberately-wrong-password', 'root', driver.site['CONSOLE_PASSWORD'],
                          'print("metal-login-ok", 6*7)', 'exit'])

    def test_password_is_sent_on_stdin_and_redacted(self):
        driver = self.driver()
        driver.tmux = ['tmux', '-L', 'fixture']
        secret = driver.site['CONSOLE_PASSWORD']
        with patch.object(boot.subprocess, 'run') as run:
            boot.Driver.console_send(driver, secret)
        self.assertNotIn(secret, repr(run.call_args_list[0].args))
        self.assertEqual(run.call_args_list[0].kwargs['input'], (secret + '\r').encode())
        self.assertNotIn(secret, existing.m.Redactor(driver.site)('Password: ' + secret))

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
            shadow = read('cat /etc/shadow')
            self.assertRegex(shadow, rb'^root:\$6\$[./A-Za-z0-9]{16}\$[./A-Za-z0-9]{86}:')
            self.assertNotIn(site['CONSOLE_PASSWORD'].encode(), shadow)
            self.assertIn(b'Mode:  0600', read('stat /etc/shadow'))
            before = image.read_bytes()
            with patch.object(metal_root.subprocess, 'run', return_value=SimpleNamespace(stdout=b'$6$malformed')):
                with self.assertRaisesRegex(ValueError, 'malformed'):
                    metal_root.stage_root(image, 'console', None, site, hasher=hasher)
            self.assertEqual(before, image.read_bytes())
            with self.assertRaises(ValueError): metal_root.console_identity({'CONSOLE_USER': 'root'})

if __name__ == '__main__': unittest.main()
