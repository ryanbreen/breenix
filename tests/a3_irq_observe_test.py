"""Exercise the missing A.3 observation boundaries without launching a guest."""
import importlib.util
from pathlib import Path
import os
import subprocess
import tempfile
import unittest
from suite_collection_test import run_rust
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text()


class IrqObserve(unittest.TestCase):
    def test_trace_and_pre_exec_gate_keep_qos_launcher_inside_gate(self):
        spec = importlib.util.spec_from_file_location('arm', ROOT / 'scripts/qemu-arm64.py')
        arm = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(arm)
        argv = arm.command('qemu-system-aarch64', ['-chardev', 'stdio,id=con,logfile=/tmp/capture.txt'],
                           'hvf', 'Darwin', 'arm64')
        self.assertIn('-d', argv, 'missing host block request/notify/IRQ trace arguments')
        self.assertEqual(argv[argv.index('-d') + 1], ','.join('trace:' + p for p in [
            'virtio_blk_handle_*', 'virtio_blk_req_complete', 'virtio_blk_rw_complete',
            'virtio_queue_notify', 'virtio_notify', 'virtio_mmio_queue_write', 'virtio_mmio_setting_irq']))
        self.assertEqual(argv[argv.index('-D') + 1], '/tmp/capture.txt.qemu-trace.log')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            receipt = path / 'order'
            gate, qos, qemu = [path / name for name in ['gate', 'qos', 'qemu-system-fixture']]
            for script, label in [(gate, 'gate'), (qos, 'qos')]:
                script.write_text(f'#!/bin/sh\necho {label} >> "$RECEIPT"\nexec "$@"\n')
                script.chmod(0o755)
            qemu.write_text('#!/bin/sh\necho qemu >> "$RECEIPT"\n')
            qemu.chmod(0o755)
            env = dict(os.environ, RECEIPT=str(receipt), BREENIX_QEMU_PRE_EXEC_GATE=str(gate))
            result = subprocess.run(['python3', str(ROOT / 'scripts/qemu-arm64.py'), '--accel', 'tcg',
                                     '--launcher', str(qos), str(qemu)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(receipt.read_text().splitlines(), ['gate', 'qos', 'qemu'])
            gate.write_text('#!/bin/sh\nexit 42\n')
            receipt.unlink()
            result = subprocess.run(['python3', str(ROOT / 'scripts/qemu-arm64.py'), '--accel', 'tcg',
                                     '--launcher', str(qos), str(qemu)], env=env, capture_output=True)
            self.assertEqual(result.returncode, 42)
            self.assertFalse(receipt.exists(), 'a refused artifact gate must not execute QEMU')
        self.assertIn('qemu-trace.log', read('scripts/boot-interactive.sh'))
        self.assertIn('MH_MAGIC_64', read('scripts/qemu-ui-launch.c'))

    def test_idle_switch_out_does_not_charge_an_entire_window(self):
        source = read('kernel/src/window/accounting.rs').replace('//!', '//').replace('use alloc::', 'use std::')
        dispatch = item(read('kernel/src/tracing/providers/sched.rs'), 'pub fn trace_ctx_switch(')
        # Execute the production dispatch producer; the rest of tracing is inert.
        hook = 'TRACE.switch_out(140, tid);' if 'pub fn switch_out(' in source else ''
        run_rust(self, source + r'''
static TRACE: Accounting<128,16,16> = Accounting::new();
struct Counter; impl Counter { fn increment(&self) {} }
static CTX_SWITCH_TOTAL: Counter = Counter;
struct Provider; impl Provider { fn is_enabled(&self)->bool {false} }
static SCHED_PROVIDER: Provider = Provider;
const CTX_SWITCH_ENTRY:u16=1;
mod tracing { pub fn is_enabled()->bool{false} pub fn record_event_2(_:u16,_:u16,_:u16){} }
mod window { pub mod accounting { pub fn switch_out(tid:u64) { HOOK let _=tid; } } }
DISPATCH
#[test]fn idle_is_not_running_behind_the_worker(){
 let _boundary: fn(u64)=window::accounting::switch_out;
 TRACE.state(80,0,1,1,0,0); // swapper kept Running by scheduler design
 TRACE.begin(100,7,1);
 trace_ctx_switch(1,10);
 TRACE.state(140,7,10,1,0,0);
 TRACE.end(200,7);
 let out=TRACE.text(200);
 let charged:u64=out.lines().filter(|l|l.starts_with("account-thread "))
  .map(|l|l.split_whitespace().find_map(|v|v.strip_prefix("on_cpu_ns=")).unwrap().parse::<u64>().unwrap()).sum();
 assert_eq!(charged,100,"idle double-count exceeds one-vCPU capacity: {out}");
 assert!(out.contains("tid=1 on_cpu_ns=40"),"{out}");
 assert!(out.contains("tid=10 on_cpu_ns=60"),"{out}");
}
'''.replace('HOOK', hook.replace('TRACE.', 'crate::TRACE.')).replace('DISPATCH', dispatch))

    def test_irq_entry_precedes_ring_consumption_and_retains_cpu(self):
        irq = item(read('kernel/src/drivers/virtio/block_mmio.rs'), 'pub fn handle_interrupt(')
        self.assertIn('accounting::disk_irq_entry', irq, 'missing device IRQ entry stamp')
        self.assertLess(irq.index('accounting::disk_irq_entry'), irq.index('block_device_state'))
        source = read('kernel/src/window/accounting.rs').replace('//!', '//').replace('use alloc::', 'use std::')
        run_rust(self, source + r'''
#[test]fn irq_is_independent_of_used_consumption(){
 let t=Accounting::<64,8,8>::new(); t.begin(100,7,4);
 let id=t.disk_enqueue(101,7,10,0,0,42,4096);
 t.disk_stage(id,2,110); t.disk_irq_entry(130,0,3);
 // A watchdog before used-ring consumption still sees the independent entry.
 let out=t.text(135);
 assert!(out.contains("disk-irq-entry device=0 ns=130 cpu=3"),"{out}");
 assert!(out.contains("submit_ns=110 irq_ns=0"),"{out}");
 t.disk_stage(id,3,150); t.end(160,7);
 let out=t.text(170);
 assert!(out.contains("disk-irq-entry device=0 ns=130 cpu=3"),"{out}");
 assert!(out.contains("submit_ns=110 irq_ns=150"),"{out}");
}
''')

    def test_join_gpu_wake_and_budget_have_producers_and_retained_names(self):
        futex = read('kernel/src/syscall/futex.rs')
        self.assertIn('WaitSite::enter', item(futex, 'fn futex_wait('), 'ordinary FUTEX_WAIT loses join word identity')
        self.assertIn('accounting::wake', item(futex, 'pub fn futex_wake_cleared_tid('))
        gpu = read('kernel/src/gpu/mod.rs')
        for fn in ['fn wait_event_prepared(', 'fn reaper_block(']:
            self.assertIn('WaitSite::enter', item(gpu, fn), 'unnamed GPU wait')
        self.assertIn('accounting::wake', item(gpu, 'fn notify('))
        journal = read('kernel/src/window/accounting.rs')
        for label in ['account-wake', 'gpu-events', 'gpu-reaper', 'thread-exit-futex']:
            self.assertIn(label, journal)
        stall = read('kernel/src/window/stall_trace.rs')
        self.assertIn('case-budget', stall, 'no watchdog-surviving outer-case budget attribution')
        suite = read('libs/libbreenix/src/suite.rs')
        body = item(suite, 'fn run_case_body(')
        for phase in ['case-begin', 'logic-begin', 'logic-end', 'report-begin', 'report-end', 'parent-outcome']:
            self.assertIn(phase, body)
        stop = item(suite, 'fn stop_child(')
        self.assertLess(stop.index('case_budget'), stop.index('dump_stall_evidence'))
        desktop = read('userspace/programs/src/wm_suite/desktop.rs')
        self.assertIn('case_budget', item(desktop, 'fn failure_diagnostic('))
        self.assertIn('wait-budget', stall)


if __name__ == '__main__':
    unittest.main()
