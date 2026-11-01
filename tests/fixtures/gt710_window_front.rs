#![allow(dead_code)] // Production records expose fields covered by other host harnesses.
extern crate alloc;
use std::{cell::RefCell, collections::{BTreeMap, VecDeque}, sync::{Arc, Mutex, MutexGuard, atomic::Ordering}};
use gpu_abi::{self as abi, fence_status, format, stream::{self, Resolver, TargetInfo}};
use breengel_abi::{Plain, Rect};
use gpu_abi::Plain as GpuPlain;
const PAGE:usize=4096;
const EIO:i64=5; const EINVAL:i64=22; const ENOMEM:i64=12; const ENODEV:i64=19;
const EINTR:i64=4; const EFAULT:i64=14; const FLIP_Y:bool=false;
#[allow(dead_code)] // The native translator embeds the full catalogue.
#[path="ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] mod shaders;
mod gk208 {
    pub(crate) use crate::shaders;
    #[path="ROOT/kernel/src/gpu/gk208/caps.rs"] pub mod caps;
    #[path="NATIVE"] pub mod encode;
}
#[derive(Default)] struct Pages(RefCell<Vec<u8>>);
impl Pages {
    fn new(n:usize)->Option<Self>{Some(Self(RefCell::new(vec![0;n*PAGE])))}
    fn write(&self,at:usize,b:&[u8]){self.0.borrow_mut()[at..at+b.len()].copy_from_slice(b);}
    fn read(&self,at:usize,b:&mut[u8]){b.copy_from_slice(&self.0.borrow()[at..at+b.len()]);}
    fn maintain(&self,_:usize,_:usize,_:bool){}
    fn charge_copy(&mut self,_:Charge){}
}
struct Charge;
impl Charge {fn new(_:u64,_:u64,_:bool)->Result<Self,i64>{Ok(Self)}}
struct Staging{pages:Pages}
impl Staging {fn new(_:u64,n:usize)->Result<Self,i64>{Ok(Self{pages:Pages::new(n.div_ceil(PAGE)).unwrap()})}}
#[derive(Clone,Copy)] struct LastWrite{ctx:u32,device_ctx:u32,fence:u64,serial:u64,answer:u32,kind:u32}
#[derive(PartialEq)] enum CtxState{Live}
#[derive(Clone,Default)] struct Objects;
#[derive(Clone,Default)] struct Dx;
struct Ctx{state:CtxState,dropped:bool,busy:bool,inflight:Vec<u64>,objects:Objects,dx:Dx,vb:u32,scratch:u32,scratch_texture:u32}
struct Resource{lost:bool,invalid:bool,window_sealed:bool,reading:bool,writers:u32,
    accepted:Vec<Arc<gpu::window_targets::Writer>>,epoch:u64,generation:u64,last:Option<LastWrite>,
    backing:Arc<Pages>,pitch:u32,image:Vec<u8>}
