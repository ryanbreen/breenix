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
        self.assertIn('-msg', argv, 'QEMU log traces omit timestamps by default')
        self.assertEqual(argv[argv.index('-msg') + 1], 'timestamp=on')
        explicit = arm.command('qemu-system-aarch64', [
            '-chardev', 'stdio,logfile=/tmp/capture.txt', '-d', 'guest_errors', '-D', '/tmp/explicit.log',
            '-msg', 'guest-name=on,timestamp=off'],
            'hvf', 'Darwin', 'arm64')
        self.assertEqual(explicit.count('-d'), 1)
        self.assertTrue(explicit[explicit.index('-d') + 1].startswith('guest_errors,trace:'))
        self.assertEqual(explicit[explicit.index('-D') + 1], '/tmp/explicit.log')
        self.assertEqual(explicit.count('-msg'), 1)
        self.assertEqual(explicit[explicit.index('-msg') + 1], 'guest-name=on,timestamp=on')
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

    def test_budget_writer_delivers_whole_commands_to_the_device(self):
        # Keep the production formatting, per-write dispatch and retained ring.
        # Only the open File is substituted, so write_fmt fragmentation is real.
        producer = item(read('libs/libbreenix/src/suite.rs'), 'pub fn case_budget(')
        producer = producer.replace('std::fs::OpenOptions', 'fixture::OpenOptions')
        dispatch = item(read('kernel/src/fs/devfs/mod.rs'),
                        'if let Some(command) = buf.strip_prefix(b"WM_STALL ")')
        ring = read('kernel/src/window/stall_trace.rs').replace('//!', '//').replace('use alloc::', 'use std::')
        run_rust(self, r'''
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static STALL_EVIDENCE: AtomicBool = AtomicBool::new(true);
static NOW: AtomicU64 = AtomicU64::new(100);
fn monotonic_ns()->Option<u128>{Some(NOW.fetch_add(10,Ordering::Relaxed) as u128)}
mod spin {
 pub struct Mutex<T>(std::sync::Mutex<T>);
 impl<T> Mutex<T>{pub const fn new(t:T)->Self{Self(std::sync::Mutex::new(t))}
 pub fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
}
mod signal { pub fn monotonic_nanos()->u64{300} }
mod task { pub mod scheduler { pub fn stall_snapshot()->Vec<String>{Vec::new()} } }
pub mod window {
 pub mod gpu_core_trace {pub fn case_boundary(_:u64,_:&str){}}
 pub mod stall_trace { use crate::spin; RING }
}
static WRITES: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
fn device_write(buf:&[u8])->Result<usize,i32>{
 struct Caller {tid:u64} let caller=Caller{tid:7};
 DISPATCH
 Ok(buf.len())
}
mod fixture {
 pub struct OpenOptions;
 impl OpenOptions {
  pub fn new()->Self{Self}
  pub fn write(&mut self,enabled:bool)->&mut Self{assert!(enabled);self}
  pub fn open(&self,path:&str)->std::io::Result<File>{
   assert_eq!(path,"/dev/screen-evidence");Ok(File)
  }
 }
 pub struct File;
 impl std::io::Write for File {
  fn write(&mut self,buf:&[u8])->std::io::Result<usize>{
   crate::WRITES.lock().unwrap().push(buf.to_vec());
   crate::device_write(buf).map_err(std::io::Error::from_raw_os_error)
  }
  fn flush(&mut self)->std::io::Result<()>{Ok(())}
 }
}
PRODUCER
#[test]fn formatted_budget_reaches_the_ring_before_watchdog_kill(){
 let phases=["case-begin","logic-begin","diagnostic-begin","diagnostic-end"];
 for phase in phases {case_budget(phase,"recovery/stack-restored");}
 let out=window::stall_trace::text();
 let writes=WRITES.lock().unwrap();
 assert!(out.contains("case-budget tid=7 case=recovery/stack-restored case_begin_ns=100 logic_begin_ns=110 logic_end_ns=0"),"missing retained budget after {} device writes: {out}",writes.len());
 assert_eq!(writes.len(),phases.len(),"each budget command must be one device write");
 assert!(out.contains("diagnostic_ns=10 diagnostics=1 diagnostic_begin_ns=0"),"{out}");
 assert!(out.contains("last_ns=130 phase=diagnostic-end"),"{out}");
 drop(writes);
 STALL_EVIDENCE.store(false,Ordering::Relaxed);
 case_budget("report-end","recovery/stack-restored");
 assert_eq!(WRITES.lock().unwrap().len(),phases.len());
}
'''.replace('RING', ring).replace('DISPATCH', dispatch).replace('PRODUCER', producer))

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
 t.join(115,7,10,88,11,8);
 t.state(120,7,10,2,3,88);
 t.wake(140,7,10,9,88,0,1);
 t.wake(141,0,20,10,99,10,1);
 t.disk_stage(id,3,150); t.end(160,7);
 let out=t.text(170);
 assert!(out.contains("disk-irq-entry device=0 ns=130 cpu=3"),"{out}");
 assert!(out.contains("submit_ns=110 irq_ns=150"),"{out}");
 assert!(out.contains("name=thread-exit-futex object=88"),"{out}");
 assert!(out.contains("object=88 blocked_ns=40 name=thread-exit-futex"),"{out}");
 assert!(out.contains("account-join window=1 ns=115 pid=7 tid=10 object=88 target_tid=11 target_pid=8"),"{out}");
 assert!(out.contains("name=gpu-events object=99 target=10 count=1"),"{out}");
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
        source = stall.replace('//!', '//').replace('use alloc::', 'use std::')
        run_rust(self, r'''
mod spin {
 pub struct Mutex<T>(std::sync::Mutex<T>);
 impl<T> Mutex<T>{pub const fn new(t:T)->Self{Self(std::sync::Mutex::new(t))}
 pub fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
}
mod signal { pub fn monotonic_nanos()->u64{300} }
mod task { pub mod scheduler { pub fn stall_snapshot()->Vec<String>{vec![String::from("thread fixture")]} } }
''' + source + r'''
#[test]fn budget_survives_a_killed_diagnostic_writer(){
 command(7,"budget 100 case-begin recovery/stack-restored");
 command(7,"budget 110 logic-begin recovery/stack-restored");
 command(7,"budget 120 diagnostic-begin recovery/stack-restored");
 command(7,"budget 150 diagnostic-end recovery/stack-restored");
 command(7,"arm 160 100 positive-click");
 command(7,"end 160");
 command(7,"budget 220 diagnostic-begin recovery/stack-restored");
 let out=text();
 assert!(out.contains("case-budget tid=7 case=recovery/stack-restored case_begin_ns=100 logic_begin_ns=110 logic_end_ns=0"),"{out}");
 assert!(out.contains("diagnostic_ns=30 diagnostics=1 diagnostic_begin_ns=220"),"{out}");
 assert!(out.contains("wait_ns=140 wait_begin_ns=0 last_wait=positive-click"),"{out}");
 assert!(out.contains("wait-budget tid=7 wait=positive-click begin_ns=160 end_ns=300 deadline_ns=260"),"{out}");
}
''')


if __name__ == '__main__':
    unittest.main()
