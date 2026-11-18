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
        preempt = (ROOT / 'kernel/src/fs/breenfs/preempt.rs').read_text()
        scheduler = (ROOT / 'kernel/src/task/scheduler.rs').read_text()
        # Compile the kernel-only predicate, not preempt.rs's cfg(test) model.
        predicate = function(preempt, 'can_yield')
        idle_query = function(scheduler, 'is_current_idle_thread')
        implementation = scheduler[scheduler.index('impl Scheduler {'):]
        methods = '\n'.join(function(implementation, name) for name in
                            ('current_thread_id_inner', 'idle_thread_id'))
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                cpu = re.search(rf'#\[cfg\(target_arch = "{arch}"\)\]\s*'
                                r'use crate::(\w+) as cpu;', preempt).group(1)
                harness = r'''
use std::cell::Cell;
thread_local! {
    static COUNT: Cell<u32> = const { Cell::new(1) };
    static IRQ_CONTEXT: Cell<bool> = const { Cell::new(false) };
    static IRQ_ON: Cell<bool> = const { Cell::new(true) };
    static CURRENT: Cell<Option<u64>> = const { Cell::new(Some(7)) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}
mod __CPU__ {
    pub fn preempt_count() -> u32 { crate::COUNT.get() }
    pub fn in_interrupt() -> bool { crate::IRQ_CONTEXT.get() }
}
fn arch_interrupts_enabled() -> bool { IRQ_ON.get() }
mod task { pub mod scheduler {
    struct CpuState { current_thread: Option<u64>, idle_thread: u64 }
    struct Scheduler { cpu_state: [CpuState; 1] }
    impl Scheduler {
        fn current_cpu_id() -> usize { 0 }
        __METHODS__
    }
    fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
        let on = crate::IRQ_ON.replace(false);
        let result = f();
        crate::IRQ_ON.set(on);
        result
    }
    fn try_lock_scheduler() -> Option<Option<Scheduler>> {
        assert!(!crate::IRQ_ON.get(), "scheduler read must be IRQ masked");
        (!crate::BUSY.get()).then_some(Some(Scheduler { cpu_state: [CpuState {
            current_thread: crate::CURRENT.get(), idle_thread: 7,
        }] }))
    }
    pub fn current_thread_id() -> Option<u64> { crate::CURRENT.get() }
    __IDLE_QUERY__
} }
use crate::__CPU__ as cpu;
__PREDICATE__
fn main() {
    let _ = task::scheduler::is_current_idle_thread as fn() -> Option<bool>;
    // Both boots publish their running continuation as the scheduler's idle
    // thread; ARM additionally pins it at count 1 before starting the timer.
    assert!(!can_yield(), "bootstrap/idle continuation must stay pinned");
    CURRENT.set(Some(8)); // ordinary schedulable breenfs-commit kthread
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
    BUSY.set(true);
    assert!(!can_yield(), "scheduler identity unavailable");
    BUSY.set(false);
    assert!(can_yield());
}
'''
                run_rust(harness.replace('__CPU__', cpu).replace('__METHODS__', methods)
                         .replace('__IDLE_QUERY__', idle_query).replace('__PREDICATE__', predicate))

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
