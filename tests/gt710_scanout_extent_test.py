"""Retained native frames may differ from the owner's subsequently selected mode."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class Extent(unittest.TestCase):
    def test_retained_frame_extent_is_validated_against_its_resource(self):
        code=item((ROOT/'kernel/src/gpu/gk208/ce/readback.rs').read_text(),'fn scanout(')
        validate=item((ROOT/'kernel/src/gpu/gk208/ce/process.rs').read_text(),'pub(super) fn presentable(')
        validate=validate.replace('pub(super)','pub').replace('super::super::modeset','crate::modeset')
        fixture=r'''
#![allow(dead_code)] // Real modeset includes APIs outside this admission fixture.
extern crate alloc;
#[path="@ROOT@/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="@ROOT@/kernel/src/gpu/gk208/modeset.rs"] mod modeset;
#[derive(Clone,Copy,Debug,PartialEq)]struct Surface{width:u32,height:u32}
struct Resources;
impl Resources{fn scanout(&self,id:u32)->Option<Surface>{match id {1=>Some(Surface{width:2560,height:1600}),3=>Some(Surface{width:2560,height:800}),4=>Some(Surface{width:1024,height:768}),_=>None}}}
fn with_resources<R>(f:impl FnOnce(&mut Resources,&mut (),&mut ())->Result<R,&'static str>)->Result<R,&'static str>{f(&mut Resources,&mut (),&mut ())}
mod present{pub type Surface=crate::Surface;}
mod process {
 struct Session {registers:usize,evo:Evo}struct Evo{head:u32}
 struct Registers(usize);impl Registers {fn read(&self,r:usize)->u32{match r {0x64041c=>0,0x640420=>1280|1024<<16,_=>panic!("unexpected raster read")}}}
 struct Lock(std::sync::Mutex<Option<Session>>);impl Lock {fn lock(&self)->std::sync::MutexGuard<'_,Option<Session>>{self.0.lock().unwrap()}}
 static SESSION:Lock=Lock(std::sync::Mutex::new(Some(Session{registers:0,evo:Evo{head:0}})));
 VALIDATE
}
mod ce{pub mod readback{CODE}}
#[test]fn saved_high_frame_is_not_rejected_by_later_low_mode(){

 assert_eq!(ce::readback::scanout(1,2560,1600),Some(Surface{width:2560,height:1600}));
 assert_eq!(ce::readback::scanout(1,1280,800),None);
 assert_eq!(ce::readback::scanout(2,2560,1600),None);
 assert_eq!(ce::readback::scanout(4,1024,768),Some(Surface{width:1024,height:768}));
 assert_eq!(ce::readback::scanout(3,2560,800),None,"unsupported viewport reached the adapter");
}
'''.replace('@ROOT@',str(ROOT)).replace('VALIDATE',validate).replace('CODE',code.replace('fn scanout(', 'pub(crate) fn scanout(').replace('super::super::present','crate::present').replace('super::process','crate::process').replace('super::with_resources','crate::with_resources'))
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'extent.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'extent.rs'),'-o',str(p/'extent')],check=True)
            subprocess.run([str(p/'extent'),'--nocapture'],check=True)
if __name__=='__main__':unittest.main()
