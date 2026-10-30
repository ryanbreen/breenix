#![allow(dead_code)] // Hardware stand-ins cover baseline and repaired restart dependencies.
// Hardware substitutes for the production restart and constructor FIFO writes.
use std::{cell::RefCell, collections::BTreeMap};
mod gpu {pub mod panic {pub struct Access;impl Access {pub fn enter()->Self{Self}}}}
const COMMAND: u64 = 0x1000000;
struct State { vram: BTreeMap<u64,u32>, health: Result<(u32,u64,bool), &'static str>, reasons: Vec<String>, accessible: bool, refuse_access: bool }
thread_local! { static STATE: RefCell<State> = RefCell::new(State {
    vram: BTreeMap::new(), health: Ok((0,0,false)), reasons: vec![], accessible:false, refuse_access:false }); }
struct Registers(usize);
impl Registers {
    fn mask(&mut self, _:usize, _:u32, _:u32) { assert_eq!(self.0, 7); }
    fn write(&mut self, _:usize, _:u32) {}
    fn read(&mut self, _:usize)->u32 { u32::MAX }
}
fn words(_: &mut Registers, _:u64, address:u64, values:&[u32])->Result<(), &'static str> {
    STATE.with(|s| { let mut s=s.borrow_mut(); for (i,v) in values.iter().enumerate() { s.vram.insert(address+i as u64*4,*v); } }); Ok(())
}
fn word(address:u64)->u32 { STATE.with(|s| *s.borrow().vram.get(&address).unwrap_or(&0)) }
mod modeset { pub fn bar_flush(_: &mut super::Registers)->Result<(), &'static str> { Ok(()) } }
mod topology { pub fn decode(_: &[[u32;2];3])->Result<(), &'static str> { Ok(()) } }
mod ctxctl {
    pub fn enable(_: &mut super::Registers)->Result<(), &'static str> {
        super::STATE.with(|s| {let mut s=s.borrow_mut();
            if s.refuse_access {Err("GR PRI inaccessible")} else {s.accessible=true;Ok(())}})
    }
    pub struct Controller;
    impl Controller {
        pub fn upload(_: &mut super::Registers, _: &(), _: [u64;2])->Result<Self, &'static str> { super::STATE.with(|s|assert!(s.borrow().accessible,"firmware upload before GR accessibility confirmation")); Ok(Self) }
        pub fn start(&mut self, _: &mut super::Registers)->Result<(), &'static str> { Ok(()) }
    }
}
fn hardware_error(e:&'static str)->&'static str { e }
struct Context;
impl Context { unsafe fn confirm_reset(&mut self) {} }
struct Range { start:u64 }
struct Allocation { range:Range }
#[derive(Clone,Copy)] struct Identity { instance:u64, epoch:u64, id:u32 }
struct Report { identity:Identity }
struct Copy { base:usize, reset:u32 }
struct Pages(Box<[u8;4096]>);
impl Pages { fn va(&self,_:usize)->u64 { self.0.as_ptr() as u64 } fn publish(&self) {} }
struct Session {
    second:Option<Box<Session>>, pristine:Vec<u8>,
    image:Allocation, commands:Allocation, userd:u64, report:Report, context:Context,
    admitted:bool, quiesced:bool, copy:Option<Copy>, next_sequence:u32, pages:Pages,
}
impl Session {
    fn activate_regs(&mut self, _: &mut Registers, _:u64)->Result<(), &'static str> {
        self.admitted=true; Ok(())
    }
}
struct NativeChannel {
    confirmed:bool, initial:Vec<u8>, session:Session, registers:usize, total:u64,
    topology:[[u32;2];3], mmu:[u64;2], put:u32,
}
struct Monitor<'a> {
    session:&'a Session, regs:&'a mut Registers, total:u64, sequence:u32,
    stop_on_trap:bool, retry_traps:bool, release_slot:usize, queued_put:Option<u32>,
}
impl Monitor<'_> {
    fn snapshot(&mut self)->Result<(u32,u64,bool), &'static str> {
        let _=(self.session,self.regs.0,self.total,self.sequence,self.stop_on_trap,self.retry_traps,self.release_slot,self.queued_put);
        STATE.with(|s| s.borrow().health)
    }
}
// INITIALIZER
impl NativeChannel {
    // RESTART
    // HEALTH
    fn record_fault(&self, error:&str) { STATE.with(|s|s.borrow_mut().reasons.push(error.into())); }
}
fn channel()->NativeChannel {
    NativeChannel { confirmed:true, initial:[5u32,0x1234,0,0].into_iter().flat_map(u32::to_le_bytes).collect(), registers:7, total:1<<30, topology:[[0;2];3], mmu:[0;2], put:6481,
        session:Session { second:None, pristine:vec![], image:Allocation {range:Range {start:0x400000}}, commands:Allocation {range:Range {start:0x100000}},
            userd:0x102200, report:Report {identity:Identity {instance:0x300000,epoch:17,id:1}}, context:Context,
            admitted:true, quiesced:true, copy:Some(Copy {base:0x106000,reset:22}), next_sequence:6482, pages:Pages(Box::new([85;4096])) } }
}
fn constructor(c:&NativeChannel)->Result<(), &'static str> {
    let mut regs=Registers(c.registers);
    let (instance,userd,commands,id)=(c.session.report.identity.instance,c.session.userd,c.session.commands.range.start,c.session.report.identity.id);
    let _=commands;
    // CONSTRUCT
    Ok(())
}
#[test] fn restart_rebuilds_ramfc_and_empty_ring() {
    let mut c=channel(); constructor(&c).unwrap();
    let instance=c.session.report.identity.instance;
    let expected:Vec<u32>=(0..0x200).step_by(4).map(|off|word(instance+off)).collect();
    assert_eq!(word(instance+0x10),0xface);
    // Stale GET, PUT, PB pointers and saved method state, plus old descriptors.
    STATE.with(|s| { let mut s=s.borrow_mut();
        for off in (0..0x200).step_by(4) {s.vram.insert(instance+off,0xdeadbeef);s.vram.insert(c.session.userd+off,0xdeadbeef);}
        for off in (0..4096).step_by(4) {s.vram.insert(c.session.commands.range.start+off,0xdeadbeef);}
        s.vram.insert(instance+0x200,0x12345001);s.vram.insert(instance+0x210,0x76543004);
        // FECS consumed the live image's MMIO header. Restart must use the
        // pristine golden template and restore its count (G19).
        s.vram.insert(c.session.image.range.start,0);
        s.vram.insert(c.session.image.range.start+4,0);
    });
    assert_eq!(c.restart(),Ok(()));
    assert_eq!(word(c.session.image.range.start),5);
    assert_eq!(word(c.session.image.range.start+4),0x1234);
    assert_eq!(word(instance+0x10),0xface,"restart must preserve the RAMFC signature");
    assert_eq!((0..0x200).step_by(4).map(|off|word(instance+off)).collect::<Vec<_>>(),expected);
    assert_eq!(word(instance+0x200),0x12345001);assert_eq!(word(instance+0x210),0x76543004);
    for off in (0..0x200).step_by(4) { assert_eq!(word(c.session.userd+off),0,"stale USERD at {off:x}"); }
    for off in (0..4096).step_by(4) { assert_eq!(word(c.session.commands.range.start+off),0,"stale GPFIFO descriptor"); }
    assert_eq!(word(instance+0x14),word(c.session.userd+0x8c),"GP_GET must match empty GP_PUT");
    assert_eq!(c.put,0);assert_eq!(c.session.next_sequence,1);assert_eq!(c.session.report.identity.epoch,18);
    assert!(c.session.pages.0.iter().all(|&b|b==0));
}
#[test] fn restart_returns_and_records_the_health_refusal() {
    let mut c=channel();
    STATE.with(|s|s.borrow_mut().health=Err("injected post-restart health failure"));
    assert_eq!(c.restart(),Err("injected post-restart health failure"));
    STATE.with(|s|assert_eq!(s.borrow().reasons,["injected post-restart health failure"]));
}

