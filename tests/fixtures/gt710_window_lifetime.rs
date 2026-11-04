#![allow(dead_code)] // Production records include fields exercised by the other native harnesses.
extern crate alloc;
use std::{cell::{Cell,RefCell},collections::BTreeMap,sync::{Arc,Mutex,MutexGuard,atomic::{AtomicU64,Ordering}}};
const PAGE:usize=4096;
const EINVAL:i64=22;
const ENODEV:i64=19;
const ENOMEM:i64=12; const EIO:i64=5; const EAGAIN:i64=11; const RESOURCE_QUOTA:usize=4096;
const MEMORY_QUOTA:u64=128<<20;
thread_local! {
    static BACKEND:Cell<u32>=const {Cell::new(6)};
    static OWNER:RefCell<(gk208::resource::Resources<gpu::dma::Pages>,gk208::memory::allocator::Allocator,Model)>=
        RefCell::new((gk208::resource::Resources::new(),gk208::memory::allocator::Allocator::new(1<<30,&[]).unwrap(),Model::default()));
    static CORE:RefCell<Core>=RefCell::default();
    static PROCESS:RefCell<process::ProcessManager>=RefCell::new(process::ProcessManager::new());
    static RECEIPTS:RefCell<Vec<gk208::adapter::Request>>=RefCell::default();
    static CLOCK:Cell<u64>=const {Cell::new(0)};
    static COMPLETE_AT:Cell<u64>=const {Cell::new(25)};
    static CONTEXT_DROPS:Cell<usize>=const {Cell::new(0)};
    static RESOURCE_DROPS:Cell<usize>=const {Cell::new(0)};
    static NEXT:Cell<u32>=const {Cell::new(1)};
}
#[derive(Default)] struct Proc {quota:gpu::quota::Quota,resources:BTreeMap<u64,u32>,contexts:BTreeMap<u64,u32>,prepared_context:Option<u64>}
#[derive(PartialEq)] enum CtxState {Live,Creating,Quarantined}
struct Scratch{id:u32}
struct Ctx {owner:u64,state:CtxState,dropped:bool,device_alive:bool,busy:bool,inflight:Vec<u64>,vertex_arena:Option<Scratch>,private_scratch:Vec<Scratch>,vb:u32,scratch:u32,scratch_texture:u32}
struct Resource {owner:u64,backing:Arc<Pages>,holds:u32,writers:u32,reading:bool,dropped:bool,lost:bool,epoch:u64}
#[derive(Default)] struct Core {procs:BTreeMap<u64,Proc>,resources:BTreeMap<u32,Resource>,contexts:BTreeMap<u32,Ctx>}
impl Core {
    fn handle(&mut self)->u64{NEXT.with(|n|{let id=n.get();n.set(id+1);u64::from(id)+20000})}
    fn proc_mut(&mut self,owner:u64)->&mut Proc {self.procs.entry(owner).or_default()}
    fn ctx_of(&self,owner:u64,h:u64)->Option<u32>{self.procs.get(&owner)?.contexts.get(&h).copied()}
    // SETTLE
    fn background(&mut self,r:gk208::adapter::Request){self.background_in(0,r);}
    fn background_in(&mut self,_:u64,r:gk208::adapter::Request)->Option<u64>{RECEIPTS.with(|q|q.borrow_mut().push(r));Some(1)}
    // CORE_LIFETIME
}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn notify(){}
mod display {pub fn ended(_: &mut super::Core,_:u64){}}
static GPU_RESOURCES:u8=0; static GPU_CONTEXTS:u8=1;
fn gauge(kind: &u8,n:i64){assert_eq!(n,-1);if *kind==0{RESOURCE_DROPS.with(|c|c.set(c.get()+1));}else{CONTEXT_DROPS.with(|c|c.set(c.get()+1));}}
// DESTROY_RESOURCE
// DESTROY_CONTEXT
fn create_context_with_catalogue(owner:u64,warm:bool)->Result<(u64,u32),i64>{
    assert!(!warm);
    let id=NEXT.with(|n|{let id=n.get();n.set(id+1);id});let h=u64::from(id)+10000;
    drop(gk208::resource::ctx_create(Tag::Background,id).unwrap());
    with_core(|c|{c.contexts.insert(id,Ctx{owner,state:CtxState::Live,dropped:false,device_alive:true,busy:false,inflight:vec![],vertex_arena:None,private_scratch:vec![],vb:0,scratch:0,scratch_texture:0});c.proc_mut(owner).contexts.insert(h,id);});
    Ok((h,0))
}
struct CreationGuard;
impl CreationGuard {fn acquire(_:u64)->Result<Self,i64>{Ok(Self)}}
fn attach_all(_: &[(u32,u32)])->Vec<Result<(),i64>>{vec![Ok(())]}
mod tracing {pub mod providers {pub mod gpu {
    pub static GPU_RESOURCE_BYTES:()=(); pub fn gauge(_: &(),_:i64){}
}}}
mod memory{pub mod vma{pub struct Vma{pub bytes:std::cell::Cell<u64>}impl Vma{pub fn size(&self)->u64{self.bytes.get()}}}}
mod process {
    use super::*;
    #[derive(Clone,Copy,PartialEq)] pub struct ProcessId(u64);
    impl ProcessId{pub fn new(id:u64)->Self{Self(id)}pub fn as_u64(self)->u64{self.0}}
    pub mod limits {
        use super::*;
        pub const AS:usize=0;
        #[derive(Default)] pub struct Limits{pub gpu_bytes:AtomicU64}
        pub struct Limit{pub soft:u64}
        impl Limits {pub fn release_gpu_bytes(&self,b:u64){self.gpu_bytes.fetch_sub(b,Ordering::AcqRel);}pub fn get(&self,_:usize)->Limit{Limit{soft:u64::MAX}}}
    }
    pub struct Process{pub id:ProcessId,pub image_size:u64,pub heap_end:u64,pub heap_start:u64,pub user_stack_top:u64,pub user_stack_bottom:u64,pub vmas:Vec<crate::memory::vma::Vma>,pub limits:Arc<limits::Limits>,pub thread_group_id:Option<u64>,pub exit_started:bool,pub group_exit_code:Option<i32>,pub terminated:bool}
    impl Process {
        pub fn is_terminated(&self)->bool{self.terminated}
        // MAPPED_BYTES
        pub fn reserve_gpu_bytes(&self,b:u64)->bool{self.limits.gpu_bytes.fetch_add(b,Ordering::AcqRel);true}
    }
    pub struct ProcessManager{pub rows:BTreeMap<u64,Process>}
    impl ProcessManager {
        pub fn new()->Self{let mut rows=BTreeMap::new();for id in [7,8,9]{rows.insert(id,Process{id:ProcessId(id),image_size:0,heap_end:0,heap_start:0,user_stack_top:0,user_stack_bottom:0,vmas:vec![crate::memory::vma::Vma{bytes:Cell::new(0)}],limits:Default::default(),thread_group_id:Some(if id==9{7}else{id}),exit_started:false,group_exit_code:None,terminated:false});}Self{rows}}
        pub fn get_process(&self,id:ProcessId)->Option<&Process>{self.rows.get(&id.0)}
        pub fn find_process_by_thread(&self,tid:u64)->Option<(ProcessId,&Process)>{self.rows.get(&tid).map(|p|(ProcessId(tid),p))}
        pub fn thread_group_peers(&self,pid:ProcessId)->Vec<ProcessId>{let g=self.rows[&pid.0].thread_group_id;self.rows.iter().filter(|(id,p)|**id!=pid.0 && p.thread_group_id==g).map(|(id,_)|ProcessId(*id)).collect()}
    }
    pub fn with_process_manager<T>(f:impl FnOnce(&ProcessManager)->T)->Option<T>{Some(PROCESS.with(|p|f(&p.borrow())))}
}
mod abi {pub mod backend {pub const GK208:u32=6;}}
mod kind {pub const BUFFER:u32=3;pub const TARGET:u32=1;}
mod format {pub const B8G8R8A8:u32=0;pub fn bytes(f:u32)->u32{if f==2{1}else{4}}}
#[path="ROOT/kernel/src/gpu/quota.rs"] pub(crate) mod quota;
mod gpu {
    pub(crate) use crate::quota;
    pub(crate) use crate::targets as window_targets;
    pub mod dev {
        pub enum Tag {Background}
        pub(crate) use crate::gk208::resource::{resource_unref,ctx_destroy};
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
use gpu::{dev,dev::Tag,dma::Pages,quota::Charge};
mod console {
    use super::*;
    // PITCH
    pub(super) fn resource_with(ctx:u32,k:u32,f:u32,w:u32,h:u32,scanout:bool,pages:Arc<Pages>)->Result<u32,i64>{
        assert!(with_core(|c|c.contexts.contains_key(&ctx)).unwrap());
        let id=NEXT.with(|n|{let id=n.get();n.set(id+1);id});
        gk208::resource::resource_create(dev::Tag::Background,id,k,f,w,h,scanout).ok_or(ENOMEM)?;
        drop(gk208::resource::attach_backing(dev::Tag::Background,id,pages.clone()).ok_or(ENOMEM)?);
        with_core(|c|{c.resources.insert(id,Resource{owner:0,backing:pages,holds:1,writers:0,reading:false,dropped:false,lost:false,epoch:0});});
        Ok(id)
    }
    pub(super) fn target_pitch(w:u32)->u32{pitch_of(kind::TARGET,format::B8G8R8A8,w)}
}
// BACKING_LEN
// DEVICE_FRAMES
mod irq_safe_mutex {pub use crate::Lock as IrqSafeMutex;}
pub struct Lock<T>(Mutex<T>);
impl<T> Lock<T>{const fn new(t:T)->Self{Self(Mutex::new(t))}fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}}
mod targets {
    use super::*;
    // CONTEXT
    // TARGET_RECORD
    impl Target {
        // TARGET_MAKE
        // TARGET_REVOKE
        pub fn len(&self)->u64{self.bytes}
        pub fn clear(&self)->Result<(),i64>{Ok(())}
        pub fn new(owner:u64,w:u32,h:u32)->Arc<Self>{Self::make(owner,w,h).unwrap()}
        pub fn resource(&self)->u32{self.id}
    }
    // TARGET_DROP
}
use targets::Target;
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
mod window {
    use super::*;
    use breengel_abi::{errno::{EPERM,ENOENT},limits};
    pub struct Caller{pub tgid:u64}
    impl Caller{pub fn current()->Result<Self,i32>{Ok(Self{tgid:7})}pub fn is_root(&self)->bool{true}}
    #[derive(Default)] pub struct Waits{pub acquire:Queue,pub events:Queue}
    #[derive(Default)] pub struct Queue;
    impl Queue{fn wake_up(&self){}}
    pub struct Buffer{pub mem:Arc<Target>}
    pub struct Tree{pub bytes:Vec<u8>}
    impl Tree{fn len(&self)->u64{self.bytes.len() as u64}}
    pub struct Window{pub tree_addr:u64,pub id:u64,pub owner:u64,pub holder:Option<u64>,pub front:Option<usize>,pub buffers:Vec<Buffer>,pub tree:Arc<Tree>,pub width:u32,pub height:u32,pub front_commit:u64,pub waits:Arc<Waits>,pub present:Present}
    impl Window{fn tree_len(&self)->u64{64*1024}}
    pub struct Present{pub shown:()}
    pub struct State;
    impl State{fn uncover(&mut self,_:()){}}
    #[derive(Clone,Copy)] pub struct Claim{pub holder:u64}
    // REMNANT
    pub struct Registry{pub windows:BTreeMap<u64,Window>,pub remnants:BTreeMap<u64,Remnant>,pub claim:Option<Claim>,pub present:State}
    impl Registry{fn window(&mut self,id:u64,c:&Caller,allow:fn(bool,bool,&Caller)->bool)->Result<&mut Window,i32>{
        let w=self.windows.get_mut(&id).ok_or(ENOENT)?;
        if !allow(w.owner==c.tgid,false,c){return Err(EPERM);}Ok(w)
    }}
    mod who{use super::Caller;pub fn owner(o:bool,_:bool,_:&Caller)->bool{o}}
    pub mod mem{
        pub use super::Tree as Mem;
        pub fn unmap(_: &Mem,_:u64){crate::PROCESS.with(|p|p.borrow().rows[&7].vmas[0].bytes.set(0));}
        pub fn unmapped_len(_: &Mem,_:u64,_:&[crate::memory::vma::Vma])->u64{0}
    }
    // CHARGED_BYTES
    pub static REGISTRY:Lock<Registry>=Lock::new(Registry{windows:BTreeMap::new(),remnants:BTreeMap::new(),claim:None,present:State});
    pub mod scene{pub fn invalidate(_:u64){} pub fn changed(){}}
    fn end_claim(reg: &mut Registry)->Vec<Arc<Waits>>{reg.claim=None;vec![]}
    fn claim_ended(_:Vec<Arc<Waits>>){}
    pub mod ops{
        use super::*;
        fn publish_ready(){} // No pending submissions in this teardown harness.
        use super::mem::Mem;
        // UNMAP_ALL
        // DESTROY_WINDOW
        // GROUP_GONE
        // ROOM_OF
        // RELEASE_REMNANT
    }
    // EXIT_HOOKS
}
// Both the native allocator and quota/limits retain the backing; only the
// device boundary (receipt arrival) is modeled. No direct counter reset.
fn advance_device(){
    if CLOCK.with(Cell::get)>=COMPLETE_AT.with(Cell::get){
        RECEIPTS.with(|q|q.borrow_mut().clear());
    }
}
fn now_ms()->u64{CLOCK.with(Cell::get)}
fn since(start:u64)->u64{now_ms()-start}
mod time {pub fn sleep_ms(n:u64)->Result<(),()>{crate::CLOCK.with(|c|c.set(c.get()+n));crate::advance_device();Ok(())}}
mod gpu_checks {
    pub struct Counters{pub resource_bytes:u64,pub resources:u64}
    pub fn counters()->Result<Counters,String>{Ok(crate::with_core(|c|Counters{
        resource_bytes:c.procs.get(&7).map_or(0,|p|p.quota.resource_bytes()),
        resources:c.procs.get(&7).map_or(0,|p|p.resources.len() as u64)}).unwrap())}
}
type CaseResult=Result<(),String>;
trait Cite<T>{fn cite(self,_:&str)->Result<T,String>;}
impl<T> Cite<T> for Result<T,String>{fn cite(self,_:&str)->Self{self}}
fn ensure(yes:bool,_:&str,why:&str)->CaseResult{if yes{Ok(())}else{Err(why.into())}}
fn b12_drain(gpu_baseline:gpu_checks::Counters)->CaseResult{
    // B12_DRAIN
    drain()
}
fn usage(owner:u64)->u64{
    process::with_process_manager(|m|m.get_process(process::ProcessId::new(owner)).unwrap().mapped_bytes()).unwrap()

}
fn add_window(id:u64,holder:Option<u64>)->(u32,std::sync::Weak<Target>,std::sync::Weak<window::Tree>){
    let buffers:Vec<_>=(0..3).map(|_|window::Buffer{mem:Target::new(7,128,64)}).collect();
    let front=Arc::downgrade(&buffers[1].mem);let rid=buffers[1].mem.resource();
    let mut bytes=vec![0;3*64*1024];bytes[64*1024..64*1024+16].copy_from_slice(b"published-tree!!");
    // A later, uncommitted tree is deliberately different from the front.
    bytes[2*64*1024..2*64*1024+16].copy_from_slice(b"pending-not-kept");
    let tree=Arc::new(window::Tree{bytes});let weak=Arc::downgrade(&tree);
    PROCESS.with(|p|p.borrow().rows[&7].vmas[0].bytes.set(p.borrow().rows[&7].vmas[0].bytes.get()+tree.bytes.len() as u64));
    window::REGISTRY.lock().windows.insert(id,window::Window{tree_addr:0,id,owner:7,holder,front:Some(1),buffers,tree,width:128,height:64,front_commit:42,waits:Default::default(),present:window::Present{shown:()}});
    (rid,front,weak)
}
fn end_gpu_owner(){with_core(|c|c.reclaim(7));PROCESS.with(|p|p.borrow().rows[&7].vmas[0].bytes.set(0));}
fn counts()->(usize,usize){(CONTEXT_DROPS.with(Cell::get),RESOURCE_DROPS.with(Cell::get))}
fn drain_receipts(){CLOCK.with(|c|c.set(COMPLETE_AT.with(Cell::get)));advance_device();}
fn stop_peer(){PROCESS.with(|p|p.borrow_mut().rows.get_mut(&9).unwrap().terminated=true);}
#[test]
fn last_thread_exit_and_kill_hooks_clean_handles_once(){
    let baseline=gk208::resource::ownership_census();
    let (_,front,tree)=add_window(1,None);
    // First thread exits with a live peer: neither windows nor GPU handles end.
    window::thread_exit(7);
    assert_eq!(window::REGISTRY.lock().windows.len(),1);assert_eq!(counts(),(0,0));
    stop_peer();
    window::row_exit(process::ProcessId::new(7));
    end_gpu_owner();
    // Natural exit can follow the row-kill hook, and must be idempotent.
    window::thread_exit(7);end_gpu_owner();
    assert_eq!(counts(),(1,3));
    assert!(front.upgrade().is_none() && tree.upgrade().is_none());
    assert!(window::REGISTRY.lock().windows.is_empty());
    assert!(with_core(|c|c.procs.is_empty()&&c.resources.is_empty()&&c.contexts.is_empty()).unwrap());
    drain_receipts();assert_eq!(usage(7),0);assert_eq!(gk208::resource::ownership_census(),baseline);
}
#[test]
fn exec_creates_no_remnant_and_new_image_has_independent_accounting(){
    let baseline=gk208::resource::ownership_census();
    let (_,front,tree)=add_window(1,Some(8));
    // Exec commits its new address space before these production cleanup hooks.
    window::exec_done_current();end_gpu_owner();
    assert!(front.upgrade().is_none()&&tree.upgrade().is_none());
    assert!(window::REGISTRY.lock().remnants.is_empty());assert_eq!(counts(),(1,3));
    assert_eq!(usage(8),0);assert_eq!(usage(7),98_304,"old device receipts still own their charge");
    let old_limits=PROCESS.with(|p|p.borrow().rows[&7].limits.clone());
    PROCESS.with(|p|p.borrow_mut().rows.get_mut(&7).unwrap().limits=Default::default());
    let target=Target::new(7,128,64);
    assert_eq!(usage(7),32_768);
    drain_receipts();assert_eq!(old_limits.gpu_bytes.load(Ordering::Acquire),0);
    assert_eq!(usage(7),32_768,"old receipts must not debit the new incarnation");
    drop(target);targets::release_context(7);end_gpu_owner();drain_receipts();
    assert_eq!(usage(7),0);assert_eq!(gk208::resource::ownership_census(),baseline);
}
#[test]
fn sixteen_remnants_keep_only_published_front_and_release_all_charges(){
    let baseline=gk208::resource::ownership_census();
    let mut weak:Vec<_>=(1..=16).map(|id|add_window(id,Some(8))).collect();
    stop_peer();window::row_exit(process::ProcessId::new(7));end_gpu_owner();drain_receipts();
    // A new owner incarnation reaches the per-holder remnant cap without
    // exceeding the 16-live-window limit of either incarnation.
    weak.push(add_window(17,Some(8)));
    window::thread_exit(7);end_gpu_owner();drain_receipts();
    assert_eq!(window::REGISTRY.lock().remnants.len(),16);
    assert_eq!(usage(8),16*(32_768+196_608));assert_eq!(usage(7),16*32_768);
    assert_eq!(counts(),(2,35),"private context handles must end even while remnants retain Targets");
    for (id,(_,front,tree)) in (1..=17).zip(&weak){
        if id==17 {assert!(front.upgrade().is_none()&&tree.upgrade().is_none());continue;}
        let reg=window::REGISTRY.lock();let r=&reg.remnants[&id];
        assert_eq!(r.commit_id,42);assert_eq!(r.tree_offset,64*1024);
        assert!(Arc::ptr_eq(r.pixels.as_ref().unwrap(),&front.upgrade().unwrap()));
        assert!(Arc::ptr_eq(&r.tree,&tree.upgrade().unwrap()));
        assert_eq!(&r.tree.bytes[r.tree_offset as usize..r.tree_offset as usize+16],b"published-tree!!");
    }
    let holder=window::Caller{tgid:8};
    for id in 1..=16 {window::ops::release_remnant(&holder,id).unwrap();}
    assert_eq!(usage(8),0);assert_eq!(counts(),(2,51));
    drain_receipts();assert_eq!(usage(7),0);assert_eq!(gk208::resource::ownership_census(),baseline);
    assert!(weak.iter().all(|(_,f,t)|f.upgrade().is_none()&&t.upgrade().is_none()));
}
#[test]
fn holder_exit_releases_retained_targets(){
    let baseline=gk208::resource::ownership_census();add_window(1,Some(8));stop_peer();
    window::thread_exit(7);end_gpu_owner();drain_receipts();
    assert_eq!(window::REGISTRY.lock().remnants.len(),1);
    window::ops::group_gone(8,true);
    assert!(window::REGISTRY.lock().remnants.is_empty());drain_receipts();
    assert_eq!(usage(8),0);assert_eq!(usage(7),0);assert_eq!(gk208::resource::ownership_census(),baseline);
}
#[test]
fn hundred_native_window_destroys_use_b12_bounded_wait(){
    // Match the suite's warmup before taking its baseline: the process's
    // reusable private context survives window churn.
    let warm=Target::new(7,128,64);drop(warm);drain_receipts();
    let baseline=gk208::resource::ownership_census();
    COMPLETE_AT.with(|t|t.set(50));
    let quota=with_core(|c|c.proc_mut(7).quota.clone()).unwrap();
    let gpu_baseline=gpu_checks::counters().unwrap();
    for id in 1..=100 {
        add_window(id,None);
        window::ops::destroy(&window::Caller{tgid:7},id).unwrap();
    }
    assert!(window::REGISTRY.lock().windows.is_empty());
    assert_eq!(quota.resource_bytes(),100*98_304);assert_eq!(usage(7),100*98_304);
    b12_drain(gpu_baseline).unwrap();
    assert_eq!(now_ms(),50,"must poll real delayed receipts through the shared B12 loop");
    assert_eq!(usage(7),0);assert_eq!(quota.resource_bytes(),0);
    assert_eq!(gk208::resource::ownership_census(),baseline);
    targets::release_context(7);end_gpu_owner();
}
#[test]
fn b12_timeout_does_not_forge_completion_or_drop_charge(){
    let quota=with_core(|c|c.proc_mut(7).quota.clone()).unwrap();
    let baseline=gpu_checks::counters().unwrap();let target=Target::new(7,128,64);drop(target);
    COMPLETE_AT.with(|t|t.set(1_005));
    assert!(b12_drain(baseline).unwrap_err().contains("one second"));
    assert_eq!(now_ms(),1_000);assert_eq!(quota.resource_bytes(),32_768);assert_eq!(usage(7),32_768);
    drain_receipts();assert_eq!(usage(7),0);
    targets::release_context(7);end_gpu_owner();
}

#[test]
fn native_device_holds_outlive_window_and_unref_receipt(){
    let baseline=gk208::resource::ownership_census();
    let target=Target::new(7,128,64);let id=target.resource();
    let hold=gk208::ce::with_resources(|r,_,_|r.hold(id)).unwrap();
    drop(target);drain_receipts();
    assert_eq!(usage(7),32_768,"native queued access must keep backing after the core unref receipt");
    let held=gk208::resource::ownership_census();
    assert!(held.contains("entries=1"));assert!(held.contains("device_refs=2"));
    assert!(held.contains("backing_refs=1"));assert!(held.contains("dropped=true"));
    drop(hold);
    // The native request owner collects after releasing its access token.
    gk208::ce::with_resources(|r,a,io|r.collect(a,io)).unwrap();
    assert_eq!(usage(7),0);
    targets::release_context(7);end_gpu_owner();drain_receipts();
    assert_eq!(gk208::resource::ownership_census(),baseline);
}
