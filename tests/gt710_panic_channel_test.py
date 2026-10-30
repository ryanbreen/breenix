"""Execute the production reserved-channel snapshot and VM alias preparation."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class PanicChannel(unittest.TestCase):
    def test_snapshot_uses_reserved_transport_and_retained_vm(self):
        channel = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        snapshot = item(channel, '        pub(crate) fn terminal_identity(') + '\n' + item(channel, '        pub(crate) fn terminal_queue(')
        resource = (ROOT / 'kernel/src/gpu/gk208/resource.rs').read_text()
        bind = item(resource, '    pub(crate) fn bind_terminal(')
        source = r'''
use std::{cell::RefCell,collections::BTreeMap};
thread_local! {static WRITES:RefCell<Vec<u64>>=RefCell::new(vec![]);}
struct IoState {values:BTreeMap<u64,u32>, fail:bool}
trait Io {fn read(&mut self,a:u64)->Result<u32,&'static str>;fn write(&mut self,a:u64,v:u32)->Result<(),&'static str>;fn flush(&mut self)->Result<(),&'static str>;fn invalidate(&mut self,a:u64)->Result<u32,&'static str>;}
impl Io for IoState {
 fn read(&mut self,a:u64)->Result<u32,&'static str>{Ok(*self.values.get(&a).unwrap_or(&0))}
 fn write(&mut self,a:u64,v:u32)->Result<(),&'static str>{self.values.insert(a,v);Ok(())}
 fn flush(&mut self)->Result<(),&'static str>{if self.fail{Err("flush refused")}else{Ok(())}}
 fn invalidate(&mut self,a:u64)->Result<u32,&'static str>{WRITES.with(|w|w.borrow_mut().push(a));Ok(0)}
}
struct Range {start:u64} struct Allocation {range:Range}
struct Space {directory:Allocation,valid:bool}
impl Space {fn usable(&self)->bool{self.valid}fn table_ranges(&self)->Vec<(u64,())>{vec![(32,())]}}
struct Resources {space:Option<Space>}
impl Resources {
 fn bind(&self,_:&mut impl Io,d:u64,i:u64,e:u64)->Result<(),&'static str>{assert_eq!((d,i,e),(0x1000,0x2000,10));Ok(())}
 // ALIAS
}
mod gk208 {
 pub mod ce {
  pub fn with_resources<T>(f:impl FnOnce(&mut crate::Resources,&mut (),&mut crate::IoState)->Result<T,&'static str>)->Result<T,&'static str>{
   f(&mut crate::Resources{space:Some(crate::Space{directory:crate::Allocation{range:crate::Range{start:0x9000}},valid:true})},&mut (),&mut crate::IoState{values:std::collections::BTreeMap::from([(0x9100,0x555)]),fail:false})
  }
 }
 pub mod emergency {pub struct Queue {pub registers:usize,pub total:u64,pub ring:u64,pub userd:u64,pub pbdma:usize,pub ce_error:usize,pub channel:u32,pub instance:u64,pub report:u64,pub report_va:u64,pub epoch:u64,pub quarantine:u64,pub parking:Vec<u32>}}
}
const TARGET:u64=0x100000;
struct Context(Space);impl Context {fn space(&self)->&Space{&self.0}}
struct Pages(u64);impl Pages {fn va(&self,_:usize)->u64{self.0}}
struct Identity {id:u32,instance:u64,epoch:u64} struct Report {identity:Identity}
struct Copy {base:usize}
struct Session {second:Option<Box<Session>>,context:Context,bound:bool,quiesced:bool,copy:Option<Copy>,report:Report,commands:Allocation,userd:u64,pbdma:usize,pages:Pages}
struct NativeChannel {session:Session,registers:usize,total:u64,terminal_quarantine:u64,parking:Vec<u32>}
impl NativeChannel {// SNAPSHOT
}
fn session(id:u32)->Session {
 let n=u64::from(id);
 Session{second:None,context:Context(Space{directory:Allocation{range:Range{start:0x1000+n*0x1000}},valid:true}),bound:true,quiesced:false,
  copy:Some(Copy{base:0x104000}),report:Report{identity:Identity{id,instance:0x2000+n*0x1000,epoch:10+n}},commands:Allocation{range:Range{start:0x4000+n*0x1000}},userd:0x6000+n*512,pbdma:2,pages:Pages(0x7000+n*0x1000)}
}
#[test] fn reserved_transport_is_disjoint_and_snapshot_refuses_unavailable_state() {
 let mut c=NativeChannel{session:session(0),registers:7,total:1<<30,terminal_quarantine:0x400000,parking:vec![12]};
 assert!(c.terminal_queue().is_err());assert_eq!(c.terminal_identity(),None);
 c.session.second=Some(Box::new(session(1)));
 assert_eq!(c.terminal_identity(),Some((0x3000,11)));
 let q=c.terminal_queue().unwrap();
 assert_eq!((q.channel,q.instance,q.epoch),(1,0x3000,11));
 assert_eq!((q.ring,q.userd,q.report),(0x5000,0x6200,0x8200));
 assert_eq!((q.registers,q.total,q.pbdma,q.ce_error,q.report_va,q.quarantine,q.parking),(7,1<<30,2,0x104908,TARGET+512,0x400000,vec![12]));
 assert_eq!(c.session.commands.range.start,0x4000);
 WRITES.with(|w|assert_eq!(*w.borrow(),vec![0x2000]));
 c.session.second.as_mut().unwrap().quiesced=true;assert!(c.terminal_queue().is_err());assert_eq!(c.terminal_identity(),None);
 c.session.second.as_mut().unwrap().quiesced=false;c.parking.clear();assert!(c.terminal_queue().is_err());
 c.parking.push(12);c.session.second.as_mut().unwrap().bound=false;assert!(c.terminal_queue().is_err());
 c.session.second.as_mut().unwrap().bound=true;c.session.copy=None;assert!(c.terminal_queue().is_err());
}
#[test] fn terminal_alias_refuses_foreign_entries_and_failed_publication() {
 for (valid,occupied,fail,okay) in [(true,0,false,true),(false,0,false,false),(true,0x666,false,false),(true,0,true,false)] {
  let r=Resources{space:Some(Space{directory:Allocation{range:Range{start:0x9000}},valid})};
  let mut io=IoState{values:BTreeMap::from([(0x9100,0x555),(0x2100,occupied)]),fail};
  assert_eq!(r.bind_terminal(&mut io,0x2000).is_ok(),okay);
  if okay {assert_eq!(io.values[&0x2100],0x555);}
 }
 let r=Resources{space:None};let mut io=IoState{values:BTreeMap::new(),fail:false};
 assert!(r.bind_terminal(&mut io,0x2000).is_err());
}
'''.replace('// ALIAS', bind).replace('// SNAPSHOT', snapshot)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'channel.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'channel.rs'), '-o', str(path / 'channel')], check=True)
            subprocess.run([str(path / 'channel')], check=True)

if __name__ == '__main__':
    unittest.main()