#[test] fn inaccessible_gr_forbids_firmware_upload_and_channel_restart() {
    let mut c=channel();
    STATE.with(|s| s.borrow_mut().refuse_access=true);
    assert_eq!(c.restart(),Err("GR PRI inaccessible"));
    assert_eq!(c.session.report.identity.epoch,17);
    assert!(c.session.quiesced);
    assert_eq!(c.put,6481);
}

#[test] fn reserved_terminal_channel_restarts_with_its_own_cold_image_and_receipts() {
    let mut c=channel();
    let mut terminal=channel().session;
    terminal.report.identity.id=1;
    terminal.report.identity.instance=0x600000;
    terminal.userd=0x102400;
    terminal.commands.range.start=0x700000;
    terminal.image.range.start=0x800000;
    terminal.pristine=[7u32,0x9876,0,0].into_iter().flat_map(u32::to_le_bytes).collect();
    c.session.second=Some(Box::new(terminal));
    assert_eq!(c.restart(),Ok(()));
    let t=c.session.second.as_ref().unwrap();
    assert_eq!(word(t.image.range.start),7);
    assert_eq!(word(t.image.range.start+4),0x9876);
    assert_eq!(word(t.report.identity.instance+0xe8),1);
    assert_eq!(t.report.identity.epoch,c.session.report.identity.epoch);
    assert!(t.pages.0.iter().all(|&b|b==0));
    for off in (0..0x200).step_by(4) {assert_eq!(word(t.userd+off),0);}
    for off in (0..4096).step_by(4) {assert_eq!(word(t.commands.range.start+off),0);}
}
