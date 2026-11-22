"""Check read-suite fixture geometry and the small-buffer walk's allocation cost."""

from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ReadFixtureTest(unittest.TestCase):
    def test_imported_sparse_file_has_two_mapped_blocks(self):
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / 'read.img'
            manifest = image.with_suffix('.manifest')
            subprocess.run(['bash', str(ROOT / 'scripts/breenfs-read-fixture.sh'),
                            str(image), str(manifest), '8'], check=True)
            row, = [line.split('\t') for line in manifest.read_text().splitlines()
                    if line.startswith(b'/sparse'.hex() + '\t')]
            self.assertEqual((int(row[5]), int(row[6])), (4 * 1024 * 1024 + 23, 16))

    def test_small_getdents_batches_do_not_allocate_per_entry(self):
        source = (ROOT / 'userspace/programs/src/filesystem_read.rs').read_text()
        walk = source[source.index('#[derive(Debug, PartialEq, Eq)]\nstruct Entry'):]
        walk = walk.split('pub fn readdir_resume', 1)[0]
        with tempfile.TemporaryDirectory() as directory:
            program = Path(directory) / 'walk.rs'
            binary = program.with_suffix('')
            program.write_text(WALK_HARNESS + walk)
            subprocess.run(['rustc', '--edition=2021', str(program), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


WALK_HARNESS = r'''
use std::{alloc::{GlobalAlloc, Layout, System}, fs::File, os::fd::AsRawFd};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
struct Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

struct Fd;
impl Fd {
    fn from_raw(_: u64) -> Self { Self }
}

mod sysfs {
    use super::{AtomicUsize, Fd, Relaxed};
    pub(super) static NEXT: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn getdents64(_: Fd, mut bytes: &mut [u8]) -> Result<usize, &'static str> {
        let mut used = 0;
        while NEXT.load(Relaxed) < 5000 && bytes.len() >= 32 {
            let i = NEXT.fetch_add(1, Relaxed);
            let record = &mut bytes[..32];
            record.fill(0);
            record[..8].copy_from_slice(&(i as u64 + 16).to_ne_bytes());
            record[8..16].copy_from_slice(&(i as u64 + 3).to_ne_bytes());
            record[16..18].copy_from_slice(&32u16.to_ne_bytes());
            record[19] = b'n';
            for (column, divisor) in [1000, 100, 10, 1].into_iter().enumerate() {
                record[20 + column] = b'0' + (i / divisor % 10) as u8;
            }
            bytes = &mut bytes[32..];
            used += 32;
        }
        Ok(used)
    }
}

fn main() {
    let file = File::open("/dev/null").unwrap();
    for size in [128, 64] {
        sysfs::NEXT.store(0, Relaxed);
        let before = ALLOCATIONS.load(Relaxed);
        let entries = remainder(&file, size).unwrap();
        let allocated = ALLOCATIONS.load(Relaxed) - before;
        assert_eq!(entries.len(), 5000);
        assert_eq!((entries[4999].ino, entries[4999].cookie), (5015, 5002));
        assert_eq!(entries[4999].name, *b"n4999");
        assert_eq!(allocated, 1, "the {size}-byte walk needs only its result vector");
    }
}
'''


if __name__ == '__main__':
    unittest.main()
