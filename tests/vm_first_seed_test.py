"""Publish and promote a first seed without booting a VM."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
INSTALL = runpy.run_path(str(ROOT / 'scripts/vm_install.py'))
SLOTS = runpy.run_path(str(ROOT / 'scripts/vm_slots.py'))


class FirstSeedTest(unittest.TestCase):
    def test_first_install_and_later_promotion_preserve_whole_record(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            first = None
            for generation in ('seed-1', 'seed-2'):
                image, kernel = b'root image', generation.encode()
                (run / 'candidate.img').write_bytes(image)
                (run / 'candidate-kernel').write_bytes(kernel)
                (run / 'candidate.json').write_text(json.dumps(dict(
                    generation=generation, imageSha256=hashlib.sha256(image).hexdigest(),
                    kernelSha256=hashlib.sha256(kernel).hexdigest())))
                INSTALL['prepare'](run)
                self.assertEqual((run / 'install-references').read_text(),
                                 'flat empty empty' if first is None else 'seed-1 empty empty')
                INSTALL['publish'](run)
                record = INSTALL['publish_record'](run)
                candidate = record['next']
                SLOTS['write'](run, record)
                SLOTS['promote'](run)
                self.assertEqual(json.loads((run / 'slots.json').read_text()),
                                 dict(current=candidate, previous=first, next=None))
                self.assertEqual(SLOTS['select'](run, 'current'), candidate)
                first = candidate

    def test_missing_or_incomplete_established_current_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            candidate = dict(kernel='kernel-next', kernelSha256='a' * 64, userland='seed-3')
            for current, previous in ((None, candidate), ({}, None),
                                      (dict(kernel='kernel-old'), candidate)):
                with self.subTest(current=current, previous=previous):
                    record = dict(current=current, previous=previous, next=candidate)
                    SLOTS['write'](run, record)
                    with self.assertRaises(ValueError):
                        SLOTS['promote'](run)
                    self.assertEqual(SLOTS['read'](run), record)


if __name__ == '__main__':
    unittest.main()
