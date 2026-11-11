"""Account for a compositor restart without needing a VM or a case survivor."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class Accounting(unittest.TestCase):
    def test_interval_partition_and_request_syscall_retention(self):
        path = ROOT / 'kernel/src/window/accounting.rs'
        self.assertTrue(path.exists(), 'missing exec-to-first-composite accounting and disk/file timing records')
        source = path.read_text().replace('//!', '//').replace('use alloc::', 'use std::')
        run_rust(self, source + r'''
#[test] fn measured_window_survives_its_process() {
 let trace = Accounting::<128,16,16>::new();
 // Initial states precede exec. Include a competitor, and a new bwm worker.
 trace.state(80,7,10,1,0,0);
 trace.state(80,9,20,0,0,0);
 trace.begin(100,7,2);
 trace.state(110,7,10,0,0,0);
 trace.state(110,9,20,1,0,0);
 trace.state(120,7,11,0,0,0);
 trace.state(130,7,10,2,2,99);
 trace.state(150,7,10,2,1,88);
 trace.state(160,7,10,0,0,0);
 trace.state(170,7,10,1,0,0);
 trace.state(180,7,11,3,0,0);
 trace.queue(125,0,3);
 trace.syscall(126,7,11,63,true,0);
 trace.syscall(181,7,11,63,false,4096);
 let request = trace.disk_enqueue(115,9,20,0,0,42,4096);
 trace.disk_stage(request,1,121);
 trace.disk_stage(request,2,123);
 trace.disk_stage(request,3,140);
 trace.disk_stage(request,4,145);
 trace.end(200,7);
 trace.state(205,7,10,3,0,0);
 let out=trace.text(210);
 assert!(out.contains("account-window id=1 pid=7 begin_ns=100 end_ns=200 complete=1 vcpus=2"),"{out}");
 assert!(out.contains("account-thread window=1 pid=7 tid=10 on_cpu_ns=40 runnable_ns=30 blocked_ns=30"),"{out}");
 assert!(out.contains("account-thread window=1 pid=7 tid=11 on_cpu_ns=0 runnable_ns=60 blocked_ns=0"),"{out}");
 assert!(out.contains("account-thread window=1 pid=9 tid=20 on_cpu_ns=90 runnable_ns=10 blocked_ns=0"),"{out}");
 assert!(out.contains("account-wait window=1 pid=7 tid=10 kind=2 object=99 blocked_ns=20"),"{out}");
 assert!(out.contains("account-wait window=1 pid=7 tid=10 kind=1 object=88 blocked_ns=10"),"{out}");
 assert!(out.contains("account-runqueue window=1 ns=125 cpu=0 len=3"),"{out}");
 assert!(out.contains("disk-request id=1 pid=9 tid=20 device=0 op=0 sector=42 bytes=4096 enqueue_ns=115 gate_ns=121 submit_ns=123 irq_ns=140 wake_ns=145"),"{out}");
 assert!(out.contains("file-syscall window=1 pid=7 tid=11 nr=63 entry=1 ns=126 result=0"),"{out}");
 assert!(out.contains("file-syscall window=1 pid=7 tid=11 nr=63 entry=0 ns=181 result=4096"),"{out}");
 // No end/return/IRQ: the watchdog must still see partial measurements.
 trace.begin(300,7,2);
 trace.state(310,7,10,2,2,99);
 trace.syscall(315,7,10,63,true,0);
 let pending=trace.disk_enqueue(316,7,10,0,0,50,512);
 let out=trace.text(350);
 assert!(out.contains("begin_ns=300 end_ns=350 complete=0"),"{out}");
 assert!(out.contains("entry=1 ns=315"),"{out}");
 assert!(out.contains("sector=50 bytes=512 enqueue_ns=316 gate_ns=0 submit_ns=0 irq_ns=0 wake_ns=0"),"{out}");
 trace.disk_stage(pending,1,351);
 // Bounded overflow is reported rather than represented as complete evidence.
 for i in 0..140 {trace.queue(400+i,0,1);}
 let out=trace.text(600);assert!(out.contains("account-buffer"));assert!(out.contains("overwritten="));
}
''')

    def test_producers_and_parent_export(self):
        def read(path): return (ROOT / path).read_text()
        thread = read('kernel/src/task/thread.rs')
        self.assertIn('accounting::thread_state', thread, 'no scheduler transition accounting')
        self.assertIn('accounting::file_syscall', read('kernel/src/tracing/providers/syscall.rs'))
        self.assertIn('accounting::exec', read('kernel/src/window/mod.rs'))
        self.assertIn('accounting::composite', read('kernel/src/window/scene.rs'))
        self.assertIn('accounting::queue', read('kernel/src/tracing/providers/sched.rs'))
        for name in ['block_mmio.rs', 'block.rs']:
            disk = read('kernel/src/drivers/virtio/' + name)
            for hook in ['disk_enqueue', 'disk_stage']:
                self.assertIn(hook, disk, name + ' missing ' + hook)
        self.assertIn('accounting::text', read('kernel/src/fs/procfs/mod.rs'))
        parent = read('libs/libbreenix/src/suite.rs')
        stop = item(parent, 'fn stop_child(')
        self.assertLess(stop.index('dump_stall_evidence'), stop.index('signal::kill'))
        self.assertIn('dump_stall_evidence', item(parent, 'fn run_case_body('))
        self.assertIn('dump_stall_evidence', item(parent, '    pub fn run('))


if __name__ == '__main__':
    unittest.main()
