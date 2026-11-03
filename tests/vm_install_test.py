#!/usr/bin/env python3
"""Exercise the install console protocol using recorded guest messages."""
import hashlib
import importlib.util
import json
import tempfile
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('vm_install', Path(__file__).resolve().parents[1] / 'scripts/vm_install.py')
install = importlib.util.module_from_spec(spec)
spec.loader.exec_module(install)


class InstallTranscriptTest(unittest.TestCase):
    def test_answer_once_and_complete(self):
        protocol = install.InstallProtocol('old', 'prior', 'pending')
        sent = []
        for line in ('booting', '[persistent] recovered slot references needed',
                     '[persistent] recovered slot references needed',
                     '[persistent] install complete: files=1 dirs=2 links=0 hard-links=0 bytes=3 left-out=0'):
            protocol.feed(line, sent.append)
        self.assertEqual(sent, [b'BREENIX-SLOTS-1 old prior pending\n'])
        self.assertEqual(protocol.outcome, 'success')

    def test_failure_and_empty_slots(self):
        protocol = install.InstallProtocol('flat', 'empty', 'empty')
        sent = []
        protocol.feed('[persistent] recovered slot references needed', sent.append)
        protocol.feed('[persistent] install failed: copy failed', sent.append)
        self.assertEqual(sent, [b'BREENIX-SLOTS-1 flat empty empty\n'])
        self.assertEqual(protocol.outcome, 'failure')

    def test_prepare_and_publish_sealed_candidate(self):
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            image, kernel = b'candidate image', b'candidate kernel'
            (run / 'candidate.img').write_bytes(image)
            (run / 'candidate-kernel').write_bytes(kernel)
            candidate = {'generation': 'vm-new', 'imageSha256': hashlib.sha256(image).hexdigest(),
                         'kernelSha256': hashlib.sha256(kernel).hexdigest()}
            (run / 'candidate.json').write_text(json.dumps(candidate))
            current = {'kernel': 'old-kernel', 'kernelSha256': 'a' * 64, 'userland': 'old'}
            (run / 'slots.json').write_text(json.dumps({'current': current, 'previous': None, 'next': None}))
            install.prepare(run)
            self.assertEqual((run / 'boot-intent').read_text(),
                             'BREENIX-BOOT 2\ntarget persistent-install\nuserland vm-new\nprevious-userland old\n')
            self.assertEqual((run / 'install-references').read_text(), 'old empty empty')
            self.assertEqual((run / 'install-root.img').read_bytes(), image)
            install.publish(run)
            record = install.publish_record(run)
            self.assertEqual(record['current'], current)
            self.assertEqual(record['next']['userland'], 'vm-new')
            (run / 'candidate.img').write_bytes(b'tampered')
            with self.assertRaises(ValueError):
                install.prepare(run)


if __name__ == '__main__':
    unittest.main()
