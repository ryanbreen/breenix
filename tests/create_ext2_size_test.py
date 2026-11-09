"""Exercise the image builder's size selection before filesystem creation."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ImageSizeTest(unittest.TestCase):
    def select(self, payload_mb, *args):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / 'scripts'
            scripts.mkdir()
            (scripts / 'userspace-manifest.py').write_text('')
            installer = scripts / 'install-libc-busybox.sh'
            installer.write_text('#!/bin/sh\nexit 0\n')
            installer.chmod(0o755)
            (root / 'userspace/programs').mkdir(parents=True)
            (root / 'fonts').mkdir()
            frames = root / 'docs/suites/frames'
            frames.mkdir(parents=True)
            (frames / 'PROVENANCE.json').write_text('{}')
            with (root / 'userspace/programs/payload.elf').open('wb') as payload:
                payload.truncate(payload_mb * 1024 * 1024)
            source = (ROOT / 'scripts/create_ext2_disk.sh').read_text()
            prefix = source.split('# Ensure target directory exists', 1)[0]
            script = scripts / 'create_ext2_disk.sh'
            script.write_text(prefix + '\nprintf "SELECTED=%s\\n" "$SIZE_MB"\n')
            return subprocess.run(['bash', str(script), *args], capture_output=True, text=True)

    def test_default_grows_to_fit_payload(self):
        result = self.select(176)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('SELECTED=260', result.stdout)

    def test_small_payload_keeps_default_floor(self):
        result = self.select(1)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('SELECTED=256', result.stdout)

    def test_explicit_shortfall_still_refused(self):
        result = self.select(176, '--size', '256')
        self.assertEqual(result.returncode, 1)
        self.assertIn('shortfall: 4MB', result.stderr)
        self.assertNotIn('SELECTED=', result.stdout)


if __name__ == '__main__':
    unittest.main()
