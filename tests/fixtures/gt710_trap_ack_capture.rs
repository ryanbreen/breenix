extern crate alloc;
use alloc::{format, string::String, vec::Vec};
use std::{cell::RefCell, collections::BTreeMap};
#[path="ROOT/kernel/src/gpu/gk208/gr/channel.rs"] pub mod channel;
#[path="ROOT/kernel/src/gpu/gk208/gr/shaders.rs"] pub mod shaders;
#[path="ROOT/kernel/src/gpu/gk208/gr/draw.rs"] pub mod draw;
#[path="ROOT/kernel/src/gpu/gk208/gr/binding.rs"] pub mod binding;
use channel::{Observation,receipt_nonce};
const BOUND_US:u64=2_000_000;
const TARGET:u64=0x600000;
const RUNLIST:u64=12288;
thread_local! {static LOG:RefCell<Vec<(usize,u32)>>=const{RefCell::new(Vec::new())};}
const SAVED:[(usize,u32);10]=[(0x2254,0x123),(0x2140,0x456),(0x204,7),(0x2630,0x14),(0x800004,0x10000),(0x200,0x1100),(0x4013c,1),(0x4010c,2),(0x4014c,3),(0x4012c,4)];
struct Registers {values:BTreeMap<usize,u32>,stuck:bool,reset_fails:bool}
impl Registers {
    fn read(&self,r:usize)->u32 {*self.values.get(&r).unwrap_or(&0)}
    fn write(&mut self,r:usize,v:u32) {
        LOG.with(|l|l.borrow_mut().push((r,v)));
        if [0x400100,0x400108].contains(&r) {self.values.insert(r,self.read(r)&!v);}
        else {self.values.insert(r,v);}
        if r==0x404000 && v==0xc0000000 {self.values.insert(r,0);}
    }
    fn mask(&mut self,r:usize,m:u32,v:u32) {self.write(r,(self.read(r)&!m)|v);}
}
// ADAPTER
mod ctxctl {
    pub fn reset(r:&mut super::Registers)->Result<(),&'static str> {
        r.write(0x200, r.read(0x200)&!0x1000);
        if r.reset_fails {return Err("reset failed");}
        r.stuck=false;r.values.insert(0x2640,0);r.values.insert(0x409b00,0);Ok(())
    }
}
fn hardware_error(s:&'static str)->&'static str{s}
fn register_wait(r:&mut Registers,a:usize,_:u32,_:u32,_:u64)->Result<(),&'static str>{
    r.write(0xf00000+a,0);
    if a==0x2634 && (r.stuck || r.read(0x400100)!=0 || r.read(0x400500)!=0x10001) {return Err("preempt timeout");}
    Ok(())
}
fn words(r:&mut Registers,_:u64,_:u64,_:&[u32])->Result<(),&'static str>{r.write(0xf00001,0);Ok(())}
mod modeset {pub fn activate_bar1(_:&mut super::Registers,_:u64,_:u64)->Result<(),&'static str>{Ok(())}}
#[derive(Clone,Copy)] struct Identity {epoch:u64,runlist:u32,id:u32,engine:u32}
struct Report {identity:Identity,binding:u32,runlist_count:u32}
struct Range {start:u64}
struct Allocation {range:Range}
struct DrawCopy {base:usize,engine:u32,reset:u32,old_irq:u32,old_enable:u32}
struct Session {report:Report,next_sequence:u32,bound:bool,quiesced:bool,commands:Allocation,pbdma:usize,copy:Option<DrawCopy>,alias:bool,pte:u64,bar_inst:u64,bar_dir:u64,old:[u32;6],old_pbdma:[u32;4],trap:bool}
struct Hardware<'a> {regs:&'a mut Registers,graphics:Option<Session>}
impl Session {
    fn issue_words(&mut self,h:&mut Hardware<'_>,_:u32,_:u32,_:u32,_:u64,_:&[u32],_:bool)->Result<Observation,&'static str>{
        h.regs.values.extend([(0x400100,0x100000 | if self.trap {0x200000}else{0}), (0x400108,if self.trap {1}else{0}), (0x400110,4),
            (0x400704,0x21400),(0x400708,0xdeadbeef),(0x40070c,0x1234),(0x400500,0),(0x409b00,0x80123456),(0x404208,0xa197),(0x404000,0x80000001)]);
        Ok(Observation{value:85,epoch:1,consumed:false,elapsed_us:10,accepted:false})
    }
    // METHODS
}
fn hash(_:&[u8])->String {"00".repeat(32)}
fn hex(b:&[u8])->String {b.iter().map(|v|format!("{v:02x}")).collect()}
macro_rules! serial_println {($($arg:tt)*)=>{println!($($arg)*)};}
pub(crate) use serial_println;
mod bulk {pub fn used()->usize{0} pub fn line(_:String)->Result<(),&'static str>{Ok(())}}
// EVIDENCE
struct ShaderBuffer {va:u64}
fn draw_probe(h:&mut Hardware<'_>,s:&mut Session,_:u64,e:&mut DrawEvidence)->Result<(),&'static str>{
    let mut receipt=binding::Receipt{target:0,epoch:0,nonce:0,before:[0;4],after:[0;4],observation:None};
    let result=s.draw(h,draw::Draw{target_va:0x100000,pitch:256,width:64,height:64,vertex_va:0x200000,quarantine_va:0x400000,vertex_program:0x30,fragment_program:0x1b0,report_va:0,epoch:0,nonce:0},&mut receipt);
    e.draws.push(DrawCapture{name:"triangle",receipt,images:None});result
}
fn present_probe(_:&mut Hardware<'_>,_:&mut Session,_:&mut (),_:&mut DrawEvidence)->Result<(),&'static str>{panic!("present after failing draw")}
fn run(trap:bool,stuck:bool,reset_fails:bool){
    let _ = (ctxctl::reset as fn(&mut Registers)->Result<(), &'static str>, hardware_error as fn(&'static str)->&'static str);
    LOG.with(|l|l.borrow_mut().clear());
    let mut regs=Registers{values:SAVED.into_iter().collect(),stuck,reset_fails};regs.values.insert(0x800000,0x80000300);
    let mut hardware=Hardware{regs:&mut regs,graphics:None};
    let mut graphics=Session{report:Report{identity:Identity{epoch:1,runlist:0,id:0,engine:0},binding:0,runlist_count:1},next_sequence:7,bound:true,quiesced:false,commands:Allocation{range:Range{start:0x100000}},pbdma:0,copy:None,alias:true,pte:0,bar_inst:0,bar_dir:0,old:[0x123,0x456,7,0x14,0x10000,0x1100],old_pbdma:[1,2,3,4],trap};
    let shader_buffer=ShaderBuffer{va:0x300000};let evo=&mut ();
    // FLOW
    assert!(draw_result.is_err());assert!(draw_emit.is_ok());assert!(draw_evidence.target.is_none());
    let result=hardware.graphics.as_mut().unwrap().quiesce(hardware.regs,1<<24);
    let log=LOG.with(|l|l.borrow().clone());
    let pos=|a,v|log.iter().position(|p|*p==(a,v)).unwrap_or_else(||panic!("missing write {a:x}={v:x}: {log:x?}"));
    assert!(pos(0x400100,0x100000)<pos(0x400500,0x10001));
    if trap {assert!(pos(0x400100,0x100000)<pos(0x404000,0xc0000000));assert!(pos(0x404000,0xc0000000)<pos(0x400108,1));assert!(pos(0x400108,1)<pos(0x400100,0x200000));}
    assert!(pos(0x400500,0x10001)<pos(0x2634,0));
    if !stuck {
        assert!(!log.iter().any(|&(a,v)|a==0x200&&v&0x1000==0),"acknowledged FECS preempt needed a reset");
    }
    if reset_fails {
        assert!(result.is_err());
        // A failed reset may not free a live channel, but runlist removal,
        // unbind and restores must still be attempted for a stopped model.
    } else {assert!(result.is_ok(),"{result:?}");}
    assert_eq!(hardware.regs.read(0x800000),0,"channel 0 still bound");
    assert_eq!(hardware.regs.read(0x2274)&0xfffff,0);
    assert!(SAVED.iter().all(|&(a,v)|hardware.regs.read(a)==v),"saved state not restored");
    assert!(!hardware.graphics.as_ref().unwrap().bound);
    assert_eq!(log.iter().filter(|&&(a,_)|a==0x400500).count(),1,"duplicate trap acknowledgement");
}
#[test] fn data_error(){run(false,false,false)}
#[test] fn unit_trap(){run(true,false,false)}
#[test] fn preempt_timeout_resets_and_unbinds(){run(false,true,false)}
#[test] fn failed_reset_still_attempts_fifo_cleanup(){run(false,true,true)}
