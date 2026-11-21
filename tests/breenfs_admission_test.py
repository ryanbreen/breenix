#!/usr/bin/env python3
"""Run the production admission loop with an unscheduled commit worker.

The normal host adapter compiles cfg(test), which used to drive commit_due
inline and hide a self-wait in the kernel worker. Compile the actual method
without cfg(test); the fixture's wait rejects sleeping before any publisher
can make progress. Only the mount/device surroundings are modeled.
"""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def extract_method(source, name):
    start = source.index(f'    fn {name}(')
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


def admission():
    source = (ROOT / 'kernel/src/fs/breenfs/vfs.rs').read_text()
    return '\n'.join(extract_method(source, name) for name in (
        'mutation_admission_lock', 'foreground_lock', 'mutation_wait', 'admit',
    ))


class Admission(unittest.TestCase):
    def test_worker_publishes_before_waiting_for_dirty_room(self):
        harness = r'''
use std::cell::{Cell, RefCell, RefMut};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
const DIRTY_BUDGET: u64 = 8 * 1024 * 1024;
const EPOCH: u64 = 17;
#[derive(Debug)] struct Errno;
trait Wait {}
impl Wait for () {}
type MountGuard<'a, T> = RefMut<'a, T>;
struct MountState<W> { dirty_bytes: u64, _wait: std::marker::PhantomData<W> }
impl<W> MountState<W> {
    fn writable(&self) -> Result<(), Errno> { Ok(()) }
    fn charged(&self) -> u64 { self.dirty_bytes }
}
struct Lock<T>(RefCell<T>);
impl<T> Lock<T> {
    fn lock(&self) -> RefMut<'_, T> { self.0.borrow_mut() }
    fn lock_interruptible(&self) -> Result<RefMut<'_, T>, Errno> { Ok(self.lock()) }
}
#[derive(Clone, Copy)]
enum CostOp { Foreground, Writeback }
enum CostPhase { Wait, MountExclusion, Publication }
struct Costs;
impl Costs { fn span(&self, _: CostOp, _: CostPhase) {} }
struct Worker;
impl Worker {
    fn wake(&self) {}
}
struct Signal<W> {
    pressure: AtomicBool,
    requested: AtomicBool,
    worker: Worker,
    detached: bool,
    state: Rc<Lock<MountState<W>>>,
    waits: Cell<usize>,
}
impl<W> Signal<W> {
    fn epoch(&self) -> u64 { EPOCH }
    fn wait(&self, observed: u64, deadline: Option<u64>) -> Result<(), Errno> {
        assert!(self.detached, "the only publisher waited for its own generation");
        assert_eq!(observed, EPOCH);
        assert_eq!(deadline, None);
        self.state.lock().dirty_bytes = 0;
        self.waits.set(self.waits.get() + 1);
        Ok(())
    }
}
struct Inner<W> { state: Rc<Lock<MountState<W>>>, signal: Signal<W>, costs: Costs }
struct BreenFs<W> { inner: Inner<W>, commits: Cell<usize> }
fn monotonic_ns() -> u64 { 0 }
impl<W: Wait> BreenFs<W> {
    fn materialized<'a>(&self, state: MountGuard<'a, MountState<W>>) -> MountGuard<'a, MountState<W>> {
        state
    }
    fn commit_generation(&self, _: u64) -> Result<bool, Errno> {
        assert!(self.inner.signal.requested.load(Ordering::Acquire));
        assert_eq!(self.commits.replace(1), 0, "retried admission without waiting");
        let owns_publication = !self.inner.signal.detached;
        if owns_publication {
            self.inner.state.lock().dirty_bytes = 0;
        }
        Ok(owns_publication)
    }
    ADMISSION
}
fn fixture(detached: bool) -> BreenFs<()> {
    let state = Rc::new(Lock(RefCell::new(MountState {
        dirty_bytes: DIRTY_BUDGET - 4096,
        _wait: std::marker::PhantomData,
    })));
    BreenFs { inner: Inner {
        state: Rc::clone(&state),
        signal: Signal {
            pressure: AtomicBool::new(false),
            requested: AtomicBool::new(false),
            worker: Worker,
            detached,
            state,
            waits: Cell::new(0),
        },
        costs: Costs,
    }, commits: Cell::new(0) }
}
fn main() {
    let fs = fixture(false);
    let admitted = fs.admit(64 * 4096, CostOp::Writeback).unwrap();
    assert!(admitted.charged() + 64 * 4096 <= DIRTY_BUDGET);
    assert_eq!(fs.commits.get(), 1);

    let fs = fixture(true);
    let admitted = fs.admit(64 * 4096, CostOp::Foreground).unwrap();
    assert!(admitted.charged() + 64 * 4096 <= DIRTY_BUDGET);
    assert_eq!(fs.commits.get(), 1);
    assert_eq!(fs.inner.signal.waits.get(), 1);
}
'''.replace('    ADMISSION', admission())
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'admission.rs'
            binary = Path(directory) / 'admission'
            source.write_text(harness)
            subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(source), '-o', str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
