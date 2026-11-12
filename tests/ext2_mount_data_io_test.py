"""A namespace create can progress while mapped data writeback is in flight."""
from pathlib import Path
import unittest

from file_map_orphan_cache_test import run
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class MountDataIo(unittest.TestCase):
    def test_service_releases_namespace_before_data_write(self):
        service = item((ROOT / 'kernel/src/fs/ext2/writeback.rs').read_text(), 'fn service()')
        ext2 = (ROOT / 'kernel/src/fs/ext2/mod.rs').read_text()
        # The detached service helper is included when present; the old service
        # calls finalize_writeback directly under its mount guard.
        detached = (item(ext2, 'pub(super) fn service_mapped_writeback(')
                    if 'pub(super) fn service_mapped_writeback(' in ext2 else '')
        result = run(r'''
#![allow(dead_code)]
extern crate alloc;
use std::sync::{RwLock, RwLockWriteGuard, atomic::{AtomicBool, Ordering}};
static ROOT: RwLock<Option<Fs>> = RwLock::new(Some(Fs));
static CREATED: AtomicBool = AtomicBool::new(false);
static DATA_STARTED: AtomicBool = AtomicBool::new(false);
struct FakeDevice;
impl FakeDevice {
    fn write(&self) {
        DATA_STARTED.store(true, Ordering::SeqCst);
        // The data request is in flight until this callback returns. No
        // scheduler or wall clock is involved: try_write is the ordered
        // concurrent namespace operation's admission point.
        let created = ROOT.try_write().is_ok();
        CREATED.store(created, Ordering::SeqCst);
        assert!(created, "create_new waits for another inode's in-flight data write");
    }
}
struct Fs;
impl Fs {
    fn finalize_writeback(&mut self) -> Finalize { FakeDevice.write(); Finalize::Idle }
    fn finalize_metadata(&mut self) -> Finalize { Finalize::Idle }
    fn finalize_pending_orphans(&mut self) -> Finalize { Finalize::Idle }
    fn mapped_writeback_handles(&self) -> Result<Vec<Handle>, &'static str> { Ok(vec![Handle { object: Object { map: Map } }]) }
}
struct Handle { object: Object }
struct Object { map: Map }
struct Map;
impl Map {
    fn service_writeback(&self, _handle: &Handle, _budget: &mut usize) -> Result<bool, &'static str> {
        FakeDevice.write(); Ok(false)
    }
}
enum Finalize { Idle, More, Retry }
fn fs_write_raw(home: bool) -> Result<RwLockWriteGuard<'static, Option<Fs>>, &'static str> {
    if home { panic!("finished") }
    Ok(ROOT.write().unwrap())
}
mod memory { pub mod file_map { pub const EVICT_BUDGET: usize = 64; } }
DETACHED
mod per_cpu { pub fn preempt_disable() {} pub fn preempt_enable() {} }
mod per_cpu_aarch64 { pub fn preempt_disable() {} pub fn preempt_enable() {} }
mod worker {
    use super::*;
    static PENDING: AtomicBool = AtomicBool::new(true);
    static ORPHANS: AtomicBool = AtomicBool::new(false);
    const ORPHAN_COMMIT_NS: u64 = 1;
    static WORK: () = ();
    fn pause(_ns: u64) {}
    fn wait(_w: &(), _deadline: Option<u64>, _condition: impl FnOnce() -> bool) { panic!("finished") }
    SERVICE
}
fn main() {
    let result = std::panic::catch_unwind(worker::run);
    assert!(result.is_err());
    assert!(DATA_STARTED.load(Ordering::SeqCst), "service did not reach device data I/O");
    assert!(CREATED.load(Ordering::SeqCst), "namespace create could not complete during data I/O");
}
'''.replace('DETACHED', detached).replace('SERVICE', service + '\npub fn run() { service(); }'))
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
