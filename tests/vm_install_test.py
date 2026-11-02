#!/usr/bin/env python3
"""Exercise the install console protocol using recorded guest messages."""
import importlib.util
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


if __name__ == '__main__':
    unittest.main()
