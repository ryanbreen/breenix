"""The early control-stream timeout retains its state before returning refusal."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class ControlTimeout(unittest.TestCase):
    def test_timeout_records_identity_and_bound_without_retry_or_receipt(self):
        method = item((ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text(),
                      '        fn issue_words(')
        harness = r'''
use std::cell::Cell;
const BOUND_US:u64=2_000_000;
struct Pages(Box<[u8;4096]>);
impl Pages {fn va(&self,_:usize)->u64{self.0.as_ptr() as u64} fn publish(&self){}}
struct Identity {epoch:u64}
struct Report {identity:Identity}
struct Session {pages:Pages,report:Report,published:Cell<usize>,captures:Cell<usize>}
struct Registers;
struct Hardware<'a> {regs:&'a mut Registers,total:u64}
struct Observation {consumed:bool}
struct Monitor<'a> {
 session:&'a Session,regs:&'a mut Registers,total:u64,sequence:u32,
 stop_on_trap:bool,retry_traps:bool,release_slot:usize,queued_put:Option<u32>,
}
impl Monitor<'_> {
 fn snapshot(&mut self)->Result<(u32,u64,bool),&'static str>{
  let _=(&self.regs,self.total,self.stop_on_trap,self.retry_traps,self.release_slot,self.queued_put);
  Ok((85,0,false))
 }
 fn now_us(&mut self)->u64{BOUND_US}
 fn relax(&mut self){}
 fn record_control_timeout(&mut self,nonce:u32,elapsed_us:u64){
  assert_eq!((self.sequence,self.session.report.identity.epoch,nonce),(1,17,23));
  assert!(elapsed_us>=BOUND_US);
  self.session.captures.set(self.session.captures.get()+1);
 }
}
fn wait(_: &mut Monitor<'_>,_:u64,_:u32,_:u64)->Result<Observation,&'static str>{
 panic!("a timed-out control cannot enter receipt observation")
}
impl Session {
 fn publish_words(&self,_:&mut Registers,_:u64,_:u32,_:&[u32],_:u64,_:usize,_:Option<()>)->Result<(),&'static str>{
  self.published.set(self.published.get()+1);Ok(())
 }
 // METHOD
}
fn main(){
 let s=Session{pages:Pages(Box::new([0;4096])),report:Report{identity:Identity{epoch:17}},
  published:Cell::new(0),captures:Cell::new(0)};
 let mut regs=Registers;
 // Keep the fake clock advancing past the original two-second deadline.
 let _=Monitor::record_control_timeout;
 let result=s.issue_words(&mut Hardware{regs:&mut regs,total:1},1,85,23,10000,&[1],false);
 assert_eq!(result.err(),Some("GR control stream not consumed; backing retained"));
 assert_eq!(s.published.get(),1,"no timeout retry is allowed");
 assert_eq!(s.captures.get(),1,"refusal must capture live registers before returning to cleanup");
}
'''.replace('// METHOD', method).replace(
            'fn now_us(&mut self)->u64{BOUND_US}',
            'fn now_us(&mut self)->u64{static CLOCK:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);CLOCK.fetch_add(BOUND_US,std::sync::atomic::Ordering::Relaxed)}')
        prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'timeout.rs').write_text(harness)
            subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                           str(p / 'timeout.rs'), '-o', str(p / 'timeout')], check=True)
            subprocess.run([str(p / 'timeout')], check=True)


if __name__ == '__main__':
    unittest.main()
