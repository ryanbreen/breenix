"""Force a waiting caller to discover an MMU fault while kgpu is asleep."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT=Path(__file__).resolve().parents[1]


class FaultWakeup(unittest.TestCase):
    def test_attributed_empty_reap_wakes_recovery_and_publishes_eio(self):
        revision=os.environ.get('BREENIX_FAULT_SOURCE_REV')
        def source(path):
            if revision:
                return subprocess.check_output(['git','show',f'{revision}:{path}'],cwd=ROOT,text=True)
            return (ROOT/path).read_text()
        adapter=source('kernel/src/gpu/gk208/adapter.rs')
        core=source('kernel/src/gpu/mod.rs')
        fixture=(ROOT/'tests/gt710_adapter_test.rs').read_text()
        fixture=fixture[:fixture.index('fn request(caller:')]+item(fixture,'fn mmu_fault(')
        fixture=fixture.replace('copy::Rect,','')
        # Keep the existing device and channel boundary; run production reap_all.
        fixture=fixture.replace('pub enum Reap {Empty,Accepted(Tag),Done(Done),Fault,Reset(Tag)}', '''
        pub enum Reap {Empty,Accepted(Tag),Done(Done),Fault,Reset(Tag)}
        pub enum Device{Gk208(crate::gk208::adapter::Device)}
        impl Device{pub fn reap(&mut self)->Reap{match self{Self::Gk208(d)=>d.reap()}}pub fn needs_reset(&self)->bool{false}}
        ''')
        if 'self.channel.present(surface, matches!' not in adapter:
            fixture=fixture.replace(', restoring:bool','').replace('                    self.0.borrow_mut().events.push(if restoring {"restore"} else {"client-present"});','')
        fixture=fixture.replace('../kernel/',str(ROOT/'kernel')+'/')
        fixture += r'''
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool,AtomicU64,Ordering};
use gpu::dev::Device as CoreDevice;
static EVENTS:AtomicU64=AtomicU64::new(0);
static CONFIG_CHANGED:AtomicBool=AtomicBool::new(false);
static WOKEN:AtomicBool=AtomicBool::new(false);
fn wake_reaper(){WOKEN.store(true,Ordering::Release);}
fn notify(){EVENTS.fetch_add(1,Ordering::AcqRel);wake_reaper();}
fn now_ns()->u64{123}
struct Sync{upload:Option<(u32,u32)>}
struct Resource{invalid:bool,invalidated_by:u64}
mod completion{pub fn finish_write(_: &mut bool,_:&mut u64,_:u64,_:bool,_:bool){}}
struct Core{dev:Option<CoreDevice>,stalled:bool,reset_wanted:bool,last_progress:u64,
 syncs:BTreeMap<u64,Sync>,resources:BTreeMap<u32,Resource>,answers:Vec<(Tag,u32)>}
impl Core{
 REAP
 fn fault(&mut self){self.reset_wanted=true;notify();}
 fn present_accepted(&mut self,_:u64){panic!("no display update in this fault");}
 fn ended_tag(&mut self,_:Tag){panic!("attributed error lost to reset");}
 fn returned(&mut self,d:gpu::dev::Done){self.answers.push((d.tag,d.answer));notify();}
}
impl gk208::gr::channel::NativeChannel{pub fn record_refusal(&self)->Result<(),&'static str>{Ok(())}}
#[test]fn caller_discovers_fault_between_worker_check_and_block(){
 let (mut d,s)=device();s.borrow_mut().epoch=1;
 let bad=Tag::Submit{ctx:7,serial:9001,last:true};
 assert!(d.send(Request::undefined_command(bad,7,9001).unwrap()).is_ok());
 mmu_fault(&s,1);
 let seen=EVENTS.load(Ordering::Acquire);
 let mut c=Core{dev:Some(CoreDevice::Gk208(d)),stalled:false,reset_wanted:false,last_progress:0,
  syncs:BTreeMap::new(),resources:BTreeMap::new(),answers:vec![]};
 // kgpu has checked for work; a caller's WAIT now discovers the fault.
 c.reap_all();
 assert!(WOKEN.load(Ordering::Acquire),"WAIT discovered an attributed error but left kgpu asleep");
 assert_ne!(EVENTS.load(Ordering::Acquire),seen,"kgpu can block after the wake without an event");
 let CoreDevice::Gk208(d)=c.dev.as_mut().unwrap();assert!(d.recovery_pending());
 d.recover_fault().unwrap();c.reap_all();
 assert_eq!(c.answers,[(bad,0x60010000)]);
 assert!(!c.reset_wanted);assert_eq!(s.borrow().epoch,2);
}
'''.replace('REAP',item(core,'    fn reap_all(').replace('#[cfg(target_arch = "x86_64")]','').replace('Device::Gk208','CoreDevice::Gk208'))
        adapter+='\nimpl Device {\n'+item(adapter,'pub(crate) fn recover_fault(').replace('pub(crate)','pub')+'''
 fn reprobe(&mut self)->Result<(),&'static str>{assert_eq!(self.outstanding(),0);Ok(())}
}
'''
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p=Path(directory);(p/'adapter.rs').write_text(adapter)
            fixture=fixture.replace('#[path = "adapter.rs"] pub mod adapter;',f'#[allow(dead_code)] // Only fault recovery APIs are exercised here.\n#[path = "{p/"adapter.rs"}"] pub mod adapter;')
            (p/'check.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib','-Dwarnings',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check'),'--nocapture'],check=True)


if __name__=='__main__':unittest.main()
