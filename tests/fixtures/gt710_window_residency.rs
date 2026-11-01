extern crate alloc;
use std::{cell::{Cell,RefCell},collections::BTreeMap,sync::{Arc,atomic::{AtomicU64,Ordering}}};
const PAGE:usize=4096;
const EINVAL:i64=22;
const ENODEV:i64=19;
const ENOMEM:i64=12;
const MEMORY_QUOTA:u64=128<<20;
thread_local! {
    static BACKEND:Cell<u32>=const {Cell::new(6)};
    static OWNER:RefCell<(gk208::resource::Resources<gpu::dma::Pages>,gk208::memory::allocator::Allocator,Model)>=
        RefCell::new((gk208::resource::Resources::new(),gk208::memory::allocator::Allocator::new(1<<30,&[]).unwrap(),Model::default()));
    static CORE:RefCell<Core>=RefCell::new(Core{quota:Default::default()});
    static PROCESS:RefCell<process::ProcessManager>=RefCell::new(process::ProcessManager(process::Process {
        limits:Arc::new(process::limits::Limits::default()),tree:Cell::new(0),limit:Cell::new(u64::MAX)}));
    static BACKINGS:RefCell<BTreeMap<u32,Arc<gpu::dma::Pages>>>=RefCell::default();
    static NEXT:Cell<u32>=const {Cell::new(1)};
}
struct Core{quota:gpu::quota::Quota}
impl Core {fn proc_mut(&mut self,owner:u64)->&mut Self{assert_eq!(owner,7);self}}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
mod tracing {pub mod providers {pub mod gpu {
    pub static GPU_RESOURCE_BYTES:()=(); pub fn gauge(_: &(),_:i64){}
}}}
mod process {
    use super::*;
    pub struct ProcessId(u64);
    impl ProcessId{pub fn new(id:u64)->Self{Self(id)}}
    pub mod limits {
        use super::*;
        #[derive(Default)] pub struct Limits{pub gpu_bytes:AtomicU64}
        impl Limits {pub fn release_gpu_bytes(&self,bytes:u64){self.gpu_bytes.fetch_sub(bytes,Ordering::AcqRel);}}
    }
    pub struct Process{pub limits:Arc<limits::Limits>,pub tree:Cell<u64>,pub limit:Cell<u64>}
    impl Process {
        pub fn mapped_bytes(&self)->u64{self.tree.get()+self.limits.gpu_bytes.load(Ordering::Acquire)}
        pub fn reserve_gpu_bytes(&self,bytes:u64)->bool{
            if self.mapped_bytes().saturating_add(bytes)>self.limit.get(){return false;}
            self.limits.gpu_bytes.fetch_add(bytes,Ordering::AcqRel);true
        }
    }
    pub struct ProcessManager(pub Process);
    impl ProcessManager {pub fn get_process(&self,id:ProcessId)->Option<&Process>{(id.0==7).then_some(&self.0)}}
    pub fn with_process_manager<T>(f:impl FnOnce(&ProcessManager)->T)->Option<T>{Some(PROCESS.with(|p|f(&p.borrow())))}
}
mod abi {pub mod backend {pub const GK208:u32=6;}}
mod kind {pub const BUFFER:u32=3;pub const TARGET:u32=1;}
mod format {pub const B8G8R8A8:u32=0;pub fn bytes(f:u32)->u32{if f==2{1}else{4}}}
#[path="ROOT/kernel/src/gpu/quota.rs"] pub(crate) mod quota;
mod gpu {
    pub(crate) use crate::quota;
    pub mod dev {
        pub struct Tag;
        pub fn backend()->u32{crate::BACKEND.with(|b|b.get())}
        pub fn is_svga()->bool{backend()==5}
    }
    pub mod dma {
        pub struct Pages{count:usize,_copy_charge:Option<super::quota::Charge>}
        impl Pages {
            pub fn new(count:usize)->Option<Self>{Some(Self{count,_copy_charge:None})}
            pub fn count(&self)->usize{self.count}
            pub fn bytes(&self)->usize{self.count*crate::PAGE}
            pub fn page_addr(&self,i:usize)->u64{0x200000+i as u64*4096}
            pub fn identify(&self,_:&str,_:u32,_:u32){}
            // CHARGE_COPY
        }
    }
}
use gpu::{dev,dma::Pages,quota::Charge};
mod console {
    use super::*;
    // PITCH
    pub(super) fn resource_with(ctx:u32,k:u32,f:u32,w:u32,h:u32,scanout:bool,pages:Arc<Pages>)->Result<u32,i64>{
        assert_eq!(ctx,1);
        let id=NEXT.with(|n|{let id=n.get();n.set(id+1);id});
        gk208::resource::resource_create(dev::Tag,id,k,f,w,h,scanout).ok_or(ENOMEM)?;
        gk208::resource::attach_backing(dev::Tag,id,pages.clone()).ok_or(ENOMEM)?;
        BACKINGS.with(|b|b.borrow_mut().insert(id,pages));
        Ok(id)
    }
    pub(super) fn target_pitch(w:u32)->u32{pitch_of(kind::TARGET,format::B8G8R8A8,w)}
}
// BACKING_LEN
// DEVICE_FRAMES
struct Context{id:u32}
fn make(width:u32,height:u32)->Result<u32,i64>{
    let owner=7;let context=Context{id:1};
    // WINDOW_ALLOCATION
    Ok(id)
}
#[derive(Default)]
struct Model{words:BTreeMap<u64,u64>,fail:bool}
impl gk208::vm::Io for Model {
    fn read(&mut self,a:u64)->Result<u64,&'static str>{Ok(*self.words.get(&a).unwrap_or(&0))}
    fn write(&mut self,a:u64,v:u64)->Result<(),&'static str>{self.words.insert(a,v);Ok(())}
    fn flush(&mut self)->Result<(),&'static str>{Ok(())}
    fn invalidate(&mut self,_:u64)->Result<u32,&'static str>{if self.fail{Err("withheld invalidation")}else{Ok(0)}}
    fn register(&mut self,_:usize)->u32{0}
}
mod gk208 {
    #[allow(dead_code)] // Full hardware memory modules at the modeled MMIO boundary.
    #[path="ROOT/kernel/src/gpu/gk208/memory.rs"] pub mod memory;
    #[allow(dead_code)]
    #[path="ROOT/kernel/src/gpu/gk208/vm.rs"] pub mod vm;
    #[allow(dead_code)]
    #[path="ROOT/kernel/src/gpu/gk208/copy.rs"] pub mod copy;
    // RESOURCE_MODULE
    pub use resource::present;
    pub mod adapter {
        use alloc::sync::Arc;
        use crate::dev::Tag;
        pub struct Operation;
        pub struct EncodedDraw;
        #[allow(dead_code)] // Complete request record; only storage retirement runs here.
        // STORAGE_REQUEST
        impl Request {
            // STORAGE_CONSTRUCTOR
        }
    }
    pub mod ce {
        pub fn with_resources<T>(f:impl FnOnce(&mut super::resource::Resources<crate::Pages>,&mut super::memory::allocator::Allocator,&mut crate::Model)->Result<T,&'static str>)->Result<T,&'static str>{
            crate::OWNER.with(|s|{let mut s=s.borrow_mut();let(r,a,io)=&mut *s;f(r,a,io)})
        }
    }
}
fn usage()->(u64,u64){
    (PROCESS.with(|p|p.borrow().0.mapped_bytes()),with_core(|c|c.quota.resource_bytes()).unwrap())
}
fn unref(id:u32)->Option<gk208::adapter::Request>{
    let backing=BACKINGS.with(|b|b.borrow_mut().remove(&id));
    gk208::resource::resource_unref(dev::Tag,id,backing)
}
#[test]
fn hundred_window_cycles_return_native_census_and_charges_after_unref(){
    let baseline=gk208::resource::ownership_census();
    let baseline_usage=usage();
    for cycle in 0..100 {
        PROCESS.with(|p|p.borrow().0.tree.set(3*64*1024));
        let ids:Vec<_>=(0..3).map(|_|make(128,64).unwrap()).collect();
        let holds:Vec<_>=ids.iter().map(|&id|gk208::ce::with_resources(|r,_,_|r.hold(id)).unwrap()).collect();
        let receipts:Vec<_>=ids.into_iter().map(|id|unref(id).unwrap()).collect();
        PROCESS.with(|p|p.borrow().0.tree.set(0));
        drop(receipts);
        assert_eq!(usage().0,baseline_usage.0+98_304,"cycle {cycle}: device holds lost their charge");
        drop(holds);
        gk208::ce::with_resources(|r,a,io|r.collect(a,io)).unwrap();
        assert_eq!(usage(),baseline_usage,"cycle {cycle}: charges did not return");
        assert_eq!(gk208::resource::ownership_census(),baseline,"cycle {cycle}: native registry did not return");
    }
}
#[test]
fn b12_split_and_native_census_keep_vram_out_of_vmsize(){
    PROCESS.with(|p|p.borrow().0.tree.set(3*64*1024));
    let ids:Vec<_>=(0..3).map(|_|make(128,64).unwrap()).collect();
    assert_eq!(usage(),(294_912,98_304));
    let vram=gk208::ce::with_resources(|r,a,_|{assert_eq!(r.vram_bytes(),a.stats().charged);Ok(r.vram_bytes())}).unwrap();
    assert!(vram>=98_304,"native images and their page tables must be counted additionally");
    let census=gk208::resource::ownership_census();
    let receipts:Vec<_>=ids.into_iter().map(|id|unref(id).unwrap()).collect();
    PROCESS.with(|p|p.borrow().0.tree.set(0));
    assert_eq!(usage(),(98_304,98_304),"unreaped unref receipts still own guest backing");
    drop(receipts);
    assert_eq!(usage(),(0,0));
    gk208::ce::with_resources(|r,a,_|{assert_eq!(r.vram_bytes(),0);assert_eq!(a.stats().charged,0);Ok(())}).unwrap();
    assert!(census.lines().next().unwrap().split_whitespace().any(|v|v==format!("vram_bytes={vram}")),"native census omits VRAM: {census}");
}
#[test]
fn held_device_work_and_failed_unref_retain_both_charges(){
    for fail in [false,true]{
        let id=make(128,64).unwrap();
        let hold=gk208::ce::with_resources(|r,_,_|r.hold(id)).unwrap();
        drop(unref(id));
        assert_eq!(usage(),(32_768,32_768),"dropping a handle or receipt cannot release held backing");
        drop(hold);
        let retained=gk208::ce::with_resources(|r,a,_|Ok((r.vram_bytes(),a.stats()))).unwrap();
        gk208::ce::with_resources(|r,a,io|{io.fail=fail;let result=r.collect(a,io);assert_eq!(result.is_err(),fail);Ok(())}).unwrap();
        if fail{
            assert_eq!(usage(),(32_768,32_768));
            gk208::ce::with_resources(|r,a,_|{assert_eq!((r.vram_bytes(),a.stats()),retained);Ok(())}).unwrap();
            // Failed invalidation poisons the VM; a later successful MMIO
            // operation is not a confirmed reset and cannot release storage.
        }else{assert_eq!(usage(),(0,0));}
    }
}
#[test]
fn window_pitch_and_guest_extent_match_native_targets(){
    for (w,h) in [(1,65),(17,65),(48,65),(65,129),(128,64),(129,64)]{
        let native=gk208::resource::Layout::new(w,h,4).unwrap();
        assert_eq!(console::target_pitch(w),native.pitch,"window/native pitch at width {w}");
        assert_eq!(backing_len(w,h),(u64::from(native.pitch)*u64::from(h)).next_multiple_of(4096));
    }
    for (backend,pitch) in [(4,256),(5,192)]{
        BACKEND.with(|b|b.set(backend));
        assert_eq!(console::target_pitch(48),pitch);
        assert_eq!(backing_len(48,65),(u64::from(pitch)*65).next_multiple_of(4096));
    }
}
#[test]
fn denied_rlimit_or_quota_reservation_leaves_no_charge(){
    PROCESS.with(|p|p.borrow().0.limit.set(32_767));
    assert!(device_frames(7,32_768).is_err());assert_eq!(usage(),(0,0));
    PROCESS.with(|p|p.borrow().0.limit.set(u64::MAX));
    let full=Charge::new(7,MEMORY_QUOTA,false).unwrap();
    assert!(device_frames(7,1).is_err());assert_eq!(usage(),(MEMORY_QUOTA,MEMORY_QUOTA));
    drop(full);assert_eq!(usage(),(0,0));
}
