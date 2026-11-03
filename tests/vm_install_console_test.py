#!/usr/bin/env python3
"""Replay the seed request from 20261102T105159Z-qemu over the console transport."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('vm_install', Path(__file__).resolve().parents[1] / 'scripts/vm_install.py')
install = importlib.util.module_from_spec(spec)
spec.loader.exec_module(install)


class ConsoleExchangeTest(unittest.TestCase):
    def test_seed_request_uses_paced_enter_like_metal(self):
        protocol = install.InstallProtocol('flat', 'empty', 'empty')
        chunks = []
        class Console:
            def sendall(self, data):
                chunks.append(data)
        console = Console()
        # The captured guest waited here after echoing the host's LF response.
        transcript = (
            "[persistent] install starting: the test root's userland onto the system partition at /home",
            '[persistent] recovered slot references needed',
            '',
            '[watchdog] armed seconds=120 action=hard-reset health=root-io,root-inode',
            'T9T0',
            'BREENIX-SLOTS-1 flat empty empty',
        )
        with patch.object(install.time, 'sleep') as sleep:
            for line in transcript:
                protocol.feed(line, lambda data: install.console_answer(console, data))
        self.assertEqual(b''.join(chunks), b'BREENIX-SLOTS-1 flat empty empty\r')
        self.assertTrue(chunks)
        self.assertTrue(all(0 < len(chunk) <= 4 for chunk in chunks))
        self.assertEqual(sleep.call_count, len(chunks))
        self.assertIsNone(protocol.outcome)
        protocol.feed('[persistent] install complete: files=1 dirs=2 links=0 hard-links=0 bytes=3 left-out=0', chunks.append)
        self.assertEqual(protocol.outcome, 'success')


if __name__ == '__main__':
    unittest.main()
