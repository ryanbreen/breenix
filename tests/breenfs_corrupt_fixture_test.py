"""Check the disposable checksum and metadata corruption fixtures."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'tools/breenfs/target')) / 'release'


def run(*args, check=True):
    return subprocess.run([str(a) for a in args], capture_output=True, text=True, check=check)


def flip(path, offset):
    with path.open('r+b') as image:
        image.seek(offset)
        byte = image.read(1)
        image.seek(offset)
        image.write(bytes([byte[0] ^ 1]))


class CorruptFixture(unittest.TestCase):
    def test_corruption_and_reproducibility(self):
        start = time.monotonic()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            outputs = []
            for n in range(2):
                data, node, manifest = (root / f'{name}{n}' for name in ('data', 'node', 'manifest'))
                run('bash', ROOT / 'scripts/breenfs-corrupt-fixture.sh', data, node, manifest)
                outputs.append((data, node, manifest))
            self.assertEqual(outputs[0][2].read_bytes(), outputs[1][2].read_bytes())
            for first, second in zip(outputs[0][:2], outputs[1][:2]):
                self.assertEqual(first.read_bytes(), second.read_bytes())
            data, node, manifest = outputs[0]
            rows = manifest.read_text().splitlines()
            self.assertEqual(len(rows), 2)
            m1 = re.fullmatch(r'data-block (\d+) rot/target', rows[0])
            m2 = re.fullmatch(r'node-block (\d+) rot-dir/f0001', rows[1])
            self.assertIsNotNone(m1)
            self.assertIsNotNone(m2)
            for image, addr, offset, invariant in (
                (data, int(m1[1]), 100, 'C1'),
                (node, int(m2[1]), 16383, 'N1'),
            ):
                result = run(TARGET / 'breenfs-check', '--full', image, check=False)
                errors = [line for line in (result.stdout + result.stderr).splitlines() if line.startswith('error ')]
                self.assertEqual(result.returncode, 4, result.stdout + result.stderr)
                self.assertEqual(len(errors), 1, errors)
                self.assertRegex(errors[0], rf'error {invariant}\b.*\b{addr}\b')
                owners = run(TARGET / 'breenfs-extract', 'owners', image, addr).stdout.strip()
                if invariant == 'N1':
                    self.assertIn('corrupt candidates', owners)
                    self.assertIn('/rot-dir/f0001', owners)
                else:
                    self.assertIn('data /rot/target offset=0', owners)
                flip(image, addr * 4096 + offset)
                pristine = run(TARGET / 'breenfs-check', '--full', image, check=False)
                self.assertEqual(pristine.returncode, 0, pristine.stdout + pristine.stderr)
            data_blocks = run(TARGET / 'breenfs-extract', 'blocks', data, '/rot/target').stdout.splitlines()
            self.assertEqual(data_blocks[0], f'data {m1[1]} 0')
            node_blocks = run(TARGET / 'breenfs-extract', 'blocks', node, '/rot-dir/f0001').stdout.splitlines()
            self.assertIn(f'inode-leaf {m2[1]}', node_blocks)
            owners = run(TARGET / 'breenfs-extract', 'owners', node, m2[1]).stdout.strip()
            self.assertTrue(owners.startswith('fs-node level=0 '), owners)
            paths = owners.split()[2:]
            self.assertIn('/rot-dir/f0001', paths)
            self.assertTrue(paths and all(re.fullmatch(r'/rot-dir/f\d{4}', p) for p in paths), owners)
            self.assertLess(time.monotonic() - start, 60)


if __name__ == '__main__':
    unittest.main()
