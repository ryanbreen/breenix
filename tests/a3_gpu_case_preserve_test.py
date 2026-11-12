"""Exercise case retention against the production active GPU journal, without a boot."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class CasePreservation(unittest.TestCase):
    def test_case_commands_survive_live_ring_wrap_and_case_end(self):
        trace = (ROOT / 'kernel/src/window/gpu_core_trace.rs').read_text()
        trace = trace.split('// Kernel bindings.')[0]
        trace = trace.replace('//!', '//').replace('use alloc::', 'use std::')
        # Run the old journal too: before preservation exists, boundaries have
        # no effect, and the real rolling journal evicts the originating command.
        if 'pub fn case_boundary(' not in trace:
            trace += '\nimpl<const N: usize> Commands<N> { pub fn case_boundary(&self, _: u64, _: &str) {} }\n'
        if 'pub fn export_text(' not in trace:
            trace += '\nimpl<const N: usize> Commands<N> { pub fn export_text(&self) -> String { self.text() } }\n'
        slots = (ROOT / 'kernel/src/window/gpu_command_trace.rs').read_text()
        slots = slots[slots.index('// Use one CAS'):slots.index('pub struct Commands')]
        run_rust(self, r'''
pub mod spin {
 pub struct Once<T>(std::sync::OnceLock<T>);
 impl<T> Once<T> {
  pub const fn new()->Self {Self(std::sync::OnceLock::new())}
  pub fn get(&self)->Option<&T>{self.0.get()}
  pub fn call_once(&self,f:impl FnOnce()->T)->&T{self.0.get_or_init(f)}
 }
}
mod gpu_command_trace {
 use core::sync::atomic::{AtomicU64, Ordering::{Acquire,Release,Relaxed,AcqRel}};
 SLOTS
}
mod gpu_core_trace { SPIN TRACE }
use gpu_core_trace::Commands;
#[test] fn preserve_origin_through_wrap_freeze_and_next_case() {
 let ring=Commands::<3>::new();
 ring.submit(1,1,1,1,512,32,0,1,1); // before the case: must stay outside
 ring.case_boundary(10,"budget 100 parent-fork recovery/stack-restored\n");
 let create=ring.submit(101,722,949,338,512,32,0,2,2);
 ring.finish(create,102);
 let shader=ring.submit(103,724,952,339,519,32,19660,3,3);
 let bell=ring.doorbell(104,0,20,3);
 ring.finish(bell,105);
 // Deterministic eviction on the old journal, before any diagnostic export.
 for i in 0..12 {let seq=ring.submit(110+i,722,949,338,519,32,4,4+i,4+i);ring.finish(seq,130+i);}
 ring.finish(shader,150);
 assert!(!ring.export_text().contains("gpu-core-case-window"),"active archives stay out of suite exports");
 ring.case_boundary(99,"budget 160 parent-outcome recovery/stack-restored\n"); // not the owner
 ring.case_boundary(10,"budget 170 parent-outcome recovery/stack-restored\n");
 let frozen=ring.text();
 assert!(frozen.contains("submit_ns=101 fence_return_ns=102 pid=722 tid=949 context=338 type=512"),"evicted context creator: {frozen}");
 assert!(frozen.contains("submit_ns=103 fence_return_ns=150 pid=724 tid=952 context=339 type=519 cmd_bytes=32 payload_bytes=19660"),"evicted blocking command: {frozen}");
 assert!(frozen.contains("case=recovery/stack-restored owner_tid=10 begin_ns=100 end_ns=170"),"{frozen}");
 assert!(ring.export_text().contains("gpu-core-case-window case=recovery/stack-restored"));
 assert!(!ring.export_text().contains("gpu-core-case-window"),"emit each frozen case once");
 let preserved=|s:String| s.lines().filter(|l|l.contains("case=recovery/")).collect::<Vec<_>>().join("\n");
 for i in 0..12 {ring.submit(180+i,9,9,9,519,32,4,30+i,30+i);}
 ring.finish(shader,999); // completion after freeze cannot rewrite its boundary
 assert_eq!(preserved(frozen),preserved(ring.text()));
 ring.case_boundary(10,"budget 200 parent-fork focus/keys-to-focused\n");
 let pending=ring.submit(201,7,8,9,519,32,37912,50,50);
 ring.case_boundary(10,"budget 202 watchdog-before-kill child-7\n");
 ring.finish(pending,203);
 let next=ring.text();
 assert!(next.contains("case=focus/keys-to-focused owner_tid=10 begin_ns=200 end_ns=202"),"{next}");
 assert!(next.contains("submit_ns=201 fence_return_ns=0 pid=7 tid=8"),"{next}");
 assert!(!next.contains("case=recovery/stack-restored"),"stale case attribution: {next}");
 // Saturation is bounded, visible, and keeps the earliest identity, not the tail.
 ring.case_boundary(10,"budget 300 parent-fork recovery/supervisor-restarts\n");
 let first=ring.submit(301,71,81,91,512,32,0,60,60);
 for i in 0..300 {ring.submit(302+i,72,82,92,519,32,4,61+i,61+i);}
 ring.case_boundary(10,"budget 700 parent-outcome recovery/supervisor-restarts\n");
 let saturated=ring.text();
 assert!(saturated.contains(&format!("gpu-core-command seq={first} submit_ns=301")),"{saturated}");
 assert!(saturated.contains("capacity=192 retained=192 overflow=109"),"{saturated}");
}
'''.replace('SLOTS', slots).replace('SPIN', 'use crate::spin;' if 'spin::' in trace else '').replace('TRACE', trace))

    def test_existing_parent_boundaries_reach_arm_preservation_before_export(self):
        devfs = (ROOT / 'kernel/src/fs/devfs/mod.rs').read_text()
        dispatch = item(devfs, 'if let Some(command) = buf.strip_prefix(b"WM_STALL ")')
        self.assertIn('#[cfg(target_arch = "aarch64")]', dispatch)
        self.assertIn('gpu_core_trace::case_boundary(caller.tid, command)', dispatch)
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertLess(suite.index('case_budget("parent-fork", name)'), suite.index('CASE {suite} {name} START'))
        self.assertLess(suite.index('case_budget("parent-outcome", name)'), suite.index('dump_stall_evidence();', suite.index('case_budget("parent-outcome", name)')))
        stop = item(suite, 'fn stop_child(')
        self.assertLess(stop.index('case_budget("watchdog-before-kill"'), stop.index('dump_stall_evidence();'))


if __name__ == '__main__':
    unittest.main()
