#!/usr/bin/env python3
"""Keep space for retained VM generations when preparing an install."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TOOLS = os.environ['PATH'] + ':/opt/homebrew/opt/e2fsprogs/sbin:/usr/local/opt/e2fsprogs/sbin'


class InstallCapacityTest(unittest.TestCase):
    def test_prepare_grows_ext2_and_retains_raw_files(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            tree = run / 'tree'
            tree.mkdir()
            (tree / 'sentinel').write_bytes(b'raw system state')
            image = run / 'system.img'
            with image.open('wb') as output:
                output.truncate(16 * 1024 * 1024)
            subprocess.run([shutil.which('mke2fs', path=TOOLS), '-q', '-t', 'ext2', '-F',
                            '-d', str(tree), str(image)], check=True)
            shutil.copyfile(image, run / 'candidate.img')
            kernel = b'candidate kernel'
            (run / 'candidate-kernel').write_bytes(kernel)
            (run / 'candidate.json').write_text(json.dumps(dict(
                generation='vm-seed', imageSha256=hashlib.sha256(image.read_bytes()).hexdigest(),
                kernelSha256=hashlib.sha256(kernel).hexdigest())))
            subprocess.run([sys.executable, '-B', str(ROOT / 'scripts/vm_install.py'),
                            'prepare', str(run)], check=True)
            # Raw, current, previous, next, an in-progress copy and shared data.
            capacity = 6 * (run / 'candidate.img').stat().st_size
            self.assertGreaterEqual(image.stat().st_size, capacity)
            with image.open('rb') as source:
                source.seek(1024)
                superblock = source.read(1024)
            blocks = struct.unpack_from('<I', superblock, 4)[0]
            block_size = 1024 << struct.unpack_from('<I', superblock, 24)[0]
            self.assertGreaterEqual(blocks * block_size, capacity)
            content = subprocess.check_output([shutil.which('debugfs', path=TOOLS), '-R',
                                              'cat /sentinel', str(image)], stderr=subprocess.DEVNULL)
            self.assertEqual(content, b'raw system state')


if __name__ == '__main__':
    unittest.main()