struct SyncReq{reading:Vec<u32>}
#[derive(Default)] struct Core{syncs:BTreeMap<u64,SyncReq>,resources:BTreeMap<u32,Resource>,contexts:BTreeMap<u32,Ctx>,sequence:u64}
impl Core{fn serial(&mut self)->u64{self.sequence+=1;self.sequence} fn device_fence(&mut self)->u64{self.sequence+100}}
thread_local!{
    static CORE:RefCell<Core>=RefCell::default();
    static USER:RefCell<Vec<u8>>=RefCell::new(vec![0;16384]);
    static EVENTS:RefCell<Vec<&'static str>>=const{RefCell::new(Vec::new())};
    static PROGRESS:RefCell<Option<Box<dyn FnOnce()>>>=RefCell::new(None);
    static CAPTURE_ACTION:RefCell<Option<Box<dyn FnOnce()>>>=RefCell::new(None);
    static INTERRUPT_COPY:RefCell<bool>=const{RefCell::new(false)};
    static FAIL_COPY:RefCell<bool>=const{RefCell::new(false)};
}
fn event(e:&'static str){EVENTS.with(|v|v.borrow_mut().push(e));}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(CORE.with(|v|f(&mut v.borrow_mut())))}
fn notify(){}
fn caller()->Result<u64,i64>{Ok(7)}
fn wait_for<T>(_:Option<u64>,_:bool,mut f:impl FnMut(&mut Core)->Option<T>)->Result<T,()> {
    for _ in 0..4 {
        if let Some(value)=with_core(&mut f).flatten(){return Ok(value);}
        let next=PROGRESS.with(|p|p.borrow_mut().take());
        if let Some(next)=next{next();} else{return Err(());}
    }
    panic!("wait failed to make progress")
}
fn release_other(id:Option<u32>){with_core(|c|{
    if let Some(id)=id{if !c.syncs.values().any(|s|s.reading.contains(&id)){
        c.resources.get_mut(&id).unwrap().reading=false;
    }}
});}
struct Done{answer:u32,echoed:u64}
impl Done{fn matches_fence(&self,f:u64)->bool{self.echoed==f}}
enum Request{Attach,Virtual,Native(gk208::encode::Chunk)}
#[derive(Clone,Copy)] enum Tag{Sync(u64)}
mod dev {
    use super::*;
    pub fn backend()->u32{6}
    pub fn is_svga()->bool{false}
    pub fn backend_code()->u32{backend()}
    pub fn context_id(id:u32)->u32{id}
    pub fn draw_kind()->u32{abi::gk208::DRAW_RELEASE}
    pub fn ok(answer:u32)->bool{answer==abi::gk208::SUCCESS}
    pub fn ctx_attach(_:Tag,_:u32,_:u32)->Option<Request>{Some(Request::Attach)}
    pub fn submit_virgl(_:Tag,_:u32,_:Option<u64>,_:())->Request{Request::Virtual}
    pub fn submit_svga(_:Tag,_:u32,_:Option<u64>,_:())->Option<Request>{Some(Request::Virtual)}
    pub fn submit_gk208(_:Tag,_:u64,chunk:gk208::encode::Chunk)->Request{Request::Native(chunk)}
}
fn exec_ok(build:impl FnOnce(Tag)->Option<Request>,_:u32,_:&[u32])->Result<Done,i64>{
    assert!(matches!(build(Tag::Sync(0)),Some(Request::Attach))); Ok(Done{answer:abi::gk208::SUCCESS,echoed:0})
}
fn exec_req(serial:u64,req:Request,_:u32,holds:&[u32],reading:&[u32])->Result<Done,i64>{
    let Request::Native(chunk)=req else{event("virtual-submit");return Err(EIO);};
    event("native-submit");
    if INTERRUPT_COPY.with(|f|*f.borrow()){
        with_core(|c|{c.syncs.insert(serial,SyncReq{reading:reading.to_vec()});});
        return Err(EINTR);
    }
    if FAIL_COPY.with(|f|*f.borrow()){return Err(EIO);}
    if let Some((src,dst,r))=chunk.copy {
        assert!(holds.contains(&src) && holds.contains(&dst));
        with_core(|c|{
            let s=&c.resources[&src];
            assert!(s.accepted.iter().all(|w|w.ended()),"copy overtook native source fence");
            let input=s.image.clone(); let output=&mut c.resources.get_mut(&dst).unwrap().image;
            for y in 0..r[3] as usize {let n=r[2] as usize*4;
                let from=((r[1] as usize+y)*8+r[0] as usize)*4;
                let to=((r[5] as usize+y)*8+r[4] as usize)*4;
                output[to..to+n].copy_from_slice(&input[from..from+n]);
            }
        });
        event("gpu-copy-complete");
    } else {assert!(!chunk.words.is_empty());event("gpu-clear-complete");}
    let fence=with_core(|c|c.sequence+100).unwrap();
    Ok(Done{answer:abi::gk208::SUCCESS,echoed:fence})
}
struct Seen{unwritten:u64}
fn observe(_:u64,id:u32,last:Option<LastWrite>,_:&Arc<Pages>,_:u32,_:u32,a:abi::Readback,stage:&Pages,_:Option<u32>)
    ->Result<Seen,(i64,u32)>{
    event("native-readback");
    with_core(|c|{
        let r=&c.resources[&id];assert!(r.reading && r.writers==0 && r.accepted.iter().all(|w|w.ended()));
        assert_eq!(last.map(|w|w.fence),r.last.map(|w|w.fence));
        let start=a.y as usize*8*4;let len=a.height as usize*8*4;
        stage.write(0,&r.image[start..start+len]);
    });
    Ok(Seen{unwritten:0})
}
enum Chunks{Virgl(Vec<()>),Svga(Vec<()>),Gk208(Vec<gk208::encode::Chunk>)}
mod wire{pub mod format{pub const B8G8R8A8_UNORM:u32=1;}}
mod encode {
    use super::*;
    pub struct TargetRes{pub id:u32,pub format:u32,pub info:TargetInfo}
    pub struct SourceRes{pub id:u32,pub format:u32,pub width:u32,pub height:u32}
    pub trait Bindings{fn target(&self,h:u64)->Option<TargetRes>;fn source(&self,h:u64)->Option<SourceRes>;fn buffer_rect(&self,h:u64,o:u64)->[i32;4];fn buffer_word(&self,h:u64,o:u64)->u32;}
    pub struct Enc;
    impl Enc{pub fn new()->Self{Self}pub fn finish(self)->Option<Vec<()>>{Some(vec![()])}}
    pub struct Extras{pub vertex_buffer:u32,pub scratch:[u32;2],pub data:u32,pub data_width:u32,pub flip_y:bool,pub corrupt:Option<u64>,pub suppress:Option<u64>,pub swap:Option<(usize,u64)>}
    pub fn translate(_: &mut Enc,_:&mut Objects,_:&[u32],_:&impl Bindings,_:Extras){}
}
mod svga {pub mod encode{
    use crate::*;
    pub struct Extras{pub scratch:[[u32;2];2],pub data_width:u32,pub corrupt:Option<u64>,pub suppress:Option<u64>,pub swap:Option<(usize,u64)>}
    pub struct Out{pub sizes:Vec<usize>,pub chunks:Vec<()>}
    pub struct Backing;
    impl Backing{pub fn allocate(_: &[usize])->Option<Self>{Some(Self)}}
    pub fn translate(_: &mut Dx,_:&[u32],_:&[u32],_:&impl crate::encode::Bindings,_:Extras,_:Option<Backing>,_:Option<u64>)->Option<(Out,())>{Some((Out{sizes:vec![],chunks:vec![]},()))}
}}
mod syscall {
    pub mod window{pub fn copy_out<T>(_:u64,_:&T)->Result<(),i32>{Ok(())}}
    pub mod userptr{pub fn write_user_bytes(dst:u64,src:*const u8,n:usize)->Result<(),()>{
        let bytes=unsafe{std::slice::from_raw_parts(src,n)};
        crate::USER.with(|u|u.borrow_mut()[dst as usize..dst as usize+n].copy_from_slice(bytes));Ok(())
    }}
}
mod tracing{pub mod providers{pub mod window{pub fn count_damage_rects(_:u64){}}}}
mod test_control{pub fn take(point:u32,_:u64)->Option<()> {
    (point==breengel_abi::tc_point::CAPTURE_STALL && crate::CAPTURE_ACTION.with(|a|a.borrow().is_some())).then_some(())
}}
mod gpu {pub mod window_targets {
    use crate::*;
    // WRITER
    pub struct Context{pub owner:u64,pub id:u32}
    impl Context{fn get(owner:u64)->Result<Arc<Self>,i64>{Ok(Arc::new(Self{owner,id:1}))}}
    pub struct Target{pub context:Arc<Context>,pub id:u32,pub handle:u64,pub width:u32,pub height:u32}
    impl Target{
        // TARGET_METHODS
    }
    // BINDINGS
}}
use gpu::window_targets::Target;
struct Lock<T>(Mutex<T>);
impl<T> Lock<T>{fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}}
#[derive(Default)] struct Queue;
impl Queue{fn wake_up(&self){}}
mod window {
    use crate::*;
    use breengel_abi::{limits,Meta,errno::{ENOENT,EPERM}};
    type WaitQueueHead=Queue;
    #[derive(Default)] pub struct Caller;
    // WINDOW_RECORDS
    pub struct Window{pub id:u64,pub buffers:Vec<Buffer>,pub tree_pins:[u32;3],pub tree:Arc<mem::Mem>,pub tree_addr:u64,pub tree_kib:u32,
        pub width:u32,pub height:u32,pub front:Option<usize>,pub front_commit:u64,pub commit_id:u64,pub accepted_id:u64,
        pub pixel_gen:u64,pub accepted_pixel_gen:u64,pub pending:Option<Pending>,pub gate:bool,pub meta:Meta,
        pub acquire_error:bool,pub failed_publications:u64,pub waits:Arc<Waits>,pub log:VecDeque<(u64,Damage)>,
        pub log_floor:u64,pub present:Present}
    impl Window{fn buffer_mut(&mut self,uid:u64)->Option<&mut Buffer>{self.buffers.iter_mut().find(|b|b.uid==uid)} fn tree_len(&self)->u64{self.tree_kib as u64*1024}}
    pub struct Present{pub pending:Damage}
    pub struct Registry{pub windows:BTreeMap<u64,Window>}
    impl Registry{fn window(&mut self,id:u64,_:&Caller,allow:fn(bool,bool,&Caller)->bool)->Result<&mut Window,i32>{
        if !allow(true,false,&Caller){return Err(EPERM);} self.windows.get_mut(&id).ok_or(ENOENT)
    }}
    static REGISTRY:Lock<Registry>=Lock(Mutex::new(Registry{windows:BTreeMap::new()}));
    mod who{use super::Caller;pub fn owner(o:bool,_:bool,_:&Caller)->bool{o}pub fn owner_compositor_or_root(_:bool,_:bool,_:&Caller)->bool{true}}
    pub mod mem {
        pub struct Mem(pub Vec<u8>);
        impl Mem{pub fn read(&self,at:u64,b:&mut[u8]){b.copy_from_slice(&self.0[at as usize..at as usize+b.len()]);}}
        pub fn protect_range(_: &Mem,_:u64,_:u64,_:u64,write:bool)->Result<(),i32>{if write{crate::event("tree-unseal");}Ok(())}
        pub fn probe_writable(_:u64,_:u64)->Result<(),i32>{Ok(())}
        pub fn copy_out(dst:u64,b:&[u8])->Result<(),i32>{crate::USER.with(|u|u.borrow_mut()[dst as usize..dst as usize+b.len()].copy_from_slice(b));Ok(())}
    }
    pub mod scene{pub fn changed(){}}
    pub mod present {
        use super::*;
        // PIN
    }
    pub mod ops {
        use super::*;
        use breengel_abi::{Acquire,Capture,Commit,Meta,errno::{EBUSY,EFAULT,EINTR,EINVAL,ENOENT,ENOMEM,ETIMEDOUT}};
        use super::present::Pin;
        use mem::Mem;
        enum IpcDeadline{Never,Monotonic(u64)}
        enum WaitEnd{Woken,TimedOut,Interrupted(())}
        fn now_ns()->u64{0}
        fn wait_on(_: &Queue,_:&IpcDeadline,pending:impl FnOnce()->bool)->WaitEnd{
            if pending(){event("pool-wait");WaitEnd::TimedOut}else{WaitEnd::Woken}
        }
        fn stall(){let action=CAPTURE_ACTION.with(|a|a.borrow_mut().take());if let Some(action)=action{action();}}
        // OPS
    }
    pub fn reset(){REGISTRY.lock().windows.clear();}
    pub fn install(){
        let targets:[Arc<Target>;2]=[1,2].map(|id|Arc::new(Target{context:Arc::new(gpu::window_targets::Context{owner:7,id:1}),id,handle:100+u64::from(id),width:8,height:8}));
        let buffers=targets.into_iter().enumerate().map(|(i,mem)|Buffer{uid:i as u64+10,write_incarnation:1,
            draining:false,mem,addr:101+i as u64,state:if i==0{BufState::Front}else{BufState::Free},
            acquired:false,replaying:false,committing:false,base:if i==0{1}else{0},pins:0}).collect();
        REGISTRY.lock().windows.insert(1,Window{id:1,buffers,tree_pins:[0;3],tree:Arc::new(mem::Mem(vec![0x71;2048])),tree_addr:0x10000,tree_kib:1,
            width:8,height:8,front:Some(0),front_commit:1,commit_id:1,accepted_id:1,pixel_gen:1,accepted_pixel_gen:1,pending:None,gate:false,meta:Meta::ZERO,
            acquire_error:false,failed_publications:0,waits:Arc::new(Waits{acquire:Queue,events:Arc::new(Queue),gate:Queue}),
            log:VecDeque::new(),log_floor:0,present:Present{pending:Damage::none()}});
    }
    pub fn front()->u64{REGISTRY.lock().windows[&1].front_commit}
    pub fn target(i:usize)->Arc<Target>{REGISTRY.lock().windows[&1].buffers[i].mem.clone()}
    pub fn pin()->present::Pin{let mut r=REGISTRY.lock();let w=r.windows.get_mut(&1).unwrap();let b=&mut w.buffers[w.front.unwrap()];b.pins+=1;present::Pin::new(1,b.uid,b.mem.clone())}
    pub fn commit_candidate()->Result<(),i32>{
        REGISTRY.lock().windows.get_mut(&1).unwrap().buffers[1].acquired=true;
        let mut c=breengel_abi::Commit::ZERO;c.id=1;c.buffer=1;c.damage_count=1;c.damage[0]=Rect::new(0,0,8,8);
        ops::commit(&Caller,&mut c,Meta::ZERO,8192).map(|_|())
    }
    pub fn acquire()->Result<breengel_abi::Acquire,i32>{let mut a=breengel_abi::Acquire::ZERO;a.id=1;a.buffer=99;
        ops::acquire(&Caller,&mut a)?;Ok(a)}
    pub fn capture()->breengel_abi::Capture{let mut c=breengel_abi::Capture::ZERO;c.id=1;c.dst=0;c.dst_len=256;c.tree_dst=1024;c.tree_len=1024;
        assert_eq!(ops::capture(&Caller,&mut c),Ok(0));c}
    pub fn remove(){REGISTRY.lock().windows.remove(&1);}
    pub fn replace_pool(){
        let old=REGISTRY.lock().windows.remove(&1).unwrap();let pins=old.tree_pins;drop(old);install();
        let mut r=REGISTRY.lock();let w=r.windows.get_mut(&1).unwrap();w.tree_pins=pins;w.tree=Arc::new(mem::Mem(vec![0x88;2048]));w.front_commit=9;
    }
    pub fn tree_pins()->[u32;3]{REGISTRY.lock().windows[&1].tree_pins}
    pub fn len()->usize{REGISTRY.lock().windows[&1].buffers.len()}
}
fn setup(){
    INTERRUPT_COPY.with(|f|*f.borrow_mut()=false);
    window::reset();USER.with(|u|u.borrow_mut().fill(0));EVENTS.with(|e|e.borrow_mut().clear());FAIL_COPY.with(|f|*f.borrow_mut()=false);
    with_core(|c|{*c=Core::default();c.contexts.insert(1,Ctx{state:CtxState::Live,dropped:false,busy:false,inflight:vec![],objects:Objects,dx:Dx,vb:3,scratch:4,scratch_texture:5});
        for id in [1,2]{let mut image=vec![id as u8*0x11;256];for p in image.chunks_exact_mut(4){p[3]=0x13;}
            c.resources.insert(id,Resource{lost:false,invalid:false,window_sealed:id==1,reading:false,writers:0,accepted:vec![],epoch:1,generation:1,last:None,
                backing:Arc::new(Pages::new(1).unwrap()),pitch:64,image});}
    });window::install();
}
fn writer(id:u32)->Arc<gpu::window_targets::Writer>{let w=Arc::new(gpu::window_targets::Writer::new(1,1,19,119));
    with_core(|c|{let r=c.resources.get_mut(&id).unwrap();r.accepted.push(w.clone());r.writers=1;});w}
