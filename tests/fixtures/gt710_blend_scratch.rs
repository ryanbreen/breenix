// Production scratch allocation, registry, adapter and Sub retirement run here.
// Only device observations and the process quota reservation are substituted.
use std::{collections::BTreeMap, sync::{Arc, atomic::{AtomicU64, Ordering}}};
use gk208::{ce, resource::Resources, memory::allocator::Allocator, vm};
const ENOMEM: i64 = -12;
const EIO: i64 = -5;
#[derive(Default)]
pub struct Model { words: BTreeMap<u64,u64>, fail: bool }
impl vm::Io for Model {
    fn read(&mut self, at:u64)->Result<u64,&'static str>{Ok(*self.words.get(&at).unwrap_or(&0))}
    fn write(&mut self, at:u64,v:u64)->Result<(),&'static str>{self.words.insert(at,v);Ok(())}
    fn flush(&mut self)->Result<(),&'static str>{Ok(())}
    fn invalidate(&mut self,_:u64)->Result<u32,&'static str>{if self.fail {Err("invalidation failed")}else{Ok(0)}}
    fn register(&mut self,_:usize)->u32{0}
}
thread_local! { static OWNER: RefCell<(Resources<gpu::dma::Pages>,Allocator,Model)> =
    RefCell::new((Resources::new(),Allocator::new(1<<30,&[]).unwrap(),Model::default())); }
