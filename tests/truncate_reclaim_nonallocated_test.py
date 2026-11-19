"""Replay the guest truncate cases against E24's durable marker receipts."""
import unittest

from ui_idle_capture_test import ROOT, function, run_rust

FILES_IO = 'userspace/programs/src/suite_files_io.rs'


class TruncateNonallocated(unittest.TestCase):
    def test_cases_require_the_gain_at_each_post_truncate_marker(self):
        source = (ROOT / FILES_IO).read_text()
        start = source.index('struct RootMarker(')
        end = source.index('\nfn metadata_ftruncate_large()', start)
        cases = (
            source[start:end],
            function(FILES_IO, 'fn metadata_ftruncate_large()'),
            function(FILES_IO, 'fn metadata_statfs_allocation()'),
        )
        harness = r'''
use std::cell::Cell;
type CaseResult = Result<(), String>;
type Sample = (u64, u64, u64);

// After-marker rows 0-5 from ledger 645's files-io-marker-receipts.txt.
// The capture ended there. Rows 6-11 exercise the later truncate to zero.
const LARGE: [Sample; 12] = [
    (2469, 24824, 134), (2470, 24869, 89), (2471, 24852, 106),
    (2472, 24859, 227), (2473, 24863, 223), (2474, 24976, 110),
    (2475, 24976, 108), (2476, 24976, 108), (2477, 24976, 108),
    (2478, 24977, 109), (2479, 24978, 108), (2480, 24977, 109),
];
// The statfs case had no raw receipts. Its synthetic +2 is all in PENDING.
const STATFS: [Sample; 9] = [
    (1, 1002, 10), (2, 1002, 10), (3, 1002, 10),
    (4, 1000, 10), (5, 1000, 10), (6, 1000, 10),
    (7, 1000, 12), (8, 1000, 12), (9, 1000, 12),
];
thread_local! {
    static ROWS: Cell<&'static [Sample]> = Cell::new(&LARGE);
    static CASE: Cell<&'static str> = Cell::new("");
    static FAULT: Cell<Option<(usize, u64)>> = Cell::new(None);
    static COMMITS: Cell<usize> = Cell::new(0);
    static SAMPLES: Cell<usize> = Cell::new(0);
    static NEXT_FIXTURE: Cell<Fd> = Cell::new(MARKER);
    static SIZE: Cell<i64> = Cell::new(0);
    static BLOCKS: Cell<u64> = Cell::new(0);
}

#[derive(Clone, Copy, PartialEq)]
enum Fd { File, Marker }
const FILE: Fd = Fd::File;
const MARKER: Fd = Fd::Marker;
const SEEK_SET: u32 = 0;
const O_RDWR: u32 = 2;

struct Fixture(Fd);
impl Fixture {
    fn new(_: &[u8]) -> Result<Self, String> {
        Ok(Self(NEXT_FIXTURE.replace(MARKER)))
    }
    fn fd(&self) -> Fd { self.0 }
}
struct Contract { reuses_inode_numbers: bool }
impl Contract {
    fn blocks(&self, data: u64, _: u64, sectors: u64) -> u64 { data * sectors }
}
fn root_contract() -> Result<Contract, String> {
    Ok(Contract { reuses_inode_numbers: false })
}
fn prepare_large_truncate_fixture(_: &str) -> CaseResult {
    SIZE.set(129 * 4096);
    BLOCKS.set(129);
    Ok(())
}
struct Metadata { st_size: i64, st_blocks: u64 }
fn fstat(_: Fd) -> Result<Metadata, String> {
    Ok(Metadata { st_size: SIZE.get(), st_blocks: BLOCKS.get() * 8 })
}
fn truncate_fd(fd: Fd, size: i64) -> Result<u64, String> {
    if fd == FILE {
        SIZE.set(size);
        BLOCKS.set(BLOCKS.get().min((size as u64).div_ceil(4096)));
    }
    Ok(0)
}
fn write_all(fd: Fd, data: &[u8]) -> CaseResult {
    if fd == FILE {
        SIZE.set(SIZE.get().max(data.len() as i64));
        BLOCKS.set(2);
    }
    Ok(())
}
fn sync_fd(fd: Fd, _: bool) -> CaseResult {
    assert!(fd == MARKER);
    COMMITS.set(COMMITS.get() + 1);
    Ok(())
}
fn contents(_: Fd, bytes: &[u8]) -> CaseResult {
    assert_eq!(SIZE.get(), bytes.len() as i64);
    Ok(())
}
fn root_free_blocks() -> Result<u64, String> { panic!("ext2-only path") }
fn check(ok: bool, detail: &str) -> CaseResult {
    if ok { Ok(()) } else { Err(detail.into()) }
}
fn fs_stats(_: Option<&str>, _: Fd) -> Result<[u64; 15], String> { Ok([0; 15]) }
fn stats_fields(_: &[u64; 15]) -> CaseResult { Ok(()) }
fn stable_stats(a: &[u64; 15], b: &[u64; 15]) -> bool { a == b }
fn elapsed_ms(_: ()) -> Result<u64, String> { panic!("ext2-only path") }
mod time { pub fn now_monotonic() -> Result<(), String> { panic!("ext2-only path") } }
mod process { pub fn yield_now() -> Result<(), String> { panic!("ext2-only path") } }
mod io { pub fn close(_: super::Fd) -> Result<(), String> { Ok(()) } }
mod fs {
    use super::{Fd, COMMITS, FILE, ROWS};
    pub struct Statfs { pub f_bfree: u64 }
    pub fn fstatfs(_: Fd) -> Result<Statfs, String> {
        let free = ROWS.get()[COMMITS.get() - 1].1;
        Ok(Statfs { f_bfree: free })
    }
    pub fn open(_: &str, _: u32) -> Result<Fd, String> { Ok(FILE) }
    pub fn lseek(_: Fd, offset: i64, _: u32) -> Result<i64, String> { Ok(offset) }
}
mod breenix_suite { pub mod suite {
    pub fn running_case() -> Option<&'static str> { Some(crate::CASE.get()) }
    pub mod control { pub mod markers {
        pub fn sample_root(index: usize, after: bool) -> Option<crate::Sample> {
            use crate::{CASE, COMMITS, FAULT, ROWS, SAMPLES};
            assert_eq!(COMMITS.get(), index + usize::from(after));
            SAMPLES.set(SAMPLES.get() + 1);
            if !after && index == 0 {
                return Some(if CASE.get() == "metadata/ftruncate-large" {
                    (2468, 24813, 146)
                } else {
                    (0, 1002, 10)
                });
            }
            let (generation, free, mut pending) = ROWS.get()[COMMITS.get() - 1];
            if let Some((at, retained)) = FAULT.get() {
                if after && index == at { pending -= retained; }
            }
            Some((generation, free, pending))
        }
    } }
} }

CASE_BODIES

fn replay(case: fn() -> CaseResult, name: &'static str, rows: &'static [Sample],
          fault: Option<(usize, u64)>) -> CaseResult {
    CASE.set(name);
    ROWS.set(rows);
    FAULT.set(fault);
    COMMITS.set(0);
    SAMPLES.set(0);
    NEXT_FIXTURE.set(if name == "metadata/statfs-allocation" { FILE } else { MARKER });
    SIZE.set(0);
    BLOCKS.set(0);
    let result = case();
    if result.is_ok() {
        assert_eq!(COMMITS.get(), rows.len(), "marker sequence changed");
        assert_eq!(SAMPLES.get(), 2 * rows.len(), "missing raw marker sides");
    }
    result
}

fn main() {
    // The original case did not use the y2 helper. Keep it part of the host
    // compilation when running this check against that version.
    let _ = RootMarker::reclaim;
    let large = |fault| replay(metadata_ftruncate_large, "metadata/ftruncate-large", &LARGE, fault);
    let statfs = |fault| replay(metadata_statfs_allocation, "metadata/statfs-allocation", &STATFS, fault);
    let result = large(None);
    assert!(result.is_ok(), "645's +128 FREE+PENDING capture must pass: {result:?}");
    for marker in 3..=5 {
        let error = large(Some((marker, 4))).expect_err("a +124 gain passed");
        assert!(error.starts_with(&format!("marker {marker} ")), "{error}");
    }
    let error = large(Some((10, 1))).expect_err("zero truncate missed its two-block bound");
    assert!(error.starts_with("marker 10 "), "{error}");
    let result = statfs(None);
    assert!(result.is_ok(), "statfs truncate with +2 in PENDING must pass: {result:?}");
    let error = statfs(Some((7, 1))).expect_err("statfs truncate accepted a one-block gain");
    assert!(error.starts_with("marker 7 "), "{error}");
}
'''
        result = run_rust(harness.replace('CASE_BODIES', '\n'.join(cases)))
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
