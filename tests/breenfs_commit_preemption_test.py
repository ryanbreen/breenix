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

from breenfs_mount_lock_test import (
    BREENFS, ROOT, block_end, function, strip_comments, guard_ranges, spin_locks,
)

# Fixed bound on pages in a dirty run / data extent between points.
MAX_WORK_ITEMS_PER_POINT = 256

# Transaction-sized loops on the worker's preparation/publication call tree.
# Each named function must contain a point; each listed loop must start with
# one, so continue/error paths cannot bypass the next scheduling opportunity.
LOOPS = {
    'vfs.rs': {
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
    def test_service_mappings_preempts_before_each_mapping(self):
        source = strip_comments((BREENFS / 'vfs.rs').read_text())
        service = function(source, 'service_mappings')
        # Check both the handle walk and the post-publication receipt walk.
        # The point must run before upgrade can skip a dead handle.
        for iterable in (r'handles\.iter\(\)\.enumerate\(\)', r'staged'):
            with self.subTest(iterable=iterable):
                head = rf'\bfor\s+[^{{}}\n]+?\s+in\s+{iterable}\s*\{{'
                point = f'({head})' + r'\s*preemption_point\(\);'
                self.assertRegex(service, point)
                no_point = re.sub(point, r'\1', service, count=1)
                with self.assertRaises(AssertionError):
                    self.assertRegex(no_point, point)

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
        waitqueue = (ROOT / 'kernel/src/task/waitqueue.rs').read_text()
        source = strip_comments(waitqueue)
        self.assertRegex(waitqueue, r'(?m)^\s*"b \{wait\}",')
        self.assertIn('wait = sym schedule_current_wait_at', source)
        self.assertIn('run_current_wait(return_pc);', function(source, 'schedule_current_wait_at'))
        non_arm_entry = re.search(r'\bpub\s+fn\s+schedule_current_wait\s*\(\)\s*\{', source)
        self.assertIsNotNone(non_arm_entry)
        self.assertIn('run_current_wait();', function(source[non_arm_entry.start():], 'schedule_current_wait'))
        wait = function(source, 'run_current_wait')
        for cfg, cpu, park in (
            ('target_arch = "aarch64"', 'per_cpu_aarch64', 'schedule_from_kernel();'),
            ('not(target_arch = "aarch64")', 'per_cpu', 'halt_blocked_current();'),
        ):
            with self.subTest(cpu=cpu):
                prefix = re.escape(strip_comments(f'#[cfg({cfg})]')) + r'\s*'
                enable = prefix + re.escape(f'crate::{cpu}::preempt_enable();')
                disable = prefix + re.escape(f'crate::{cpu}::preempt_disable();')

                def assert_park_releases_count(code):
                    loop = re.search(r'\bloop\s*\{', code)
                    self.assertIsNotNone(loop)
                    end = block_end(code, loop.end())
                    self.assertRegex(code[:loop.start()], enable)
                    self.assertNotIn('preempt_disable()', code[:loop.start()])
                    self.assertIn(park, code[loop.end():end])
                    self.assertNotRegex(code[loop.end():end], r'\bpreempt_(?:enable|disable)\(')
                    self.assertRegex(code[end + 1:], disable)

                assert_park_releases_count(wait)
                with self.assertRaises(AssertionError):
                    assert_park_releases_count(re.sub(enable, '', wait, count=1))
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
