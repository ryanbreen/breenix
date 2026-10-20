"""Run scripts/check-test-binaries.py against real ext2 images built with e2fsprogs."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/check-test-binaries.py'
spec = importlib.util.spec_from_file_location('check_test_binaries', SCRIPT)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class CheckTestBinariesTest(unittest.TestCase):
    def test_lists_come_from_the_kernel_sources(self):
        self.assertIn('fifo_fs_test', check.required_binaries())
        self.assertIn('/usr/local/test/bin', check.search_dirs())

    def test_follows_symlinks_and_refuses_what_the_loader_cannot_load(self):
        debugfs = check.debugfs_path()
        mke2fs = str(Path(debugfs).with_name('mke2fs'))
        with tempfile.TemporaryDirectory() as scratch:
            tree = Path(scratch) / 'tree'
            for directory in ('bin', 'usr/lib', 'usr/local/test/bin'):
                (tree / directory).mkdir(parents=True)
            (tree / 'bin/in_bin').write_bytes(b'\x7fELF')
            (tree / 'usr/local/test/bin/in_test_bin').write_bytes(b'\x7fELF')
            (tree / 'usr/lib/real').write_bytes(b'\x7fELF')
            (tree / 'bin/empty').write_bytes(b'')
            (tree / 'bin/relative').symlink_to('../usr/lib/real')
            (tree / 'bin/absolute').symlink_to('/usr/lib/real')
            (tree / 'bin/slow').symlink_to('/usr/lib' + '/../lib' * 12 + '/real')
            (tree / 'bin/dangling').symlink_to('/usr/lib/gone')
            (tree / 'bin/loop').symlink_to('loop')
            disk = Path(scratch) / 'disk.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), str(disk), '4M'], check=True)
            names = ['in_bin', 'in_test_bin', 'relative', 'absolute', 'slow', 'empty', 'dangling', 'loop', 'absent']
            dirs = ['/bin', '/usr/local/cbin', '/usr/local/test/bin']
            self.assertEqual(check.missing_binaries(disk, names, dirs, debugfs), ['empty', 'dangling', 'loop', 'absent'])

            result = subprocess.run([sys.executable, str(SCRIPT), str(disk)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('fifo_fs_test', result.stderr)


if __name__ == '__main__':
    unittest.main()
