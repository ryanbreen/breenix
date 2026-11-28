#!/usr/bin/env python3
"""24.15m / E24 ledger 503: execute production yield and root-label decisions."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    match = re.search(rf'(?m)^\s*(?:pub(?:\([^)]*\))? )?fn {name}\(', source)
    if match is None:
        raise AssertionError(f'missing production function {name}')
    start = source.index('fn ', match.start())
    opened = source.index('{', start)
    depth, end = 1, opened + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return 'pub ' + source[start:end]


def run_rust(source):
    with tempfile.TemporaryDirectory(prefix='breenfs-yield-') as tmp:
        path = Path(tmp) / 'check.rs'
        path.write_text(source)
        binary = path.with_suffix('')
        subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                        str(path), '-o', str(binary)], check=True, timeout=30)
        subprocess.run([str(binary)], check=True, timeout=10)


class YieldPredicate(unittest.TestCase):
    def test_bootstrap_and_worker_on_both_architectures(self):
        # E2.2: can_yield asks the park contract. The harness compiles the
        # production predicate and the production park_core::classify, and
        # gathers classify's inputs as task::park::context does.
        preempt = (ROOT / 'kernel/src/fs/breenfs/preempt.rs').read_text()
        predicate = function(preempt, 'can_yield')
        self.assertIn('crate::task::park::sleepable_braked()', predicate)
        park_core = ROOT / 'kernel/src/task/park_core.rs'
        harness = r'''
use std::cell::Cell;
thread_local! {
    static COUNT: Cell<u32> = const { Cell::new(1) };
    static BASELINE: Cell<u32> = const { Cell::new(0) };
    static IRQ_CONTEXT: Cell<bool> = const { Cell::new(false) };
    static IRQ_ON: Cell<bool> = const { Cell::new(true) };
    static CURRENT: Cell<Option<u64>> = const { Cell::new(Some(7)) };
}
const IDLE: u64 = 7;
fn arch_interrupts_enabled() -> bool { IRQ_ON.get() }
/// park_core's queue operations, which classify never reaches.
#[allow(dead_code)]
mod event_queue {
    pub enum PrepareOutcome { Mismatch, Queued, PublishFailed, OutOfMemory }
    pub trait Hooks { fn current_tid(&self) -> Option<u64>; fn end_wait(&self, tid: u64); }
    pub struct EventQueue;
    impl EventQueue {
        pub fn prepare_checked<H: Hooks, F: FnOnce() -> bool>(&self, _: &H, _: Option<u64>, _: F) -> PrepareOutcome { unreachable!() }
        pub fn finish_for<H: Hooks>(&self, _: &H, _: u64) { unreachable!() }
        pub fn add<H: Hooks>(&self, _: &H, _: u64) -> bool { unreachable!() }
        pub fn take<H: Hooks>(&self, _: &H, _: u64) -> bool { unreachable!() }
    }
}
#[allow(dead_code)]
#[path = "__PARK_CORE__"]
mod park_core;
mod task { pub mod park {
    use crate::park_core::{classify, ContextInputs, ParkContext};
    pub fn sleepable_braked() -> bool {
        let tid = crate::CURRENT.get();
        let inputs = ContextInputs {
            hardirq: crate::IRQ_CONTEXT.get(),
            dispatching: tid.is_some(),
            has_thread: tid.is_some(),
            idle: tid == Some(crate::IDLE),
            depth: crate::COUNT.get(),
            baseline: crate::BASELINE.get(),
            irqs_masked: !crate::IRQ_ON.get(),
            ..ContextInputs::default()
        };
        classify(inputs) == ParkContext::Sleepable && crate::BASELINE.get() >= 1
    }
} }
__PREDICATE__
fn main() {
    // Both boots publish their running continuation as the scheduler's idle
    // thread; ARM additionally pins it at count 1 before starting the timer.
    assert!(!can_yield(), "bootstrap/idle continuation must stay pinned");
    CURRENT.set(Some(8)); // breenfs-commit, its pass under park::Braked
    assert!(!can_yield(), "a worker holding a bare level is not a braked context");
    BASELINE.set(1);
    assert!(can_yield(), "commit worker must retain cooperative progress");
    IRQ_CONTEXT.set(true);
    assert!(!can_yield(), "interrupt context");
    IRQ_CONTEXT.set(false);
    IRQ_ON.set(false);
    assert!(!can_yield(), "IRQs masked");
    IRQ_ON.set(true);
    COUNT.set(2); // entry brake plus a preemption-disabling spin guard
    assert!(!can_yield(), "nested spin guard");
    COUNT.set(1);
    CURRENT.set(None);
    assert!(!can_yield(), "unpublished bootstrap thread");
    CURRENT.set(Some(8));
    assert!(can_yield());
}
'''
        run_rust(harness.replace('__PARK_CORE__', str(park_core)).replace('__PREDICATE__', predicate))

    def test_common_boot_labels_use_the_mounted_backend(self):
        # Keep the architecture entry points wired to the mounted table, rather
        # than the launcher's declaration or the filesystem that was probed first.
        for arch in ('main.rs', 'main_aarch64.rs'):
            source = (ROOT / 'kernel/src' / arch).read_text()
            mounted = re.search(r'(?:log::info!|serial_println!)\(\s*'
                                r'"[^"\n]*root filesystem mounted[^;]+;', source)
            self.assertIsNotNone(mounted, arch)
            self.assertIn('kernel::fs::vfs::root_kind()', mounted.group(), arch)
        arm = (ROOT / 'kernel/src/main_aarch64.rs').read_text()
        preload = re.findall(r'serial_println!\(\s*"\[boot\] Pre-loading [^;]+;', arm)
        self.assertEqual(len(preload), 2)
        for label in preload:
            self.assertIn('kernel::fs::vfs::root_kind()', label)
        boot = (ROOT / 'kernel/src/fs/vfs/boot.rs').read_text()
        run_rust(r'''
use std::sync::Arc;
#[derive(Debug)] pub struct Errno(i32);
const ENODEV: i32 = 19;
trait FileSystem { fn kind(&self) -> Result<&'static str, Errno>; }
struct Backend(&'static str);
impl FileSystem for Backend {
    fn kind(&self) -> Result<&'static str, Errno> { Ok(self.0) }
}
mod mount {
    use super::*;
    std::thread_local! { pub static MOUNTED: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
    pub fn find_mount(path: &str) -> Option<usize> {
        assert_eq!(path, "/"); MOUNTED.get().map(|_| 1)
    }
    pub fn filesystem(id: usize) -> Result<Arc<dyn FileSystem>, Errno> {
        assert_eq!(id, 1); Ok(Arc::new(Backend(MOUNTED.get().unwrap())))
    }
}
mod boot {
    use super::*;
    __ROOT_FILESYSTEM__
    __ROOT_KIND__
}
fn main() {
    assert_eq!(boot::root_kind(), "unmounted");
    for name in ["ext2", "breenfs"] {
        mount::MOUNTED.set(Some(name));
        assert_eq!(boot::root_kind(), name);
    }
    assert_eq!(Errno(ENODEV).0, ENODEV);
}
'''.replace('__ROOT_FILESYSTEM__', function(boot, 'root_filesystem'))
                 .replace('__ROOT_KIND__', function(boot, 'root_kind')))


if __name__ == '__main__':
    unittest.main()
