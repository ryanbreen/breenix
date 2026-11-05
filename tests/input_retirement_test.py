"""Run the production input retirement handoff while its state lock is held."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def item(source, start):
    begin = source.index(start)
    brace = source.index('{', begin)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[begin:end]


class InputRetirement(unittest.TestCase):
    def test_final_close_defers_state_work_without_allocation(self):
        source = (ROOT / 'kernel/src/input/stream.rs').read_text()
        fragments = [item(source, name) for name in (
            'struct Retirement {', 'fn retire_descriptors()',
            'impl Drop for InputFile {')]
        program = r'''
use std::{alloc::{GlobalAlloc, Layout, System}, collections::{BTreeMap, VecDeque},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering}};
struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed); System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
}
#[global_allocator] static ALLOCATOR: Counting = Counting;
static UNDER_PM: AtomicBool = AtomicBool::new(false);
static WAKES: AtomicUsize = AtomicUsize::new(0);
fn wake_service() { WAKES.fetch_add(1, Ordering::Relaxed); }
mod irq_safe_mutex {
    pub struct IrqSafeMutex<T>(std::sync::Mutex<T>);
    impl<T> IrqSafeMutex<T> {
        pub const fn new(v: T) -> Self { Self(std::sync::Mutex::new(v)) }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    }
}
struct StateMutex(std::sync::Mutex<State>);
impl StateMutex {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        assert!(!UNDER_PM.load(Ordering::Relaxed), "STATE entered under PM");
        self.0.lock().unwrap()
    }
}
struct DeviceSlot(Option<u32>);
impl DeviceSlot { fn get_mut(&mut self) -> &mut Option<u32> { &mut self.0 } }
struct InputFile { device: DeviceSlot, retirement: Option<Box<Retirement>> }
struct State { readers: BTreeMap<u64, ()>, grab: Option<u64>, removals: VecDeque<u32> }
impl State {
    fn release_grab(&mut self, id: u64, outcome: u32, why: u32) {
        assert_eq!((outcome, why), (5, 11));
        if self.grab == Some(id) { self.grab = None; }
    }
}
mod disposition { pub const LOST: u32 = 5; }
mod reason { pub const COMPOSITOR_DIED: u32 = 11; }
static STATE: StateMutex = StateMutex(std::sync::Mutex::new(State {
    readers: BTreeMap::new(), grab: None, removals: VecDeque::new() }));
static RETIRED_INPUT: irq_safe_mutex::IrqSafeMutex<Option<Box<Retirement>>> =
    irq_safe_mutex::IrqSafeMutex::new(None);
'''
        program += '\n'.join(fragments)
        program += r'''
fn main() {
    let reader = InputFile { device: DeviceSlot(None), retirement: Some(Box::new(
        Retirement { reader: Some(7), device: None, next: None })) };
    let injector = InputFile { device: DeviceSlot(Some(1024)), retirement: Some(Box::new(
        Retirement { reader: None, device: None, next: None })) };
    let mut held = STATE.lock();
    held.readers.insert(7, ()); held.grab = Some(7);
    drop(RETIRED_INPUT.lock()); // Initialize the host mutex before allocation accounting.
    UNDER_PM.store(true, Ordering::Relaxed);
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    drop(reader); drop(injector);
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), before);
    assert_eq!(WAKES.load(Ordering::Relaxed), 2);
    assert_eq!(held.grab, Some(7)); assert!(held.removals.is_empty());
    UNDER_PM.store(false, Ordering::Relaxed); drop(held);
    retire_descriptors();
    let s = STATE.lock();
    assert!(s.readers.is_empty()); assert_eq!(s.grab, None);
    assert_eq!(s.removals, [1024]); drop(s);
    assert!(RETIRED_INPUT.lock().is_none());
}
'''
        with tempfile.TemporaryDirectory(prefix='input-retirement-') as temp:
            path = Path(temp) / 'check.rs'
            path.write_text(program)
            binary = Path(temp) / 'check'
            compile_result = subprocess.run(
                (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                ['rustc', '--edition=2021', '-Dwarnings',
                 str(path), '-o', str(binary)], cwd=ROOT,
                text=True, capture_output=True, timeout=60,
                env={**os.environ, 'CARGO_BUILD_JOBS': '4'})
            self.assertEqual(compile_result.returncode, 0, compile_result.stderr)
            result = subprocess.run([str(binary)], text=True, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
