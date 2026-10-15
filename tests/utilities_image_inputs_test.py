"""The Breenix image build has no foreign libc recipe or unrecorded ELF input."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ImageInputsTests(unittest.TestCase):
    def test_build_and_install_inputs_use_only_breenix_libc(self):
        # These are Breenix's build/install recipes. Vendored Rust libc platform
        # declarations and its upstream CI are not Breenix build recipes.
        names = subprocess.check_output([
            'git', '-C', str(ROOT), 'ls-files', '-z', '--', 'scripts', '.cargo',
            'run.sh', 'xtask', 'vendor/busybox/*.config', 'userspace/**/build.sh',
            'userspace/**/Makefile', 'userspace/**/*.ld']).split(b'\0')
        for name in filter(None, names):
            path = ROOT / name.decode()
            if path.is_file():
                self.assertNotIn('musl', path.read_text().lower(), str(path.relative_to(ROOT)))

    def test_both_image_paths_validate_the_build_manifest_before_install(self):
        source = (ROOT / 'scripts/create_ext2_disk.sh').read_text()
        check = source.index('userspace-manifest.py" check --arch "$ARCH"')
        self.assertLess(check, source.index('docker run'))
        self.assertLess(check, source.index('MOUNT_DIR=$(mktemp -d)'))
        self.assertIn('sh /install-libc-busybox.sh /binaries /required-applets /mnt/ext2', source)
        self.assertIn('"$SCRIPT_DIR/install-libc-busybox.sh" "$USERSPACE_DIR"', source)
        self.assertNotIn('busybox.elf belongs', (ROOT / 'userspace/programs/build.sh').read_text())


if __name__ == '__main__':
    unittest.main()
