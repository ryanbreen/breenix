#!/usr/bin/env python3
"""E24 ledger 520(r): force pending SIGKILL during an ARM request-gate wait.

Compile the production gate, kill-custody atomic transitions and no-sleep
predicate. The host scheduler double applies that predicate at publication;
the device owner releases the gate at the modeled park, without a timing race.
"""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from virtio_read_cause_test import block, ROOT


def function(source, name):
    return block(source, source.index('fn ' + name + '('))


class VirtioReadContext(unittest.TestCase):
    def test_pending_kill_does_not_become_device_read_failure(self):
        source = (ROOT / 'kernel/src/drivers/virtio/block_mmio.rs').read_text()
        thread = (ROOT / 'kernel/src/task/thread.rs').read_text()
        gate = '\n'.join(block(source, source.index(head)) for head in (
            'struct BlockMmioRequestGate', "struct BlockMmioRequestGuard<'a>",
            'impl BlockMmioRequestGate', "impl BlockMmioRequestGuard<'_>",
            "impl Drop for BlockMmioRequestGuard<'_>"))
        constants = '\n'.join(re.findall(r'const (?:KILL_CLAIMED|KILL_PENDING|CUSTODY_COUNT):[^;]+;', thread))
        predicate = function(thread, 'must_not_sleep')
        custody = function(thread, 'try_enter')
        custody_drop = block(thread, thread.index('impl Drop for KillCustody'))
        can_sleep = function(source, 'block_mmio_request_gate_can_sleep')
        # The model's refusal is the production scheduler's actual predicate.
        scheduler = (ROOT / 'kernel/src/task/scheduler.rs').read_text()
        self.assertIn('if thread.must_not_sleep()', function(scheduler, 'block_current_for_io_publish'))
        harness = r'''
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
const BLOCK_MMIO_WEDGED_ERROR: &str = "Block MMIO device wedged after an abandoned request";
static THREAD: Thread = Thread { kill_custody: AtomicU64::new(1) };
struct Thread { kill_custody: AtomicU64 }
impl Thread { @PREDICATE@ }
pub struct KillCustody(Option<&'static AtomicU64>);
impl KillCustody { @CUSTODY@ }
@CUSTODY_DROP@
mod per_cpu {
    pub fn current_thread() -> Option<&'static crate::Thread> { Some(&crate::THREAD) }
    pub fn preempt_count() -> u32 { 1 }
}
use per_cpu as per_cpu_aarch64;
mod arch_impl { pub mod aarch64 { pub mod timer_interrupt {
    pub fn is_initialized() -> bool { true }
} } }
mod task {
    pub mod thread {
        pub type KillCustody = crate::KillCustody;
        #[derive(PartialEq, Eq)] pub enum ThreadState { BlockedOnIO }
    }
    pub mod scheduler { pub fn current_thread_id() -> Option<u64> { Some(73) } }
    pub mod waitqueue {
        pub fn schedule_current_wait() {
            crate::PARKS.fetch_add(1, crate::Ordering::Relaxed);
            // Completion of the other owner's single already-issued request.
            crate::GATE.unlock();
        }
    }
}
struct WaitQueueHead;
impl WaitQueueHead {
    const fn new() -> Self { Self }
    fn prepare_to_wait(&self, _: task::thread::ThreadState) -> Option<u64> {
        if THREAD.must_not_sleep() { None } else { Some(73) }
    }
    fn finish_wait(&self) {}
    fn wake_up_one(&self) {}
    fn wake_up(&self) {}
}
static PARKS: AtomicU32 = AtomicU32::new(0);
static GATE: BlockMmioRequestGate = BlockMmioRequestGate::new();
fn main() {
    // A syscall has custody count 1. A concurrent SIGKILL marks it pending
    // while another request still owns the device. No media failure occurs.
    THREAD.kill_custody.store(KILL_PENDING | 1, Ordering::Release);
    GATE.locked.store(true, Ordering::Release);
    let result = GATE.lock();
    let guard = match result {
        Ok(guard) => guard,
        Err(reason) => panic!("context refusal would latch metadata device-read: {reason}"),
    };
    assert_eq!(PARKS.load(Ordering::Relaxed), 1);
    assert!(!THREAD.must_not_sleep(), "request ownership lost its nested custody");
    // The read is issued once only after the gate is acquired. The nested
    // guard also spans its completion wait. It changes no preemption count.
    assert_eq!(per_cpu::preempt_count(), 1);
    drop(guard);
    assert!(THREAD.must_not_sleep(), "SIGKILL was cleared rather than deferred");
    assert_eq!(THREAD.kill_custody.load(Ordering::Acquire), KILL_PENDING | 1);
    assert!(!GATE.locked.load(Ordering::Acquire));
}
'''.replace('@PREDICATE@', predicate).replace('@CUSTODY_DROP@', custody_drop).replace('@CUSTODY@', custody)
        # Unused production helpers and doubles are expected in this one-path model.
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'model.rs').write_text(constants + '\n' + gate + '\n' + can_sleep + '\n' + harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Adead_code',
                            str(path / 'model.rs'), '-o', str(path / 'model')], check=True, timeout=45)
            subprocess.run([str(path / 'model')], check=True, timeout=5)


if __name__ == '__main__':
    unittest.main()
