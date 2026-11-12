"""Ownership evidence precedes observers and preserves empty tables and overflow."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class Ownership(unittest.TestCase):
    def test_sample_captures_before_allocating_reads_and_exports_both_on_failure(self):
        sample = (ROOT / 'libs/libbreenix/src/suite_memory.rs').read_text()
        read = sample.split('fn read()')[1].split('fn release_allocation_cache')[0]
        self.assertIn('Ownership::capture()', read)
        self.assertLess(read.index('Ownership::capture()'), read.index('std::fs::read_to_string'))
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        failure = suite.split('if !reclaimed {')[1].split('reclaimed\n')[0]
        self.assertIn('sample.ownership_detail()', failure)
        self.assertIn('[("before", before.as_ref()), ("after", after.as_ref())]', failure)

    def test_production_walk_retains_empty_tables_ranges_and_overflow_without_allocating(self):
        path = ROOT / 'kernel/src/memory/ownership_snapshot.rs'
        self.assertTrue(path.exists(), 'bounded ownership walker is absent')
        run_rust(self, f'#[path = "{path}"] mod snapshot;\n' + r'''
use snapshot::{Entry, Source, Snapshot, Row, walk};
use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
struct Alloc;
static WATCH:AtomicBool=AtomicBool::new(false);
static ALLOCS:AtomicUsize=AtomicUsize::new(0);
unsafe impl std::alloc::GlobalAlloc for Alloc {
 unsafe fn alloc(&self,l:std::alloc::Layout)->*mut u8 {if WATCH.load(Ordering::Relaxed){ALLOCS.fetch_add(1,Ordering::Relaxed);}std::alloc::System.alloc(l)}
 unsafe fn dealloc(&self,p:*mut u8,l:std::alloc::Layout){std::alloc::System.dealloc(p,l)}
}
#[global_allocator]static A:Alloc=Alloc;
struct Tree;
impl Source for Tree {
 fn entry(&self, table:u64,index:usize,level:u8)->Entry {
  match (table,index,level) {
   (1,0,4)=>Entry::table(2), (2,1,3)=>Entry::table(3),
   (3,2,2)=>Entry::table(4), (3,3,2)=>Entry::table(5),
   (4,7,1)=>Entry::leaf(99,5), _=>Entry::empty()
  }
 }
}
#[test]fn capture_owns_no_observer_allocations_and_reports_incomplete_evidence(){
 let mut s=Snapshot::<16>::new();
 WATCH.store(true,Ordering::Relaxed);
 walk(&Tree,1,17,&mut s);
 WATCH.store(false,Ordering::Relaxed);
 assert_eq!(ALLOCS.load(Ordering::Relaxed),0);
 assert!(!s.overflow);assert_eq!(s.rows().len(),6);
 let leaf=s.rows().iter().find(|r|r.kind=="leaf").unwrap();
 assert_eq!((leaf.pid,leaf.root,leaf.va,leaf.end,leaf.pa,leaf.flags),(17,1,(1<<30)+(2<<21)+(7<<12),(1<<30)+(2<<21)+(8<<12),99,5));
 let empty=s.rows().iter().find(|r|r.kind=="table" && r.pa==5).unwrap();
 assert_eq!((empty.level,empty.present,empty.nonzero,empty.va,empty.end),(1,0,0,(1<<30)+(3<<21),(1<<30)+(4<<21)));
 let mut small=Snapshot::<2>::new();walk(&Tree,1,17,&mut small);
 assert_eq!(small.rows().len(),2);assert!(small.overflow);
 small.clear();assert!(small.rows().is_empty());assert!(!small.overflow);
 small.push(Row{kind:"process",pid:18,..Row::EMPTY});assert_eq!(small.rows()[0].pid,18);
}
''')


if __name__ == '__main__':
    unittest.main()
