#!/usr/bin/env python3
"""Host checks for the SDK image editor; needs e2fsprogs, no VM or SDK build."""
import hashlib
import importlib.util
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('sdk_image', ROOT / 'sdk/libexec/image.py')
image_tool = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(image_tool)
DEBUGFS = image_tool.debugfs_tool()
SBIN = Path(DEBUGFS).parent


def elf(machine=183, interp=False, payload=b''):
    header = struct.pack('<16sHHIQQQIHHHHHH', b'\x7fELF\x02\x01\x01' + bytes(9),
                         2, machine, 1, 0x400000, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    return header + struct.pack('<IIQQQQQQ', 3 if interp else 1, 5, 0, 0x400000, 0x400000,
                                120 + len(payload), 120 + len(payload), 4096) + payload


class ImageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='sdk image checks ')
        self.root = Path(self.temp.name)
        self.disk = self.root / 'disk image.ext2'
        with self.disk.open('wb') as handle:
            handle.truncate(16 * 1024 * 1024)
        subprocess.run([str(SBIN / 'mke2fs'), '-q', '-t', 'ext2', '-F', str(self.disk)], check=True)
        self.app = self.root / 'hello'
        self.app.write_bytes(elf(payload=b'original'))
        subprocess.run([DEBUGFS, '-w', '-R', 'mkdir /bin', str(self.disk)], capture_output=True, check=True)
        subprocess.run([DEBUGFS, '-w', '-R', 'write ' + image_tool.quote(self.app) + ' /bin/busybox',
                        str(self.disk)], capture_output=True, check=True)
        self.fsck()

    def tearDown(self):
        self.temp.cleanup()

    def fsck(self):
        result = subprocess.run([str(SBIN / 'e2fsck'), '-fn', str(self.disk)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def run_tool(self, *args, ok=True):
        result = subprocess.run([sys.executable, str(ROOT / 'sdk/libexec/image.py'), *args[:1],
                                 '--image', str(self.disk), *map(str, args[1:])],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, ok, result.stdout + result.stderr)
        self.fsck()
        return result

    def digest(self):
        return hashlib.sha256(self.disk.read_bytes()).digest()

    def view(self):
        return image_tool.Image(self.disk, self.root, DEBUGFS)

    def test_install_replace_remove_registry_and_modes(self):
        self.run_tool('install', self.app)
        view = self.view()
        self.assertEqual(view.read('/usr/local/bin/hello'), self.app.read_bytes())
        view.verify_metadata('/usr/local/bin/hello', 0o100755)
        view.verify_metadata('/usr/local/bin', 0o40755)
        self.run_tool('install', '--name', 'private', '--mode', '0750', self.app)
        view.verify_metadata('/usr/local/bin/private', 0o100750)
        self.assertEqual(self.run_tool('list').stdout, 'hello\nprivate\n')
        self.app.write_bytes(elf(payload=b'replacement' * 8192))
        self.run_tool('install', self.app)
        self.assertEqual(view.read('/usr/local/bin/hello'), self.app.read_bytes())
        self.run_tool('remove', 'hello')
        self.assertIsNone(view.stat('/usr/local/bin/hello'))
        self.assertEqual(self.run_tool('list').stdout, 'private\n')
        self.run_tool('remove', 'private')
        self.assertEqual(self.run_tool('list').stdout, '')

    def test_refusals_preserve_entire_image(self):
        before = self.digest()
        for data in (b'not ELF', elf(62), elf(interp=True), elf()[:90]):
            self.app.write_bytes(data)
            self.run_tool('install', self.app, ok=False)
            self.assertEqual(self.digest(), before)
        self.run_tool('remove', 'missing', ok=False)
        self.assertEqual(self.digest(), before)
        self.app.write_bytes(elf())
        self.run_tool('install', '--name', '../escape', self.app, ok=False)
        self.run_tool('install', '--mode', '0899', self.app, ok=False)
        self.assertEqual(self.digest(), before)

    def test_no_existing_directory_inode_leaks(self):
        self.run_tool('install', self.app)
        first = self.view().stat('/usr/local/bin')
        inode = first.split()[1]
        for name in ('second', 'third'):
            self.run_tool('install', '--name', name, self.app)
            self.assertEqual(self.view().stat('/usr/local/bin').split()[1], inode)
        self.assertEqual(self.run_tool('list').stdout, 'hello\nsecond\nthird\n')

    def test_symlink_destination_refused_without_changes(self):
        self.run_tool('install', self.app)
        subprocess.run([DEBUGFS, '-w', '-R', 'symlink /usr/local/bin/link /bin/busybox', str(self.disk)],
                       check=True, capture_output=True)
        before = self.digest()
        self.run_tool('install', '--name', 'link', self.app, ok=False)
        self.assertEqual(self.digest(), before)

    def test_packaged_launcher_relocates_and_reports_version(self):
        stage = self.root / 'sdk'
        (stage / 'bin').mkdir(parents=True)
        (stage / 'share/breenix-sdk/libexec').mkdir(parents=True)
        commit = 'a' * 40
        (stage / 'VERSION').write_text('breenix-sdk 0.1.0 ' + commit + '\n')
        subprocess.run(['/bin/sh', str(ROOT / 'sdk/build/stages/50-image.sh')], check=True,
                       env={**os.environ, 'SDK_ROOT': str(ROOT), 'SDK_STAGE': str(stage)})
        moved = self.root / 'relocated'
        stage.rename(moved)
        result = subprocess.run([str(moved / 'bin/breenix-image'), '--version'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, 'breenix-sdk 0.1.0 image ' + commit + '\n')


if __name__ == '__main__':
    unittest.main()
