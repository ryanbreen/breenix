#!/usr/bin/env python3
"""Drive the scheduler's wake transport with fixed, host-side interleavings."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCHEDULER = ROOT / "kernel/src/task"
TRANSPORT = SCHEDULER / "wake_inbox.rs"

HARNESSES = r"""
#[test]
fn burst_and_duplicates() {
    let queue = Queue::new();
    let mut wakes = [0; 96];
    for tid in 1..=96 {
        queue.publish(tid);
        queue.publish(tid);
    }
    let (first_pass, more) = queue.drain(|tid| wakes[tid - 1] += 1);
    assert!(first_pass <= 32, "wake drain exceeded its budget: {first_pass}");
    assert!(more, "a partial drain must re-arm");
    while queue.drain(|tid| wakes[tid - 1] += 1).1 {}
    assert_eq!(wakes, [1; 96], "each distinct waiter must wake exactly once");
}

#[test]
fn completion_between_check_and_block() {
    let queue = Queue::new();
    // One CPU already received a full burst. On the owner CPU the waiter
    // has checked the receipt under its scheduler lock and is about to park.
    for tid in 1..=64 {
        queue.publish(tid);
    }
    let receipt_seen_by_waiter = false;
    // The device publishes its receipt and its wake before the owner stores
    // the blocked state. The owner's lock excludes the drain until that store.
    queue.publish(65);
    let mut blocked = !receipt_seen_by_waiter;
    while queue.drain(|tid| {
        if tid == 65 {
            blocked = false;
        }
    }).1 {}
    assert!(!blocked, "the completion lost its wake between check and block");
}

#[test]
fn sparse_drain_does_not_walk_the_thread_index() {
    let queue = Queue::new();
    queue.hold(4096);
    let mut wakes = Vec::new();
    let (visited, more) = queue.drain(|tid| wakes.push(tid));
    assert_eq!(wakes, [4096]);
    assert!(!more);
    assert_eq!(visited, 1, "one pending wake should cost one visit");
}
"""

LIVE_ADAPTER = r"""
use wake_inbox::{WakeInbox, WakeNode};
struct Queue {
    inbox: WakeInbox,
    nodes: Box<[WakeNode]>,
}

impl Queue {
    fn new() -> Self {
        Self {
            inbox: WakeInbox::new(),
            nodes: (1..=4096).map(WakeNode::new).collect(),
        }
    }

    fn publish(&self, tid: usize) {
        // The fixture owns the stable nodes for the life of the inbox.
        assert!(unsafe { self.inbox.publish(&self.nodes[tid - 1], 1, 0) }.is_some());
    }

    fn hold(&self, tid: usize) {
        self.publish(tid);
    }

    fn drain(&self, mut wake: impl FnMut(usize)) -> (usize, bool) {
        let mut visited = 0;
        let more = self.inbox.drain(|tid, _| {
            visited += 1;
            wake(tid as usize);
        });
        (visited, more)
    }
}
"""


def legacy_adapter():
    """Run the same assertions against the old slot buffer and held-wake scan."""
    scheduler = (SCHEDULER / "scheduler.rs").read_text()
    rq = (SCHEDULER / "scheduler/rq.rs").read_text()
    buffer = scheduler[scheduler.index("const ISR_WAKEUP_SLOTS"):scheduler.index("static ISR_WAKEUP_BUFFERS")]
    buffer = buffer.replace(
        "fn take_each(&self, mut apply: impl FnMut(u64)) {\n        for slot in &self.slots {",
        "fn take_each(&self, mut apply: impl FnMut(u64), visited: &mut usize) {\n"
        "        for slot in &self.slots {\n            *visited += 1;",
    )
    scan = rq[rq.index("pub(super) fn take_held"):rq.index("/// Uncount a held placement")]
    scan = scan.replace(
        "pub(super) fn take_held(mut apply: impl FnMut(u64, u64, usize))",
        "fn take_held(&self, mut apply: impl FnMut(u64, u64, usize), visited: &mut usize)",
    ).replace("HELD_SLOTS", "self.held_slots").replace(
        "for slot in INDEX.iter() {",
        "for slot in self.index.iter() {\n        *visited += 1;",
    ).replace("release_held_place(word);", "")
    return buffer + r"""
use std::sync::atomic::{AtomicU64, Ordering};
const HELD_TID_SHIFT: u32 = 16;
const HELD_HOME_SHIFT: u32 = 8;
const HELD_BITS: u64 = 0xff;

struct Slot { held: AtomicU64 }
struct Queue {
    buffer: IsrWakeupBuffer,
    held_slots: AtomicU64,
    index: Box<[Slot]>,
}

impl Queue {
    fn new() -> Self {
        Self {
            buffer: IsrWakeupBuffer::new(),
            held_slots: AtomicU64::new(0),
            index: (0..4096).map(|_| Slot { held: AtomicU64::new(0) }).collect(),
        }
    }

    fn publish(&self, tid: usize) {
        // Hard IRQ returns without doing anything with a Full result.
        let _ = self.buffer.push(tid as u64);
    }

    fn hold(&self, tid: usize) {
        self.held_slots.fetch_add(1, Ordering::Relaxed);
        self.index[tid - 1].held.store((tid as u64) << HELD_TID_SHIFT | 1, Ordering::Release);
    }

    fn drain(&self, mut wake: impl FnMut(usize)) -> (usize, bool) {
        let mut visited = 0;
        self.buffer.take_each(|tid| wake(tid as usize), &mut visited);
        self.take_held(|tid, _, _| wake(tid as usize), &mut visited);
        (visited, false)
    }
""" + scan + "}"


class DurableIrqWakes(unittest.TestCase):
    def test_fixed_interleavings(self):
        if TRANSPORT.exists():
            adapter = f'#[path = "{TRANSPORT}"] mod wake_inbox;\n' + LIVE_ADAPTER
        else:
            adapter = legacy_adapter()
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "wake.rs"
            binary = Path(directory) / "wake"
            source.write_text(adapter + HARNESSES)
            subprocess.run(
                ["rustc", "--edition=2021", "--test", "-A", "dead_code", str(source), "-o", str(binary)],
                check=True,
            )
            subprocess.run([str(binary)], check=True, timeout=20)


if __name__ == "__main__":
    unittest.main()
