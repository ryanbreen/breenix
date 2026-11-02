"""Slot transitions and sealed kernel selection for disposable VM runs."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class VmSlots(unittest.TestCase):
    def test_transitions_and_refusals(self):
        spec = importlib.util.spec_from_file_location('vm_slots', ROOT / 'scripts/vm_slots.py')
        slots = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(slots)
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            a = {'kernel': 'kernel-a', 'kernelSha256': 'a' * 64, 'userland': 'gen-a'}
            b = {'kernel': 'kernel-b', 'kernelSha256': 'b' * 64, 'userland': 'gen-b'}
            path = run / 'slots.json'
            path.write_text(json.dumps({'current': a, 'previous': None, 'next': b}))
            slots.promote(run)
            self.assertEqual(json.loads(path.read_text()), {'current': b, 'previous': a, 'next': None})
            with self.assertRaises(ValueError):
                slots.promote(run)
            path.write_text(json.dumps({'current': a, 'previous': None, 'next': b}))
            slots.rollback(run)
            self.assertEqual(json.loads(path.read_text()), {'current': a, 'previous': None, 'next': None})
            for record in ({'current': None, 'previous': None, 'next': None},
                           {'current': {'kernel': 'kernel-a'}, 'previous': None, 'next': None}):
                path.write_text(json.dumps(record))
                with self.assertRaises(ValueError):
                    slots.select(run, 'current')
            path.unlink()
            self.assertIsNone(slots.select(run, 'current'))

    def test_intent_and_sealed_kernel(self):
        spec = importlib.util.spec_from_file_location('vm_slots', ROOT / 'scripts/vm_slots.py')
        slots = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(slots)
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            kernel = run / 'kernel'
            kernel.write_bytes(b'kernel')
            digest = hashlib.sha256(b'kernel').hexdigest()
            entry = {'kernel': 'kernel', 'kernelSha256': digest, 'userland': 'gen-1'}
            (run / 'slots.json').write_text(json.dumps({'current': entry, 'previous': None, 'next': None}))
            self.assertEqual(slots.intent(slots.select(run, 'current')), b'BREENIX-BOOT 2\ntarget persistent\nuserland gen-1\n')
            kernel.write_bytes(b'tampered')
            with self.assertRaises(ValueError):
                slots.select(run, 'current')


if __name__ == '__main__':
    unittest.main()
