"""Exercise GPU stall observation without starting QEMU or sampling a real process."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from suite_collection_test import run_rust
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class GpuLockObserve(unittest.TestCase):
    def test_gl_trace_uses_selected_binary_inventory_and_keeps_timestamps(self):
        arm = load('arm_gpu_observe', 'scripts/qemu-arm64.py')
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'selected-gl-qemu'
            binary.write_text('#!/bin/sh\n[ "$1 $2" = "-trace help" ] || exit 42\n'
                              'printf "virtio_gpu_cmd_ctx_submit\\nvirtio_gpu_fence_resp\\nvirgl_fixture_finish\\n"\n')
            binary.chmod(0o755)
            patterns, inventory = arm.gpu_trace_inventory(str(binary))
            self.assertEqual(patterns, ('virtio_gpu_*', 'virgl*'))
            self.assertIn('virgl_fixture_finish', inventory)
            args = arm.command(str(binary), ['-device', 'virtio-gpu-gl-pci', '-chardev',
                               'stdio,logfile=/tmp/capture'], 'hvf', 'Darwin', 'arm64',
                               gpu_patterns=patterns)
            trace = args[args.index('-d') + 1]
            for p in (*arm.BLOCK_TRACE_PATTERNS, *patterns):
                self.assertIn('trace:' + p, trace.split(','))
            self.assertEqual(args[args.index('-D') + 1], '/tmp/capture.qemu-trace.log')
            self.assertEqual(args[args.index('-msg') + 1], 'timestamp=on')
            self.assertEqual(args[args.index('-pidfile') + 1], '/tmp/capture.qemu.pid')
            binary.write_text('#!/bin/sh\nprintf "virtio_gpu_cmd_ctx_submit\\n"\n')
            self.assertEqual(arm.gpu_trace_inventory(str(binary))[0], ('virtio_gpu_*',))
        main = (ROOT / 'scripts/qemu-arm64.py').read_text()
        self.assertIn('gpu_trace_inventory(arguments[0])', main)
        self.assertIn('.qemu-trace-events.txt', main)

    def test_serial_triggers_sample_the_launched_pid_and_reap_only_owned_children(self):
        sampler = load('qemu_sample_fixture', 'scripts/qemu-case-sample.py')
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / 'serial.txt'
            class Child:
                def __init__(self):
                    self.pid = 4242 + len(children)
                    self.terminated = False; self.reaped = False
                def poll(self): return None if not self.terminated else 0
                def terminate(self): self.terminated = True
                def send_signal(self, sig): self.terminated = True
                def wait(self, timeout=None): self.reaped = True; return 0
            children = []
            def spawn(argv, **kwargs):
                self.assertEqual(argv[:4], ['/usr/bin/sample', '31337', '3', '1'])
                self.assertEqual(argv[4], '-file')
                self.assertTrue(argv[5].startswith(str(serial) + '.qemu-samples/'))
                child = Child(); children.append(child); return child
            with patch.object(sampler.subprocess, 'Popen', side_effect=spawn):
                watcher = sampler.CaseSamples(31337, serial, system='Darwin')
                for case in ['recovery/stack-restored', 'recovery/supervisor-restarts',
                             'stacking/click-raises', 'stacking/all-minimized', 'focus/pair-click']:
                    watcher.observe(f'SUITE window-manager CASE {case} PASS ms=1\n')
                    self.assertEqual(len(children), 0)
                watcher.observe('CASE window-manager recovery/stack-restored STA')
                self.assertEqual(len(children), 0)
                watcher.observe('RT\n')
                watcher.observe('CASE window-manager recovery/stack-restored START\n')
                watcher.observe('CASE window-manager focus/pair-click START\n')
                self.assertEqual(len(children), 2)
                watcher.observe('CASE window-manager recovery/stack-restored END\n')
                watcher.tick(float('inf'))
                self.assertTrue(all(c.terminated and c.reaped for c in children))
                watcher.observe('CASE window-manager recovery/supervisor-restarts START\n')
                watcher.close()
                self.assertTrue(children[-1].terminated and children[-1].reaped)
                # QoS unclamping may supervise a different QEMU PID. The native
                # QEMU pidfile, not its wrapper's Popen PID, is authoritative.
                pidfile = Path(directory) / 'qemu.pid'
                pidfile.write_text('31337\n')
                watcher = sampler.CaseSamples(99999, serial, system='Darwin', pid_file=pidfile)
                watcher.observe('CASE window-manager recovery/restart-adopts START\n')
                self.assertEqual(watcher.qemu_pid, 31337)
                watcher.close()
                pidfile.unlink()
                before = len(children)
                watcher = sampler.CaseSamples(99999, serial, system='Darwin', pid_file=pidfile)
                watcher.observe('CASE window-manager recovery/restart-adopts START\n')
                self.assertEqual(len(children), before, 'missing QEMU PID must not sample its supervisor')
                watcher.close()
            records = (Path(str(serial) + '.qemu-samples') / 'samples.jsonl').read_text()
            self.assertIn('"qemu_pid": 31337', records)
            self.assertIn('"sample_pid": 4242', records)
        watch = (ROOT / 'scripts/watch-qemu.py').read_text()
        self.assertIn('CaseSamples(process.pid', watch)
        self.assertIn('case_samples.observe(', watch)
        self.assertIn('case_samples.close()', watch)
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        body = item(suite, 'fn run_case_body(')
        self.assertIn('CASE {suite} {name} START', body)
        self.assertLess(body.index('CASE {suite} {name} START'), body.index('process::fork()'))

    def test_command_stamps_retain_pending_commands_and_bound_overwrite(self):
        source_path = ROOT / 'kernel/src/window/gpu_command_trace.rs'
        self.assertTrue(source_path.exists(), 'missing bounded GPU command journal')
        source = source_path.read_text().replace('//!', '//').replace('use alloc::', 'use std::')
        # Kernel bindings alone are substituted; execute the production ring and guard.
        source = source.split('// Kernel bindings.')[0]
        run_rust(self, source + r'''
#[test] fn pending_command_survives_and_latest_commands_stay_bounded() {
 let ring=Commands::<2>::new();
 let first=ring.begin(100,7,8,2,0x207,24,4096);
 let out=ring.text();
 assert!(out.contains("gpu-command seq=1 begin_ns=100 return_ns=0 pid=7 tid=8 transport=2 type=519 cmd_bytes=24 payload_bytes=4096"),"{out}");
 ring.finish(first,310100);
 let second=ring.begin(310200,9,10,1,0x105,56,0);
 ring.begin(310300,11,12,2,0x201,40,0);
 ring.finish(first,999999); // stale completion cannot overwrite a newer reservation
 ring.finish(second,310400);
 let out=ring.text();
 assert!(out.contains("gpu-command-ring retained=2 overwritten=1 lost=0"),"{out}");
 assert!(!out.contains("seq=1 "),"{out}");
 assert!(out.contains("seq=2 begin_ns=310200 return_ns=310400 pid=9 tid=10"),"{out}");
 assert!(out.contains("seq=3 begin_ns=310300 return_ns=0 pid=11 tid=12"),"{out}");
}
''')
        for path, functions in [
            ('gpu_pci.rs', ['fn send_command_with_trace(', 'fn send_command_3desc_with_trace(', 'fn submit_polled<T>(']),
            ('gpu_mmio.rs', ['fn send_command('])]:
            driver = (ROOT / 'kernel/src/drivers/virtio' / path).read_text()
            for function in functions:
                body = item(driver, function)
                self.assertIn('gpu_command_trace::begin(', body, function)
                self.assertLess(body.index('gpu_command_trace::begin('), body.index('notify_queue'))
        proc = (ROOT / 'kernel/src/fs/procfs/mod.rs').read_text()
        self.assertIn('gpu_command_trace::text()', proc)
        self.assertIn('current_accounting_identity()', source_path.read_text())


if __name__ == '__main__':
    unittest.main()
