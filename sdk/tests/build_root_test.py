#!/usr/bin/env python3
"""Exercise the packaging script's ownership gate and local-path scans without building an SDK."""
from pathlib import Path
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
BUILD_SDK = (ROOT / 'scripts/build-sdk.sh').read_text()
BOOTSTRAP = BUILD_SDK.split("<<'PYBASE'\n", 1)[1].split('\nPYBASE', 1)[0]
LOCAL_PATH_SCAN = BUILD_SDK.split('LOCAL_PATH_SCAN="$(cat <<\'PY\'\n', 1)[1].split('\nPY\n', 1)[0]


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


class LocalPathScanTests(unittest.TestCase):
    """A path the pinned Rust library's doc comments write (std's docs show
    /root/secret_file.txt) passes in the sysroot's rlibs, where rustc keeps doc text, and
    nowhere else; a path its code writes passes nowhere."""
    HOME = '/root'
    WRITTEN = b'/root/secret_file.txt'
    CODE = b'/root/code_path'

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='breenix-path-scan-')
        self.addCleanup(temp.cleanup)
        self.temp = Path(temp.name)
        self.library = self.temp / 'library'
        self.library.mkdir()
        (self.library / 'fs.rs').write_bytes(b'/// let f = File::open("' + self.WRITTEN + b'")?;\n'
                                             b'const P: &str = "' + self.CODE + b'";\n')
        self.sdk = self.temp / 'sdk'
        rlib = self.sdk / 'lib/rust/lib/rustlib/x86_64-breenix/lib/libstd-0.rlib'
        rlib.parent.mkdir(parents=True)
        rlib.write_bytes(b'rmeta File::open("' + self.WRITTEN + b'")')
        (self.sdk / 'bin').mkdir()

    def driver(self, text):
        (self.sdk / 'bin/driver').write_bytes(b'#!/bin/sh\n' + text + b'\n')

    def build_sdk_scan(self):
        paths = self.temp / 'library-paths'
        for mode, tree in (('record', self.library), ('scan', self.sdk)):
            ran = subprocess.run([sys.executable, '-c', LOCAL_PATH_SCAN, mode, str(tree), str(paths), self.HOME],
                                 capture_output=True, text=True)
            if mode == 'record':
                self.assertEqual(ran.returncode, 0, ran.stderr)
        return ran.returncode

    def suite_scan(self):
        spec = importlib.util.spec_from_file_location('sdk_suite', ROOT / 'scripts/sdk-suite.py')
        suite = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(suite)
        needle = self.HOME.encode()
        written = suite.whole_paths(suite.doc_text((self.library / 'fs.rs').read_bytes()), needle)
        return suite.scan(self.sdk, [needle], written)

    def test_library_text_passes_in_the_sysroot_rlibs(self):
        self.driver(b'exec /usr/local/bin/app "$@"')
        self.assertEqual(self.build_sdk_scan(), 0)
        self.assertEqual(self.suite_scan(), {})

    def test_library_text_fails_anywhere_else(self):
        self.driver(b'exec ' + self.WRITTEN + b' "$@"')
        self.assertNotEqual(self.build_sdk_scan(), 0)
        self.assertEqual(self.suite_scan(), {self.HOME.encode(): ['bin/driver']})

    def test_library_code_text_fails_in_the_rlibs(self):
        self.driver(b'exec /usr/local/bin/app "$@"')
        rlib = next(self.sdk.rglob('*.rlib'))
        rlib.write_bytes(rlib.read_bytes() + b' ' + self.CODE)
        self.assertNotEqual(self.build_sdk_scan(), 0)
        self.assertEqual(list(self.suite_scan()), [self.HOME.encode()])

    def test_other_home_paths_fail_in_the_rlibs(self):
        self.driver(b'exec /usr/local/bin/app "$@"')
        rlib = next(self.sdk.rglob('*.rlib'))
        rlib.write_bytes(rlib.read_bytes() + b' /root/.cargo/registry/src')
        self.assertNotEqual(self.build_sdk_scan(), 0)
        self.assertEqual(list(self.suite_scan()), [self.HOME.encode()])

    def test_home_roots_and_bare_tildes_are_not_paths(self):
        for home in ('~', '/root', '/Users/', '/home/'):
            with self.subTest(home=home):
                self.HOME = home
                self.driver(b'// ~ ~100 ~/ /Users/ /home/ root:x:0:0:root:/root:/bin/bsh')
                self.assertEqual(self.build_sdk_scan(), 0)
                self.assertEqual(self.suite_scan(), {})

    def test_real_home_paths_fail_in_sources_drivers_and_rlibs(self):
        for home, path in (('~', b'~/.ssh/authorized_keys'),
                           ('~', '~/\u00e9/project'.encode()),
                           ('/root', b'/root/.cargo/registry/src'),
                           ('/Users/', b'/Users/builder/project'),
                           ('/home/', b'/home/builder/project')):
            with self.subTest(home=home):
                self.HOME = home
                for relative in ('bin/driver', 'share/vendor/account.rs',
                                 'lib/rust/lib/rustlib/x86_64-breenix/lib/libleak-0.rlib'):
                    with self.subTest(file=relative):
                        target = self.sdk / relative
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_bytes(b'/// local path: ' + path)
                        self.assertNotEqual(self.build_sdk_scan(), 0)
                        self.assertEqual(self.suite_scan(), {home.encode(): [relative]})
                        target.unlink()


if __name__ == '__main__':
    unittest.main()
