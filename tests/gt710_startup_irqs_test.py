"""Run production first-CAPS orchestration with stalled firmware IO on host threads.

The hardware substitute checks IRQ state at upload and wait boundaries. The
second caller must reach either startup or the core predicate before we release
the first, so an unscheduled host thread cannot masquerade as serialization.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class StartupIrqs(unittest.TestCase):
    def test_first_caps_interrupts_and_concurrent_publication(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        caps = item(source, 'fn op_caps(').replace('#[cfg(target_arch = "x86_64")]', '')
        startup = item(source, 'fn native_caps_startup(') if 'fn native_caps_startup(' in source else ''
        arch = (ROOT / 'kernel/src/gpu/arch.rs').read_text()
        enabled = item(arch, 'pub(super) fn with_interrupts') if 'pub(super) fn with_interrupts' in arch else ''
        fixture = r'''
use std::cell::Cell;
use std::sync::{Mutex,Condvar,atomic::{AtomicBool,AtomicU32,Ordering}};
use std::time::Duration;
const EINVAL:i64=22;const EINTR:i64=4;
mod abi {
 #[derive(Clone,Copy)] pub struct Caps {pub backend:u32,pub status:u32}
 pub mod status {pub const NO_DEVICE:u32=1;pub const DEVICE_FAILED:u32=3;}
}
struct Core {dev:Option<()>,status:u32,reason:&'static str}
impl Core {
 fn empty()->Self{Self{dev:None,status:1,reason:"absent"}}
 fn caps_record(&self)->abi::Caps{abi::Caps{backend:if self.dev.is_some(){6}else{0},status:self.status}}
}
static CORE:Mutex<Core>=Mutex::new(Core{dev:None,status:1,reason:"absent"});
static NATIVE_STARTING:AtomicBool=AtomicBool::new(false);
static CALLS:AtomicU32=AtomicU32::new(0);
static GATE:Mutex<(bool,bool,bool)>=Mutex::new((false,false,false));
static CHANGED:Condvar=Condvar::new();
static FAIL:AtomicBool=AtomicBool::new(false);
static INSTALL_EARLY:AtomicBool=AtomicBool::new(false);
thread_local!{static IRQS:Cell<bool>=const{Cell::new(false)};static SECOND:Cell<bool>=const{Cell::new(false)};}
pub fn arch_interrupts_enabled()->bool{IRQS.with(Cell::get)}
pub unsafe fn arch_enable_interrupts(){IRQS.with(|i|i.set(true));}
pub unsafe fn arch_disable_interrupts(){IRQS.with(|i|i.set(false));}
mod arch { ENABLED }
fn with_core<R>(f:impl FnOnce(&mut Core)->R)->Option<R>{
 let mut c=CORE.lock().unwrap();
 if SECOND.with(Cell::get){let mut g=GATE.lock().unwrap();g.2=true;CHANGED.notify_all();}
 Some(f(&mut c))
}
fn write_arg(_:u64,c:&abi::Caps)->Result<(),i64>{
 assert_eq!(c.status,if FAIL.load(Ordering::Acquire){3}else{2});
 assert_eq!(c.backend,if FAIL.load(Ordering::Acquire){0}else{6});Ok(())
}
fn notify(){CHANGED.notify_all();}
fn wait_for<T>(_:Option<u64>,_:bool,mut f:impl FnMut(&mut Core)->Option<T>)->Result<T,()> {
 loop {
  if let Some(v)=with_core(&mut f).unwrap(){return Ok(v);}
  let g=GATE.lock().unwrap();let _=CHANGED.wait_timeout(g,Duration::from_millis(1)).unwrap();
 }
}
mod gk208 {
 pub fn takeover_started()->bool{true}
 pub fn initialize_core()->Result<(),&'static str>{
  super::CALLS.fetch_add(1,super::Ordering::AcqRel);
  // Substitute hardware only: upload, Falcon startup/mailbox waits, channel receipts.
  for phase in ["FECS upload","GPCCS upload","firmware wait","channel wait"] {
   assert!(super::arch_interrupts_enabled(),"IRQs masked across {phase}");
  }
  if super::INSTALL_EARLY.load(super::Ordering::Acquire){super::with_core(|c|{c.dev=Some(());c.status=2;});}
  let mut g=super::GATE.lock().unwrap();g.0=true;super::CHANGED.notify_all();
  while !g.1{g=super::CHANGED.wait(g).unwrap();}drop(g);
  if super::FAIL.load(super::Ordering::Acquire){return Err("injected startup failure");}
  super::with_core(|c|{c.dev=Some(());c.status=2;});Ok(())
 }
}
STARTUP
// INSERT_CAPS
#[test]fn first_caps_enables_irqs_for_upload_and_waits_and_restores_on_errors(){
 for fail in [false,true] {
  *CORE.lock().unwrap()=Core::empty();*GATE.lock().unwrap()=(false,true,false);
  NATIVE_STARTING.store(false,Ordering::Release);FAIL.store(fail,Ordering::Release);
  for entry_enabled in [false,true] {
   IRQS.with(|i|i.set(entry_enabled));
   assert_eq!(op_caps(0,core::mem::size_of::<abi::Caps>() as u64),Ok(0));
   assert_eq!(arch_interrupts_enabled(),entry_enabled,"CAPS changed caller IRQ state");
  }
 }
}
#[test]fn second_first_caps_waits_for_the_same_result(){
 for early in [false,true]{
 INSTALL_EARLY.store(early,Ordering::Release);
 *CORE.lock().unwrap()=Core::empty();*GATE.lock().unwrap()=(false,false,false);
 NATIVE_STARTING.store(false,Ordering::Release);CALLS.store(0,Ordering::Release);FAIL.store(false,Ordering::Release);
 let a=std::thread::spawn(||{unsafe{arch_enable_interrupts();}op_caps(0,core::mem::size_of::<abi::Caps>() as u64)});
 let g=GATE.lock().unwrap();let(g,timeout)=CHANGED.wait_timeout_while(g,Duration::from_secs(2),|g|!g.0).unwrap();
 assert!(!timeout.timed_out(),"first caller never reached firmware");drop(g);
 let(tx,rx)=std::sync::mpsc::channel();
 let b=std::thread::spawn(move||{SECOND.with(|s|s.set(true));unsafe{arch_enable_interrupts();}tx.send(op_caps(0,core::mem::size_of::<abi::Caps>() as u64)).unwrap();});
 let g=GATE.lock().unwrap();let(mut g,timeout)=CHANGED.wait_timeout_while(g,Duration::from_secs(2),|g|!g.2).unwrap();
 assert!(!timeout.timed_out(),"second caller never inspected startup");
 assert!(rx.recv_timeout(Duration::from_millis(50)).is_err(),"second CAPS returned before startup publication");
 assert_eq!(CALLS.load(Ordering::Acquire),1,"concurrent first CAPS raced hardware startup");
 g.1=true;CHANGED.notify_all();drop(g);
 assert_eq!(a.join().unwrap(),Ok(0));assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(),Ok(0));b.join().unwrap();
 assert_eq!(CALLS.load(Ordering::Acquire),1);
 }
 INSTALL_EARLY.store(false,Ordering::Release);
}
'''
        # These harness hooks are used only after the repair adds serialization.
        fixture = fixture.replace('const EINTR:i64=4;', '#[allow(dead_code)] const EINTR:i64=4;')
        fixture = fixture.replace('fn notify()', '#[allow(dead_code)] fn notify()')
        fixture = fixture.replace('fn wait_for<T>', '#[allow(dead_code)] fn wait_for<T>')
        fixture = fixture.replace('ENABLED', enabled).replace('STARTUP', startup).replace('// INSERT_CAPS', caps)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'startup.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(path / 'startup.rs'), '-o', str(path / 'startup')], check=True)
            subprocess.run([str(path / 'startup'), '--test-threads=1'], check=True, timeout=15)


if __name__ == '__main__':
    unittest.main()
