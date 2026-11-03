"""Retired stage accounting preserves totals without retaining a lazy mutex allocation."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class RetiredStorage(unittest.TestCase):
    def test_retired_totals_do_not_allocate_on_first_reset(self):
        source = (ROOT / 'libs/libbreenix/src/suite_stage.rs').read_text()
        declaration = source[source.index('#[derive(Clone, Copy)]\nstruct Retired {'):source.index('/// Horizontal runs')]
        update = '            let mut t = RETIRED.lock().unwrap();' if 'let mut t = RETIRED.lock()' in source else '            use core::sync::atomic::Ordering::Relaxed;'
        record = source[source.index(update):source.index('            if let Some(reset) = reset {')]
        read = '        let t = *RETIRED.lock().unwrap();' if 'let t = *RETIRED.lock()' in source else '        let t = RETIRED.snapshot();'
        snapshot = source[source.index(read):source.index('        if t.unread != 0 {')]
        fixture = r'''
use std::sync::{Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::alloc::{GlobalAlloc, Layout, System};
struct Tracked;
static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) { ALLOCS.fetch_add(1, Ordering::Relaxed); }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
#[global_allocator] static ALLOCATOR: Tracked = Tracked;
DECLARATION
struct G { artifact_bytes: u64, recovery_bytes: u64 }
struct C { uploads_image: u64, uploads_fixture: u64 }
fn record(r: i32, c: C, g: &G) {
RECORD
}
fn snapshot() -> Retired {
SNAPSHOT
    t
}
#[test]
fn first_reset_has_no_retained_allocator_storage() {
    TRACK.store(true, Ordering::Relaxed);
    record(0, C { uploads_image: 3, uploads_fixture: 7 }, &G { artifact_bytes: 11, recovery_bytes: 13 });
    record(-1, C { uploads_image: 100, uploads_fixture: 100 }, &G { artifact_bytes: 17, recovery_bytes: 19 });
    let t = snapshot();
    TRACK.store(false, Ordering::Relaxed);
    assert_eq!((t.image, t.fixture, t.artifacts, t.recovery, t.resets, t.unread), (3, 7, 28, 32, 0, 1));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "retirement statistics retain allocator storage on first reset");
}
'''
        if 'Mutex<' not in declaration:
            fixture = fixture.replace('Mutex, atomic', 'atomic')
        # Baseline uses Mutex; its replacement uses allocation-free atomic totals.
        # Both imports serve the source slices, not platform-specific mocked storage.
        fixture = fixture.replace('DECLARATION', declaration).replace('RECORD', record).replace('SNAPSHOT', snapshot)
        with tempfile.TemporaryDirectory(prefix='stage-retired-') as tmp:
            path = Path(tmp)
            (path / 'case.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'case.rs'), '-o', str(path / 'case')], check=True)
            subprocess.run([str(path / 'case'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
