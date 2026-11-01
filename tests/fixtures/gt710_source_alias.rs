#![allow(dead_code)] // Production records include fields exercised by other core tests.
extern crate alloc;
use std::{cell::{Cell, RefCell}, collections::{BTreeMap,VecDeque}, sync::Arc};
use gpu_abi as abi;
use abi::{fence_status,kind, format, pipeline, stream::{self, Resolver, TargetInfo, Words}};
const EINVAL:i64=22; const ENODEV:i64=19; const ENOMEM:i64=12; const EAGAIN:i64=11;
const OWN:u64=0xb610000000001000; const OUTPUT:u64=OWN+1; const ALIAS:u64=OWN+2;
const FOREIGN:u64=OWN+3;
#[allow(dead_code)] // The production encoder embeds the complete shader catalogue.
#[path="ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] mod shaders;
mod gk208 { pub(crate) use crate::shaders; #[path="ROOT/kernel/src/gpu/gk208/caps.rs"] pub mod caps; #[path="NATIVE"] pub mod encode; }
#[derive(Clone, Default)] struct Pages;
struct WordBuf(Vec<u32>);
impl Words for WordBuf {fn len(&self)->usize{self.0.len()} fn word(&self,i:usize)->u32{self.0[i]}}
struct Snapshot;
impl Snapshot {fn rect(&self,_:u64,_:u64)->[i32;4]{[0,0,4,4]} fn word(&self,_:u64,_:u64)->u32{0}}
struct Resolved<'a>{view:&'a View,snap:&'a Snapshot,native_scratch:Cell<[Option<gk208::encode::Target>;3]>}
#[derive(Default)] struct Proc{resources:BTreeMap<u64,u32>}
struct Res{kind:u32,format:u32,width:u32,height:u32,size:u64,invalid:bool,lost:bool,backing:Arc<Pages>}
#[derive(Default)] struct Context{vertex_arena:Option<Arena>}
struct Arena{ranges:Ranges} struct Ranges;
impl Ranges{fn release(&mut self,_:u64){}}
mod window_targets {#[derive(Default)] pub struct Writer; impl Writer{pub fn end_access(&self){}}}
mod window {pub mod present {pub struct Pin;}}
mod display {#[derive(Clone, Copy)] pub struct Presenting;}
struct Charge; struct NativeScratch;
struct Device; struct Request; struct SyncReq{failed:bool,why:u32}
mod dev{pub fn backend()->u32{6}}
// SUB
#[derive(Default)] struct Core{procs:BTreeMap<u64,Proc>,resources:BTreeMap<u32,Res>,subs:BTreeMap<u64,Sub>,
    contexts:BTreeMap<u32,Context>,backlog:VecDeque<(u64,Request)>,wedged:Option<(Device,Vec<Request>)>,
    resetting:bool,status:u32,reason:&'static str,syncs:BTreeMap<u64,SyncReq>,retired_pins:Vec<Vec<Arc<window::present::Pin>>>}
impl Core {
    // RES_OF
    // RETIRE
    // FAILED_RESET
    fn unhold(&mut self,_:u32){}
}
thread_local!{static CORE:RefCell<Core>=RefCell::new(Core::default());}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn notify(){}
// RESOLUTION
fn setup()->(View,Vec<Alias>){
    CORE.with(|c|{let mut c=c.borrow_mut();*c=Core::default();
        c.procs.insert(7,Proc{resources:BTreeMap::from([(OWN,19),(OUTPUT,20)])});
        c.procs.insert(8,Proc{resources:BTreeMap::from([(FOREIGN,23)])});
        for id in [19,20,23]{c.resources.insert(id,Res{kind:kind::TARGET,format:format::B8G8R8A8,
            width:64,height:64,size:0,invalid:false,lost:false,backing:Arc::new(Pages)});}
    });
    (with_core(|c|View::of(c,7)).unwrap(),vec![Alias{handle:ALIAS,id:23,/* ALIAS_PIN */}])
}
fn frame(source:u64,command:u32)->WordBuf{
    let mut w=Vec::new();stream::encode::begin(&mut w,OWN);
    match command {
        stream::cmd::COPY=>stream::encode::copy(&mut w,source,[0,0,4,4],OWN,[0,0]),
        stream::cmd::DRAW_SAMPLED=>stream::encode::draw_sampled(&mut w,pipeline::COPY,source,&[[0,0,4,4,0,0]]),
        _=>abi::draw::encode(&mut w,pipeline::COPY,0,source,0,0,&[abi::draw::Instance{rect:[0,0,4,4],params:[0;16]}]),
    }
    stream::encode::begin(&mut w,OUTPUT);stream::encode::copy(&mut w,OWN,[0,0,64,64],OUTPUT,[0,0]);WordBuf(w)
}
fn sampling(aliases:&[Alias],named_ids:&[u32])->Result<Vec<Arc<window::present::Pin>>,i64>{
    // SAMPLE_PREPARE
}
fn resolved(view:&View)->Resolved<'_>{Resolved{view,snap:&Snapshot,native_scratch:Cell::new([None;3])}}
#[test] fn only_op19_source_positions_reach_the_native_resource(){
    for command in [stream::cmd::COPY,stream::cmd::DRAW_SAMPLED,abi::draw::COMMAND]{
        let (mut view,aliases)=setup();view.add_aliases(&aliases).unwrap();let w=frame(ALIAS,command);
        stream::validate(&w,&resolved(&view)).unwrap();
        let scene=scene_of(&w,&view,OWN,OUTPUT,64,64).unwrap();assert_eq!(scene.sampled,vec![ALIAS]);
        let native=gk208::encode::translate(&w,&resolved(&view)).unwrap();
        assert_eq!(native.chunks[0].source,Some(23));assert_eq!(native.chunks[0].target,19);
        let (written,all,_,_)=named(&w,&view);assert!(all.contains(&23));assert!(!written.contains(&23));
        for tgid in [7,8]{let plain=with_core(|c|View::of(c,tgid)).unwrap();
            for h in [ALIAS,23,FOREIGN]{
                // Give the foreign process an owned destination too, so its source
                // refusal cannot be masked by an absent BEGIN target.
                let mut plain=View{entries:plain.entries.iter().map(|e|Entry{handle:e.handle,id:e.id,kind:e.kind,
                    format:e.format,width:e.width,height:e.height,size:e.size,invalid:e.invalid,
                    backing:e.backing.clone(),sample_only:e.sample_only}).collect()};
                if tgid==8 {let own=with_core(|c|View::of(c,7)).unwrap();plain.entries.extend(own.entries);plain.entries.sort_by_key(|e|e.handle);}
                if h==FOREIGN && tgid==8 {continue;}
                let r=stream::validate(&frame(h,command),&resolved(&plain)).err().unwrap();assert_eq!(r.error,4);
            }
        }
    }
}
#[test] fn aliases_never_resolve_as_writable_targets_or_geometry(){
    let (mut v,a)=setup();v.add_aliases(&a).unwrap();let r=resolved(&v);
    assert!(r.target(ALIAS).is_none());assert!(r.buffer_size(ALIAS).is_none());
    assert!(gk208::encode::Bindings::render_target(&r,ALIAS).is_none());
    for words in [vec![stream::cmd::BEGIN,4,ALIAS as u32,(ALIAS>>32)as u32],{
        let mut w=vec![];stream::encode::begin(&mut w,OWN);stream::encode::copy(&mut w,OWN,[0,0,4,4],ALIAS,[0,0]);w
    },{let mut w=vec![];stream::encode::begin(&mut w,OWN);stream::encode::draw_buffer(&mut w,pipeline::SOLID,0,ALIAS,0,1);w}, {
        let mut w=vec![];stream::encode::begin(&mut w,OWN);abi::draw::encode(&mut w,pipeline::COVERAGE_SPANS,0,0,ALIAS,0,
            &[abi::draw::Instance{rect:[0,0,4,4],params:[0;16]}]);w
    }]{assert_eq!(stream::validate(words.as_slice(),&r).err().unwrap().error,4);}
    // UPLOAD, PRESENT and READBACK resolve public handles through res_of,
    // independently of any attachment or op19 view.
    for h in [ALIAS,23,FOREIGN]{assert!(with_core(|c|c.res_of(7,h)).unwrap().is_none());}
}
#[test] fn sampled_pool_pins_end_only_after_requests_and_publication(){
    for (requests,published) in [(2,false),(2,true),(0,false),(0,true)]{
        let pin=Arc::new(window::present::Pin);let weak=Arc::downgrade(&pin);
        let pins=vec![pin.clone()];assert_eq!(pins.len(),1);
        let mut c=Core::default();c.subs.insert(9,Sub{requests,published,/* SAMPLE_INIT */..Sub::default()});
        // RELEASE_LOCAL
        drop(pin);
        if requests>0 {c.reset_failed(Device);assert!(c.wedged.is_some());}
        c.retire_if_done(9);
        if requests!=0 || !published {assert!(weak.upgrade().is_some(),"pool pin escaped while DMA or its unpublished fence is live");
            c.subs.get_mut(&9).unwrap().requests=0;c.subs.get_mut(&9).unwrap().published=true;c.retire_if_done(9);}
        // Deferred outside the core lock: a pin can drop its Target, re-enter
        // core resource retirement, and wake the window pool.
        c.retired_pins.clear();assert!(weak.upgrade().is_none());
    }
}
#[derive(Clone,PartialEq)] struct Key{uid:u64,incarnation:u64,pixel_gen:u64}
struct Saved{key:Key} struct Reservation{sources:Vec<Saved>,cursor:u64}
struct Registry{current:Vec<Key>,cursor:u64}
fn keys(reg:&Registry)->Result<Vec<Key>,i64>{Ok(reg.current.clone())}
fn snapshot_check(reg:&Registry,reservation:&Reservation)->Result<(),i64>{
    // STALE
    Ok(())
}
#[test] fn stale_uid_write_incarnation_generation_or_cursor_is_eagain(){
    for change in 0..4{let key=Key{uid:3,incarnation:5,pixel_gen:7};
        let reservation=Reservation{sources:vec![Saved{key:key.clone()}],cursor:0};
        let mut reg=Registry{current:vec![key],cursor:0};assert_eq!(snapshot_check(&reg,&reservation),Ok(()));
        match change{0=>reg.current[0].uid+=1,1=>reg.current[0].incarnation+=1,2=>reg.current[0].pixel_gen+=1,_=>reg.cursor+=1};
        assert_eq!(snapshot_check(&reg,&reservation),Err(EAGAIN));
    }
}

#[test] fn submission_preparation_keeps_only_sampled_alias_pins(){
    let (_,aliases)=setup();
    assert_eq!(sampling(&aliases,&[]).unwrap().len(),0);
    let held=sampling(&aliases,&[23]).unwrap();assert_eq!(held.len(),1);
    // Dropping the call-local alias vector still leaves the submission's exact pin.
    let weak=Arc::downgrade(&held[0]);drop(aliases);assert!(weak.upgrade().is_some());
    drop(held);assert!(weak.upgrade().is_none());
}
