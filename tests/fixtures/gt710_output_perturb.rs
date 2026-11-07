extern crate alloc;
extern crate self as gpu_abi;
extern crate self as spin;
extern crate self as breengel_abi;
TABLES
#[path="ABI/screen_crc.rs"] pub mod screen_crc;
#[allow(dead_code)] #[path="DRIVER/edid.rs"] mod edid;
#[allow(dead_code)] #[path="DRIVER/modeset.rs"] mod modeset;
#[path="DRIVER/display_crc.rs"] pub mod display_crc;
// Only the host lock primitive replaces the kernel's spin implementation.
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(t:T)->Self { Self(std::sync::Mutex::new(t)) }
    pub fn lock(&self)->std::sync::MutexGuard<'_,T> { self.0.lock().unwrap() }
}
#[derive(Clone,Copy)] pub struct TcArm { point:u32,reserved:u32,target:u64,param:u64 }
#[derive(Clone,Copy)] pub struct TcFired { point:u32,reserved:u32,count:u64 }
pub trait Plain:Copy {}
impl Plain for TcArm {} impl Plain for TcFired {}
mod syscall {
    #[derive(Debug,PartialEq)] pub enum SyscallResult { Ok(u64),Err(u64) }
    pub mod errno { pub const EINVAL:i32=22; pub const EFAULT:i32=14; }
    pub mod userptr {
        pub fn copy_to_user<T:Copy>(p:*mut T,v:&T)->Result<(),()> { unsafe{*p=*v;} Ok(()) }
    }
}
fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T { f() }
mod tracing { pub mod providers {
    pub mod gpu { pub fn reset_irq_masked(){} }
    pub mod window { pub fn reset_irq_masked(){} }
} }
mod gpu { pub mod dev {
    use std::sync::atomic::{AtomicU32,Ordering};
    static BACKEND:AtomicU32=AtomicU32::new(6);
    pub fn backend()->u32 { BACKEND.load(Ordering::Relaxed) }
    pub fn select(b:u32) { BACKEND.store(b,Ordering::Relaxed); }
    SUPPORTED
    pub fn restore_output_perturbation()->Result<(),i32> {
        crate::OUTPUT.lock().restore(&mut *crate::MODEL.lock()).map_err(|_|5)
    }
} }
mod test_control {
    use super::{tc_point,test_point,Plain,TcArm,TcFired,syscall::SyscallResult};
    use super::syscall::errno::{EINVAL,EFAULT};
    use crate::test_control as tc_op;
    pub const STATE:u32=0; pub const ARM:u32=1; pub const DISARM_ALL:u32=2; pub const FIRED:u32=3;
    #[path="POINT"] mod point;
    use point::Point;
    const POINTS:usize=tc_point::LAST as usize+1;
    static POINT_TABLE:[Point;POINTS]=[const {Point::new()};POINTS];
    fn enabled()->bool { true }
    fn caller_is_root()->bool { true }
    fn err(e:i32)->SyscallResult { SyscallResult::Err(e as u64) }
    fn read_arg<T:Plain>(a:u64,n:u64)->Result<T,i32> {
        if n!=std::mem::size_of::<T>() as u64 {return Err(EINVAL);}
        Ok(unsafe{*(a as *const T)})
    }
    pub fn reserve(p:u32,t:u64)->Option<point::Reservation<'static>> {
        POINT_TABLE.get(p as usize)?.reserve(t, |_|true)
    }
    DEFINED
    DISPATCH
}
CHECKPOINT
use display_crc::{Binding,Io,Sample};
use std::collections::BTreeMap;
static OUTPUT:Mutex<display_crc::Perturbation>=Mutex::new(display_crc::Perturbation::new());
static MODEL:Mutex<Model>=Mutex::new(Model {methods:BTreeMap::new(),writes:Vec::new(),fail:false});
struct Model { methods:BTreeMap<u32,u32>,writes:Vec<Vec<(u32,u32)>>,fail:bool }
impl modeset::Io for Model {
    fn read(&mut self,r:usize)->u32 {self.methods[&((r&0xffff) as u32)]}
    fn write(&mut self,_:usize,_:u32){panic!("raw MMIO output write");}
    fn delay_us(&mut self,_:u32){}
}
impl Io for Model {
    fn now_us(&self)->u64 {0}
    fn binding(&mut self)->Result<Binding,&'static str>{Ok(Binding{resource:7,present_fence:9})}
    fn clear(&mut self)->Result<(),&'static str>{Ok(())}
    fn update(&mut self,p:&[(u32,u32)],_:u64)->Result<(),&'static str>{
        self.writes.push(p.to_vec());
        for &(m,v) in p {if m>=0x200 {self.methods.insert(m,v);}}
        if self.fail {self.fail=false;Err("partial UPDATE failure")}else{Ok(())}
    }
    fn context_status(&mut self)->Result<u32,&'static str>{Ok(1)}
    fn sample(&mut self,_:usize)->Result<Option<Sample>,&'static str>{Ok(None)}
    fn display_error(&mut self)->u32{0}
    fn recover_present(&mut self)->Result<(),&'static str>{Ok(())}
}
fn arm(target:u64)->syscall::SyscallResult {
    let a=TcArm{point:tc_point::DISPLAY_OUTPUT_PERTURB,reserved:0,target,param:0};
    test_control::sys_test_control(1,&a as *const _ as u64,24)
}
fn fired()->u64 {
    let mut f=TcFired{point:tc_point::DISPLAY_OUTPUT_PERTURB,reserved:0,count:0};
    assert_eq!(test_control::sys_test_control(3,&mut f as *mut _ as u64,16),syscall::SyscallResult::Ok(0));f.count
}
fn disarm()->syscall::SyscallResult {test_control::sys_test_control(2,0,0)}
fn checkpoint()->Result<(),&'static str> {
    perturb_checkpoint(&mut *MODEL.lock(),&mut *OUTPUT.lock(),Binding{resource:7,present_fence:9},0,0)
}
fn main() {
    assert_eq!(tc_point::DISPLAY_OUTPUT_PERTURB,35);
    assert_eq!(tc_point::RELEASE_STALL,11);assert_eq!(tc_point::CAPTURE_STALL,18);
    assert_eq!(test_point::CRED_ORPHAN,32);
    for b in [4,5,6] {
        gpu::dev::select(b);
        let expected=if b==6 && cfg!(target_arch = "x86_64") {syscall::SyscallResult::Ok(0)}
                     else {syscall::SyscallResult::Err(22)};
        assert_eq!(arm(0),expected);
        assert_eq!(disarm(),syscall::SyscallResult::Ok(0));
    }
    if !cfg!(target_arch = "x86_64") {return;}
    let t=edid::Timing {width:1280,height:1024,clock_khz:108000,hsync_start:1328,
        hsync_end:1440,htotal:1688,vsync_start:1025,vsync_end:1028,vtotal:1066,flags:30};
    MODEL.lock().methods=modeset::logical_head_methods(&t,0,0,1,0x08000000,10240,2560,1600)
        .unwrap().into_iter().collect();
    for old in [0,0x82,0x84,0x101] {
        MODEL.lock().methods.insert(0x4a0,old);
        let before=modeset::crc_output_methods(&mut *MODEL.lock(),0,0).unwrap();
        let count=fired();let writes=MODEL.lock().writes.len();
        assert_eq!(arm(99),syscall::SyscallResult::Ok(0));
        checkpoint().unwrap();assert_eq!(fired(),count); // target mismatch
        assert_eq!(disarm(),syscall::SyscallResult::Ok(0));
        assert_eq!(arm(7),syscall::SyscallResult::Ok(0));
        assert_eq!(fired(),count);assert_eq!(MODEL.lock().writes.len(),writes);
        checkpoint().unwrap();assert_eq!(fired(),count+1);
        let changed=modeset::crc_output_methods(&mut *MODEL.lock(),0,0).unwrap();
        assert_ne!(changed,before);
        assert_eq!(MODEL.lock().methods[&0x4a0],(old&!0x1ff)|0x19);
        checkpoint().unwrap();assert_eq!(fired(),count+1);
        assert_eq!(MODEL.lock().writes.len(),writes+1);
        assert_eq!(disarm(),syscall::SyscallResult::Ok(0));
        assert_eq!(fired(),count+1);
        assert_eq!(modeset::crc_output_methods(&mut *MODEL.lock(),0,0).unwrap(),before);
        assert_eq!(MODEL.lock().writes[writes],[(0x4a0,(old&!0x1ff)|0x19),(0x84,0x80000000),(0x80,0)]);
        assert_eq!(MODEL.lock().writes[writes+1],[(0x4a0,old),(0x84,0x80000000),(0x80,0)]);
        assert_eq!(disarm(),syscall::SyscallResult::Ok(0));
        assert_eq!(MODEL.lock().writes.len(),writes+2);
    }
    // Failed publication must not manufacture a firing; disarm still restores.
    let before=modeset::crc_output_methods(&mut *MODEL.lock(),0,0).unwrap();
    let count=fired();arm(0);MODEL.lock().fail=true;
    assert!(checkpoint().is_err());assert_eq!(fired(),count);
    assert_eq!(disarm(),syscall::SyscallResult::Ok(0));
    assert_eq!(modeset::crc_output_methods(&mut *MODEL.lock(),0,0).unwrap(),before);
    // Restoration failure is visible and keeps the saved value for a retry.
    arm(0);checkpoint().unwrap();MODEL.lock().fail=true;
    assert_eq!(disarm(),syscall::SyscallResult::Err(5));
    assert_eq!(disarm(),syscall::SyscallResult::Ok(0));
    assert_eq!(modeset::crc_output_methods(&mut *MODEL.lock(),0,0).unwrap(),before);
}
