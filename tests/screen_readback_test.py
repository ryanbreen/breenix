"""Exercise checkpoint readback policy using the production GPU observer and ABI."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function
ROOT = Path(__file__).resolve().parents[1]
class Readback(unittest.TestCase):
    def test_provenance_limit_and_visible_rectangle(self):
        source=(ROOT/'kernel/src/gpu/mod.rs').read_text()
        policy=function(source,'impl Observer')
        abi=ROOT/'libs/gpu-abi/src/screen.rs'
        fixture = r'''use std::convert::TryInto;
mod screen { ABI }
#[derive(Clone,Copy,PartialEq,Eq)] enum Observer {Owner,Window,Screen(u64)}
POLICY
#[test] fn provenance(){
 for observer in [Observer::Owner,Observer::Window] {
   assert!(observer.records_composition());
 }
 assert!(!Observer::Screen(77).records_composition());
}
#[test] fn ownership(){
 for observer in [Observer::Owner,Observer::Window] {
  assert!(!observer.excludes_owner(None));
  assert!(!observer.excludes_owner(Some(77)));
 }
 let screen=Observer::Screen(77);
 assert!(screen.excludes_owner(None));
 assert!(screen.excludes_owner(Some(77)),"runner ownership after reset is not a case");
 assert!(!screen.excludes_owner(Some(88)),"an active case must still be observed");
}
#[test] fn rectangle(){
 assert_eq!(screen::rectangle([2,1],[4,2],Some([0,0])),Some([0,0,2,1]));
 assert_eq!(screen::rectangle([2,1],[4,3],Some([2,2])),Some([2,2,2,1]));
 for (mode,binding,offset) in [([2,1],[1,1],Some([0,0])),([2,1],[4,2],None),([2,1],[4,2],Some([3,0])),([0,1],[4,2],Some([0,0]))] {
   assert_eq!(screen::rectangle(mode,binding,offset),None);
 }
}
#[test] fn request(){
 assert_eq!(screen::NO_CASE,61,"suite endpoint no-case errno");
 let bytes=screen::Request{pixels:1,len:2,record:3,runner:77}.encode();
 let r=screen::Request::decode(&bytes).unwrap();
 assert_eq!((r.pixels,r.len,r.record,r.runner),(1,2,3,77));
 assert!(screen::Request::decode(&bytes[..39]).is_none());
 let mut bad=bytes;bad[0]^=1;assert!(screen::Request::decode(&bad).is_none());
}
'''
        fixture=fixture.replace('use std::convert::TryInto;','').replace('ABI',abi.read_text()).replace('POLICY',policy)
        with tempfile.TemporaryDirectory() as d:
            p=Path(d);(p/'test.rs').write_text(fixture)
            prefix=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(prefix+['rustc','--edition=2021','--test','-Dwarnings',str(p/'test.rs'),'-o',str(p/'test')],check=True)
            subprocess.run([str(p/'test')],check=True)
if __name__=='__main__': unittest.main()
