"""Display-mechanics cases select their geometry from MODES, including virtio's pair."""
import pathlib
import re
import subprocess
import tempfile
import unittest
ROOT=pathlib.Path(__file__).resolve().parents[1]
def body(source,name):
    match=re.search(r'fn '+name+r'\b[^\{]*\{',source)
    assert match, name
    pos=match.end(); start=pos; depth=1
    while depth:
        depth+=(source[pos]=='{')-(source[pos]=='}'); pos+=1
    return source[start:pos-1]
class CaseModes(unittest.TestCase):
    def test_cases_and_virtual_values(self):
        cases={
          'userspace/programs/src/suite_gpu/present.rs': {
            'flip':'native_mode', 'mode_change_content':'smallest_mode',
            'fault_skip_flush':'current_mode','fault_wrong_scanout':'current_mode'},
          'userspace/programs/src/suite_breengel/show.rs': {
            'composition_fbinfo_geometry':'scale2_mode','scaled_stage':'scale2_mode'},
          'userspace/programs/src/suite_breengel/window_cases/composition.rs': {
            'screen_size':'current_mode','composition_scale_2':'scale2_mode'},
          'userspace/programs/src/suite_breengel/window_cases/scanout.rs': {
            'scanout_mode_set':'scale2_mode','scanout_hidpi_mode':'scale2_mode'}}
        for path,names in cases.items():
            source=(ROOT/path).read_text()
            for name,selector in names.items():
                with self.subTest(case=name):
                    code=body(source,name)
                    self.assertIn(selector,code)
                    self.assertNotRegex(code,r'\b(?:1280|800|2560|1600|1599)\b')
        code=r'''extern crate gpu_abi;
use gpu_abi::{Mode,Modes,Plain};
fn main(){
 let mut m=Modes::ZERO;
 m.count=2;m.current=1;
 m.modes=[Mode{width:1280,height:800,scale:1,reserved:0},Mode{width:2560,height:1600,scale:2,reserved:0}];
 assert_eq!(m.native_mode().unwrap(),(0,m.modes[0]));
 assert_eq!(m.smallest_mode().unwrap(),(0,m.modes[0]));
 assert_eq!(m.scale2_mode().unwrap(),(1,m.modes[1]));
 assert_eq!(m.current_mode().unwrap(),m.modes[1]);
 assert_eq!(core::mem::size_of::<Modes>(),256);
 assert_eq!(core::mem::offset_of!(Modes,scanout_resource),64);
 m.count=3;m.current=2;m.modes=[Mode{width:1024,height:768,scale:1,reserved:0},Mode{width:1280,height:1024,scale:1,reserved:0}];
 m.extra_modes[0]=Mode{width:2560,height:2048,scale:2,reserved:0};
 assert_eq!(m.native_mode().unwrap(),(1,m.modes[1]));
 assert_eq!(m.scale2_mode().unwrap(),(2,m.extra_modes[0]));
 assert_eq!(m.smallest_mode().unwrap(),(0,m.modes[0]));
 assert_eq!(m.current_mode().unwrap(),m.extra_modes[0]);
}'''
        with tempfile.TemporaryDirectory() as tmp:
            tmp=pathlib.Path(tmp); src=tmp/'check.rs'; src.write_text(code)
            lib=tmp/'libgpu_abi.rlib'
            subprocess.run(['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(lib)],check=True)
            subprocess.run(['rustc','--edition=2021',str(src),'--extern',f'gpu_abi={lib}','-o',str(tmp/'check')],check=True)
            subprocess.run([str(tmp/'check')],check=True)
if __name__=='__main__': unittest.main()
