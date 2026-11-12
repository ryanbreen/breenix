"""Check active GPU attribution and replacement sampling without a VM."""
import json
import signal
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from a3_gpu_lock_observe_test import load
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class LockAttribution(unittest.TestCase):
    def test_active_requests_keep_origin_until_fence_return_and_export_bounded_rows(self):
        virgl = (ROOT / 'kernel/src/gpu/virgl/mod.rs').read_text()
        send = item(virgl, 'pub fn send(')
        self.assertIn('gpu_core_trace::submit(', send, 'active sends need command stamps')
        self.assertLess(send.index('gpu_core_trace::submit('), send.index('self.queue.add('))
        new = item(virgl, 'fn new(tag:')
        self.assertIn('current_accounting_identity()', new)
        self.assertIn('req.origin', send, 'deferred sends must retain the client, not kgpu')
        reap = item(virgl, 'pub fn reap(')
        self.assertIn('gpu_core_trace::finish(req.trace_seq)', reap)
        self.assertLess(reap.index('req.resp.observe()'), reap.index('gpu_core_trace::finish('))
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        doorbell = item(core, 'fn ring_doorbell(')
        self.assertLess(doorbell.index('gpu_core_trace::doorbell('), doorbell.index('write_volatile('))
        self.assertGreater(doorbell.index('gpu_core_trace::finish('), doorbell.index('write_volatile('))
        sync = item(core, 'fn queue_sync(')
        self.assertIn('r.origin = context.trace_origin', sync, 'worker warm-up must identify its client')
        trace = (ROOT / 'kernel/src/window/gpu_core_trace.rs').read_text()
        trace = trace.replace('//!', '//').replace('use alloc::', 'use std::')
        trace = trace.split('// Kernel bindings.')[0]
        slots = (ROOT / 'kernel/src/window/gpu_command_trace.rs').read_text()
        slots = slots[slots.index('// Use one CAS'):slots.index('pub struct Commands')]
        # The journal now owns a lazily initialized case archive as well. Keep
        # this check's original request/doorbell assertions on the recent ring.
        run_rust(self, 'pub mod spin { pub struct Once<T>(std::sync::OnceLock<T>); impl<T> Once<T> { '
                 'pub const fn new()->Self {Self(std::sync::OnceLock::new())} '
                 'pub fn get(&self)->Option<&T>{self.0.get()} '
                 'pub fn call_once(&self,f:impl FnOnce()->T)->&T{self.0.get_or_init(f)} } }\n'
                 'mod gpu_command_trace { use core::sync::atomic::{AtomicU64, Ordering::{Acquire,Release,Relaxed,AcqRel}};\n'
                 + slots + '}\nmod gpu_core_trace { use crate::spin;\n' + trace + '}\nuse gpu_core_trace::Commands;\n' + r'''
#[test] fn pending_fence_and_doorbell_have_distinct_boundaries() {
 let ring = Commands::<3>::new();
 ring.case_boundary(1, "unrelated-command");
 assert!(!ring.export_text().contains("gpu-core-case-window"));
 let command = ring.submit(100, 758, 995, 347, 0x207, 24, 19660, 921, 921);
 let bell = ring.doorbell(110, 0, 20, 921);
 ring.finish(bell, 274000110);
 let out = ring.text();
 assert!(out.contains("submit_ns=100 fence_return_ns=0 pid=758 tid=995 context=347 type=519 cmd_bytes=24 payload_bytes=19660 queue_seq=921 fence=921"), "{out}");
 assert!(out.contains("gpu-core-doorbell seq=2 begin_ns=110 return_ns=274000110 pid=0 tid=20 through=921"), "{out}");
 ring.finish(command, 274000200);
 assert!(ring.text().contains("submit_ns=100 fence_return_ns=274000200 pid=758 tid=995"));
 ring.submit(300000000, 7, 8, 9, 0x206, 32, 0, 922, 0);
 ring.submit(300000001, 7, 8, 9, 0x207, 24, 4, 923, 923);
 ring.finish(command, 999999999); // late return cannot replace the new reservation
 let out = ring.text();
 assert!(out.contains("gpu-core-command-ring retained=3 overwritten=1 lost=0"), "{out}");
 assert!(!out.contains("seq=1 "), "{out}");
 assert!(out.contains("queue_seq=923 fence=923"), "{out}");
}
''')
        self.assertIn('gpu_core_trace::text()', (ROOT / 'kernel/src/window/gpu_command_trace.rs').read_text())

    def test_replacement_samples_continue_past_three_seconds_until_case_end(self):
        sampler = load('replacement_samples', 'scripts/qemu-case-sample.py')
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / 'serial'
            pidfile = Path(directory) / 'qemu.pid'
            pidfile.write_text('31337\n')
            children, paths = [], []
            clock = [100.0]

            class Child:
                def __init__(self):
                    self.pid = 4200 + len(children)
                    self.done = False
                    self.stopped = False
                    self.reaped = False
                    self.signals = []
                def poll(self): return 0 if self.done else None
                def terminate(self): self.stopped = True; self.done = True
                def send_signal(self, sig):
                    self.signals.append(sig)
                    self.stopped = True; self.done = True
                def wait(self, timeout=None): self.reaped = True; return 0

            def spawn(argv, **kwargs):
                self.assertEqual(argv[:4], ['/usr/bin/sample', '31337', '3', '1'])
                child = Child()
                children.append(child)
                paths.append(argv[-1])
                return child

            with patch.object(sampler.subprocess, 'Popen', side_effect=spawn), \
                 patch.object(sampler.time, 'monotonic', side_effect=lambda: clock[0]), \
                 patch.object(sampler.time, 'monotonic_ns', side_effect=lambda: int(clock[0] * 10**9)), \
                 patch.object(sampler.time, 'time_ns', side_effect=lambda: int((clock[0] + 1000) * 10**9)):
                watcher = sampler.CaseSamples(99999, serial, system='Darwin', pid_file=pidfile)
                for case, end in [('recovery/stack-restored', 'CASE window-manager recovery/stack-restored END\n'),
                                  ('recovery/supervisor-restarts', 'SUITE window-manager CASE recovery/supervisor-restarts FAIL ms=10000 msg=watchdog\n')]:
                    before = len(children)
                    watcher.observe(f'CASE window-manager {case} START\n')
                    for _ in range(2):
                        children[-1].done = True
                        clock[0] += 3.2
                        watcher.tick()
                        self.assertEqual(len(children), before + _ + 2,
                                         'sampling must restart after 3 s while replacement work is still running')
                    # No end action until the complete serial line arrives.
                    watcher.observe(end[:-1])
                    self.assertFalse(children[-1].stopped)
                    watcher.observe('\n')
                    self.assertTrue(children[-1].stopped and children[-1].reaped)
                    self.assertEqual(children[-1].signals, [signal.SIGINT],
                                     "SIGINT retains a partial macOS sample report at CASE END")
                    watcher.tick()
                    self.assertEqual(len(children), before + 3)
                    clock[0] += 1
                watcher.close()
            self.assertTrue(all(child.reaped for child in children))
            self.assertEqual(len(paths), len(set(paths)), 'sample windows must not overwrite one another')
            records = [json.loads(line) for line in (watcher.directory / 'samples.jsonl').read_text().splitlines()]
            starts = {row['sample_pid']: row for row in records if row['event'] == 'sample-start'}
            stops = {row['sample_pid']: row for row in records if row['event'] == 'sample-stop'}
            self.assertEqual(starts.keys(), stops.keys())
            for pid, start in starts.items():
                stop = stops[pid]
                self.assertEqual(stop['window'], start['window'])
                self.assertEqual(stop['host_start_unix_ns'], start['host_unix_ns'])
                self.assertEqual(stop['host_start_monotonic_ns'], start['host_monotonic_ns'])
                self.assertGreaterEqual(stop['host_end_unix_ns'], start['host_unix_ns'])
                self.assertGreaterEqual(stop['host_end_monotonic_ns'], start['host_monotonic_ns'])
                self.assertEqual(stop['qemu_pid'], 31337)


if __name__ == '__main__':
    unittest.main()
