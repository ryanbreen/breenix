"""Midpoint evidence must survive a case killed by the suite parent."""
from pathlib import Path
import importlib.util
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class StallSnapshot(unittest.TestCase):
    def test_midpoint_and_bounded_kernel_retention(self):
        path = ROOT / 'kernel/src/window/stall_trace.rs'
        self.assertTrue(path.exists(), 'missing kernel stall snapshot and retained evidence ring')
        source = path.read_text().replace('//!', '//').replace('use alloc::', 'use std::')
        source = source.replace('spin::Mutex', 'crate::HostMutex')
        run_rust(self, r'''
struct HostMutex<T>(std::sync::Mutex<T>);
impl<T> HostMutex<T>{const fn new(v:T)->Self{Self(std::sync::Mutex::new(v))}fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
mod task{pub mod scheduler{
 pub fn stall_snapshot()->std::vec::Vec<String>{vec!["pid=7 tid=8 state=BlockedOnIO syscall=202 wait=completion object=99 since_run_ns=123 last_cpu=2 cpu_ns=456 queue=none".into()]}
}}
mod signal{pub fn monotonic_nanos()->u64{0}}
mod window{pub mod stall_trace{
@SOURCE@
#[test]fn midpoint_survives_case_death(){
 command(7,"arm 100 1000 recovery");
 sample(599);assert!(!text().contains("pid=7 tid=8"));
 sample(600);let out=text();assert!(out.contains("wait=recovery begin_ns=100 deadline_ns=1100 snapshot_ns=600"));
 assert!(out.contains("pid=7 tid=8 state=BlockedOnIO syscall=202 wait=completion object=99 since_run_ns=123 last_cpu=2 cpu_ns=456 queue=none"));
 sample(999);assert_eq!(text(),out,"a wait snapshots only once");
 command(7,"end 100");assert_eq!(text(),out,"ending or killing case must retain rows");
 for _ in 0..CAPACITY+3{record("click-delivery",7,8,9);}
 let out=text();assert!(out.contains("overwritten="));assert!(out.lines().count()<=CAPACITY+1);assert!(out.contains("click-delivery"));
}
}}
'''.replace('@SOURCE@', source))

    def test_parent_dumps_before_kill_and_at_done(self):
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        stop = item(suite, 'fn stop_child(')
        self.assertIn('dump_stall_evidence', stop, 'watchdog currently kills the case before retained snapshot export')
        self.assertLess(stop.index('dump_stall_evidence'), stop.index('signal::kill'))
        self.assertIn('sample_stall_evidence', item(suite, 'fn wait_exit('))
        run = item(suite, '    pub fn run(')
        self.assertIn('dump_stall_evidence', run)
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        self.assertIn('StallWait::new', item(desktop, 'pub fn until_from('))
        self.assertIn('StallWait::new', item(desktop, 'fn read_n('))

    def test_host_line_clock_keeps_split_lines(self):
        path = ROOT / 'scripts/serial-clock.py'
        self.assertTrue(path.exists(), 'missing host timestamp sidecar')
        spec = importlib.util.spec_from_file_location('serial_clock', path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        import tempfile, json
        with tempfile.TemporaryDirectory() as tmp:
            serial = Path(tmp) / 'serial.txt'
            serial.write_bytes(b'guest_ns=12')
            clock = module.SerialClock(serial)
            clock.observe(100, 10)
            with serial.open('ab') as f: f.write(b'3\nsecond\npartial')
            clock.observe(200, 20)
            rows = [json.loads(line) for line in clock.path.read_text().splitlines()]
            self.assertEqual([r['line'] for r in rows], ['guest_ns=123', 'second'])
            self.assertEqual([r['offset'] for r in rows], [0, 13])
            self.assertTrue(all(r['host_unix_ns'] == 200 and r['host_monotonic_ns'] == 20 for r in rows))
            self.assertIn('SerialClock', (ROOT / 'scripts/watch-qemu.py').read_text())

if __name__ == '__main__':
    unittest.main()