fn end(id:u32,w:&gpu::window_targets::Writer){w.end_access();with_core(|c|{c.resources.get_mut(&id).unwrap().writers=0;});event("source-fence-ended");}
#[test] fn pending_native_render_preserves_front_until_ok_and_access_end(){
    setup();let w=writer(2);assert_eq!(window::commit_candidate(),Ok(()));assert_eq!(window::front(),1);
    w.publish(fence_status::OK);window::ops::publish_ready();assert_eq!(window::front(),1);
    end(2,&w);window::ops::publish_ready();assert_eq!(window::front(),2);
    assert!(with_core(|c|c.resources[&2].window_sealed).unwrap());
}
#[test] fn late_failure_is_one_eio_and_never_reuses_live_candidate(){
    setup();let w=writer(2);assert_eq!(window::commit_candidate(),Ok(()));w.publish(fence_status::DEVICE_ERROR);
    with_core(|c|c.resources.get_mut(&2).unwrap().invalid=true);window::ops::publish_ready();assert_eq!(window::front(),1);
    assert_eq!(window::acquire().err(),Some(5));assert_eq!(window::acquire().err(),Some(breengel_abi::errno::ETIMEDOUT));
    end(2,&w);let a=window::acquire().unwrap();assert_eq!(a.base,1);assert_eq!(a.buffer,1);
}
#[test] fn sampled_old_front_blocks_two_buffer_pool_then_replays_on_native_queue(){
    setup();let pin=window::pin();assert_eq!(window::commit_candidate(),Ok(()));assert_eq!(window::front(),2);
    assert_eq!(window::acquire().err(),Some(breengel_abi::errno::ETIMEDOUT));assert_eq!(window::len(),2);
    assert!(with_core(|c|c.resources[&1].window_sealed && c.resources[&2].window_sealed).unwrap());
    assert!(EVENTS.with(|e|!e.borrow().contains(&"native-submit")));drop(pin);
    let a=window::acquire().unwrap();assert_eq!((a.buffer,a.base,a.replay_full),(0,2,1));
    with_core(|c|assert_eq!(c.resources[&1].image,c.resources[&2].image));
    EVENTS.with(|e|{let e=e.borrow();assert!(e.iter().position(|x|*x=="gpu-copy-complete").unwrap()<e.iter().position(|x|*x=="tree-unseal").unwrap());assert!(!e.contains(&"virtual-submit"));});
}
#[test] fn replay_waits_for_source_writer_and_refuses_failed_copy(){
    setup();let source=window::target(0);let dest=window::target(1);let w=writer(1);w.publish(fence_status::OK);
    PROGRESS.with(|p|*p.borrow_mut()=Some(Box::new(move||{assert!(EVENTS.with(|e|!e.borrow().contains(&"native-submit")));end(1,&w)})));
    assert_eq!(dest.copy_from(&source,&[Rect::new(1,1,3,5)]),Ok(()));
    EVENTS.with(|e|{let e=e.borrow();assert!(e.iter().position(|x|*x=="source-fence-ended").unwrap()<e.iter().position(|x|*x=="native-submit").unwrap());});
    FAIL_COPY.with(|f|*f.borrow_mut()=true);assert_eq!(window::acquire().err(),Some(5));
    assert!(with_core(|c|c.resources[&2].window_sealed).unwrap());
}
#[test] fn capture_keeps_whole_old_pixels_and_tree_across_resize_or_destroy(){
    for resize in [true,false]{setup();
        // read_to's pause callback models a lifecycle transition between rows.
        let target=window::target(0);let pin=window::pin();
        target.read_to(0,Some(4),||{assert_eq!(window::tree_pins(),[0;3]);if resize{window::replace_pool()}else{window::remove()}}).unwrap();
        USER.with(|u|for pixel in u.borrow()[..256].chunks_exact(4){assert_eq!(pixel,&[0x11,0x11,0x11,255]);});drop(pin);
        setup();CAPTURE_ACTION.with(|a|*a.borrow_mut()=Some(Box::new(move||{assert_eq!(window::tree_pins(),[1,0,0]);if resize{window::replace_pool()}else{window::remove()}})));
        // Point the production read_to pause at the modeled capture control.
        let c=window::capture();assert_eq!((c.commit_id,c.width,c.height),(1,8,8));
        USER.with(|u|{let u=u.borrow();for pixel in u[..256].chunks_exact(4){assert_eq!(pixel,&[0x11,0x11,0x11,255]);}assert!(u[1024..2048].iter().all(|b|*b==0x71));});
        if resize{assert_eq!(window::tree_pins(),[0;3]);}
    }
}
#[test] fn private_clear_uses_native_draw_release(){setup();assert_eq!(window::target(1).clear(),Ok(()));assert!(EVENTS.with(|e|e.borrow().contains(&"gpu-clear-complete")));}

#[test] fn interrupted_replay_retains_source_exclusion_until_device_completion(){
    setup();let source=window::target(0);let dest=window::target(1);
    INTERRUPT_COPY.with(|f|*f.borrow_mut()=true);
    assert_eq!(dest.copy_from(&source,&[Rect::new(0,0,8,8)]),Err(EINTR as i32));
    assert_eq!(source.seal(false),Err(breengel_abi::errno::EBUSY),"source became writable under queued GPU copy");
    assert_eq!(dest.seal(false),Err(breengel_abi::errno::EBUSY),"destination became writable under queued GPU copy");
    assert!(!source.idle() && !dest.idle());
    with_core(|c|c.syncs.clear());
    release_other(Some(1));release_other(Some(2));
    assert_eq!(source.seal(false),Ok(()));
}
