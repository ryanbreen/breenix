"""Native address packets must not allocate temporary payload vectors."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class AddressPacket(unittest.TestCase):
    def test_exact_packet_and_relocation_without_temporary_allocation(self):
        source = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text()
        code = r'''use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Count;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Count {
 unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
  ALLOCS.fetch_add(1, Ordering::Relaxed);
  unsafe { System.alloc(layout) }
 }
 unsafe fn dealloc(&self, p: *mut u8, layout: Layout) { unsafe { System.dealloc(p, layout) } }
}
#[global_allocator] static HEAP: Count = Count;
const NO_MEMORY: &str = "allocation refused";
struct Chunk { words: Vec<u32>, relocations: Vec<usize> }
impl Chunk { EMIT ADDRESS }
fn main() {
 let mut c = Chunk { words: Vec::with_capacity(128), relocations: Vec::with_capacity(16) };
 c.emit(0x100, &[7]).unwrap();
 let before = ALLOCS.load(Ordering::Relaxed);
 c.address(0x2380, 0x123456789abc, &[256, 17]).unwrap();
 c.address(0x1608, 0x987654321000, &[]).unwrap();
 let allocated = ALLOCS.load(Ordering::Relaxed) - before;
 assert_eq!(c.words, [0x20010040, 7, 0x200408e0, 0x1234, 0x56789abc, 256, 17,
                     0x20020582, 0x9876, 0x54321000]);
 assert_eq!(c.relocations, [3, 8]);
 assert_eq!(allocated, 0, "address emission allocates temporary payloads despite reserved output");
 // Relocation indices must continue to name the high/low address words.
 for &i in &c.relocations { let a = (u64::from(c.words[i]) << 32) | u64::from(c.words[i+1]);
  let relocated = a + 0x20000; c.words[i] = (relocated >> 32) as u32; c.words[i+1] = relocated as u32; }
 assert_eq!(&c.words[3..5], &[0x1234, 0x567a9abc]);
 assert_eq!(&c.words[8..10], &[0x9876, 0x54341000]);
}
'''.replace('EMIT', item(source, '    fn emit(')).replace('ADDRESS', item(source, '    fn address('))
        with tempfile.TemporaryDirectory(prefix='gt710-address-') as tmp:
            p = Path(tmp)
            (p / 'check.rs').write_text(code)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)

if __name__ == '__main__':
    unittest.main()
