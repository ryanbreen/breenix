#!/usr/bin/env python3
"""Snapshot wire definitions and command surface are shared."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class SnapshotAbi(unittest.TestCase):
    def test_layout_and_constants(self):
        abi = ROOT / 'kernel/src/fs/breenfs/abi.rs'
        source = f'''#[path = "{abi}"] mod abi;
use abi::*;
fn main() {{
 assert_eq!([SNAPSHOT_CREATE, SNAPSHOT_DELETE, SNAPSHOT_STATUS], [0xb824_1401, 0xb824_1402, 0xb824_1403]);
 for x in [SNAPSHOT_CREATE, SNAPSHOT_DELETE, SNAPSHOT_STATUS] {{ assert!(![SCRUB_START, SCRUB_CANCEL, SCRUB_STATUS].contains(&x)); }}
 assert_eq!(SNAPSHOT_READ_ONLY, 1);
 assert_eq!(std::mem::size_of::<SnapshotRequest>(), 264);
 assert_eq!(std::mem::offset_of!(SnapshotRequest, flags), 0);
 assert_eq!(std::mem::offset_of!(SnapshotRequest, name_len), 4);
 assert_eq!(std::mem::offset_of!(SnapshotRequest, name), 8);
 assert_eq!(std::mem::size_of::<SnapshotStatus>(), 24);
 assert_eq!(std::mem::offset_of!(SnapshotStatus, root_id), 0);
 assert_eq!(std::mem::offset_of!(SnapshotStatus, flags), 8);
 assert_eq!(std::mem::offset_of!(SnapshotStatus, roots), 12);
 assert_eq!(std::mem::offset_of!(SnapshotStatus, dead_remaining), 16);
}}'''
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / 'check.rs'
            src.write_text(source)
            subprocess.run(['rustc', '-Awarnings', str(src), '-o', str(Path(tmp) / 'check')], check=True)
            subprocess.run([str(Path(tmp) / 'check')], check=True)

    def test_command_usage(self):
        command = (ROOT / 'userspace/programs/src/breenfs.rs').read_text()
        self.assertIn('#[path = "../../../kernel/src/fs/breenfs/abi.rs"]', command)
        for usage in ('snapshot create|clone|delete NAME PATH', 'snapshot status|wait PATH'):
            self.assertIn(usage, command)
        for name in ('SNAPSHOT_CREATE', 'SNAPSHOT_DELETE', 'SNAPSHOT_STATUS', 'dead_remaining'):
            self.assertIn(name, command)

if __name__ == '__main__':
    unittest.main()
