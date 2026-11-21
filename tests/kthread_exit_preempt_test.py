"""Exercise balanced and faulty kthread exits against the real notice drain."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from canvas_wait_test import ROOT, function


HARNESS = r"""
extern crate alloc;

#[path = "@KERNEL@/log_notice.rs"]
mod log_notice;

mod serial {
    pub fn write_notices() { unreachable!("the exit path must defer output"); }
}

mod arch_impl {
    pub trait PerCpuOps {
        fn current_thread_ptr() -> *const ();
    }
    pub mod current {
        pub mod constants {
            pub const PREEMPT_MASK: u32 = 0xff;
        }
        pub mod percpu {
            #[cfg(target_arch = "aarch64")]
            pub use crate::Cpu as Aarch64PerCpu;
            #[cfg(target_arch = "x86_64")]
            pub use crate::Cpu as X86PerCpu;
        }
    }
}

pub struct Cpu;
impl arch_impl::PerCpuOps for Cpu {
    fn current_thread_ptr() -> *const () {
        (&*THREAD as *const task::thread::Thread).cast()
    }
}

static THREAD: std::sync::LazyLock<task::thread::Thread> =
    std::sync::LazyLock::new(|| task::thread::Thread { id: 42, name: "worker".into() });

fn arch_without_interrupts<T>(f: impl FnOnce() -> T) -> T { f() }

mod per_cpu {
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
    pub static COUNT: AtomicU32 = AtomicU32::new(0);

    pub fn preempt_count() -> u32 { COUNT.load(Relaxed) }
    pub fn preempt_underflow_count() -> u64 { 0 }
    pub fn preempt_enable() { assert!(COUNT.fetch_sub(1, Relaxed) > 0); }
}

mod task {
    pub mod thread {
        pub struct Thread {
            pub id: u64,
            pub name: String,
        }
    }
    #[path = "@KERNEL@/task/preempt_fault.rs"]
    mod preempt_fault;
    pub mod kthread {
        pub @RELEASE@
    }
}

fn main() {
    use std::sync::atomic::Ordering::Relaxed;
    use task::kthread::release_held_preemption as exit;

    exit();
    assert!(!log_notice::pending(), "balanced exit must stay quiet");
    for count in [2, 1, 3] {
        per_cpu::COUNT.store(count, Relaxed);
        exit();
        assert_eq!(per_cpu::preempt_count(), 0);
    }

    let mut lines = Vec::new();
    log_notice::take(|line| lines.push(std::str::from_utf8(line).unwrap().to_owned()));
    assert_eq!(lines, [
        "[PREEMPT_FAULT reason=kthread_exit_released tid=42 name=worker count=0x2 underflows=0]\n",
        "[PREEMPT_FAULT reason=kthread_exit_suppressed exits=2]\n",
    ]);
    assert!(!log_notice::pending(), "the drain must consume the aggregate");

    per_cpu::COUNT.store(1, Relaxed);
    exit();
    lines.clear();
    log_notice::take(|line| lines.push(std::str::from_utf8(line).unwrap().to_owned()));
    assert_eq!(lines, [
        "[PREEMPT_FAULT reason=kthread_exit_released tid=42 name=worker count=0x1 underflows=0]\n",
    ]);
    assert!(!log_notice::pending());
}
"""


class KthreadExitPreemptTest(unittest.TestCase):
    def test_faults_remain_visible_when_the_detail_record_is_busy(self):
        harness = HARNESS.replace("@KERNEL@", str(ROOT / "kernel/src")).replace(
            "@RELEASE@", function("task/kthread.rs", "release_held_preemption")
        )
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "exit.rs"
            binary = Path(tmp) / "exit"
            source.write_text(harness)
            subprocess.run(
                ["rustc", "--edition=2021", "-A", "dead_code", str(source), "-o", str(binary)],
                check=True,
                timeout=20,
            )
            subprocess.run([str(binary)], check=True, timeout=5)


if __name__ == "__main__":
    unittest.main()
