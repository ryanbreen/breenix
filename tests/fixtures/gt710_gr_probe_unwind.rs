// Host substitutes for fallible startup operations; reset/restore are production code.
extern crate alloc;
use std::{cell::RefCell, collections::BTreeMap};
#[path = "ROOT/libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;
pub mod block { pub mod install { pub use crate::sha256; } }
#[path = "ROOT/kernel/src/gpu/gk208/gr/firmware.rs"]
pub mod firmware;
#[path = "ROOT/kernel/src/gpu/gk208/gr/tables.rs"]
pub mod tables;
#[path = "ROOT/kernel/src/gpu/gk208/gr/topology.rs"]
pub mod topology;
#[path = "ROOT/kernel/src/gpu/gk208/gr/ctxctl.rs"]
pub mod real_ctxctl;
const POWER: [u32; 7] = [0x200, 0x10a78c, 0x20520, 0x20524, 0x20528, 0x2052c, 0x20530];
#[derive(Default)]
struct Injection { target: &'static str, counts: BTreeMap<&'static str, usize>, stopped: bool, releases: usize }
thread_local! { static INJECT: RefCell<Injection> = RefCell::new(Injection::default()); }
fn step(name: &'static str) -> Result<(), &'static str> {
    INJECT.with(|i| {
        let mut i = i.borrow_mut();
        let n = { let n = i.counts.entry(name).or_default(); *n += 1; *n };
        if i.target == name || i.target == format!("{name}-{n}") { Err("injected failure") } else { Ok(()) }
    })
}
#[macro_export]
macro_rules! serial_println { ($($a:tt)*) => { let _ = format!($($a)*); }; }
struct Registers { values: BTreeMap<u32, u32>, now: u64, stuck_stop: bool }
impl Registers {
    fn new() -> Self {
        Self { values: BTreeMap::from([
            (0x200, 0xe011216d), (0x10a78c, 0x44),
            (0x20520, 3), (0x20524, 0x1f), (0x20528, 3),
            (0x2052c, 0x7f03ff), (0x20530, 1),
            (0x409604, 0x10001), (0x502608, 0x40001), (0x500c30, 1),
        ]), now: 0, stuck_stop: false }
    }
    fn read(&mut self, r: usize) -> u32 { *self.values.get(&(r as u32)).unwrap_or(&0) }
    fn write(&mut self, r: usize, v: u32) {
        let r = r as u32;
        if r == 0x200 && v & 0x1000 == 0 {
            if self.stuck_stop { return; }
            INJECT.with(|i| i.borrow_mut().stopped = true);
            self.values.insert(0x409100, 0); self.values.insert(0x502100, 0);
        }
        let value = if (0x20520..=0x20530).contains(&r) { v & 0x7fffffff } else { v };
        self.values.insert(r, value);
    }
}
impl real_ctxctl::Access for Registers {
    fn read(&mut self, r: u32) -> u32 { self.read(r as usize) }
    fn write(&mut self, r: u32, v: u32) { self.write(r as usize, v); }
    fn now_us(&mut self) -> u64 { self.now += 100; self.now }
    fn relax(&mut self) {}
}
mod ctxctl {
    pub use crate::real_ctxctl::*;
    use crate::{step, Registers, topology::Topology, INJECT};
    fn fail(name: &'static str) -> Result<(), Fault> {
        step(name).map_err(|reason| Fault { reason, address: 0, value: 0, elapsed_us: 100 })
    }
    pub fn reset(r: &mut Registers) -> Result<(), Fault> {
        crate::real_ctxctl::reset(r)?;
        fail("reset")?;
        if INJECT.with(|i| i.borrow().target == "topology") { r.values.insert(0x500c30, 0xbadf1002); }
        Ok(())
    }
    pub struct Controller;
    impl Controller {
        pub fn upload(_: &mut Registers, _: &Topology<'_>, _: [u64; 2]) -> Result<Self, Fault> {
            fail("upload")?; Ok(Self)
        }
        pub fn withheld_start(&mut self, _: &mut Registers) -> Result<(), Fault> { fail("withheld") }
        pub fn start(&mut self, r: &mut Registers) -> Result<(), Fault> {
            r.write(0x409100, 2); r.write(0x502100, 2);
            INJECT.with(|i| i.borrow_mut().stopped = false);
            fail("start")
        }
        pub fn stale_reply(&mut self, r: &mut Registers, _: u32) -> Result<(), Fault> {
            if INJECT.with(|i| i.borrow().target == "stop-stuck") { r.stuck_stop = true; }
            fail("stale")
        }
    }
}
mod memory {
    use super::*;
    pub mod allocator {
        use super::*;
        pub enum Kind { Engine }
        #[derive(Clone, Copy)] pub struct Range { pub start: u64 }
        #[derive(Clone, Copy)] pub struct Allocation { pub range: Range }
        #[derive(Clone, Copy, Debug, PartialEq)] pub struct Stats {pub reserved:u64, pub charged:u64, pub free:u64}
        pub struct Allocator { pub live: Vec<Allocation> }
        impl Allocator {
            pub fn stats(&self) -> Stats { let charged=self.live.len() as u64*0x22000;Stats{reserved:0,charged,free:0x1000000-charged} }
            pub fn allocate(&mut self, _:u64, _:u64, _:Kind) -> Result<Allocation,&'static str> {
                step("allocate")?;
                let a=Allocation{range:Range{start:(self.live.len() as u64+1)*0x22000}};self.live.push(a);Ok(a)
            }
            pub fn release(&mut self,a:Allocation) -> Result<(),&'static str> {
                INJECT.with(|i| { let mut i=i.borrow_mut(); assert!(i.stopped,"scratch freed before firmware stopped");i.releases+=1; });
                let n=self.live.iter().position(|b|b.range.start==a.range.start).unwrap();self.live.remove(n);Ok(())
            }
        }
    }
    pub struct Layout {pub allocator:allocator::Allocator}
    pub struct Pramin<'a> { regs:&'a mut Registers }
    impl<'a> Pramin<'a> {
        pub fn new(regs:&'a mut Registers,_:u64)->Self {Self{regs}}
        pub fn write(&mut self,address:u64,_:u32)->Result<(),&'static str> {
            let _=self.regs.read(0x200);
            if address%0x22000==0 {step("clear")?;} Ok(())
        }
    }
}
mod drivers {pub mod pci {pub struct Device;}}
struct ConsoleSurface;
mod gr {
    use super::{memory, Registers, ctxctl, topology};
    use alloc::vec::Vec;
    fn probe_static_inputs()->Result<(),&'static str>{Ok(())}
    fn hardware_error(e:ctxctl::Fault)->&'static str {e.reason}
    PRODUCTION
}
#[test]
fn startup_errors_unwind_before_ce_can_run() {
    let mut failed=Vec::new();
    for target in ["reset-1","topology","allocate-1","clear-1","allocate-2","clear-2",
                   "upload-1","withheld","reset-2","upload-2","start","stale","late"] {
        INJECT.with(|i| *i.borrow_mut()=Injection{target,..Injection::default()});
        let mut regs=Registers::new();let initial=POWER.map(|r|regs.read(r as usize));
        let mut layout=memory::Layout{allocator:memory::allocator::Allocator{live:vec![]}};
        let result=gr::probe(&mut regs,&drivers::pci::Device,&mut layout,&ConsoleSurface);
        assert!(result.is_err(),"injection {target} was not reached");
        let after=POWER.map(|r|regs.read(r as usize));
        if after!=initial || !layout.allocator.live.is_empty() || regs.read(0x409100)!=0 || regs.read(0x502100)!=0 {
            failed.push(format!("{target}: power={after:x?} expected={initial:x?}, live={}, FECS={}, GPCCS={}",
                layout.allocator.live.len(),regs.read(0x409100),regs.read(0x502100)));
        }
    }
    assert!(failed.is_empty(),"{}",failed.join("\n"));
}
#[test]
fn unwind_failed_stop_retains_scratch() {
    INJECT.with(|i| *i.borrow_mut()=Injection{target:"stop-stuck",..Injection::default()});
    let mut regs=Registers::new();
    let mut layout=memory::Layout{allocator:memory::allocator::Allocator{live:vec![]}};
    assert!(gr::probe(&mut regs,&drivers::pci::Device,&mut layout,&ConsoleSurface).is_err());
    assert_eq!(layout.allocator.live.len(),2);
    INJECT.with(|i|assert_eq!(i.borrow().releases,0));
    // A later confirmed stop permits explicit reclamation of retained backing.
    regs.stuck_stop=false;regs.write(0x200,0xe011216d);
    for a in core::mem::take(&mut layout.allocator.live) {layout.allocator.live.push(a);layout.allocator.release(a).unwrap();}
}
