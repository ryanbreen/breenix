#!/usr/bin/env python3
"""E24 ledger 485(i): commit work must leave CPU0 available to other threads.

The worker is cfg(not(test)), so VFS host conformance cannot see its outer
preemption mask. Check the production loop and its commit preparation and
publication path directly. Wait-local scheduler brakes belong to MountLock
and CommitSignal; neither needs a caller-wide brake around filesystem work.
"""
import re
import unittest

from breenfs_mount_lock_test import BREENFS, function, strip_comments


class CommitPreemption(unittest.TestCase):
    def test_commit_work_does_not_mask_preemption(self):
        vfs = strip_comments((BREENFS / 'vfs.rs').read_text())
        # Include the idle wait and deadline lock in init_service, mapping
        # servicing, dirty draining, and the actual prepare/write/flush path.
        functions = (
            'init_service', 'commit_due', 'service_mappings', 'mapped_nodes',
            'complete_mapping', 'commit_generation', 'commit_generation_up_to',
            'apply_touches', 'retain_open', 'drain_dirty',
        )
        sections = [(f'vfs.rs::{name}', function(vfs, name)) for name in functions]
        sections += [(name, strip_comments((BREENFS / name).read_text()))
                     for name in ('dirty.rs', 'ops.rs', 'txn.rs', 'commit.rs')]
        mask = re.compile(r'\b(?:preempt_disable|arch_without_interrupts|'
                          r'without_interrupts|disable_interrupts)\s*\(')
        violations = [name for name, code in sections if mask.search(code)]
        self.assertEqual(
            violations, [],
            'commit scans, preparation, I/O and waits must be preemptible; '
            'caller-wide scheduling masks found in ' + ', '.join(violations),
        )


if __name__ == '__main__':
    unittest.main()
