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
struct State {mask:u32,fail_gr:bool,events:Vec<&'static str>}
thread_local! {static STATE:RefCell<State>=RefCell::new(State{mask:0,fail_gr:false,events:vec![]});}
struct Registers(usize);
impl Registers {
 fn mask(&mut self,_:usize,_:u32,_:u32){let _=self.0;}
 fn read(&self,r:usize)->u32{if r==0x259c{STATE.with(|s|s.borrow().mask)}else{0}}
 fn write(&mut self,r:usize,v:u32){if r==0x259c{STATE.with(|s|{let mut s=s.borrow_mut();s.events.push("ack");s.mask&=!v;});}}
}
struct VmIo<'a>{regs:&'a mut Registers,total:u64}
mod gk208 {
 pub mod ce {pub fn reset_engine(_: &mut crate::Registers,_:usize,_:u32)->Result<(),&'static str>{crate::STATE.with(|s|s.borrow_mut().events.push("ce-stop"));Ok(())}}
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
#[derive(Clone,Copy)] struct Identity {runlist:u32,id:u32,instance:u64}
struct Report {identity:Identity}
struct Copy {base:usize,reset:u32}
struct Session {report:Report,copy:Option<Copy>}
impl Session {fn quiesce(&mut self,r:&mut Registers,_:u64)->Result<(),&'static str>{
 STATE.with(|s|s.borrow_mut().events.push("quiesce"));
 if r.read(0x259c)!=0{Err("MMU fault still latched")}else{Ok(())}
}}
struct NativeChannel {registers:usize,total:u64,session:Session,confirmed:bool}
impl NativeChannel {// RESET
}
fn main(){
 let mut regs=Registers(0);regs.write(0x259c,0);
 let (_,faults)=gk208::vm::faults(&mut VmIo{regs:&mut regs,total:1<<30});
 assert!(faults.iter().all(|f|f.unit<32&&f.instance!=0));
 for (mask,fail_gr,okay,remaining) in [(1,false,true,0),(3,false,false,2),(1,true,false,1),(1<<0x15,false,false,1<<0x15)] {
  STATE.with(|s|*s.borrow_mut()=State{mask,fail_gr,events:vec![]});
  let mut c=NativeChannel{registers:0,total:1<<30,confirmed:false,session:Session{
   report:Report{identity:Identity{runlist:0,id:0,instance:0x123000}},copy:Some(Copy{base:0x106000,reset:22})}};
  assert_eq!(c.session.report.identity.instance,0x123000);
  assert_eq!(c.reset().is_ok(),okay);assert_eq!(c.confirmed,okay);
  STATE.with(|s|{let s=s.borrow();assert_eq!(s.mask,remaining);
   if okay {assert_eq!(s.events,["gr-stop","ce-stop","ack","quiesce"]);}
   if fail_gr {assert_eq!(s.events,["gr-stop"]);}
  });
 }
}
'''.replace('// RESET',reset)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'reset.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'reset.rs'),'-o',str(p/'reset')],check=True)
            subprocess.run([str(p/'reset')],check=True)
if __name__=='__main__':unittest.main()
