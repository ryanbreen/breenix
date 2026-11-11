"""Exercise production node retirement with owned directory-scan allocations."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]

class ScanCleanup(unittest.TestCase):
    def test_removing_a_directory_releases_its_scan_buffers_and_handle(self):
        source = (ROOT / 'libs/smbfs/src/lib.rs').read_text()
        forget = item(source, 'fn forget(')
        within = item(source, 'fn within(')
        run_rust(self, r'''
use std::collections::BTreeMap;
struct Node { path:String }
struct Handle { path:Option<String> }
struct Scan { id:Option<u64>, _entries:Vec<String> }
#[derive(Default)] struct Backend { closed:Vec<u64> }
impl Backend { fn close(&mut self,id:u64)->Result<(),u32> { self.closed.push(id); Ok(()) } }
#[derive(Default)] struct Namespace {
 backend:Backend, handles:BTreeMap<u64,Handle>, nodes:BTreeMap<u64,Node>,
 paths:BTreeMap<String,u64>, cache:BTreeMap<u64,()>, scans:BTreeMap<u64,Scan>
}
WITHIN
impl Namespace { FORGET }
#[test] fn retired_directory_owns_no_scan() {
 for remote in [None,Some(42)] {
  let mut fs=Namespace::default();
  fs.nodes.insert(1,Node{path:"drawing".into()}); fs.paths.insert("drawing".into(),1);
  fs.scans.insert(1,Scan{id:remote,_entries:vec![".drawing.png.atomic-temp".repeat(1024)]});
  fs.scans.insert(2,Scan{id:None,_entries:vec!["unrelated".into()]});
  assert_eq!(fs.scans[&1].id,remote);
  fs.backend.close(99).unwrap(); fs.backend.closed.clear();
  fs.forget("drawing");
  assert!(!fs.scans.contains_key(&1),"removed directory still retains scan allocations");
  assert!(fs.scans.contains_key(&2));
  assert_eq!(fs.backend.closed,remote.into_iter().collect::<Vec<_>>());
 }
}
'''.replace('WITHIN', within).replace('FORGET', forget))

if __name__ == '__main__': unittest.main()
