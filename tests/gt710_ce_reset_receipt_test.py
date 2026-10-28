"""The shared CE reset helper must reject a failed pulse or a surviving fault."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item

class CeResetReceipt(unittest.TestCase):
    def test_reset_requires_disable_enable_and_no_engine_fault(self):
        source = (ROOT / 'kernel/src/gpu/gk208/ce/recovery.rs').read_text()
        method = item(source, 'pub(in crate::gk208) fn reset_engine(').replace('pub(in crate::gk208)', '')
        harness = r'''
struct Registers {pmc:u32,fail_disable:bool,fail_enable:bool,fault:u32,delay:u64,clears:usize}
impl Registers {
 fn mask(&mut self,a:usize,m:u32,v:u32){assert_eq!(a,0x200);if (v==0&&self.fail_disable)||(v!=0&&self.fail_enable){return;}self.pmc=(self.pmc&!m)|v;}
 fn read(&mut self,a:usize)->u32 {match a {0x200=>self.pmc,0x106908=>self.fault,_=>panic!("unexpected register")}}
 fn write(&mut self,a:usize,v:u32){match a {0x106908=>{assert_eq!(v,7);self.clears+=1;},0x106f14=>assert_eq!(v,0),_=>panic!("unexpected register")}}
 fn delay_us(&mut self,n:u64){self.delay+=n;}
}
METHOD
#[test]fn pulse_and_fault_receipts(){
 for case in 0..4 {
  let mut r=Registers{pmc:(1<<21)|0x1000,fail_disable:case==1,fail_enable:case==2,fault:if case==3 {1}else{0},delay:0,clears:0};
  let result=reset_engine(&mut r,0x106000,21);
  assert_eq!(result.is_ok(),case==0,"accepted reset failure {case}");
  assert_eq!(r.pmc&!(1<<21),0x1000,"modified another engine");assert_eq!(r.delay,50);
  if case==0 {assert_eq!(result.unwrap(),(0x1000,(1<<21)|0x1000));}
  assert_eq!(r.clears,usize::from(case==0||case==3),"cleared a fault without a confirmed reset");
 }
}
'''.replace('METHOD', method)
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            (out / 'reset.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(out / 'reset.rs'), '-o', str(out / 'reset')], check=True)
            subprocess.run([str(out / 'reset')], check=True)

if __name__ == '__main__':
    unittest.main()
