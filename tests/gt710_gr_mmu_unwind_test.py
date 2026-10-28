"""Check the production GR teardown with the retained IDXFETCH fault shape.

MIT nouveau v6.8 gf100.c:696-733 decodes instance and acknowledges MMU
units; gk104.c:404-418 clears and resumes the runlist. The host model
holds runlist pending until those operations follow a successful GR reset.
It does not establish a hardware recovery result.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class MmuUnwind(unittest.TestCase):
    def test_fault_ack_follows_reset_and_precedes_drain(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        method = item(source, '        pub(in crate::gk208::gr) fn quiesce(') + '\n' + item(source, '        fn quiesce_channel(')
        method = method.replace('pub(in crate::gk208::gr)', 'pub(crate)').replace('super::super::', 'crate::')
        harness = r'''
use std::collections::BTreeMap;
const BOUND_US:u64=2_000_000; const RUNLIST:u64=12288;
#[derive(Clone,Copy)] struct Identity {id:u32,runlist:u32,engine:u32,instance:u64}
struct Report {identity:Identity,binding:u32,runlist_count:u32}
struct Range {start:u64} struct Allocation {range:Range}
struct DrawCopy {base:usize,engine:u32,reset:u32,old_irq:u32,old_enable:u32}
struct Session {second:Option<Box<Session>>,quiesced:bool,preempt_error:Option<&'static str>,bound:bool,report:Report,commands:Allocation,pbdma:usize,copy:Option<DrawCopy>,alias:bool,pte:u64,bar_inst:u64,bar_dir:u64,old:[u32;6],old_pbdma:[u32;4]}
struct Registers {values:BTreeMap<usize,u32>,log:Vec<(usize,u32)>,reset:bool,fail:bool,cleared:bool,resumed:bool}
impl Registers {
 fn read(&self,a:usize)->u32{*self.values.get(&a).unwrap_or(&0)}
 fn write(&mut self,a:usize,v:u32){
  self.log.push((a,v));
  if a==0x259c {assert!(self.reset,"MMU ack before reset");assert_eq!(v,1);self.values.insert(a,self.read(a)&!v);}
  else {self.values.insert(a,v);}
  if a==0x262c {assert_eq!(self.read(0x259c),0,"runlist fault clear before MMU ack");self.cleared=true;}
  if a==0x2630 && v&1==0 && self.cleared {self.resumed=true;}
 }
 fn mask(&mut self,a:usize,m:u32,v:u32){self.write(a,(self.read(a)&!m)|v);}
}
mod ctxctl {pub fn reset(r:&mut super::Registers)->Result<(),&'static str>{r.log.push((0xf00000,0));if r.fail{return Err("reset failed");}r.reset=true;Ok(())}}
fn hardware_error(e:&'static str)->&'static str{e}
fn register_wait(r:&mut Registers,a:usize,_:u32,_:u32,_:u64)->Result<(),&'static str>{
 if a==0x2634{return Err("preempt timed out");}
 if a==0x2284 && !(r.reset&&r.read(0x259c)==0&&r.cleared&&r.resumed){return Err("runlist pending");}
 Ok(())
}
fn words(_:&mut Registers,_:u64,_:u64,_:&[u32])->Result<(),&'static str>{Ok(())}
mod modeset {pub fn activate_bar1(_:&mut super::Registers,_:u64,_:u64)->Result<(),&'static str>{Ok(())}}
impl Session {METHOD}
fn session()->Session{Session{second:None,quiesced:false,preempt_error:None,bound:true,report:Report{identity:Identity{id:0,runlist:0,engine:0,instance:0x851c000},binding:1,runlist_count:1},commands:Allocation{range:Range{start:0x100000}},pbdma:0,copy:None,alias:false,pte:0,bar_inst:0,bar_dir:0,old:[0;6],old_pbdma:[0;4]}}
fn regs(instance:u32,fail:bool)->Registers{Registers{values:[(0x259c,1),(0x2800,instance),(0x200,0x1000)].into_iter().collect(),log:vec![],reset:false,fail,cleared:false,resumed:false}}
#[test] fn own_fault_is_acknowledged_after_reset_before_runlist_drain(){
 let mut r=regs(0x851c,false);let mut s=session();assert_eq!(s.report.identity.instance,0x851c000);assert_eq!(s.quiesce(&mut r,1<<30),Ok(()));
 assert!(s.quiesced);assert_eq!(r.read(0x259c),0);assert!(r.cleared&&r.resumed);
 let pos=|a|r.log.iter().position(|p|p.0==a).unwrap();assert!(pos(0xf00000)<pos(0x259c));assert!(pos(0x259c)<pos(0x262c));
}
#[test] fn failed_reset_keeps_fault_and_retains_backing(){
 let mut r=regs(0x851c,true);let mut s=session();assert!(s.quiesce(&mut r,1<<30).is_err());assert!(!s.quiesced);assert_eq!(r.read(0x259c),1);assert!(!r.cleared);
}
#[test] fn another_context_fault_is_never_acknowledged(){
 let mut r=regs(0x851d,false);let mut s=session();assert!(s.quiesce(&mut r,1<<30).is_err());assert!(!s.quiesced);assert_eq!(r.read(0x259c),1);assert!(!r.cleared);
}
'''.replace('METHOD', method)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'unwind.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'unwind.rs'), '-o', str(path / 'unwind')], check=True)
            subprocess.run([str(path / 'unwind')], check=True)


if __name__ == '__main__':
    unittest.main()
