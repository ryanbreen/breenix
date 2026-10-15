#!/usr/bin/env python3
"""Exercise the real runner with fake build/VM tools; never start a VM."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class SuppliedDiskTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='breenix-run-disk-')
        self.root = Path(self.temp.name)
        shutil.copyfile(ROOT / 'run.sh', self.root / 'run.sh')
        for directory in ('scripts', 'docker/qemu/lib', 'target/aarch64-breenix-kernel/release',
                          'target/release/build/breenix-test/out', 'target/ovmf/x64', 'bin'):
            (self.root / directory).mkdir(parents=True)
        (self.root / 'scripts/host-slots.sh').write_text('''
host_slots_start() { :; }
host_slot_acquire() { :; }
host_slot_quiet() { :; }
''')
        (self.root / 'docker/qemu/lib/qemu-host-lock.sh').write_text('''
qemu_host_lock_acquire() { :; }
qemu_host_lock_track_pid() { :; }
''')
        (self.root / 'scripts/qemu-arm64.py').write_text('''import subprocess, sys
sys.exit(subprocess.call(sys.argv[1:]))
''')
        for artifact in ('aarch64-breenix-kernel/release/kernel-aarch64',
                         'release/build/breenix-test/out/breenix-uefi.img', 'ovmf/x64/code.fd',
                         'ovmf/x64/vars.fd', 'ext2-home-aarch64.img', 'ext2-home.img'):
            (self.root / 'target' / artifact).write_bytes(b'existing boot artifact')
        self.source = self.root / 'supplied image.ext2'
        data = bytearray(b'SUPPLIED' + bytes(4096))
        data[1080:1082] = b'\x53\xef'
        self.source.write_bytes(data)
        self.original = hashlib.sha256(data).digest()
        self.write_tool('cargo', '''import os
from pathlib import Path
assert os.environ['CARGO_BUILD_JOBS'] == '4'
Path('build-called').write_text('yes')
''')
        self.write_tool('taskpolicy', '''import os, sys
assert sys.argv[1] == '-b'
os.execvp(sys.argv[2], sys.argv[2:])
''')
        for qemu in ('qemu-system-aarch64', 'qemu-system-x86_64'):
            self.write_tool(qemu, '''from pathlib import Path
import sys
session = Path('target/ext2-session.img')
assert session.read_bytes().startswith(b'SUPPLIED')
assert any('file=' + str(session.resolve()) in arg for arg in sys.argv)
session.write_bytes(b'VM writes go to the session copy')
Path('vm-called').write_text('yes')
''')
        self.env = {**os.environ, 'PATH': str(self.root / 'bin') + ':' + os.environ['PATH']}

    def tearDown(self):
        self.temp.cleanup()

    def write_tool(self, name, body):
        path = self.root / 'bin' / name
        path.write_text('#!/usr/bin/env python3\n' + body)
        path.chmod(0o755)

    def run_runner(self, *args):
        return subprocess.run(['/bin/bash', str(self.root / 'run.sh'), *args], cwd=self.root,
                              env=self.env, capture_output=True, text=True, timeout=15)

    def test_supplied_image_skips_userspace_and_preserves_source(self):
        for arch in ('--arm64', '--x86'):
            with self.subTest(arch=arch):
                result = self.run_runner(arch, '--headless', '--no-audio', '--disk', str(self.source))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue((self.root / 'build-called').is_file())
                self.assertTrue((self.root / 'vm-called').is_file())
                self.assertEqual(hashlib.sha256(self.source.read_bytes()).digest(), self.original)
                self.assertIn('Creating fresh session disk', result.stdout)

    def test_no_build_skips_kernel_too(self):
        result = self.run_runner('--no-build', '--headless', '--no-audio', '--disk', str(self.source))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / 'build-called').exists())
        self.assertTrue((self.root / 'vm-called').is_file())
        self.assertEqual(hashlib.sha256(self.source.read_bytes()).digest(), self.original)

    def test_bad_image_fails_before_build_or_vm(self):
        self.source.write_bytes(b'not an ext2 image')
        result = self.run_runner('--disk', str(self.source))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('not an ext2 image', result.stderr)
        self.assertFalse((self.root / 'build-called').exists())
        self.assertFalse((self.root / 'vm-called').exists())


if __name__ == '__main__':
    unittest.main()
