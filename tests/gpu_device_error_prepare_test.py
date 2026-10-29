"""A refused native control must not count as an executed device error."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item
class DeviceErrorPreparation(unittest.TestCase):
    def test_refusal_rolls_back_the_point_and_only_queueing_counts(self):
        helper = item((ROOT / 'kernel/src/gpu/mod.rs').read_text(), 'fn prepare_device_error(')
        source = r'''
extern crate self as spin;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {pub const fn new(t:T)->Self {Self(std::sync::Mutex::new(t))}pub fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
#[path="POINT"] mod point;
mod test_control {
 pub use crate::point::Reservation;
 pub static POINT:crate::point::Point=crate::point::Point::new();
 pub fn reserve(p:u32,t:u64)->Option<Reservation<'static>>{assert_eq!(p,27);POINT.reserve(t,|_|true)}
}
mod test_point {pub const DEVICE_ERROR:u32=27;}
#[derive(Clone,Copy)] struct Tag(u64);
#[derive(Debug,PartialEq)] struct Request(u64,u64,u32);
const ENOSYS:i64=38;
mod dev {
 use super::{Request,Tag};
 pub static READY:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
 pub fn undefined_command(tag:Tag,ctx:u32,fence:u64)->Option<Request>{READY.load(std::sync::atomic::Ordering::Relaxed).then_some(Request(tag.0,fence,ctx))}
}
// HELPER
fn main(){
 use std::sync::atomic::Ordering;
 test_control::POINT.arm(0,0);
 assert!(matches!(prepare_device_error(false,Tag(7),19,23),Err(ENOSYS)));
 assert_eq!(test_control::POINT.fired(),0);
 dev::READY.store(true,Ordering::Relaxed);
 let (request,point)=prepare_device_error(false,Tag(7),19,23).unwrap();
 assert_eq!(request,Some(Request(7,23,19)));assert_eq!(test_control::POINT.fired(),0);
 drop(point); // another failure before queueing must roll back too
 let (request,point)=prepare_device_error(false,Tag(7),19,23).unwrap();
 assert_eq!(request,Some(Request(7,23,19)));point.unwrap().fire();
 assert_eq!(test_control::POINT.fired(),1);
 let (request,point)=prepare_device_error(false,Tag(7),19,23).unwrap();
 assert!(request.is_none()&&point.is_none());
 let (request,point)=prepare_device_error(true,Tag(7),19,23).unwrap();
 assert!(request.is_some()&&point.is_none());
 test_control::POINT.disarm();
}
'''.replace('"POINT"', '"' + str(ROOT / 'kernel/src/test_control/point.rs') + '"').replace('// HELPER', helper)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'prepare.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'prepare.rs'),'-o',str(p/'prepare')],check=True)
            subprocess.run([str(p/'prepare')],check=True)
if __name__=='__main__':unittest.main()
