#!/usr/bin/env python3
"""Kernel and command use one scrub wire definition."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]

class ScrubAbi(unittest.TestCase):
    def test_kernel_and_command_share_abi(self):
        abi = ROOT / 'kernel/src/fs/breenfs/abi.rs'
        self.assertTrue(abi.is_file(), 'missing shared scrub ABI')
        text = abi.read_text()
        self.assertIn('#[repr(C)]', text)
        self.assertNotIn('crate::', text)
        self.assertIn('pub mod abi;', (abi.parent / 'mod.rs').read_text())
        command = (ROOT / 'userspace/programs/src/breenfs.rs').read_text()
        self.assertIn('#[path = "../../../kernel/src/fs/breenfs/abi.rs"]', command)
        self.assertIn('mod abi;', command)
        self.assertIn('name = "breenfs"', (ROOT / 'userspace/programs/Cargo.toml').read_text())
        self.assertIn('"breenfs"', (ROOT / 'userspace/programs/build.sh').read_text())

if __name__ == '__main__':
    unittest.main()
