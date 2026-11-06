"""W15: run the production receipt parser with a substituted procfs input."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ReceiptParsing(unittest.TestCase):
    def test_receipt_parsing_preserves_validation_without_row_allocations(self):
        source = (ROOT / 'userspace/programs/src/wm_suite/measure.rs').read_text()
        receipt = item(source, 'pub struct Receipt {')
        parser = item(source, 'pub fn receipts(').replace(
            'std::fs::read_to_string("/proc/breenix/composites")', 'history_text()')
        fixture = r'''
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static TRACK: AtomicBool = AtomicBool::new(false);
static SMALL: AtomicUsize = AtomicUsize::new(0);
thread_local! { static INPUT: RefCell<Option<String>> = const { RefCell::new(None) }; }
struct Count;
fn count(size: usize) {
    // The clean-room allocator caches blocks up to 2 KiB. Its cache guard
    // queries process identity; W15 removes per-row visits to that path.
    if TRACK.load(Ordering::Relaxed) && size <= 2048 {
        SMALL.fetch_add(1, Ordering::Relaxed);
    }
}
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(layout.size());
        count(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator] static ALLOCATOR: Count = Count;
fn history_text() -> Result<String, std::io::Error> {
    Ok(INPUT.with(|input| input.borrow_mut().take().expect("procfs fixture")))
}
fn input(text: String) { INPUT.with(|input| *input.borrow_mut() = Some(text)); }
RECEIPT
PARSER
fn tuples(rows: Vec<Receipt>) -> Vec<(u64, u64, u64, u64, u64)> {
    rows.into_iter().map(|r| (r.serial, r.ns, r.window, r.commit, r.generation)).collect()
}
#[test]
fn full_history_does_not_allocate_temporary_field_vectors() {
    let mut text = String::new();
    for n in 1..=4096 {
        text.push_str(&format!("{n} 200000000000 {} {n} {n}\n", n % 8 + 1));
    }
    input(text);
    SMALL.store(0, Ordering::Relaxed);
    TRACK.store(true, Ordering::Relaxed);
    let result = receipts();
    TRACK.store(false, Ordering::Relaxed);
    let operations = SMALL.load(Ordering::Relaxed);
    let rows = result.unwrap();
    assert_eq!(rows.len(), 4096);
    for (i, row) in rows.iter().enumerate() {
        let n = i as u64 + 1;
        assert_eq!((row.serial, row.ns, row.window, row.commit, row.generation),
            (n, 200000000000, n % 8 + 1, n, n));
    }
    // Result-vector growth can visit small classes; a temporary per row cannot.
    assert!(operations <= 16, "per-row small allocation/deallocation operations: {operations}");
}
#[test]
fn keeps_five_fields_nonzero_identity_and_numeric_parse_errors() {
    for (text, expected) in [
        ("", vec![]),
        ("1 2 3 4 5", vec![(1, 2, 3, 4, 5)]),
        (" 1\t2 3 4 5 \n2 3 4 5 6\n", vec![(1, 2, 3, 4, 5), (2, 3, 4, 5, 6)]),
        ("18446744073709551615 18446744073709551615 0 0 0",
            vec![(u64::MAX, u64::MAX, 0, 0, 0)]),
    ] {
        input(text.into());
        assert_eq!(tuples(receipts().unwrap()), expected);
    }
    for text in ["\n", "1 2 3 4", "1 2 3 4 5 6", "0 2 3 4 5", "1 0 3 4 5"] {
        input(text.into());
        assert_eq!(receipts().err().unwrap(), "malformed completed-composite receipt");
    }
    for (text, bad) in [
        ("x 2 3 4 5", "x"), ("1 2 3 4 5 x", "x"),
        ("1 2 3 4 5 6 x", "x"), ("-1 2 3 4 5", "-1"),
        ("18446744073709551616 2 3 4 5", "18446744073709551616"),
    ] {
        input(text.into());
        assert_eq!(receipts().err().unwrap(), bad.parse::<u64>().unwrap_err().to_string());
    }
}
'''.replace('RECEIPT', receipt).replace('PARSER', parser)
        with tempfile.TemporaryDirectory(prefix='wm-receipts-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(fixture)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings',
                       str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            subprocess.run(command, check=True)
            subprocess.run([str(path / 'check'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
