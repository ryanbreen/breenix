"""Check the real ext2 publisher's generation boundary without booting."""

import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
from metal_root import require_e2fsprogs


class PublicationTest(unittest.TestCase):
    def test_legacy_publisher_preserves_current_image_on_staged_input(self):
        mkfs, = require_e2fsprogs('mke2fs')
        with tempfile.TemporaryDirectory(prefix='e6-publication-') as directory:
            root = Path(directory)
            seed = root / 'seed.ext2'
            subprocess.run([mkfs, '-q', '-t', 'ext2', '-F', str(seed), '2048'],
                           check=True, capture_output=True)
            output = root / 'current.ext2'

            def publish(base):
                return subprocess.run(
                    [sys.executable, ROOT / 'scripts/toolchain-image.py', 'stage',
                     base, output, '--arch', 'aarch64'],
                    capture_output=True, text=True,
                )

            control = publish(seed)
            self.assertEqual(control.returncode, 0, control.stderr)
            self.assertEqual(output.read_bytes(), seed.read_bytes())
            prior = hashlib.sha256(output.read_bytes()).digest()

            staged = root / 'staged'
            staged.mkdir()
            (staged / '.breenix-generation.json').write_text(
                '{"generation":"e6","arch":"aarch64","artifacts":{}}\n')
            candidate = staged / 'candidate.ext2'
            # A distinct, otherwise acceptable image makes accidental replacement observable.
            subprocess.run([mkfs, '-q', '-t', 'ext2', '-F', str(candidate), '4096'],
                           check=True, capture_output=True)
            alias = root / 'candidate.ext2'
            alias.symlink_to(candidate)
            refused = publish(alias)
            self.assertEqual(hashlib.sha256(output.read_bytes()).digest(), prior)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn('staged E6 input', refused.stderr)
            self.assertTrue(candidate.is_file())


if __name__ == '__main__':
    unittest.main()
