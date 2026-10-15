"""Exercise the shared image installer with real files and inode checks."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BusyboxInstallTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.binaries = self.root / 'binaries'
        self.binaries.mkdir()
        self.required = self.root / 'required'
        self.required.write_text('sh\n[\ntrue\nfalse\n')
        (self.binaries / 'libc-busybox.elf').write_bytes(b'fixture ELF')
        (self.binaries / 'libc-busybox.links').write_text(self.required.read_text() + 'cat\n')
        self.image = self.root / 'image'

    def install(self):
        return subprocess.run(['sh', str(ROOT / 'scripts/install-libc-busybox.sh'),
                               str(self.binaries), str(self.required), str(self.image)],
                              capture_output=True, text=True)

    def test_all_paths_share_one_executable_inode(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        busybox = self.image / 'bin/busybox'
        self.assertEqual(busybox.stat().st_mode & 0o777, 0o755)
        paths = ['bin/sh', 'bin/[', 'bin/cat', 'bin/true', 'bin/false',
                 'sbin/true', 'sbin/false', 'usr/local/libc/bin/busybox',
                 'usr/local/libc/bin/sh', 'usr/local/libc/bin/[']
        for path in paths:
            self.assertEqual((self.image / path).stat().st_ino, busybox.stat().st_ino)

    def test_missing_required_applet_fails_before_image_creation(self):
        (self.binaries / 'libc-busybox.links').write_text('true\nfalse\n')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Required BusyBox applet not built: sh', result.stderr)
        self.assertFalse(self.image.exists())

    def test_collision_refused_before_image_creation(self):
        for name in ('cat', 'busybox', 'true'):
            with self.subTest(name=name):
                elf = self.binaries / (name + '.elf')
                elf.write_bytes(b'other executable')
                result = self.install()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('ELF collides', result.stderr)
                self.assertFalse(self.image.exists())
                elf.unlink()

    def test_utility_rust_collision_before_shared_copy(self):
        source = (ROOT / 'userspace/programs/build.sh').read_text()
        start = source.index('# Check producer names')
        end = source.index('RELEASE_DIR=', start)
        stage = self.root / 'staged'
        stage.mkdir()
        (stage / 'init.elf').write_bytes(b'new utility')
        destination = self.binaries / 'init.elf'
        destination.write_bytes(b'original Rust program')
        environment = dict(os.environ, PROJECT_ROOT=str(ROOT),
                           TESTS_DIR=str(self.binaries), UTILITY_BUILD_DIR=str(stage),
                           UTILITY_NAMES=' init ')
        result = subprocess.run(['bash', '-euc', 'STD_BINARIES=(init)\n' + source[start:end]],
                                env=environment, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('ELF output name collision: init', result.stderr)
        self.assertEqual(destination.read_bytes(), b'original Rust program')

    def test_native_mount_cleanup_on_installer_failure(self):
        source = (ROOT / 'scripts/create_ext2_disk.sh').read_text()
        start = source.index('    MOUNT_DIR=$(mktemp -d)')
        end = source.index('    # Copy remaining binaries', start)
        marker = self.root / 'unmounted'
        environment = dict(os.environ, SCRIPT_DIR=str(ROOT / 'scripts'),
                           PROJECT_ROOT=str(ROOT), USERSPACE_DIR=str(self.binaries),
                           OUTPUT_FILE=str(self.root / 'disk'), MARKER=str(marker),
                           BREENIX_REPRODUCIBLE_EXT2='0')
        # Execute the native setup and installer with mount operations modeled.
        # The incomplete fixture applet list makes the real installer fail.
        result = subprocess.run(['bash', '-euc',
            'mount() { :; }\numount() { printf "%s" "$1" > "$MARKER"; }\n' + source[start:end]],
            env=environment, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(marker.exists(), result.stderr)
        self.assertFalse(Path(marker.read_text()).exists())

    def test_missing_binary_fails_before_image_creation(self):
        (self.binaries / 'libc-busybox.elf').unlink()
        self.assertNotEqual(self.install().returncode, 0)
        self.assertFalse(self.image.exists())


if __name__ == '__main__':
    unittest.main()
