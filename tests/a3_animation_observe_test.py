"""Force retention of the slow acquire and host contention evidence, without a boot."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
from suite_collection_test import run_rust
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class AnimationObserve(unittest.TestCase):
    def test_input_window_freezes_first_over_75ms_with_phase_and_cpu_wait_intervals(self):
        source = (ROOT / 'kernel/src/window/accounting.rs').read_text().replace('//!', '//').replace('use alloc::', 'use std::')
        run_rust(self, source + r'''
#[test] fn first_slow_input_is_immutable() {
 let trace = Accounting::<128,16,4>::new();
 trace.state(1,7,10,1,0,0);
 trace.reset_input();
 trace.begin_input(100,7,2);
 trace.acquire_phase(110,7,10,1,1,127,20);
 trace.acquire_phase(120,7,10,7,1,801,30);
 trace.state(130,7,10,2,1,801);
 trace.state(160,7,10,0,0,0);
 trace.state(180,7,10,1,0,0);
 trace.acquire_phase(190,7,10,7,0,801,40);
 trace.acquire_phase(200,7,10,1,0,127,50);
 assert!(!trace.finish_input(75_000_100), "exactly 75ms must not freeze");
 trace.begin_input(100_000_000,7,2);
 trace.acquire_phase(100_000_010,7,10,7,1,802,60);
 trace.state(100_000_020,7,10,2,1,802);
 trace.state(170_000_020,7,10,0,0,0);
 trace.state(180_000_020,7,10,1,0,0);
 trace.acquire_phase(180_000_030,7,10,7,0,802,70);
 assert!(trace.finish_input(180_000_100));
 // Later frames, inputs and compositor completions cannot replace the frozen window.
 trace.begin_input(200_000_000,7,2);
 trace.begin(210_000_000,99,2);
 trace.end(220_000_000,7);
 trace.acquire_phase(230_000_000,7,10,7,1,999,90);
 let out=trace.text(240_000_000);
 assert!(out.contains("account-input frozen=1 begin_ns=100000000 end_ns=180000100"),"{out}");
 assert!(out.contains("name=virgl-copy-completion edge=1 object=802 cpu_ns=60"),"{out}");
 assert!(!out.contains("object=999"),"{out}");
 assert!(out.contains("account-interval window=2 pid=7 tid=10 begin_ns=100000020 end_ns=170000020 mode=2 kind=1 object=802"),"{out}");
 assert!(out.contains("on_cpu_ns=100 runnable_ns=10000000 blocked_ns=70000000"),"{out}");
 trace.close_input();
 trace.begin(250_000_000,99,2); trace.end(250_000_100,99);
 assert!(trace.text(260_000_000).contains("pid=99 begin_ns=250000000"));
}
''')
        ops = (ROOT / 'kernel/src/window/ops.rs').read_text()
        gpu = (ROOT / 'kernel/src/gpu/window_targets.rs').read_text()
        for name in ('pending-front', 'free-buffer', 'replay-copy', 'seal', 'tree-protect'):
            self.assertIn('"' + name + '"', item(ops, 'pub fn acquire('))
        for name in ('source-ready', 'dependency', 'virgl-copy-completion', 'source-attach',
                     'context-busy', 'context-inflight', 'target-writer', 'target-reader',
                     'source-writer', 'accepted-writer', 'sync-reader'):
            self.assertIn('"' + name + '"', gpu)
        suite = item((ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text(), 'fn input_latency(')
        for command in ('input-reset', 'input-begin', 'input-end', 'input-close'):
            self.assertIn(command, suite)
        self.assertIn('/proc/breenix/window-stall', suite)

    def test_performance_samples_cover_whole_case_and_target_qemu_not_wrapper(self):
        sampler = load('qemu-case-sample')
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / 'serial'
            pidfile = Path(directory) / 'pid'; pidfile.write_text('31337\n')
            children = []
            def spawn(argv, **kwargs):
                self.assertEqual(argv[:4], ['/usr/bin/sample', '31337', '3', '1'])
                child = Mock(pid=500 + len(children)); child.poll.return_value = 0
                child.wait.return_value = 0; children.append(child); return child
            with patch.object(sampler.subprocess, 'Popen', side_effect=spawn):
                watcher = sampler.CaseSamples(99999, serial, system='Darwin', pid_file=pidfile)
                watcher.observe('CASE window-manager performance/latency-under-animation START\n')
                self.assertEqual(len(children), 1, 'performance case has no macOS QEMU sample')
                watcher.tick(); self.assertEqual(len(children), 2, 'sample must continue beyond three seconds')
                watcher.observe('CASE window-manager performance/latency-under-animation END\n')
                watcher.tick(); self.assertEqual(len(children), 2)
                watcher.close()
            records = [json.loads(s) for s in (watcher.directory / 'samples.jsonl').read_text().splitlines()]
            self.assertTrue(all(r['qemu_pid'] == 31337 for r in records))
            gate = load('a3-proof-gate')
            self.assertIn('acquire-phase', gate.KERNEL_LABELS)
            self.assertIn('input-reset', gate.SHARED_SUITE_LABELS)
            self.assertTrue(callable(gate.check_observers))

    def test_load_sampler_writes_timestamped_cpu_top_ten_each_tick(self):
        gate = load('a3-proof-gate')
        rows = '\n'.join(f'{pid} {pid / 2:.1f} 00:01.00 /Applications/process {pid}' for pid in range(1, 13))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'load.jsonl'
            def stop(_): raise InterruptedError('end fixture')
            with patch.object(gate.os, 'kill'), patch.object(gate, 'read_load', return_value=(8, '{ 8 7 6 }')), \
                    patch.object(gate, 'output', return_value=rows) as ps, \
                    patch.object(gate.time, 'sleep', side_effect=stop):
                with self.assertRaises(InterruptedError): gate.sample_load(123, path)
            ps.assert_called_once_with(['/bin/ps', '-axo', 'pid=,pcpu=,time=,comm='])
            top = json.loads(path.with_name('cpu-top.jsonl').read_text().splitlines()[0])
            loadrow = json.loads(path.read_text())
            self.assertEqual(top['host_monotonic_ns'], loadrow['host_monotonic_ns'])
            self.assertEqual([p['pid'] for p in top['processes']], list(range(12, 2, -1)))
            self.assertEqual(top['processes'][0]['command'], '/Applications/process 12')
            self.assertEqual(top['processes'][0]['cpu_time'], '00:01.00')


if __name__ == '__main__': unittest.main()