static CHARGED: AtomicU64=AtomicU64::new(0);
static RELEASES: AtomicU64=AtomicU64::new(0);
struct Charge { bytes:u64 }
impl Charge {
    fn new(tgid:u64,bytes:u64,copy:bool)->Result<Self,i64>{
        assert_eq!(tgid,42);assert!(copy);CHARGED.fetch_add(bytes,Ordering::SeqCst);Ok(Self{bytes})
    }
}
impl Drop for Charge {fn drop(&mut self){CHARGED.fetch_sub(self.bytes,Ordering::SeqCst);RELEASES.fetch_add(1,Ordering::SeqCst);}}
// OWNERSHIP
struct Writer;
impl Writer {fn end_access(&self){}}
struct Ranges;
impl Ranges {fn release(&mut self,_:u64){}}
struct Arena {ranges:Ranges}
struct Context {vertex_arena:Option<Arena>}
struct Sub {requests:u32,published:bool,held:Vec<u32>,ctx:u32,writer:Option<Writer>,_scratch:NativeScratch}
struct Core {subs:BTreeMap<u64,Sub>,contexts:BTreeMap<u32,Context>}
impl Core {
    fn unhold(&mut self,_:u32){panic!("scratch is native-owned")}
    // RETIRE
}
fn notify(){}
fn fresh()->u64 {
    OWNER.with(|o| *o.borrow_mut()=(Resources::new(),Allocator::new(1<<30,&[]).unwrap(),Model::default()));
    CHARGED.store(0,Ordering::SeqCst);RELEASES.store(0,Ordering::SeqCst);
    ce::with_resources(|_,a,_|Ok(a.stats().charged)).unwrap()
}
fn collect(){ce::with_resources(|r,a,io|r.collect(a,io)).unwrap();}
fn submit()->(Core,Device,Rc<RefCell<State>>,Vec<u32>,u64) {
    let baseline=fresh();
    let scratch=native_blend_scratch(42,[[48,17],[81,129]]).unwrap();
    let ids:Vec<_>=scratch.targets.iter().flatten().map(|t|t.id).collect();
    assert_eq!(ids.len(),2);
    for (slot,(w,h,bytes)) in [(48,17,8192),(81,129,98304)].into_iter().enumerate(){
        let t=scratch.targets[slot].unwrap();assert_eq!((t.width,t.height,t.format),(w,h,slot as u32));
        ce::with_resources(|r,_,_|{let l=r.layout(t.id).unwrap();assert_eq!(l.allocation_bytes(),bytes);assert_eq!(t.address,r.address(t.id).unwrap());Ok(())}).unwrap();
    }
    assert_eq!(CHARGED.load(Ordering::SeqCst),106496);
    let storage=gk208::encode::storage(&ids);
    let (mut d,s)=device();
    let draw=gk208::adapter::EncodedDraw { words:vec![0x20010367,0],target:19,transfer:None,_storage:storage };
    assert!(d.send(Request::draw(Tag::Sync(9001),9001,19,1,draw)).is_ok());
    let c=Core {subs:BTreeMap::from([(9,Sub{requests:1,published:false,held:vec![],ctx:1,writer:None,_scratch:scratch})]),contexts:BTreeMap::new()};
    (c,d,s,ids,baseline)
}
#[test]
fn incomplete_fence_and_publication_keep_each_formats_storage_and_charge() {
    let (mut c,mut d,s,ids,baseline)=submit();
    assert!(matches!(d.reap(),Reap::Empty));c.retire_if_done(9);collect();
    assert_eq!(RELEASES.load(Ordering::SeqCst),0);
    s.borrow_mut().release=1;assert!(matches!(d.reap(),Reap::Done(_)));
    c.subs.get_mut(&9).unwrap().requests=0;c.retire_if_done(9);collect();
    assert_eq!(RELEASES.load(Ordering::SeqCst),0,"completed but unpublished fence still holds scratch");
    c.subs.get_mut(&9).unwrap().published=true;c.retire_if_done(9);
    assert_eq!(RELEASES.load(Ordering::SeqCst),2);assert_eq!(CHARGED.load(Ordering::SeqCst),0);
    ce::with_resources(|r,a,_|{assert_eq!(a.stats().charged,baseline);for id in ids {assert!(r.address(id).is_none());}Ok(())}).unwrap();
    collect();assert_eq!(RELEASES.load(Ordering::SeqCst),2);
}
#[test]
fn failed_reset_or_unbind_releases_nothing_even_after_caller_leaves() {
    for unbind in [false,true] {
        let (mut c,mut d,s,_,_)=submit();
        s.borrow_mut().fail_reset=!unbind;s.borrow_mut().fail_unbind=unbind;
        d.begin_reset();assert!(!d.reset_done());let (old,result)=d.restart();
        assert!(old.is_empty()&&result.is_err());assert_eq!(d.outstanding(),1);
        // Even an abandoned core owner cannot defeat the pending device hold.
        c.subs.clear();collect();assert_eq!(RELEASES.load(Ordering::SeqCst),0);
        assert_eq!(CHARGED.load(Ordering::SeqCst),106496);
        // Dropping the harness device happens only after the assertions.
    }
}
#[test]
fn confirmed_reset_and_unbind_allow_exact_retirement() {
    let (mut c,mut d,_,_,baseline)=submit();d.begin_reset();assert!(d.reset_done());
    let (old,result)=d.restart();assert!(result.is_ok());drop(old);
    let sub=c.subs.get_mut(&9).unwrap();sub.requests=0;sub.published=true;c.retire_if_done(9);
    assert_eq!(RELEASES.load(Ordering::SeqCst),2);assert_eq!(CHARGED.load(Ordering::SeqCst),0);
    ce::with_resources(|_,a,_|{assert_eq!(a.stats().charged,baseline);Ok(())}).unwrap();
}
#[test]
fn failed_collection_keeps_the_process_charge_with_the_unreclaimed_allocation() {
    let (mut c,mut d,s,_,_)=submit();s.borrow_mut().release=1;assert!(matches!(d.reap(),Reap::Done(_)));
    ce::with_resources(|_,_,io|{io.fail=true;Ok(())}).unwrap();
    let sub=c.subs.get_mut(&9).unwrap();sub.requests=0;sub.published=true;c.retire_if_done(9);
    assert_eq!(RELEASES.load(Ordering::SeqCst),0);assert_eq!(CHARGED.load(Ordering::SeqCst),106496);
}
#[test]
fn failed_mapping_keeps_its_charge_and_unused_formats_allocate_nothing() {
    fresh();drop(native_blend_scratch(42,[[0,0];2]).unwrap());assert_eq!(RELEASES.load(Ordering::SeqCst),0);
    ce::with_resources(|_,_,io|{io.fail=true;Ok(())}).unwrap();
    assert!(native_blend_scratch(42,[[48,17],[0,0]]).is_err());
    assert_eq!(RELEASES.load(Ordering::SeqCst),0);assert_eq!(CHARGED.load(Ordering::SeqCst),8192);
}
