#!/usr/bin/env python3
"""Fixture checks for network-shares boot staging."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/metal'))
import metal_root


class SmbStagingTest(unittest.TestCase):
    def test_only_network_shares_receives_private_fixture(self):
        mke2fs, debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tree = root / 'tree'
            (tree / 'etc/breenix').mkdir(parents=True)
            (tree / 'sbin').mkdir()
            for suite in ('network-shares', 'smoke'):
                (tree / 'sbin' / ('suite-' + suite)).write_bytes(b'\x7fELFfixture')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '4096'], check=True, capture_output=True)
            fixture = {'SMB_TEST_USER': 'fixture-user', 'SMB_TEST_PASSWORD': 'fixture-secret-52',
                       'SMB_TEST_HOST': '192.0.2.42', 'SMB_TEST_PORT': '10445'}
            def read(command):
                return subprocess.run([debugfs, '-R', command, str(image)], check=True, capture_output=True)
            output = io.StringIO()
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                facts = metal_root.stage_root(image, 'suite', 'network-shares', {}, smb_config=fixture)
            cred = read('cat /etc/breenix/smb-test.cred').stdout.decode()
            self.assertEqual(cred, 'username=fixture-user\npassword=fixture-secret-52\n')
            stat = read('stat /etc/breenix/smb-test.cred').stdout.decode()
            self.assertIn('Mode:  0600', stat)
            self.assertRegex(stat, r'User:\s+0\s+Group:\s+0')
            self.assertEqual(read('cat /etc/breenix/smb-test-server').stdout.decode(), '192.0.2.42 10445\n')
            self.assertNotIn(fixture['SMB_TEST_PASSWORD'], output.getvalue() + json.dumps(facts))
            metal_root.stage_root(image, 'suite', 'smoke', {}, smb_config=fixture)
            for name in ('smb-test.cred', 'smb-test-server'):
                self.assertIn(b'File not found', read('stat /etc/breenix/' + name).stderr)


if __name__ == '__main__':
    unittest.main()
