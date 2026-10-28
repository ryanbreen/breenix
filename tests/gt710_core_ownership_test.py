"""Execute native probe admission and recovery-worker restoration on the host.

Hardware begins at an injected error boundary. The production worker must
return an ownership refusal without crossing it or changing retained state.
"""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class CoreOwnership(unittest.TestCase):
    def test_probes_refuse_core_ownership_and_restore_the_session(self):
        revision = os.environ.get('GT710_OWNERSHIP_REV')

        def source(path):
            if revision:
                return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=ROOT, text=True)
            return (ROOT / path).read_text()

        process = source('kernel/src/gpu/gk208/ce/process.rs')
        recovery = source('kernel/src/gpu/gk208/ce/recovery.rs')
        ce = source('kernel/src/gpu/gk208/ce.rs')
        gr = source('kernel/src/gpu/gk208/gr/mod.rs')
        helper = item(process, 'pub(in crate::gk208) fn test_ownership(') if 'fn test_ownership(' in process else ''
        # Keep production admission up to the first hardware/cleanup boundary.
        def preflight(text, marker, boundary):
            function = item(text, marker)
            return function[function.index('{') + 1:function.index(boundary)]

        recovery_guard = preflight(recovery, 'pub(super) fn probe(', '    let mut regs =')
        ce_guard = preflight(ce, 'pub fn probe(', '    let probe_start=')
        gr_guard = preflight(gr, 'pub(super) fn probe(', '    probe_static_inputs()?;')
        methods = '\n'.join(
            f'fn {name}(&mut self)->Result<(), &\'static str>{{' +
            preflight(process, f'    fn {name}(', boundary) + '\n hardware() }'
            for name, boundary in [('begin', '        let mut regs='),
                                   ('status', '        let Some(w)='),
                                   ('drain', '        let w=')])
        fixture = r'''
use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
static HARDWARE:AtomicUsize=AtomicUsize::new(0);
static FAIL_START:AtomicBool=AtomicBool::new(false);
fn hardware<T>()->Result<T,&'static str>{HARDWARE.fetch_add(1,Ordering::Relaxed);Err("injected hardware failure")}
mod spin {
 pub struct Mutex<T>(std::sync::Mutex<T>);
 impl<T> Mutex<T>{pub const fn new(v:T)->Self{Self(std::sync::Mutex::new(v))}
 pub fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
}
mod task {
 pub mod thread {pub enum ThreadState{BlockedOnIO}}
 pub mod waitqueue {
  pub struct WaitQueueHead;
  impl WaitQueueHead {
   pub const fn new()->Self{Self} pub fn wake_up(&self){}
   pub fn prepare_to_wait_checked(&self,_:super::thread::ThreadState,_:Option<u64>,_:impl FnOnce()->bool){}
  }
 }
 pub mod kthread {pub fn kthread_run(f:impl FnOnce(),_:&str)->Result<(),()>{f();Ok(())}}
}
mod syscall {pub mod blocking_io {
 pub fn wait_prepared(_:&crate::task::waitqueue::WaitQueueHead,_:())->Result<(),i32>{panic!("worker did not publish refusal")}
}}
mod drivers {pub mod pci {pub fn find_device(_:u16,_:u16)->Option<()>{Some(())}}}
mod gpu {pub fn install_native(_:())->Result<(),&'static str>{Ok(())}}
mod gk208 {
 pub mod gr {
  pub fn initialize(_: &mut super::ce::Registers,_:&(),_:&mut [u64;4],_:&u64)->Result<(),&'static str>{
   if crate::FAIL_START.load(crate::Ordering::Acquire){Err("startup failure")}else{Ok(())}
  }
  pub fn probe()->Result<(),&'static str>{ GR_GUARD crate::hardware() }
 }
 pub mod ce {
  pub struct Registers(pub usize);
  pub fn probe()->Result<(),&'static str>{ CE_GUARD crate::hardware() }
  pub mod recovery {
   pub(super) fn probe(s:&mut super::process::Session)->Result<String,&'static str>{
    RECOVERY_GUARD
    let _ = s; crate::hardware()
   }
  }
  pub mod process {
   use crate::{spin,hardware,AtomicBool,Ordering,task::{thread::ThreadState,waitqueue::WaitQueueHead}};
   use super::Registers;
   static SESSION:spin::Mutex<Option<Session>>=spin::Mutex::new(None);
   static CORE_STARTED:AtomicBool=AtomicBool::new(false);
   static RECOVERY_STARTED:AtomicBool=AtomicBool::new(false);
   static RECOVERY_READY:AtomicBool=AtomicBool::new(false);
   static RECOVERY_RESULT:spin::Mutex<Option<Result<String,&'static str>>>=spin::Mutex::new(None);
   static RECOVERY_WAITERS:WaitQueueHead=WaitQueueHead::new();
   pub(super) struct Session{registers:usize,layout:[u64;4],console:u64,CORE_FIELD work:Option<()>}
   HELPER
   INITIALIZE
   impl Session { METHODS }
   RECOVERY_READ
   pub fn run(){
    for fail in [false,true]{
     CORE_STARTED.store(false,Ordering::Release);
     RECOVERY_STARTED.store(false,Ordering::Release);RECOVERY_READY.store(false,Ordering::Release);
     *RECOVERY_RESULT.lock()=None;
     *SESSION.lock()=Some(Session{registers:0x1234,layout:[11,22,33,44],console:55,CORE_INIT work:None});
     crate::FAIL_START.store(fail,Ordering::Release);
     assert_eq!(initialize_core(),if fail{Err("startup failure")}else{Ok(())});
     crate::HARDWARE.store(0,Ordering::Relaxed);
     let expected=Err("native GPU core owns FIFO");
     assert_eq!(recovery_read().unwrap(),b"GT710_RECOVERY_ERROR native GPU core owns FIFO\n");
     assert_eq!(super::probe(),expected,"boot CE probe ignored core ownership");
     assert_eq!(super::super::gr::probe(),expected,"GR probe entered cleanup for core-owned state");
     let mut owner=SESSION.lock();let s=owner.as_mut().expect("worker lost session on early error");
     assert_eq!(s.begin(),expected);assert_eq!(s.status(),expected);assert_eq!(s.drain(),expected);
     assert_eq!((s.registers,s.layout,s.console,s.work),(0x1234,[11,22,33,44],55,None));
     drop(owner);
     assert_eq!(initialize_core(),Err("native core startup already attempted"));
     assert_eq!(crate::HARDWARE.load(Ordering::Relaxed),0,"refusal touched hardware or cleanup");
    }
    // Before handoff, the same recovery preflight still reaches hardware.
    CORE_STARTED.store(false,Ordering::Release);
    let mut s=SESSION.lock();let s=s.as_mut().unwrap(); RESET_FIELD
    assert_eq!(super::recovery::probe(s),Err("injected hardware failure"));
    assert_eq!(crate::HARDWARE.load(Ordering::Relaxed),1);
    assert_eq!(super::probe(),Err("injected hardware failure"));
    assert_eq!(super::super::gr::probe(),Err("injected hardware failure"));
    let r=Registers(0);assert_eq!(r.0,0);
   }
  }
 }
}
#[test]fn native_test_paths_preserve_core_ownership(){gk208::ce::process::run();}
'''
        replacements = {
            'HELPER': helper, 'INITIALIZE': item(process, 'pub(in crate::gk208) fn initialize_core('),
            'RECOVERY_READ': item(process, 'fn recovery_read('), 'METHODS': methods,
            'RECOVERY_GUARD': recovery_guard, 'CE_GUARD': ce_guard, 'GR_GUARD': gr_guard,
            'CORE_FIELD': 'core_started:bool,' if 'core_started:bool' in process else '',
            'CORE_INIT': 'core_started:false,' if 'core_started:bool' in process else '',
            'RESET_FIELD': 's.core_started=false;' if 'core_started:bool' in process else '',
        }
        for key, value in replacements.items():
            fixture = re.sub(r'\b' + key + r'\b', lambda _: value, fixture)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'ownership.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(path / 'ownership.rs'), '-o', str(path / 'ownership')], check=True)
            subprocess.run([str(path / 'ownership'), '--nocapture'], check=True, timeout=10)


if __name__ == '__main__':
    unittest.main()
