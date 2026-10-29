"""Execute the shared display-hold retirement after a native viewing copy."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class PresentRetirement(unittest.TestCase):
    def test_return_to_console_releases_copy_sources_but_preserves_pending_count(self):
        source=(ROOT/'kernel/src/gpu/display.rs').read_text()
        method=item(source,'    fn native_console_restored(')
        restore=item(source,'pub(super) fn restore()')
        self.assertIn('native_console_restored()',restore)
        harness=r'''
use std::collections::BTreeSet;
struct Display {holds:BTreeSet<u32>,binding:Option<u32>,outstanding:u32,known:bool,latest_ok:bool}
struct Core {display:Display,released:Vec<u32>}
impl Core {
 fn unhold(&mut self,id:u32){self.released.push(id);}
 // METHOD
}
fn main(){
 for outstanding in [0,2] {
  let mut c=Core{display:Display{holds:[7,9].into_iter().collect(),binding:Some(9),outstanding,known:false,latest_ok:false},released:vec![]};
  c.native_console_restored();
  assert_eq!(c.released,[7,9]);assert!(c.display.holds.is_empty());assert_eq!(c.display.binding,None);
  assert_eq!(c.display.outstanding,outstanding);assert_eq!(c.display.known,outstanding==0);assert!(c.display.latest_ok);
 }
}
'''.replace('// METHOD',method)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'retire.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'retire.rs'),'-o',str(p/'retire')],check=True)
            subprocess.run([str(p/'retire')],check=True)
if __name__=='__main__':unittest.main()
