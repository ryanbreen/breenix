"""Pending coherent receipts avoid redundant PCIe scans without accepting work."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class PendingReceipt(unittest.TestCase):
    def test_negative_control_resample_reads_hardware_after_drain(self):
        channel = (ROOT/'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        helper = item(channel, '        fn pending_receipt(')
        sample = item(channel, '        fn sample_with(')
        # Execute the production dispatch; replace only the MMIO monitor with
        # a counted hardware sample of the deliberately withheld release.
        sample = sample[:sample.index('            let mut regs = Registers(self.registers);')] + r'''
            self.scans += 1;
            self.retry_traps = retry_traps;
            Ok(crate::gpu_completion::CeSample {
                context: 0x123000, epoch: 17, sequence: 0,
                consumed: sequence, healthy: true,
            })
        }
'''
        source = r'''
fn elapsed(_:u64)->u64{1}
mod gpu_completion {pub struct CeSample{pub context:u64,pub epoch:u64,pub sequence:u32,pub consumed:u32,pub healthy:bool}}
mod gk208 {pub mod queue{pub struct Queue;impl Queue{pub fn slot(s:u32)->u32{(s-1)%4}}}}
struct Pages{words:[u32;16]}
impl Pages{fn observe(&self){}fn va(&self,_:usize)->u64{self.words.as_ptr()as u64}}
struct Identity{instance:u64,epoch:u64}struct Report{identity:Identity}
struct Session{pages:Pages,report:Report}
struct Channel{session:Session,health_after:u64,scans:u32,retry_traps:bool}
impl Channel{// METHODS
}
fn main(){
 let mut words=[0;16];words[1]=17;
 let mut c=Channel{session:Session{pages:Pages{words},report:Report{identity:Identity{instance:0x123000,epoch:17}}},health_after:0,scans:0,retry_traps:false};
 for _ in 0..2 {
  let s=c.sample_wait(1).unwrap();
  assert_eq!((s.context,s.epoch,s.sequence,s.consumed,s.healthy),(0x123000,17,0,1,true),"negative-control resample must retain the real consumed identity");
 }
 assert_eq!(c.scans,2,"each diagnostic resample must check hardware");
 assert!(c.retry_traps);
 c.health_after=50;
 assert_eq!(c.sample_with(1,false).unwrap().consumed,0);
 assert_eq!(c.scans,2,"ordinary pending polls retain their amortized scan");
}
'''.replace('// METHODS', helper + item(channel, '        pub(crate) fn sample_wait(') + sample)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-control-resample-') as directory:
            p = Path(directory)
            (p/'check.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

    def test_pending_poll_keeps_health_deadline_and_completion_checks(self):
        channel = (ROOT/'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        helper = item(channel, '        fn pending_receipt(') if '        fn pending_receipt(' in channel else ''
        source = r'''
use std::cell::Cell;
thread_local!{static NOW:Cell<u64>=const{Cell::new(0)};}
fn elapsed(_:u64)->u64{NOW.with(Cell::get)}
mod gpu_completion {#[derive(Debug)]pub struct CeSample{pub context:u64,pub epoch:u64,pub sequence:u32,pub consumed:u32,pub healthy:bool}}
mod gk208 {pub mod queue{pub struct Queue;impl Queue{pub fn slot(s:u32)->u32{(s-1)%4}}}}
struct Pages{words:[u32;16]}
impl Pages{fn observe(&self){}fn va(&self,_:usize)->u64{self.words.as_ptr()as u64}}
struct Identity{instance:u64,epoch:u64}struct Report{identity:Identity}
struct Session{pages:Pages,report:Report}struct Channel{session:Session,health_after:u64}
impl Channel{ // HELPER
 fn sample(&mut self,seq:u32,fault:bool)->Result<bool,&'static str>{
  if let Some(s)=self.pending_receipt(seq){assert!(s.healthy);assert_eq!(s.context,0x123000);assert_eq!(s.epoch,17);assert_eq!(s.sequence,0);assert_eq!(s.consumed,0);return Ok(false);}
  if fault{return Err("fault");}Ok(self.session.pages.words[0]==seq)
 }
}
fn main(){
 let mut c=Channel{session:Session{pages:Pages{words:[0;16]},report:Report{identity:Identity{instance:0x123000,epoch:17}}},health_after:0};
 c.session.pages.words[1]=17;
 assert!(!c.sample(1,false).unwrap()); // first call must scan health
 assert_eq!(c.health_after,50);
 for us in 1..50{NOW.with(|n|n.set(us));assert!(!c.sample(1,true).unwrap());}
 NOW.with(|n|n.set(50));assert_eq!(c.sample(1,true),Err("fault"));
 NOW.with(|n|n.set(51));c.session.pages.words[0]=1;
 assert!(c.pending_receipt(1).is_none(),"a matching receipt must run full health and identity checks immediately");
 assert_eq!(c.sample(1,true),Err("fault"),"health failure cannot be accepted on receipt");
 c.session.pages.words[1]=16;
 assert_eq!(c.pending_receipt(1).unwrap().consumed,0,"stale epochs stay pending");
}
'''.replace('// HELPER', helper)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-pending-receipt-') as directory:
            p = Path(directory)
            (p/'check.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)
        sample = item(channel, '        fn sample_with(')
        self.assertLess(sample.index('self.pending_receipt(sequence)'),sample.index('Registers(self.registers)'))
        self.assertIn('monitor.snapshot()?',sample)
        self.assertIn('&& consumed',sample)
        self.assertIn('bound == 0x80000000',sample)


if __name__ == '__main__':
    unittest.main()
