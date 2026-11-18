"""E24 coordinator 583(y2): execute the guest oracle over raw marker captures."""
from pathlib import Path
import unittest
from ui_idle_capture_test import function, run_rust

ROOT = Path(__file__).resolve().parents[1]


class UnlinkNonallocated(unittest.TestCase):
    def test_capture_passes_and_every_post_unlink_marker_is_required(self):
        source = (ROOT / 'userspace/programs/src/suite_files_io.rs').read_text()
        marker = source[source.index('struct RootMarker('):source.index('\nfn metadata_ftruncate_large()')]
        case = function('userspace/programs/src/suite_files_io.rs',
                        'fn descriptors_unlink_reclaim_breenfs()')
        harness = r'''
use std::sync::atomic::{AtomicUsize, Ordering};
type Fd = usize;
type CaseResult = Result<(), String>;
const SEEK_SET: u32 = 0;
static FAULT: AtomicUsize = AtomicUsize::new(0);
static INDEX: AtomicUsize = AtomicUsize::new(0);
static AFTER: AtomicUsize = AtomicUsize::new(0);
struct Fixture { file: Option<Fd>, path: String }
impl Fixture {
    fn new(_: &[u8]) -> Result<Self, String> {
        Ok(Self { file: Some(1), path: String::new() })
    }
    fn fd(&self) -> Fd { self.file.unwrap() }
}
fn orphan_fixture() -> Result<Fixture, String> {
    Ok(Fixture { file: Some(2), path: format!("{}/victim", std::env::var("ORACLE_DIR").unwrap()) })
}
struct Metadata { st_ino: u64, st_blocks: u64 }
fn fstat(_: Fd) -> Result<Metadata, String> { Ok(Metadata { st_ino: 42, st_blocks: 16 }) }
fn stat(_: &str, _: bool) -> Result<(), String> { Err("ENOENT".into()) }
fn expect_errno(r: Result<(), String>, n: i32, _: &str) -> CaseResult {
    assert_eq!(n, 2); assert_eq!(r, Err("ENOENT".into())); Ok(())
}
fn check(ok: bool, text: &str) -> CaseResult { if ok { Ok(()) } else { Err(text.into()) } }
fn truncate_fd(_: Fd, _: u64) -> CaseResult { Ok(()) }
fn write_all(_: Fd, _: &[u8]) -> CaseResult { Ok(()) }
fn sync_fd(_: Fd, _: bool) -> CaseResult { Ok(()) }
mod io { pub fn close(_: usize) -> Result<(), String> { Ok(()) } }
// The seven after-marker raw records from 581. Faults plant an insufficient
// non-allocated gain at one marker, even when all later markers recover.
fn raw(index: usize) -> Option<(u64,u64,u64)> {
    let rows = [(4672,24840,102), (4673,24842,100), (4674,24844,98),
                (4675,24825,119), (4676,24837,107), (4677,24846,98), (4678,24830,114)];
    let (g,f,p) = rows[index];
    let fault = FAULT.load(Ordering::SeqCst);
    if fault == 20 && index == 3 { return None; }
    if fault == 21 && index == 3 { return Some((g,u64::MAX,1)); }
    Some((g,f,if fault == index && index >= 3 { p - 1 } else { p }))
}
mod fs {
    pub struct Statfs { pub f_bsize: i64, pub f_bfree: u64 }
    pub fn fstatfs(_: usize) -> Result<Statfs, String> {
        Ok(Statfs { f_bsize: 4096, f_bfree: super::raw(super::INDEX.load(super::Ordering::SeqCst)).unwrap().1 })
    }
    pub fn unlink(_: &str) -> Result<(),String> { Ok(()) }
    pub fn lseek(_: usize, _: u64, _: u32) -> Result<u64,String> { Ok(0) }
}
mod breenix_suite { pub mod suite {
    pub fn running_case() -> Option<&'static str> { Some("descriptors/unlink-reclaim") }
    pub mod control { pub mod markers {
        pub fn sample_root(index: usize, after: bool) -> Option<(u64,u64,u64)> {
            super::super::super::super::INDEX.store(index, super::super::super::super::Ordering::SeqCst);
            if after { super::super::super::super::AFTER.fetch_add(1, super::super::super::super::Ordering::SeqCst); }
            super::super::super::super::raw(index)
        }
    } }
} }
MARKER
CASE
fn main() {
    let directory = std::env::temp_dir().join(format!("breenfs-y2-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    std::env::set_var("ORACLE_DIR", &directory);
    let _truncate_marker = RootMarker::free;
    let result = descriptors_unlink_reclaim_breenfs();
    assert!(result.is_ok(), "583 capture must pass despite FREE falling: {result:?}");
    assert_eq!(AFTER.load(Ordering::SeqCst), 7, "equalized marker sequence changed");
    for fault in [3,4,5,6,20,21] {
        FAULT.store(fault, Ordering::SeqCst);
        AFTER.store(0, Ordering::SeqCst);
        let result = descriptors_unlink_reclaim_breenfs();
        assert!(result.is_err(), "missing, overflowing, or insufficient marker {fault} passed");
    }
    std::fs::remove_dir(directory).unwrap();
}
'''.replace('MARKER', marker).replace('CASE\n', case + '\n')
        result = run_rust(harness)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
