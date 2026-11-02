"""Exercise slot launch verification with distinct current and candidate kernels."""
import hashlib
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = runpy.run_path(str(ROOT / 'scripts/vm-artifacts.py'))


class VmSlotSeals(unittest.TestCase):
    def test_slot_requires_seal_and_receipt_names_selected_kernel(self):
        with tempfile.TemporaryDirectory() as name:
            repo, run = Path(name) / 'repo', Path(name) / 'run'
            repo.mkdir(); run.mkdir()

            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()

            git('init', '-q')
            git('config', 'user.name', 'Fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            git('commit', '--allow-empty', '-qm', 'fixture')
            for path in ('kernel', 'candidate', 'system.img', 'home.img', 'power-token'):
                (run / path).write_bytes(path.encode())
            ARTIFACTS['seal'](repo, run, git('rev-parse', 'HEAD'))

            def entry(kernel, userland):
                return dict(kernel=kernel, kernelSha256=hashlib.sha256((run / kernel).read_bytes()).hexdigest(), userland=userland)

            record = dict(current=entry('kernel', 'gen-a'), previous=None, next=entry('candidate', 'gen-b'))
            path = run / 'slots.json'
            path.write_text(json.dumps(record))
            with self.assertRaises((ValueError, FileNotFoundError)):
                ARTIFACTS['verify_slot'](repo, run, 'next')
            ARTIFACTS['seal_slot'](repo, run, run / 'kernel')
            with self.assertRaisesRegex(ValueError, 'not sealed'):
                ARTIFACTS['verify_slot'](repo, run, 'next')
            ARTIFACTS['seal_slot'](repo, run, run / 'candidate')
            for slot, userland in (('current', 'gen-a'), ('next', 'gen-b')):
                receipt = ARTIFACTS['verify_slot'](repo, run, slot)
                expected = (run / record[slot]['kernel']).read_bytes()
                self.assertEqual((run / 'boot-kernel').read_bytes(), expected)
                self.assertEqual(receipt['artifacts']['kernel'], record[slot]['kernelSha256'])
                self.assertTrue(receipt['evidenceEligible'])
                self.assertEqual((run / 'boot-intent').read_bytes(), f'BREENIX-BOOT 2\ntarget persistent\nuserland {userland}\n'.encode())
            # Updating the record's digest must not substitute for sealing bytes.
            (run / 'candidate').write_bytes(b'replaced')
            record['next'] = entry('candidate', 'gen-b')
            path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, 'not sealed'):
                ARTIFACTS['verify_slot'](repo, run, 'next')


if __name__ == '__main__':
    unittest.main()
