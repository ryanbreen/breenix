"""Retained native frames may differ from the owner's subsequently selected mode."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class Extent(unittest.TestCase):
    def test_retained_frame_extent_is_validated_against_its_resource(self):
        code=item((ROOT/'kernel/src/gpu/gk208/ce/readback.rs').read_text(),'fn scanout(')
        fixture=r'''
#[derive(Clone,Copy,Debug,PartialEq)]struct Surface{width:u32,height:u32}
struct Resources;
impl Resources{fn scanout(&self,id:u32)->Option<Surface>{(id==1).then_some(Surface{width:2560,height:1600})}}
fn with_resources<R>(f:impl FnOnce(&mut Resources,&mut (),&mut ())->Result<R,&'static str>)->Result<R,&'static str>{f(&mut Resources,&mut (),&mut ())}
mod present{pub type Surface=crate::Surface;}
mod process{pub fn mode_extent()->Option<(u32,u32)>{Some((1280,800))}}
mod ce{pub mod readback{CODE}}
#[test]fn saved_high_frame_is_not_rejected_by_later_low_mode(){
 let _=process::mode_extent();
 assert_eq!(ce::readback::scanout(1,2560,1600),Some(Surface{width:2560,height:1600}));
 assert_eq!(ce::readback::scanout(1,1280,800),None);
 assert_eq!(ce::readback::scanout(2,2560,1600),None);
}
'''.replace('CODE',code.replace('fn scanout(', 'pub(crate) fn scanout(').replace('super::super::present','crate::present').replace('super::process','crate::process').replace('super::with_resources','crate::with_resources'))
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'extent.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'extent.rs'),'-o',str(p/'extent')],check=True)
            subprocess.run([str(p/'extent'),'--nocapture'],check=True)
if __name__=='__main__':unittest.main()
