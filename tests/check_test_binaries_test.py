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
            for directory in ('bin', 'usr/lib', 'usr/local/test/real-bin'):
                (tree / directory).mkdir(parents=True)
            (tree / 'usr/local/test/bin').symlink_to('real-bin')
            (tree / 'bin/in_bin').write_bytes(b'\x7fELF')
            (tree / 'usr/local/test/bin/in_test_bin').write_bytes(b'\x7fELF')
            (tree / 'usr/lib/real').write_bytes(b'\x7fELF')
            (tree / 'bin/empty').write_bytes(b'')
            (tree / 'bin/relative').symlink_to('../usr/lib/real')
            (tree / 'bin/absolute').symlink_to('/usr/lib/real')
            (tree / 'bin/slow').symlink_to('/usr/lib' + '/../lib' * 12 + '/real')
            (tree / 'bin/dangling').symlink_to('/usr/lib/gone')
            (tree / 'bin/loop').symlink_to('loop')
            (tree / 'bin/missing_parent').symlink_to('/nonexistent/../usr/lib/real')
            (tree / 'bin/file_parent').symlink_to('/usr/lib/real/../real')
            (tree / 'bin/directory').mkdir()
            (tree / 'bin/empty_link').symlink_to('empty')
            for name in ('empty', 'directory', 'empty_link', 'dangling', 'loop'):
                (tree / 'usr/local/test/real-bin' / name).write_bytes(b'\x7fELF')
            disk = Path(scratch) / 'disk.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), str(disk), '4M'], check=True)
            names = ['in_bin', 'in_test_bin', 'relative', 'absolute', 'slow', 'empty', 'directory',
                     'empty_link', 'dangling', 'loop', 'missing_parent', 'file_parent', 'absent']
            dirs = ['/bin', '/usr/local/cbin', '/usr/local/test/bin']
            self.assertEqual(check.missing_binaries(disk, names, dirs, debugfs),
                             ['empty', 'directory', 'empty_link', 'missing_parent', 'file_parent', 'absent'])

            result = subprocess.run([sys.executable, str(SCRIPT), str(disk)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('fifo_fs_test', result.stderr)

    def test_required_binaries_through_directory_links_and_shadowing(self):
        debugfs = check.debugfs_path()
        mke2fs = str(Path(debugfs).with_name('mke2fs'))
        names = check.required_binaries()
        with tempfile.TemporaryDirectory() as scratch:
            tree = Path(scratch) / 'tree'
            (tree / 'bin').mkdir(parents=True)
            (tree / 'usr/real-bin').mkdir(parents=True)
            (tree / 'usr/local').symlink_to('locations')
            (tree / 'usr/locations/test').mkdir(parents=True)
            (tree / 'usr/locations/test/bin').symlink_to('../../real-bin')
            for name in names:
                (tree / 'usr/real-bin' / name).write_bytes(b'\x7fELF')
            disk = Path(scratch) / 'disk.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), str(disk), '4M'], check=True)
            result = subprocess.run([sys.executable, str(SCRIPT), str(disk)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(f'all {len(names)} testing binaries', result.stdout)

            (tree / 'bin/fifo_fs_test').write_bytes(b'')
            disk.unlink()
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), str(disk), '4M'], check=True)
            result = subprocess.run([sys.executable, str(SCRIPT), str(disk)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('lacks 1 of', result.stderr)
            self.assertIn('fifo_fs_test', result.stderr)

            (tree / 'bin/fifo_fs_test').unlink()
            (tree / 'usr/real-bin/fifo_fs_test').unlink()
            (tree / 'bin/fifo_fs_test').symlink_to('/nonexistent/../usr/real-bin/in_bin')
            (tree / 'usr/real-bin/in_bin').write_bytes(b'\x7fELF')
            disk.unlink()
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), str(disk), '4M'], check=True)
            result = subprocess.run([sys.executable, str(SCRIPT), str(disk)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('lacks 1 of', result.stderr)
            self.assertIn('fifo_fs_test', result.stderr)


if __name__ == '__main__':
    unittest.main()
