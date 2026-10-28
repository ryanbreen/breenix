// Host substitutes for pre-live allocations and command DMA. Production slices
// own the live FIFO state and decide whether reset/reclamation is permitted.
extern crate alloc;
use std::{cell::RefCell, collections::BTreeMap, mem::ManuallyDrop};
#[derive(Default)]
struct Injection { target: String, reached: bool, calls: Vec<String>, resets: usize, releases: usize }
thread_local! { static INJECT: RefCell<Injection> = RefCell::new(Injection::default()); }
fn step(name: &str) -> Result<(), &'static str> {
    INJECT.with(|i| {
        let mut i = i.borrow_mut();
        let occurrence=i.calls.iter().filter(|n| n.as_str()==name).count()+1;
        i.calls.push(name.into());
        if (i.target == name || i.target == format!("{name}-{occurrence}")) && !i.reached { i.reached = true; Err("injected") } else { Ok(()) }
    })
}
const SAVED: [(usize, u32); 10] = [
    (0x2254, 0x12345), (0x2140, 0x10000001), (0x204, 7),
    (0x2630, 0x14), (0x800004, 0x10000), (0x200, 0x1000),
    (0x4013c, 0x10000100), (0x4010c, 0x1234), (0x4014c, 0x5678), (0x4012c, 0x4321),
];
struct Registers { values: BTreeMap<usize, u32>, vram: BTreeMap<u64, u32>, stuck: bool }
impl Registers {
    fn new() -> Self { Self { values: SAVED.into_iter().collect(), vram: BTreeMap::new(), stuck: false } }
    fn read(&self, a: usize) -> u32 { *self.values.get(&a).unwrap_or(&0) }
    fn write(&mut self, a: usize, v: u32) {
        if a == 0x800000 && v == 0 && self.read(a) != 0 {
            let failed = INJECT.with(|i| {
                let mut i = i.borrow_mut();
                if i.target == "failed-unbind" {
                    assert!(i.resets > 0, "unbind failure must follow confirmed GR reset");
                    i.reached = true;
                    true
                } else { false }
            });
            if failed { return; }
        }
        self.values.insert(a, v);
    }
    fn mask(&mut self, a: usize, m: u32, v: u32) { self.write(a, (self.read(a) & !m) | v); }
    fn restored(&self) -> bool {
        self.read(0x800000) == 0 && self.read(0x2274) & 0xfffff == 0 &&
        self.vram.get(&0x900000).copied().unwrap_or(0) == 0 &&
        self.vram.get(&0x900004).copied().unwrap_or(0) == 0 &&
        SAVED.iter().all(|&(a,v)| self.read(a) == v)
    }
}
fn register_wait(r: &mut Registers, a: usize, _: u32, _: u32, _: u64) -> Result<(), &'static str> {
    if r.stuck && a == 0x2634 {
        INJECT.with(|i| i.borrow_mut().calls.push("stuck-preempt".into()));
        return Err("stuck preempt");
    }
    step(&format!("wait-{a:x}-{}", r.read(0x2274) & 0xfffff))
}
fn words(r: &mut Registers, _: u64, a: u64, w: &[u32]) -> Result<(), &'static str> {
    step(if w.iter().any(|&v| v != 0) { "alias-install" } else { "alias-clear" })?;
    for (n,&v) in w.iter().enumerate() { r.vram.insert(a+n as u64*4,v); }
    Ok(())
}
mod modeset {
    use super::*;
    pub fn activate_bar1(_: &mut Registers, _: u64, _: u64) -> Result<(), &'static str> { step("bar1") }
    pub fn bar_flush(_: &mut Registers) -> Result<(), &'static str> { step("flush") }
}
mod system_dma { pub struct Pages; }
mod memory {
    pub mod allocator {
        #[derive(Clone, Copy)] pub struct Range { pub start: u64 }
        #[derive(Clone, Copy)] pub struct Allocation { pub range: Range }
        impl Allocation { pub fn at(start: u64) -> Self { Self { range: Range { start } } } }
        pub struct Allocator;
        impl Allocator {
            pub fn stats(&self) -> Stats { Stats { reserved: 0, charged: 0, free: 1<<24 } }
            pub fn release(&mut self, a: Allocation) -> Result<(), &'static str> {
                let _ = a;
                crate::INJECT.with(|i| { let mut i=i.borrow_mut(); assert!(i.resets>0); i.releases+=1; }); Ok(())
            }
        }
        pub struct Stats { pub reserved: u64, pub charged: u64, pub free: u64 }
    }
    pub struct Layout { pub allocator: allocator::Allocator }
}
use memory::allocator::Allocation;
struct VmIo<'a> { regs: &'a mut Registers, total: u64 }
struct Context;
impl Context {
    unsafe fn admit_device(&mut self) -> Result<(), &'static str> { step("admit") }
    unsafe fn confirm_reset(&mut self) { INJECT.with(|i| assert!(i.borrow().resets>0)); }
    fn retire(self, _: &mut memory::allocator::Allocator, io: &mut VmIo<'_>) -> Result<(), &'static str> {
        assert!(io.regs.restored()); assert!(io.total>0); Ok(())
    }
}
struct Device;
impl Device { fn enable_bus_master(&self) {} }
struct Console { pointer: usize }
mod arch_impl { pub mod x86_64 { pub mod timer { pub fn rdtsc() -> u64 { 42 } } } }
fn hardware_error(e: &'static str) -> &'static str { e }
fn context_error(e: &context::Error) { let _ = e.reason; }
fn interrupt_snapshot(_: &mut Registers, _: &Device, _: &str) {}
mod ctxctl {
    use super::*;
    pub struct SavedState;
    impl SavedState {
        pub fn stop(&self, r: &mut Registers) -> Result<(), &'static str> { reset(r) }
        pub fn restore(&self, _: &mut Registers) -> Result<(), &'static str> { Ok(()) }
    }
    pub fn reset(r: &mut Registers) -> Result<(), &'static str> {
        let failed = INJECT.with(|i| {
            let mut i = i.borrow_mut();
            if i.target == "failed-reset" {
                assert!(r.stuck, "reset failure must follow stuck preempt");
                assert!(i.calls.iter().any(|c| c == "stuck-preempt"));
                if !i.reached {
                    assert_ne!(r.read(0x2630)&2, 0);
                    assert_ne!(r.read(0x800004)&0x800, 0);
                }
                i.reached = true;
                true
            } else { false }
        });
        if failed { return Err("GR reset failed"); }
        if r.read(0x800000) != 0 {
            // This unit explicitly permits reset after bounded FECS preempt.
            assert!(r.stuck || INJECT.with(|i|i.borrow().target=="wait-2634-2"));
            assert_ne!(r.read(0x2630)&2,0,"runlist not disabled before fallback");
            assert_ne!(r.read(0x800004)&0x800,0,"channel not disabled before fallback");
            r.stuck=false;
        } else {
            assert!(r.restored(), "reset under unrestored FIFO: {:x?}, VRAM={:x?}", r.values, r.vram);
        }
        INJECT.with(|i| i.borrow_mut().resets+=1); Ok(())
    }
}
mod context {
    use super::*;
    pub struct Error { pub reason: &'static str }
    pub struct Golden;
    #[derive(Clone,Copy)] pub struct Buffer {pub va:u64}
    pub struct Spec {pub name:&'static str,pub bytes:u64,pub align:u64}
    pub trait Io {fn allocate(&mut self, spec:Spec)->Result<Buffer,Error>;}
    impl Io for Hardware<'_> {
        fn allocate(&mut self,spec:Spec)->Result<Buffer,Error> {
            let _=(spec.name,spec.bytes,spec.align); Ok(Buffer{va:0x10000000})
        }
    }
    pub fn generate(_: &mut Hardware<'_>, _: &(), _: u32) -> Result<Golden, Error> { Ok(Golden) }
}
pub(crate) struct Reader;
impl Reader {
    fn new(_: &mut Registers, _: &Device, _: &mut memory::Layout, _: &Console, _: &[(u64,Allocation)]) -> Result<Self, &'static str> {
        INJECT.with(|i| assert!(!i.borrow().calls.iter().any(|c| c == "graphics-new"),
            "CE reused the graphics channel ID before GR reset"));
        step("reader-new")?; Ok(Self)
    }
    fn quiesce(&mut self, _: &mut Registers, _: &memory::Layout) -> Result<(), &'static str> { Ok(()) }
    fn finish(self, _: &mut Registers, _: &mut memory::Layout) -> Result<(), &'static str> { Ok(()) } }
mod shaders {
    pub const SEGMENT_SIZE:usize=640;
    pub fn exercise_uploads(u:&mut ShaderUpload<'_, '_>)->Result<(),&'static str> {
        let _=(&u.buffer,&u.evidence);
        assert!(u.hardware.reader.is_some()); step("shader-upload")
    }
    use super::*;
}
#[derive(Default)] struct ShaderEvidence;
impl ShaderEvidence {
    fn emit(&self,_:u64,r:&binding::Receipt,error:Option<&str>)->Result<(),&'static str> {
        let _=(r.target,r.epoch,r.nonce,r.before,r.after,r.observation,error); Ok(())
    }
}
struct ShaderUpload<'a,'b>{hardware:&'a mut Hardware<'b>,buffer:context::Buffer,evidence:&'a mut ShaderEvidence}
mod binding {
    pub struct TrapCapture;
    pub fn capture_trap(_: &mut super::Registers)->Option<TrapCapture>{None}
    pub fn acknowledge_trap(_: &mut super::Registers,_:&TrapCapture){}

