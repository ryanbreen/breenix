// Fake host IO executes production constructors and pair lifetime operations.
extern crate alloc;
use std::{collections::BTreeMap, mem::ManuallyDrop};
#[path="ROOT/kernel/src/gpu/gk208/gr/channel.rs"] mod channel;
use channel::{Identity, Observation, Release, Report, Stale, Withheld};
const BOUND_US:u64=2_000_000;
#[derive(Default)] struct Registers {
    values:BTreeMap<usize,u32>, vram:BTreeMap<u64,u32>, lists:Vec<Vec<u32>>,
    fail_at:Option<u64>,
}
impl Registers {
    fn read(&self,a:usize)->u32 {*self.values.get(&a).unwrap_or(&0)}
    fn write(&mut self,a:usize,v:u32) {
        if a==0x2274 {
            let base=u64::from(self.read(0x2270))<<12;
            self.lists.push((0..u64::from(v&0xfffff)*2).map(|i|self.vram[&(base+i*4)]).collect());
        }
        self.values.insert(a,v);
    }
    fn mask(&mut self,a:usize,m:u32,v:u32) {self.write(a,(self.read(a)&!m)|v);}
}
fn words(r:&mut Registers,_:u64,a:u64,w:&[u32])->Result<(),&'static str> {
    if r.fail_at==Some(a) {r.fail_at=None;return Err("injected image write");}
    for (i,&v) in w.iter().enumerate() {r.vram.insert(a+i as u64*4,v);}
    Ok(())
}
fn read(r:&mut Registers,_:u64,a:u64)->Result<u32,&'static str>{Ok(*r.vram.get(&a).unwrap_or(&0))}
fn register_wait(_: &mut Registers,_:usize,_:u32,_:u32,_:u64)->Result<(),&'static str>{Ok(())}
mod memory {
    pub mod allocator {
        use super::super::*;
        #[derive(Clone,Copy,Debug,PartialEq,Eq)] pub struct Range {pub start:u64,pub size:u64}
        #[derive(Clone,Copy,Debug,PartialEq,Eq)] pub struct Allocation {pub range:Range}
        pub enum Kind {Engine}
        pub struct Allocator {pub next:u64,pub live:BTreeMap<u64,u64>}
        impl Allocator {
            pub fn allocate(&mut self,n:u64,align:u64,_:Kind)->Result<Allocation,&'static str>{
                self.next=(self.next+align-1)&!(align-1);let start=self.next;self.next+=n;
                self.live.insert(start,n);Ok(Allocation{range:Range{start,size:n}})
            }
            pub fn release(&mut self,a:Allocation)->Result<(),&'static str>{assert_eq!(self.live.remove(&a.range.start),Some(a.range.size));Ok(())}
        }
    }
    pub struct Object {pub name:&'static str,pub range:allocator::Range}
    pub struct Layout {pub allocator:allocator::Allocator,pub objects:Vec<Object>}
}
use memory::allocator::{Allocation,Kind};
#[derive(Clone,Copy,Default)] struct Flags {privileged:bool,volatile:bool}
struct VmIo<'a>{regs:&'a mut Registers,total:u64}
struct Space {instance:Allocation}
struct Context {space:Space,maps:BTreeMap<u64,Allocation>,admitted:bool}
impl Context {
    fn new(a:&mut memory::allocator::Allocator,io:&mut VmIo<'_>)->Result<Self,&'static str>{
        assert!(io.total>0);let _=io.regs.read(0);
        Ok(Self{space:Space{instance:a.allocate(4096,4096,Kind::Engine)?},maps:BTreeMap::new(),admitted:false})
    }
    fn space(&self)->&Space {&self.space}
    fn map(&mut self,_:&mut memory::allocator::Allocator,_:&mut VmIo<'_>,va:u64,a:Allocation,f:Flags)->Result<(),&'static str>{
        assert!(f.volatile);let _=f.privileged;assert!(self.maps.insert(va,a).is_none());Ok(())
    }
    fn map_system(&mut self,_:&mut memory::allocator::Allocator,_:&mut VmIo<'_>,va:u64,pages:&[u64])->Result<(),&'static str>{assert_eq!(va,0x2000000);assert_eq!(pages.len(),17);Ok(())}
    unsafe fn admit_device(&mut self)->Result<(),&'static str>{assert!(!self.admitted);self.admitted=true;Ok(())}
    unsafe fn confirm_reset(&mut self){self.admitted=false;}
    fn retire(&mut self,a:&mut memory::allocator::Allocator,_:&mut VmIo<'_>)->Result<(),&'static str>{assert!(!self.admitted);self.maps.clear();a.release(self.space.instance)}
    fn cancel_unsubmitted(&mut self)->Result<(),&'static str>{assert!(!self.admitted);Ok(())}
}
mod system_dma {
    pub struct Pages;
    impl Pages {pub fn new(n:usize,_:&())->Result<Self,&'static str>{assert_eq!(n,17);Ok(Self)}pub fn address(&self,i:usize)->u64{0x80000000+i as u64*4096}}
    pub fn translation_status()->Result<(),&'static str>{Ok(())}
}
mod modeset {
    use super::*;
    pub fn bar1_view(_:u64,_:u64,_:u64,_:u32)->Result<(u64,usize,u64),&'static str>{Ok((0,0,0))}
    pub fn activate_bar1(_: &mut Registers,_:u64,_:u64)->Result<(),&'static str>{Ok(())}
    pub fn bar_flush(_: &mut Registers)->Result<(),&'static str>{Ok(())}
}
mod ctxctl {pub fn reset(_: &mut super::Registers)->Result<(),&'static str>{Ok(())}}
fn hardware_error(e:&'static str)->&'static str{e}
mod arch_impl {pub mod x86_64 {pub mod timer {pub fn rdtsc()->u64{42}}}}
struct Bar{size:u64} struct Device{bars:[Bar;2]}
impl Device {fn enable_bus_master(&self){}}
struct Console{pointer:usize,bytes:usize}
#[derive(Clone,Copy)] struct Buffer{va:u64}
struct Golden{images:[Vec<u8>;2],runnable:Vec<u8>,header:[u32;2],buffers:Vec<Buffer>,patches:Vec<[u32;2]>}
struct Hardware<'a>{regs:&'a mut Registers,total:u64,layout:&'a mut memory::Layout,device:&'a Device,console:&'a Console,backing:Vec<(u64,Allocation)>}
// STATE
impl Session {
    // METHODS
}
const SAVED:[(usize,u32);11]=[(0x2254,0x1234),(0x2140,0x10),(0x204,1),(0x2630,0x80),(0x800004,0x10000),(0x80000c,0x20000),(0x200,0x1000),(0x4013c,0x10000100),(0x4010c,0x1234),(0x4014c,0x5678),(0x4012c,0x4321)];
fn fixture(f:impl FnOnce(&mut Hardware<'_>,&Golden)) {
    let mut regs=Registers::default();for (a,v) in SAVED {regs.values.insert(a,v);}
    regs.values.insert(0x22700,0x80000032);regs.values.insert(0x22704,3);regs.values.insert(0x2390,1);
    let mut layout=memory::Layout{allocator:memory::allocator::Allocator{next:0x100000,live:BTreeMap::new()},objects:vec![memory::Object{name:"instance",range:memory::allocator::Range{start:0x80000,size:0x10000}}]};
    let mut backing=Vec::new();for va in [0x10000000,0x10010000,0x10020000,0x10030000] {backing.push((va,layout.allocator.allocate(4096,4096,Kind::Engine).unwrap()));}
    let golden=Golden{images:[vec![0;4096],vec![0;4096]],runnable:(0..4096).map(|i|(i%251) as u8).collect(),header:[2,0x100300],buffers:backing.iter().map(|(va,_)|Buffer{va:*va}).collect(),patches:vec![[0x408004,0x1234],[0x408008,0x5678]]};
    let device=Device{bars:[Bar{size:0},Bar{size:8<<20}]};let console=Console{pointer:0x10000000,bytes:4096};
    f(&mut Hardware{regs:&mut regs,total:1<<30,layout:&mut layout,device:&device,console:&console,backing},&golden);
}
fn assert_pair(s:&Session,h:&Hardware<'_>,g:&Golden){
    let b=s.second.as_ref().unwrap();assert_eq!((s.report.identity.id,b.report.identity.id),(0,1));
    assert_ne!(s.report.identity.instance,b.report.identity.instance);assert_ne!(s.commands,b.commands);assert_ne!(s.image,b.image);assert_ne!(s.mmio,b.mmio);
    assert_eq!(b.userd,s.userd+512);
    for c in [s,&**b] {
        let inst=c.report.identity.instance;
        assert_eq!(h.regs.vram[&(inst+8)] as u64,c.userd);assert_eq!(h.regs.vram[&(inst+0xe8)],c.report.identity.id);
        assert_eq!(h.regs.vram[&(inst+0x210)],0x3000004);
        let header=[h.regs.vram[&c.image.range.start],h.regs.vram[&(c.image.range.start+4)]];
        assert_eq!(header,[2,0x100300]);
        for off in (8..256).step_by(4) {assert_eq!(h.regs.vram[&(c.image.range.start+off)],0);}
        for off in (256..4096).step_by(4) {assert_eq!(h.regs.vram[&(c.image.range.start+off)],u32::from_le_bytes(g.runnable[off as usize..off as usize+4].try_into().unwrap()));}
        for (i,p) in g.patches.iter().enumerate(){for j in 0..2 {assert_eq!(h.regs.vram[&(c.mmio.range.start+i as u64*8+j as u64*4)],p[j]);}}
        assert_eq!(c.context.maps[&0x10030000],c.mmio);
        for &(va,a) in &h.backing[..3]{assert_eq!(c.context.maps[&va],a);}
        let _=(c.bar,c.next_sequence,c.report.releases,c.report.withheld,c.report.stale,c.preempt_error);
    }
}
#[test] fn two_sessions_publish_one_list_and_clear_both_slots(){fixture(|h,g|{
    let baseline=h.layout.allocator.live.clone();let mut s=Session::new(h,g,0x10000000).unwrap();
    s.open_second(h,g,0x10000000).unwrap();assert_pair(&s,h,g);
    s.activate(h).unwrap();assert_eq!(h.regs.lists,vec![vec![0,0,1,0]]);
    assert_eq!(h.regs.read(0x800000),0x80000000|(s.report.identity.instance>>12) as u32);
    assert_eq!(h.regs.read(0x800008),0x80000000|(s.second.as_ref().unwrap().report.identity.instance>>12) as u32);
    s.quiesce(h.regs,h.total).unwrap();assert!(h.regs.lists.last().unwrap().is_empty());
    assert_eq!(h.regs.read(0x800000),0);assert_eq!(h.regs.read(0x800008),0);
    for (a,v) in SAVED {assert_eq!(h.regs.read(a),v,"register {a:x}");}
    assert_eq!(h.regs.vram[&s.pte],0);assert_eq!(h.regs.vram[&(s.pte+4)],0);
    s.finish(h.regs,h.layout,h.total).unwrap();assert_eq!(h.layout.allocator.live,baseline);
});}
#[test] fn failed_second_open_is_atomic_and_first_can_teardown(){fixture(|h,g|{
    let baseline=h.layout.allocator.live.clone();let mut s=Session::new(h,g,0x10000000).unwrap();
    let first=h.layout.allocator.live.clone();
    // Context instance, ring, then private image: fail during the second image.
    h.regs.fail_at=Some(h.layout.allocator.next+4096+65536);
    assert!(s.open_second(h,g,0x10000000).is_err());assert!(h.regs.fail_at.is_none());
    assert!(s.second.is_none());assert_eq!(h.layout.allocator.live,first);assert!(h.regs.lists.is_empty());
    s.quiesce(h.regs,h.total).unwrap();s.finish(h.regs,h.layout,h.total).unwrap();assert_eq!(h.layout.allocator.live,baseline);
});}
