#!/usr/bin/env python3
"""Exercise conversion of a staged metal root through the real host tools."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_root


class BreenfsRootTest(unittest.TestCase):
    def test_staged_root_and_install_record(self):
        mke2fs, = metal_root.require_e2fsprogs('mke2fs')
        tools = Path(os.environ.get('BREENFS_TOOLS_DIR', ROOT / 'tools/breenfs/target/release'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tree = root / 'tree'
            for directory in ('etc/breenix', 'sbin', 'bin', 'mnt'):
                (tree / directory).mkdir(parents=True)
            (tree / 'sbin/suite-smoke').write_bytes(b'\x7fELFfixture')
            (tree / 'sbin/init').write_bytes(b'\x7fELFfixture')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '65536'],
                           check=True, capture_output=True)
            facts = metal_root.stage_root(image, 'suite', 'smoke', {}, root_fs='breenfs', tools_dir=tools)
            output = subprocess.check_output([str(tools / 'breenfs-extract'), str(image), 'cat', '/etc/breenix/root-fs'])
            self.assertEqual(output, b'root-fs breenfs\n')
            subprocess.run([str(tools / 'breenfs-check'), '--full', str(image)], check=True)
            with image.open('rb') as disk:
                self.assertEqual(disk.read(16 * 4096), bytes(16 * 4096))
                self.assertEqual(disk.read(8), b'BREENFSS')
            self.assertGreater(facts['extentCount'], 0)
            self.assertEqual(facts['breenfsSha256'], hashlib.sha256(image.read_bytes()).hexdigest())
            record = metal_root.write_install_record(image, 'fixture')
            with image.open('rb') as disk:
                self.assertEqual(disk.read(512), metal_root.install_record('fixture'))
            self.assertTrue(metal_root.root_record('fixture', record['bytes'], record['sha256AfterRecord'])
                            .startswith(b'BREENIX-STORAGE 1\nroot fixture '))


if __name__ == '__main__':
    unittest.main()
