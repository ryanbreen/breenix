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

    def test_scrub_mutations_require_root_before_filesystem_dispatch(self):
        text = (ROOT / 'kernel/src/syscall/ioctl.rs').read_text()
        gate = text.index('if matches!(request, SCRUB_START | SCRUB_CANCEL)')
        dispatch = text.index('node.filesystem.ioctl(&node, request, arg)')
        self.assertLess(gate, dispatch)
        check = text[gate:dispatch]
        self.assertIn('!matches!(super::handlers::sys_geteuid(), SyscallResult::Ok(0))', check)
        self.assertIn('return SyscallResult::Err(EPERM)', check)

if __name__ == '__main__':
    unittest.main()
