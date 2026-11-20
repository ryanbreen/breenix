"""Run the production canvas wait with a deterministic presentation wake."""

import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def function(path, name):
    source = (ROOT / "kernel/src" / path).read_text()
    start = source.index(f"fn {name}(")
    opening = source.index("{", start)
    depth = 1
    for end in range(opening + 1, len(source)):
        if source[end] == "{":
            depth += 1
        elif source[end] == "}":
            depth -= 1
            if depth == 0:
                return source[start : end + 1]
    raise AssertionError(f"{path}: unterminated {name}")


HARNESS = r"""
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

static PRESENTING: AtomicBool = AtomicBool::new(false);

thread_local! {
    static WAITING: Cell<bool> = const { Cell::new(false) };
    static PARK_COUNT: Cell<Option<u32>> = const { Cell::new(None) };
}

fn arch_halt_while_blocked() {
    PARK_COUNT.with(|count| assert!(count.replace(Some(per_cpu::preempt_count())).is_none()));
    PRESENTING.store(false, Ordering::SeqCst);
    WAITING.with(|waiting| waiting.set(false));
}

mod arch_impl {
    pub mod current {
        pub mod constants {
            pub const PREEMPT_MASK: u32 = 0xff;
        }
    }
}

mod per_cpu {
    use std::sync::atomic::{compiler_fence, AtomicBool, AtomicU64, Ordering};

    static PER_CPU_INITIALIZED: AtomicBool = AtomicBool::new(true);
    pub(super) static PREEMPT_UNDERFLOW_COUNT: AtomicU64 = AtomicU64::new(0);
    use crate::arch_impl::current::constants::PREEMPT_MASK;
    fn run_pending_softirqs() {}
    mod hal_percpu {
        use std::cell::Cell;
        thread_local! { static COUNT: Cell<u32> = const { Cell::new(0) }; }

        pub struct X86PerCpu;
        impl X86PerCpu {
            pub fn preempt_count() -> u32 { COUNT.with(Cell::get) }
            pub fn preempt_disable() { COUNT.with(|n| n.set(n.get() + 1)); }
            pub fn preempt_enable() { COUNT.with(|n| n.set(n.get() - 1)); }
        }
    }

    pub fn preempt_count() -> u32 { hal_percpu::X86PerCpu::preempt_count() }
    pub fn underflows() -> u64 { PREEMPT_UNDERFLOW_COUNT.load(Ordering::Relaxed) }

    pub @PREEMPT_ENABLE@
    pub @PREEMPT_DISABLE@
}

mod task {
    pub mod preempt_fault {
        // The deferred receipt belongs to the kernel. Count any unpaired release
        // here even when the host harness cannot publish a serial notice.
        #[allow(dead_code)]
        pub fn underflow(_: u32) {
            crate::per_cpu::PREEMPT_UNDERFLOW_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    pub mod thread {
        #[derive(PartialEq)]
        pub enum ThreadState { Ready, BlockedOnIO }
    }
    pub mod scheduler {
        use super::thread::ThreadState;

        pub fn current_thread_id() -> Option<u64> { Some(26) }
        pub struct Thread { pub state: ThreadState }
        impl Thread {
            pub fn current_thread_mut(&mut self) -> Option<&mut Self> { Some(self) }
        }
        pub fn with_current<R>(f: impl FnOnce(&mut Thread) -> R) -> Option<R> {
            let state = if crate::WAITING.with(std::cell::Cell::get) {
                ThreadState::BlockedOnIO
            } else { ThreadState::Ready };
            Some(f(&mut Thread { state }))
        }
        pub fn yield_current() {}
    }
    pub mod waitqueue {
        use super::thread::ThreadState;

        #[derive(PartialEq)]
        #[allow(dead_code)]
        pub enum PrepareOutcome { Mismatch, PublishFailed, OutOfMemory, Queued }
        pub struct WaitQueueHead;
        impl WaitQueueHead {
            pub fn prepare_to_wait_checked(
                &self, _: ThreadState, _: Option<u64>, condition: impl FnOnce() -> bool,
            ) -> PrepareOutcome {
                if !condition() { return PrepareOutcome::Mismatch; }
                crate::WAITING.with(|waiting| assert!(!waiting.replace(true)));
                PrepareOutcome::Queued
            }
            pub fn finish_wait(&self) {}
            pub fn take_waiter(&self, tid: u64) { assert_eq!(tid, 26); }
            pub fn finish_wait_for(&self, tid: u64) { assert_eq!(tid, 26); }
        }
    }
}

mod time {
    pub fn get_monotonic_time_ns() -> (u64, u64) { (0, 0) }
}
mod drivers {
    pub mod virtio {
        pub mod gpu_pci {
            pub fn presentation_pending() -> bool { false }
        }
    }
}
mod display {
    pub fn wait_until_idle() {}
}

static PASS_ENDED: task::waitqueue::WaitQueueHead = task::waitqueue::WaitQueueHead;
mod syscall {
    mod errno { pub const ESRCH: i32 = 3; pub const ENOMEM: i32 = 12; }
    pub fn check_signals_for_restartable_wait() -> Option<i32> { None }
    pub mod blocking_io {
        use super::errno;
        use crate::task::{thread::ThreadState, waitqueue::{PrepareOutcome, WaitQueueHead}};
        pub(crate) @WAIT_PREPARED@
    }
}

@CANVAS_WAIT@

fn busy_wait(entry_count: u32) {
    PRESENTING.store(true, Ordering::SeqCst);
    wait_until_not_presenting();
    let observed = per_cpu::preempt_count();
    let underflows = per_cpu::underflows();
    eprintln!("busy canvas: entry={entry_count}, return={observed}, underflows={underflows}");
    assert_eq!(observed, entry_count, "canvas wait must preserve the caller's preempt count");
    assert_eq!(underflows, 0, "canvas wait must never release an unheld level");
    assert_eq!(PARK_COUNT.with(Cell::take), Some(entry_count.saturating_sub(1)));
}

fn main() {
    wait_until_not_presenting();
    assert_eq!((per_cpu::preempt_count(), per_cpu::underflows()), (0, 0));
    assert_eq!(PARK_COUNT.with(Cell::get), None);

    per_cpu::preempt_disable();
    busy_wait(1);
    per_cpu::preempt_disable();
    busy_wait(2);
    per_cpu::preempt_enable();
    per_cpu::preempt_enable();

    busy_wait(0);
}
"""


class CanvasWait(unittest.TestCase):
    def test_busy_wait_preserves_preemption(self):
        source = (
            HARNESS.replace("@CANVAS_WAIT@", function("console/mod.rs", "wait_until_not_presenting"))
            .replace("@WAIT_PREPARED@", function("syscall/blocking_io.rs", "wait_prepared"))
            .replace("@PREEMPT_ENABLE@", function("per_cpu.rs", "preempt_enable"))
            .replace("@PREEMPT_DISABLE@", function("per_cpu.rs", "preempt_disable"))
        )
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "canvas_wait.rs"
            fixture.write_text(source)
            probe = fixture.with_suffix("")
            subprocess.run(
                ["rustc", "--edition=2021", "-C", "debug-assertions=no", str(fixture), "-o", str(probe)],
                check=True,
                timeout=20,
            )
            result = subprocess.run([probe], capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
