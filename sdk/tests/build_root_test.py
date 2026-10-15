#!/usr/bin/env python3
"""Exercise the packaging script's ownership gate without building an SDK."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
BOOTSTRAP = (ROOT / 'scripts/build-sdk.sh').read_text().split("<<'PYBASE'\n", 1)[1].split('\nPYBASE', 1)[0]


class BuildRootTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='breenix-build-root-')
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name) / 'root'

    def check_root(self, foreign_owner=False):
        code = BOOTSTRAP.replace("base = '/tmp/breenix-sdk-build'", 'base = ' + repr(str(self.base)))
        if foreign_owner:
            code = 'import os\nos.getuid = lambda: ' + str(os.getuid() + 1) + '\n' + code
        return subprocess.run([sys.executable, '-c', code], capture_output=True, text=True)

    def test_new_root_is_private_and_owned(self):
        result = self.check_root()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.base.stat().st_uid, os.getuid())
        self.assertEqual(self.base.stat().st_mode & 0o777, 0o700)

    def test_existing_owned_readable_root_preserves_stable_path(self):
        self.base.mkdir(mode=0o755)
        self.assertEqual(self.check_root().returncode, 0)

    def test_foreign_owner_is_refused(self):
        self.base.mkdir(mode=0o700)
        self.assertNotEqual(self.check_root(foreign_owner=True).returncode, 0)

    def test_symlinks_and_shared_writable_roots_are_refused(self):
        target = self.base.parent / 'target'
        target.mkdir()
        self.base.symlink_to(target, target_is_directory=True)
        self.assertNotEqual(self.check_root().returncode, 0)
        self.base.unlink()
        self.base.mkdir()
        for mode in (0o777, 0o770):
            self.base.chmod(mode)
            self.assertNotEqual(self.check_root().returncode, 0)
        self.assertEqual(list(target.iterdir()), [])


if __name__ == '__main__':
    unittest.main()