    pub const TRAP_REGISTERS:[usize;4]=[0x400100,0x400108,0x400110,0x400118];
    pub fn check_traps(w:[u32;4])->Result<(),&'static str>{if w==[0;4] {Ok(())} else {Err("trap")}}
    pub fn commands(_:u64,_:u64,_:u64,_:u32,_:u32)->Result<Vec<u32>,&'static str>{Ok(vec![])}
    pub struct Receipt {pub target:u64,pub epoch:u64,pub nonce:u32,pub before:[u32;4],pub after:[u32;4],pub observation:Option<super::channel::Observation>}
}
fn commands(_:u64,epoch:u64,_:u32,_:bool,_:u32)->Result<Vec<u32>,&'static str>{Ok(vec![epoch as u32,(epoch>>32) as u32])}
struct Controller { readback_sha256: [[u8;32];4] }
struct Hardware<'a> {
    regs: &'a mut Registers, layout: &'a mut memory::Layout, device: &'a Device, console: &'a Console,
    total: u64, context: Context, reader: Option<Reader>, graphics: Option<channel::Session>,
    backing: Vec<(u64, Allocation)>, runnable_header: Vec<u8>, controller: Controller,
}
impl Hardware<'_> {
    fn emit_images(&mut self) -> Result<(usize, usize), &'static str> { Ok((0,0)) }
    // FINISH
}
#[derive(Default)] struct DrawEvidence {trap_capture:Option<binding::TrapCapture>}
impl DrawEvidence { fn emit(&self,_:Option<&str>)->Result<(),&'static str>{let _=&self.trap_capture;Ok(())} }
fn present_probe(_: &mut Hardware<'_>,_: &mut channel::Session,_:(),_:&mut DrawEvidence)->Result<(),&'static str>{Ok(())}
fn contexts_probe(_: &mut Hardware<'_>,_: &mut channel::Session,_:u64)->Result<(),&'static str>{step("contexts-probe")}
fn draw_probe(_: &mut Hardware<'_>,_: &mut channel::Session,_:u64,_:&mut DrawEvidence)->Result<(),&'static str>{step("draw-probe")}
mod channel {
    pub(crate) struct DrawSession(Session);
    impl DrawSession {
        pub fn new(session: Session) -> Self { Self(session) }
        pub fn into_session(self) -> Session { self.0 }
        pub fn emit_core(&self) -> Result<(), &'static str> { Ok(()) }
    }
    impl core::ops::Deref for DrawSession { type Target = Session; fn deref(&self)->&Session { &self.0 } }
    impl core::ops::DerefMut for DrawSession { fn deref_mut(&mut self)->&mut Session { &mut self.0 } }

    struct DrawCopy { base:usize,engine:u32,reset:u32,old_irq:u32,old_enable:u32 }
    use super::*;
    const BOUND_US: u64 = 2_000_000;
    const USERD: u64 = 8192;
    const RUNLIST: u64 = 12288;
    const SLOT: u64 = (8<<20)-4096;
    const TARGET: u64 = 0x2000000;
    #[derive(Clone, Copy)] pub struct Observation { pub value:u32, pub epoch:u64, pub consumed:bool, pub elapsed_us:u64, pub accepted:bool }
    #[derive(Clone, Copy)] pub struct Identity { pub id:u32, pub epoch:u64, pub instance:u64, pub top:[u32;64], pub engine:u32, pub runlist:u32, pub bind:u32, pub state:u32, pub current_context:u32 }
    #[derive(Clone, Copy)] pub struct Release { pub target:u64, pub poison:u32, pub nonce:u32, pub observed:u32, pub epoch:u64, pub elapsed_us:u64 }
    #[derive(Clone, Copy)] pub struct Withheld { pub poison:u32, pub observation:Observation }
    #[derive(Clone, Copy)] pub struct Stale { pub nonce:u32, pub observation:Observation }
    #[derive(Clone, Copy)] pub struct Report { pub identity:Identity, pub releases:[Release;2], pub withheld:Withheld, pub stale:Stale, pub faults:[[u32;2];3], pub runlist_count:u32, pub binding:u32 }
    use context::Golden;
    // SESSION
    pub fn touch(r: Report) {
        // Read the captured evidence fields that the real formatter consumes.
        let c=r.identity; let _=(c.id,c.epoch,c.instance,c.top,c.engine,c.runlist,c.bind,c.state,c.current_context);
        for v in r.releases { let _=(v.target,v.poison,v.nonce,v.observed,v.epoch,v.elapsed_us); }
        let o=r.withheld.observation; let _=(r.withheld.poison,o.value,o.epoch,o.consumed,o.elapsed_us,o.accepted);
        let o=r.stale.observation; let _=(r.stale.nonce,o.value,o.epoch,o.consumed,o.elapsed_us,o.accepted,r.faults,r.runlist_count,r.binding);
    }
}
// CLEANUP
fn run(regs: &mut Registers, stuck: bool) -> Result<(), &'static str> {
    let mut storage=memory::Layout{allocator:memory::allocator::Allocator};
    let layout=&mut storage;
    let evo=(); let console=Console{pointer:0}; let owned_device=Device; let device=&owned_device;
    regs.stuck=stuck;
    let mut cleanup=ProbeCleanup{state:ctxctl::SavedState,scratch:vec![],finished:false,fifo_quiesced:false};
    let result=(|| {
    let mut hardware=Hardware{regs,layout,device:&device,console:&console,total:1<<24,context:Context,
        reader:None,graphics:None,backing:vec![],runnable_header:vec![],controller:Controller{readback_sha256:[[0;32];4]}};
    let topology=(); struct Ready {size:u32} let ready=Ready{size:1024};
    // FLOW
    // Keep the same error propagation boundary; report formatting is irrelevant.
    let _=(generated,artifact,runnable_bytes,firmware_hashes);
    if let Some(r)=graphics {channel::touch(r);}
    Ok(())
    })();
    // PROBE_FINISH
}
#[test]
fn live_errors_restore_fifo_alias_and_binding_before_reset() {
    for target in ["alias-install", "alias-clear", "bar1", "bar1-2", "flush", "admit", "wait-228c-2", "issue-1", "issue-2", "issue-3", "issue-4", "issue-5", "shader-upload", "draw-probe", "contexts-probe", "wait-2634-2", "wait-228c-0", "wait-3080-0", "wait-2640-0"] {
        INJECT.with(|i|*i.borrow_mut()=Injection{target:target.into(),..Injection::default()});
        let mut regs=Registers::new();
        let result=run(&mut regs,false);
        assert!(result.is_err(), "wrong recovery result: {target}");
        INJECT.with(|i|assert!(i.borrow().reached,"not reached: {target}"));
        assert!(regs.restored(),"{target}: FIFO={:x?}, VRAM={:x?}",regs.values,regs.vram);
        INJECT.with(|i|assert!(i.borrow().resets>0,"no reset after safe unwind: {target}"));
    }
}
#[test]
fn rejected_channel_receipts_prevent_shader_submission() {
    for sequence in [2, 3, 4] {
        INJECT.with(|i| *i.borrow_mut() = Injection { target: format!("reject-{sequence}"), ..Injection::default() });
        let mut regs = Registers::new();
        assert!(run(&mut regs, false).is_err());
        INJECT.with(|i| assert!(!i.borrow().calls.iter().any(|call| call == "issue-5")));
        assert!(regs.restored());
    }
}
#[test]
fn clean_channel_restores_and_stuck_preempt_resets_before_reclaim() {
    INJECT.with(|i|*i.borrow_mut()=Injection::default());
    let mut regs=Registers::new(); assert!(run(&mut regs,false).is_ok()); assert!(regs.restored());
    INJECT.with(|i|*i.borrow_mut()=Injection{target:"issue-1".into(),..Injection::default()});
    let mut regs=Registers::new(); assert_eq!(run(&mut regs,true),Err("injected"));
    INJECT.with(|i| { let i=i.borrow();assert!(i.resets>0);assert!(i.releases>0); });
    assert!(regs.restored());
}
#[test]
fn stuck_preempt_with_failed_reset_or_unbind_retains_backing() {
    for target in ["failed-reset", "failed-unbind"] {
        INJECT.with(|i| *i.borrow_mut() = Injection { target: target.into(), ..Injection::default() });
        let mut regs = Registers::new();
        assert!(run(&mut regs, true).is_err(), "accepted failed cleanup: {target}");
        INJECT.with(|i| {
            let i = i.borrow();
            assert!(i.reached, "failure not injected: {target}");
            assert!(i.calls.iter().any(|c| c == "stuck-preempt"));
            assert_eq!(i.releases, 0, "released backing after {target}");
            assert_eq!(i.resets > 0, target == "failed-unbind");
        });
    }
}
mod ce {
    use super::*;
    // CE_HELPER
    struct Engine { base:usize, pbdma:usize }
    fn discover(_: &mut Registers) -> Result<Engine,&'static str> { step("discover")?;Ok(Engine{base:0x104000,pbdma:0}) }
    const SLOT:u64=(8<<20)-4096;
    struct Session { registers:usize, work:Option<()> }
    impl Session {
        // CE_BEGIN
    }
    fn reader(regs:&mut Registers,layout:&memory::Layout) -> Result<(),&'static str> {
        // CE_READER
    }
    fn constructor(regs:&mut Registers,id:u32) -> Result<(),&'static str> {
        // CE_CONSTRUCTOR
    }
    #[test]
    fn bound_channels_are_refused_before_fifo_writes_or_construction() {
        for id in [0,1] {
            let mut regs=Registers::new();regs.write(0x800000+id*8,0x80000300);
            let before=regs.values.clone();
            let mut s=Session{registers:&mut regs as *mut _ as usize,work:None};
            assert!(s.begin().is_err(),"process CE replaced bound channel {id}");assert_eq!(regs.values,before);
            assert!(reader(&mut regs,&memory::Layout{allocator:memory::allocator::Allocator}).is_err());assert_eq!(regs.values,before);
            assert!(constructor(&mut regs,id as u32).is_err());assert_eq!(regs.values,before);
        }
    }
}

mod readback_unwind {
    use super::*;
    struct Engine {base:usize,reset:u32,run:u32,pbdma:usize}
    struct Channel;
    impl Channel {
        fn drain(&mut self,r:&mut Registers,_:&Engine) -> Result<(),&'static str> {
            step("reader-drain")?;r.write(0x800000,0);r.write(0x2274,1<<20);Ok(())
        }
    }
    // READER_QUIESCE
    #[test]
    fn borrowed_reader_cleanup_keeps_state_until_retry_restores_fifo() {
        for target in ["reader-drain","alias-clear","bar1"] {
            INJECT.with(|i|*i.borrow_mut()=Injection{target:target.into(),..Injection::default()});
            let mut regs=Registers::new();
            let old=[regs.read(0x2254),regs.read(0x2140),regs.read(0x204),regs.read(0x2630),regs.read(0x800004),0,regs.read(0x200)];
            regs.write(0x800000,0x80000300);regs.write(0x2140,0);
            regs.vram.insert(0x900000,0x1231);
            let mut reader=Reader{channel:Channel,engine:Engine{base:0x104000,reset:7,run:1,pbdma:0},bar:0,bar_inst:0,bar_dir:0,pte:0x900000,old,quiesced:false};
            let layout=memory::Layout{allocator:memory::allocator::Allocator};
            let _=(reader.bar,reader.engine.pbdma);
            assert!(reader.quiesce(&mut regs,&layout).is_err());assert!(!reader.quiesced);
            assert!(reader.quiesce(&mut regs,&layout).is_ok());assert!(reader.quiesced);
            assert!(regs.restored(),"{target}: {:x?}",regs.values);
        }
    }
}
