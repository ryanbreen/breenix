"""Exercise the production GR queue publisher and receipt sampler on host IO."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item

class NativeQueue(unittest.TestCase):
    def test_receipt_channel_faults_and_wrapped_publication(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        monitor = item(source, "    struct Monitor<'a>") + '\n' + item(source, "    impl Poll for Monitor<'_>")
        monitor = monitor.replace('super::super::binding', 'binding').replace('crate::gk208::queue', 'queue').replace('super::super::error', 'hardware_error')
        publisher = item(source, '        fn publish_words(').replace('crate::gk208::queue', 'queue')
        traps = (ROOT / 'kernel/src/gpu/gk208/gr/binding.rs').read_text()
        health = item(traps, 'pub fn check_traps(')
        registers = traps[traps.index('pub const TRAP_REGISTERS'):traps.index('];', traps.index('pub const TRAP_REGISTERS')) + 2]
        status_io = item(source, '    impl super::super::error::Io for Registers')
        status_io = status_io.replace('super::super::error', 'hardware_error').replace('elapsed(0)', 'self.now')
        refusal = source[source.index('    const REFUSAL_REGISTERS'):source.index('    /// Keep the words')]
        refusal = refusal.replace('fn save_refusal', 'pub(super) fn save_refusal').replace('fn take_refusal', 'pub(super) fn take_refusal')
        harness = r'''
extern crate alloc;
use std::collections::{BTreeMap,VecDeque};
#[path="ROOT/kernel/src/gpu/gk208/queue.rs"] pub mod queue;
#[path="ROOT/kernel/src/gpu/gk208/gr/error.rs"] pub mod hardware_error;
const SLOT:u64=(8<<20)-4096;const COMMAND:u64=0x01000000;
#[derive(Default)] struct Registers {values:BTreeMap<usize,u32>,vram:BTreeMap<u64,u32>,
 scripted:BTreeMap<usize,VecDeque<u32>>,reads:BTreeMap<usize,usize>,now:u64,refusals:Vec<[u32;4]>,captures:Vec<([u32;4],[u32;7],[u32;4])>}
impl Registers {
 fn read(&mut self,a:usize)->u32{*self.reads.entry(a).or_default()+=1;
  self.scripted.get_mut(&a).and_then(VecDeque::pop_front).unwrap_or_else(||*self.values.get(&a).unwrap_or(&0))}
 fn delay_us(&mut self,us:u64){self.now+=us;}
 fn write(&mut self,a:usize,v:u32){self.values.insert(a,v);}
}
mod refusal { use super::Registers; REFUSAL }
fn save_refusal(r:&mut Registers,words:[u32;4]){
 r.refusals.push(words);refusal::save_refusal(r,words);r.captures.push(refusal::take_refusal().unwrap());
}
STATUS_IO
fn read(r:&mut Registers,_:u64,a:u64)->Result<u32,&'static str>{Ok(*r.vram.get(&a).unwrap_or(&0))}
fn words(r:&mut Registers,_:u64,a:u64,words:&[u32])->Result<(),&'static str>{for(i,v)in words.iter().enumerate(){r.vram.insert(a+i as u64*4,*v);}Ok(())}
fn elapsed(_:u64)->u64{0}
mod modeset {
 pub trait Io {fn delay_us(&mut self,us:u32);}
 impl Io for super::Registers {fn delay_us(&mut self,us:u32){self.delay_us(u64::from(us));}}
 pub fn bar_flush(_:&mut super::Registers)->Result<(),&'static str>{Ok(())}}
mod gpu_cache {pub fn drain_wc(){}}
mod binding {BINDING_REGISTERS HEALTH}
struct Pages(Box<[u32;1024]>);impl Pages {fn va(&self,_:usize)->u64{self.0.as_ptr() as u64}fn observe(&self){}}
struct Range {start:u64}struct Allocation {range:Range}
struct Identity {id:u32,instance:u64}struct Report {identity:Identity}struct Copy {base:usize}
struct Session {pages:Pages,commands:Allocation,bar:usize,report:Report,pbdma:usize,userd:u64,copy:Option<Copy>,bound:bool,quiesced:bool}
trait Poll {fn snapshot(&mut self)->Result<(u32,u64,bool),&'static str>;fn now_us(&mut self)->u64;fn relax(&mut self);}
MONITOR
impl Session {PUBLISHER}
fn session(bar:usize)->Session {Session{pages:Pages(Box::new([0;1024])),commands:Allocation{range:Range{start:0x100000}},bar,
 report:Report{identity:Identity{id:0,instance:0x123000}},pbdma:0,userd:0x200000,copy:Some(Copy{base:0x106000}),bound:true,quiesced:false}}
#[test]fn receipt_requires_the_consuming_channel_and_zero_faults(){
 let mut s=session(0);s.pages.0[..3].copy_from_slice(&[1,17,0]);s.pages.0[4..7].copy_from_slice(&[2,16,0]);
 let mut r=Registers::default();r.values.extend([(0x40120,0x2000),(0x40014,4),(0x40028,4),(0x800000,0x80000123)]);
 let sample=|r:&mut Registers,slot|Monitor{session:&s,regs:r,total:1<<24,sequence:1,stop_on_trap:true,release_slot:slot,queued_put:Some(4)}.snapshot();
 assert_eq!(sample(&mut r,0),Ok((1,17,true)));assert_eq!(sample(&mut r,1),Ok((2,16,true)),"never replace an observed old epoch");
 r.values.insert(0x40120,0x2001);assert_eq!(sample(&mut r,0),Ok((1,17,false)),"a peer's GET/reference cannot complete this channel");
 r.values.insert(0x40120,0x2000);r.values.insert(0x40014,0);assert_eq!(sample(&mut r,0),Ok((1,17,false)));
 r.values.insert(0x40014,4);
 for a in binding::TRAP_REGISTERS.into_iter().chain([0x259c,0x40108,0x40148,0x106908]) {r.values.insert(a,1);assert!(sample(&mut r,0).is_err(),"fault {a:x} accepted");r.values.insert(a,0);}
 let mut m=Monitor{session:&s,regs:&mut r,total:1<<24,sequence:1,stop_on_trap:true,release_slot:0,queued_put:Some(4)};assert_eq!(m.now_us(),0);m.relax();
}
fn sample(r:&mut Registers)->Result<(u32,u64,bool),&'static str>{
 let s=session(0);Monitor{session:&s,regs:r,total:1<<24,sequence:1,stop_on_trap:true,release_slot:0,queued_put:Some(4)}.snapshot()
}
#[test]fn transient_pri_is_retried_without_refusal(){
 for address in binding::TRAP_REGISTERS {
  let mut r=Registers::default();r.scripted.insert(address,VecDeque::from([0xbadf1201,0]));
  assert!(sample(&mut r).is_ok(),"transient PRI at {address:x} was treated as a trap");
  assert!(r.refusals.is_empty());assert!((1..=1000).contains(&r.now));
  assert_eq!(r.reads[&address],2);
 }
}
#[test]fn persistent_pri_refuses_within_one_millisecond(){
 let mut r=Registers::default();r.values.insert(0x400100,0xbadf1201);
 assert_eq!(sample(&mut r),Err("GR PRI unreachable"));
 assert!((1..=1000).contains(&r.now));assert_eq!(r.refusals,vec![[0xbadf1201,0,0,0]]);
 assert!(r.reads[&0x400100]>1);
 assert_eq!(r.captures[0].2,[0xbadf1201,0,0,0]);
 for a in [0x400100,0x200,0x409614,0x404170] {assert!(r.reads[&a]>0);}
}
#[test]fn real_trap_is_never_retried_even_alongside_pri(){
 for pri in [false,true] {
  let mut r=Registers::default();r.scripted.insert(0x400100,VecDeque::from([0x00200000,0]));
  if pri {r.values.insert(0x400108,0xbadf1200);}
  assert_eq!(sample(&mut r),Err("shader binding PGRAPH trap/exception"));
  assert_eq!(r.now,0);assert_eq!(r.reads[&0x400100],2,"one status read plus the diagnostic re-read");assert_eq!(r.refusals.len(),1);
  assert_eq!(r.refusals[0][0],0x00200000);
 }
}
#[test]fn four_immutable_stream_slots_and_descriptor_wrap(){
 let mut bar=vec![0u32;2<<20];let s=session(bar.as_mut_ptr() as usize);let mut r=Registers::default();
 for seq in 1..=4 {let offset=16384+u64::from(queue::Queue::slot(seq))*8192;s.publish_words(&mut r,1<<24,seq,&[seq,0x20010014,seq],offset,8192).unwrap();}
 for seq in 1..=4 {let offset=16384+u64::from(queue::Queue::slot(seq))*8192;assert_eq!(r.vram[&(0x100000+offset)],seq);assert_eq!(r.vram[&(0x100000+u64::from(seq-1)*8)],(COMMAND+offset) as u32);}
 for seq in [511,512,513] {let offset=16384+u64::from(queue::Queue::slot(seq))*8192;s.publish_words(&mut r,1<<24,seq,&[seq],offset,8192).unwrap();assert_eq!(bar[(SLOT as usize+0x8c)/4],seq%512);assert_eq!(r.vram[&(0x100000+u64::from(queue::Queue::entry(seq))*8)],(COMMAND+offset) as u32);}
}
'''
        for key, value in [('ROOT', str(ROOT)), ('BINDING_REGISTERS', registers), ('HEALTH', health), ('MONITOR', monitor), ('PUBLISHER', publisher), ('STATUS_IO', status_io), ('REFUSAL', refusal)]:
            harness = harness.replace(key, value)
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            (out / 'queue.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(out / 'queue.rs'), '-o', str(out / 'queue')], check=True)
            subprocess.run([str(out / 'queue'), '--test-threads=1'], check=True)

if __name__ == '__main__':
    unittest.main()
