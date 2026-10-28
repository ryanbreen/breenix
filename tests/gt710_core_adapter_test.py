"""Execute the guest producer and adapter with controlled engine-release words."""
from pathlib import Path
import hashlib
import json
import runpy
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
class CoreAdapter(unittest.TestCase):
    def test_guest_producer_controls(self):
        source = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        producer = source[source.index('/// The testing producer uses'):]
        fixture = (ROOT / 'tests/fixtures/gt710_core_producer.rs').read_text().replace('// PRODUCER', producer)
        fixture = fixture.replace('../../kernel/', str(ROOT / 'kernel') + '/').replace('../kernel/', str(ROOT / 'kernel') + '/').replace('../libs/', str(ROOT / 'libs') + '/')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            (out / 'fixture.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(out / 'libgpu_abi.rlib')], check=True, cwd=ROOT)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', '--cfg', 'feature="testing"', '--extern', f'gpu_abi={out / "libgpu_abi.rlib"}', str(out / 'fixture.rs'), '-o', str(out / 'fixture')], check=True, cwd=ROOT)
            result = subprocess.run([str(out / 'fixture'), 'gk208::', '--nocapture', '--test-threads=1'], capture_output=True, text=True, check=True)
            print(result.stdout)
            # Authenticate both the intentionally failing control and clean run blocks.
            rows = []
            for line in result.stdout.splitlines():
                at = line.find('GT710_CORE ')
                if at >= 0:
                    line = line[at:]
                    rows.append(line + '\n')
                    record = json.loads(line[len('GT710_CORE '):])
                    if record['type'] == 'caps':
                        data = bytes.fromhex(record['hex'])
                        self.assertEqual(len(data), 512)
                        self.assertEqual(int.from_bytes(data[16:20], 'little'), 6)
                elif line.startswith('GT710_CORE_BULK '):
                    _, size, digest = line.split()
                    data = ''.join(rows).encode()
                    self.assertEqual(len(data), int(size))
                    self.assertEqual(hashlib.sha256(data).hexdigest(), digest)
                    with tempfile.TemporaryDirectory() as capture:
                        path = Path(capture)
                        path.joinpath('gt710-pixels.rle').write_bytes(data)
                        serial = f'{line}\nGT710_BULK {size} {digest}\n'
                        path.joinpath('serial.txt').write_text(serial)
                        join = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))['capture_serial']
                        self.assertEqual(join(path), data.decode() + serial)
                        path.joinpath('gt710-pixels.rle').write_bytes(data.replace(b'GT710_CORE ', b'GT710_XORE ', 1))
                        with self.assertRaises(ValueError): join(path)
                        path.joinpath('gt710-pixels.rle').write_bytes(data)
                        path.joinpath('serial.txt').write_text(serial.replace(digest, '0' * 64, 1))
                        with self.assertRaises(ValueError): join(path)
                    rows = []
            self.assertEqual(rows, [])
if __name__ == '__main__': unittest.main()
