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


def admission():
    source = (ROOT / 'kernel/src/fs/breenfs/vfs.rs').read_text()
    start = source.index('    fn admit(')
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class Admission(unittest.TestCase):
    def test_worker_publishes_before_waiting_for_dirty_room(self):
        harness = r'''
use std::cell::{Cell, RefCell, RefMut};
use std::sync::atomic::{AtomicBool, Ordering};
const DIRTY_BUDGET: u64 = 8 * 1024 * 1024;
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
impl<T> Lock<T> { fn lock(&self) -> RefMut<'_, T> { self.0.borrow_mut() } }
struct Signal { pressure: AtomicBool, requested: AtomicBool }
impl Signal {
    fn epoch(&self) -> u64 { 0 }
    fn wake(&self) {}
    fn wait(&self, _: u64, _: Option<u64>) {
        panic!("the only publisher waited for its own generation");
    }
}
struct Inner<W> { state: Lock<MountState<W>>, signal: Signal }
struct BreenFs<W> { inner: Inner<W>, commits: Cell<usize> }
fn monotonic_ns() -> u64 { 0 }
impl<W: Wait> BreenFs<W> {
    fn commit_generation(&self, _: u64) -> Result<bool, Errno> {
        assert!(self.inner.signal.requested.load(Ordering::Acquire));
        self.inner.state.lock().dirty_bytes = 0;
        self.commits.set(self.commits.get() + 1);
        Ok(true)
    }
    ADMISSION
}
fn main() {
    let _publisher = BreenFs::<()>::commit_generation;
    let _clock = monotonic_ns;
    let fs = BreenFs::<()> { inner: Inner {
        state: Lock(RefCell::new(MountState {
            dirty_bytes: DIRTY_BUDGET - 4096,
            _wait: std::marker::PhantomData,
        })),
        signal: Signal { pressure: AtomicBool::new(false), requested: AtomicBool::new(false) },
    }, commits: Cell::new(0) };
    let admitted = fs.admit(64 * 4096).unwrap();
    assert!(admitted.charged() + 64 * 4096 <= DIRTY_BUDGET);
    assert_eq!(fs.commits.get(), 1);
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
