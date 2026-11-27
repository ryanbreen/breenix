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
    use std::sync::{atomic::{AtomicBool, Ordering}, Mutex};
    let receipt = AtomicBool::new(false);
    let state = Mutex::new(false);
    let mut waiter = state.lock().unwrap();
    assert!(!receipt.load(Ordering::Acquire));
    // The IRQ publishes its receipt and wake after the locked check.
    receipt.store(true, Ordering::Release);
    queue.publish(65);
    // The owner (or a stalled-owner helper) cannot drain before the blocked
    // state is published, even though the IRQ can publish without this lock.
    assert!(state.try_lock().is_err());
    *waiter = true;
    drop(waiter);
    let mut waiter = state.lock().unwrap();
    while queue.drain(|tid| {
        if tid == 65 {
            assert!(*waiter);
            *waiter = false;
        }
    }).1 {}
    assert!(!*waiter, "the completion lost its wake between check and block");
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
        // The fixtures drain sequentially, as the scheduler does under its lock.
        let more = unsafe {
            self.inbox.drain(|tid, _| {
                visited += 1;
                wake(tid as usize);
            })
        };
        (visited, more)
    }
}
"""


class DurableIrqWakes(unittest.TestCase):
    def test_fixed_interleavings(self):
        adapter = f'#[path = "{TRANSPORT}"] mod wake_inbox;\n' + LIVE_ADAPTER
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
