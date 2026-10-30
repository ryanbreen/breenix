"""Run native reset ordering with owned and foreign latched MMU faults."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
class NativeResetFault(unittest.TestCase):
    def test_acknowledges_only_owned_faults_after_both_engines_stop(self):
        reset=item((ROOT/'kernel/src/gpu/gk208/gr/channel.rs').read_text(),
                   "        pub(crate) fn reset(&mut self)")
        reset=reset.replace('super::super::ctxctl','ctxctl').replace('super::super::hardware_error','hardware_error')
        source=r'''
use std::cell::RefCell;
const RUNLIST:u64=12288; const BOUND_US:u64=100000;
struct State {mask:u32,blocked:u32,fail_wait:usize,fail_ce:bool,fail_quiesce:bool,fail_gr:bool,fail_preempt:bool,fail_unbind:bool,writes:Vec<(usize,u32)>,events:Vec<&'static str>}
thread_local! {static STATE:RefCell<State>=RefCell::new(State{mask:0,blocked:0x10,fail_wait:0,fail_ce:false,fail_quiesce:false,fail_gr:false,fail_preempt:false,fail_unbind:false,writes:vec![],events:vec![]});}
struct Registers(usize);
impl Registers {
 fn mask(&mut self,r:usize,m:u32,v:u32){let _=self.0;self.write(r,(self.read(r)&!m)|v);}
 fn read(&self,r:usize)->u32{if r==0x2630{STATE.with(|s|s.borrow().blocked)}else if r==0x259c{STATE.with(|s|s.borrow().mask)}else if r==0x800000{STATE.with(|s|u32::from(s.borrow().fail_unbind))}else if r==0x800008{STATE.with(|s|u32::from(s.borrow().fail_wait==r))}else{0}}
 fn write(&mut self,r:usize,v:u32){STATE.with(|s|s.borrow_mut().writes.push((r,v)));if r==0x2630{STATE.with(|s|s.borrow_mut().blocked=v);}if r==0x259c{STATE.with(|s|{let mut s=s.borrow_mut();s.events.push("ack");s.mask&=!v;});}}
}
fn register_wait(_: &mut Registers,r:usize,_:u32,_:u32,_:u64)->Result<(),&'static str>{
 if STATE.with(|s|s.borrow().fail_wait==r || r==0x2634 && s.borrow().fail_preempt){Err("preempt refused")}else{Ok(())}
}
struct VmIo<'a>{regs:&'a mut Registers,total:u64}
mod gpu {pub mod panic {pub struct Access;impl Access {pub fn enter()->Self{Self}}pub fn native_reset(){crate::STATE.with(|s|s.borrow_mut().events.push("invalidate"));}}}
mod gk208 {
 pub mod ce {pub fn reset_engine(_: &mut crate::Registers,_:usize,_:u32)->Result<(),&'static str>{crate::STATE.with(|s|s.borrow_mut().events.push("ce-stop"));if crate::STATE.with(|s|s.borrow().fail_ce){Err("CE refused")}else{Ok(())}}}
 pub mod vm {
  pub struct Fault {pub unit:u8,pub instance:u64}
  pub fn faults(io:&mut crate::VmIo<'_>)->(u32,Vec<Fault>){
   let mask=io.regs.read(0x259c);assert_eq!(io.total,1<<30);
   let mut faults=Vec::new();
   for (unit,instance) in [(0,0x123000),(1,0x999000),(0x15,0x123000)] {
    if mask&(1<<unit)!=0{faults.push(Fault{unit,instance});}
   }
   (mask,faults)
  }
 }
}
mod ctxctl {pub fn reset(_: &mut crate::Registers)->Result<(),&'static str>{crate::STATE.with(|s|{let mut s=s.borrow_mut();s.events.push("gr-stop");if s.fail_gr{Err("GR reset refused")}else{Ok(())}})}}
fn hardware_error(e:&'static str)->&'static str{e}
#[derive(Clone,Copy)] struct Identity {runlist:u32,id:u32,instance:u64,engine:u32}
struct Report {identity:Identity}
struct Copy {base:usize,reset:u32,engine:u32}
struct Range {start:u64}
struct Allocation {range:Range}
struct Session {second:Option<Box<Session>>,report:Report,copy:Option<Copy>,commands:Allocation,pbdma:usize,bound:bool}
impl Session {fn quiesce(&mut self,r:&mut Registers,_:u64)->Result<(),&'static str>{
 assert!(!self.bound,"FIFO restore before unbind");STATE.with(|s|s.borrow_mut().events.push("quiesce"));
 if STATE.with(|s|s.borrow().fail_quiesce){return Err("restore refused");}
 if r.read(0x259c)!=0{Err("MMU fault still latched")}else{Ok(())}
}}
struct NativeChannel {registers:usize,total:u64,session:Session,confirmed:bool}
impl NativeChannel {// RESET
}
fn main(){
 let mut regs=Registers(0);regs.write(0x259c,0);
 let (_,faults)=gk208::vm::faults(&mut VmIo{regs:&mut regs,total:1<<30});
 assert!(faults.iter().all(|f|f.unit<32&&f.instance!=0));
 for reserved in [false,true] {
 for previously_confirmed in [false,true] {
 for (mask,fail_gr,fail_preempt,fail_unbind,okay,remaining) in [(1,false,false,false,true,0),(3,false,false,false,false,2),(1,true,false,false,false,1),(1<<0x15,false,false,false,false,1<<0x15),(1,false,true,false,false,0),(1,false,false,true,false,0)] {
 for (fail_wait,fail_ce,fail_quiesce) in [(0,false,false),(0,true,false),(0,false,true),(0x2284,false,false),(0x3080,false,false),(0x2640,false,false),(0x2650,false,false),(0x800008,false,false)] {
  let okay=okay && (fail_wait==0 || fail_wait==0x800008 && !reserved) && !fail_ce && !fail_quiesce;
  let remaining=if fail_ce && !fail_gr {mask}else{remaining};
  STATE.with(|s|*s.borrow_mut()=State{mask,blocked:0x10,fail_wait,fail_ce,fail_quiesce,fail_gr,fail_preempt,fail_unbind,writes:vec![],events:vec![]});
  let mut c=NativeChannel{registers:0,total:1<<30,confirmed:previously_confirmed,session:Session{
   second:if reserved {Some(Box::new(Session{second:None,
    report:Report{identity:Identity{runlist:0,id:1,instance:0x124000,engine:0}},commands:Allocation{range:Range{start:0x200000}},pbdma:0,bound:true,copy:None}))}else{None},report:Report{identity:Identity{runlist:0,id:0,instance:0x123000,engine:0}},commands:Allocation{range:Range{start:0x100000}},pbdma:0,bound:true,copy:Some(Copy{base:0x106000,reset:22,engine:2})}};
  assert_eq!(c.session.report.identity.instance,0x123000);
  assert_eq!(c.reset().is_ok(),okay);assert_eq!(c.confirmed,okay,"a refused reset must revoke an earlier confirmation (G19)");
  STATE.with(|s|{let s=s.borrow();assert_eq!(s.events.first(),Some(&"invalidate"),"a reset refusal must still invalidate panic preparation");assert_eq!(s.mask,remaining);assert_eq!(s.blocked,0x10,"every refusal must unblock its runlist and preserve other runlists");
   if okay {assert_eq!(s.events,["invalidate","gr-stop","ce-stop","ack","quiesce"]);
    if reserved {assert!(s.writes.contains(&(0x800008,0)));assert!(!c.session.second.as_ref().unwrap().bound);}}
   if reserved && fail_wait==0x800008 {assert!(!c.confirmed,"terminal unbind refusal must retain all backing");}
   if fail_gr {assert_eq!(s.events,["invalidate","gr-stop"]);}
   if fail_preempt {assert_eq!(s.events.get(1),Some(&"gr-stop"));assert!(!c.confirmed,"timed-out preempt must retain backing");}
   if okay {
    let at=|r|s.writes.iter().position(|w|w.0==r).unwrap();
    assert!(at(0x2634)<at(0x2274));assert!(at(0x2274)<at(0x262c));
    let unblock=s.writes.iter().position(|w|*w==(0x2630,0x10)).unwrap();
    assert!(at(0x262c)<unblock);assert!(unblock<at(0x800000));
   }
  });
  // A failed attempt must leave controls recoverable by a later reset.
  STATE.with(|s|{let mut s=s.borrow_mut();s.mask=0;s.fail_wait=0;s.fail_gr=false;s.fail_ce=false;s.fail_preempt=false;s.fail_unbind=false;s.fail_quiesce=false;});
  assert!(c.reset().is_ok());assert!(c.confirmed);
  STATE.with(|s|assert_eq!(s.borrow().blocked,0x10));
 }
 }
}
}
}
'''.replace('// RESET',reset)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'reset.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'reset.rs'),'-o',str(p/'reset')],check=True)
            subprocess.run([str(p/'reset')],check=True)
if __name__=='__main__':unittest.main()
