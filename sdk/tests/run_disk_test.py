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
        self.root = Path(self.temp.name).resolve()
        shutil.copyfile(ROOT / 'run.sh', self.root / 'run.sh')
        for directory in ('scripts', 'docker/qemu/lib', 'target/aarch64-breenix-kernel/release',
                          'target/release/build/breenix-test/out', 'target/ovmf/x64', 'bin'):
            (self.root / directory).mkdir(parents=True)
        shutil.copyfile(ROOT / 'scripts/ext2-machine.py', self.root / 'scripts/ext2-machine.py')
        shutil.copyfile(ROOT / 'scripts/x86-qemu-policy.sh', self.root / 'scripts/x86-qemu-policy.sh')
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
        # The fixture is not a git checkout; tests/effort_target_guard_test.py covers the guard.
        (self.root / 'scripts/effort-target-guard.py').write_text('''import sys
assert sys.argv[1:] == ['--repo', sys.argv[2], '--target', 'x86'], sys.argv
print('0' * 40)
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
        # The fake VM never owns the production QMP socket. Keep every host
        # scratch artifact in the fixture and preserve another runner's IPC.
        self.write_tool('rm', '''import os, sys
if '/tmp/breenix-qmp.sock' not in sys.argv[1:]:
    os.execv('/bin/rm', ['/bin/rm', *sys.argv[1:]])
''')
        self.write_tool('mktemp', '''import tempfile
from pathlib import Path
print(tempfile.mkdtemp(dir=Path.cwd()))
''')
        self.write_tool('cargo', '''import os
from pathlib import Path
assert os.environ['CARGO_BUILD_JOBS'] == '3'
Path('build-called').write_text('yes')
''')
        self.write_tool('taskpolicy', '''import os, sys
assert sys.argv[1] == '-b'
from pathlib import Path
Path('priority-called').write_text('yes')
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
        self.write_tool('debugfs', '''import os, struct, sys
sys.stdout.buffer.write(b'\\x7fELF\\x02\\x01\\x01' + bytes(11) + struct.pack('<H', int(os.environ.get('TEST_MACHINE', '183'))) + bytes(44))
''')
        self.env = {**os.environ, 'BREENIX_ALLOW_X86_QEMU': '1', 'CARGO_BUILD_JOBS': '3', 'BREENIX_DEBUGFS': str(self.root / 'bin/debugfs'), 'PATH': str(self.root / 'bin') + ':' + os.environ['PATH']}

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
                self.env['TEST_MACHINE'] = '62' if arch == '--x86' else '183'
                for marker in ('build-called', 'vm-called'):
                    (self.root / marker).unlink(missing_ok=True)
                result = self.run_runner(arch, '--headless', '--no-audio', '--disk', str(self.source))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue((self.root / 'build-called').is_file())
                self.assertTrue((self.root / 'vm-called').is_file())
                self.assertFalse((self.root / 'priority-called').exists())
                self.assertEqual(hashlib.sha256(self.source.read_bytes()).digest(), self.original)
                self.assertIn('Creating fresh session disk', result.stdout)

    def test_background_builds_are_opt_in_and_keep_caller_jobs(self):
        self.env['BREENIX_BUILD_BACKGROUND'] = '1'
        result = self.run_runner('--headless', '--no-audio', '--disk', str(self.source))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((self.root / 'build-called').is_file())
        self.assertTrue((self.root / 'vm-called').is_file())

        if os.uname().sysname == 'Darwin':
            self.assertTrue((self.root / 'priority-called').is_file())

    def test_logical_checkout_keeps_existing_session_writes(self):
        alias = self.root / 'checkout-alias'
        alias.symlink_to(self.root, target_is_directory=True)
        disk = self.root / 'target/ext2-aarch64.img'
        disk.write_bytes(b'original system')
        session = self.root / 'target/ext2-session.img'
        session.write_bytes(b'guest writes')
        (self.root / 'target/ext2-session.img.base').write_text(str(alias / 'target/ext2-aarch64.img') + '\n')
        self.write_tool('qemu-system-aarch64', "from pathlib import Path\nassert Path('target/ext2-session.img').read_bytes() == b'guest writes'\n")
        result = subprocess.run(['/bin/bash', str(alias / 'run.sh'), '--no-build',
                                 '--headless', '--no-audio'], cwd=self.root,
                                env=self.env, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('Reusing existing session disk', result.stdout)
        self.assertEqual(session.read_bytes(), b'guest writes')

    def test_no_build_skips_kernel_too(self):
        result = self.run_runner('--no-build', '--headless', '--no-audio', '--disk', str(self.source))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / 'build-called').exists())
        self.assertTrue((self.root / 'vm-called').is_file())
        self.assertEqual(hashlib.sha256(self.source.read_bytes()).digest(), self.original)

    def test_wrong_architecture_fails_before_build_or_vm(self):
        self.env['TEST_MACHINE'] = '62'
        result = self.run_runner('--arm64', '--disk', str(self.source))
        self.assertEqual(result.returncode, 2)
        self.assertIn('wrong architecture', result.stderr)
        self.assertFalse((self.root / 'build-called').exists())
        self.assertFalse((self.root / 'vm-called').exists())

    def test_bad_image_fails_before_build_or_vm(self):
        self.source.write_bytes(b'not an ext2 image')
        result = self.run_runner('--disk', str(self.source))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('not an ext2 image', result.stderr)
        self.assertFalse((self.root / 'build-called').exists())
        self.assertFalse((self.root / 'vm-called').exists())


if __name__ == '__main__':
    unittest.main()
