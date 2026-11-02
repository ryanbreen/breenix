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
#[derive(Clone,Copy,PartialEq,Eq)] enum Observer {Owner,Window,Screen}
POLICY
#[test] fn provenance(){
 for observer in [Observer::Owner,Observer::Window] {
   assert!(observer.records_composition());
 }
 assert!(!Observer::Screen.records_composition());
}
#[test] fn deadline(){
 assert_eq!(Observer::Screen.deadline(100),Some(5_000_000_100));
 assert_eq!(Observer::Owner.deadline(100),None);
 assert_eq!(Observer::Window.deadline(100),None);
}
#[test] fn empty_binding(){
 for observer in [Observer::Owner,Observer::Window] {
  for known in [false,true] { for bound in [false,true] {
   assert!(!observer.excludes_binding(known,bound));
  }}
 }
 assert!(Observer::Screen.excludes_binding(true,false),"confirmed reset removed scanout");
 assert!(!Observer::Screen.excludes_binding(false,false),"pending PRESENT must settle");
 assert!(!Observer::Screen.excludes_binding(false,true));
 assert!(!Observer::Screen.excludes_binding(true,true));
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
 let bytes=screen::Request{pixels:1,len:2,record:3}.encode();
 let r=screen::Request::decode(&bytes).unwrap();
 assert_eq!((r.pixels,r.len,r.record),(1,2,3));
 assert!(screen::Request::decode(&bytes[..31]).is_none());
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
