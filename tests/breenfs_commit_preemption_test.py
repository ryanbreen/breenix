#!/usr/bin/env python3
"""E24 coordinator ruling 488 corrects 51117672's counter prohibition.

The I/O entry count stays: waits park and release it, while CPU work reaches
an explicit point between bounded items. A work item is one scan entry, node,
data extent (at most 256 pages), deferred edit or device publication batch.
Node edits have format-bounded height/size; transaction-sized loops do not.
This census checks those loop boundaries, not elapsed time or guest affinity.
"""
import re
import unittest

from breenfs_mount_lock_test import BREENFS, ROOT, function, strip_comments, guard_ranges, spin_locks

# Fixed bound on pages in a dirty run / data extent between points.
MAX_WORK_ITEMS_PER_POINT = 256

# Transaction-sized loops on the worker's preparation/publication call tree.
# Each named function must contain a point; each listed loop must start with
# one, so continue/error paths cannot bypass the next scheduling opportunity.
LOOPS = {
    'vfs.rs': {
        'service_mappings': ('for node in handles',),
        'mapped_nodes': ('for entry in state.nodes.values()',),
        'retain_open': ('for times in batch.by_ref().take(RETAIN_OPEN_STEPS)',),
        'drain_pages': ('for &ino in &self.dirty_nodes',),
        'commit_generation_up_to': ('for ino in touched',
                                    'for (start, count) in prepared.data_ranges()',
                                    'for grant in grants'),
        'abandon': ('for grant in frozen.grants',),
        'materialize': ('while steps < MATERIALIZE_STEPS', 'while steps < MATERIALIZE_STEPS'),
    },
    'dirty.rs': {'write_runs': ('while let Some((first, page)) = self.pages.pop_first()',)},
    'txn.rs': {'settle': ('while let Some(change) = self.deferred.pop_front()',)},
    'commit.rs': {
        'wake': ('for waiter in &self.waiters',),
        'prepare': ('for addr in remaining', 'for (_, bytes) in free',
                    'for (chunk, crc) in bytes.chunks_exact(BLOCK_SIZE).zip(r.crcs)',
                    'for (addr, len) in ranges'),
        'fill': ('for i in 0..usize::from(h.nitems)',),
        'write_cached': ('for (&addr, bytes) in self.nodes.iter().chain(self.data.iter())',
                         'for (i, bytes) in bytes.chunks_exact(sector).enumerate()'),
    },
    'ops.rs': {'prepare_commit': (), 'reclaim_orphans': ('for ino in candidates',)},
    'btree.rs': {'range_limit': ()},
    'space.rs': {'range': ('while let Some(ptr) = todo.pop()',)},
    'recover.rs': {'space_items': ('while let Some((ptr, limit)) = todo.pop()',)},
}


class CommitPreemption(unittest.TestCase):
    def test_cpu_work_reaches_bounded_points(self):
        for file, functions in LOOPS.items():
            source = strip_comments((BREENFS / file).read_text())
            for name, loops in functions.items():
                with self.subTest(file=file, function=name):
                    code = function(source, name)
                    self.assertIn('preemption_point();', code)
                    for loop in loops:
                        pattern = re.escape(loop) + r'\s*\{\s*preemption_point\(\);'
                        self.assertRegex(code, pattern)
                        code = re.sub(pattern, '', code, count=1)
        dirty = (BREENFS / 'dirty.rs').read_text()
        self.assertIn(f'run.len() < {MAX_WORK_ITEMS_PER_POINT} * BLOCK_SIZE', dirty)
        commit = strip_comments((BREENFS / 'commit.rs').read_text())
        self.assertNotIn('txn.writes().clone()', function(commit, 'prepare'))
        vfs = strip_comments((BREENFS / 'vfs.rs').read_text())
        self.assertIn('preemption_point();', function(vfs, 'commit_due'))
        self.assertIn('preemption_point();', function(vfs, 'apply_touches'))
        # Points and calls into the instrumented core must not be under a
        # BreenFS spin lock (sleeping MountLock exclusion may remain held).
        for path in BREENFS.glob('*.rs'):
            source = strip_comments(path.read_text())
            for lock, line, held in guard_ranges(source, spin_locks(source)):
                self.assertNotIn('preemption_point(', held, f'{path}:{line} {lock}')
        # Require the batch point after submit/wait returned, outside the
        # device ownership in the driver request gate and cache operation.
        self.assertRegex(function(commit, 'write_cached'),
                         r'cache\.submit\(device, group\)\.wait\(\)\.map_err\([^;]+;\s*preemption_point\(\);')

    def test_waits_park_with_the_count_released(self):
        for file, name in [('mount_lock.rs', 'park'), ('commit_signal.rs', 'wait')]:
            code = function(strip_comments((BREENFS / file).read_text()), name)
            self.assertIn('schedule_current_wait()', code)
        wait = function(strip_comments((ROOT / 'kernel/src/task/waitqueue.rs').read_text()),
                        'schedule_current_wait')
        self.assertLess(wait.index('preempt_enable()'), wait.index('loop {'))
        self.assertGreater(wait.rindex('preempt_disable()'), wait.index('loop {'))
        # Ruling 488 retains the driver's existing sleepable entry contract.
        ahci = (ROOT / 'kernel/src/drivers/ahci/mod.rs').read_text()
        self.assertIn('schedule_current_wait()', function(strip_comments(ahci), 'begin_port_io'))
        completion = (ROOT / 'kernel/src/task/completion.rs').read_text()
        self.assertIn('preempt_enable()', completion)
        self.assertIn('preempt_disable()', completion)
        self.assertIn('schedule_from_kernel()', completion)
        mmio = (ROOT / 'kernel/src/drivers/virtio/block_mmio.rs').read_text()
        self.assertIn('schedule_current_wait()', mmio)


if __name__ == '__main__':
    unittest.main()
