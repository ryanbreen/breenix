"""Exercise the shared image installer with real files and inode checks."""
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

    def test_missing_binary_fails_before_image_creation(self):
        (self.binaries / 'libc-busybox.elf').unlink()
        self.assertNotEqual(self.install().returncode, 0)
        self.assertFalse(self.image.exists())


if __name__ == '__main__':
    unittest.main()
