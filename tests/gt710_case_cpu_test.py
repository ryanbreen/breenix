"""Keep native case CPU diagnostics in measured units, independent of verdicts."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class CaseCpu(unittest.TestCase):
 def test_timeval_units_and_case_deltas(self):
  path=ROOT/'userspace/programs/src/gt710_cpu.rs'
  self.assertTrue(path.exists(),'sample actual caller and reaped-child CPU time')
  source=path.read_text()
  code='use std::time::Duration;\n'+item(source,'fn split(')+'\n'+item(source,'fn report(')+r'''
fn main(){
 let mut raw=[0i64;18];raw[..4].copy_from_slice(&[2,3,4,5]);assert_eq!(split(&raw),Some([2000003,4000005]));
 raw[1]=1000000;assert_eq!(split(&raw),None);raw[1]=-1;assert_eq!(split(&raw),None);
 raw[1]=0;raw[0]=-1;assert_eq!(split(&raw),None);raw[0]=i64::MAX;assert_eq!(split(&raw),None);
 assert_eq!(report("text/size-range",[10,20,30,40],[11,22,33,44],Duration::from_secs(2)).unwrap(),
 "GT710_CASE_CPU case=text/size-range sample_ms=2000 self_user_us=1 self_system_us=2 children_user_us=3 children_system_us=4");
 assert!(report("x",[10,20,30,40],[9,22,33,44],Duration::from_secs(2)).is_none());
 assert!(report("x",[0;4],[1;4],Duration::from_millis(999)).is_none());
 assert!(report("",[0;4],[1;4],Duration::from_secs(2)).is_none());
}
'''
  priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp);(p/'cpu.rs').write_text(code)
   subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'cpu.rs'),'-o',str(p/'cpu')],check=True)
   subprocess.run([str(p/'cpu')],check=True)
if __name__=='__main__':unittest.main()
