"""Execute upload phase boundaries with a deterministic allocation/copy/wait clock."""
from pathlib import Path
import unittest

import gt710_ce_transfers_test

ROOT = Path(__file__).resolve().parents[1]


class UploadCost(unittest.TestCase):
    def test_allocation_and_acquisition_are_separate_from_user_copy(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text().split('fn op_upload(', 1)[1]
        phases = source[source.index('    // Stage the clipped source rows'):
                        source.index('    let release =')]
        # Execute the x86 instrumentation even when the host is ARM64.
        phases = phases.replace('#[cfg(target_arch = "x86_64")]', '')
        gt710_ce_transfers_test.Transfers().run_rust(r'''
use std::{cell::Cell,collections::BTreeMap};
const EFAULT:i64=14;
const EINVAL:i64=22;
const EINTR:i64=4;
thread_local! {
    static NOW:Cell<u64>=const{Cell::new(0)};
    static COST:std::cell::RefCell<BTreeMap<&'static str,(u64,u64)>>=const{std::cell::RefCell::new(BTreeMap::new())};
}
fn tick(n:u64){NOW.with(|v|v.set(v.get()+n));}
fn now()->u64{NOW.with(Cell::get)}
mod gk208 {pub mod cost {
    pub enum Slot {UserCopy,UploadStage,UploadAcquire}
    pub struct Guard(&'static str,u64);
    pub fn phase(slot:Slot)->Guard {
        Guard(match slot {Slot::UserCopy=>"user_copy",Slot::UploadStage=>"upload_stage",
            Slot::UploadAcquire=>"upload_acquire"},crate::now())
    }
    impl Drop for Guard {fn drop(&mut self){crate::COST.with(|m|{
        let mut m=m.borrow_mut();let v=m.entry(self.0).or_default();
        v.0+=1;v.1+=crate::now()-self.1;
    });}}
}}
struct Pages;
impl Pages {
    fn try_spans<E>(&self,_:usize,n:usize,mut f:impl FnMut(usize,*mut u8,usize)->Result<(),E>)->Result<(),E>{
        for done in (0..n).step_by(4096){f(done,std::ptr::null_mut(),4096.min(n-done))?;}Ok(())
    }
}
struct Staging{pages:Pages}
impl Staging{fn new(_:u64,bytes:usize)->Result<Self,i64>{assert_eq!(bytes,16<<20);tick(17);Ok(Self{pages:Pages})}}
mod syscall {pub mod userptr {
    pub fn read_user_bytes(_:*mut u8,_:u64,n:usize)->Result<(),()>{assert_eq!(n,4096);crate::tick(5);Ok(())}
}}
struct Args{resource:u64,pixels:u64}
struct Resource{reading:bool,writers:u32}
struct Sub{ctx:u32,held:Vec<u32>}
struct Core{resources:BTreeMap<u32,Resource>,subs:BTreeMap<u32,Sub>}
impl Core{fn res_of(&self,_:u64,handle:u64)->Option<u32>{(handle==2).then_some(2)}}
fn wait_for<T>(_:Option<u64>,interruptible:bool,f:impl FnOnce(&mut Core)->Option<T>)->Result<T,()>{
    assert!(interruptible);tick(31);
    let mut c=Core{resources:BTreeMap::from([(2,Resource{reading:false,writers:0})]),subs:BTreeMap::new()};
    let out=f(&mut c).unwrap();assert!(c.resources[&2].reading);Ok(out)
}
fn upload()->Result<(),i64>{
    let (tgid,ctx,id)=(1,1,2);
    let (row,ch,stride)=(8192u64,2048u64,8192u64);
    let a=Args{resource:2,pixels:0};
PHASES
    Ok(())
}
#[test] fn sixteen_mib_phase_boundaries(){
    // Reference the fixture's complete slot set before evaluating production scopes.
    let _=(gk208::cost::Slot::UploadStage,gk208::cost::Slot::UploadAcquire);
    upload().unwrap();
    COST.with(|m|{
        let m=m.borrow();
        assert_eq!(m.get("upload_stage"),Some(&(1,17)),"staging allocation needs its own slot");
        assert_eq!(m.get("user_copy"),Some(&(1,17+4096*5)),"user_copy remains inclusive of allocation");
        assert_eq!(m.get("upload_acquire"),Some(&(1,31)),"resource wait needs its own slot");
    });
}
'''.replace('PHASES', phases))


if __name__ == '__main__':
    unittest.main()
